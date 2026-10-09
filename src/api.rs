use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::{ConnectInfo, DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;

use crate::{
    AppState, assets, auth::COOKIE_NAME, config::{Agent, RestartPolicy}, modelmenu, modes, notify, ops, resume, sessions, suggest, tmux, uploads, usage,
};

pub fn router(state: Arc<AppState>) -> Router {
    let protected = Router::new()
        .route("/api/me", get(|| async { StatusCode::NO_CONTENT }))
        .route("/ws", get(crate::ws::handler))
        .route("/api/push/key", get(push_key))
        .route("/api/push/subscribe", post(push_subscribe))
        .route("/api/push/unsubscribe", post(push_unsubscribe))
        .route("/api/push/test", post(push_test))
        .route("/api/presets", get(list_presets))
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/api/sessions/{id}", delete(kill_session))
        .route("/api/sessions/{id}/rename", post(rename_session))
        .route("/api/sessions/{id}/restore", post(restore_session))
        .route("/api/sessions/{id}/output", get(session_output))
        .route("/api/sessions/{id}/screen", get(session_screen))
        .route("/api/sessions/{id}/chat", get(session_chat))
        .route("/api/sessions/{id}/keys", post(session_keys))
        .route("/api/sessions/{id}/message", post(session_message))
        .route("/api/sessions/{id}/question", post(session_question))
        .route("/api/sessions/{id}/suggest", get(session_suggest))
        .route("/api/sessions/{id}/resume/open", post(resume_open))
        .route("/api/sessions/{id}/resume/search", post(resume_search))
        .route("/api/sessions/{id}/resume/scope", post(resume_scope))
        .route("/api/sessions/{id}/resume/page", post(resume_page))
        .route("/api/sessions/{id}/resume/choose", post(resume_choose))
        .route("/api/sessions/{id}/model/open", post(model_open))
        .route("/api/sessions/{id}/model/choose", post(model_choose))
        .route("/api/sessions/{id}/model/level", post(model_level))
        .route("/api/sessions/{id}/model/close", post(model_close))
        .route("/api/sessions/{id}/effort", post(set_effort))
        .route("/api/sessions/{id}/mode", post(set_mode))
        .route("/api/sessions/{id}/reasoning", post(set_reasoning))
        .route("/api/sessions/{id}/command", post(run_command))
        .route("/api/sessions/{id}/permissions/open", post(perms_open))
        .route("/api/sessions/{id}/permissions/choose", post(perms_choose))
        .route(
            "/api/sessions/{id}/upload",
            post(session_upload).layer(DefaultBodyLimit::max(uploads::MAX_BYTES + 4096)),
        )
        .route("/api/sessions/{id}/attachments", get(session_attachments))
        .route("/api/uploads/{uid}/{file}", get(get_upload))
        .route("/api/sessions/{id}/answer", post(session_answer))
        .route("/api/sessions/{id}/meta", post(session_meta))
        .route("/api/usage", get(get_usage))
        .route("/api/dirs", get(recent_dirs))
        .route("/api/groups", get(list_groups))
        .route("/api/groups/{name}/start", post(start_group))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    Router::new()
        // Called by hook processes inside sessions: token + loopback instead of the cookie.
        .route("/api/internal/notify", post(internal_notify))
        .route("/api/internal/usage", post(internal_usage))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .merge(protected)
        .fallback(assets::serve)
        .with_state(state)
}

pub async fn require_auth(
    State(st): State<Arc<AppState>>,
    jar: CookieJar,
    req: Request,
    next: Next,
) -> Response {
    let ok = jar
        .get(COOKIE_NAME)
        .is_some_and(|c| st.auth.check_token(c.value()));
    if ok {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

fn client_ip(st: &AppState, headers: &HeaderMap, peer: SocketAddr) -> std::net::IpAddr {
    if st.cfg.trust_proxy
        && let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            // The last entry is the one our own proxy appended; earlier ones are client-supplied.
            .and_then(|v| v.split(',').next_back())
            .and_then(|v| v.trim().parse().ok())
    {
        return ip;
    }
    peer.ip()
}

fn is_https(st: &AppState, headers: &HeaderMap) -> bool {
    st.cfg.trust_proxy
        && headers
            .get("x-forwarded-proto")
            .is_some_and(|v| v.as_bytes() == b"https")
}

async fn login(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<LoginBody>,
) -> Response {
    let ip = client_ip(&st, &headers, peer);
    if !st.auth.may_attempt(ip) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    // Argon2 is CPU-heavy; keep it off the async workers.
    let st2 = st.clone();
    let ok = tokio::task::spawn_blocking(move || st2.auth.verify_password(&body.password))
        .await
        .unwrap_or(false);
    st.auth.record(ip, ok);
    if !ok {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let cookie = Cookie::build((COOKIE_NAME, st.auth.issue_token()))
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(is_https(&st, &headers))
        .path("/")
        .max_age(time_from(st.auth.ttl()))
        .build();
    (jar.add(cookie), StatusCode::NO_CONTENT).into_response()
}

async fn logout(jar: CookieJar) -> impl IntoResponse {
    (
        jar.remove(Cookie::build(COOKIE_NAME).path("/")),
        [(header::CACHE_CONTROL, "no-store")],
        StatusCode::NO_CONTENT,
    )
}

fn time_from(d: std::time::Duration) -> time::Duration {
    time::Duration::seconds(i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[derive(serde::Serialize)]
struct PresetView<'a> {
    name: &'a str,
    command: &'a str,
    dir: Option<String>,
    restart: RestartPolicy,
    /// `claude`, `codex` or `other`: only the first two can be started with permissions skipped.
    agent: Agent,
    /// The preset always starts with permissions skipped.
    yolo: bool,
}

async fn list_presets(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let v: Vec<PresetView> = st
        .cfg
        .presets
        .iter()
        .map(|p| PresetView {
            name: &p.name,
            command: &p.command,
            dir: p.dir.as_ref().map(|d| d.display().to_string()),
            restart: p.restart,
            agent: p.agent(),
            yolo: p.yolo,
        })
        .collect();
    Json(serde_json::json!(v))
}

fn server_error(e: &anyhow::Error) -> Response {
    tracing::warn!("{e:#}");
    (StatusCode::BAD_REQUEST, e.to_string()).into_response()
}

async fn list_sessions(State(st): State<Arc<AppState>>) -> Response {
    match sessions::snapshot(&st).await {
        Ok(mut v) => {
            sessions::enrich(&st, &mut v).await;
            Json(v).into_response()
        }
        Err(e) => server_error(&e),
    }
}

#[derive(Deserialize)]
struct CreateBody {
    preset: Option<String>,
    name: Option<String>,
    /// Start Claude or Codex with permission checks skipped.
    #[serde(default)]
    yolo: bool,
    /// Start in this folder (from `/api/dirs`).
    dir: Option<String>,
}

async fn recent_dirs(State(st): State<Arc<AppState>>) -> Response {
    Json(sessions::recent_dirs(&st, 8).await).into_response()
}

async fn create_session(State(st): State<Arc<AppState>>, Json(b): Json<CreateBody>) -> Response {
    let req = sessions::NewSession {
        preset: b.preset.as_deref(),
        name: b.name.as_deref(),
        group: None,
        exact: false,
        yolo: b.yolo,
        dir: b.dir.map(std::path::PathBuf::from),
    };
    match sessions::create(&st, req).await {
        Ok(id) => (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response(),
        Err(e) => server_error(&e),
    }
}

async fn kill_session(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if !tmux::valid_session_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match sessions::kill(&st, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => server_error(&e),
    }
}

#[derive(Deserialize)]
struct RenameBody {
    name: String,
}

async fn rename_session(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(b): Json<RenameBody>,
) -> Response {
    if !tmux::valid_session_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match sessions::rename(&st, &id, &b.name).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => server_error(&e),
    }
}

const MAX_SUBSCRIPTIONS: usize = 20;

async fn push_key(State(st): State<Arc<AppState>>) -> Response {
    match notify::public_key(&st.vapid_private) {
        Ok(key) => Json(serde_json::json!({ "key": key })).into_response(),
        Err(e) => server_error(&e),
    }
}

async fn push_subscribe(State(st): State<Arc<AppState>>, Json(sub): Json<web_push::SubscriptionInfo>) -> Response {
    // The server will POST to this URL, so only accept public-looking https endpoints.
    if !sub.endpoint.starts_with("https://") || sub.endpoint.len() > 2048 {
        return (StatusCode::BAD_REQUEST, "endpoint must be an https URL").into_response();
    }
    {
        let Ok(mut subs) = st.subscriptions.lock() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        subs.retain(|s| s.endpoint != sub.endpoint);
        if subs.len() >= MAX_SUBSCRIPTIONS {
            subs.remove(0);
        }
        subs.push(sub);
    }
    sessions::save(&st).await;
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct UnsubBody {
    endpoint: String,
}

async fn push_unsubscribe(State(st): State<Arc<AppState>>, Json(b): Json<UnsubBody>) -> Response {
    if let Ok(mut subs) = st.subscriptions.lock() {
        subs.retain(|s| s.endpoint != b.endpoint);
    }
    sessions::save(&st).await;
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct TestBody {
    session: Option<String>,
}

async fn push_test(State(st): State<Arc<AppState>>, Json(b): Json<TestBody>) -> Response {
    let session = b.session.filter(|s| tmux::valid_session_id(s)).unwrap_or_else(|| "main".into());
    let n = st.subscriptions.lock().map_or(0, |g| g.len());
    tokio::spawn(notify::send_arc(
        st,
        notify::Alert::new(&session, notify::Kind::Test, Some("Notifications are working".into())),
    ));
    Json(serde_json::json!({ "devices": n })).into_response()
}

/// Hook processes call the internal endpoints from the same machine with the shared token.
fn internal_caller_ok(st: &AppState, peer: SocketAddr, headers: &HeaderMap) -> bool {
    use subtle::ConstantTimeEq;
    let presented = headers.get("x-ttm-token").map_or(&[][..], |v| v.as_bytes());
    let token_ok: bool = presented.ct_eq(st.notify_token.as_bytes()).into();
    peer.ip().is_loopback() && token_ok
}

#[derive(Deserialize, Default)]
struct UsageQuery {
    /// `1` when the panel is opened: accept data only a minute old instead of five.
    refresh: Option<u8>,
}

async fn get_usage(State(st): State<Arc<AppState>>, Query(q): Query<UsageQuery>) -> Response {
    Json(usage::snapshot(&st, q.refresh.unwrap_or(0) != 0).await).into_response()
}

/// Claude Code's statusLine reports plan-limit usage here while it runs inside a session.
async fn internal_usage(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    if !internal_caller_ok(&st, peer, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    usage::ingest_statusline(&st, &payload);
    StatusCode::NO_CONTENT.into_response()
}

async fn internal_notify(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(b): Json<notify::NotifyBody>,
) -> Response {
    if !internal_caller_ok(&st, peer, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ops::record_agent_session(&st, &b.uid, b.agent_session_id.as_deref(), b.cwd.as_deref()).await;
    let Some(session) = sessions::name_for_uid(&st, &b.uid) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match b.kind {
        notify::Kind::Session => {}
        notify::Kind::NeedsInput => {
            tokio::spawn(notify::needs_input(st, session, b.message));
        }
        kind => {
            tokio::spawn(notify::send_arc(st, notify::Alert::new(&session, kind, b.message)));
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

fn valid_id(id: &str) -> bool {
    tmux::valid_session_id(id)
}

#[derive(Deserialize, Default)]
struct RestoreBody {
    #[serde(default)]
    fresh: bool,
}

async fn restore_session(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<RestoreBody>>,
) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let fresh = body.is_some_and(|b| b.fresh);
    match sessions::restore(&st, &id, fresh).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => server_error(&e),
    }
}

#[derive(Deserialize)]
struct OutputQuery {
    lines: Option<usize>,
}

async fn session_output(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<OutputQuery>,
) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match ops::output(&st, &id, q.lines.unwrap_or(200)).await {
        Ok(text) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], text)
            .into_response(),
        Err(e) => server_error(&e),
    }
}

#[derive(Deserialize, Default)]
struct ChatQuery {
    /// The `epoch` of the response the client last applied; a different one means start over.
    epoch: Option<u64>,
    /// The `seq` of that response: only newer changes are wanted.
    after: Option<u64>,
}

/// The conversation as chat items, for the chat view. Poll with the last `epoch` and `seq`.
async fn session_chat(State(st): State<Arc<AppState>>, Path(id): Path<String>, Query(q): Query<ChatQuery>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match crate::chat::snapshot(&st, &id, q.epoch, q.after.unwrap_or(0)).await {
        Ok(r) => ([(header::CACHE_CONTROL, "no-store")], Json(r)).into_response(),
        Err(e) => server_error(&e),
    }
}

async fn session_screen(Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match ops::screen(&id).await {
        Ok(a) => ([(header::CACHE_CONTROL, "no-store")], Json(a)).into_response(),
        Err(e) => server_error(&e),
    }
}

#[derive(Deserialize)]
struct KeysBody {
    text: Option<String>,
    #[serde(default)]
    keys: Vec<String>,
    #[serde(default)]
    enter: bool,
}

async fn session_keys(Path(id): Path<String>, Json(b): Json<KeysBody>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match ops::send(&id, b.text.as_deref(), &b.keys, b.enter).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => server_error(&e),
    }
}

#[derive(Deserialize)]
struct MessageBody {
    text: String,
}

/// A chat message: typed into the agent and submitted.
async fn session_message(Path(id): Path<String>, Json(b): Json<MessageBody>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match ops::send_message(&id, &b.text).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => server_error(&e),
    }
}

/// Open Claude's `/model` menu and read its list. The menu stays open until a choice or a close.
async fn model_open(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _typing = st.driver.lock().await;
    let opened = match st.sessions.get(&id).and_then(|m| m.agent()) {
        Some(Agent::Codex) => modelmenu::codex_open(&st, &id).await,
        _ => modelmenu::open(&st, &id).await,
    };
    match opened {
        Ok(menu) => Json(menu).into_response(),
        Err(e) => {
            let _ = modelmenu::close(&id).await; // do not leave a half-opened menu behind
            server_error(&e)
        }
    }
}

#[derive(Deserialize)]
struct ChooseModel {
    number: usize,
}

/// With the model menu open, switch this session to the model on row `number`. Claude applies it
/// at once (`{message}`). Codex then needs a reasoning level, so it answers with the levels
/// (`{levels}`) and keeps its menu open for `model/level`.
async fn model_choose(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(b): Json<ChooseModel>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _typing = st.driver.lock().await;
    let reply = if st.sessions.get(&id).and_then(|m| m.agent()) == Some(Agent::Codex) {
        modelmenu::codex_choose(&id, b.number).await.map(|levels| serde_json::json!({ "levels": levels }))
    } else {
        modelmenu::choose(&id, b.number).await.map(|message| serde_json::json!({ "message": message }))
    };
    match reply {
        Ok(v) => Json(v).into_response(),
        Err(e) => {
            let _ = modelmenu::close(&id).await;
            server_error(&e)
        }
    }
}

/// Codex only: with its reasoning-level screen open, apply row `number` to this session.
async fn model_level(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(b): Json<ChooseModel>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _typing = st.driver.lock().await;
    match modelmenu::codex_level(&id, b.number).await {
        Ok(message) => Json(serde_json::json!({ "message": message })).into_response(),
        Err(e) => {
            let _ = modelmenu::close(&id).await;
            server_error(&e)
        }
    }
}

/// Open the agent's own `/resume` list and read it. It stays open until a pick or `model/close`.
async fn resume_open(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _typing = st.driver.lock().await;
    match resume::open(&st, &id).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => {
            let _ = modelmenu::close(&id).await; // do not leave a half-opened list behind
            server_error(&e)
        }
    }
}

#[derive(Deserialize)]
struct ResumeSearch {
    query: String,
    /// How many characters are in the box now, to erase first.
    #[serde(default)]
    previous: usize,
}

async fn resume_search(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(b): Json<ResumeSearch>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _typing = st.driver.lock().await;
    match resume::search(&st, &id, &b.query, b.previous).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => server_error(&e),
    }
}

async fn resume_scope(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _typing = st.driver.lock().await;
    match resume::scope(&st, &id).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => server_error(&e),
    }
}

#[derive(Deserialize)]
struct ResumePage {
    down: bool,
}

async fn resume_page(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(b): Json<ResumePage>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _typing = st.driver.lock().await;
    match resume::page(&st, &id, b.down).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => server_error(&e),
    }
}

#[derive(Deserialize)]
struct ResumeChoose {
    title: String,
    meta: String,
    /// Where the row was in the list the caller saw, to tell apart rows that look the same.
    #[serde(default)]
    index: usize,
}

/// Resume the conversation with this title and meta line. The list closes as it opens.
async fn resume_choose(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(b): Json<ResumeChoose>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _typing = st.driver.lock().await;
    match resume::choose_for(&st, &id, &b.title, &b.meta, b.index).await {
        Ok(message) => Json(serde_json::json!({ "message": message })).into_response(),
        Err(e) => {
            let _ = modelmenu::close(&id).await;
            server_error(&e)
        }
    }
}

async fn model_close(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _typing = st.driver.lock().await;
    match modelmenu::close(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => server_error(&e),
    }
}

#[derive(Deserialize)]
struct ModeBody {
    mode: modes::Mode,
}

/// Put the agent in plan, build or another mode.
async fn set_mode(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(b): Json<ModeBody>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _typing = st.driver.lock().await;
    match modes::switch(&st, &id, b.mode).await {
        Ok(mode) => Json(serde_json::json!({ "mode": mode })).into_response(),
        Err(e) => server_error(&e),
    }
}

#[derive(Deserialize)]
struct ReasoningBody {
    /// `true` for more reasoning, `false` for less.
    up: bool,
}

/// Codex only: one step of reasoning up or down.
async fn set_reasoning(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(b): Json<ReasoningBody>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _typing = st.driver.lock().await;
    match modes::codex_reasoning(&st, &id, b.up).await {
        Ok(level) => Json(serde_json::json!({ "level": level })).into_response(),
        Err(e) => server_error(&e),
    }
}

#[derive(Deserialize)]
struct CommandBody {
    name: String,
}

/// A one-tap command from a short allow-list (`compact`, `context`, `clear`, `new`).
async fn run_command(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(b): Json<CommandBody>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _typing = st.driver.lock().await;
    match modes::run_quick_command(&st, &id, &b.name).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => server_error(&e),
    }
}

