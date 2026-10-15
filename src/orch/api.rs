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
        .route("/api/orch/flows/{id}", axum::routing::put(save_flow).delete(delete_flow))
        .route("/api/orch/node-prompt", post(node_prompt))
        .route("/api/orch/runs", get(list_runs).post(create_run))
        .route("/api/orch/runs/{id}", get(get_run))
        .route("/api/orch/runs/{id}/grill", post(grill))
        .route("/api/orch/runs/{id}/start", post(start))
        .route("/api/orch/runs/{id}/approve", post(approve))
        .route("/api/orch/runs/{id}/resume", post(resume))
        .route("/api/orch/runs/{id}/cancel", post(cancel))
        .route("/api/orch/events", get(events))
}

async fn models(State(st): St) -> Response {
    json_nocache(st.orch.models().await)
}

async fn flows(State(st): St) -> Response {
    json_nocache(st.orch.flows())
}

#[derive(Deserialize)]
struct GoalBody {
    goal: String,
}

async fn suggest(State(st): St, Json(b): Json<GoalBody>) -> Result<Response, Fail> {
    Ok(json_nocache(st.orch.suggest_flow(&st, &b.goal).await?))
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

async fn delete_flow(State(st): St, Path(id): Path<String>) -> Result<Response, Fail> {
    st.orch.store.delete_flow(&id).map_err(|_| Fail::Bad("bad id".into()))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct NodePromptBody {
    role: Role,
    name: String,
    goal: String,
}

async fn node_prompt(State(st): St, Json(b): Json<NodePromptBody>) -> Result<Response, Fail> {
    let p = st.orch.node_prompt(b.role, &b.name, &b.goal).await?;
    Ok(json_nocache(json!({"systemPrompt": p})))
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
}

async fn create_run(State(st): St, Json(b): Json<CreateBody>) -> Result<Response, Fail> {
    let run = st.orch.create_run(&st, b.flow, b.goal, &b.project, b.skip_grill).await?;
    Ok((StatusCode::CREATED, Json(run)).into_response())
}

async fn get_run(State(st): St, Path(id): Path<String>) -> Result<Response, Fail> {
    Ok(json_nocache(st.orch.get(&id)?))
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

async fn resume(State(st): St, Path(id): Path<String>) -> Result<Response, Fail> {
    Ok(json_nocache(st.orch.resume(&st, &id)?))
}

async fn cancel(State(st): St, Path(id): Path<String>) -> Result<Response, Fail> {
    Ok(json_nocache(st.orch.cancel(&id)?))
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
