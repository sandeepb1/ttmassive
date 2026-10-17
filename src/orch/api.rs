//! HTTP routes for orchestrator mode. Thin: each one checks its input and calls the service.
//! They are merged into the protected router, so the login cookie guards them like everything else.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{
        Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;

use super::graph::{Flow, Role};
use super::service::Fail;

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        match self {
            Fail::NotFound => StatusCode::NOT_FOUND.into_response(),
            Fail::Bad(m) => (StatusCode::BAD_REQUEST, m).into_response(),
            Fail::Conflict(m) => (StatusCode::CONFLICT, m).into_response(),
            Fail::Internal(e) => {
                tracing::warn!("orchestrator: {e:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response()
            }
        }
    }
}

type St = State<Arc<AppState>>;

/// Responses are never cached: they change as a run goes.
fn json_nocache(v: impl serde::Serialize) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(v)).into_response()
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/orch/models", get(models))
        .route("/api/orch/flows", get(flows))
        .route("/api/orch/flows/suggest", post(suggest))
        .route("/api/orch/flows/activity/{track}", get(design_activity))
        .route("/api/orch/flows/review", post(review_flow))
        .route("/api/orch/flows/optimise", post(optimise_flow))
        .route("/api/orch/flows/edit", post(edit_flow))
        .route("/api/orch/flows/{id}", axum::routing::put(save_flow).delete(delete_flow))
        .route("/api/orch/flows/{id}/move", post(move_flow))
        .route("/api/orch/folders", post(create_folder))
        .route("/api/orch/folders/link", post(link_folder))
        .route("/api/orch/folders/rename", post(rename_folder))
        .route("/api/orch/folders/delete", post(delete_folder))
        .route("/api/orch/node-prompt", post(node_prompt))
        .route("/api/orch/custom-role", post(custom_role))
        .route("/api/orch/roles", get(roles))
        .route("/api/orch/runs", get(list_runs).post(create_run))
        .route("/api/orch/runs/{id}", get(get_run).delete(delete_run))
        .route("/api/orch/runs/{id}/turns/{n}", get(turn))
        .route("/api/orch/runs/{id}/grill", post(grill))
        .route("/api/orch/runs/{id}/start", post(start))
        .route("/api/orch/runs/{id}/approve", post(approve))
        .route("/api/orch/runs/{id}/more-rounds", post(more_rounds))
        .route("/api/orch/runs/{id}/answers", post(answer))
        .route("/api/orch/runs/{id}/resume", post(resume))
        .route("/api/orch/runs/{id}/cancel", post(cancel))
        .route("/api/orch/runs/{id}/popout", post(pop_out))
        .route("/api/orch/limits", get(limits).put(set_limits))
        .route("/api/orch/project", get(project))
        .route("/api/orch/project/init", post(project_init))
        .route("/api/orch/events", get(events))
}

async fn models(State(st): St) -> Response {
    json_nocache(st.orch.models(&st.data_dir).await)
}

async fn flows(State(st): St) -> Response {
    json_nocache(st.orch.flows())
}

#[derive(Deserialize)]
struct GoalBody {
    goal: String,
    /// A name the page chose for this piece of work, to ask what it is doing by.
    #[serde(default)]
    track: Option<String>,
}

#[derive(Deserialize)]
struct StageBody {
    goal: String,
    flow: super::graph::Flow,
    #[serde(default)]
    track: Option<String>,
}

/// Step two of designing a flow: check the design against the kind of work and what is available.
async fn review_flow(State(st): St, Json(b): Json<StageBody>) -> Result<Response, Fail> {
    let (flow, summary) = st.orch.review_flow(&st, &b.goal, b.flow, &st.orch.track(b.track.as_deref())).await?;
    Ok(json_nocache(json!({"flow": flow, "summary": summary})))
}