/// Codex only: open `/permissions` and read its presets.
async fn perms_open(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _typing = st.driver.lock().await;
    match modes::perms_open(&st, &id).await {
        Ok(menu) => Json(menu).into_response(),
        Err(e) => {
            let _ = modelmenu::close(&id).await;
            server_error(&e)
        }
    }
}

#[derive(Deserialize)]
struct PermsBody {
    number: usize,
    /// You confirmed Full Access in the app.
    #[serde(default)]
    allow_full: bool,
}

/// With the permissions menu open, apply the preset on row `number`.
async fn perms_choose(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(b): Json<PermsBody>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _typing = st.driver.lock().await;
    match modes::perms_choose(&id, b.number, b.allow_full).await {
        Ok(message) => Json(serde_json::json!({ "message": message })).into_response(),
        Err(e) => {
            let _ = modelmenu::close(&id).await;
            server_error(&e)
        }
    }
}

#[derive(Deserialize)]
struct EffortBody {
    level: String,
}

/// Set this session's effort (`low`, `medium`, ...).
async fn set_effort(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(b): Json<EffortBody>) -> Response {
    if !valid_id(&id) || b.level.len() > 16 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let _typing = st.driver.lock().await;
    let done = match st.sessions.get(&id).and_then(|m| m.agent()) {
        Some(Agent::Codex) => modelmenu::codex_effort(&st, &id, &b.level).await,
        _ => modelmenu::set_effort(&st, &id, &b.level).await,
    };
    match done {
        Ok(message) => Json(serde_json::json!({ "message": message })).into_response(),
        Err(e) => {
            let _ = modelmenu::close(&id).await;
            server_error(&e)
        }
    }
}

