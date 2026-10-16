//! What a flow is: agents (nodes), the artifacts that pass between them (edges), and the checks a
//! flow must pass before it can run. A flow is a template; a run of it lives in `run.rs`.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::roles::{self, Capability, Output};

const MAX_NODES: usize = 24;
const MAX_EDGES: usize = 64;
const MAX_ID: usize = 40;
const MAX_NAME: usize = 60;
const MAX_PROMPT: usize = 16_000;
const MAX_RETRIES: u8 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Orchestrator,
    Researcher,
    Search,
    Analyst,
    Judge,
    Builder,
    Architect,
    Debugger,
    Reporter,
    /// A role the user wrote: its label, capability and result shape are on the node (`Node::custom`).
    Custom,
}

impl Role {
    /// What a built-in role may do. A custom role says so itself, on the node.
    pub fn capability(self) -> Capability {
        roles::def_for(self).map_or(Capability::Read, |d| d.capability)
    }

    pub fn output(self) -> Output {
        roles::def_for(self).map_or(Output::Report, |d| d.output)
    }
}

/// What makes a custom role what it is. Kept on the node so a saved flow carries it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomRole {
    pub label: String,
    pub capability: Capability,
    pub output: Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Claude,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    /// The sender's output is handed to the receiver.
    Forward,
    /// A judge sending work back to an earlier node. Followed at most `max_retries` times.
    Retry,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    pub name: String,
    pub role: Role,
    /// Present exactly when `role` is `custom`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<CustomRole>,
    pub engine: Engine,
    pub model: String,
    /// The OpenRouter account this agent runs on, when `model` is an OpenRouter model id. The CLI that
    /// runs it is still `engine`; it reaches OpenRouter through ttm's proxy instead of your own login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Call the model itself, with no CLI and so no tools: it sees only the brief and what earlier agents handed it.
    /// The only way an OpenAI-compatible endpoint can run an agent; for an agent that only has to think, it is also the cheap way.
    #[serde(default, skip_serializing_if = "is_false")]
    pub direct: bool,
    /// Written by Sonnet and editable. Identical on every turn so the prompt cache holds.
    pub system_prompt: String,
    /// Stop and wait for approval before this node starts.
    #[serde(default)]
    pub checkpoint: bool,
    /// Most tokens (input, output, cache writes) this node may use. `None` means no node cap.
    #[serde(default)]
    pub token_cap: Option<u64>,
    /// Never switch this agent to a cheaper model, whatever the plan windows say.
    #[serde(default)]
    pub keep_model: bool,
    /// Canvas position, kept so a saved flow opens the way it was laid out.
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub id: String,
    pub from: String,
    pub to: String,
    #[serde(default = "forward")]
    pub kind: EdgeKind,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn forward() -> EdgeKind {
    EdgeKind::Forward
}

/// The plan limits: the share of each window a single run may use, and the line no window may pass. One set for
/// the whole app, changed any time and applied to runs that are going.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Budget {
    pub claude_pct: f64,
    pub codex_pct: f64,
    /// Stop the run when the whole window (your own sessions included) passes this.
    pub stop_window_pct: f64,
    /// Most dollars a run may spend on OpenRouter, or `None` for no cap.
    #[serde(default)]
    pub run_usd: Option<f64>,
}

