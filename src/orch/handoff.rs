//! What passes between agents. Each role must return JSON of a fixed shape, and a node is shown only
//! the outputs of the nodes wired into it, never their transcripts. The variable part of a prompt
//! (brief, inputs, feedback) comes after the role's fixed system prompt, which keeps the cache warm.

use serde_json::{Value, json};

use super::graph::Node;
use super::roles::Output;
use super::run::{NodeStatus, Run};

/// JSON Schema for the answer an agent must give, by what it hands back. Both CLIs enforce it. A verdict from an
/// agent that can send work back to more than one place must also say which: `send_back_to` is one of `targets`, or
/// `none` when it approves.
pub fn schema_for(output: Output, targets: &[&str]) -> Value {
    let obj = |props: Value, required: &[&str]| {
        json!({"type": "object", "properties": props, "required": required, "additionalProperties": false})
    };
    let text = json!({"type": "string"});
    let list = json!({"type": "array", "items": {"type": "string"}});
    match output {
        Output::Plan => obj(json!({"plan": text, "steps": list}), &["plan", "steps"]),
        Output::Verdict if targets.len() >= 2 => {
            let mut ids: Vec<&str> = targets.to_vec();
            ids.push("none");
            obj(
                json!({"verdict": {"type": "string", "enum": ["approve", "reject"]}, "feedback": text,
                       "send_back_to": {"type": "string", "enum": ids}}),
                &["verdict", "feedback", "send_back_to"],
            )
        }
        Output::Verdict => obj(
            json!({"verdict": {"type": "string", "enum": ["approve", "reject"]}, "feedback": text}),
            &["verdict", "feedback"],
        ),
        Output::Change => obj(
            json!({"summary": text, "files_changed": list, "notes": text}),
            &["summary", "files_changed", "notes"],
        ),
        Output::Report => {
            obj(json!({"summary": text, "findings": list, "sources": list}), &["summary", "findings", "sources"])
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Approve,
    /// Work is not good enough: what is wrong, and where it said to send it, if it said.
    Reject { feedback: String, send_back_to: Option<String> },
}

/// A judge's decision. An answer that is missing or unclear counts as a rejection, since letting
/// unchecked work through is the worse mistake.
pub fn verdict(output: &Value) -> Verdict {
    let feedback = output.get("feedback").and_then(Value::as_str).unwrap_or("").to_string();
    match output.get("verdict").and_then(Value::as_str) {
        Some("approve") => Verdict::Approve,
        _ => Verdict::Reject {
            feedback: if feedback.is_empty() { "The judge gave no reason.".into() } else { feedback },
            send_back_to: output.get("send_back_to").and_then(Value::as_str).filter(|s| *s != "none" && !s.is_empty()).map(String::from),
        },
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
            p.push_str(&format!("\n## From {} ({})\n\n{body}\n", from.name, from.role_label()));
        }
    }
    let back = run.flow.send_back_targets(&node.id);
    if node.decides() && back.len() >= 2 {
        p.push_str("\n# Where you can send work back to\n\n");
        for id in &back {
            if let Some(n) = run.flow.node(id) {
                p.push_str(&format!("- `{id}`: {} ({})\n", n.name, n.role_label()));
            }
        }
        p.push_str(
            "\nIf you reject, set `send_back_to` to the id of the agent that should redo its work: the earliest one whose \
             output was wrong (the design if the design was wrong, otherwise whoever made the change). Use `none` if you approve.\n",
        );
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
    use crate::orch::graph::{Budget, Edge, EdgeKind, Engine, Flow, Role};

    fn node(id: &str, name: &str, role: Role) -> Node {
        Node {
            id: id.into(),
            name: name.into(),
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

    fn run() -> Run {
        let flow = Flow {
            id: "f".into(),
            name: "F".into(),
            nodes: vec![node("r", "Research", Role::Researcher), node("b", "Build", Role::Builder)],
            edges: vec![Edge { id: "e".into(), from: "r".into(), to: "b".into(), kind: EdgeKind::Forward }],
            max_retries: 2,
            budget: Budget::default(),
            downshift: false,
        };
        Run::new("run1".into(), flow, "Add a login page".into(), 0)
    }

    #[test]
    fn every_role_has_a_closed_object_schema() {
        for output in [Output::Plan, Output::Report, Output::Change, Output::Verdict] {
            let s = schema_for(output, &[]);
            assert_eq!(s["type"], "object");
            assert_eq!(s["additionalProperties"], false);
            for key in s["required"].as_array().unwrap() {
                assert!(s["properties"].get(key.as_str().unwrap()).is_some());
            }
        }
        assert_eq!(schema_for(Output::Verdict, &[])["properties"]["verdict"]["enum"], json!(["approve", "reject"]));
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
            Verdict::Reject { feedback: "No tests".into(), send_back_to: None }
        );
        assert!(matches!(verdict(&json!({"verdict": "maybe"})), Verdict::Reject { .. }));
        assert!(matches!(verdict(&json!({})), Verdict::Reject { .. }));
    }

    #[test]
    fn a_verdict_that_can_go_two_ways_must_say_which() {
        let one = schema_for(Output::Verdict, &["build"]);
        assert!(one["properties"].get("send_back_to").is_none(), "one place to go needs no answer");
        let two = schema_for(Output::Verdict, &["design", "build"]);
        assert_eq!(two["properties"]["send_back_to"]["enum"], json!(["design", "build", "none"]));
        assert!(two["required"].as_array().unwrap().contains(&json!("send_back_to")));
        assert_eq!(two["additionalProperties"], false);
        // Only a verdict has it.
        assert!(schema_for(Output::Report, &["a", "b"])["properties"].get("send_back_to").is_none());
    }

    #[test]
    fn the_verdict_names_where_it_went_and_none_means_nowhere() {
        let v = |j: Value| verdict(&j);
        assert_eq!(
            v(json!({"verdict": "reject", "feedback": "Wrong design", "send_back_to": "design"})),
            Verdict::Reject { feedback: "Wrong design".into(), send_back_to: Some("design".into()) }
        );
        assert!(matches!(v(json!({"verdict": "reject", "feedback": "x", "send_back_to": "none"})), Verdict::Reject { send_back_to: None, .. }));
        assert!(matches!(v(json!({"verdict": "reject", "feedback": "x"})), Verdict::Reject { send_back_to: None, .. }));
        assert_eq!(v(json!({"verdict": "approve", "feedback": "", "send_back_to": "none"})), Verdict::Approve);
    }

    #[test]
    fn a_judge_with_two_places_to_go_is_told_what_they_are() {
        let mut r = run(); // r -> b, both Researcher and Builder
        r.flow.nodes.push(node("j", "Judge", Role::Judge));
        r.nodes.insert("j".into(), crate::orch::run::Run::new("x".into(), r.flow.clone(), "g".into(), 0).nodes.remove("j").unwrap());
        for (i, to) in ["r", "b"].iter().enumerate() {
            r.flow.edges.push(Edge { id: format!("x{i}"), from: "j".into(), to: (*to).into(), kind: EdgeKind::Retry });
        }
        let p = build_prompt(&r, r.flow.node("j").unwrap());
        assert!(p.contains("Where you can send work back to") && p.contains("`r`: Research") && p.contains("`b`: Build"), "{p}");
        assert!(p.contains("send_back_to") && p.contains("earliest one whose output was wrong"));
        // Nothing of the kind for an agent with one place to go, or none.
        assert!(!build_prompt(&r, r.flow.node("b").unwrap()).contains("send back"));
        r.flow.edges.pop();
        assert!(!build_prompt(&r, r.flow.node("j").unwrap()).contains("Where you can send work back to"));
    }
}
