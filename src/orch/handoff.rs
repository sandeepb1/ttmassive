//! What passes between agents. Each role must return JSON of a fixed shape, and a node is shown only
//! the outputs of the nodes wired into it, never their transcripts. The variable part of a prompt
//! (brief, inputs, feedback) comes after the role's fixed system prompt, which keeps the cache warm.

use serde_json::{Value, json};

use super::graph::{Node, Role};
use super::run::{NodeStatus, Run};

/// JSON Schema for the answer a role must give. Both CLIs enforce it.
pub fn schema_for(role: Role) -> Value {
    let obj = |props: Value, required: &[&str]| {
        json!({"type": "object", "properties": props, "required": required, "additionalProperties": false})
    };
    let text = json!({"type": "string"});
    let list = json!({"type": "array", "items": {"type": "string"}});
    match role {
        Role::Orchestrator => obj(json!({"plan": text, "steps": list}), &["plan", "steps"]),
        Role::Judge => obj(
            json!({"verdict": {"type": "string", "enum": ["approve", "reject"]}, "feedback": text}),
            &["verdict", "feedback"],
        ),
        Role::Builder => obj(
            json!({"summary": text, "files_changed": list, "notes": text}),
            &["summary", "files_changed", "notes"],
        ),
        Role::Researcher | Role::Search | Role::Analyst => {
            obj(json!({"summary": text, "findings": list, "sources": list}), &["summary", "findings", "sources"])
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Approve,
    Reject(String),
}

/// A judge's decision. An answer that is missing or unclear counts as a rejection, since letting
/// unchecked work through is the worse mistake.
pub fn verdict(output: &Value) -> Verdict {
    let feedback = output.get("feedback").and_then(Value::as_str).unwrap_or("").to_string();
    match output.get("verdict").and_then(Value::as_str) {
        Some("approve") => Verdict::Approve,
        _ => Verdict::Reject(if feedback.is_empty() { "The judge gave no reason.".into() } else { feedback }),
    }
}

/// The user prompt for a node's next turn.
pub fn build_prompt(run: &Run, node: &Node) -> String {
    let mut p = format!("# Brief\n\n{}\n", run.brief.trim());
    let inputs: Vec<_> = run
        .flow
        .inputs_of(&node.id)
        .into_iter()
        .filter_map(|id| {
            let from = run.flow.node(id)?;
            let n = run.nodes.get(id)?;
            if n.status != NodeStatus::Done {
                return None;
            }
            Some((from, n.output.as_ref()?))
        })
        .collect();
    if !inputs.is_empty() {
        p.push_str("\n# Inputs\n");
        for (from, out) in inputs {
            let body = serde_json::to_string_pretty(out).unwrap_or_default();
            p.push_str(&format!("\n## From {} ({:?})\n\n{body}\n", from.name, from.role));
        }
    }
    if let Some(f) = run.feedback.get(&node.id) {
        p.push_str(&format!("\n# Judge feedback\n\nYour previous work was sent back. Fix this:\n\n{}\n", f.trim()));
    }
    p.push_str(
        "\nDo your role's part and answer in the required JSON shape. Refer to files by paths relative to your \
         working folder, never by absolute path.",
    );
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orch::graph::{Budget, Edge, EdgeKind, Engine, Flow};

    fn node(id: &str, name: &str, role: Role) -> Node {
        Node {
            id: id.into(),
            name: name.into(),
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

    fn run() -> Run {
        let flow = Flow {
            id: "f".into(),
            name: "F".into(),
            nodes: vec![node("r", "Research", Role::Researcher), node("b", "Build", Role::Builder)],
            edges: vec![Edge { id: "e".into(), from: "r".into(), to: "b".into(), kind: EdgeKind::Forward }],
            max_retries: 2,
            budget: Budget::default(),
        };
        Run::new("run1".into(), flow, "Add a login page".into(), 0)
    }

    #[test]
    fn every_role_has_a_closed_object_schema() {
        for role in
            [Role::Orchestrator, Role::Researcher, Role::Search, Role::Analyst, Role::Judge, Role::Builder]
        {
            let s = schema_for(role);
            assert_eq!(s["type"], "object");
            assert_eq!(s["additionalProperties"], false);
            for key in s["required"].as_array().unwrap() {
                assert!(s["properties"].get(key.as_str().unwrap()).is_some());
            }
        }
        assert_eq!(schema_for(Role::Judge)["properties"]["verdict"]["enum"], json!(["approve", "reject"]));
    }

    #[test]
    fn the_first_node_sees_only_the_brief() {
        let r = run();
        let p = build_prompt(&r, r.flow.node("r").unwrap());
        assert!(p.contains("Add a login page"));
        assert!(!p.contains("# Inputs"));
    }

    #[test]
    fn a_node_sees_the_output_of_finished_upstream_nodes_only() {
        let mut r = run();
        let b = r.flow.node("b").unwrap().clone();
        // Upstream not finished: nothing to hand over.
        r.nodes.get_mut("r").unwrap().output = Some(json!({"summary": "stale"}));
        assert!(!build_prompt(&r, &b).contains("stale"));

        let n = r.nodes.get_mut("r").unwrap();
        n.status = NodeStatus::Done;
        n.output = Some(json!({"summary": "Use bcrypt"}));
        let p = build_prompt(&r, &b);
        assert!(p.contains("## From Research"));
        assert!(p.contains("Use bcrypt"));
    }

    #[test]
    fn feedback_goes_only_to_the_node_it_was_sent_to() {
        let mut r = run();
        r.feedback.insert("b".into(), "Tests are missing.".into());
        assert!(build_prompt(&r, r.flow.node("b").unwrap()).contains("Tests are missing."));
        assert!(!build_prompt(&r, r.flow.node("r").unwrap()).contains("Tests are missing."));
    }

    #[test]
    fn unclear_verdicts_count_as_rejections() {
        assert_eq!(verdict(&json!({"verdict": "approve", "feedback": ""})), Verdict::Approve);
        assert_eq!(
            verdict(&json!({"verdict": "reject", "feedback": "No tests"})),
            Verdict::Reject("No tests".into())
        );
        assert!(matches!(verdict(&json!({"verdict": "maybe"})), Verdict::Reject(_)));
        assert!(matches!(verdict(&json!({})), Verdict::Reject(_)));
    }
}
