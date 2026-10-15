//! Sonnet writes the flow and each agent's system prompt; Opus grills the user. Both run as one-off
//! `claude -p` calls with no tools. The system prompts here are fixed text so they are cached; only
//! the user message changes.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::Outcome;
use super::claude::{self, Access};
use super::graph::{Budget, CustomRole, Edge, EdgeKind, Engine, Flow, Node, Role};
use super::roles::{Capability, Output};
use super::run::{Qa, Question};
use super::starters;

pub const CLAUDE_MODELS: [&str; 3] = ["opus", "sonnet", "haiku"];
const MAX_SUGGESTED_NODES: usize = 10;
const MAX_QUESTIONS: usize = 6;
pub const MAX_ROUNDS: u8 = 3;

fn role_id(r: Role) -> String {
    format!("{r:?}").to_lowercase()
}

/// The roles on offer, one line each, for the model that designs a flow. Built from the catalog so a role added
/// there is offered here without anyone remembering to.
fn role_lines() -> String {
    super::roles::catalog()
        .iter()
        .map(|d| {
            let can = match d.capability {
                Capability::Read => "reads only",
                Capability::Inspect => "reads and runs checks in a sandbox",
                Capability::Write => "the only kind that changes files",
            };
            let gives = match d.output {
                Output::Plan => "hands back a plan",
                Output::Report => "hands back a report",
                Output::Change => "hands back a change",
                Output::Verdict => "hands back approve or reject, and may send work back",
            };
            format!("- {} ({}): {}. {can}; {gives}.", role_id(d.id), d.label, d.blurb.to_lowercase())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Rules from docs/research/multi-agent-roles.md: few agents, one writer, a checker that can run things, and a
/// cap on how many roles a task needs.
fn suggest_system() -> String {
    format!(
        "You design small teams of AI agents. Given a goal, return a flow: the agents (nodes), which agent's output \
goes to which (edges), and a system prompt for each agent.\n\
Roles:\n{}\n\
Engines: claude (models opus, sonnet, haiku) or codex (models are given with the goal).\n\
{DESIGN_RULES}",
        role_lines()
    )
}

/// How a good team is built, from the research. Shared by the designer and the editor.
const DESIGN_RULES: &str = "Rules: use as few agents as the goal needs. Most flows are 3 to 5 agents; over 6 needs a real reason. Start with one \
orchestrator. Add an agent only for a different kind of work (reading, running checks, changing files), a clean-context \
check, or a part that can run in parallel with another. Agents that read can run at the same time; never have two agents \
change the same files, one agent that changes files per piece of work. Anything that changes files is followed by a judge \
that can run the checks, and the judge's edge back to that agent is marked retry. If a plan or design could be wrong, the \
judge may also have a retry edge to that earlier agent (the architect, say). A flow that changes nothing has no retry edge. Use the cheapest model that can do the job: opus for the orchestrator, architect and \
judge; sonnet (or a small codex model) for researchers, debuggers and reporters; haiku or a small codex model for simple \
lookups. Use a different model for the judge than for the agent whose work it checks. Mark checkpoint true on every agent \
that changes files, so the user approves before files change. Each system_prompt is 2 to 5 sentences: the agent's job, \
what to hand back, what not to do. Never put the user's goal in a system_prompt; the goal is given to every agent separately.";

const GRILL_SYSTEM: &str = "You are the orchestrator of a team of AI agents, about to start work for the user. Before \
you start, ask the user the questions whose answers would change what the team builds. Ask only what you cannot \
decide from the goal, the flow and sensible defaults. Each question must be answerable in a sentence and must come \
with the answer you would pick. Give each question two to four short, distinct options for the user to choose from \
(a few words each, mutually exclusive unless multiple is true), and put the one you would pick in recommended, \
word for word the same as one of the options. The user can always type a different answer, so never add an option like \
\"other\" or \"something else\". Set multiple to true only for a question where several options can apply together \
(for example which features matter most). Ask at most 6 questions in a round. When you know enough, or when told it is the \
last round, stop asking and write the brief: what to build, the decisions made, what is out of scope and how to \
tell it is done, in under 300 words.";

const NODE_PROMPT_SYSTEM: &str = "You write the system prompt for one AI agent in a team. Reply with 2 to 5 sentences: \
the agent's job, what to hand back, what not to do. Never put the user's goal in it; the goal is given separately.";

fn text() -> Value {
    json!({"type": "string"})
}

fn suggest_schema() -> Value {
    let ids: Vec<String> = super::roles::catalog().iter().map(|d| role_id(d.id)).collect();
    let role = json!({"type": "string", "enum": ids});
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
                "type": "object", "additionalProperties": false, "required": ["text", "options", "recommended", "multiple"],
                "properties": {"text": text(), "options": {"type": "array", "items": text()}, "recommended": text(),
                               "multiple": {"type": "boolean"}}}},
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
            custom: None,
            engine,
            model,
            system_prompt: if prompt.is_empty() { starters::default_prompt(role).into() } else { prompt },
            checkpoint: n.get("checkpoint").and_then(Value::as_bool).unwrap_or(false),
            token_cap: None,
            keep_model: false,
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
        downshift: false,
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
                n.x = col as f64 * 280.0;
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

const MAX_OPTIONS: usize = 4;
const MAX_OPTION: usize = 140;

/// The options the model offered, made fit to show: trimmed, no blanks or repeats, a sensible length, at most four,
/// and always containing the recommended answer (added first if the model left it out). Fewer than two is not a
/// choice, so those questions get none and the page asks for text.
fn clean_options(raw: Option<&Value>, recommended: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |s: &str| {
        let s: String = s.trim().chars().take(MAX_OPTION).collect();
        if !s.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&s)) {
            out.push(s);
        }
    };
    if !recommended.is_empty() {
        add(recommended);
    }
    for o in raw.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        add(o);
    }
    out.truncate(MAX_OPTIONS);
    if out.len() < 2 {
        out.clear();
    }
    out
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
            let options = clean_options(q.get("options"), &recommended);
            let multiple = options.len() >= 2 && q.get("multiple").and_then(Value::as_bool).unwrap_or(false);
            (!text.is_empty()).then_some(Question { text, recommended, options, multiple })
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
    let (v, outcome) = one_shot("sonnet", &suggest_system(), &suggest_prompt(goal, codex_models), suggest_schema(), cwd).await?;
    Ok((flow_from_suggestion(&v, flow_id, codex_models)?, outcome))
}