impl Budget {
    pub fn validate(&self) -> Result<(), String> {
        let pct = |v: f64| v.is_finite() && v > 0.0 && v <= 100.0;
        if !(pct(self.claude_pct) && pct(self.codex_pct) && pct(self.stop_window_pct)) {
            return Err("limits must be above 0 and at most 100".into());
        }
        match self.run_usd {
            Some(c) if !(c.is_finite() && c > 0.0 && c < 1.0e6) => Err("the dollar cap must be more than zero".into()),
            _ => Ok(()),
        }
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self { claude_pct: 15.0, codex_pct: 10.0, stop_window_pct: 85.0, run_usd: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flow {
    pub id: String,
    pub name: String,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    #[serde(default = "default_retries")]
    pub max_retries: u8,
    /// Older flows carry their own limits. They are no longer used: the limits are the same for every flow.
    #[serde(default)]
    pub budget: Budget,
    /// Give agents that have not started yet a cheaper model as the plan windows fill up. Never a dearer one.
    #[serde(default)]
    pub downshift: bool,
}

fn default_retries() -> u8 {
    2
}

pub fn id_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_ID && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Node {
    /// Whether this agent draws on your Claude or Codex plan. One on OpenRouter is paid per token instead,
    /// so the plan windows say nothing about it.
    pub fn uses_plan(&self) -> bool {
        self.account.is_none()
    }

    /// What this agent may do.
    pub fn capability(&self) -> Capability {
        self.custom.as_ref().map_or_else(|| self.role.capability(), |c| c.capability)
    }

    /// The shape of the answer it hands back.
    pub fn output(&self) -> Output {
        self.custom.as_ref().map_or_else(|| self.role.output(), |c| c.output)
    }

    pub fn writes(&self) -> bool {
        self.capability() == Capability::Write
    }

    /// Hands back a decision, and so may send work back to an earlier agent.
    pub fn decides(&self) -> bool {
        self.output() == Output::Verdict
    }

    /// The name to show: a custom role's own, else the built-in role's.
    pub fn role_label(&self) -> String {
        match (&self.custom, roles::def_for(self.role)) {
            (Some(c), _) => c.label.clone(),
            (None, Some(d)) => d.label.to_string(),
            (None, None) => "Agent".to_string(),
        }
    }
}

impl Flow {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Check a flow from the page before it is stored or run.
    pub fn validate(&self) -> Result<(), String> {
        if !id_ok(&self.id) {
            return Err("flow id must be letters, digits, - or _".into());
        }
        if self.name.trim().is_empty() || self.name.len() > MAX_NAME {
            return Err("flow name must be 1 to 60 characters".into());
        }
        if self.nodes.is_empty() || self.nodes.len() > MAX_NODES {
            return Err(format!("a flow needs 1 to {MAX_NODES} nodes"));
        }
        if self.edges.len() > MAX_EDGES {
            return Err("too many edges".into());
        }
        if self.max_retries > MAX_RETRIES {
            return Err(format!("retries must be at most {MAX_RETRIES}"));
        }
        let b = &self.budget;
        let pct = |v: f64| v.is_finite() && v > 0.0 && v <= 100.0;
        if !pct(b.claude_pct) || !pct(b.codex_pct) || !pct(b.stop_window_pct) {
            return Err("budget percentages must be above 0 and at most 100".into());
        }

        let mut seen = HashSet::new();
        for n in &self.nodes {
            if !id_ok(&n.id) || !seen.insert(n.id.as_str()) {
                return Err(format!("node id '{}' is invalid or repeated", n.id));
            }
            if n.name.trim().is_empty() || n.name.len() > MAX_NAME {
                return Err(format!("node '{}' needs a name of 1 to 60 characters", n.id));
            }
            if n.model.trim().is_empty() {
                return Err(format!("node '{}' has no model", n.name));
            }
            if n.system_prompt.len() > MAX_PROMPT {
                return Err(format!("node '{}' has too long a prompt", n.name));
            }
            if !n.x.is_finite() || !n.y.is_finite() {
                return Err(format!("node '{}' has a bad position", n.name));
            }
            if n.token_cap == Some(0) {
                return Err(format!("node '{}' token cap must be above 0", n.name));
            }
            match (&n.role, &n.custom) {
                (Role::Custom, None) => return Err(format!("node '{}' is a custom role with no definition", n.name)),
                (Role::Custom, Some(c)) if c.label.trim().is_empty() || c.label.len() > MAX_NAME => {
                    return Err(format!("node '{}' has a custom role name of 1 to {MAX_NAME} characters", n.name));
                }
                (r, Some(_)) if *r != Role::Custom => {
                    return Err(format!("node '{}' has a custom definition but is not a custom role", n.name));
                }
                _ => {}
            }
            if n.direct {
                if n.account.is_none() {
                    return Err(format!("node '{}' calls its model directly, so it needs an account", n.name));
                }
                if n.capability() != super::roles::Capability::Read {
                    return Err(format!("node '{}' changes files or runs commands, which needs an agent: it cannot call its model directly", n.name));
                }
            }
        }

        let mut edge_ids = HashSet::new();
        for e in &self.edges {
            if !id_ok(&e.id) || !edge_ids.insert(e.id.as_str()) {
                return Err(format!("edge id '{}' is invalid or repeated", e.id));
            }
            let (Some(from), Some(_)) = (self.node(&e.from), self.node(&e.to)) else {
                return Err(format!("edge '{}' points at a node that does not exist", e.id));
            };
            if e.from == e.to {
                return Err(format!("edge '{}' joins a node to itself", e.id));
            }
            if e.kind == EdgeKind::Retry {
                if !from.decides() {
                    return Err("only an agent that hands back a verdict can send work back".into());
                }
                if self.edges.iter().filter(|x| x.kind == EdgeKind::Retry && x.from == e.from && x.to == e.to).count() > 1 {
                    return Err("two send-back edges join the same pair of agents".into());
                }
            }
        }

        let layers = self.layers()?;
        let position: HashMap<&str, usize> =
            layers.iter().enumerate().flat_map(|(i, l)| l.iter().map(move |id| (id.as_str(), i))).collect();
        for e in self.edges.iter().filter(|e| e.kind == EdgeKind::Retry) {
            if position[e.to.as_str()] >= position[e.from.as_str()] {
                return Err("a retry edge must lead back to a node that runs earlier".into());
            }
        }
        Ok(())
    }

    /// Nodes grouped by when they can start: every node in a layer depends only on earlier layers.
    /// Retry edges are not dependencies. Errors on a cycle among forward edges.
    pub fn layers(&self) -> Result<Vec<Vec<String>>, String> {
        let mut indegree: HashMap<&str, usize> = self.nodes.iter().map(|n| (n.id.as_str(), 0)).collect();
        for e in self.edges.iter().filter(|e| e.kind == EdgeKind::Forward) {
            if let Some(d) = indegree.get_mut(e.to.as_str()) {
                *d += 1;
            }
        }
        let mut layers = Vec::new();
        let mut placed = 0;
        loop {
            let mut ready: Vec<&str> =
                self.nodes.iter().map(|n| n.id.as_str()).filter(|id| indegree.get(id) == Some(&0)).collect();
            if ready.is_empty() {
                break;
            }
            for id in &ready {
                indegree.remove(id);
            }
            for e in self.edges.iter().filter(|e| e.kind == EdgeKind::Forward) {
                if ready.contains(&e.from.as_str()) {
                    if let Some(d) = indegree.get_mut(e.to.as_str()) {
                        *d -= 1;
                    }
                }
            }
            placed += ready.len();
            ready.sort_unstable();
            layers.push(ready.into_iter().map(String::from).collect());
        }
        if placed != self.nodes.len() {
            return Err("the flow has a loop; only an agent that gives a verdict may send work back".into());
        }
        Ok(layers)
    }

    /// The closest builder upstream of `id`: where the work it should look at lives. Searches
    /// breadth-first back along forward edges, so a nearer builder wins over a farther one.
    pub fn nearest_writer(&self, id: &str) -> Option<&str> {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut queue: std::collections::VecDeque<&str> = self.inputs_of(id).into();
        while let Some(n) = queue.pop_front() {
            if !seen.insert(n) {
                continue;
            }
            if self.node(n).is_some_and(Node::writes) {
                return Some(n);
            }
            queue.extend(self.inputs_of(n));
        }
        None
    }

    /// Every node on a forward path from `from` to `to`, both included: what a judge sending work back
    /// to `from` makes redo. A parallel branch that merely sits between them in run order is not on one.
    pub fn between(&self, from: &str, to: &str) -> Vec<&str> {
        fn reach<'a>(flow: &'a Flow, start: &'a str, forward: bool) -> HashSet<&'a str> {
            let mut seen: HashSet<&str> = HashSet::new();
            let mut stack = vec![start];
            while let Some(n) = stack.pop() {
                if !seen.insert(n) {
                    continue;
                }
                for e in flow.edges.iter().filter(|e| e.kind == EdgeKind::Forward) {
                    if forward && e.from == n {
                        stack.push(e.to.as_str());
                    } else if !forward && e.to == n {
                        stack.push(e.from.as_str());
                    }
                }
            }
            seen
        }
        let (Some(a), Some(b)) = (self.node(from), self.node(to)) else { return Vec::new() };
        let down = reach(self, a.id.as_str(), true);
        let up = reach(self, b.id.as_str(), false);
        self.nodes.iter().map(|n| n.id.as_str()).filter(|id| down.contains(id) && up.contains(id)).collect()
    }

    /// Where `id` can send work back to, in edge order.
    pub fn send_back_targets(&self, id: &str) -> Vec<&str> {
        self.edges.iter().filter(|e| e.kind == EdgeKind::Retry && e.from == id).map(|e| e.to.as_str()).collect()
    }

    /// Ids of nodes whose output feeds `id`, in edge order.
    pub fn inputs_of(&self, id: &str) -> Vec<&str> {
        self.edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Forward && e.to == id)
            .map(|e| e.from.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_agent_that_needs_no_tools_may_call_its_model_directly() {
        let mut f = Flow { id: "f".into(), name: "f".into(), nodes: vec![node("a", Role::Orchestrator)], edges: vec![], max_retries: 1, budget: Budget::default(), downshift: false };
        f.nodes[0].direct = true;
        assert!(f.validate().unwrap_err().contains("needs an account"));
        f.nodes[0].account = Some("acct".into());
        assert_eq!(f.validate(), Ok(()), "a reader of the brief is fine");
        for role in [Role::Builder, Role::Judge, Role::Debugger] {
            f.nodes[0].role = role;
            assert!(f.validate().unwrap_err().contains("cannot call its model directly"), "{role:?} needs real tools");
        }
        f.nodes[0].role = Role::Researcher;
        assert_eq!(f.validate(), Ok(()));
    }

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
            x: 0.0,
            y: 0.0,
        }
    }

    fn edge(id: &str, from: &str, to: &str, kind: EdgeKind) -> Edge {
        Edge { id: id.into(), from: from.into(), to: to.into(), kind }
    }

    fn line() -> Flow {
        Flow {
            id: "f".into(),
            name: "Line".into(),
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

    #[test]
    fn straight_line_with_a_retry_is_valid() {
        let f = line();
        assert_eq!(f.validate(), Ok(()));
        let layers = f.layers().unwrap();
        assert_eq!(layers, vec![vec!["orch"], vec!["res"], vec!["build"], vec!["judge"]]);
    }

    #[test]
    fn branches_share_a_layer() {
        let mut f = line();
        f.nodes.push(node("search", Role::Search));
        f.edges.push(edge("e5", "orch", "search", EdgeKind::Forward));
        f.edges.push(edge("e6", "search", "build", EdgeKind::Forward));
        let layers = f.layers().unwrap();
        assert_eq!(layers[1], vec!["res", "search"]);
        assert_eq!(f.inputs_of("build"), vec!["res", "search"]);
    }

    #[test]
    fn a_node_finds_the_builder_whose_work_it_should_look_at() {
        let f = line();
        assert_eq!(f.nearest_writer("judge"), Some("build"));
        assert_eq!(f.nearest_writer("build"), None, "a builder has none upstream");
        assert_eq!(f.nearest_writer("res"), None);
    }

    #[test]
    fn the_nearest_builder_wins_and_cycles_do_not_hang() {
        let mut f = line();
        f.nodes.push(node("build2", Role::Builder));
        f.nodes.push(node("an", Role::Analyst));
        f.edges.push(edge("e5", "judge", "build2", EdgeKind::Forward));
        f.edges.push(edge("e6", "build2", "an", EdgeKind::Forward));
        assert_eq!(f.nearest_writer("an"), Some("build2"));
        // The retry edge is not a dependency, so it must not be followed.
        assert_eq!(f.nearest_writer("build"), None);
    }

    #[test]
    fn between_is_the_nodes_on_a_path_not_the_ones_beside_it() {
        let f = line();
        assert_eq!(f.between("build", "judge"), vec!["build", "judge"]);
        assert_eq!(f.between("res", "judge"), vec!["res", "build", "judge"]);
        // A branch that hangs off the builder but does not lead to the judge is not between them.
        let mut g = line();
        g.nodes.push(node("aside", Role::Analyst));
        g.edges.push(edge("e5", "build", "aside", EdgeKind::Forward));
        assert_eq!(g.between("build", "judge"), vec!["build", "judge"]);
        assert!(g.between("judge", "build").is_empty(), "no forward path backwards");
    }

    #[test]
    fn a_forward_loop_is_refused() {
        let mut f = line();
        f.edges.push(edge("e5", "judge", "res", EdgeKind::Forward));
        assert!(f.validate().unwrap_err().contains("loop"));
    }

    #[test]
    fn only_an_agent_that_gives_a_verdict_may_send_work_back() {
        let mut f = line();
        f.edges[3] = edge("e4", "res", "build", EdgeKind::Retry);
        assert!(f.validate().unwrap_err().contains("verdict"));
    }

    #[test]
    fn work_can_be_sent_back_to_any_earlier_agent_not_only_one_that_writes() {
        // A failed design goes back to the planner, or the code goes back to the builder.
        let mut f = line();
        f.edges[3] = edge("e4", "judge", "res", EdgeKind::Retry);
        assert_eq!(f.validate(), Ok(()));
        f.edges.push(edge("e5", "judge", "build", EdgeKind::Retry));
        assert_eq!(f.validate(), Ok(()), "a judge may have several places to send work back to");
        assert_eq!(f.send_back_targets("judge"), vec!["res", "build"]);
        assert!(f.send_back_targets("build").is_empty());
    }

    #[test]
    fn work_cannot_be_sent_forward_or_twice_to_the_same_agent() {
        let mut f = line();
        f.nodes.push(node("late", Role::Analyst));
        f.edges.push(edge("e5", "judge", "late", EdgeKind::Forward));
        f.edges.push(edge("e6", "judge", "late", EdgeKind::Retry));
        assert!(f.validate().unwrap_err().contains("earlier"));
        let mut g = line();
        g.edges.push(edge("e5", "judge", "build", EdgeKind::Retry));
        assert!(g.validate().unwrap_err().contains("same pair"), "{:?}", g.validate());
    }

    #[test]
    fn a_custom_role_is_judged_by_what_it_may_do_not_by_its_name() {
        use crate::orch::roles::{Capability, Output};
        let custom = |cap, out| Some(CustomRole { label: "Security reviewer".into(), capability: cap, output: out });
        // A custom reviewer that hands back a verdict can send work back, exactly like the built-in judge.
        let mut f = line();
        f.nodes[3].role = Role::Custom;
        f.nodes[3].custom = custom(Capability::Inspect, Output::Verdict);
        assert_eq!(f.validate(), Ok(()));
        assert!(f.nodes[3].decides() && !f.nodes[3].writes());
        // One that only reports cannot.
        f.nodes[3].custom = custom(Capability::Inspect, Output::Report);
        assert!(f.validate().unwrap_err().contains("verdict"));
        // A custom writer is a writer: it gets a worktree and is a valid place to send work back to.
        let mut g = line();
        g.nodes[2].role = Role::Custom;
        g.nodes[2].custom = custom(Capability::Write, Output::Change);
        assert_eq!(g.validate(), Ok(()));
        assert!(g.nodes[2].writes());
        assert_eq!(g.nodes[2].role_label(), "Security reviewer");
    }

    #[test]
    fn a_custom_role_needs_its_definition_and_only_it() {
        use crate::orch::roles::{Capability, Output};
        let mut f = line();
        f.nodes[1].role = Role::Custom;
        assert!(f.validate().unwrap_err().contains("no definition"));
        f.nodes[1].custom = Some(CustomRole { label: " ".into(), capability: Capability::Read, output: Output::Report });
        assert!(f.validate().unwrap_err().contains("custom role name"));
        let mut g = line();
        g.nodes[1].custom = Some(CustomRole { label: "X".into(), capability: Capability::Read, output: Output::Report });
        assert!(g.validate().unwrap_err().contains("not a custom role"));
    }

    #[test]
    fn a_retry_must_point_backwards() {
        let mut f = line();
        f.nodes.push(node("late", Role::Builder));
        f.edges.push(edge("e5", "judge", "late", EdgeKind::Forward));
        f.edges.push(edge("e6", "judge", "late", EdgeKind::Retry));
        assert!(f.validate().unwrap_err().contains("earlier"));
    }

    #[test]
    fn bad_input_is_refused() {
        let mut f = line();
        f.edges[0].to = "ghost".into();
        assert!(f.validate().unwrap_err().contains("does not exist"));

        let mut f = line();
        f.nodes[1].id = "orch".into();
        assert!(f.validate().is_err());

        let mut f = line();
        f.max_retries = 9;
        assert!(f.validate().is_err());

        let mut f = line();
        f.budget.claude_pct = 0.0;
        assert!(f.validate().is_err());

        let mut f = line();
        f.nodes[0].token_cap = Some(0);
        assert!(f.validate().is_err());
    }

    #[test]
    fn json_round_trip_uses_the_page_field_names() {
        let json = serde_json::to_value(line()).unwrap();
        assert_eq!(json["maxRetries"], 2);
        assert_eq!(json["nodes"][0]["systemPrompt"], "");
        assert_eq!(json["edges"][3]["kind"], "retry");
        let back: Flow = serde_json::from_value(json).unwrap();
        assert_eq!(back, line());
    }
}
