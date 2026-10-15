//! Flows to start from, and the fixed system prompt each role begins with. The prompts are plain on
//! purpose: they are the same on every turn so the cache holds, and Sonnet can rewrite them per flow.

use super::graph::{Budget, Edge, EdgeKind, Engine, Flow, Node, Role};

pub fn default_prompt(role: Role) -> &'static str {
    match role {
        Role::Orchestrator => {
            "You are the orchestrator. Turn the brief into a short, concrete plan the other agents can follow. \
             Name the steps in order and say what done looks like. Do not do the work yourself."
        }
        Role::Researcher => {
            "You are a researcher. Find out what is needed to do the task well, from the code in the working \
             folder and from reliable sources. Report findings and where they came from. Do not change files."
        }
        Role::Search => {
            "You are a search agent. Locate the files, symbols and facts the task touches, quickly and \
             broadly. Report exact locations. Do not change files."
        }
        Role::Analyst => {
            "You are an analyst. Weigh the findings you are given, point out risks and gaps, and recommend one \
             approach with reasons. Do not change files."
        }
        Role::Judge => {
            "You are a strict reviewer. Check the work against the brief and the plan. Approve only if it is \
             correct and complete; otherwise reject and say exactly what to fix."
        }
        Role::Builder => {
            "You are the builder. Do the work described by the plan and findings, in the working folder. Keep \
             changes small and focused, run the project's checks, and report what you changed."
        }
    }
}

fn node(id: &str, name: &str, role: Role, engine: Engine, model: &str, layer: usize, row: usize) -> Node {
    Node {
        id: id.into(),
        name: name.into(),
        role,
        engine,
        model: model.into(),
        system_prompt: default_prompt(role).into(),
        checkpoint: false,
        token_cap: None,
        keep_model: false,
        x: layer as f64 * 280.0,
        y: row as f64 * 180.0,
    }
}

fn edge(from: &str, to: &str, kind: EdgeKind) -> Edge {
    Edge { id: format!("{from}-{to}-{}", if kind == EdgeKind::Retry { "retry" } else { "next" }), from: from.into(), to: to.into(), kind }
}

pub fn all() -> Vec<Flow> {
    // Plan first, then research, then the builder; the judge can send the builder back.
    let mut build = Flow {
        id: "starter-research-build-review".into(),
        name: "Research, build, review".into(),
        nodes: vec![
            node("plan", "Planner", Role::Orchestrator, Engine::Claude, "opus", 0, 0),
            node("research", "Researcher", Role::Researcher, Engine::Claude, "sonnet", 1, 0),
            node("build", "Builder", Role::Builder, Engine::Codex, "gpt-6.1-sol", 2, 0),
            node("review", "Judge", Role::Judge, Engine::Claude, "opus", 3, 0),
        ],
        edges: vec![
            edge("plan", "research", EdgeKind::Forward),
            edge("research", "build", EdgeKind::Forward),
            edge("build", "review", EdgeKind::Forward),
            edge("review", "build", EdgeKind::Retry),
        ],
        max_retries: 2,
        budget: Budget::default(),
        downshift: false,
    };
    // Approve the plan before anything starts, and approve again before files change.
    build.nodes[1].checkpoint = true;
    build.nodes[2].checkpoint = true;

    let report = Flow {
        id: "starter-research-report".into(),
        name: "Research and report".into(),
        nodes: vec![
            node("plan", "Planner", Role::Orchestrator, Engine::Claude, "opus", 0, 1),
            node("search", "Searcher", Role::Search, Engine::Codex, "gpt-6-luna", 1, 0),
            node("research", "Researcher", Role::Researcher, Engine::Claude, "sonnet", 1, 2),
            node("analyse", "Analyst", Role::Analyst, Engine::Claude, "sonnet", 2, 1),
            node("review", "Judge", Role::Judge, Engine::Claude, "opus", 3, 1),
        ],
        edges: vec![
            edge("plan", "search", EdgeKind::Forward),
            edge("plan", "research", EdgeKind::Forward),
            edge("search", "analyse", EdgeKind::Forward),
            edge("research", "analyse", EdgeKind::Forward),
            edge("analyse", "review", EdgeKind::Forward),
        ],
        max_retries: 1,
        budget: Budget { claude_pct: 10.0, codex_pct: 5.0, stop_window_pct: 85.0 },
        downshift: false,
    };
    vec![build, report]
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_builder_waits_for_approval_in_the_first_starter() {
        let f = &all()[0];
        assert!(f.node("build").unwrap().checkpoint);
        assert!(f.node("research").unwrap().checkpoint, "the plan is approved first");
    }
}
