//! One execution of a flow: the brief, how far each node has got, and what they produced. Saved
//! to disk after every change so a restart loses at most the node that was running.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::graph::Flow;
use super::usage::Tokens;

/// Most transcript lines kept for each node. The page shows these live; they are not the handoff.
const MAX_TRANSCRIPT: usize = 200;

/// Most entries kept in a run's log, and the longest body kept for each. The log is shown live and is saved with
/// the run on every change, so both stay small.
const MAX_LOG: usize = 300;
const MAX_BODY: usize = 5000;

/// A line an agent's driver writes when the agent calls a skill, and when it starts a sub-agent. The executor
/// notices them and adds an entry to the run log.
pub const SKILL_PREFIX: &str = "Skill: ";
pub const SUBAGENT_PREFIX: &str = "Sub-agent: ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogKind {
    /// The executor chose something: what starts next, why work was sent back, why the run stopped.
    Decision,
    /// What an agent was sent.
    Request,
    /// What an agent answered.
    Response,
    /// An agent used a skill.
    Skill,
    /// An agent started a sub-agent.
    Subagent,
    Event,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    pub at: i64,
    /// The agent it is about, if any.
    pub node: Option<String>,
    pub kind: LogKind,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeStatus {
    Pending,
    Running,
    Done,
    Failed,
    /// Was running when ttm stopped. Resume runs it again.
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Pause {
    /// Waiting for the user to approve this node before it starts.
    Approval { node: String },
    /// The judge turned work back more times than the flow allows.
    RetriesExhausted { node: String },
    /// A cap was reached; the message says which.
    Budget { message: String },
    /// A node failed; Resume tries it again.
    Failed { node: String, message: String },
    /// An agent asked questions that only the user can answer; the run waits for them.
    Questions { node: String },
}

/// Why an agent is being run again, which decides how its prompt puts it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedbackKind {
    /// A judge rejected its work.
    #[default]
    Judge,
    /// An agent asked questions about its work, answered by the user.
    Answers,
    /// An agent asked questions about its work, for it to answer itself.
    Questions,
}

/// One question an agent asked, and the answer if it has one yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asked {
    pub question: String,
    pub answer: Option<String>,
}

/// Questions waiting for the user, and where the work goes once they are answered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingQuestions {
    /// The agent that asked.
    pub node: String,
    /// The agent the work goes back to.
    pub target: String,
    /// The send-back edge it goes along.
    pub edge: String,
    pub questions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum RunStatus {
    /// The orchestrator is asking questions before the flow starts.
    Grilling,
    Running,
    Paused { pause: Pause },
    Done,
    Cancelled,
}

/// One question the orchestrator asks before the flow starts, with the answer it would pick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub text: String,
    /// The answer the orchestrator would pick. One of `options` when there are any.
    pub recommended: String,
    /// Two to four distinct answers to choose from. Empty for a question asked without them (an older run, or one
    /// the model could not give choices for), which the page shows as a text box.
    #[serde(default)]
    pub options: Vec<String>,
    /// More than one option can be chosen: "which of these matter most".
    #[serde(default)]
    pub multiple: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Qa {
    pub question: String,
    pub answer: String,
}

/// Where the grilling step has got to.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Grill {
    /// Rounds of questions asked so far.
    pub round: u8,
    pub history: Vec<Qa>,
    /// Questions waiting for answers.
    pub pending: Vec<Question>,
    /// The orchestrator's summary of what to build, once it has enough. The user confirms it.
    pub brief: Option<String>,
}

/// How full each plan-limit window was when the run started, so the run's own share can be told.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub claude: Option<f64>,
    pub codex: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRun {
    pub status: NodeStatus,
    /// How many times this node has started (more than one after a judge sends work back).
    pub attempts: u32,
    /// The structured answer handed to the nodes downstream.
    pub output: Option<Value>,
    pub error: Option<String>,
    /// Use over all attempts.
    pub tokens: Tokens,
    /// Dollars spent over all attempts. On OpenRouter this is what was charged; for a Claude agent on your plan
    /// it is Claude Code's estimate at API prices, which is not billed.
    #[serde(default)]
    pub cost_usd: f64,
    /// Firecrawl credits its web tools used over all attempts.
    #[serde(default)]
    pub credits: u64,
    pub transcript: Vec<String>,
    /// The agent's own conversation id after its latest finished turn, for opening it in a live session.
    #[serde(default)]
    pub session: Option<String>,
    /// The model this agent actually ran on: the flow's, or a cheaper one if the plan was filling up.
    #[serde(default)]
    pub model_used: Option<String>,
    /// How hard it was set to think on its latest turn, when that was set.
    #[serde(default)]
    pub effort_used: Option<String>,
    /// Why its latest turn ran on the model and effort it did, when the run chose them.
    #[serde(default)]
    pub route_reason: Option<String>,
    /// Share of the previous turn's input that came from the prompt cache.
    #[serde(default)]
    pub last_hit: Option<f64>,
    /// Times a turn got far less from the cache than the one before it: something made the agent pay for its
    /// prompt again.
    #[serde(default)]
    pub cache_breaks: u32,
}