// ---------------------------------------------------------------------------- editing a flow from a request

fn edit_system() -> String {
    format!(
        "You change a team of AI agents to match what the user asks. You are given the team as it is now (agents and \
the edges between them) and the user's request. Return the whole team as it should be afterwards, and a summary.\n\
Roles:\n{}\n\
Engines: claude (models opus, sonnet, haiku) or codex (models are given with the request). An agent whose role is \
\"custom\" is one the user wrote: keep it exactly as given unless asked to change it, and never create a new one.\n\
Keep the id of every agent that stays, and give a new agent an id nobody has used. Change only what the request needs: \
leave every other agent's name, prompt, model and checkpoint exactly as given, and remove an agent only when asked or \
when the change cannot work without it. When you add or remove an agent, rewire the edges so the flow still makes \
sense (a new agent in the middle is joined to what comes before and after it). summary is one or two plain sentences \
saying what you changed and why, as you would tell the user. If the request cannot be done or is not about the team, \
return the team unchanged and say so in summary.\n\
When you add or change an agent, follow these rules for a good team.\n{DESIGN_RULES}",
        role_lines()
    )
}

fn edit_schema() -> Value {
    let mut s = suggest_schema();
    // The editor also tells the user what it did, and an agent the user wrote may appear in the team it is given.
    s["required"] = json!(["name", "nodes", "edges", "summary"]);
    s["properties"]["summary"] = text();
    let mut ids: Vec<Value> = super::roles::catalog().iter().map(|d| json!(role_id(d.id))).collect();
    ids.push(json!("custom"));
    s["properties"]["nodes"]["items"]["properties"]["role"] = json!({"type": "string", "enum": ids});
    s
}

/// The flow as the editor sees it: the same shape it answers in, without positions.
fn edit_view(flow: &Flow) -> Value {
    json!({
        "name": flow.name,
        "nodes": flow.nodes.iter().map(|n| json!({
            "id": n.id, "name": n.name, "role": role_id(n.role),
            "engine": match n.engine { Engine::Claude => "claude", Engine::Codex => "codex" },
            "model": n.model, "system_prompt": n.system_prompt, "checkpoint": n.checkpoint,
        })).collect::<Vec<_>>(),
        "edges": flow.edges.iter().map(|e| json!({"from": e.from, "to": e.to, "retry": e.kind == EdgeKind::Retry})).collect::<Vec<_>>(),
    })
}

pub fn edit_prompt(flow: &Flow, request: &str, codex_models: &[String]) -> String {
    format!(
        "# The team now\n\n```json\n{}\n```\n\n# Codex models available\n\n{}\n\n# The user's request\n\n{}\n",
        serde_json::to_string_pretty(&edit_view(flow)).unwrap_or_default(),
        codex_models.join(", "),
        request.trim()
    )
}

