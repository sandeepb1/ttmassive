//! Sonnet writes the flow and each agent's system prompt; Opus grills the user. Both run as one-off
//! `claude -p` calls with no tools. The system prompts here are fixed text so they are cached; only
//! the user message changes.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::Outcome;
use super::claude::{self, Access};
use super::graph::{Budget, Edge, EdgeKind, Engine, Flow, Node, Role};
use super::run::{Qa, Question};
use super::starters;

pub const CLAUDE_MODELS: [&str; 3] = ["opus", "sonnet", "haiku"];
const MAX_SUGGESTED_NODES: usize = 10;
const MAX_QUESTIONS: usize = 6;
pub const MAX_ROUNDS: u8 = 3;

const SUGGEST_SYSTEM: &str = "You design small teams of AI agents. Given a goal, return a flow: the agents (nodes), \
which agent's output goes to which (edges), and a system prompt for each agent.\n\
Roles: orchestrator (plans, runs first), researcher (reads code and sources), search (finds files and facts \
broadly, cheap and fast), analyst (weighs findings, recommends), judge (reviews work and may send it back), builder \
(the only role that changes files).\n\
Engines: claude (models opus, sonnet, haiku) or codex (models are given with the goal).\n\
Rules: use as few agents as the goal needs, at most 8. Start with one orchestrator. Use the cheapest model that \
can do the job: opus only for planning and judging, sonnet or a small codex model for reading and searching. A \
builder is followed by a judge, and the judge's edge back to the builder is marked retry. Mark checkpoint true on \
the first agent after the orchestrator and on every builder, so the user approves the plan and approves before \
files change. Each system_prompt is 2 to 5 sentences: the agent's job, what to hand back, what not to do. Never put \
the user's goal in a system_prompt; the goal is given to every agent separately.";

const GRILL_SYSTEM: &str = "You are the orchestrator of a team of AI agents, about to start work for the user. Before \
you start, ask the user the questions whose answers would change what the team builds. Ask only what you cannot \
decide from the goal, the flow and sensible defaults. Each question must be answerable in a sentence and must come \
with the answer you would pick. Ask at most 6 questions in a round. When you know enough, or when told it is the \
last round, stop asking and write the brief: what to build, the decisions made, what is out of scope and how to \
tell it is done, in under 300 words.";

const NODE_PROMPT_SYSTEM: &str = "You write the system prompt for one AI agent in a team. Reply with 2 to 5 sentences: \
the agent's job, what to hand back, what not to do. Never put the user's goal in it; the goal is given separately.";

fn text() -> Value {
    json!({"type": "string"})
}

fn suggest_schema() -> Value {
    let role = json!({"type": "string", "enum": ["orchestrator", "researcher", "search", "analyst", "judge", "builder"]});
    let engine = json!({"type": "string", "enum": ["claude", "codex"]});
    json!({
        "type": "object", "additionalProperties": false, "required": ["name", "nodes", "edges"],
        "properties": {
            "name": text(),
            "nodes": {"type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "required": ["id", "name", "role", "engine", "model", "system_prompt", "checkpoint"],
                "properties": {"id": text(), "name": text(), "role": role, "engine": engine, "model": text(),
                               "system_prompt": text(), "checkpoint": {"type": "boolean"}}}},
            "edges": {"type": "array", "items": {
                "type": "object", "additionalProperties": false, "required": ["from", "to", "retry"],
                "properties": {"from": text(), "to": text(), "retry": {"type": "boolean"}}}},
        }
    })
}

fn grill_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["done", "questions", "brief"],
        "properties": {
            "done": {"type": "boolean"},
            "questions": {"type": "array", "items": {
                "type": "object", "additionalProperties": false, "required": ["text", "recommended"],
                "properties": {"text": text(), "recommended": text()}}},
            "brief": text(),
        }
    })
}

pub fn suggest_prompt(goal: &str, codex_models: &[String]) -> String {
    format!("# Goal\n\n{}\n\n# Codex models available\n\n{}\n", goal.trim(), codex_models.join(", "))
}