impl NodeRun {
    fn new() -> Self {
        Self {
            status: NodeStatus::Pending,
            attempts: 0,
            output: None,
            error: None,
            tokens: Tokens::default(),
            cost_usd: 0.0,
            credits: 0,
            transcript: Vec::new(),
            session: None,
            model_used: None,
            effort_used: None,
            route_reason: None,
            last_hit: None,
            cache_breaks: 0,
        }
    }

    pub fn log(&mut self, line: String) {
        if self.transcript.len() >= MAX_TRANSCRIPT {
            self.transcript.remove(0);
        }
        self.transcript.push(line);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    /// A copy of the flow as it was when the run started, so editing the template later changes nothing.
    pub flow: Flow,
    /// What the user typed.
    #[serde(default)]
    pub goal: String,
    /// What to build, after the grilling step. Every node is shown this.
    pub brief: String,
    /// The git folder agents work in; builders get a worktree of it.
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub grill: Option<Grill>,
    #[serde(default)]
    pub baseline: Baseline,
    /// Use outside the nodes: writing prompts and grilling.
    #[serde(default)]
    pub overhead: Tokens,
    /// Pages fetched from the links in the goal, shown to every agent after the brief.
    #[serde(default)]
    pub references: Vec<Reference>,
    /// Dollars spent outside the nodes, on paid models: choosing models for agents, mostly.
    #[serde(default)]
    pub overhead_usd: f64,
    pub status: RunStatus,
    pub nodes: HashMap<String, NodeRun>,
    /// Nodes the user has let start.
    pub approved: HashSet<String>,
    /// Judge feedback waiting for the node it was sent back to.
    pub feedback: HashMap<String, String>,
    /// What kind each waiting piece of feedback is.
    #[serde(default)]
    pub feedback_kind: HashMap<String, FeedbackKind>,
    pub retries_used: u8,
    /// How many times each send-back edge has been used, by edge id.
    #[serde(default)]
    pub iterations: HashMap<String, u8>,
    /// What each agent that asks questions has asked so far, with the answers, by node id.
    #[serde(default)]
    pub asked: HashMap<String, Vec<Asked>>,
    /// Questions the run is waiting for the user to answer.
    #[serde(default)]
    pub pending: Option<PendingQuestions>,
    pub created: i64,
    /// When it reached done or cancelled; `None` while it can still go on.
    #[serde(default)]
    pub finished: Option<i64>,
    /// The user chose to carry on past the plan limits: they are not checked again for this run.
    #[serde(default)]
    pub waived: bool,
    /// What happened and why, oldest first: requests, answers, decisions, skills and sub-agents.
    #[serde(default)]
    pub log: Vec<LogEntry>,
    /// How many entries have ever been added to `log`, which holds only the latest.
    #[serde(default)]
    pub log_total: u64,
    /// Every turn the agents have taken, oldest first.
    #[serde(default)]
    pub turns: Vec<Turn>,
}

/// A web page fetched for a run, given to every agent as reference material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    pub url: String,
    /// The page's text, or why it could not be read.
    pub text: String,
    /// Firecrawl credits it cost.
    pub credits: u64,
    /// The page could not be fetched.
    #[serde(default)]
    pub failed: bool,
}

/// Longest page text kept per reference.
pub const MAX_REFERENCE: usize = 8_000;

/// Unix time in milliseconds.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Most turns kept on a run; the oldest drop off. Each is small: the full record of a turn is in its own file.
pub const MAX_TURNS: usize = 500;