/// Apply the editor's answer to the flow it was given. Agents it kept stay the same agents (position, token cap and
/// a custom role are theirs, not the model's to change); new ones get ids of ours and a free spot on the canvas.
/// Edges are rebuilt. The result must be a valid flow, or nothing changes.
pub fn flow_from_edit(base: &Flow, v: &Value, codex_models: &[String]) -> Result<(Flow, String)> {
    let raw: Vec<Value> = v.get("nodes").and_then(Value::as_array).cloned().unwrap_or_default();
    if raw.is_empty() {
        bail!("the change would leave the flow with no agents");
    }
    if raw.len() > MAX_SUGGESTED_NODES {
        bail!("the change would make {} agents; the most is {MAX_SUGGESTED_NODES}", raw.len());
    }
    let mut taken: Vec<String> = base.nodes.iter().map(|n| n.id.clone()).collect();
    let mut ids: HashMap<String, String> = HashMap::new();
    let mut nodes: Vec<Node> = Vec::new();
    let mut added: Vec<String> = Vec::new();
    for n in &raw {
        let s = |k: &str| n.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let said = s("id");
        if ids.contains_key(&said) {
            bail!("two agents were given the same id, so the edges cannot be told apart");
        }
        let existing = base.nodes.iter().find(|b| b.id == said);
        let engine: Engine = serde_json::from_value(n.get("engine").cloned().unwrap_or_default()).context("unknown engine")?;
        let mut model = s("model");
        let known = match engine {
            Engine::Claude => CLAUDE_MODELS.iter().any(|m| *m == model),
            Engine::Codex => codex_models.contains(&model),
        };
        if !known {
            // An agent that already has a model keeps it when the editor invents one; a new one gets a sensible default.
            model = match (existing, engine) {
                (Some(e), _) if e.engine == engine => e.model.clone(),
                (_, Engine::Claude) => "sonnet".into(),
                (_, Engine::Codex) => codex_models.first().cloned().context("no Codex model to fall back to")?,
            };
        }
        let role: Role = serde_json::from_value(n.get("role").cloned().unwrap_or_default()).context("unknown role")?;
        let (id, mut node) = match existing {
            Some(e) => (e.id.clone(), e.clone()),
            None => {
                if role == Role::Custom {
                    bail!("a new agent cannot have a custom role; add it yourself with More roles, Custom role");
                }
                let id = free_id(&taken);
                taken.push(id.clone());
                added.push(id.clone());
                let blank = Node {
                    id: id.clone(),
                    name: String::new(),
                    role,
                    custom: None,
                    engine,
                    model: model.clone(),
                    system_prompt: String::new(),
                    checkpoint: false,
                    token_cap: None,
                    keep_model: false,
                    x: 0.0,
                    y: 0.0,
                };
                (id, blank)
            }
        };
        ids.insert(said, id);
        // A kept custom agent keeps its role; any other agent may be given a different built-in one.
        if node.role != Role::Custom && role != Role::Custom {
            node.role = role;
        }
        node.engine = engine;
        node.model = model;
        if !s("name").is_empty() {
            node.name = s("name").chars().take(60).collect();
        } else if node.name.is_empty() {
            node.name = format!("{:?}", node.role);
        }
        if !s("system_prompt").is_empty() {
            node.system_prompt = s("system_prompt");
        } else if node.system_prompt.is_empty() {
            node.system_prompt = starters::default_prompt(node.role).into();
        }
        node.checkpoint = n.get("checkpoint").and_then(Value::as_bool).unwrap_or(node.checkpoint);
        nodes.push(node);
    }
    let mut edges = Vec::new();
    for e in v.get("edges").and_then(Value::as_array).cloned().unwrap_or_default() {
        let end = |k: &str| e.get(k).and_then(Value::as_str).and_then(|x| ids.get(x)).cloned();
        let (Some(from), Some(to)) = (end("from"), end("to")) else { bail!("an edge points at an agent that does not exist") };
        let kind = if e.get("retry").and_then(Value::as_bool) == Some(true) { EdgeKind::Retry } else { EdgeKind::Forward };
        edges.push(Edge { id: format!("e{}", edges.len() + 1), from, to, kind });
    }
    let name = v.get("name").and_then(Value::as_str).unwrap_or("").trim();
    let mut flow = Flow {
        id: base.id.clone(),
        name: if name.is_empty() { base.name.clone() } else { name.chars().take(60).collect() },
        nodes,
        edges,
        max_retries: base.max_retries,
        budget: base.budget,
        downshift: base.downshift,
    };
    flow.validate().map_err(anyhow::Error::msg).context("the changed flow is not valid")?;
    place_new(&mut flow, &added);
    let summary = v.get("summary").and_then(Value::as_str).unwrap_or("").trim();
    Ok((flow, if summary.is_empty() { "Changed the flow.".into() } else { summary.chars().take(600).collect() }))
}

/// `n1`, `n2`, ... the first one no agent has.
fn free_id(taken: &[String]) -> String {
    (1..).map(|i| format!("n{i}")).find(|id| !taken.contains(id)).expect("an unused id exists")
}

/// Put each added agent where the flow's order says it belongs (its column left to right, a free row), without
/// moving any agent that was already placed.
fn place_new(flow: &mut Flow, added: &[String]) {
    if added.is_empty() {
        return;
    }
    let layers = flow.layers().unwrap_or_default();
    for id in added {
        let col = layers.iter().position(|l| l.contains(id)).unwrap_or(0) as f64;
        let x = col * 280.0;
        let mut y = layers.iter().find(|l| l.contains(id)).and_then(|l| l.iter().position(|i| i == id)).unwrap_or(0) as f64 * 180.0;
        let busy = |flow: &Flow, y: f64| flow.nodes.iter().any(|n| n.id != *id && (n.x - x).abs() < 240.0 && (n.y - y).abs() < 150.0);
        while busy(flow, y) {
            y += 180.0;
        }
        if let Some(n) = flow.nodes.iter_mut().find(|n| n.id == *id) {
            n.x = x;
            n.y = y;
        }
    }
}

/// Change a flow as the user asks, using Claude or Codex for the one call. Nothing is saved.
pub async fn edit_flow(
    base: &Flow,
    request: &str,
    engine: Engine,
    codex_models: &[String],
    hub: &super::codex::Hub,
    cwd: &Path,
) -> Result<(Flow, String, Outcome)> {
    let (system, prompt, schema) = (edit_system(), edit_prompt(base, request, codex_models), edit_schema());
    let (v, outcome) = match engine {
        Engine::Claude => one_shot("sonnet", &system, &prompt, schema, cwd).await?,
        Engine::Codex => {
            // The smallest Codex model on offer does this well enough, and is the cheap one.
            let model = codex_models.iter().find(|m| m.contains("luna")).or(codex_models.first()).context("no Codex model is available")?;
            one_shot_codex(hub, model, &system, &prompt, &schema, cwd).await?
        }
    };
    let (flow, summary) = flow_from_edit(base, &v, codex_models)?;
    Ok((flow, summary, outcome))
}

