//! The roles an agent can be given. Each is a fixed system prompt plus two properties that decide how the
//! rest of the system treats it: what it may do, and what shape of answer it hands back. Every rule keys on
//! those two, never on a role's name, so a custom role (written by the user, with AI-drafted instructions)
//! behaves exactly like a built-in one with the same properties.

use serde::{Deserialize, Serialize};

use super::graph::{Engine, Role};

/// What an agent may do. Enforced by the operating system or the agent's own sandbox, not by its prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    /// Read and search files and the web. Cannot run commands or change anything.
    Read,
    /// Read, and run commands (tests, `git diff`) in a sandbox that cannot change a file or read outside the folder.
    Inspect,
    /// Change files, in its own git worktree on its own branch.
    Write,
}

/// The shape of the one JSON answer an agent hands back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Output {
    /// A plan: what to do and the steps.
    Plan,
    /// Findings: a summary, the findings, and where they came from.
    Report,
    /// A change: what was done, the files touched, notes.
    Change,
    /// A decision: approve or reject, with feedback. May send work back to an earlier agent.
    Verdict,
}

/// Where a role sits in a flow, for grouping in the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Plan,
    Gather,
    Build,
    Check,
    Deliver,
}

/// A role, as the page needs it to offer and start one.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleDef {
    pub id: Role,
    pub label: &'static str,
    pub blurb: &'static str,
    pub stage: Stage,
    /// An icon name the page knows (see web/src/icons.ts).
    pub icon: &'static str,
    pub capability: Capability,
    pub output: Output,
    /// Frequently needed: shown first in the palette.
    pub core: bool,
    /// Ask the user before it starts (it changes files).
    pub checkpoint: bool,
    /// The engine to start it on if the model is available, then the model for each engine.
    pub prefer: Engine,
    pub claude_model: &'static str,
    pub codex_model: Option<&'static str>,
    pub prompt: &'static str,
}

const fn def(
    id: Role,
    label: &'static str,
    blurb: &'static str,
    stage: Stage,
    icon: &'static str,
    capability: Capability,
    output: Output,
    core: bool,
    prompt: &'static str,
) -> RoleDef {
    RoleDef {
        id,
        label,
        blurb,
        stage,
        icon,
        capability,
        output,
        core,
        checkpoint: matches!(capability, Capability::Write),
        prefer: Engine::Claude,
        claude_model: "sonnet",
        codex_model: None,
        prompt,
    }
}