#[derive(Deserialize)]
struct SuggestQuery {
    /// `slash` or `file`.
    kind: String,
    #[serde(default)]
    q: String,
}

/// Suggestions for the composer: slash commands, or files in the session's folder.
async fn session_suggest(State(st): State<Arc<AppState>>, Path(id): Path<String>, Query(q): Query<SuggestQuery>) -> Response {
    if !valid_id(&id) || q.q.len() > 120 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let (agent, dir) = match st.sessions.get(&id) {
        Some(m) => (m.agent(), suggest::project_dir(m.spec.dir.as_ref())),
        None => return StatusCode::NOT_FOUND.into_response(),
    };
    let items = tokio::task::spawn_blocking(move || match (q.kind.as_str(), agent, dir) {
        ("slash", Some(agent), dir) => suggest::slash(agent, dir.as_deref(), &q.q),
        ("file", _, Some(dir)) => suggest::files(&dir, &q.q),
        _ => Vec::new(),
    })
    .await
    .unwrap_or_default();
    ([(header::CACHE_CONTROL, "no-store")], Json(items)).into_response()
}

/// An image attached in the chat view: saved on the host so the agent can read it by path.
async fn session_upload(State(st): State<Arc<AppState>>, Path(id): Path<String>, body: axum::body::Bytes) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(uid) = st.sessions.get(&id).map(|m| m.uid.clone()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let data_dir = st.data_dir.clone();
    let saved = tokio::task::spawn_blocking({
        let uid = uid.clone();
        move || uploads::save(&data_dir, &uid, &body)
    })
    .await;
    match saved {
        Ok(Ok(path)) => {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
            Json(serde_json::json!({ "path": path, "url": format!("/api/uploads/{uid}/{name}") })).into_response()
        }
        Ok(Err(e)) => server_error(&e),
        Err(e) => server_error(&anyhow::anyhow!("{e}")),
    }
}

