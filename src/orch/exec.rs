//! Runs a flow. Every agent whose inputs are done starts at once (so independent branches run in
//! parallel), pausing first when the user must approve it or a limit says stop, and sending work back
//! when a judge rejects it.
//! The run sits behind a mutex that is never held across an await, so the page can read live progress.
//! Agents are behind `NodeRunner`, so every rule here is tested with fake ones.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;

use super::Outcome;
use super::downshift;
use super::graph::{Engine, Node};
use super::handoff::{self, Verdict};
use super::graph::{Answerer, Edge, OnCap};
use super::run::{Asked, FeedbackKind, LogKind, NodeStatus, Pause, PendingQuestions, Run, RunStatus, SKILL_PREFIX, SUBAGENT_PREFIX, now_secs};

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
    /// How close the run is to stopping on an engine's plan windows: 0 is far, 1 is a limit reached.
    pub pressure: &'a (dyn Fn(&Run, Engine) -> f64 + Sync),
}

fn edit<T>(shared: &Mutex<Run>, ctl: &Control, f: impl FnOnce(&mut Run) -> T) -> T {
    let mut run = shared.lock().unwrap_or_else(|e| e.into_inner());
    let out = f(&mut run);
    (ctl.changed)(&run);
    out
}

fn name_of(run: &Run, id: &str) -> String {
    run.flow.node(id).map(|n| n.name.clone()).unwrap_or_else(|| id.to_string())
}

/// "A", "A and B", "A, B and C".
fn list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// Node ids in the order they can run.
fn order(run: &Run) -> Vec<String> {
    run.flow.layers().unwrap_or_default().into_iter().flatten().collect()
}

/// Everything that can start now: each agent whose inputs are all done, and that is not waiting on
/// the user. Marks them running. Also says why starting should stop (a plan limit) and which agent,
/// if any, is held back for approval.
struct Ready {
    starts: Vec<(Node, String)>,
    halt: Option<Pause>,
    waiting: Option<String>,
}

fn start_ready(run: &mut Run, limit: &dyn Fn(&Run) -> Option<String>, pressure: &dyn Fn(&Run, Engine) -> f64) -> Ready {
    let mut out = Ready { starts: Vec::new(), halt: None, waiting: None };
    for id in order(run) {
        if matches!(run.nodes[&id].status, NodeStatus::Done | NodeStatus::Running) {
            continue;
        }
        if !run.flow.inputs_of(&id).iter().all(|i| run.nodes[*i].status == NodeStatus::Done) {
            continue;
        }
        let mut node = run.flow.node(&id).expect("ordered ids come from the flow").clone();
        if node.checkpoint && !run.approved.contains(&id) {
            out.waiting.get_or_insert(id);
            continue;
        }
        if let Some(message) = limit(run) {
            out.halt = Some(Pause::Budget { message });
            break;
        }
        // The model is decided when an agent first starts and then kept, so a send-back carries on in the same
        // conversation with its cache warm instead of starting over on another model.
        let model = match run.nodes[&id].model_used.clone() {
            Some(m) => m,
            None if run.flow.downshift && !node.keep_model => {
                downshift::choose(node.engine, &node.model, pressure(run, node.engine)).unwrap_or_else(|| node.model.clone())
            }
            None => node.model.clone(),
        };
        let note = (model != node.model).then(|| format!("Using {model} instead of {}: the plan is filling up", node.model));
        node.model = model;
        let prompt = handoff::build_prompt(run, &node);
        let inputs: Vec<String> = run.flow.inputs_of(&id).iter().map(|i| name_of(run, i)).collect();
        let why = if run.feedback.contains_key(&id) {
            format!("{} starts again with the feedback it was sent back with", node.name)
        } else if inputs.is_empty() {
            format!("{} starts first: nothing feeds it", node.name)
        } else {
            format!("{} starts: {} finished", node.name, list(&inputs))
        };
        let n = run.nodes.get_mut(&id).expect("every node has a state");
        let first_time = n.model_used.is_none();
        n.model_used = Some(node.model.clone());
        n.status = NodeStatus::Running;
        n.attempts += 1;
        n.error = None;
        let attempt = n.attempts;
        if let Some(note) = note.filter(|_| first_time) {
            n.log(note.clone());
            run.note(Some(&id), LogKind::Decision, note, None);
        }
        let engine = match (node.direct, node.engine) {
            (true, _) => "direct call to",
            (_, Engine::Claude) => "Claude",
            (_, Engine::Codex) => "Codex",
        };
        run.note(Some(&id), LogKind::Decision, why, None);
        run.note(Some(&id), LogKind::Request, format!("→ {} · attempt {attempt} · {engine} {}", node.name, node.model), Some(prompt.clone()));
        out.starts.push((node, prompt));
    }
    if !out.starts.is_empty() {
        run.status = RunStatus::Running;
    }
    out
}

/// Nothing is running and nothing more will start: the run is done, or it needs the user.
fn settle(run: &mut Run, halt: Option<Pause>, waiting: Option<String>) {
    settle_status(run, halt, waiting);
    let line = match run.status.clone() {
        RunStatus::Done => "Every agent has finished: the run is done".to_string(),
        RunStatus::Paused { pause } => match pause {
            Pause::Approval { node } => format!("Waiting for your approval before {} starts", name_of(run, &node)),
            Pause::RetriesExhausted { node } => format!("{} has sent work back as many times as the flow allows, so the run stopped for you to decide", name_of(run, &node)),
            Pause::Budget { message } => format!("Stopped: {message}"),
            Pause::Failed { node, message } if node.is_empty() => format!("Stopped: {message}"),
            Pause::Failed { node, message } => format!("Stopped: {} failed ({message})", name_of(run, &node)),
            Pause::Questions { node } => format!("{} has questions for you; the run is waiting for your answers", name_of(run, &node)),
        },
        _ => return,
    };
    run.note(None, LogKind::Decision, line, None);
}

fn settle_status(run: &mut Run, halt: Option<Pause>, waiting: Option<String>) {
    run.status = match (halt, waiting) {
        (Some(pause), _) => RunStatus::Paused { pause },
        (None, _) if run.nodes.values().all(|n| n.status == NodeStatus::Done) => {
            run.finished = Some(now_secs());
            RunStatus::Done
        }
        (None, Some(node)) => RunStatus::Paused { pause: Pause::Approval { node } },
        (None, None) => RunStatus::Paused {
            pause: Pause::Failed { node: String::new(), message: "no agent is able to start".into() },
        },
    };
}