/// One turn of one agent, in brief: when, on what, what it used, and what the run did with the answer. The full
/// record of what it did is read separately (`TurnDetail`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    /// Counts from 1 across the whole run, in the order turns finished.
    pub n: u32,
    pub node: String,
    /// Which time this agent has started.
    pub attempt: u32,
    /// How many send-backs had happened when it started: the round of the run it belongs to.
    pub round: u8,
    /// Unix milliseconds.
    pub started: i64,
    pub ended: i64,
    /// `claude`, `codex`, or `direct`.
    pub engine: String,
    pub model: String,
    /// The account it ran on, when not your own plan.
    pub account: Option<String>,
    pub tokens: Tokens,
    pub cost_usd: f64,
    pub credits: u64,
    /// The agent's own conversation id, when it keeps one.
    pub session: Option<String>,
    /// Why the turn failed, when it did.
    pub failed: Option<String>,
    /// What the run decided on seeing the answer: carried on, sent work back, asked questions.
    pub decision: Option<String>,
    /// The commit a builder's work was saved in.
    pub commit: Option<String>,
    /// Reasoning effort it ran at, when one was set.
    #[serde(default)]
    pub effort: Option<String>,
    /// Why this model and effort were chosen, when the run chose them.
    #[serde(default)]
    pub reason: Option<String>,
}