/// Turn Sonnet's suggestion into a flow we can run. Ids are renumbered so nothing odd from the model
/// reaches a file name, models it invented are replaced, and the result must pass `Flow::validate`.
pub fn flow_from_suggestion(v: &Value, flow_id: &str, codex_models: &[String]) -> Result<Flow> {
    let arr = |k: &str| v.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
    let raw_nodes = arr("nodes");
    if raw_nodes.is_empty() || raw_nodes.len() > MAX_SUGGESTED_NODES {
        bail!("the suggestion has {} agents; expected 1 to {MAX_SUGGESTED_NODES}", raw_nodes.len());
    }
    let mut ids: HashMap<String, String> = HashMap::new();
    let mut nodes = Vec::new();
    for (i, n) in raw_nodes.iter().enumerate() {
        let s = |k: &str| n.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let role: Role = serde_json::from_value(n.get("role").cloned().unwrap_or_default()).context("unknown role")?;
        let engine: Engine = serde_json::from_value(n.get("engine").cloned().unwrap_or_default()).context("unknown engine")?;
        let mut model = s("model");
        let known = match engine {
            Engine::Claude => CLAUDE_MODELS.iter().any(|m| *m == model),
            Engine::Codex => codex_models.contains(&model),
        };
        if !known {
            model = match engine {
                Engine::Claude => "sonnet".into(),
                Engine::Codex => codex_models.first().cloned().context("no Codex model to fall back to")?,
            };
        }
        let id = format!("n{}", i + 1);
        ids.insert(s("id"), id.clone());
        let prompt = s("system_prompt");
        nodes.push(Node {
            id,
            name: if s("name").is_empty() { format!("{role:?}") } else { s("name") },
            role,
            engine,
            model,
            system_prompt: if prompt.is_empty() { starters::default_prompt(role).into() } else { prompt },
            checkpoint: n.get("checkpoint").and_then(Value::as_bool).unwrap_or(false),
            token_cap: None,
            x: 0.0,
            y: 0.0,
        });
    }
    let mut edges = Vec::new();
    for e in arr("edges") {
        let end = |k: &str| e.get(k).and_then(Value::as_str).and_then(|x| ids.get(x)).cloned();
        let (Some(from), Some(to)) = (end("from"), end("to")) else { bail!("an edge points at an agent that does not exist") };
        let kind = if e.get("retry").and_then(Value::as_bool) == Some(true) { EdgeKind::Retry } else { EdgeKind::Forward };
        edges.push(Edge { id: format!("e{}", edges.len() + 1), from, to, kind });
    }
    let name = v.get("name").and_then(Value::as_str).unwrap_or("").trim();
    let mut flow = Flow {
        id: flow_id.into(),
        name: if name.is_empty() { "Suggested flow".into() } else { name.chars().take(60).collect() },
        nodes,
        edges,
        max_retries: 2,
        budget: Budget::default(),
    };
    flow.validate().map_err(anyhow::Error::msg).context("the suggested flow is not valid")?;
    lay_out(&mut flow);
    Ok(flow)
}

