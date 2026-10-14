use std::sync::Arc;

use axum::{
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::{AppState, pty::PtyClient, sessions, tmux};

const MIN_DIM: u16 = 2;
const MAX_DIM: u16 = 1000;

#[derive(Deserialize)]
pub struct AttachQuery {
    session: String,
    rows: Option<u16>,
    cols: Option<u16>,
}

/// Client → server control messages (text frames). Terminal input is binary.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMsg {
    Resize { rows: u16, cols: u16 },
    Ping,
    /// Scroll the history: positive goes up, negative down, zero asks where it stands.
    Scroll { lines: i32 },
    /// Back to the live screen.
    ScrollEnd,
}

/// Server → client control messages (text frames). Terminal output is binary.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMsg {
    Pong,
    /// Where the history view stands after a scroll.
    Scroll { active: bool, position: u32 },
    /// The tmux session is gone but remembered: show its saved output and offer a restore.
    Stopped { resumable: bool },
    Exit { code: u32 },
    Error { message: String },
}

fn clamp(v: u16) -> u16 {
    v.clamp(MIN_DIM, MAX_DIM)
}

/// Reject cross-site WebSocket hijacking: Origin must match Host.
fn origin_ok(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true; // non-browser client; the auth cookie is still required
    };
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin_host = origin.split_once("://").map(|(_, h)| h);
    host.is_some() && host == origin_host
}

pub async fn handler(
    State(st): State<Arc<AppState>>,
    Query(q): Query<AttachQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !origin_ok(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !tmux::valid_session_id(&q.session) {
        return (StatusCode::BAD_REQUEST, "invalid session id").into_response();
    }
    ws.on_upgrade(move |sock| run(sock, st, q))
}

async fn send_json(sock: &mut WebSocket, msg: &ServerMsg) -> bool {
    match serde_json::to_string(msg) {
        Ok(s) => sock.send(Message::Text(s.into())).await.is_ok(),
        Err(_) => false,
    }
}

async fn run(mut sock: WebSocket, st: Arc<AppState>, q: AttachQuery) {
    let (rows, cols) = (clamp(q.rows.unwrap_or(24)), clamp(q.cols.unwrap_or(80)));
    if !tmux::exists(&q.session).await {
        let remembered = st.sessions.get(&q.session).map(|m| m.agent().is_some());
        if let Some(resumable) = remembered {
            send_json(&mut sock, &ServerMsg::Stopped { resumable }).await;
            return;
        }
        let req = sessions::NewSession { preset: None, name: Some(&q.session), group: None, exact: true, yolo: false, dir: None };
        if let Err(e) = sessions::create(&st, req).await {
            send_json(&mut sock, &ServerMsg::Error { message: e.to_string() }).await;
            return;
        }
    }
    let args = tmux::attach_args(&q.session);
    let mut pty = match PtyClient::spawn(args, rows, cols) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("attach {}: {e:#}", q.session);
            send_json(&mut sock, &ServerMsg::Error { message: e.to_string() }).await;
            return;
        }
    };
    tracing::info!("attached {} ({cols}x{rows})", q.session);

    loop {
        tokio::select! {
            out = pty.output.recv() => match out {
                Some(bytes) => {
                    if sock.send(Message::Binary(bytes.into())).await.is_err() { break; }
                }
                // Reader hit EOF: the tmux client is gone; report its exit code.
                None => {
                    let code = pty.exited.recv().await.unwrap_or(0);
                    send_json(&mut sock, &ServerMsg::Exit { code }).await;
                    break;
                }
            },
            msg = sock.recv() => match msg {
                Some(Ok(Message::Binary(b))) => {
                    if pty.input.send(b.to_vec()).await.is_err() { break; }
                }
                Some(Ok(Message::Text(t))) => match serde_json::from_str::<ClientMsg>(&t) {
                    Ok(ClientMsg::Resize { rows, cols }) => {
                        if let Err(e) = pty.resize(clamp(rows), clamp(cols)) {
                            tracing::warn!("resize: {e:#}");
                        }
                    }
                    Ok(ClientMsg::Ping) => {
                        if !send_json(&mut sock, &ServerMsg::Pong).await { break; }
                    }
                    Ok(ClientMsg::Scroll { lines }) => match tmux::scroll(&q.session, lines).await {
                        Ok(st) => {
                            if !send_json(&mut sock, &ServerMsg::Scroll { active: st.active, position: st.position }).await { break; }
                        }
                        Err(e) => {
                            tracing::debug!("scroll {}: {e:#}", q.session);
                            // Answer anyway: the client waits for it before sending the next.
                            if !send_json(&mut sock, &ServerMsg::Scroll { active: false, position: 0 }).await { break; }
                        }
                    },
                    Ok(ClientMsg::ScrollEnd) => {
                        if let Err(e) = tmux::search_close(&q.session).await {
                            tracing::debug!("scroll end {}: {e:#}", q.session);
                        }
                        if !send_json(&mut sock, &ServerMsg::Scroll { active: false, position: 0 }).await { break; }
                    }
                    Err(e) => tracing::debug!("bad control message: {e}"),
                },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }

    tracing::info!("detached {}", q.session);
    // Dropping `pty` kills the tmux *client*; the session keeps running.
}
