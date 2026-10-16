//! A local pass-through between an agent CLI (Claude Code, Codex) and OpenRouter.
//!
//! The CLI is given a *lease* instead of the real key: a random token bound to one account, the
//! model to use now, and a dollar cap. Every request is checked against its lease, has its model
//! and routing preferences rewritten, is streamed to OpenRouter with the real key, and its reply
//! is streamed back unchanged while the usage and cost in it are added to the lease. Changing a
//! lease's model switches a running session without typing into the CLI's menus.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, LazyLock, Mutex, atomic::{AtomicBool, Ordering}},
    task::{Context, Poll},
    time::{Duration, Instant},
};

use axum::{
    body::{Body, Bytes},
    extract::{ConnectInfo, Path as UrlPath, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    AppState,
    secrets::{self, Account, Kind, Prefs},
};

/// Largest request body the proxy takes: a long conversation with images is big.
pub const MAX_BODY: usize = 64 << 20;
const FILE: &str = "openrouter-leases.json";
const SAVE_EVERY: Duration = Duration::from_secs(2);
/// A reply that is one JSON document is kept whole so its usage can be read; this bounds that.
const MAX_WHOLE: usize = 8 << 20;
const MAX_LINE: usize = 4 << 20;
pub const TOKEN_ENV: &str = "TTM_OR_TOKEN";

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(600))
        .build()
        .expect("http client")
});

// ---- usage ---------------------------------------------------------------------------------

/// Tokens and cost of one request, as OpenRouter reports them. Anthropic-style replies count
/// cache tokens apart from `input`; Responses-style replies include them in it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Dollars, as charged. `None` when the reply did not say.
    pub cost: Option<f64>,
}

impl Usage {
    /// Fold in a later report of the same request. Usage is cumulative, so later numbers replace
    /// earlier ones (a stream reports input tokens first and the totals at the end).
    fn merge(&mut self, n: &Usage) {
        self.input = self.input.max(n.input);
        self.output = self.output.max(n.output);
        self.cache_read = self.cache_read.max(n.cache_read);
        self.cache_write = self.cache_write.max(n.cache_write);
        if n.cost.is_some() {
            self.cost = n.cost;
        }
    }
}

/// The usage in one JSON value: a whole reply, or one stream event of any of the three shapes.
pub fn usage_of(v: &Value) -> Option<Usage> {
    let u = v.get("usage").or_else(|| v.pointer("/message/usage")).or_else(|| v.pointer("/response/usage"))?;
    if !u.is_object() {
        return None;
    }
    let first = |paths: &[&str]| paths.iter().find_map(|p| u.pointer(p).and_then(Value::as_u64)).unwrap_or(0);
    Some(Usage {
        input: first(&["/input_tokens", "/prompt_tokens"]),
        output: first(&["/output_tokens", "/completion_tokens"]),
        cache_read: first(&["/cache_read_input_tokens", "/input_tokens_details/cached_tokens", "/prompt_tokens_details/cached_tokens"]),
        cache_write: first(&["/cache_creation_input_tokens", "/prompt_tokens_details/cache_write_tokens"]),
        cost: u.get("cost").and_then(Value::as_f64),
    })
}

/// Reads usage out of a reply as it streams past, without holding the reply.
#[derive(Default)]
pub struct Scan {
    buf: Vec<u8>,
    /// Decided by the first byte: `{` means one JSON document, anything else is server-sent events.
    json: Option<bool>,
    usage: Option<Usage>,
}