/// Step three: rewrite every agent's prompt for its model and its place in the team.
async fn optimise_flow(State(st): St, Json(b): Json<StageBody>) -> Result<Response, Fail> {
    let (flow, summary) = st.orch.optimise_flow(&st, &b.goal, b.flow, &st.orch.track(b.track.as_deref())).await?;
    Ok(json_nocache(json!({"flow": flow, "summary": summary})))
}

/// What designing a flow is doing: each lookup and call so far, and how long since it began.
async fn design_activity(State(st): St, Path(track): Path<String>) -> Result<Response, Fail> {
    let (items, now) = st.orch.design_activity(&track).unwrap_or_default();
    Ok(json_nocache(json!({"items": items, "now": now})))
}

async fn suggest(State(st): St, Json(b): Json<GoalBody>) -> Result<Response, Fail> {
    let (flow, notes) = st.orch.suggest_flow(&st, &b.goal, &st.orch.track(b.track.as_deref())).await?;
    // The flow as it always was, plus why each agent on OpenRouter is there.
    let mut v = serde_json::to_value(&flow).map_err(|e| Fail::Internal(e.into()))?;
    v["notes"] = json!(notes);
    Ok(json_nocache(v))
}

#[derive(Deserialize)]
struct EditBody {
    flow: Flow,
    request: String,
    engine: super::graph::Engine,
}

/// Change a flow from a plain request, with Claude or Codex. The flow is not saved: the page shows it and the user
/// can undo.
async fn edit_flow(State(st): St, Json(b): Json<EditBody>) -> Result<Response, Fail> {
    let (flow, summary) = st.orch.edit_flow(&st, b.flow, &b.request, b.engine).await?;
    Ok(json_nocache(json!({ "flow": flow, "summary": summary })))
}

#[derive(Deserialize)]
struct SaveBody(Flow);