/// Left to right by when each agent can start, top to bottom within a column.
fn lay_out(flow: &mut Flow) {
    let Ok(layers) = flow.layers() else { return };
    for (col, layer) in layers.iter().enumerate() {
        for (row, id) in layer.iter().enumerate() {
            if let Some(n) = flow.nodes.iter_mut().find(|n| n.id == *id) {
                n.x = col as f64 * 300.0;
                n.y = row as f64 * 180.0;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum GrillStep {
    Questions(Vec<Question>),
    Brief(String),
}

pub fn grill_prompt(goal: &str, project: &str, flow: &Flow, history: &[Qa], round: u8, last: bool) -> String {
    let team: Vec<String> = flow.nodes.iter().map(|n| format!("- {} ({:?}, {})", n.name, n.role, n.model)).collect();
    let mut p = format!(
        "# Goal\n\n{}\n\n# Project folder\n\n{project}\n\nThe folder you are running in is a scratch folder, not the project: \
         ignore it. In the brief call this folder \"the project root\" and never mention the scratch folder.\n\n\
         # The team\n\n{}\n",
        goal.trim(),
        team.join("\n")
    );
    if !history.is_empty() {
        p.push_str("\n# Asked so far\n");
        for qa in history {
            p.push_str(&format!("\nQ: {}\nA: {}\n", qa.question, qa.answer));
        }
    }
    p.push_str(&format!("\n# Now\n\nThis is round {round} of {MAX_ROUNDS}. "));
    p.push_str(if last {
        "This is the last round: do not ask anything more. Set done to true and write the brief."
    } else {
        "Ask the next questions, or if you already know enough, set done to true and write the brief."
    });
    p
}

pub fn parse_grill(v: &Value, last: bool) -> Result<GrillStep> {
    let brief = v.get("brief").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let done = v.get("done").and_then(Value::as_bool).unwrap_or(false);
    let questions: Vec<Question> = v
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|q| {
            let text = q.get("text")?.as_str()?.trim().to_string();
            let recommended = q.get("recommended").and_then(Value::as_str).unwrap_or("").trim().to_string();
            (!text.is_empty()).then_some(Question { text, recommended })
        })
        .take(MAX_QUESTIONS)
        .collect();
    if (done || last || questions.is_empty()) && !brief.is_empty() {
        return Ok(GrillStep::Brief(brief));
    }
    if questions.is_empty() || last {
        bail!("the orchestrator gave neither questions nor a brief");
    }
    Ok(GrillStep::Questions(questions))
}

/// One no-tools call to Claude. Returns the structured answer and what it used.
async fn one_shot(model: &str, system: &str, prompt: &str, schema: Value, cwd: &Path) -> Result<(Value, Outcome)> {
    tokio::fs::create_dir_all(cwd).await?;
    let spec = claude::Spec {
        model: model.into(),
        system_prompt: system.into(),
        schema,
        access: Access::None,
        cwd: cwd.to_path_buf(),
        read_also: vec![],
    };
    let mut agent = claude::Agent::start(&spec).await?;
    let outcome = agent.turn(prompt, |_| {}).await;
    let _ = agent.finish().await;
    let outcome = outcome?;
    if outcome.is_error {
        bail!("claude reported an error: {}", outcome.text.chars().take(200).collect::<String>());
    }
    let answer = outcome.structured.clone().context("claude did not answer in the required shape")?;
    Ok((answer, outcome))
}

/// Sonnet designs a flow for a goal.
pub async fn suggest_flow(goal: &str, flow_id: &str, codex_models: &[String], cwd: &Path) -> Result<(Flow, Outcome)> {
    let (v, outcome) = one_shot("sonnet", SUGGEST_SYSTEM, &suggest_prompt(goal, codex_models), suggest_schema(), cwd).await?;
    Ok((flow_from_suggestion(&v, flow_id, codex_models)?, outcome))
}

/// Sonnet writes the system prompt for one agent.
pub async fn node_prompt(role: Role, name: &str, goal: &str, cwd: &Path) -> Result<(String, Outcome)> {
    let schema = json!({"type": "object", "additionalProperties": false, "required": ["system_prompt"],
                        "properties": {"system_prompt": text()}});
    let ask = format!("# Agent\n\n{name} ({role:?})\n\n# Overall goal, for context only\n\n{}\n", goal.trim());
    let (v, outcome) = one_shot("sonnet", NODE_PROMPT_SYSTEM, &ask, schema, cwd).await?;
    let p = v.get("system_prompt").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if p.is_empty() {
        bail!("empty prompt");
    }
    Ok((p, outcome))
}

/// Opus asks the next round of questions, or writes the brief.
pub async fn grill(
    goal: &str,
    project: &str,
    flow: &Flow,
    history: &[Qa],
    round: u8,
    cwd: &Path,
) -> Result<(GrillStep, Outcome)> {
    let last = round >= MAX_ROUNDS;
    let ask = grill_prompt(goal, project, flow, history, round, last);
    let (v, outcome) = one_shot("opus", GRILL_SYSTEM, &ask, grill_schema(), cwd).await?;
    Ok((parse_grill(&v, last)?, outcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> Vec<String> {
        vec!["gpt-6.1-sol".into(), "gpt-6-luna".into()]
    }

    fn suggestion() -> Value {
        json!({
            "name": "Login page",
            "nodes": [
                {"id": "plan", "name": "Planner", "role": "orchestrator", "engine": "claude", "model": "opus",
                 "system_prompt": "Plan it.", "checkpoint": false},
                {"id": "code", "name": "Builder", "role": "builder", "engine": "codex", "model": "gpt-6.1-sol",
                 "system_prompt": "", "checkpoint": true},
                {"id": "check", "name": "Judge", "role": "judge", "engine": "claude", "model": "opus",
                 "system_prompt": "Check it.", "checkpoint": false}
            ],
            "edges": [
                {"from": "plan", "to": "code", "retry": false},
                {"from": "code", "to": "check", "retry": false},
                {"from": "check", "to": "code", "retry": true}
            ]
        })
    }

    #[test]
    fn a_good_suggestion_becomes_a_valid_laid_out_flow() {
        let f = flow_from_suggestion(&suggestion(), "flow1", &models()).unwrap();
        assert_eq!(f.validate(), Ok(()));
        assert_eq!(f.nodes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), vec!["n1", "n2", "n3"]);
        assert_eq!(f.edges[2].kind, EdgeKind::Retry);
        assert_eq!((f.nodes[0].x, f.nodes[1].x, f.nodes[2].x), (0.0, 300.0, 600.0));
        assert!(f.nodes[1].checkpoint);
        // An empty prompt falls back to the role's default.
        assert_eq!(f.nodes[1].system_prompt, starters::default_prompt(Role::Builder));
    }

    #[test]
    fn invented_models_are_replaced() {
        let mut s = suggestion();
        s["nodes"][0]["model"] = json!("claude-ultra-9");
        s["nodes"][1]["model"] = json!("gpt-99");
        let f = flow_from_suggestion(&s, "f", &models()).unwrap();
        assert_eq!(f.nodes[0].model, "sonnet");
        assert_eq!(f.nodes[1].model, "gpt-6.1-sol");
    }

    #[test]
    fn a_suggestion_that_cannot_run_is_refused() {
        let mut s = suggestion();
        s["edges"][0]["to"] = json!("ghost");
        assert!(flow_from_suggestion(&s, "f", &models()).is_err());

        let mut s = suggestion();
        s["edges"][2]["from"] = json!("plan"); // a retry from a non-judge
        assert!(format!("{:#}", flow_from_suggestion(&s, "f", &models()).unwrap_err()).contains("judge"));

        let mut s = suggestion();
        s["nodes"] = json!([]);
        assert!(flow_from_suggestion(&s, "f", &models()).is_err());

        let mut s = suggestion();
        s["nodes"][0]["role"] = json!("wizard");
        assert!(flow_from_suggestion(&s, "f", &models()).is_err());
    }

    #[test]
    fn too_many_agents_are_refused() {
        let mut s = suggestion();
        let one = s["nodes"][0].clone();
        s["nodes"] = Value::Array(vec![one; MAX_SUGGESTED_NODES + 1]);
        assert!(flow_from_suggestion(&s, "f", &models()).unwrap_err().to_string().contains("expected"));
    }

    #[test]
    fn grilling_asks_questions_until_it_has_a_brief() {
        let qs = json!({"done": false, "brief": "", "questions": [
            {"text": "Which database?", "recommended": "SQLite"}, {"text": "  ", "recommended": "x"}]});
        assert_eq!(
            parse_grill(&qs, false).unwrap(),
            GrillStep::Questions(vec![Question { text: "Which database?".into(), recommended: "SQLite".into() }])
        );
        let done = json!({"done": true, "brief": "Build X.", "questions": []});
        assert_eq!(parse_grill(&done, false).unwrap(), GrillStep::Brief("Build X.".into()));
    }

    #[test]
    fn the_last_round_must_end_in_a_brief() {
        let more = json!({"done": false, "brief": "Build X.", "questions": [{"text": "More?", "recommended": ""}]});
        assert_eq!(parse_grill(&more, true).unwrap(), GrillStep::Brief("Build X.".into()));
        let nothing = json!({"done": false, "brief": "", "questions": [{"text": "More?", "recommended": ""}]});
        assert!(parse_grill(&nothing, true).is_err());
        assert!(parse_grill(&json!({"done": false, "brief": "", "questions": []}), false).is_err());
    }

    #[test]
    fn the_grill_prompt_carries_the_history_and_says_when_it_is_the_last_round() {
        let flow = starters::all().remove(0);
        let hist = vec![Qa { question: "Which database?".into(), answer: "SQLite".into() }];
        let p = grill_prompt("Add search", "/work/app", &flow, &hist, 2, false);
        assert!(p.contains("Add search") && p.contains("Q: Which database?") && p.contains("A: SQLite"));
        assert!(p.contains("round 2 of 3") && !p.contains("last round"));
        assert!(grill_prompt("Add search", "/work/app", &flow, &hist, 3, true).contains("last round"));
        assert!(p.contains("Builder"), "the team is listed so questions fit it");
        assert!(p.contains("/work/app") && p.contains("scratch folder"), "the real project is named, the scratch one disowned");
    }

    #[test]
    fn the_model_is_told_the_codex_models_it_may_use() {
        let p = suggest_prompt("Do a thing", &models());
        assert!(p.contains("Do a thing") && p.contains("gpt-6.1-sol, gpt-6-luna"));
    }

    #[test]
    fn both_schemas_are_closed_objects() {
        for s in [suggest_schema(), grill_schema()] {
            assert_eq!(s["additionalProperties"], false);
        }
    }
}