impl Scan {
    pub fn push(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        let json = *self.json.get_or_insert_with(|| chunk.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{'));
        self.buf.extend_from_slice(chunk);
        if json {
            if self.buf.len() > MAX_WHOLE {
                self.buf.clear();
                self.json = Some(false); // too big to read; stop trying
            }
            return;
        }
        while let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            self.line(&line);
        }
        if self.buf.len() > MAX_LINE {
            self.buf.clear();
        }
    }

    fn line(&mut self, line: &[u8]) {
        let t = line.trim_ascii();
        let Some(rest) = t.strip_prefix(b"data:") else { return };
        let rest = rest.trim_ascii();
        if rest.starts_with(b"{") && rest.windows(7).any(|w| w == b"\"usage\"") {
            self.take(rest);
        }
    }

    fn take(&mut self, bytes: &[u8]) {
        if let Some(n) = serde_json::from_slice::<Value>(bytes).ok().as_ref().and_then(usage_of) {
            match &mut self.usage {
                Some(u) => u.merge(&n),
                None => self.usage = Some(n),
            }
        }
    }

    pub fn finish(&mut self) -> Option<Usage> {
        let rest = std::mem::take(&mut self.buf);
        if self.json == Some(true) {
            self.take(&rest);
        } else if !rest.is_empty() {
            self.line(&rest);
        }
        self.usage.take()
    }
}

// ---- leases --------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Spend {
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost_usd: f64,
    /// Requests whose reply carried no cost, so `cost_usd` undercounts by their share.
    pub unpriced: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lease {
    pub account: String,
    pub model: String,
    /// What the lease is for, such as a session name or `run:node`. Looked up by the page.
    pub tag: String,
    pub cap_usd: Option<f64>,
    #[serde(default)]
    pub spent: Spend,
}

impl Lease {
    pub fn over_cap(&self) -> bool {
        self.cap_usd.is_some_and(|c| self.spent.cost_usd >= c)
    }
}

/// Every live lease, kept on disk so a session that outlives a ttm restart keeps working.
pub struct Leases {
    map: Arc<DashMap<String, Lease>>,
    path: Option<PathBuf>,
    last_save: Arc<Mutex<Instant>>,
    /// A save is already waiting out the spacing, and will write whatever has changed by then.
    pending: Arc<AtomicBool>,
    every: Duration,
}

fn new_token() -> String {
    let mut b = [0u8; 24];
    rand::fill(&mut b[..]);
    format!("ttm-or-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

impl Leases {
    pub fn new() -> Self {
        Self { map: Arc::default(), path: None, last_save: Arc::new(Mutex::new(Instant::now() - SAVE_EVERY)), pending: Arc::default(), every: SAVE_EVERY }
    }

    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join(FILE);
        let map = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<std::collections::BTreeMap<String, Lease>>(&b).ok())
            .map(|m| m.into_iter().collect::<DashMap<_, _>>())
            .unwrap_or_default();
        Self { map: Arc::new(map), path: Some(path), last_save: Arc::new(Mutex::new(Instant::now() - SAVE_EVERY)), pending: Arc::default(), every: SAVE_EVERY }
    }

    /// Hand out a token for `account` that runs `model`, replacing any lease the same `tag` held.
    pub fn mint(&self, account: &str, model: &str, tag: &str, cap_usd: Option<f64>) -> String {
        self.revoke_tag(tag);
        let token = new_token();
        self.map.insert(token.clone(), Lease { account: account.into(), model: model.into(), tag: tag.into(), cap_usd, spent: Spend::default() });
        self.save(true);
        token
    }

    /// Space saves `every` apart instead of the default.
    #[cfg(test)]
    fn spaced(mut self, every: Duration) -> Self {
        self.every = every;
        *self.last_save.lock().unwrap() = Instant::now() - every;
        self
    }

    pub fn get(&self, token: &str) -> Option<Lease> {
        self.map.get(token).map(|l| l.clone())
    }

    pub fn by_tag(&self, tag: &str) -> Option<(String, Lease)> {
        self.map.iter().find(|e| e.value().tag == tag).map(|e| (e.key().clone(), e.value().clone()))
    }

    pub fn all(&self) -> Vec<Lease> {
        let mut v: Vec<Lease> = self.map.iter().map(|e| e.value().clone()).collect();
        v.sort_by(|a, b| a.tag.cmp(&b.tag));
        v
    }

    /// Switch the model a tagged lease runs from the next request on.
    pub fn set_model(&self, tag: &str, model: &str) -> bool {
        let Some(token) = self.by_tag(tag).map(|(t, _)| t) else { return false };
        if let Some(mut l) = self.map.get_mut(&token) {
            l.model = model.into();
        }
        self.save(true);
        true
    }

    pub fn set_cap(&self, tag: &str, cap_usd: Option<f64>) -> bool {
        let Some(token) = self.by_tag(tag).map(|(t, _)| t) else { return false };
        if let Some(mut l) = self.map.get_mut(&token) {
            l.cap_usd = cap_usd;
        }
        self.save(true);
        true
    }

    /// Follow a session that was renamed.
    pub fn rename_tag(&self, old: &str, new: &str) -> bool {
        let Some(token) = self.by_tag(old).map(|(t, _)| t) else { return false };
        if let Some(mut l) = self.map.get_mut(&token) {
            l.tag = new.into();
        }
        self.save(true);
        true
    }

    pub fn revoke_tag(&self, tag: &str) -> bool {
        let before = self.map.len();
        self.map.retain(|_, l| l.tag != tag);
        let gone = self.map.len() != before;
        if gone {
            self.save(true);
        }
        gone
    }

    /// Add one finished request to its lease.
    pub fn record(&self, token: &str, u: &Usage) {
        if let Some(mut l) = self.map.get_mut(token) {
            let s = &mut l.spent;
            s.requests += 1;
            s.input += u.input;
            s.output += u.output;
            s.cache_read += u.cache_read;
            s.cache_write += u.cache_write;
            match u.cost {
                Some(c) if c.is_finite() && c >= 0.0 => s.cost_usd += c,
                _ => s.unpriced += 1,
            }
        }
        self.save(false);
    }

    /// Write the table out. Spend changes often, so saves are spaced: one that comes too soon is
    /// held and written once the spacing is over, so the last change is never lost. `force` writes now.
    fn save(&self, force: bool) {
        let Some(path) = self.path.clone() else { return };
        let wait = {
            let mut last = self.last_save.lock().unwrap();
            let since = last.elapsed();
            if force || since >= self.every {
                *last = Instant::now();
                None
            } else {
                Some(self.every - since)
            }
        };
        match (wait, tokio::runtime::Handle::try_current()) {
            (None, Ok(h)) => {
                let map = self.map.clone();
                h.spawn_blocking(move || write_table(&map, &path));
            }
            (None, Err(_)) => write_table(&self.map, &path),
            (Some(wait), Ok(h)) => {
                if self.pending.swap(true, Ordering::SeqCst) {
                    return;
                }
                let (map, last, pending) = (self.map.clone(), self.last_save.clone(), self.pending.clone());
                h.spawn(async move {
                    tokio::time::sleep(wait).await;
                    pending.store(false, Ordering::SeqCst);
                    *last.lock().unwrap() = Instant::now();
                    let _ = tokio::task::spawn_blocking(move || write_table(&map, &path)).await;
                });
            }
            (Some(_), Err(_)) => write_table(&self.map, &path),
        }
    }

    /// Write out any spend the spacing held back. Call before ttm exits.
    pub fn flush(&self) {
        self.save(true);
    }
}

fn write_table(map: &DashMap<String, Lease>, path: &Path) {
    let snapshot: std::collections::BTreeMap<String, Lease> = map.iter().map(|e| (e.key().clone(), e.value().clone())).collect();
    match serde_json::to_vec_pretty(&snapshot) {
        Ok(bytes) => {
            if let Err(e) = secrets::write_private(path, &bytes) {
                tracing::warn!("saving OpenRouter leases: {e:#}");
            }
        }
        Err(e) => tracing::warn!("saving OpenRouter leases: {e:#}"),
    }
}

// ---- requests ------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Anthropic,
    Openai,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Send on to OpenRouter at this path under the account's base URL.
    Forward(Shape, &'static str),
    /// OpenRouter has no token counter for the Anthropic API, so answer with an estimate.
    CountTokens,
}

/// Which calls the proxy carries. `rest` is the path after `/or/`.
pub fn route(method: &Method, rest: &str) -> Option<Route> {
    let post = method == Method::POST;
    match (rest, post) {
        ("anthropic/v1/messages", true) => Some(Route::Forward(Shape::Anthropic, "/messages")),
        ("anthropic/v1/messages/count_tokens", true) => Some(Route::CountTokens),
        ("openai/v1/responses", true) => Some(Route::Forward(Shape::Openai, "/responses")),
        ("openai/v1/chat/completions", true) => Some(Route::Forward(Shape::Openai, "/chat/completions")),
        ("openai/v1/models", false) if method == Method::GET => Some(Route::Forward(Shape::Openai, "/models")),
        _ => None,
    }
}

/// Point a request at the lease's model and add the account's routing preferences. A `provider`
/// object the CLI already sent wins over the preferences.
pub fn transform(body: &mut Value, model: &str, prefs: &Prefs) {
    let Some(o) = body.as_object_mut() else { return };
    o.insert("model".into(), model.into());
    let mut p = o.get("provider").and_then(Value::as_object).cloned().unwrap_or_default();
    if let Some(s) = &prefs.sort {
        p.entry("sort").or_insert_with(|| s.clone().into());
    }
    if prefs.deny_data_collection {
        p.entry("data_collection").or_insert_with(|| "deny".into());
    }
    if prefs.zdr {
        p.entry("zdr").or_insert(true.into());
    }
    if !prefs.allow_fallbacks {
        p.entry("allow_fallbacks").or_insert(false.into());
    }
    if !p.is_empty() {
        o.insert("provider".into(), Value::Object(p));
    }
}

/// About four characters to a token, over every string in the request.
pub fn estimate_tokens(body: &Value) -> u64 {
    fn chars(v: &Value) -> usize {
        match v {
            Value::String(s) => s.chars().count(),
            Value::Array(a) => a.iter().map(chars).sum(),
            Value::Object(o) => o.values().map(chars).sum(),
            _ => 0,
        }
    }
    chars(body) as u64 / 4 + 1
}

/// The token from `Authorization: Bearer` (Claude Code with `ANTHROPIC_AUTH_TOKEN`, Codex) or `x-api-key`.
pub fn token_of(headers: &HeaderMap) -> Option<String> {
    let bearer = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    let key = headers.get("x-api-key").and_then(|v| v.to_str().ok());
    bearer.or(key).map(|t| t.trim().to_owned()).filter(|t| !t.is_empty())
}

/// A request that came through a tunnel or reverse proxy is not from an agent on this machine.
fn forwarded(headers: &HeaderMap) -> bool {
    ["x-forwarded-for", "forwarded", "cf-connecting-ip", "x-real-ip"].iter().any(|h| headers.contains_key(*h))
}

fn error(status: StatusCode, kind: &str, message: &str) -> Response {
    (status, axum::Json(json!({"type": "error", "error": {"type": kind, "message": message}}))).into_response()
}

/// What the CLI is told when the lease's dollar cap has been reached.
const CAP_MESSAGE: &str = "ttm: this session reached its dollar cap on OpenRouter";

/// Passes bytes on while reading their usage; whatever was seen is recorded when the reply ends,
/// or when the client goes away part-way through.
struct Tee {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    scan: Scan,
    done: Option<Box<dyn FnOnce(Usage) + Send>>,
}

impl Tee {
    fn finish(&mut self) {
        if let Some(done) = self.done.take() {
            if let Some(u) = self.scan.finish() {
                done(u);
            }
        }
    }
}

impl Stream for Tee {
    type Item = Result<Bytes, reqwest::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(b))) => {
                self.scan.push(&b);
                Poll::Ready(Some(Ok(b)))
            }
            Poll::Ready(None) => {
                self.finish();
                Poll::Ready(None)
            }
            other => other,
        }
    }
}

