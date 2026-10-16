//! Flows to start from, and the fixed system prompt each role begins with. The prompts are plain on
//! purpose: they are the same on every turn so the cache holds, and Sonnet can rewrite them per flow.

use super::graph::{Budget, Edge, EdgeKind, Engine, Flow, Node, Role};

pub fn default_prompt(role: Role) -> &'static str {
    super::roles::prompt(role)
}

fn node(id: &str, name: &str, role: Role, engine: Engine, model: &str, layer: usize, row: usize) -> Node {
    Node {
        id: id.into(),
        name: name.into(),
        role,
        custom: None,
        engine,
        model: model.into(),
        system_prompt: default_prompt(role).into(),
        checkpoint: false,
        token_cap: None,
        keep_model: false,
        account: None,
        direct: false,
        web: super::roles::web_default(role),
        effort: None,
        routing: crate::orch::graph::Routing::Pinned,
        ladder: Vec::new(),
        x: layer as f64 * 280.0,
        y: row as f64 * 180.0,
    }
}

fn edge(from: &str, to: &str, kind: EdgeKind) -> Edge {
    Edge::new(format!("{from}-{to}-{}", if kind == EdgeKind::Retry { "retry" } else { "next" }), from, to, kind)
}

/// A flow of these agents and edges. Nodes are (id, name, role, engine, model, column, row, asks first).
type Spec<'a> = (&'a str, &'a str, Role, Engine, &'a str, usize, usize, bool);

fn flow(id: &str, name: &str, specs: &[Spec], edges: &[(&str, &str, EdgeKind)], max_retries: u8, budget: Budget) -> Flow {
    Flow {
        id: id.into(),
        name: name.into(),
        nodes: specs
            .iter()
            .map(|&(nid, label, role, engine, model, col, row, asks)| {
                let mut n = node(nid, label, role, engine, model, col, row);
                n.checkpoint = asks;
                n
            })
            .collect(),
        edges: edges.iter().map(|&(a, b, k)| edge(a, b, k)).collect(),
        max_retries,
        budget,
        downshift: false,
        dial: 2,
    }
}