/// One no-tools-needed call to Codex in a read-only sandbox, answering in the given shape.
async fn one_shot_codex(hub: &super::codex::Hub, model: &str, system: &str, prompt: &str, schema: &Value, cwd: &Path) -> Result<(Value, Outcome)> {
    tokio::fs::create_dir_all(cwd).await?;
    let server = hub.get().await?;
    let spec = super::codex::Spec { model: model.into(), system_prompt: system.into(), read_only: true, cwd: cwd.to_path_buf() };
    let mut thread = server.start_thread(&spec).await?;
    let outcome = server.turn(&mut thread, prompt, schema, |_| {}).await?;
    let answer = outcome.structured.clone().context("codex did not answer in the required shape")?;
    Ok((answer, outcome))
}

/// What a role is, in words an instruction-writer can use.
pub fn describe_role(label: &str, capability: Capability, output: Output) -> String {
    let can = match capability {
        Capability::Read => "read and search files and the web; it cannot run commands or change anything",
        Capability::Inspect => "read, and run commands such as tests and `git diff` in a sandbox that cannot change anything",
        Capability::Write => "change files, in its own copy of the project",
    };
    let gives = match output {
        Output::Plan => "a plan: what to do, and the steps",
        Output::Report => "a report: a summary, the findings and their sources",
        Output::Change => "a change: what it did, the files it touched, and notes",
        Output::Verdict => "a verdict: approve or reject, with feedback saying what to fix",
    };
    format!("{label}. It can {can}. It hands back {gives}.")
}

/// Sonnet writes the system prompt for one agent.
pub async fn node_prompt(label: &str, capability: Capability, output: Output, name: &str, goal: &str, cwd: &Path) -> Result<(String, Outcome)> {
    let schema = json!({"type": "object", "additionalProperties": false, "required": ["system_prompt"],
                        "properties": {"system_prompt": text()}});
    let role = describe_role(label, capability, output);
    let ask = format!("# Agent\n\n{name}: {role}\n\n# Overall goal, for context only\n\n{}\n", goal.trim());
    let (v, outcome) = one_shot("sonnet", NODE_PROMPT_SYSTEM, &ask, schema, cwd).await?;
    let p = v.get("system_prompt").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if p.is_empty() {
        bail!("empty prompt");
    }
    Ok((p, outcome))
}

const CUSTOM_ROLE_SYSTEM: &str = "You design one new role for an AI agent in a team of agents that work on software. The user describes \
the role in their own words. Reply with: a short name (1 to 3 words); what the agent may do; what it hands back; its \
instructions.\n\
What it may do (capability), pick the least that does the job: read (look at files and the web, nothing else), inspect \
(read, and run commands such as tests or `git diff` in a sandbox that cannot change anything, for checking work), or \
write (change files in its own copy of the project: only for an agent whose job is to make a change).\n\
What it hands back (output), exactly one: plan (what to do, and steps), report (a summary, findings and sources), \
change (what it did and the files it touched, only for write), or verdict (approve or reject with feedback, for an \
agent that checks other agents' work and may send it back).\n\
Rules: a reviewer or checker gets inspect or read and a verdict, never write, so it cannot alter what it is judging. An \
agent that finds or reads things gets read and a report. One role does one job; if the description is two jobs, design \
the main one. Instructions are 3 to 6 sentences in the second person: the one job and how to do it well, what exactly to \
hand back, what not to do (for a reader, not to change files; for a checker, not to fix what it finds). Say what a good \
answer looks like, give a stopping rule (when to stop), and say what to do when it cannot tell. For a verdict role, say \
what counts as a reason to reject and that style preferences are not one, and ask for evidence it observed rather than the \
author's claims. Never mention a specific project, file or goal: the goal is given to \
every agent separately. No placeholders, no markdown headings.";

fn custom_role_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["name", "capability", "output", "instructions"],
        "properties": {
            "name": text(),
            "capability": {"type": "string", "enum": ["read", "inspect", "write"]},
            "output": {"type": "string", "enum": ["plan", "report", "change", "verdict"]},
            "instructions": text(),
        }
    })
}

/// A custom role as proposed, checked and made consistent. The model proposes; these rules decide, so a bad
/// combination can never reach a run whatever the model said.
#[derive(Debug, Clone, PartialEq)]
pub struct NewRole {
    pub role: CustomRole,
    pub instructions: String,
}

pub fn role_from_generation(v: &Value) -> Result<NewRole> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let label: String = s("name").chars().take(40).collect();
    if label.trim().is_empty() {
        bail!("the role has no name");
    }
    let capability: Capability = serde_json::from_value(v.get("capability").cloned().unwrap_or_default()).context("unknown capability")?;
    let mut output: Output = serde_json::from_value(v.get("output").cloned().unwrap_or_default()).context("unknown result shape")?;
    // A writer reports a change, and only a writer does; a verdict is never given by something that writes.
    output = match (capability, output) {
        (Capability::Write, _) => Output::Change,
        (_, Output::Change) => Output::Report,
        (_, o) => o,
    };
    let instructions = s("instructions");
    if instructions.len() < 40 {
        bail!("the instructions are too short to be useful");
    }
    if instructions.len() > 2000 {
        bail!("the instructions are too long");
    }
    Ok(NewRole { role: CustomRole { label, capability, output }, instructions })
}