impl Drop for Tee {
    fn drop(&mut self) {
        self.finish();
    }
}

/// A reply's body as a stream of chunks that ends after the first error.
fn body_stream(res: reqwest::Response) -> impl Stream<Item = Result<Bytes, reqwest::Error>> + Send {
    futures_util::stream::unfold(Some(res), |state| async move {
        let mut res = state?;
        match res.chunk().await {
            Ok(Some(b)) => Some((Ok(b), Some(res))),
            Ok(None) => None,
            Err(e) => Some((Err(e), None)),
        }
    })
}

/// Carry one request for `token` to OpenRouter. The caller has checked who is asking.
pub async fn forward(leases: Arc<Leases>, account: &Account, token: &str, method: Method, rest: &str, headers: &HeaderMap, body: Bytes) -> Response {
    let Some(lease) = leases.get(token) else {
        return error(StatusCode::UNAUTHORIZED, "authentication_error", "unknown or revoked ttm lease");
    };
    let Some(route) = route(&method, rest) else {
        return error(StatusCode::NOT_FOUND, "not_found_error", "ttm's OpenRouter proxy does not carry that call");
    };
    if lease.over_cap() {
        return error(StatusCode::PAYMENT_REQUIRED, "billing_error", CAP_MESSAGE);
    }
    let parsed = if method == Method::POST {
        match serde_json::from_slice::<Value>(&body) {
            Ok(v) => Some(v),
            Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_request_error", "the request body is not JSON"),
        }
    } else {
        None
    };
    let upstream_path = match route {
        Route::CountTokens => {
            let n = parsed.as_ref().map(estimate_tokens).unwrap_or(1);
            return axum::Json(json!({"input_tokens": n})).into_response();
        }
        Route::Forward(_, path) => path,
    };
    if account.kind != Kind::Openrouter || account.key.is_empty() {
        return error(StatusCode::BAD_GATEWAY, "api_error", "the OpenRouter account has no key");
    }

    let mut req = HTTP.request(method, format!("{}{upstream_path}", account.base_url)).bearer_auth(&account.key).header("X-Title", "ttm");
    for name in ["content-type", "accept", "anthropic-version", "anthropic-beta", "openai-beta", "user-agent"] {
        if let Some(v) = headers.get(name) {
            req = req.header(name, v.as_bytes());
        }
    }
    if let Some(mut v) = parsed {
        transform(&mut v, &lease.model, &account.prefs);
        req = req.body(serde_json::to_vec(&v).unwrap_or_default());
    }
    let res = match req.send().await {
        Ok(r) => r,
        Err(e) => return error(StatusCode::BAD_GATEWAY, "api_error", &format!("could not reach OpenRouter: {e}")),
    };

    let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut out = Response::builder().status(status);
    for name in ["content-type", "cache-control", "x-request-id", "retry-after"] {
        if let Some(v) = res.headers().get(name) {
            out = out.header(name, v.as_bytes());
        }
    }
    let token = token.to_owned();
    let tee = Tee {
        inner: Box::pin(body_stream(res)),
        scan: Scan::default(),
        done: status.is_success().then(|| Box::new(move |u: Usage| leases.record(&token, &u)) as Box<dyn FnOnce(Usage) + Send>),
    };
    out.body(Body::from_stream(tee)).unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// `ANY /or/{*rest}`: authenticated by lease token, never by the page's login cookie.
pub async fn handle(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    method: Method,
    UrlPath(rest): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !peer.ip().is_loopback() || forwarded(&headers) {
        return error(StatusCode::FORBIDDEN, "permission_error", "the OpenRouter proxy only serves agents on this machine");
    }
    let Some(token) = token_of(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "authentication_error", "missing ttm lease token");
    };
    let Some(lease) = st.leases.get(&token) else {
        return error(StatusCode::UNAUTHORIZED, "authentication_error", "unknown or revoked ttm lease");
    };
    let accounts = secrets::load(&st.data_dir).await;
    let Some(account) = accounts.get(&lease.account) else {
        return error(StatusCode::BAD_GATEWAY, "api_error", "the lease's OpenRouter account was removed");
    };
    forward(st.leases.clone(), account, &token, method, &rest, &headers, body).await
}