/// The images attached in this session's chat, newest first, for the details panel.
async fn session_attachments(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(uid) = st.sessions.get(&id).map(|m| m.uid.clone()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let data_dir = st.data_dir.clone();
    let found = tokio::task::spawn_blocking({
        let uid = uid.clone();
        move || uploads::list(&data_dir, &uid)
    })
    .await
    .unwrap_or_default();
    let items: Vec<_> = found
        .into_iter()
        .map(|f| serde_json::json!({ "name": f.name, "size": f.size, "ts": f.ts, "url": format!("/api/uploads/{uid}/{}", f.name) }))
        .collect();
    ([(header::CACHE_CONTROL, "no-store")], Json(items)).into_response()
}

/// An uploaded image, for the thumbnail in the chat.
async fn get_upload(State(st): State<Arc<AppState>>, Path((uid, file)): Path<(String, String)>) -> Response {
    let Some(path) = uploads::locate(&st.data_dir, &uid, &file) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_owned();
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, uploads::content_type(&ext)),
                (header::CACHE_CONTROL, "private, max-age=86400"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct QuestionBody {
    /// One per question, in order.
    #[serde(default)]
    answers: Vec<crate::question::QuestionAnswer>,
    /// Dismiss the question without answering.
    #[serde(default)]
    dismiss: bool,
}

/// Answer the interactive question the agent is waiting on, or dismiss it.
async fn session_question(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(b): Json<QuestionBody>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match crate::question::answer(&st, &id, &b.answers, b.dismiss).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => server_error(&e),
    }
}

#[derive(Deserialize)]
struct AnswerBody {
    choice: String,
    prompt_id: Option<String>,
}

async fn session_answer(Path(id): Path<String>, Json(b): Json<AnswerBody>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match ops::answer(&id, &b.choice, b.prompt_id.as_deref()).await {
        Ok(label) => Json(serde_json::json!({ "sent": label })).into_response(),
        // 409: the screen no longer shows the question the caller saw.
        Err(e) if e.to_string().contains("question changed") => (StatusCode::CONFLICT, e.to_string()).into_response(),
        Err(e) => server_error(&e),
    }
}

async fn session_meta(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(b): Json<ops::MetaPatch>) -> Response {
    if !valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match ops::set_meta(&st, &id, b).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => server_error(&e),
    }
}

async fn list_groups(State(st): State<Arc<AppState>>) -> Response {
    let v: Vec<_> = st.cfg.groups.iter().map(|g| serde_json::json!({ "name": g.name, "presets": g.presets })).collect();
    Json(v).into_response()
}

async fn start_group(State(st): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    match ops::start_group(&st, &name).await {
        Ok(ids) => (StatusCode::CREATED, Json(serde_json::json!({ "ids": ids }))).into_response(),
        Err(e) => server_error(&e),
    }
}