async fn save_flow(State(st): St, Path(id): Path<String>, Json(SaveBody(flow)): Json<SaveBody>) -> Result<Response, Fail> {
    if flow.id != id {
        return Err(Fail::Bad("the flow's id does not match the address".into()));
    }
    st.orch.save_flow(&flow)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct FolderBody {
    folder: String,
}

async fn move_flow(State(st): St, Path(id): Path<String>, Json(b): Json<FolderBody>) -> Result<Response, Fail> {
    st.orch.move_flow(&id, &b.folder)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct PathBody {
    path: String,
}

#[derive(Deserialize)]
struct NewFolderBody {
    path: String,
    /// A directory to link it to.
    #[serde(default)]
    dir: Option<String>,
}

async fn create_folder(State(st): St, Json(b): Json<NewFolderBody>) -> Result<Response, Fail> {
    Ok(json_nocache(json!({"path": st.orch.create_folder(&b.path, b.dir.as_deref())?})))
}

#[derive(Deserialize)]
struct LinkBody {
    path: String,
    /// The directory to link it to; left out or empty takes the link off.
    #[serde(default)]
    dir: Option<String>,
}

async fn link_folder(State(st): St, Json(b): Json<LinkBody>) -> Result<Response, Fail> {
    Ok(json_nocache(json!({"dir": st.orch.link_folder(&b.path, b.dir.as_deref())?})))
}

#[derive(Deserialize)]
struct RenameBody {
    from: String,
    to: String,
}

async fn rename_folder(State(st): St, Json(b): Json<RenameBody>) -> Result<Response, Fail> {
    Ok(json_nocache(json!({"moved": st.orch.rename_folder(&b.from, &b.to)?})))
}

async fn delete_folder(State(st): St, Json(b): Json<PathBody>) -> Result<Response, Fail> {
    st.orch.delete_folder(&b.path)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn delete_flow(State(st): St, Path(id): Path<String>) -> Result<Response, Fail> {
    st.orch.store.delete_flow(&id).map_err(|_| Fail::Bad("bad id".into()))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct NodePromptBody {
    role: Role,
    /// A custom role's definition; for a built-in role it is read from the catalog.
    custom: Option<super::graph::CustomRole>,
    name: String,
    goal: String,
}

async fn node_prompt(State(st): St, Json(b): Json<NodePromptBody>) -> Result<Response, Fail> {
    let (label, capability, output) = match (&b.custom, super::roles::def_for(b.role)) {
        (Some(c), _) => (c.label.clone(), c.capability, c.output),
        (None, Some(d)) => (d.label.to_string(), d.capability, d.output),
        (None, None) => return Err(Fail::Bad("that role has no definition".into())),
    };
    let p = st.orch.node_prompt(&label, capability, output, &b.name, &b.goal).await?;
    Ok(json_nocache(json!({"systemPrompt": p})))
}

#[derive(Deserialize)]
struct CustomRoleBody {
    description: String,
}

/// Design a custom role from a description. Nothing is saved: the page puts it on the canvas for the user to edit.
async fn custom_role(State(st): St, Json(b): Json<CustomRoleBody>) -> Result<Response, Fail> {
    let r = st.orch.custom_role(&b.description).await?;
    Ok(json_nocache(json!({
        "label": r.role.label,
        "capability": r.role.capability,
        "output": r.role.output,
        "instructions": r.instructions,
    })))
}

/// The built-in roles, for the page to offer.
async fn roles() -> Response {
    json_nocache(super::roles::catalog())
}

async fn list_runs(State(st): St) -> Response {
    json_nocache(st.orch.store.list_runs())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody {
    flow: Flow,
    goal: String,
    project: String,
    #[serde(default)]
    skip_grill: bool,
    /// Fetch the pages the goal links to, when there is a web account. On unless the user turns it off.
    #[serde(default = "yes")]
    read_links: bool,
    /// Documentation sites to crawl into each builder's folder to read.
    #[serde(default)]
    docs: Vec<String>,
    /// The brief to start from, with `skip_grill`: running an earlier run again keeps the brief it was given.
    #[serde(default)]
    brief: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct MoreRounds {
    extra: u8,
}

async fn more_rounds(State(st): St, Path(id): Path<String>, Json(b): Json<MoreRounds>) -> Result<Response, Fail> {
    Ok(json_nocache(st.orch.more_rounds(&st, &id, b.extra)?))
}

async fn create_run(State(st): St, Json(b): Json<CreateBody>) -> Result<Response, Fail> {
    let run = st.orch.create_run(&st, b.flow, b.goal, &b.project, b.skip_grill, b.read_links, b.docs, b.brief).await?;
    Ok((StatusCode::CREATED, Json(run)).into_response())
}

async fn get_run(State(st): St, Path(id): Path<String>) -> Result<Response, Fail> {
    Ok(json_nocache(st.orch.get(&id)?))
}

async fn delete_run(State(st): St, Path(id): Path<String>) -> Result<Response, Fail> {
    st.orch.delete_run(&id)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize, Default)]
struct AnswersBody {
    #[serde(default)]
    answers: Vec<String>,
}

async fn grill(State(st): St, Path(id): Path<String>, body: Option<Json<AnswersBody>>) -> Result<Response, Fail> {
    let answers = body.map(|Json(b)| b.answers).unwrap_or_default();
    Ok(json_nocache(st.orch.grill(&id, answers).await?))
}

#[derive(Deserialize, Default)]
struct StartBody {
    brief: Option<String>,
}

async fn start(State(st): St, Path(id): Path<String>, body: Option<Json<StartBody>>) -> Result<Response, Fail> {
    let brief = body.and_then(|Json(b)| b.brief);
    Ok(json_nocache(st.orch.start(&st, &id, brief)?))
}

#[derive(Deserialize)]
struct ApproveBody {
    node: String,
}

async fn approve(State(st): St, Path(id): Path<String>, Json(b): Json<ApproveBody>) -> Result<Response, Fail> {
    Ok(json_nocache(st.orch.approve(&st, &id, &b.node)?))
}

#[derive(Deserialize, Default)]
struct ResumeBody {
    /// Carry on past the plan limits for the rest of the run.
    #[serde(default)]
    waive: bool,
}

async fn limits(State(st): St) -> Response {
    json_nocache(st.orch.limits())
}

async fn set_limits(State(st): St, Json(b): Json<super::graph::Budget>) -> Result<Response, Fail> {
    Ok(json_nocache(st.orch.set_limits(b)?))
}

async fn resume(State(st): St, Path(id): Path<String>, body: Option<Json<ResumeBody>>) -> Result<Response, Fail> {
    let waive = body.map(|Json(b)| b.waive).unwrap_or_default();
    Ok(json_nocache(st.orch.resume(&st, &id, waive)?))
}

#[derive(Deserialize)]
struct RunAnswersBody {
    answers: Vec<String>,
}

async fn answer(State(st): St, Path(id): Path<String>, Json(b): Json<RunAnswersBody>) -> Result<Response, Fail> {
    Ok(json_nocache(st.orch.answer(&st, &id, b.answers)?))
}

async fn turn(State(st): St, Path((id, n)): Path<(String, u32)>) -> Result<Response, Fail> {
    Ok(json_nocache(st.orch.turn(&id, n)?))
}

async fn cancel(State(st): St, Path(id): Path<String>) -> Result<Response, Fail> {
    Ok(json_nocache(st.orch.cancel(&id)?))
}

async fn pop_out(State(st): St, Path(id): Path<String>, Json(b): Json<ApproveBody>) -> Result<Response, Fail> {
    let session = st.orch.pop_out(&st, &id, &b.node).await?;
    Ok(json_nocache(json!({ "session": session })))
}

#[derive(Deserialize)]
struct ProjectQuery {
    path: String,
}

fn project_dir(path: &str) -> Result<std::path::PathBuf, Fail> {
    let dir = std::fs::canonicalize(path.trim()).map_err(|_| Fail::Bad("that folder does not exist".into()))?;
    if dir.is_dir() { Ok(dir) } else { Err(Fail::Bad("that is not a folder".into())) }
}

/// Is this folder ready for builders: a git repository with a commit?
async fn project(Query(q): Query<ProjectQuery>) -> Result<Response, Fail> {
    // A folder that is not there is an answer, not a failure: the page asks as the user types.
    let Ok(dir) = project_dir(&q.path) else { return Ok(json_nocache(json!({ "state": "missing" }))) };
    Ok(json_nocache(json!({ "state": super::worktree::repo_state(&dir).await })))
}

/// Make a folder ready for builders: `git init` and a first commit of what is in it.
async fn project_init(Json(q): Json<ProjectQuery>) -> Result<Response, Fail> {
    let dir = project_dir(&q.path)?;
    let state = super::worktree::init(&dir).await.map_err(|e| Fail::Bad(format!("could not set up git there: {e:#}")))?;
    Ok(json_nocache(json!({ "state": state })))
}

#[derive(Deserialize)]
struct EventsQuery {
    run: String,
}

/// Live updates for one run: the whole run as JSON now, and again after every change.
async fn events(State(st): St, Query(q): Query<EventsQuery>, ws: WebSocketUpgrade) -> Result<Response, Fail> {
    // Subscribe before reading, so a change between the two is not lost.
    let rx = st.orch.subscribe();
    let first = serde_json::to_string(&st.orch.get(&q.run)?).map_err(|e| Fail::Internal(e.into()))?;
    Ok(ws.on_upgrade(move |socket| stream(socket, rx, q.run, first)))
}

async fn stream(
    mut socket: WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<(String, Arc<String>)>,
    run: String,
    first: String,
) {
    if socket.send(Message::text(first)).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok((id, json)) if id == run => {
                    if socket.send(Message::text(json.as_str())).await.is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                // Missed some changes: the next one carries the whole run, so just carry on.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            },
            // The page closing the socket, or sending anything, is the signal to stop.
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            },
        }
    }
}