/// A turn that got under this share of its input from the cache, after one that got at least `WAS`, is a break.
const BROKE_BELOW: f64 = 0.2;
const WAS_AT_LEAST: f64 = 0.6;
/// Below this much input, a low share means little: a small prompt has little to cache.
const MIN_INPUT: u64 = 2000;

/// Notice when an agent's cache stopped working between two turns, and say so in its transcript.
fn watch_cache(n: &mut super::run::NodeRun, turn: &super::usage::Tokens) {
    let Some(now) = turn.cache_hit() else { return };
    let input = turn.input + turn.cache_read + turn.cache_write;
    if let Some(was) = n.last_hit {
        if was >= WAS_AT_LEAST && now < BROKE_BELOW && input >= MIN_INPUT {
            n.cache_breaks += 1;
            n.log(format!("Cache break: only {:.0}% of the input came from the cache, after {:.0}% last turn", now * 100.0, was * 100.0));
        }
    }
    n.last_hit = Some(now);
}

/// Record how a node ended. Returns a reason to stop starting new agents, if there is one.
fn finish(run: &mut Run, node: &Node, result: anyhow::Result<Outcome>) -> Option<Pause> {
    let fail = |run: &mut Run, message: String| {
        let n = run.nodes.get_mut(&node.id).expect("every node has a state");
        n.status = NodeStatus::Failed;
        n.error = Some(message.clone());
        run.note(Some(&node.id), LogKind::Error, format!("✕ {} failed", node.name), Some(message.clone()));
        Some(Pause::Failed { node: node.id.clone(), message })
    };
    let outcome = match result {
        Ok(o) => o,
        Err(e) => return fail(run, e.to_string()),
    };
    let n = run.nodes.get_mut(&node.id).expect("every node has a state");
    n.tokens.add(&outcome.tokens);
    n.cost_usd += outcome.cost_usd;
    n.credits += outcome.credits;
    watch_cache(n, &outcome.tokens);
    if let Some(id) = &outcome.session {
        n.session = Some(id.clone());
    }
    if outcome.is_error {
        return fail(run, format!("the agent reported an error: {}", outcome.text.chars().take(300).collect::<String>()));
    }
    let Some(output) = outcome.structured else {
        return fail(run, "the agent did not give an answer in the required shape".into());
    };
    n.status = NodeStatus::Done;
    n.output = Some(output.clone());
    let t = outcome.tokens;
    let shown = serde_json::to_string_pretty(&output).unwrap_or_default();
    run.feedback.remove(&node.id);
    run.note(
        Some(&node.id),
        LogKind::Response,
        format!("← {} answered", node.name),
        Some(format!("{} in · {} out · {} read from cache · {} written to cache\n\n{shown}", t.input, t.output, t.cache_read, t.cache_write)),
    );

    if let Some(cap) = node.token_cap {
        let spent = run.nodes[&node.id].tokens.spent();
        if spent > cap {
            let message = format!("{} used {spent} tokens, over its cap of {cap}", node.name);
            return Some(Pause::Budget { message });
        }
    }
    if node.decides() {
        return judged(run, node, &output);
    }
    if node.asks() {
        return questioned(run, node, &output);
    }
    None
}

/// A judge has answered: either carry on, or send work back to the builder it points at.
fn judged(run: &mut Run, judge: &Node, output: &serde_json::Value) -> Option<Pause> {
    match handoff::verdict(output) {
        Verdict::Approve => {
            run.note(Some(&judge.id), LogKind::Decision, format!("{} approved the work: carrying on", judge.name), None);
            None
        }
        Verdict::Reject { feedback, send_back_to } => send_back(run, judge, feedback, send_back_to, Why::Rejected),
    }
}

/// An agent that asks questions has answered: either it has none and the flow carries on, or the work goes back
/// along its send-back edge and the questions go with it.
fn questioned(run: &mut Run, asker: &Node, output: &serde_json::Value) -> Option<Pause> {
    let questions = handoff::open_questions(output);
    if questions.is_empty() {
        run.note(Some(&asker.id), LogKind::Decision, format!("{} has no questions left: carrying on", asker.name), None);
        return None;
    }
    send_back(run, asker, questions.join("\n"), None, Why::Asked(questions))
}

/// Why work is being sent back.
enum Why {
    Rejected,
    Asked(Vec<String>),
}

/// Send work back from `from` along its send-back edge, or say why not. Each edge may be used only so many times, and
/// the flow only so many sends-back in all; when an edge is used up, its own setting says what happens.
fn send_back(run: &mut Run, from: &Node, feedback: String, send_back_to: Option<String>, why: Why) -> Option<Pause> {
    let targets: Vec<String> = run.flow.send_back_targets(&from.id).into_iter().map(String::from).collect();
    if targets.is_empty() {
        // No way back drawn: it is advisory.
        let what = match why {
            Why::Rejected => "rejected the work, but no send-back is drawn from it",
            Why::Asked(_) => "asked questions, but no send-back is drawn from it",
        };
        run.note(Some(&from.id), LogKind::Decision, format!("{} {what}, so the flow carries on", from.name), Some(feedback));
        return None;
    }
    // One place to go needs no answer. With several, the verdict must name one of them.
    let target = match (targets.as_slice(), send_back_to) {
        ([only], _) => only.clone(),
        (_, Some(t)) if targets.contains(&t) => t,
        _ => {
            let message = "it rejected the work but did not say which agent should redo it".to_string();
            let n = run.nodes.get_mut(&from.id).expect("every node has a state");
            n.status = NodeStatus::Failed;
            n.output = None;
            n.error = Some(message.clone());
            run.note(Some(&from.id), LogKind::Error, format!("✕ {} did not say where to send the work", from.name), Some(message.clone()));
            return Some(Pause::Failed { node: from.id.clone(), message });
        }
    };
    let edge: Edge = run.flow.edges.iter().find(|e| e.kind == super::graph::EdgeKind::Retry && e.from == from.id && e.to == target).cloned().expect("a target comes from an edge");
    let used = run.iterations.get(&edge.id).copied().unwrap_or(0);
    // The flow's own limit is a backstop over every edge together: reaching it always stops for the user.
    if run.retries_used >= run.flow.max_retries {
        run.note(Some(&from.id), LogKind::Decision, format!("The flow allows {} send-backs in all and they have been used", run.flow.max_retries), None);
        return Some(Pause::RetriesExhausted { node: from.id.clone() });
    }
    if used >= edge.max_iterations {
        return edge_used_up(run, from, &target, &edge);
    }
    *run.iterations.entry(edge.id.clone()).or_insert(0) += 1;
    run.retries_used += 1;
    match why {
        Why::Rejected => {
            apply_loop(run, from, &target, feedback, FeedbackKind::Judge);
            None
        }
        Why::Asked(questions) => {
            run.asked.entry(from.id.clone()).or_default().extend(questions.iter().map(|q| Asked { question: q.clone(), answer: None }));
            if edge.answered_by == Answerer::User {
                run.note(Some(&from.id), LogKind::Decision, format!("{} asked {} question{}; waiting for your answers", from.name, questions.len(), if questions.len() == 1 { "" } else { "s" }), Some(feedback));
                run.pending = Some(PendingQuestions { node: from.id.clone(), target, edge: edge.id, questions });
                Some(Pause::Questions { node: from.id.clone() })
            } else {
                let pairs: Vec<_> = questions.iter().map(|q| (q.clone(), None)).collect();
                apply_loop(run, from, &target, handoff::qa_text(&pairs), FeedbackKind::Questions);
                None
            }
        }
    }
}