/// Every built-in role, in the order the palette shows them: by stage, then within it. The set and the default
/// instructions come from docs/research/multi-agent-roles.md, adapted to how ttm runs a flow: the user draws
/// the flow, so the orchestrator plans the work rather than the team, and where work is sent back to is fixed by
/// the edge, not named by the judge.
pub fn catalog() -> Vec<RoleDef> {
    vec![
        RoleDef {
            claude_model: "opus",
            ..def(
                Role::Orchestrator,
                "Orchestrator",
                "Plans the work",
                Stage::Plan,
                "workflow",
                Capability::Read,
                Output::Plan,
                true,
                "You are the orchestrator. Turn the brief into the smallest plan that gets the work done. Name the steps in order, say which can run at the same time, what each must hand back, and what done looks like. Write every step so the agent doing it needs nothing else: give its inputs in full and a boundary on what it must not do. Do not do the work yourself and do not add a step whose result no later step uses.",
            )
        },
        RoleDef {
            claude_model: "opus",
            ..def(
                Role::Architect,
                "Architect",
                "Turns findings into a file-level plan",
                Stage::Plan,
                "route",
                Capability::Read,
                Output::Plan,
                false,
                "You turn findings into an implementation plan for someone who will write the code and cannot ask you questions. List the files to change, the change in each, the order, the interfaces that must not change, and the exact checks that prove it works. Name the risks and what you assumed. Do not write the code, and do not widen the scope beyond what your inputs ask for.",
            )
        },
        RoleDef {
            prefer: Engine::Codex,
            claude_model: "haiku",
            codex_model: Some("gpt-6-luna"),
            ..def(
                Role::Search,
                "Search",
                "Finds files and facts, fast",
                Stage::Gather,
                "search",
                Capability::Read,
                Output::Report,
                true,
                "You answer one narrow question about a codebase or a set of documents. Return a short list of findings, each with a file path and line range or a quoted fragment, and say plainly when something was not found. Stop as soon as the question is answered. Do not summarise whole files, do not propose changes, and do not search beyond the scope you were given.",
            )
        },
        def(
            Role::Researcher,
            "Researcher",
            "Reads sources and the web, with citations",
            Stage::Gather,
            "book",
            Capability::Read,
            Output::Report,
            false,
            "You research one question using primary sources: official documentation, the project's own repository, original papers. Return findings as claims, each with the source and the exact passage relied on. Say what you could not verify. Prefer fewer, better sources over many weak ones, and stop when the question is answered. Do not give recommendations, and do not state anything you did not see in a source.",
        ),
        // Sonnet, not the strong tier the research suggested for it: measured on one analyst task, Opus cost 2.2x as much
        // ($0.059 vs $0.027) and reached the same recommendation. The research marked this its weakest-evidenced role.
        def(
            Role::Analyst,
            "Analyst",
            "Weighs reports against each other",
            Stage::Gather,
            "scale",
            Capability::Read,
            Output::Report,
            false,
            "You compare the reports you are given. Identify where they agree, where they conflict, and what is missing. For each decision the reports bear on, rank the options with the reason, and state your confidence. Quote the report each point came from. Do not do new research, do not invent facts that are not in your inputs, and do not write code.",
        ),
        def(
            Role::Debugger,
            "Debugger",
            "Reproduces a failure and finds its cause",
            Stage::Gather,
            "bug",
            Capability::Inspect,
            Output::Report,
            false,
            "You find why something fails. Reproduce the failure first and record the exact command and its output. Narrow it with the smallest experiments you can run, then report the root cause, the evidence for it (commands and output), and the files involved. If you cannot reproduce it, say so and what you tried. You can run read-only commands only. Do not edit files, and do not propose a fix beyond one sentence.",
        ),
        RoleDef {
            prefer: Engine::Codex,
            codex_model: Some("gpt-6.1-sol"),
            ..def(
                Role::Builder,
                "Builder",
                "The one that changes files",
                Stage::Build,
                "hammer",
                Capability::Write,
                Output::Change,
                true,
                "You implement the change described in your inputs, in your own worktree. Follow the plan and the existing patterns in the code, run the checks you were given, and fix failures you caused. Hand back a summary of what changed and why, the files touched, and the output of the checks you ran. Do not change anything outside the stated scope, do not skip or weaken tests to make them pass, and say clearly when you could not finish part of the task.",
            )
        },
        RoleDef {
            claude_model: "opus",
            ..def(
                Role::Judge,
                "Judge",
                "Checks the work, and can send it back",
                Stage::Check,
                "gavel",
                Capability::Inspect,
                Output::Verdict,
                true,
                "You decide whether the work meets the criteria you are given. Read the change and the plan, then run the checks yourself (tests, build, git diff) and base your verdict on what you observe, not on the author's claims. Treat test results as one input, not the verdict: tests can pass a wrong change or fail a right one, so prefer the project's own existing tests over any the author just added. Return approve or reject. On reject, list each problem with its evidence and what must change. Flag only problems that affect correctness or the stated criteria; style preferences and extra features are not reasons to reject. Do not fix problems yourself, and do not reject twice for an issue that has been addressed.",
            )
        },
        def(
            Role::Reporter,
            "Reporter",
            "Writes the final result",
            Stage::Deliver,
            "read",
            Capability::Read,
            Output::Report,
            false,
            "You write the final result from the material you are given. Lead with the answer, then the evidence, then the open questions. Keep every claim traceable to an input and keep its source attached. Say plainly what is uncertain or unverified. Do not add new findings, do not research, and do not soften a rejected or failed result.",
        ),
    ]
}

pub fn def_for(role: Role) -> Option<RoleDef> {
    catalog().into_iter().find(|d| d.id == role)
}