// ---- what to give a CLI --------------------------------------------------------------------

/// Environment for Claude Code to talk to the proxy at `origin` (like `http://127.0.0.1:7681`).
pub fn claude_env(origin: &str, token: &str) -> Vec<(&'static str, String)> {
    vec![
        ("ANTHROPIC_BASE_URL", format!("{}/or/anthropic", origin.trim_end_matches('/'))),
        ("ANTHROPIC_AUTH_TOKEN", token.to_owned()),
        // An empty key stops Claude Code looking for one elsewhere and preferring it to the token.
        ("ANTHROPIC_API_KEY", String::new()),
    ]
}

/// `-c` overrides that make Codex use the proxy as its provider. It reads the lease token from
/// the `TTM_OR_TOKEN` environment variable, so the token never shows in the process list.
pub fn codex_args(origin: &str) -> Vec<String> {
    let base = format!("{}/or/openai/v1", origin.trim_end_matches('/'));
    vec![
        "-c".into(),
        format!("model_providers.ttm_or={{name=\"OpenRouter via ttm\",base_url=\"{base}\",wire_api=\"responses\",env_key=\"{TOKEN_ENV}\"}}"),
        "-c".into(),
        "model_provider=\"ttm_or\"".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::post};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn prefs() -> Prefs {
        Prefs::default()
    }

    #[test]
    fn routes_only_the_calls_agents_make() {
        use Shape::*;
        assert_eq!(route(&Method::POST, "anthropic/v1/messages"), Some(Route::Forward(Anthropic, "/messages")));
        assert_eq!(route(&Method::POST, "anthropic/v1/messages/count_tokens"), Some(Route::CountTokens));
        assert_eq!(route(&Method::POST, "openai/v1/responses"), Some(Route::Forward(Openai, "/responses")));
        assert_eq!(route(&Method::GET, "openai/v1/models"), Some(Route::Forward(Openai, "/models")));
        assert_eq!(route(&Method::GET, "anthropic/v1/messages"), None);
        assert_eq!(route(&Method::POST, "openai/v1/keys"), None, "no admin calls");
        assert_eq!(route(&Method::POST, "anthropic/../openai/v1/responses"), None);
    }

    #[test]
    fn transform_sets_the_model_and_merges_preferences() {
        let mut b = json!({"model": "claude-sonnet-5", "max_tokens": 5, "provider": {"order": ["anthropic"], "sort": "latency"}});
        let p = Prefs { sort: Some("price".into()), deny_data_collection: true, zdr: true, allow_fallbacks: false };
        transform(&mut b, "openai/gpt-6-luna", &p);
        assert_eq!(b["model"], "openai/gpt-6-luna");
        assert_eq!(b["provider"]["sort"], "latency", "the CLI's own choice wins");
        assert_eq!(b["provider"]["order"], json!(["anthropic"]));
        assert_eq!((&b["provider"]["data_collection"], &b["provider"]["zdr"], &b["provider"]["allow_fallbacks"]), (&json!("deny"), &json!(true), &json!(false)));
        assert_eq!(b["max_tokens"], 5);

        let mut plain = json!({"model": "x"});
        transform(&mut plain, "a/b", &prefs());
        assert!(plain.get("provider").is_none(), "no preferences, no provider object");
        let mut not_object = json!([1]);
        transform(&mut not_object, "a/b", &prefs());
        assert_eq!(not_object, json!([1]));
    }

    #[test]
    fn reads_usage_from_each_shape() {
        let anthropic = json!({"usage": {"input_tokens": 12, "output_tokens": 14, "cache_creation_input_tokens": 3, "cache_read_input_tokens": 4, "cost": 0.0000082}});
        assert_eq!(usage_of(&anthropic), Some(Usage { input: 12, output: 14, cache_read: 4, cache_write: 3, cost: Some(0.0000082) }));
        let start = json!({"type": "message_start", "message": {"usage": {"input_tokens": 9, "output_tokens": 1}}});
        assert_eq!(usage_of(&start).unwrap().input, 9);
        let responses = json!({"type": "response.completed", "response": {"usage": {"input_tokens": 8, "input_tokens_details": {"cached_tokens": 2}, "output_tokens": 6, "cost": 3.8e-06}}});
        assert_eq!(usage_of(&responses), Some(Usage { input: 8, output: 6, cache_read: 2, cache_write: 0, cost: Some(3.8e-06) }));
        let chat = json!({"usage": {"prompt_tokens": 5, "completion_tokens": 7, "prompt_tokens_details": {"cached_tokens": 1}}});
        assert_eq!(usage_of(&chat), Some(Usage { input: 5, output: 7, cache_read: 1, cache_write: 0, cost: None }));
        assert_eq!(usage_of(&json!({"type": "ping"})), None);
        assert_eq!(usage_of(&json!({"usage": null})), None);
    }

    #[test]
    fn scan_reads_a_stream_split_anywhere() {
        let sse = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12,\"output_tokens\":1}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"usage\"}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"input_tokens\":12,\"output_tokens\":14,\"cost\":0.5}}\n\nevent: data\ndata: [DONE]\n";
        for cut in [1, 7, 60, 200] {
            let mut s = Scan::default();
            for chunk in sse.as_bytes().chunks(cut) {
                s.push(chunk);
            }
            let u = s.finish().unwrap_or_else(|| panic!("no usage with chunks of {cut}"));
            assert_eq!((u.input, u.output, u.cost), (12, 14, Some(0.5)), "chunks of {cut}");
        }
    }

    #[test]
    fn scan_reads_a_single_json_reply_and_ignores_the_rest() {
        let mut s = Scan::default();
        s.push(b"{\"id\":\"m\",\n \"usage\": {\"input_tokens\": 3,\n \"output_tokens\": 4, \"cost\": 0.01}}");
        let u = s.finish().unwrap();
        assert_eq!((u.input, u.output, u.cost), (3, 4, Some(0.01)));
        let mut none = Scan::default();
        none.push(b"event: ping\ndata: {\"type\":\"ping\"}\n\n");
        assert_eq!(none.finish(), None);
        let mut half = Scan::default();
        half.push(b"data: {\"usage\":{\"input_tokens\":2}}"); // no trailing newline at the end of the stream
        assert_eq!(half.finish().unwrap().input, 2);
    }

    #[test]
    fn leases_track_model_spend_and_cap() {
        let l = Leases::new();
        let t = l.mint("acct", "a/one", "sess", Some(0.01));
        assert!(t.starts_with("ttm-or-") && t.len() > 40);
        assert_eq!(l.get(&t).unwrap().model, "a/one");
        assert!(l.set_model("sess", "b/two"));
        assert_eq!(l.get(&t).unwrap().model, "b/two");
        assert!(!l.set_model("other", "x"));
        l.record(&t, &Usage { input: 10, output: 5, cache_read: 2, cache_write: 1, cost: Some(0.004) });
        assert!(!l.get(&t).unwrap().over_cap());
        l.record(&t, &Usage { cost: None, ..Default::default() });
        l.record(&t, &Usage { input: 1, cost: Some(0.007), ..Default::default() });
        let s = l.get(&t).unwrap().spent;
        assert_eq!((s.requests, s.input, s.output, s.unpriced), (3, 11, 5, 1));
        assert!((s.cost_usd - 0.011).abs() < 1e-12);
        assert!(l.get(&t).unwrap().over_cap());
        assert!(l.set_cap("sess", None));
        assert!(!l.get(&t).unwrap().over_cap());
        let t2 = l.mint("acct", "a/one", "sess", None);
        assert!(l.get(&t).is_none(), "a new lease for a tag replaces the old one");
        assert_eq!(l.get(&t2).unwrap().spent, Spend::default());
        assert!(l.revoke_tag("sess") && !l.revoke_tag("sess"));
    }

    #[test]
    fn leases_survive_a_restart_and_the_file_is_private() {
        let dir = std::env::temp_dir().join(format!("ttm-leases-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let l = Leases::load(&dir);
        let t = l.mint("acct", "a/one", "sess", Some(2.0));
        l.record(&t, &Usage { cost: Some(0.5), ..Default::default() });
        l.flush();
        let back = Leases::load(&dir).get(&t).expect("lease reloaded");
        assert_eq!((back.model.as_str(), back.cap_usd, back.spent.cost_usd), ("a/one", Some(2.0), 0.5));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.join(FILE)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_save_held_back_by_the_spacing_is_still_written() {
        let dir = std::env::temp_dir().join(format!("ttm-leases-late-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let l = Leases::load(&dir).spaced(Duration::from_millis(150));
        let t = l.mint("acct", "a/one", "sess", None);
        l.record(&t, &Usage { cost: Some(0.1), ..Default::default() });
        l.record(&t, &Usage { cost: Some(0.2), ..Default::default() }); // too soon after the mint's save
        tokio::time::sleep(Duration::from_millis(400)).await;
        let mut spent = 0.0;
        for _ in 0..100 {
            spent = Leases::load(&dir).get(&t).map(|l| l.spent.cost_usd).unwrap_or(0.0);
            if spent > 0.29 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!((spent - 0.3).abs() < 1e-9, "both requests were saved, got {spent}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tokens_come_from_either_header_and_tunnels_are_spotted() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer ttm-or-abc".parse().unwrap());
        assert_eq!(token_of(&h).as_deref(), Some("ttm-or-abc"));
        let mut h = HeaderMap::new();
        h.insert("x-api-key", " ttm-or-xyz ".parse().unwrap());
        assert_eq!(token_of(&h).as_deref(), Some("ttm-or-xyz"));
        assert_eq!(token_of(&HeaderMap::new()), None);
        assert!(!forwarded(&h));
        h.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
        assert!(forwarded(&h));
    }

    #[test]
    fn what_a_cli_is_given() {
        let env = claude_env("http://127.0.0.1:7681/", "ttm-or-t");
        assert!(env.contains(&("ANTHROPIC_BASE_URL", "http://127.0.0.1:7681/or/anthropic".to_string())));
        assert!(env.contains(&("ANTHROPIC_AUTH_TOKEN", "ttm-or-t".to_string())));
        assert!(env.contains(&("ANTHROPIC_API_KEY", String::new())));
        let args = codex_args("http://127.0.0.1:7681");
        assert_eq!(args[0], "-c");
        assert!(args[1].contains("base_url=\"http://127.0.0.1:7681/or/openai/v1\"") && args[1].contains("wire_api=\"responses\"") && args[1].contains("env_key=\"TTM_OR_TOKEN\""));
        assert!(!args.join(" ").contains("ttm-or-"), "the token is never on the command line");
    }

    #[test]
    fn token_estimate_counts_every_string() {
        assert_eq!(estimate_tokens(&json!({"messages": [{"role": "user", "content": "x".repeat(400)}]})), 101 + 1);
    }

    // ---- against a stand-in for OpenRouter ----

    /// What the stand-in saw.
    #[derive(Default)]
    struct Seen {
        hits: AtomicUsize,
        last: Mutex<Option<(String, Value)>>,
    }

    async fn upstream(seen: Arc<Seen>, sse: bool) -> String {
        let app = Router::new().route(
            "/messages",
            post(move |headers: HeaderMap, body: Bytes| {
                let seen = seen.clone();
                async move {
                    seen.hits.fetch_add(1, Ordering::SeqCst);
                    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
                    *seen.last.lock().unwrap() = Some((auth, serde_json::from_slice(&body).unwrap()));
                    if sse {
                        let parts: Vec<Result<Bytes, std::io::Error>> = vec![
                            Ok(Bytes::from("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12}}}\n\n")),
                            Ok(Bytes::from("event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"input_tokens\":12,\"output_tokens\":14,\"cost\":0.25}}\n\n")),
                        ];
                        ([(header::CONTENT_TYPE, "text/event-stream")], Body::from_stream(futures_util::stream::iter(parts))).into_response()
                    } else {
                        axum::Json(json!({"content": [], "usage": {"input_tokens": 3, "output_tokens": 4, "cost": 0.5}})).into_response()
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    fn account(base: &str, prefs: Prefs) -> Account {
        Account { id: "acct".into(), name: "OR".into(), kind: Kind::Openrouter, base_url: base.into(), key: "sk-real-key".into(), use_in_orchestrator: true, models: vec![], prefs }
    }

    async fn body_text(r: Response) -> String {
        String::from_utf8(axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap()
    }

    #[tokio::test]
    async fn forwards_with_the_real_key_the_lease_model_and_preferences() {
        let seen = Arc::new(Seen::default());
        let base = upstream(seen.clone(), false).await;
        let leases = Arc::new(Leases::new());
        let token = leases.mint("acct", "anthropic/claude-haiku-5.5", "sess", None);
        let acct = account(&base, Prefs { sort: Some("price".into()), ..Prefs::default() });
        let body = Bytes::from(json!({"model": "claude-sonnet-5", "max_tokens": 8, "messages": []}).to_string());

        let res = forward(leases.clone(), &acct, &token, Method::POST, "anthropic/v1/messages", &HeaderMap::new(), body).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(body_text(res).await.contains("\"cost\""));

        let (auth, got) = seen.last.lock().unwrap().clone().unwrap();
        assert_eq!(auth, "Bearer sk-real-key", "the real key goes upstream, never the lease token");
        assert_eq!(got["model"], "anthropic/claude-haiku-5.5");
        assert_eq!(got["provider"]["sort"], "price");
        let s = leases.get(&token).unwrap().spent;
        assert_eq!((s.requests, s.input, s.output), (1, 3, 4));
        assert!((s.cost_usd - 0.5).abs() < 1e-12);
    }

    #[tokio::test]
    async fn streams_the_reply_unchanged_and_records_its_cost() {
        let seen = Arc::new(Seen::default());
        let base = upstream(seen.clone(), true).await;
        let leases = Arc::new(Leases::new());
        let token = leases.mint("acct", "a/b", "sess", None);
        let res = forward(leases.clone(), &account(&base, Prefs::default()), &token, Method::POST, "anthropic/v1/messages", &HeaderMap::new(), Bytes::from("{\"model\":\"x\",\"stream\":true}")).await;
        assert_eq!(res.headers()[header::CONTENT_TYPE], "text/event-stream");
        let text = body_text(res).await;
        assert!(text.starts_with("event: message_start") && text.contains("\"cost\":0.25"), "bytes pass through as they came");
        let s = leases.get(&token).unwrap().spent;
        assert_eq!((s.input, s.output), (12, 14), "the stream's last report replaces its first");
        assert!((s.cost_usd - 0.25).abs() < 1e-12);
    }

    #[tokio::test]
    async fn refuses_without_calling_upstream_when_the_lease_is_unknown_or_capped() {
        let seen = Arc::new(Seen::default());
        let base = upstream(seen.clone(), false).await;
        let leases = Arc::new(Leases::new());
        let acct = account(&base, Prefs::default());
        let call = |l: Arc<Leases>, tok: String, rest: &'static str, body: &'static str| {
            let acct = acct.clone();
            async move { forward(l, &acct, &tok, Method::POST, rest, &HeaderMap::new(), Bytes::from(body)).await }
        };

        assert_eq!(call(leases.clone(), "ttm-or-nope".into(), "anthropic/v1/messages", "{}").await.status(), StatusCode::UNAUTHORIZED);

        let capped = leases.mint("acct", "a/b", "capped", Some(0.1));
        leases.record(&capped, &Usage { cost: Some(0.1), ..Default::default() });
        let r = call(leases.clone(), capped.clone(), "anthropic/v1/messages", "{}").await;
        assert_eq!(r.status(), StatusCode::PAYMENT_REQUIRED);
        assert!(body_text(r).await.contains("dollar cap"));

        let ok = leases.mint("acct", "a/b", "ok", None);
        assert_eq!(call(leases.clone(), ok.clone(), "anthropic/v1/keys", "{}").await.status(), StatusCode::NOT_FOUND);
        assert_eq!(call(leases.clone(), ok.clone(), "anthropic/v1/messages", "not json").await.status(), StatusCode::BAD_REQUEST);
        assert_eq!(seen.hits.load(Ordering::SeqCst), 0, "none of these reached OpenRouter");

        let counted = call(leases.clone(), ok, "anthropic/v1/messages/count_tokens", "{\"messages\":[{\"content\":\"abcdefgh\"}]}").await;
        assert_eq!(counted.status(), StatusCode::OK);
        assert_eq!(serde_json::from_str::<Value>(&body_text(counted).await).unwrap()["input_tokens"], 3);
        assert_eq!(seen.hits.load(Ordering::SeqCst), 0, "token counts are answered here");
    }
}