/// What a turn left besides its summary, kept in a file of its own and read when someone opens the turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnDetail {
    pub n: u32,
    /// The answer it handed on.
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub trace: Vec<super::trace::Entry>,
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl Run {
    pub fn new(id: String, flow: Flow, brief: String, created: i64) -> Run {
        let nodes = flow.nodes.iter().map(|n| (n.id.clone(), NodeRun::new())).collect();
        Run {
            id,
            flow,
            goal: brief.clone(),
            brief,
            project: String::new(),
            grill: None,
            baseline: Baseline::default(),
            overhead: Tokens::default(),
            overhead_usd: 0.0,
            references: Vec::new(),
            status: RunStatus::Running,
            nodes,
            approved: HashSet::new(),
            feedback: HashMap::new(),
            feedback_kind: HashMap::new(),
            retries_used: 0,
            iterations: HashMap::new(),
            asked: HashMap::new(),
            pending: None,
            created,
            finished: None,
            waived: false,
            log: Vec::new(),
            log_total: 0,
            turns: Vec::new(),
        }
    }

    /// Dollars the run has spent on OpenRouter agents. Estimates for agents on your plan are not counted:
    /// nothing was charged for them.
    pub fn cost_usd(&self) -> f64 {
        self.overhead_usd + self.flow.nodes.iter().filter(|n| !n.uses_plan()).filter_map(|n| self.nodes.get(&n.id)).map(|n| n.cost_usd).sum::<f64>()
    }

    /// Firecrawl credits the run's agents have used on web tools.
    pub fn credits(&self) -> u64 {
        self.nodes.values().map(|n| n.credits).sum::<u64>() + self.references.iter().map(|r| r.credits).sum::<u64>()
    }

    /// Add a line to the run's log.
    pub fn note(&mut self, node: Option<&str>, kind: LogKind, title: impl Into<String>, body: Option<String>) {
        if self.log.len() >= MAX_LOG {
            self.log.remove(0);
        }
        let body = body.map(|b| {
            let n = b.chars().count();
            if n <= MAX_BODY {
                b
            } else {
                format!("{}\n… ({} more characters)", b.chars().take(MAX_BODY).collect::<String>(), n - MAX_BODY)
            }
        });
        self.log.push(LogEntry { at: now_secs(), node: node.map(String::from), kind, title: title.into(), body });
        self.log_total += 1;
    }

    /// Use by every node so far, and by the grilling.
    pub fn tokens(&self) -> Tokens {
        let mut t = self.overhead;
        for n in self.nodes.values() {
            t.add(&n.tokens);
        }
        t
    }

    /// After a restart, anything that was running did not finish.
    pub fn mark_interrupted(&mut self) {
        for n in self.nodes.values_mut() {
            if n.status == NodeStatus::Running {
                n.status = NodeStatus::Interrupted;
            }
        }
        if self.status == RunStatus::Running {
            self.status = RunStatus::Paused {
                pause: Pause::Failed { node: String::new(), message: "ttm restarted while this run was going".into() },
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orch::graph::{Budget, Engine, Node, Role};

    fn run() -> Run {
        let node = |id: &str| Node {
            id: id.into(),
            name: id.into(),
            role: Role::Researcher,
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
            effort: None,
            routing: crate::orch::graph::Routing::Pinned,
            ladder: Vec::new(),
            x: 0.0,
            y: 0.0,
        };
        let flow = Flow {
            id: "f".into(),
            name: "F".into(),
            nodes: vec![node("a"), node("b")],
            edges: vec![],
            max_retries: 2,
            budget: Budget::default(),
            downshift: false,
            dial: 2,
        };
        Run::new("r1".into(), flow, "brief".into(), 100)
    }

    #[test]
    fn a_runs_cost_is_what_openrouter_charged_for_its_agents_on_it() {
        let mut r = Run::new("r".into(), Flow { id: "f".into(), name: "f".into(), nodes: vec![], edges: vec![], max_retries: 1, budget: Default::default(), downshift: false, dial: 2 }, "g".into(), 0);
        assert_eq!(r.cost_usd(), 0.0);
        let mk = |id: &str, account: Option<&str>| Node { id: id.into(), name: id.into(), role: Role::Builder, custom: None, engine: Engine::Claude, model: "m".into(), account: account.map(String::from), direct: false, web: false, effort: None, routing: crate::orch::graph::Routing::Pinned, ladder: Vec::new(), system_prompt: String::new(), checkpoint: false, token_cap: None, keep_model: false, x: 0.0, y: 0.0 };
        r.flow.nodes = vec![mk("plan", None), mk("or1", Some("a")), mk("or2", Some("a"))];
        for (id, cost) in [("plan", 5.0), ("or1", 0.25), ("or2", 0.5)] {
            r.nodes.insert(id.into(), NodeRun { cost_usd: cost, ..NodeRun::new() });
        }
        assert!((r.cost_usd() - 0.75).abs() < 1e-12, "the estimate for the agent on your plan is not counted");
    }

    #[test]
    fn a_new_run_has_every_node_pending() {
        let r = run();
        assert_eq!(r.nodes.len(), 2);
        assert!(r.nodes.values().all(|n| n.status == NodeStatus::Pending && n.attempts == 0));
        assert_eq!(r.status, RunStatus::Running);
    }

    #[test]
    fn tokens_add_up_across_nodes() {
        let mut r = run();
        r.nodes.get_mut("a").unwrap().tokens = Tokens { input: 1, output: 2, cache_read: 3, cache_write: 4 };
        r.nodes.get_mut("b").unwrap().tokens = Tokens { input: 10, output: 20, cache_read: 30, cache_write: 40 };
        assert_eq!(r.tokens(), Tokens { input: 11, output: 22, cache_read: 33, cache_write: 44 });
    }

    #[test]
    fn a_restart_marks_running_nodes_interrupted_and_pauses_the_run() {
        let mut r = run();
        r.nodes.get_mut("a").unwrap().status = NodeStatus::Done;
        r.nodes.get_mut("b").unwrap().status = NodeStatus::Running;
        r.mark_interrupted();
        assert_eq!(r.nodes["a"].status, NodeStatus::Done);
        assert_eq!(r.nodes["b"].status, NodeStatus::Interrupted);
        assert!(matches!(r.status, RunStatus::Paused { .. }));
    }

    #[test]
    fn the_log_keeps_the_latest_entries_and_cuts_long_bodies() {
        let mut r = run();
        for i in 0..(MAX_LOG + 3) {
            r.note(None, LogKind::Event, format!("e{i}"), None);
        }
        assert_eq!(r.log.len(), MAX_LOG);
        assert_eq!(r.log[0].title, "e3");
        r.note(Some("a"), LogKind::Request, "long", Some("x".repeat(MAX_BODY + 10)));
        let body = r.log.last().unwrap().body.clone().unwrap();
        assert!(body.ends_with("… (10 more characters)"));
        assert_eq!(r.log.last().unwrap().node.as_deref(), Some("a"));
    }

    #[test]
    fn the_transcript_keeps_only_the_latest_lines() {
        let mut n = NodeRun::new();
        for i in 0..(MAX_TRANSCRIPT + 5) {
            n.log(format!("line {i}"));
        }
        assert_eq!(n.transcript.len(), MAX_TRANSCRIPT);
        assert_eq!(n.transcript[0], "line 5");
    }

    #[test]
    fn a_run_survives_a_json_round_trip() {
        let mut r = run();
        r.status = RunStatus::Paused { pause: Pause::Approval { node: "b".into() } };
        r.approved.insert("a".into());
        let back: Run = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }
}