/// Sonnet designs a role from a description.
pub async fn custom_role(description: &str, cwd: &Path) -> Result<(NewRole, Outcome)> {
    let ask = format!("# The role, in the user's words\n\n{}\n", description.trim());
    let (v, outcome) = one_shot("sonnet", CUSTOM_ROLE_SYSTEM, &ask, custom_role_schema(), cwd).await?;
    Ok((role_from_generation(&v)?, outcome))
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

    /// A flow as a person would have left it: laid out by hand, one agent with a token cap, one of their own roles.
    fn base() -> Flow {
        let mut f = flow_from_suggestion(&suggestion(), "mine", &models()).unwrap();
        f.nodes[0].x = 11.0;
        f.nodes[1].y = 222.0;
        f.nodes[1].token_cap = Some(50_000);
        f.nodes[1].keep_model = true;
        f.nodes.push(Node {
            id: "n9".into(),
            name: "Security".into(),
            role: Role::Custom,
            custom: Some(CustomRole { label: "Security reviewer".into(), capability: Capability::Read, output: Output::Report }),
            engine: Engine::Claude,
            model: "sonnet".into(),
            system_prompt: "Look for holes.".into(),
            checkpoint: false,
            token_cap: None,
            keep_model: false,
            x: 900.0,
            y: 0.0,
        });
        f.edges.push(Edge { id: "e9".into(), from: "n2".into(), to: "n9".into(), kind: EdgeKind::Forward });
        f
    }

    /// What the editor would answer for `base()` with the given changes applied by hand to its view.
    fn edit_answer(change: impl FnOnce(&mut Value)) -> Value {
        let mut v = edit_view(&base());
        v["summary"] = json!("Did it.");
        change(&mut v);
        v
    }

    /// Asks the real Claude and Codex to edit a flow, so it uses a little of each plan:
    /// `cargo test -- --ignored live_flow_edit --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_flow_edit_works_with_claude_and_with_codex() {
        let hub = super::super::codex::Hub::default();
        let models = vec!["gpt-6.1-sol".to_string(), "gpt-6-luna".to_string()];
        let cwd = std::env::temp_dir().join(format!("ttm-orch-edit-{}", std::process::id()));
        let b = base();
        for engine in [Engine::Claude, Engine::Codex] {
            let (f, summary, _) = edit_flow(&b, "Add a reporter at the end that writes up what was done.", engine, &models, &hub, &cwd).await.unwrap();
            eprintln!("{engine:?}: {} agents -> {}; summary: {summary}", b.nodes.len(), f.nodes.len());
            assert!(f.nodes.iter().any(|n| n.role == Role::Reporter), "{engine:?} added no reporter");
            for old in &b.nodes {
                let now = f.node(&old.id).unwrap_or_else(|| panic!("{engine:?} dropped {}", old.name));
                assert_eq!((now.x, now.y), (old.x, old.y));
            }
            assert_eq!(f.node("n9").unwrap().role, Role::Custom);
        }
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn an_edit_that_changes_nothing_keeps_every_agent_as_it_was() {
        let b = base();
        let (f, summary) = flow_from_edit(&b, &edit_answer(|_| {}), &models()).unwrap();
        assert_eq!(summary, "Did it.");
        assert_eq!(f.nodes, b.nodes, "same agents, positions, caps and custom role");
        let wires = |f: &Flow| f.edges.iter().map(|e| (e.from.clone(), e.to.clone(), e.kind)).collect::<Vec<_>>();
        assert_eq!(wires(&f), wires(&b), "same connections (their ids are renumbered)");
        assert_eq!((f.id.as_str(), f.max_retries, f.downshift), ("mine", b.max_retries, b.downshift));
    }

    #[test]
    fn a_changed_agent_keeps_its_place_and_what_the_editor_cannot_touch() {
        let b = base();
        let (f, _) = flow_from_edit(&b, &edit_answer(|v| {
            v["nodes"][1]["model"] = json!("gpt-6-luna");
            v["nodes"][1]["system_prompt"] = json!("Build it carefully.");
            v["nodes"][1]["name"] = json!("Coder");
        }), &models())
        .unwrap();
        let n = f.nodes.iter().find(|n| n.id == "n2").unwrap();
        assert_eq!((n.name.as_str(), n.model.as_str(), n.system_prompt.as_str()), ("Coder", "gpt-6-luna", "Build it carefully."));
        assert_eq!((n.y, n.token_cap, n.keep_model), (222.0, Some(50_000), true));
    }

    #[test]
    fn a_new_agent_gets_a_fresh_id_and_a_free_spot_and_the_others_stay_put() {
        let b = base();
        let (f, _) = flow_from_edit(&b, &edit_answer(|v| {
            v["nodes"].as_array_mut().unwrap().push(json!({"id": "docs", "name": "Docs", "role": "reporter", "engine": "claude",
                "model": "sonnet", "system_prompt": "Write the docs.", "checkpoint": false}));
            v["edges"].as_array_mut().unwrap().push(json!({"from": "n3", "to": "docs", "retry": false}));
        }), &models())
        .unwrap();
        // The editor's own id for it is not ours: it gets one that no agent has.
        assert_eq!(f.nodes.len(), b.nodes.len() + 1);
        let new = f.nodes.last().unwrap();
        assert!(!b.nodes.iter().any(|n| n.id == new.id), "{}", new.id);
        assert_eq!(new.name, "Docs");
        for old in &b.nodes {
            let now = f.nodes.iter().find(|n| n.id == old.id).unwrap();
            assert_eq!((now.x, now.y), (old.x, old.y), "{} did not move", old.name);
        }
        assert!(f.nodes.iter().filter(|n| n.id != new.id).all(|n| (n.x - new.x).abs() >= 240.0 || (n.y - new.y).abs() >= 150.0), "no overlap");
    }

    #[test]
    fn removing_an_agent_drops_it_and_its_edges() {
        let b = base();
        let (f, _) = flow_from_edit(&b, &edit_answer(|v| {
            let nodes = v["nodes"].as_array_mut().unwrap();
            nodes.retain(|n| n["id"] != "n9");
            let edges = v["edges"].as_array_mut().unwrap();
            edges.retain(|e| e["to"] != "n9" && e["from"] != "n9");
        }), &models())
        .unwrap();
        assert!(f.node("n9").is_none());
        assert!(f.edges.iter().all(|e| e.from != "n9" && e.to != "n9"));
    }

    #[test]
    fn a_kept_custom_agent_stays_custom_and_a_new_one_cannot_be() {
        let b = base();
        // The editor says "reporter" for the custom agent: it stays the user's own role.
        let (f, _) = flow_from_edit(&b, &edit_answer(|v| v["nodes"][3]["role"] = json!("reporter")), &models()).unwrap();
        let c = f.node("n9").unwrap();
        assert_eq!(c.role, Role::Custom);
        assert_eq!(c.custom.as_ref().unwrap().label, "Security reviewer");
        let err = flow_from_edit(&b, &edit_answer(|v| {
            v["nodes"].as_array_mut().unwrap().push(json!({"id": "z", "name": "Mine", "role": "custom", "engine": "claude",
                "model": "sonnet", "system_prompt": "x", "checkpoint": false}));
        }), &models())
        .unwrap_err();
        assert!(err.to_string().contains("custom role"), "{err}");
    }

    #[test]
    fn a_model_the_editor_invents_is_replaced_by_one_that_exists() {
        let b = base();
        let (f, _) = flow_from_edit(&b, &edit_answer(|v| v["nodes"][1]["model"] = json!("gpt-99")), &models()).unwrap();
        assert_eq!(f.node("n2").unwrap().model, "gpt-6.1-sol", "kept the model it had");
        let (f, _) = flow_from_edit(&b, &edit_answer(|v| {
            v["nodes"].as_array_mut().unwrap().push(json!({"id": "q", "name": "Checker", "role": "judge", "engine": "claude",
                "model": "claude-imaginary", "system_prompt": "Check.", "checkpoint": false}));
        }), &models())
        .unwrap();
        assert_eq!(f.nodes.last().unwrap().model, "sonnet");
    }

    #[test]
    fn a_bad_answer_changes_nothing() {
        let b = base();
        // An edge to nowhere.
        let e = flow_from_edit(&b, &edit_answer(|v| v["edges"].as_array_mut().unwrap().push(json!({"from": "n1", "to": "ghost", "retry": false}))), &models()).unwrap_err();
        assert!(e.to_string().contains("does not exist"), "{e}");
        // No agents left.
        assert!(flow_from_edit(&b, &edit_answer(|v| v["nodes"] = json!([])), &models()).is_err());
        // A loop with no judge to send work back.
        let e = flow_from_edit(&b, &edit_answer(|v| v["edges"].as_array_mut().unwrap().push(json!({"from": "n3", "to": "n1", "retry": false}))), &models()).unwrap_err();
        assert!(format!("{e:#}").contains("not valid"), "{e:#}");
        // Two agents with one id.
        let e = flow_from_edit(&b, &edit_answer(|v| v["nodes"][1]["id"] = json!("n1")), &models()).unwrap_err();
        assert!(e.to_string().contains("same id"), "{e}");
        // Too many agents.
        let many = |v: &mut Value| {
            for i in 0..12 {
                v["nodes"].as_array_mut().unwrap().push(json!({"id": format!("x{i}"), "name": format!("A{i}"), "role": "search",
                    "engine": "claude", "model": "haiku", "system_prompt": "Find.", "checkpoint": false}));
            }
        };
        assert!(flow_from_edit(&b, &edit_answer(many), &models()).is_err());
    }

    #[test]
    fn an_empty_flow_can_be_built_from_a_request() {
        let blank = Flow { nodes: vec![], edges: vec![], ..base() };
        let (f, _) = flow_from_edit(&blank, &suggestion(), &models()).unwrap();
        assert_eq!(f.nodes.len(), 3);
        assert_eq!(f.id, "mine", "it is still the same flow");
        assert!(f.nodes.iter().all(|n| n.id.starts_with('n')));
    }

    #[test]
    fn the_editor_is_shown_the_flow_and_the_request_and_may_use_custom_roles_in_its_answer() {
        let p = edit_prompt(&base(), "  add a docs writer  ", &models());
        assert!(p.contains("\"id\": \"n9\"") && p.contains("add a docs writer") && p.contains("gpt-6-luna"));
        assert!(!p.contains("\"x\""), "positions are not the model's business");
        let roles = edit_schema()["properties"]["nodes"]["items"]["properties"]["role"]["enum"].clone();
        assert!(roles.as_array().unwrap().contains(&json!("custom")));
        assert!(edit_schema()["required"].as_array().unwrap().contains(&json!("summary")));
        assert!(!suggest_schema()["properties"]["nodes"]["items"]["properties"]["role"]["enum"].as_array().unwrap().contains(&json!("custom")));
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
        assert_eq!((f.nodes[0].x, f.nodes[1].x, f.nodes[2].x), (0.0, 280.0, 560.0));
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
        assert!(format!("{:#}", flow_from_suggestion(&s, "f", &models()).unwrap_err()).contains("verdict"));

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
            GrillStep::Questions(vec![Question { text: "Which database?".into(), recommended: "SQLite".into(), options: vec![], multiple: false }])
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

    #[test]
    fn a_generated_role_is_checked_and_made_consistent() {
        let ok = json!({"name": "Security reviewer", "capability": "inspect", "output": "verdict",
            "instructions": "You review the change for security problems such as injection and unsafe input handling. Run the checks you can. Approve only if you found none; otherwise reject and name each problem."});
        let r = role_from_generation(&ok).unwrap();
        assert_eq!(r.role.label, "Security reviewer");
        assert_eq!((r.role.capability, r.role.output), (Capability::Inspect, Output::Verdict));
    }

    #[test]
    fn a_writer_always_reports_a_change_and_nothing_else_does() {
        let body = "You do the job described and nothing beyond it. Hand back exactly what the next agent needs to continue.";
        // The model said a writer gives a verdict: a writer reports a change.
        let r = role_from_generation(&json!({"name": "Fixer", "capability": "write", "output": "verdict", "instructions": body})).unwrap();
        assert_eq!((r.role.capability, r.role.output), (Capability::Write, Output::Change));
        // The model said a reader gives a change: it cannot have made one, so it reports.
        let r = role_from_generation(&json!({"name": "Scout", "capability": "read", "output": "change", "instructions": body})).unwrap();
        assert_eq!((r.role.capability, r.role.output), (Capability::Read, Output::Report));
    }

    #[test]
    fn a_reviewer_can_never_come_out_able_to_write() {
        let body = "You check the work and say what is wrong. You never fix anything yourself, you only report what you find.";
        for cap in ["read", "inspect"] {
            let r = role_from_generation(&json!({"name": "Checker", "capability": cap, "output": "verdict", "instructions": body})).unwrap();
            assert_eq!(r.role.output, Output::Verdict);
            assert_ne!(r.role.capability, Capability::Write);
        }
        // Asking for write and a verdict gives a writer that reports a change: never a judge that can edit.
        let r = role_from_generation(&json!({"name": "Both", "capability": "write", "output": "verdict", "instructions": body})).unwrap();
        assert_ne!(r.role.output, Output::Verdict);
    }

    #[test]
    fn a_bad_generation_is_refused_with_a_reason() {
        let body = "You do the job described and nothing beyond it. Hand back exactly what the next agent needs to continue.";
        let bad = |v: Value| role_from_generation(&v).unwrap_err().to_string();
        assert!(bad(json!({"name": " ", "capability": "read", "output": "report", "instructions": body})).contains("no name"));
        assert!(bad(json!({"name": "X", "capability": "root", "output": "report", "instructions": body})).contains("capability"));
        assert!(bad(json!({"name": "X", "capability": "read", "output": "essay", "instructions": body})).contains("result shape"));
        assert!(bad(json!({"name": "X", "capability": "read", "output": "report", "instructions": "Do it."})).contains("too short"));
        assert!(bad(json!({"name": "X", "capability": "read", "output": "report", "instructions": "x".repeat(2500)})).contains("too long"));
    }

    #[test]
    fn a_long_name_is_cut_and_the_prompt_for_a_role_says_what_it_can_do() {
        let body = "You do the job described and nothing beyond it. Hand back exactly what the next agent needs to continue.";
        let r = role_from_generation(&json!({"name": "A".repeat(90), "capability": "read", "output": "report", "instructions": body})).unwrap();
        assert_eq!(r.role.label.chars().count(), 40);
        let d = describe_role("Tester", Capability::Inspect, Output::Verdict);
        assert!(d.contains("Tester") && d.contains("run commands") && d.contains("approve or reject"), "{d}");
        assert!(describe_role("Dev", Capability::Write, Output::Change).contains("change files"));
    }

    #[test]
    fn the_generation_prompt_and_schema_agree_on_the_words() {
        let sch = custom_role_schema();
        for word in ["read", "inspect", "write"] {
            assert!(sch["properties"]["capability"]["enum"].as_array().unwrap().iter().any(|x| x == word));
            assert!(CUSTOM_ROLE_SYSTEM.contains(word));
        }
        for word in ["plan", "report", "change", "verdict"] {
            assert!(sch["properties"]["output"]["enum"].as_array().unwrap().iter().any(|x| x == word));
            assert!(CUSTOM_ROLE_SYSTEM.contains(word));
        }
        assert_eq!(sch["additionalProperties"], false);
    }

    #[test]
    fn the_suggest_prompt_offers_every_catalog_role_and_the_researched_rules() {
        let p = suggest_system();
        for d in crate::orch::roles::catalog() {
            assert!(p.contains(&format!("- {} (", role_id(d.id))), "{:?} is missing from the prompt", d.id);
        }
        assert!(!p.contains("- custom ("), "a custom role is not offered to the designer");
        for rule in ["3 to 5 agents", "never have two agents", "also have a retry edge", "different model for the judge"] {
            assert!(p.contains(rule), "{rule}");
        }
        // The schema accepts exactly the ids the prompt offers.
        let sch = suggest_schema();
        let ids: Vec<&str> = sch["properties"]["nodes"]["items"]["properties"]["role"]["enum"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(ids.len(), crate::orch::roles::catalog().len());
        assert!(ids.contains(&"architect") && ids.contains(&"debugger") && ids.contains(&"reporter"));
    }

    #[test]
    fn a_suggestion_can_use_the_new_roles_and_send_work_back_to_a_reader() {
        let s = json!({
            "name": "Bug fix",
            "nodes": [
                {"id": "p", "name": "Plan", "role": "orchestrator", "engine": "claude", "model": "opus", "system_prompt": "x", "checkpoint": false},
                {"id": "d", "name": "Debug", "role": "debugger", "engine": "claude", "model": "sonnet", "system_prompt": "x", "checkpoint": false},
                {"id": "b", "name": "Build", "role": "builder", "engine": "codex", "model": "gpt-6.1-sol", "system_prompt": "x", "checkpoint": true},
                {"id": "j", "name": "Judge", "role": "judge", "engine": "claude", "model": "opus", "system_prompt": "x", "checkpoint": false}
            ],
            "edges": [
                {"from": "p", "to": "d", "retry": false}, {"from": "d", "to": "b", "retry": false},
                {"from": "b", "to": "j", "retry": false}, {"from": "j", "to": "b", "retry": true}
            ]
        });
        let f = flow_from_suggestion(&s, "f", &models()).unwrap();
        assert_eq!(f.nodes[1].role, Role::Debugger);
        // Work can be sent back to an earlier agent that only reads, such as the debugger.
        let mut ok = s.clone();
        ok["edges"][3]["to"] = json!("d");
        assert!(flow_from_suggestion(&ok, "f", &models()).is_ok());
        // But not forward: the judge runs last, so sending to itself or a later agent is refused.
        let mut bad = s.clone();
        bad["edges"][3]["to"] = json!("j");
        assert!(flow_from_suggestion(&bad, "f", &models()).is_err());
    }

    #[test]
    fn the_custom_role_prompt_asks_for_a_stopping_rule_and_a_reject_threshold() {
        for phrase in ["stopping rule", "style preferences are not one", "evidence it observed"] {
            assert!(CUSTOM_ROLE_SYSTEM.contains(phrase), "{phrase}");
        }
    }

    #[test]
    fn a_question_comes_with_choices_and_the_recommended_one_is_among_them() {
        let q = json!({"done": false, "brief": "", "questions": [
            {"text": "Which database?", "options": ["SQLite", "Postgres", "Files"], "recommended": "SQLite", "multiple": false}]});
        let GrillStep::Questions(qs) = parse_grill(&q, false).unwrap() else { panic!() };
        assert_eq!(qs[0].options, vec!["SQLite", "Postgres", "Files"]);
        assert_eq!(qs[0].recommended, "SQLite");
        assert!(!qs[0].multiple);
    }

    #[test]
    fn a_recommended_answer_the_model_left_out_of_the_options_is_added_first() {
        let q = json!({"done": false, "brief": "", "questions": [
            {"text": "Where?", "options": ["Local", "Cloud"], "recommended": "Local, with a path to cloud later", "multiple": false}]});
        let GrillStep::Questions(qs) = parse_grill(&q, false).unwrap() else { panic!() };
        assert_eq!(qs[0].options[0], "Local, with a path to cloud later");
        assert_eq!(qs[0].options.len(), 3);
    }

    #[test]
    fn options_are_cleaned_and_capped_at_four() {
        let q = json!({"done": false, "brief": "", "questions": [
            {"text": "Pick", "options": ["  A ", "a", "", "B", "C", "D", "E"], "recommended": "A", "multiple": true}]});
        let GrillStep::Questions(qs) = parse_grill(&q, false).unwrap() else { panic!() };
        assert_eq!(qs[0].options, vec!["A", "B", "C", "D"], "trimmed, de-duplicated ignoring case, no blanks, four at most");
        assert!(qs[0].multiple);
        let long = "x".repeat(500);
        let q = json!({"done": false, "brief": "", "questions": [{"text": "Pick", "options": [long, "B"], "recommended": "B", "multiple": false}]});
        let GrillStep::Questions(qs) = parse_grill(&q, false).unwrap() else { panic!() };
        assert!(qs[0].options.iter().all(|o| o.chars().count() <= MAX_OPTION));
    }

    #[test]
    fn fewer_than_two_options_is_not_a_choice_so_the_page_asks_for_text() {
        let one = json!({"done": false, "brief": "", "questions": [{"text": "Q", "options": ["Only"], "recommended": "Only", "multiple": true}]});
        let GrillStep::Questions(qs) = parse_grill(&one, false).unwrap() else { panic!() };
        assert!(qs[0].options.is_empty() && !qs[0].multiple, "no choices, so not a multi-choice either");
        assert_eq!(qs[0].recommended, "Only", "the suggestion is kept for the text box");
        // An older reply with no options at all still works.
        let old = json!({"done": false, "brief": "", "questions": [{"text": "Q", "recommended": "Yes"}]});
        let GrillStep::Questions(qs) = parse_grill(&old, false).unwrap() else { panic!() };
        assert!(qs[0].options.is_empty());
    }

    #[test]
    fn the_grilling_prompt_and_schema_ask_for_choices_and_never_an_other_option() {
        for phrase in ["two to four short, distinct options", "never add an option like", "multiple to true only"] {
            assert!(GRILL_SYSTEM.contains(phrase), "{phrase}");
        }
        let item = &grill_schema()["properties"]["questions"]["items"];
        assert_eq!(item["required"], json!(["text", "options", "recommended", "multiple"]));
        assert_eq!(item["additionalProperties"], false);
    }

    #[test]
    fn a_stored_question_without_choices_still_loads() {
        let q: Question = serde_json::from_value(json!({"text": "Q", "recommended": "A"})).unwrap();
        assert!(q.options.is_empty() && !q.multiple);
    }
}
