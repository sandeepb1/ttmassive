//! Runs a flow. One loop picks the next node that has not finished and starts it, pausing first when
//! the user must approve it or a limit says stop, and sending work back when a judge rejects it.
//! The run sits behind a mutex that is never held across an await, so the page can read live progress.
//! Agents are behind `NodeRunner`, so every rule here is tested with fake ones.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::Outcome;
use super::graph::{EdgeKind, Node, Role};
use super::handoff::{self, Verdict};
use super::run::{NodeStatus, Pause, Run, RunStatus};

/// Whatever actually runs a node: the Claude or Codex driver in production, a script in tests.
pub trait NodeRunner {
    /// `log` receives short progress lines for the node's live transcript.
    async fn run(&self, node: &Node, prompt: &str, log: &mut (dyn FnMut(String) + Send)) -> anyhow::Result<Outcome>;
}

pub struct Control<'a> {
    pub cancelled: &'a AtomicBool,
    /// Called after every change to the run, to save it and tell the page.
    pub changed: &'a (dyn Fn(&Run) + Sync),
    /// A reason to stop before the next node starts, from the plan-limit windows. `None` to go on.
    pub limit: &'a (dyn Fn(&Run) -> Option<String> + Sync),
}

fn edit<T>(shared: &Mutex<Run>, ctl: &Control, f: impl FnOnce(&mut Run) -> T) -> T {
    let mut run = shared.lock().unwrap_or_else(|e| e.into_inner());
    let out = f(&mut run);
    (ctl.changed)(&run);
    out
}

/// Node ids in the order they can run.
fn order(run: &Run) -> Vec<String> {
    run.flow.layers().unwrap_or_default().into_iter().flatten().collect()
}

enum Step {
    Finish,
    Pause,
    Start { node: Node, prompt: String },
}

fn plan(run: &mut Run, limit: &dyn Fn(&Run) -> Option<String>) -> Step {
    let next = order(run).into_iter().find(|id| run.nodes[id].status != NodeStatus::Done);
    let Some(id) = next else {
        run.status = RunStatus::Done;
        return Step::Finish;
    };
    let node = run.flow.node(&id).expect("ordered ids come from the flow").clone();
    if node.checkpoint && !run.approved.contains(&id) {
        run.status = RunStatus::Paused { pause: Pause::Approval { node: id } };
        return Step::Pause;
    }
    if let Some(message) = limit(run) {
        run.status = RunStatus::Paused { pause: Pause::Budget { message } };
        return Step::Pause;
    }
    let prompt = handoff::build_prompt(run, &node);
    let n = run.nodes.get_mut(&id).expect("every node has a state");
    n.status = NodeStatus::Running;
    n.attempts += 1;
    n.error = None;
    run.status = RunStatus::Running;
    Step::Start { node, prompt }
}

/// Record how a node ended. Returns true when the run should stop here.
fn finish(run: &mut Run, node: &Node, result: anyhow::Result<Outcome>) -> bool {
    let fail = |run: &mut Run, message: String| {
        let n = run.nodes.get_mut(&node.id).expect("every node has a state");
        n.status = NodeStatus::Failed;
        n.error = Some(message.clone());
        run.status = RunStatus::Paused { pause: Pause::Failed { node: node.id.clone(), message } };
        true
    };
    let outcome = match result {
        Ok(o) => o,
        Err(e) => return fail(run, e.to_string()),
    };
    let n = run.nodes.get_mut(&node.id).expect("every node has a state");
    n.tokens.add(&outcome.tokens);
    if outcome.is_error {
        return fail(run, format!("the agent reported an error: {}", outcome.text.chars().take(300).collect::<String>()));
    }
    let Some(output) = outcome.structured else {
        return fail(run, "the agent did not give an answer in the required shape".into());
    };
    n.status = NodeStatus::Done;
    n.output = Some(output.clone());
    run.feedback.remove(&node.id);

    if let Some(cap) = node.token_cap {
        let spent = run.nodes[&node.id].tokens.spent();
        if spent > cap {
            let message = format!("{} used {spent} tokens, over its cap of {cap}", node.name);
            run.status = RunStatus::Paused { pause: Pause::Budget { message } };
            return true;
        }
    }
    if node.role == Role::Judge {
        return judged(run, node, &output);
    }
    false
}