/// An edge has been used as many times as it may be.
fn edge_used_up(run: &mut Run, from: &Node, target: &str, edge: &Edge) -> Option<Pause> {
    let line = format!("{} wants to send work back to {} again, but that edge has been used {} time{}", from.name, name_of(run, target), edge.max_iterations, if edge.max_iterations == 1 { "" } else { "s" });
    match edge.on_cap {
        OnCap::Pause => {
            run.note(Some(&from.id), LogKind::Decision, format!("{line}: stopping for you to decide"), None);
            Some(Pause::RetriesExhausted { node: from.id.clone() })
        }
        OnCap::Proceed => {
            run.note(Some(&from.id), LogKind::Decision, format!("{line}: carrying on with the work as it is"), None);
            None
        }
        OnCap::Fail => {
            let message = format!("work was sent back along an edge as many times as it may be ({})", edge.max_iterations);
            let n = run.nodes.get_mut(&from.id).expect("every node has a state");
            n.status = NodeStatus::Failed;
            n.error = Some(message.clone());
            run.note(Some(&from.id), LogKind::Error, format!("✕ {line}"), Some(message.clone()));
            Some(Pause::Failed { node: from.id.clone(), message })
        }
    }
}

/// Send the work back: the target gets `feedback`, and what lies between it and `from` runs again.
fn apply_loop(run: &mut Run, from: &Node, target: &str, feedback: String, kind: FeedbackKind) {
    run.feedback.insert(target.to_string(), feedback);
    run.feedback_kind.insert(target.to_string(), kind);
    // Redo what lies between the target and the one that sent it back, and nothing else: a branch running beside them keeps its work.
    let redo: Vec<String> = run.flow.between(target, &from.id).into_iter().map(String::from).collect();
    let (used, max) = (run.retries_used, run.flow.max_retries);
    let again: Vec<String> = redo.iter().map(|i| name_of(run, i)).collect();
    let what = match kind {
        FeedbackKind::Judge => "rejected the work and sent it back",
        FeedbackKind::Answers => "had its questions answered and sent the work back",
        FeedbackKind::Questions => "asked questions and sent the work back",
    };
    run.note(
        Some(&from.id),
        LogKind::Decision,
        format!("{} {what} to {} (send-back {used} of {max}); {} run again", from.name, name_of(run, target), list(&again)),
        Some(feedback_for_log(run, target)),
    );
    for id in redo {
        let n = run.nodes.get_mut(&id).expect("every node has a state");
        n.status = NodeStatus::Pending;
        n.output = None;
        n.error = None;
    }
}

/// The user's answers to the questions the run is waiting on. The work then goes back as it was going to, with the
/// answers, and the run is ready to be started again.
pub fn answer_questions(run: &mut Run, answers: Vec<String>) -> Result<(), String> {
    let Some(p) = run.pending.clone() else { return Err("the run is not waiting for answers".into()) };
    if answers.len() != p.questions.len() {
        return Err(format!("there are {} questions and {} answers", p.questions.len(), answers.len()));
    }
    let answers: Vec<String> = answers.into_iter().map(|a| a.trim().chars().take(4000).collect()).collect();
    let pairs: Vec<(String, Option<String>)> = p.questions.iter().cloned().zip(answers.iter().cloned().map(Some)).collect();
    // Remember the answers, against the questions they belong to (the last ones this agent asked).
    if let Some(list) = run.asked.get_mut(&p.node) {
        let from = list.len().saturating_sub(p.questions.len());
        for (slot, a) in list[from..].iter_mut().zip(&answers) {
            slot.answer = Some(a.clone());
        }
    }
    let asker = run.flow.node(&p.node).cloned().ok_or("the agent that asked is gone")?;
    run.pending = None;
    run.note(Some(&p.node), LogKind::Decision, "You answered the questions", Some(handoff::qa_text(&pairs)));
    apply_loop(run, &asker, &p.target, handoff::qa_text(&pairs), FeedbackKind::Answers);
    run.status = RunStatus::Running;
    Ok(())
}

fn feedback_for_log(run: &Run, target: &str) -> String {
    run.feedback.get(target).cloned().unwrap_or_default()
}

async fn run_node<R: NodeRunner>(shared: &Mutex<Run>, runner: &R, ctl: &Control<'_>, node: Node, prompt: String) -> (Node, anyhow::Result<Outcome>) {
    let result = runner
        .run(&node, &prompt, &mut |line| {
            edit(shared, ctl, |r| {
                // Skills and sub-agents are worth a line of their own in the run's log, not only the node's transcript.
                if let Some(what) = line.strip_prefix(SKILL_PREFIX) {
                    r.note(Some(&node.id), LogKind::Skill, format!("{} used the skill {what}", node.name), None);
                } else if let Some(what) = line.strip_prefix(SUBAGENT_PREFIX) {
                    r.note(Some(&node.id), LogKind::Subagent, format!("{} started a sub-agent: {what}", node.name), None);
                }
                if let Some(n) = r.nodes.get_mut(&node.id) {
                    n.log(line);
                }
            })
        })
        .await;
    (node, result)
}