/// The system prompt a new agent of this role starts with.
pub fn prompt(role: Role) -> &'static str {
    def_for(role).map_or("", |d| d.prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_role_has_a_unique_id_and_the_text_the_page_shows() {
        let c = catalog();
        let mut ids: Vec<_> = c.iter().map(|d| format!("{:?}", d.id)).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), c.len(), "an id is used twice");
        for d in &c {
            assert!(!d.label.is_empty() && !d.blurb.is_empty() && !d.icon.is_empty(), "{:?}", d.id);
            assert!(d.prompt.len() > 60 && d.prompt.len() < 1200, "{:?} prompt length", d.id);
        }
        assert!(!c.iter().any(|d| d.id == Role::Custom), "a custom role is defined by the user, not here");
    }

    #[test]
    fn a_role_that_changes_files_asks_first() {
        for d in catalog() {
            assert_eq!(d.checkpoint, d.capability == Capability::Write, "{:?}", d.id);
        }
    }

    #[test]
    fn only_a_builder_style_role_writes_and_a_verdict_never_does() {
        for d in catalog() {
            if d.output == Output::Verdict {
                assert_ne!(d.capability, Capability::Write, "{:?}: a reviewer must not change what it reviews", d.id);
            }
            if d.capability == Capability::Write {
                assert_eq!(d.output, Output::Change, "{:?}: a writer reports a change", d.id);
            }
        }
    }

    #[test]
    fn a_prompt_never_carries_the_users_goal_or_a_placeholder() {
        for d in catalog() {
            for bad in ["{goal}", "{{", "TODO", "<goal>"] {
                assert!(!d.prompt.contains(bad), "{:?} contains {bad}", d.id);
            }
        }
    }

    #[test]
    fn the_catalog_serialises_for_the_page() {
        let v = serde_json::to_value(catalog()).unwrap();
        assert_eq!(v[0]["id"], "orchestrator");
        assert_eq!(v[0]["capability"], "read");
        assert_eq!(v[0]["claudeModel"], "opus");
        assert!(v[0].get("codexModel").is_some());
    }

    #[test]
    fn the_catalog_is_the_researched_nine_and_keeps_the_original_six() {
        let ids: Vec<String> = catalog().iter().map(|d| format!("{:?}", d.id).to_lowercase()).collect();
        for original in ["orchestrator", "researcher", "search", "analyst", "judge", "builder"] {
            assert!(ids.iter().any(|i| i == original), "{original} must stay so saved flows keep working");
        }
        for added in ["architect", "debugger", "reporter"] {
            assert!(ids.iter().any(|i| i == added), "{added}");
        }
        assert_eq!(ids.len(), 9, "{ids:?}");
    }

    #[test]
    fn there_is_exactly_one_kind_of_writer_and_checkers_can_run_things_but_not_change_them() {
        let c = catalog();
        let writers: Vec<_> = c.iter().filter(|d| d.capability == Capability::Write).map(|d| d.id).collect();
        assert_eq!(writers, vec![Role::Builder], "parallel writers conflict (docs/research/multi-agent-roles.md)");
        let judge = c.iter().find(|d| d.id == Role::Judge).unwrap();
        assert_eq!(judge.capability, Capability::Inspect, "a judge may run checks: the evidence that this beats reading alone is mixed (docs/research/judge-evidence.md), but it is the only way to see a test fail");
        let debugger = c.iter().find(|d| d.id == Role::Debugger).unwrap();
        assert_eq!(debugger.capability, Capability::Inspect);
    }

    #[test]
    fn the_core_roles_are_the_four_almost_every_flow_needs() {
        let mut core: Vec<_> = catalog().iter().filter(|d| d.core).map(|d| d.label).collect();
        core.sort();
        assert_eq!(core, vec!["Builder", "Judge", "Orchestrator", "Search"]);
    }

    #[test]
    fn planning_and_judging_get_a_strong_model_and_searching_a_cheap_one() {
        let model = |r: Role| catalog().into_iter().find(|d| d.id == r).unwrap().claude_model;
        assert_eq!(model(Role::Orchestrator), "opus");
        assert_eq!(model(Role::Judge), "opus", "a weak judge approves weak work");
        assert_eq!(model(Role::Search), "haiku");
        assert_ne!(model(Role::Builder), "haiku");
        assert_eq!(model(Role::Analyst), "sonnet", "measured: opus cost 2.2x for the same answer");
    }

    #[test]
    fn the_judge_is_told_not_to_reject_for_style_and_to_weigh_test_results_not_obey_them() {
        let p = prompt(Role::Judge);
        assert!(p.contains("style preferences") && p.contains("run the checks yourself"), "{p}");
        assert!(p.contains("one input, not the verdict") && p.contains("project's own existing tests"), "{p}");
    }

    #[test]
    fn stages_come_in_flow_order_so_the_palette_reads_left_to_right() {
        let order = |s: Stage| match s {
            Stage::Plan => 0,
            Stage::Gather => 1,
            Stage::Build => 2,
            Stage::Check => 3,
            Stage::Deliver => 4,
        };
        let stages: Vec<u8> = catalog().iter().map(|d| order(d.stage)).collect();
        assert!(stages.windows(2).all(|w| w[0] <= w[1]), "{stages:?}");
    }
}
