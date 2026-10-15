//! What a flow is: agents (nodes), the artifacts that pass between them (edges), and the checks a
//! flow must pass before it can run. A flow is a template; a run of it lives in `run.rs`.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

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
}

impl Role {
    /// Roles that only read: no file writes, no worktree.
    pub fn read_only(self) -> bool {
        !matches!(self, Role::Builder)
    }
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
    pub engine: Engine,
    pub model: String,
    /// Written by Sonnet and editable. Identical on every turn so the prompt cache holds.
    pub system_prompt: String,
    /// Stop and wait for approval before this node starts.
    #[serde(default)]
    pub checkpoint: bool,
    /// Most tokens (input, output, cache writes) this node may use. `None` means no node cap.
    #[serde(default)]
    pub token_cap: Option<u64>,
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

fn forward() -> EdgeKind {
    EdgeKind::Forward
}

/// Share of each plan-limit window a single run may use.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Budget {
    pub claude_pct: f64,
    pub codex_pct: f64,
    /// Stop the run when the whole window (your own sessions included) passes this.
    pub stop_window_pct: f64,
}

impl Default for Budget {
    fn default() -> Self {
        Self { claude_pct: 15.0, codex_pct: 10.0, stop_window_pct: 85.0 }
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
    #[serde(default)]
    pub budget: Budget,
}

fn default_retries() -> u8 {
    2
}

pub fn id_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_ID && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
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
        }

        let mut edge_ids = HashSet::new();
        for e in &self.edges {
            if !id_ok(&e.id) || !edge_ids.insert(e.id.as_str()) {
                return Err(format!("edge id '{}' is invalid or repeated", e.id));
            }
            let (Some(from), Some(to)) = (self.node(&e.from), self.node(&e.to)) else {
                return Err(format!("edge '{}' points at a node that does not exist", e.id));
            };
            if e.from == e.to {
                return Err(format!("edge '{}' joins a node to itself", e.id));
            }
            if e.kind == EdgeKind::Retry {
                if from.role != Role::Judge {
                    return Err("only a judge can send work back".into());
                }
                if to.role.read_only() {
                    return Err("a judge can only send work back to a builder".into());
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
            return Err("the flow has a loop; only a judge's retry edge may point backwards".into());
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
            if self.node(n).is_some_and(|x| !x.role.read_only()) {
                return Some(n);
            }
            queue.extend(self.inputs_of(n));
        }
        None
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
    fn a_forward_loop_is_refused() {
        let mut f = line();
        f.edges.push(edge("e5", "judge", "res", EdgeKind::Forward));
        assert!(f.validate().unwrap_err().contains("loop"));
    }

    #[test]
    fn only_a_judge_may_retry_and_only_into_a_builder() {
        let mut f = line();
        f.edges[3] = edge("e4", "res", "build", EdgeKind::Retry);
        assert!(f.validate().unwrap_err().contains("judge"));

        let mut f = line();
        f.edges[3] = edge("e4", "judge", "res", EdgeKind::Retry);
        assert!(f.validate().unwrap_err().contains("builder"));
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