/// The five flow shapes from docs/research/multi-agent-roles.md. Most coding flows land at 3 to 6 agents, with one
/// writer, and a judge that can run the checks and send the work back.
pub fn all() -> Vec<Flow> {
    use EdgeKind::{Forward as F, Retry as R};
    let (claude, codex) = (Engine::Claude, Engine::Codex);
    let default = Budget::default();

    // Ask before the design and before files change: the plan is approved once the findings are in.
    let feature = flow(
        "starter-feature",
        "Feature",
        &[
            ("plan", "Planner", Role::Orchestrator, claude, "opus", 0, 1, false),
            ("find", "Search", Role::Search, codex, "gpt-6-luna", 1, 0, false),
            ("read", "Researcher", Role::Researcher, claude, "sonnet", 1, 2, false),
            ("design", "Architect", Role::Architect, claude, "opus", 2, 1, true),
            ("build", "Builder", Role::Builder, codex, "gpt-6.1-sol", 3, 1, true),
            ("check", "Judge", Role::Judge, claude, "opus", 4, 1, false),
        ],
        &[
            ("plan", "find", F),
            ("plan", "read", F),
            ("find", "design", F),
            ("read", "design", F),
            ("design", "build", F),
            ("build", "check", F),
            ("check", "build", R),
            ("check", "design", R),
        ],
        2,
        default,
    );

    // A plan that is questioned before anyone builds on it: the griller asks you what the plan leaves open, the planner
    // revises with your answers, and the loop ends when the griller has nothing left to ask.
    let grilled = flow(
        "starter-grilled-plan",
        "Grilled plan",
        &[
            ("plan", "Planner", Role::Orchestrator, claude, "opus", 0, 0, false),
            ("grill", "Griller", Role::Griller, claude, "opus", 1, 0, false),
            ("report", "Reporter", Role::Reporter, claude, "sonnet", 2, 0, false),
        ],
        &[("plan", "grill", F), ("grill", "report", F), ("grill", "plan", R)],
        3,
        default,
    );

    let bug = flow(
        "starter-bug-fix",
        "Bug fix",
        &[
            ("plan", "Planner", Role::Orchestrator, claude, "opus", 0, 0, false),
            ("debug", "Debugger", Role::Debugger, claude, "sonnet", 1, 0, true),
            ("build", "Builder", Role::Builder, codex, "gpt-6.1-sol", 2, 0, true),
            ("check", "Judge", Role::Judge, claude, "opus", 3, 0, false),
        ],
        &[("plan", "debug", F), ("debug", "build", F), ("build", "check", F), ("check", "build", R)],
        2,
        default,
    );

    let refactor = flow(
        "starter-refactor",
        "Refactor",
        &[
            ("plan", "Planner", Role::Orchestrator, claude, "opus", 0, 0, false),
            ("find", "Search", Role::Search, codex, "gpt-6-luna", 1, 0, false),
            ("design", "Architect", Role::Architect, claude, "opus", 2, 0, true),
            ("build", "Builder", Role::Builder, codex, "gpt-6.1-sol", 3, 0, true),
            ("check", "Judge", Role::Judge, claude, "opus", 4, 0, false),
        ],
        &[("plan", "find", F), ("find", "design", F), ("design", "build", F), ("build", "check", F), ("check", "build", R)],
        2,
        default,
    );

    // Two reviewers look at the same work through different lenses, in parallel, with nothing to send it back to.
    let mut review = flow(
        "starter-code-review",
        "Code review",
        &[
            ("plan", "Planner", Role::Orchestrator, claude, "opus", 0, 0, false),
            ("right", "Correctness review", Role::Judge, claude, "opus", 1, 0, false),
            ("safe", "Security and spec review", Role::Judge, claude, "opus", 1, 1, false),
            ("report", "Reporter", Role::Reporter, claude, "sonnet", 2, 0, false),
        ],
        &[("plan", "right", F), ("plan", "safe", F), ("right", "report", F), ("safe", "report", F)],
        0,
        default,
    );
    review.nodes[1].system_prompt.push_str(" Review for correctness only: logic errors, broken behaviour, missing cases and failing checks.");
    review.nodes[2].system_prompt.push_str(" Review for security problems (injection, unsafe input, leaked secrets) and for gaps against the stated requirements only.");

    let report = flow(
        "starter-research-report",
        "Research report",
        &[
            ("plan", "Planner", Role::Orchestrator, claude, "opus", 0, 1, false),
            ("find", "Search", Role::Search, codex, "gpt-6-luna", 1, 0, false),
            ("read-a", "Researcher", Role::Researcher, claude, "sonnet", 1, 1, false),
            ("read-b", "Researcher 2", Role::Researcher, claude, "sonnet", 1, 2, false),
            ("weigh", "Analyst", Role::Analyst, claude, "sonnet", 2, 1, false),
            ("report", "Reporter", Role::Reporter, claude, "sonnet", 3, 1, false),
        ],
        &[
            ("plan", "find", F),
            ("plan", "read-a", F),
            ("plan", "read-b", F),
            ("find", "weigh", F),
            ("read-a", "weigh", F),
            ("read-b", "weigh", F),
            ("weigh", "report", F),
        ],
        1,
        Budget { claude_pct: 10.0, codex_pct: 5.0, stop_window_pct: 85.0, run_usd: None, run_credits: None },
    );

    vec![feature, grilled, bug, refactor, review, report]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orch::roles::Capability;

    #[test]
    fn every_starter_is_a_valid_flow() {
        for f in all() {
            assert_eq!(f.validate(), Ok(()), "{}", f.name);
        }
    }

    #[test]
    fn starter_ids_are_unique() {
        let ids: Vec<_> = all().into_iter().map(|f| f.id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
    }

    #[test]
    fn a_starter_follows_the_research_rules_of_thumb() {
        for f in all() {
            let writers = f.nodes.iter().filter(|n| n.writes()).count();
            assert!(writers <= 1, "{}: one writer per piece of work", f.name);
            assert!(f.nodes.len() <= 6, "{}: over six agents needs a written reason", f.name);
            assert!(f.max_retries <= 3, "{}: cap send-back loops at 3", f.name);
            // A judge may have a second place to send work back to (a wrong design goes to the architect), no more.
            let retries = f.edges.iter().filter(|e| e.kind == EdgeKind::Retry).count();
            assert!(retries <= 2, "{}: at most the code and the design", f.name);
            // Whatever changes files asks first.
            for n in f.nodes.iter().filter(|n| n.writes()) {
                assert!(n.checkpoint, "{}: {} changes files and must ask first", f.name, n.name);
            }
        }
    }

    #[test]
    fn a_flow_that_writes_has_a_checker_that_can_run_things_and_a_flow_that_does_not_has_no_send_back() {
        for f in all() {
            let writes = f.nodes.iter().any(|n| n.writes());
            let judge = f.nodes.iter().find(|n| n.decides() && n.capability() == Capability::Inspect);
            if writes {
                assert!(judge.is_some(), "{}: nobody checks the change", f.name);
                assert!(f.edges.iter().any(|e| e.kind == EdgeKind::Retry), "{}: no way to send it back", f.name);
            } else {
                // Only an agent that questions the work, not one that judges it, may loop back in a flow that changes nothing.
                let from_asker = |e: &Edge| f.node(&e.from).is_some_and(|n| n.asks());
                assert!(f.edges.iter().all(|e| e.kind == EdgeKind::Forward || from_asker(e)), "{}: nothing to send back to", f.name);
            }
        }
    }

    #[test]
    fn the_grilled_plan_asks_the_user_and_loops_back_to_the_planner() {
        let f = all().into_iter().find(|f| f.id == "starter-grilled-plan").expect("the starter exists");
        let back = f.edges.iter().find(|e| e.kind == EdgeKind::Retry).expect("a way back");
        assert_eq!((back.from.as_str(), back.to.as_str()), ("grill", "plan"));
        assert_eq!((back.answered_by, back.max_iterations), (crate::orch::graph::Answerer::User, 2), "the user answers, a couple of rounds");
        assert!(f.nodes.iter().all(|n| !n.writes()), "it only plans: nothing to run checks on");
        assert!(f.nodes.iter().any(|n| n.asks()));
    }

    #[test]
    fn a_wrong_design_can_be_sent_back_to_the_architect_as_well_as_wrong_code_to_the_builder() {
        let f = &all()[0];
        assert_eq!(f.send_back_targets("check"), vec!["build", "design"]);
        assert_eq!(f.validate(), Ok(()));
    }

    #[test]
    fn the_builder_waits_for_approval_and_so_does_the_design() {
        let f = &all()[0];
        assert!(f.node("build").unwrap().checkpoint);
        assert!(f.node("design").unwrap().checkpoint, "the plan is approved once the findings are in");
    }

    #[test]
    fn the_judge_and_the_builder_are_different_models() {
        // A judge favours its own style, so it should not be the same model as the one that wrote the work.
        for f in all() {
            let Some(b) = f.nodes.iter().find(|n| n.writes()) else { continue };
            let j = f.nodes.iter().find(|n| n.decides()).unwrap();
            assert!(b.engine != j.engine || b.model != j.model, "{}", f.name);
        }
    }

    #[test]
    fn every_starter_model_is_one_the_app_offers() {
        for f in all() {
            for n in &f.nodes {
                let ok = match n.engine {
                    Engine::Claude => ["opus", "sonnet", "haiku"].contains(&n.model.as_str()),
                    Engine::Codex => n.model.starts_with("gpt-6"),
                };
                assert!(ok, "{}: {} on {}", f.name, n.name, n.model);
            }
        }
    }

    #[test]
    fn the_review_lenses_are_different() {
        let f = all().into_iter().find(|f| f.id == "starter-code-review").unwrap();
        assert_ne!(f.node("right").unwrap().system_prompt, f.node("safe").unwrap().system_prompt);
        assert!(f.node("safe").unwrap().system_prompt.contains("security"));
    }
}