/// Run until the flow is done or something needs the user. Call again after approving or resuming.
///
/// Every agent whose inputs are done runs at once, so branches that do not depend on each other go in
/// parallel. If an agent fails, or a limit is reached, nothing new starts but the ones running finish
/// their work; the run pauses when none are left.
pub async fn drive<R: NodeRunner>(shared: &Mutex<Run>, runner: &R, ctl: &Control<'_>) {
    let mut running = FuturesUnordered::new();
    let mut halt: Option<Pause> = None;
    loop {
        if ctl.cancelled.load(Ordering::Relaxed) {
            // Returning drops the futures in `running`, which stops their agents.
            edit(shared, ctl, |r| {
                for n in r.nodes.values_mut().filter(|n| n.status == NodeStatus::Running) {
                    n.status = NodeStatus::Interrupted;
                }
                r.status = RunStatus::Cancelled;
                r.finished = Some(now_secs());
                r.note(None, LogKind::Event, "The run was stopped", None);
            });
            return;
        }
        let mut waiting = None;
        if halt.is_none() {
            let ready = edit(shared, ctl, |r| start_ready(r, ctl.limit, ctl.pressure));
            halt = ready.halt;
            waiting = ready.waiting;
            for (node, prompt) in ready.starts {
                running.push(run_node(shared, runner, ctl, node, prompt));
            }
        }
        if running.is_empty() {
            edit(shared, ctl, |r| settle(r, halt, waiting));
            return;
        }
        let (node, result) = running.next().await.expect("something is running");
        if let Some(pause) = edit(shared, ctl, |r| finish(r, &node, result)) {
            halt.get_or_insert(pause);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};

    use serde_json::json;

    use super::*;
    use crate::orch::graph::{Budget, Edge, EdgeKind, Engine, Flow, Role};
    use crate::orch::usage::Tokens;

    fn node(id: &str, role: Role) -> Node {
        Node {
            id: id.into(),
            name: id.into(),
            role,
            custom: None,
            engine: Engine::Claude,
            model: "sonnet".into(),
            system_prompt: String::new(),
            checkpoint: false,
            token_cap: None,
            keep_model: false,
            account: None,
            direct: false,
            web: false,
            x: 0.0,
            y: 0.0,
        }
    }

    fn edge(id: &str, from: &str, to: &str, kind: EdgeKind) -> Edge {
        Edge::new(id, from, to, kind)
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
            downshift: false,
        }
    }

    fn ok(v: serde_json::Value) -> anyhow::Result<Outcome> {
        Ok(Outcome {
            structured: Some(v),
            text: String::new(),
            tokens: Tokens { input: 10, output: 10, cache_read: 0, cache_write: 0 },
            cost_usd: 0.0,
            credits: 0,
            is_error: false,
            session: None,
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
        /// Agents that wait here until all of them have started: this deadlocks unless they run together.
        meet: Option<(std::sync::Arc<tokio::sync::Barrier>, Vec<String>)>,
        /// The model each agent was started on, in start order.
        models: Mutex<Vec<(String, String)>>,
        /// Set when a judge runs, so a test can make the plan pressure ease partway through.
        calm: Option<std::sync::Arc<AtomicBool>>,
        /// Extra progress lines every agent writes as it works.
        lines: Vec<String>,
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

        /// The model the `nth` start of this agent used.
        fn model_of(&self, id: &str, nth: usize) -> String {
            self.models.lock().unwrap().iter().filter(|(i, _)| i == id).nth(nth).unwrap().1.clone()
        }

        fn prompt(&self, id: &str, nth: usize) -> String {
            self.prompts.lock().unwrap().iter().filter(|(i, _)| i == id).nth(nth).unwrap().1.clone()
        }
    }

    impl NodeRunner for Fake {
        async fn run(&self, node: &Node, prompt: &str, log: &mut (dyn FnMut(String) + Send)) -> anyhow::Result<Outcome> {
            self.prompts.lock().unwrap().push((node.id.clone(), prompt.to_string()));
            self.models.lock().unwrap().push((node.id.clone(), node.model.clone()));
            if node.role == Role::Judge {
                if let Some(c) = &self.calm {
                    c.store(true, Ordering::SeqCst);
                }
            }
            log(format!("working on {}", node.id));
            for l in &self.lines {
                log(l.clone());
            }
            if let Some((barrier, ids)) = &self.meet {
                if ids.contains(&node.id) {
                    barrier.wait().await;
                }
            }
            let next = self.script.lock().unwrap().get_mut(&node.id).and_then(|q| q.pop_front());
            next.unwrap_or_else(|| ok(json!({"summary": format!("{} done", node.id)})))
        }
    }

    async fn go(run: &Mutex<Run>, fake: &Fake) {
        go_limited(run, fake, &|_| None).await
    }

    async fn go_limited(run: &Mutex<Run>, fake: &Fake, limit: &(dyn Fn(&Run) -> Option<String> + Sync)) {
        let cancelled = AtomicBool::new(false);
        drive(run, fake, &Control { cancelled: &cancelled, changed: &|_| {}, limit, pressure: &|_, _| 0.0 }).await;
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
    async fn the_log_tells_what_was_sent_what_came_back_and_why_each_step_happened() {
        let run = new_run(flow());
        let fake = Fake::with(vec![("judge", reject("No tests")), ("judge", approve())]);
        go(&run, &fake).await;
        let r = run.lock().unwrap();
        let of = |k: LogKind| r.log.iter().filter(|e| e.kind == k).collect::<Vec<_>>();
        // Six turns ran, and each has a request and an answer.
        assert_eq!(of(LogKind::Request).len(), 6);
        assert_eq!(of(LogKind::Response).len(), 6);
        assert!(of(LogKind::Request)[0].body.as_deref().unwrap().contains("Build a thing"));
        assert!(of(LogKind::Request)[0].title.contains("attempt 1"));
        assert!(of(LogKind::Response)[0].body.as_deref().unwrap().contains("10 in"));
        let why: Vec<&str> = of(LogKind::Decision).iter().map(|e| e.title.as_str()).collect();
        assert!(why.iter().any(|t| t.contains("orch starts first")), "{why:?}");
        assert!(why.iter().any(|t| t.contains("res starts: orch finished")), "{why:?}");
        assert!(why.iter().any(|t| t.contains("judge rejected the work and sent it back to build (send-back 1 of 2)")), "{why:?}");
        assert!(why.iter().any(|t| t.contains("build starts again with the feedback")), "{why:?}");
        assert!(why.iter().any(|t| t.contains("judge approved")), "{why:?}");
        assert!(why.last().unwrap().contains("the run is done"));
        let sent_back = of(LogKind::Decision).into_iter().find(|e| e.title.contains("sent it back")).unwrap();
        assert_eq!(sent_back.body.as_deref(), Some("No tests"));
    }

    #[tokio::test]
    async fn a_failure_and_a_pause_are_logged_with_the_reason() {
        let run = new_run(flow());
        let fake = Fake::with(vec![("res", Err(anyhow::anyhow!("boom")))]);
        go(&run, &fake).await;
        let r = run.lock().unwrap();
        let err = r.log.iter().find(|e| e.kind == LogKind::Error).expect("an error entry");
        assert!(err.title.contains("res failed") && err.body.as_deref() == Some("boom"));
        assert!(r.log.last().unwrap().title.contains("Stopped: res failed (boom)"));
    }

    #[tokio::test]
    async fn skills_and_sub_agents_get_their_own_log_entries() {
        let run = new_run(flow());
        let fake = Fake { lines: vec![format!("{SKILL_PREFIX}pdf"), format!("{SUBAGENT_PREFIX}Explore: look around")], ..Fake::default() };
        go(&run, &fake).await;
        let r = run.lock().unwrap();
        assert!(r.log.iter().any(|e| e.kind == LogKind::Skill && e.title == "orch used the skill pdf" && e.node.as_deref() == Some("orch")));
        assert!(r.log.iter().any(|e| e.kind == LogKind::Subagent && e.title == "build started a sub-agent: Explore: look around"));
        // They stay in the agent's own transcript as well.
        assert!(r.nodes["orch"].transcript.iter().any(|l| l == &format!("{SKILL_PREFIX}pdf")));
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
            credits: 0,
            is_error: false,
            session: None,
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
        drive(&run, &fake, &Control { cancelled: &cancelled, changed: &|_| {}, limit: &|_| None, pressure: &|_, _| 0.0 }).await;
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
        drive(&run, &fake, &Control { cancelled: &cancelled, changed: &changed, limit: &|_| None, pressure: &|_, _| 0.0 }).await;
        // At least a start and a finish for each of the four nodes.
        assert!(count.load(Ordering::Relaxed) >= 8);
    }

    /// orch feeds a and b, which run side by side; join needs both.
    fn fan() -> Flow {
        Flow {
            id: "fan".into(),
            name: "Fan".into(),
            nodes: vec![node("orch", Role::Orchestrator), node("a", Role::Researcher), node("b", Role::Search), node("join", Role::Analyst)],
            edges: vec![
                edge("e1", "orch", "a", EdgeKind::Forward),
                edge("e2", "orch", "b", EdgeKind::Forward),
                edge("e3", "a", "join", EdgeKind::Forward),
                edge("e4", "b", "join", EdgeKind::Forward),
            ],
            max_retries: 2,
            budget: Budget::default(),
            downshift: false,
        }
    }

    #[tokio::test]
    async fn independent_branches_run_at_the_same_time() {
        let run = new_run(fan());
        let mut fake = Fake::default();
        // Each waits for the other, so a sequential executor would hang here.
        fake.meet = Some((std::sync::Arc::new(tokio::sync::Barrier::new(2)), vec!["a".into(), "b".into()]));
        tokio::time::timeout(std::time::Duration::from_secs(3), go(&run, &fake)).await.expect("a and b never overlapped");
        assert_eq!(status(&run), RunStatus::Done);
    }

    #[tokio::test]
    async fn an_agent_waits_for_every_one_of_its_inputs() {
        let run = new_run(fan());
        let fake = Fake::default();
        go(&run, &fake).await;
        let ran = fake.ran();
        assert_eq!(ran.first().map(String::as_str), Some("orch"));
        assert_eq!(ran.last().map(String::as_str), Some("join"), "join starts only after a and b: {ran:?}");
        let p = fake.prompt("join", 0);
        assert!(p.contains("a done") && p.contains("b done"), "it is handed both: {p}");
    }

    #[tokio::test]
    async fn a_failure_lets_the_other_branch_finish_then_pauses() {
        let run = new_run(fan());
        let fake = Fake::with(vec![("a", Err(anyhow::anyhow!("claude exited")))]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Paused { pause: Pause::Failed { node: "a".into(), message: "claude exited".into() } });
        {
            let r = run.lock().unwrap();
            assert_eq!(r.nodes["b"].status, NodeStatus::Done, "the branch that was running kept going");
            assert_eq!(r.nodes["join"].status, NodeStatus::Pending);
        }
        assert!(!fake.ran().contains(&"join".to_string()));
        // Trying again redoes only what failed.
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done);
        assert_eq!(fake.ran().iter().filter(|i| *i == "b").count(), 1);
        assert_eq!(fake.ran().iter().filter(|i| *i == "a").count(), 2);
    }

    #[tokio::test]
    async fn an_agent_held_for_approval_does_not_hold_up_its_sibling() {
        let mut f = fan();
        f.nodes[1].checkpoint = true; // a waits for the user
        let run = new_run(f);
        let fake = Fake::default();
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Paused { pause: Pause::Approval { node: "a".into() } });
        assert_eq!(run.lock().unwrap().nodes["b"].status, NodeStatus::Done, "b ran while a waited");
        run.lock().unwrap().approved.insert("a".into());
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done);
    }

    #[tokio::test]
    async fn a_rejection_redoes_only_the_path_to_the_judge_not_a_branch_beside_it() {
        // orch -> build -> judge, and build -> a-aside. "a-aside" sorts between build and judge in run order.
        let mut f = flow();
        f.nodes.retain(|n| n.id != "res");
        f.edges = vec![
            edge("e1", "orch", "build", EdgeKind::Forward),
            edge("e2", "build", "judge", EdgeKind::Forward),
            edge("e3", "judge", "build", EdgeKind::Retry),
            edge("e4", "build", "a-aside", EdgeKind::Forward),
        ];
        f.nodes.push(node("a-aside", Role::Analyst));
        let run = new_run(f);
        let fake = Fake::with(vec![("judge", reject("No tests")), ("judge", approve())]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done);
        assert_eq!(fake.ran().iter().filter(|i| *i == "build").count(), 2);
        assert_eq!(fake.ran().iter().filter(|i| *i == "a-aside").count(), 1, "its work was kept: {:?}", fake.ran());
    }

    async fn go_at(run: &Mutex<Run>, fake: &Fake, pressure: &(dyn Fn(&Run, Engine) -> f64 + Sync)) {
        let cancelled = AtomicBool::new(false);
        drive(run, fake, &Control { cancelled: &cancelled, changed: &|_| {}, limit: &|_| None, pressure }).await;
    }

    /// orch (opus) -> res (sonnet) -> build (opus) -> judge (sonnet), downshift on.
    fn priced() -> Flow {
        let mut f = flow();
        f.downshift = true;
        f.nodes[0].model = "opus".into();
        f.nodes[2].model = "opus".into();
        f
    }

    #[tokio::test]
    async fn models_stay_as_they_are_unless_the_flow_asks_for_downshift() {
        let mut f = priced();
        f.downshift = false;
        let run = new_run(f);
        let fake = Fake::with(vec![("judge", approve())]);
        go_at(&run, &fake, &|_, _| 0.95).await;
        assert_eq!(fake.model_of("orch", 0), "opus");
        assert_eq!(fake.model_of("build", 0), "opus");
        assert_eq!(run.lock().unwrap().nodes["orch"].model_used.as_deref(), Some("opus"));
    }

    #[tokio::test]
    async fn a_filling_plan_steps_models_down_once_then_all_the_way() {
        let run = new_run(priced());
        go_at(&run, &Fake::with(vec![("judge", approve())]), &|_, _| 0.6).await;
        assert_eq!(run.lock().unwrap().nodes["orch"].model_used.as_deref(), Some("sonnet"));
        assert_eq!(run.lock().unwrap().nodes["res"].model_used.as_deref(), Some("haiku"), "sonnet is one step above haiku");
        assert!(run.lock().unwrap().nodes["orch"].transcript.iter().any(|l| l.contains("sonnet instead of opus")));

        let run = new_run(priced());
        let fake = Fake::with(vec![("judge", approve())]);
        go_at(&run, &fake, &|_, _| 0.9).await;
        for id in ["orch", "res", "build", "judge"] {
            assert_eq!(fake.model_of(id, 0), "haiku", "{id}");
        }
    }

    #[tokio::test]
    async fn a_pinned_agent_keeps_its_model() {
        let mut f = priced();
        f.nodes[2].keep_model = true; // the builder
        let run = new_run(f);
        let fake = Fake::with(vec![("judge", approve())]);
        go_at(&run, &fake, &|_, _| 0.95).await;
        assert_eq!(fake.model_of("build", 0), "opus");
        assert_eq!(fake.model_of("orch", 0), "haiku");
    }

    #[tokio::test]
    async fn a_send_back_carries_on_with_the_model_already_chosen() {
        // Pressure is high when the builder first starts and low by the time the judge sends it back.
        let calm = std::sync::Arc::new(AtomicBool::new(false));
        let mut fake = Fake::with(vec![("judge", reject("No tests")), ("judge", approve())]);
        fake.calm = Some(calm.clone());
        let run = new_run(priced());
        go_at(&run, &fake, &move |_, _| if calm.load(Ordering::SeqCst) { 0.0 } else { 0.9 }).await;
        assert_eq!(status(&run), RunStatus::Done);
        assert_eq!(fake.model_of("build", 0), "haiku");
        assert_eq!(fake.model_of("build", 1), "haiku", "a different model would start a new conversation and lose the cache");
    }

    #[tokio::test]
    async fn it_never_goes_up_and_leaves_unknown_models_alone() {
        let mut f = priced();
        f.nodes[1].model = "haiku".into();
        f.nodes[3].model = "mystery-1".into();
        let run = new_run(f);
        let fake = Fake::with(vec![("judge", approve())]);
        go_at(&run, &fake, &|_, _| 0.95).await;
        assert_eq!(fake.model_of("res", 0), "haiku");
        assert_eq!(fake.model_of("judge", 0), "mystery-1");
    }

    #[tokio::test]
    async fn a_finished_run_records_when() {
        let run = new_run(fan());
        go(&run, &Fake::default()).await;
        assert!(run.lock().unwrap().finished.is_some());
        let run = new_run(fan());
        let cancelled = AtomicBool::new(true);
        drive(&run, &Fake::default(), &Control { cancelled: &cancelled, changed: &|_| {}, limit: &|_| None, pressure: &|_, _| 0.0 }).await;
        assert!(run.lock().unwrap().finished.is_some(), "a cancelled run has finished too");
        // One that merely paused has not.
        let run = new_run(flow());
        go(&run, &Fake::with(vec![("res", Err(anyhow::anyhow!("x")))])).await;
        assert_eq!(run.lock().unwrap().finished, None);
    }

    fn ok_with(v: serde_json::Value, tokens: Tokens) -> anyhow::Result<Outcome> {
        Ok(Outcome { structured: Some(v), text: String::new(), tokens, cost_usd: 0.0, credits: 0, is_error: false, session: None })
    }

    fn tok(input: u64, cache_read: u64) -> Tokens {
        Tokens { input, output: 10, cache_read, cache_write: 0 }
    }

    /// The builder runs twice (the judge sends it back once) with these two turns' token use.
    async fn builder_turns(first: Tokens, second: Tokens) -> Mutex<Run> {
        let run = new_run(flow());
        let body = json!({"summary": "built"});
        let fake = Fake::with(vec![
            ("build", ok_with(body.clone(), first)),
            ("build", ok_with(body, second)),
            ("judge", reject("No tests")),
            ("judge", approve()),
        ]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done);
        run
    }

    #[tokio::test]
    async fn a_turn_that_loses_the_cache_is_flagged() {
        // 97% from the cache, then 2%: the prompt had to be paid for again.
        let run = builder_turns(tok(100, 4000), tok(3900, 100)).await;
        let r = run.lock().unwrap();
        assert_eq!(r.nodes["build"].cache_breaks, 1);
        assert!(r.nodes["build"].transcript.iter().any(|l| l.contains("Cache break: only 2%")), "{:?}", r.nodes["build"].transcript);
    }

    #[tokio::test]
    async fn a_cache_that_keeps_working_is_not_flagged() {
        let run = builder_turns(tok(100, 4000), tok(150, 4500)).await;
        assert_eq!(run.lock().unwrap().nodes["build"].cache_breaks, 0);
    }

    #[tokio::test]
    async fn a_cold_first_turn_and_a_small_prompt_are_not_breaks() {
        // The first turn has nothing before it to compare with.
        let run = builder_turns(tok(4000, 0), tok(3900, 100)).await;
        assert_eq!(run.lock().unwrap().nodes["build"].cache_breaks, 0);
        // Under the minimum input, a low share means little.
        let run = builder_turns(tok(10, 900), tok(900, 10)).await;
        assert_eq!(run.lock().unwrap().nodes["build"].cache_breaks, 0);
    }

    fn reject_to(why: &str, target: &str) -> anyhow::Result<Outcome> {
        ok(json!({"verdict": "reject", "feedback": why, "send_back_to": target}))
    }

    /// orch -> res -> build -> judge; the judge can send work back to either res (the findings) or build (the code).
    fn two_ways() -> Flow {
        let mut f = flow();
        f.edges.push(edge("e5", "judge", "res", EdgeKind::Retry));
        f
    }

    #[tokio::test]
    async fn a_verdict_sends_work_back_to_the_agent_it_names_and_redoes_what_follows_it() {
        let run = new_run(two_ways());
        let fake = Fake::with(vec![("judge", reject_to("The findings are wrong", "res")), ("judge", approve())]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done);
        // The planner is not redone; the researcher is, then everything that depends on it.
        assert_eq!(fake.ran(), vec!["orch", "res", "build", "judge", "res", "build", "judge"]);
        assert!(fake.prompt("res", 1).contains("The findings are wrong"), "the researcher is told what was wrong");
        assert!(!fake.prompt("build", 1).contains("The findings are wrong"), "the builder is only handed the new findings");
        assert_eq!(run.lock().unwrap().retries_used, 1);
    }

    #[tokio::test]
    async fn naming_the_other_place_sends_work_there_instead() {
        let run = new_run(two_ways());
        let fake = Fake::with(vec![("judge", reject_to("No tests", "build")), ("judge", approve())]);
        go(&run, &fake).await;
        assert_eq!(fake.ran(), vec!["orch", "res", "build", "judge", "build", "judge"], "the researcher kept its work");
    }

    #[tokio::test]
    async fn a_rejection_that_does_not_say_where_pauses_and_the_judge_is_asked_again() {
        for bad in [ok(json!({"verdict": "reject", "feedback": "no", "send_back_to": "none"})), reject_to("no", "nowhere")] {
            let run = new_run(two_ways());
            let fake = Fake::with(vec![("judge", bad), ("judge", reject_to("Now I say", "build")), ("judge", approve())]);
            go(&run, &fake).await;
            let RunStatus::Paused { pause: Pause::Failed { node, message } } = status(&run) else { panic!("not paused") };
            assert_eq!(node, "judge");
            assert!(message.contains("which agent should redo it"), "{message}");
            assert_eq!(run.lock().unwrap().nodes["judge"].status, NodeStatus::Failed, "so resuming asks it again");
            assert_eq!(run.lock().unwrap().retries_used, 0, "nothing was sent back");
            // Trying again asks the judge once more rather than letting the rejection pass.
            go(&run, &fake).await;
            // It ended only after the judge said where to send the work, the builder redid it, and it was approved.
            assert_eq!(fake.ran().iter().filter(|i| *i == "judge").count(), 3, "asked, asked again, then asked about the redone work");
            assert_eq!(fake.ran().iter().filter(|i| *i == "build").count(), 2, "the rejection was acted on, not waved through");
            assert_eq!(status(&run), RunStatus::Done);
        }
    }

    #[tokio::test]
    async fn the_retry_limit_is_shared_across_the_places_work_can_go() {
        let mut f = two_ways();
        f.max_retries = 2;
        let run = new_run(f);
        let fake = Fake::with(vec![
            ("judge", reject_to("a", "res")),
            ("judge", reject_to("b", "build")),
            ("judge", reject_to("c", "res")),
        ]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Paused { pause: Pause::RetriesExhausted { node: "judge".into() } });
        assert_eq!(run.lock().unwrap().retries_used, 2);
    }

    #[tokio::test]
    async fn a_judge_with_one_place_to_go_needs_no_answer() {
        // The flow() judge has one retry edge: a rejection that names nothing still goes there.
        let run = new_run(flow());
        let fake = Fake::with(vec![("judge", reject("No tests")), ("judge", approve())]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done);
        assert_eq!(fake.ran().iter().filter(|i| *i == "build").count(), 2);
    }

    // ---- loops: questions, per-edge caps, and who answers ----

    fn asks(qs: &[&str]) -> anyhow::Result<Outcome> {
        ok(json!({"questions": qs, "resolved": false}))
    }

    fn satisfied() -> anyhow::Result<Outcome> {
        ok(json!({"questions": [], "resolved": true}))
    }

    /// plan -> grill -> report, with the griller able to send work back to the plan.
    fn grilled(answered_by: Answerer, max_iterations: u8, on_cap: OnCap) -> Flow {
        let mut back = edge("back", "grill", "plan", EdgeKind::Retry);
        back.answered_by = answered_by;
        back.max_iterations = max_iterations;
        back.on_cap = on_cap;
        Flow {
            id: "f".into(),
            name: "F".into(),
            nodes: vec![node("plan", Role::Orchestrator), node("grill", Role::Griller), node("report", Role::Reporter)],
            edges: vec![edge("e1", "plan", "grill", EdgeKind::Forward), edge("e2", "grill", "report", EdgeKind::Forward), back],
            max_retries: 6,
            budget: Budget::default(),
            downshift: false,
        }
    }

    #[tokio::test]
    async fn a_griller_that_asks_sends_the_work_back_with_its_questions_until_it_is_satisfied() {
        let run = new_run(grilled(Answerer::Agent, 3, OnCap::Pause));
        let fake = Fake::with(vec![("grill", asks(&["Which database?", "Who logs in?"])), ("grill", satisfied())]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done);
        assert_eq!(fake.ran(), vec!["plan", "grill", "plan", "grill", "report"]);
        let again = fake.prompt("plan", 1);
        assert!(again.contains("# Questions about your work") && again.contains("1. Which database?") && again.contains("2. Who logs in?"), "{again}");
        assert!(!again.contains("Judge feedback"), "it is not a verdict");
        // The griller is told what it asked before, so it does not ask it again.
        let second = fake.prompt("grill", 1);
        assert!(second.contains("# What you have asked before") && second.contains("Which database?"), "{second}");
        assert!(!fake.prompt("grill", 0).contains("asked before"), "nothing before the first time");
        let r = run.lock().unwrap();
        assert_eq!(r.iterations["back"], 1);
        assert_eq!(r.retries_used, 1);
    }

    #[tokio::test]
    async fn questions_for_the_user_stop_the_run_and_the_answers_go_back_with_the_work() {
        let run = new_run(grilled(Answerer::User, 3, OnCap::Pause));
        let fake = Fake::with(vec![("grill", asks(&["Which database?", "Who logs in?"])), ("grill", satisfied())]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Paused { pause: Pause::Questions { node: "grill".into() } });
        {
            let r = run.lock().unwrap();
            let p = r.pending.clone().expect("waiting for answers");
            assert_eq!((p.node.as_str(), p.target.as_str(), p.questions.len()), ("grill", "plan", 2));
            assert_eq!(r.nodes["plan"].status, NodeStatus::Done, "nothing is redone until the user has answered");
        }
        let mut r = run.lock().unwrap();
        assert!(answer_questions(&mut r, vec!["only one".into()]).unwrap_err().contains("2 questions and 1 answers"));
        assert!(r.pending.is_some(), "a bad answer changes nothing");
        answer_questions(&mut r, vec!["Postgres".into(), "  Staff only  ".into()]).unwrap();
        assert_eq!((r.status.clone(), r.pending.clone()), (RunStatus::Running, None));
        assert_eq!(r.nodes["plan"].status, NodeStatus::Pending);
        assert_eq!(r.nodes["grill"].status, NodeStatus::Pending);
        assert_eq!(r.nodes["report"].status, NodeStatus::Pending, "report had not run, and is still waiting its turn");
        drop(r);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done);
        let again = fake.prompt("plan", 1);
        assert!(again.contains("# Answers to questions about your work") && again.contains("Answer: Postgres") && again.contains("Answer: Staff only"), "{again}");
        assert!(fake.prompt("grill", 1).contains("Answer: Postgres"), "the griller sees what was answered");
        assert!(answer_questions(&mut run.lock().unwrap(), vec![]).unwrap_err().contains("not waiting"));
    }

    #[tokio::test]
    async fn an_edge_used_up_does_what_it_says() {
        // Pause: stop and ask.
        let run = new_run(grilled(Answerer::Agent, 1, OnCap::Pause));
        let fake = Fake::with(vec![("grill", asks(&["a?"])), ("grill", asks(&["b?"]))]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Paused { pause: Pause::RetriesExhausted { node: "grill".into() } });
        assert_eq!(run.lock().unwrap().iterations["back"], 1, "the second send-back was not counted");

        // Proceed: carry on with what there is.
        let run = new_run(grilled(Answerer::Agent, 1, OnCap::Proceed));
        let fake = Fake::with(vec![("grill", asks(&["a?"])), ("grill", asks(&["b?"]))]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done);
        assert_eq!(fake.ran(), vec!["plan", "grill", "plan", "grill", "report"], "the report still runs, on the work as it is");

        // Fail: the agent that wanted to send it back fails.
        let run = new_run(grilled(Answerer::Agent, 1, OnCap::Fail));
        let fake = Fake::with(vec![("grill", asks(&["a?"])), ("grill", asks(&["b?"]))]);
        go(&run, &fake).await;
        let RunStatus::Paused { pause: Pause::Failed { node, message } } = status(&run) else { panic!("not failed") };
        assert_eq!(node, "grill");
        assert!(message.contains("as many times as it may be (1)"), "{message}");
        assert_eq!(run.lock().unwrap().nodes["grill"].status, NodeStatus::Failed);
    }

    #[tokio::test]
    async fn the_same_caps_apply_to_a_judges_send_backs() {
        let mut f = flow();
        f.edges[3].max_iterations = 1;
        f.edges[3].on_cap = OnCap::Proceed;
        let run = new_run(f);
        let fake = Fake::with(vec![("judge", reject("first")), ("judge", reject("second"))]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Done, "the second rejection could not go back, and the flow carried on");
        assert_eq!(fake.ran().iter().filter(|i| *i == "build").count(), 2);
    }

    #[tokio::test]
    async fn the_flows_own_limit_still_stops_everything_whatever_the_edges_allow() {
        let mut f = grilled(Answerer::Agent, 9, OnCap::Proceed);
        f.max_retries = 2;
        let run = new_run(f);
        let fake = Fake::with(vec![("grill", asks(&["1?"])), ("grill", asks(&["2?"])), ("grill", asks(&["3?"]))]);
        go(&run, &fake).await;
        assert_eq!(status(&run), RunStatus::Paused { pause: Pause::RetriesExhausted { node: "grill".into() } }, "the backstop pauses even when the edge would proceed");
        assert_eq!(run.lock().unwrap().retries_used, 2);
    }

    #[tokio::test]
    async fn a_griller_with_nothing_to_ask_lets_the_flow_through_and_one_with_nowhere_to_send_it_is_advisory() {
        let run = new_run(grilled(Answerer::User, 2, OnCap::Pause));
        go(&run, &Fake::with(vec![("grill", satisfied())])).await;
        assert_eq!(status(&run), RunStatus::Done);
        assert!(run.lock().unwrap().pending.is_none());

        let mut f = grilled(Answerer::User, 2, OnCap::Pause);
        f.edges.pop(); // no way back drawn
        let run = new_run(f);
        go(&run, &Fake::with(vec![("grill", asks(&["still unclear?"]))])).await;
        assert_eq!(status(&run), RunStatus::Done, "questions with nowhere to go do not stop the flow");
    }

    #[tokio::test]
    async fn work_can_be_sent_back_to_an_agent_that_does_not_write() {
        // The target here is the researcher, which only reads.
        let run = new_run(two_ways());
        let fake = Fake::with(vec![("judge", reject_to("Wrong", "res")), ("judge", approve())]);
        go(&run, &fake).await;
        assert_eq!(run.lock().unwrap().nodes["res"].attempts, 2);
    }
}