/// A judge has answered: either carry on, or send work back to the builder it points at.
fn judged(run: &mut Run, judge: &Node, output: &serde_json::Value) -> bool {
    let Verdict::Reject(feedback) = handoff::verdict(output) else { return false };
    let Some(target) = run
        .flow
        .edges
        .iter()
        .find(|e| e.kind == EdgeKind::Retry && e.from == judge.id)
        .map(|e| e.to.clone())
    else {
        return false; // no way back drawn: the judge is advisory
    };
    if run.retries_used >= run.flow.max_retries {
        run.status = RunStatus::Paused { pause: Pause::RetriesExhausted { node: judge.id.clone() } };
        return true;
    }
    run.retries_used += 1;
    run.feedback.insert(target.clone(), feedback);
    let ids = order(run);
    let (from, to) = (ids.iter().position(|i| *i == target), ids.iter().position(|i| *i == judge.id));
    if let (Some(from), Some(to)) = (from, to) {
        for id in &ids[from..=to] {
            let n = run.nodes.get_mut(id).expect("every node has a state");
            n.status = NodeStatus::Pending;
            n.output = None;
            n.error = None;
        }
    }
    false
}

/// Run until the flow is done or something needs the user. Call again after approving or resuming.
pub async fn drive<R: NodeRunner>(shared: &Mutex<Run>, runner: &R, ctl: &Control<'_>) {
    loop {
        if ctl.cancelled.load(Ordering::Relaxed) {
            edit(shared, ctl, |r| r.status = RunStatus::Cancelled);
            return;
        }
        let step = edit(shared, ctl, |r| plan(r, ctl.limit));
        let (node, prompt) = match step {
            Step::Finish | Step::Pause => return,
            Step::Start { node, prompt } => (node, prompt),
        };
        let result = runner
            .run(&node, &prompt, &mut |line| {
                edit(shared, ctl, |r| {
                    if let Some(n) = r.nodes.get_mut(&node.id) {
                        n.log(line);
                    }
                })
            })
            .await;
        if edit(shared, ctl, |r| finish(r, &node, result)) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};

    use serde_json::json;

    use super::*;
    use crate::orch::graph::{Budget, Edge, Engine, Flow};
    use crate::orch::usage::Tokens;

    fn node(id: &str, role: Role) -> Node {
        Node {
            id: id.into(),
            name: id.into(),
            role,
            engine: Engine::Claude,
            model: "sonnet".into(),
            system_prompt: String::new(),
            checkpoint: false,
            token_cap: None,
            x: 0.0,
            y: 0.0,
        }
    }

    fn edge(id: &str, from: &str, to: &str, kind: EdgeKind) -> Edge {
        Edge { id: id.into(), from: from.into(), to: to.into(), kind }
    }

    /// orch -> res -> build -> judge, with the judge able to send work back to build.
    fn flow() -> Flow {
        Flow {
            id: "f".into(),
            name: "F".into(),
            nodes: vec![
                node("orch", Role::Orchestrator),
                node("res", Role::Researcher),
                node("build", Role::Builder),
                node("judge", Role::Judge),
            ],
            edges: vec![
                edge("e1", "orch", "res", EdgeKind::Forward),
                edge("e2", "res", "build", EdgeKind::Forward),
                edge("e3", "build", "judge", EdgeKind::Forward),
                edge("e4", "judge", "build", EdgeKind::Retry),
            ],
            max_retries: 2,
            budget: Budget::default(),
        }
    }

    fn ok(v: serde_json::Value) -> anyhow::Result<Outcome> {
        Ok(Outcome {
            structured: Some(v),
            text: String::new(),
            tokens: Tokens { input: 10, output: 10, cache_read: 0, cache_write: 0 },
            cost_usd: 0.0,
            is_error: false,
        })
    }

    fn approve() -> anyhow::Result<Outcome> {
        ok(json!({"verdict": "approve", "feedback": ""}))
    }

    fn reject(why: &str) -> anyhow::Result<Outcome> {
        ok(json!({"verdict": "reject", "feedback": why}))
    }

    /// Plays back scripted outcomes per node and remembers every prompt it was given.
    #[derive(Default)]
    struct Fake {
        script: Mutex<HashMap<String, VecDeque<anyhow::Result<Outcome>>>>,
        prompts: Mutex<Vec<(String, String)>>,
    }

    impl Fake {
        fn with(items: Vec<(&str, anyhow::Result<Outcome>)>) -> Fake {
            let f = Fake::default();
            for (id, r) in items {
                f.script.lock().unwrap().entry(id.into()).or_default().push_back(r);
            }
            f
        }

        fn ran(&self) -> Vec<String> {
            self.prompts.lock().unwrap().iter().map(|(id, _)| id.clone()).collect()
        }

        fn prompt(&self, id: &str, nth: usize) -> String {
            self.prompts.lock().unwrap().iter().filter(|(i, _)| i == id).nth(nth).unwrap().1.clone()
        }
    }

    impl NodeRunner for Fake {
        async fn run(&self, node: &Node, prompt: &str, log: &mut (dyn FnMut(String) + Send)) -> anyhow::Result<Outcome> {
            self.prompts.lock().unwrap().push((node.id.clone(), prompt.to_string()));
            log(format!("working on {}", node.id));
            let next = self.script.lock().unwrap().get_mut(&node.id).and_then(|q| q.pop_front());
            next.unwrap_or_else(|| ok(json!({"summary": format!("{} done", node.id)})))
        }
    }

    async fn go(run: &Mutex<Run>, fake: &Fake) {
        go_limited(run, fake, &|_| None).await
    }

    async fn go_limited(run: &Mutex<Run>, fake: &Fake, limit: &(dyn Fn(&Run) -> Option<String> + Sync)) {
        let cancelled = AtomicBool::new(false);
        drive(run, fake, &Control { cancelled: &cancelled, changed: &|_| {}, limit }).await;
    }

    fn new_run(f: Flow) -> Mutex<Run> {
        Mutex::new(Run::new("r".into(), f, "Build a thing".into(), 0))
    }

    fn status(run: &Mutex<Run>) -> RunStatus {
        run.lock().unwrap().status.clone()
    }

    #[tokio::test]
    async fn a_line_runs_in_order_and_hands_each_node_its_upstream_output() {
        let run = new_run(flow());
        let fake = Fake::with(vec![
            ("orch", ok(json!({"plan": "Do it in two steps", "steps": []}))),
            ("judge", approve()),
        ]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done);
        assert_eq!(fake.ran(), vec!["orch", "res", "build", "judge"]);
        assert!(fake.prompt("res", 0).contains("Do it in two steps"));
        assert!(!fake.prompt("orch", 0).contains("# Inputs"));
        // A node is not shown anything from nodes that are not wired into it.
        assert!(!fake.prompt("build", 0).contains("Do it in two steps"));
        assert!(run.lock().unwrap().nodes["res"].transcript.contains(&"working on res".to_string()));
    }

    #[tokio::test]
    async fn a_checkpoint_pauses_before_the_node_until_it_is_approved() {
        let mut f = flow();
        f.nodes[2].checkpoint = true;
        let run = new_run(f);
        let fake = Fake::with(vec![("judge", approve())]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Paused { pause: Pause::Approval { node: "build".into() } });
        assert_eq!(fake.ran(), vec!["orch", "res"]);

        run.lock().unwrap().approved.insert("build".into());
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done);
        assert_eq!(fake.ran(), vec!["orch", "res", "build", "judge"]);
    }

    #[tokio::test]
    async fn a_rejection_sends_work_back_with_the_feedback_then_passes() {
        let run = new_run(flow());
        let fake = Fake::with(vec![("judge", reject("No tests")), ("judge", approve())]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done);
        // Only the builder and the judge repeat; research is not redone.
        assert_eq!(fake.ran(), vec!["orch", "res", "build", "judge", "build", "judge"]);
        assert!(fake.prompt("build", 1).contains("No tests"));
        assert!(!fake.prompt("build", 0).contains("No tests"));
        let r = run.lock().unwrap();
        assert_eq!(r.retries_used, 1);
        assert_eq!(r.nodes["build"].attempts, 2);
        assert!(r.feedback.is_empty(), "feedback is used up once the builder has seen it");
    }

    #[tokio::test]
    async fn too_many_rejections_pause_the_run_instead_of_looping() {
        let run = new_run(flow());
        let fake = Fake::with(vec![
            ("judge", reject("a")),
            ("judge", reject("b")),
            ("judge", reject("c")),
            ("judge", approve()),
        ]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Paused { pause: Pause::RetriesExhausted { node: "judge".into() } });
        assert_eq!(fake.ran().iter().filter(|i| *i == "build").count(), 3, "first try plus two retries");
    }

    #[tokio::test]
    async fn a_judge_with_no_way_back_is_advisory() {
        let mut f = flow();
        f.edges.pop();
        let run = new_run(f);
        let fake = Fake::with(vec![("judge", reject("meh"))]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done);
    }

    #[tokio::test]
    async fn a_node_over_its_token_cap_pauses_the_run_after_it_finishes() {
        let mut f = flow();
        f.nodes[1].token_cap = Some(15); // each fake turn spends 20
        let run = new_run(f);
        let fake = Fake::default();
        go(&run, &fake).await;
        let RunStatus::Paused { pause: Pause::Budget { message } } = status(&run) else { panic!("not paused") };
        assert!(message.contains("over its cap of 15"), "{message}");
        assert_eq!(run.lock().unwrap().nodes["res"].status, NodeStatus::Done, "its work is kept");
        assert_eq!(fake.ran(), vec!["orch", "res"]);
    }

    #[tokio::test]
    async fn the_plan_limit_gate_stops_the_run_before_the_next_node() {
        let run = new_run(flow());
        let fake = Fake::default();
        let limit = |r: &Run| {
            let done = r.nodes.values().filter(|n| n.status == NodeStatus::Done).count();
            (done >= 2).then(|| "Claude 5-hour window is at 86%".to_string())
        };
        go_limited(&run, &fake, &limit).await;
        assert_eq!(fake.ran(), vec!["orch", "res"]);
        assert_eq!(
            status(&run),
            RunStatus::Paused { pause: Pause::Budget { message: "Claude 5-hour window is at 86%".into() } }
        );
    }

    #[tokio::test]
    async fn a_failed_node_pauses_and_resuming_reruns_only_that_node() {
        let run = new_run(flow());
        let fake = Fake::with(vec![("res", Err(anyhow::anyhow!("claude exited"))), ("judge", approve())]);
        go(&run, &fake).await;
        assert_eq!(
            status(&run),
            RunStatus::Paused { pause: Pause::Failed { node: "res".into(), message: "claude exited".into() } }
        );
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done);
        assert_eq!(fake.ran(), vec!["orch", "res", "res", "build", "judge"]);
        assert_eq!(run.lock().unwrap().nodes["res"].attempts, 2);
    }

    #[tokio::test]
    async fn an_answer_in_the_wrong_shape_counts_as_a_failure() {
        let run = new_run(flow());
        let none = Ok(Outcome {
            structured: None,
            text: "I could not".into(),
            tokens: Tokens::default(),
            cost_usd: 0.0,
            is_error: false,
        });
        let fake = Fake::with(vec![("orch", none)]);
        go(&run, &fake).await;
        let RunStatus::Paused { pause: Pause::Failed { node, .. } } = status(&run) else { panic!("not failed") };
        assert_eq!(node, "orch");
    }

    #[tokio::test]
    async fn finished_nodes_are_not_run_again_after_a_restart() {
        let run = new_run(flow());
        let fake = Fake::with(vec![("build", Err(anyhow::anyhow!("boom"))), ("judge", approve())]);
        go(&run, &fake).await;
        // Simulate ttm restarting while the builder was mid-run.
        {
            let mut r = run.lock().unwrap();
            r.nodes.get_mut("build").unwrap().status = NodeStatus::Running;
            r.mark_interrupted();
            assert_eq!(r.nodes["build"].status, NodeStatus::Interrupted);
        }
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done);
        assert_eq!(fake.ran(), vec!["orch", "res", "build", "build", "judge"]);
    }

    #[tokio::test]
    async fn cancelling_stops_before_the_next_node() {
        let run = new_run(flow());
        let fake = Fake::default();
        let cancelled = AtomicBool::new(true);
        drive(&run, &fake, &Control { cancelled: &cancelled, changed: &|_| {}, limit: &|_| None }).await;
        assert_eq!(status(&run), RunStatus::Cancelled);
        assert!(fake.ran().is_empty());
    }

    #[tokio::test]
    async fn every_change_is_reported_so_it_can_be_saved() {
        let run = new_run(flow());
        let fake = Fake::default();
        let count = std::sync::atomic::AtomicUsize::new(0);
        let cancelled = AtomicBool::new(false);
        let changed = |_: &Run| {
            count.fetch_add(1, Ordering::Relaxed);
        };
        drive(&run, &fake, &Control { cancelled: &cancelled, changed: &changed, limit: &|_| None }).await;
        // At least a start and a finish for each of the four nodes.
        assert!(count.load(Ordering::Relaxed) >= 8);
    }
}
