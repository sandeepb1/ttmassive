//! Plan-limit usage for Claude Code and Codex: how much of the 5-hour and weekly windows is
//! spent, read from each tool's own login, plus token totals per session.
//!
//! ttm only ever *reads* the tokens the CLIs already hold. It never refreshes them: a refresh
//! rotates the refresh token and can sign the real CLI out. An expired token is reported as an
//! error and fixed by using the CLI once.
//!
//! Sources, freshest first:
//! - Claude Code's `statusLine` hook (`ttm statusline`) pushes `rate_limits` while claude runs.
//! - Claude: `GET api.anthropic.com/api/oauth/usage`. Codex: `GET chatgpt.com/backend-api/wham/usage`.
//! - Codex only, as a fallback: the newest `rate_limits` in its session logs.

use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;

use crate::{
    AppState,
    agentlog::{self, claude_dir, codex_dir, read_tail, rollouts_newest_first},
    config::Agent,
    notify, sessions,
};

/// Serve cached numbers this long before asking the provider again.
const MAX_AGE_SECS: i64 = 300;
/// Opening the panel asks for fresher numbers, but never more than once a minute.
const REFRESH_AGE_SECS: i64 = 60;
/// Push once per window when 5h or weekly usage crosses this.
const ALERT_PCT: f64 = 90.0;
/// Reset times differ by a second or so between sources; a reset this far later is a new window.
const NEW_WINDOW_SLACK_SECS: i64 = 1800;
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// How much of a log's end to read when looking for its latest entry.
const TAIL_BYTES: u64 = 512 * 1024;
/// Newest Codex logs to look through for rate limits before giving up.
const MAX_LOGS_SCANNED: usize = 30;

const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Claude,
    Codex,
}

impl Tool {
    fn label(self) -> &'static str {
        match self {
            Tool::Claude => "Claude Code",
            Tool::Codex => "Codex",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Tool::Claude => "claude",
            Tool::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Span {
    FiveHour,
    SevenDay,
}

impl Span {
    fn label(self) -> &'static str {
        match self {
            Span::FiveHour => "5h",
            Span::SevenDay => "weekly",
        }
    }
}

/// One rate-limit window. `used_pct` is 0-100; `resets_at` is Unix seconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Window {
    pub used_pct: f64,
    pub resets_at: Option<i64>,
}

/// A per-model weekly window, such as Claude's Opus or Sonnet cap.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelWindow {
    pub name: String,
    #[serde(flatten)]
    pub window: Window,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The provider's usage API.
    #[default]
    Api,
    /// Claude Code's statusLine hook.
    Hook,
    /// Codex's own session log.
    SessionLog,
    /// The pooled limits of every account behind Team Claude.
    TeamClaude,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct ToolUsage {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
    pub per_model: Vec<ModelWindow>,
    pub plan: Option<String>,
    /// When these numbers were read (Unix seconds); 0 if never.
    pub fetched_at: i64,
    pub source: Source,
    /// Why the latest refresh failed. Older numbers may still be shown next to it.
    pub error: Option<String>,
}

impl ToolUsage {
    fn new(now: i64, source: Source) -> Self {
        Self { fetched_at: now, source, ..Self::default() }
    }

    fn has_windows(&self) -> bool {
        self.five_hour.is_some() || self.seven_day.is_some()
    }

    fn span(&self, span: Span) -> Option<Window> {
        match span {
            Span::FiveHour => self.five_hour,
            Span::SevenDay => self.seven_day,
        }
    }

    /// Put `w` in the right slot: by its length when known, else by `fallback`.
    fn place(&mut self, w: Window, window_secs: Option<i64>, fallback: Span) {
        let span = match window_secs {
            Some(s) if s <= 6 * 3600 => Span::FiveHour,
            Some(_) => Span::SevenDay,
            None => fallback,
        };
        match span {
            Span::FiveHour => self.five_hour = Some(w),
            Span::SevenDay => self.seven_day = Some(w),
        }
    }
}

// ------------------------------------------------------------------ parsing

fn epoch_secs(n: f64) -> i64 {
    // Millisecond timestamps are ~1e12; second ones ~1e9.
    if n > 1e11 { (n / 1000.0) as i64 } else { n as i64 }
}

/// A time as Unix seconds from a number (seconds or milliseconds) or an RFC 3339 string.
pub(crate) fn parse_time(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_f64().map(epoch_secs),
        Value::String(s) => s.parse::<f64>().ok().map(epoch_secs).or_else(|| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
                .ok()
                .map(|t| t.unix_timestamp())
        }),
        _ => None,
    }
}

/// A window object in any of the shapes the sources use: `utilization` (Claude API),
/// `used_percentage` (Claude statusLine) or `used_percent` (Codex), with `resets_at` / `reset_at`.
fn window(v: &Value) -> Option<Window> {
    let pct = ["utilization", "used_percentage", "used_percent"]
        .iter()
        .find_map(|k| v.get(*k))?
        .as_f64()?;
    let resets_at = ["resets_at", "reset_at"].iter().find_map(|k| v.get(*k)).and_then(parse_time);
    Some(Window { used_pct: pct, resets_at })
}

fn parse_claude_api(body: &Value, plan: Option<String>, now: i64) -> ToolUsage {
    let mut out = ToolUsage::new(now, Source::Api);
    out.five_hour = body.get("five_hour").and_then(window);
    out.seven_day = body.get("seven_day").and_then(window);
    out.plan = plan;
    // `seven_day_opus`, `seven_day_sonnet`, ...: null when the plan has no such cap.
    if let Some(obj) = body.as_object() {
        for (key, v) in obj {
            if let Some(name) = key.strip_prefix("seven_day_")
                && let Some(w) = window(v)
            {
                out.per_model.push(ModelWindow { name: name.to_owned(), window: w });
            }
        }
    }
    out
}

fn parse_codex_api(body: &Value, now: i64) -> ToolUsage {
    let mut out = ToolUsage::new(now, Source::Api);
    out.plan = body.get("plan_type").and_then(Value::as_str).map(str::to_owned);
    let limits = body.get("rate_limit");
    for (key, fallback) in [("primary_window", Span::FiveHour), ("secondary_window", Span::SevenDay)] {
        if let Some(w) = limits.and_then(|l| l.get(key))
            && let Some(parsed) = window(w)
        {
            out.place(parsed, w.get("limit_window_seconds").and_then(Value::as_i64), fallback);
        }
    }
    out
}

/// The `rate_limits` object Codex writes into its session logs.
fn parse_codex_log(rate_limits: &Value, now: i64) -> ToolUsage {
    let mut out = ToolUsage::new(now, Source::SessionLog);
    out.plan = rate_limits.get("plan_type").and_then(Value::as_str).map(str::to_owned);
    for (key, fallback) in [("primary", Span::FiveHour), ("secondary", Span::SevenDay)] {
        if let Some(w) = rate_limits.get(key)
            && let Some(parsed) = window(w)
        {
            out.place(parsed, w.get("window_minutes").and_then(Value::as_i64).map(|m| m * 60), fallback);
        }
    }
    out
}

/// Claude Code's statusLine input. `None` when it carries no limits (API-key logins).
pub fn from_statusline(v: &Value, now: i64) -> Option<ToolUsage> {
    let limits = v.get("rate_limits").filter(|r| r.is_object())?;
    let mut out = ToolUsage::new(now, Source::Hook);
    out.five_hour = limits.get("five_hour").and_then(window);
    out.seven_day = limits.get("seven_day").and_then(window);
    out.plan = v.get("subscription_type").and_then(Value::as_str).map(str::to_owned);
    out.has_windows().then_some(out)
}

fn fmt_remaining(secs: i64) -> String {
    match secs {
        s if s <= 0 => "now".into(),
        s if s < 3600 => format!("{}m", (s + 59) / 60),
        s if s < 86_400 => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
        s => format!("{}d{}h", s / 86_400, (s % 86_400) / 3600),
    }
}

/// The text `ttm statusline` prints in Claude's footer: `5h 42% (2h10m) · 7d 18%`.
pub fn status_line(v: &Value, now: i64) -> String {
    let Some(u) = from_statusline(v, now) else {
        return v["model"]["display_name"].as_str().unwrap_or_default().to_owned();
    };
    let mut parts = Vec::new();
    if let Some(w) = u.five_hour {
        let left = w.resets_at.map(|r| format!(" ({})", fmt_remaining(r - now))).unwrap_or_default();
        parts.push(format!("5h {:.0}%{left}", w.used_pct));
    }
    if let Some(w) = u.seven_day {
        parts.push(format!("7d {:.0}%", w.used_pct));
    }
    parts.join(" · ")
}

// -------------------------------------------------------------- credentials

/// No `Debug`: this holds a bearer token.
struct ClaudeCreds {
    token: String,
    /// Unix seconds.
    expires_at: Option<i64>,
    plan: Option<String>,
}

fn parse_claude_creds(raw: &str) -> Result<ClaudeCreds> {
    let v: Value = serde_json::from_str(raw).context("Claude credentials are not JSON")?;
    let oauth = v
        .get("claudeAiOauth")
        .context("no subscription login (claude is using an API key, which has no plan limits)")?;
    let token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .context("the Claude login has no access token")?
        .to_owned();
    Ok(ClaudeCreds {
        token,
        expires_at: oauth.get("expiresAt").and_then(parse_time),
        plan: oauth.get("subscriptionType").and_then(Value::as_str).map(str::to_owned),
    })
}

#[cfg(target_os = "macos")]
async fn keychain_claude_creds() -> Result<String> {
    let run = tokio::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(HTTP_TIMEOUT, run)
        .await
        .context("reading the keychain timed out (is a permission prompt waiting?)")?
        .context("running `security`")?;
    if !out.status.success() {
        bail!("no Claude Code login in the keychain (or access was denied)");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[cfg(not(target_os = "macos"))]
async fn keychain_claude_creds() -> Result<String> {
    bail!("no Claude Code login found")
}

async fn load_claude_creds() -> Result<ClaudeCreds> {
    if let Some(dir) = claude_dir()
        && let Ok(raw) = tokio::fs::read_to_string(dir.join(".credentials.json")).await
    {
        return parse_claude_creds(&raw);
    }
    parse_claude_creds(&keychain_claude_creds().await?)
}

// ------------------------------------------------------------------ fetching

async fn fetch_claude(http: &reqwest::Client, now: i64) -> Result<ToolUsage> {
    let creds = load_claude_creds().await?;
    if creds.expires_at.is_some_and(|e| e <= now) {
        bail!("the Claude login has expired: use claude once so it refreshes");
    }
    let resp = http
        .get(CLAUDE_USAGE_URL)
        .bearer_auth(&creds.token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await
        .context("calling the Claude usage API")?;
    let status = resp.status();
    if matches!(status.as_u16(), 401 | 403) {
        bail!("Claude rejected the login (HTTP {status}): use claude once to sign in again");
    }
    if !status.is_success() {
        bail!("the Claude usage API answered HTTP {status}");
    }
    let body: Value = resp.json().await.context("reading the Claude usage response")?;
    let out = parse_claude_api(&body, creds.plan, now);
    if !out.has_windows() {
        bail!("the Claude usage response had no 5h or weekly window");
    }
    Ok(out)
}

async fn fetch_codex(http: &reqwest::Client, now: i64) -> Result<ToolUsage> {
    let path = codex_dir().context("HOME is not set")?.join("auth.json");
    let raw = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("reading {} (run `codex login`)", path.display()))?;
    let auth: Value = serde_json::from_str(&raw).context("Codex auth.json is not JSON")?;
    let tokens = auth
        .get("tokens")
        .context("Codex is using an API key, which has no plan limits")?;
    let token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .context("the Codex login has no access token")?;
    let mut req = http.get(CODEX_USAGE_URL).bearer_auth(token);
    if let Some(account) = tokens.get("account_id").and_then(Value::as_str) {
        req = req.header("ChatGPT-Account-Id", account);
    }
    let resp = req.send().await.context("calling the Codex usage API")?;
    let status = resp.status();
    if matches!(status.as_u16(), 401 | 403) {
        bail!("Codex rejected the login (HTTP {status}): use codex once to sign in again");
    }
    if !status.is_success() {
        bail!("the Codex usage API answered HTTP {status}");
    }
    let body: Value = resp.json().await.context("reading the Codex usage response")?;
    let out = parse_codex_api(&body, now);
    if !out.has_windows() {
        bail!("the Codex usage response had no 5h or weekly window");
    }
    Ok(out)
}

/// The live API, then Codex's own log of its last turn if the API is unreachable.
async fn fetch_codex_or_log(http: &reqwest::Client, now: i64) -> Result<ToolUsage> {
    let err = match fetch_codex(http, now).await {
        Ok(u) => return Ok(u),
        Err(e) => e,
    };
    tracing::warn!("codex usage API: {err:#}");
    let Some(root) = codex_dir().map(|d| d.join("sessions")) else { return Err(err) };
    let found = tokio::task::spawn_blocking(move || latest_rate_limits(&root)).await.ok().flatten();
    match found {
        Some((limits, read_at)) => {
            let out = parse_codex_log(&limits, read_at);
            if out.has_windows() { Ok(out) } else { Err(err) }
        }
        None => Err(err),
    }
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

// ---------------------------------------------------------------- log files

/// The newest `rate_limits` object in Codex's logs and the time that log was last written.
fn latest_rate_limits(root: &Path) -> Option<(Value, i64)> {
    for path in rollouts_newest_first(root).take(MAX_LOGS_SCANNED) {
        let Ok(tail) = read_tail(&path, TAIL_BYTES) else { continue };
        let text = String::from_utf8_lossy(&tail);
        for line in text.lines().rev().filter(|l| l.contains("\"rate_limits\"")) {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let limits = &v["payload"]["rate_limits"];
            if limits.is_object() {
                let written = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs() as i64);
                return Some((limits.clone(), written));
            }
        }
    }
    None
}

// ------------------------------------------------------------ session tokens

#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SessionTokens {
    pub tool: &'static str,
    #[serde(flatten)]
    pub tokens: Tokens,
}

fn count(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Claude transcripts repeat a message once per streamed chunk, each with the running usage,
/// so the last entry for each message id is the one that counts.
#[derive(Default)]
struct ClaudeScan {
    offset: u64,
    by_message: HashMap<String, Tokens>,
}

impl ClaudeScan {
    fn ingest(&mut self, line: &[u8]) {
        if !line.windows(7).any(|w| w == b"\"usage\"") {
            return;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else { return };
        if v["type"].as_str() != Some("assistant") {
            return;
        }
        let msg = &v["message"];
        let (Some(id), usage) = (msg["id"].as_str(), &msg["usage"]) else { return };
        if !usage.is_object() {
            return;
        }
        self.by_message.insert(
            id.to_owned(),
            Tokens {
                input: count(usage, "input_tokens"),
                output: count(usage, "output_tokens"),
                cache_read: count(usage, "cache_read_input_tokens"),
                cache_write: count(usage, "cache_creation_input_tokens"),
            },
        );
    }

    /// Read what was appended since last time. A half-written last line waits for the next call.
    fn update(&mut self, path: &Path) -> std::io::Result<()> {
        let mut f = File::open(path)?;
        let len = f.metadata()?.len();
        if len < self.offset {
            *self = Self::default();
        }
        if len == self.offset {
            return Ok(());
        }
        f.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::new();
        f.take(len - self.offset).read_to_end(&mut buf)?;
        let Some(end) = buf.iter().rposition(|&b| b == b'\n') else { return Ok(()) };
        for line in buf[..end].split(|&b| b == b'\n') {
            self.ingest(line);
        }
        self.offset += end as u64 + 1;
        Ok(())
    }

    fn total(&self) -> Tokens {
        self.by_message.values().fold(Tokens::default(), |a, t| Tokens {
            input: a.input + t.input,
            output: a.output + t.output,
            cache_read: a.cache_read + t.cache_read,
            cache_write: a.cache_write + t.cache_write,
        })
    }
}

/// The running total Codex keeps in its log: the last `total_token_usage` near the end.
/// `input_tokens` there includes the cached part, so it is split out.
fn codex_totals(path: &Path) -> Option<Tokens> {
    let tail = read_tail(path, TAIL_BYTES).ok()?;
    let text = String::from_utf8_lossy(&tail);
    for line in text.lines().rev().filter(|l| l.contains("total_token_usage")) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let u = &v["payload"]["info"]["total_token_usage"];
        if !u.is_object() {
            continue;
        }
        let cached = count(u, "cached_input_tokens");
        return Some(Tokens {
            input: count(u, "input_tokens").saturating_sub(cached),
            output: count(u, "output_tokens"),
            cache_read: cached,
            cache_write: 0,
        });
    }
    None
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A poisoned lock only means another thread panicked mid-update; the numbers are still usable.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Default)]
pub struct TokenCache {
    paths: Mutex<HashMap<(Tool, String), PathBuf>>,
    claude: Mutex<HashMap<PathBuf, ClaudeScan>>,
}

impl TokenCache {
    fn locate(&self, tool: Tool, id: &str) -> Option<PathBuf> {
        let key = (tool, id.to_owned());
        if let Some(p) = lock(&self.paths).get(&key).filter(|p| p.is_file()) {
            return Some(p.clone());
        }
        let found = agentlog::find_log(
            match tool {
                Tool::Claude => Agent::Claude,
                Tool::Codex => Agent::Codex,
            },
            id,
        );
        if let Some(p) = &found {
            lock(&self.paths).insert(key, p.clone());
        }
        found
    }

    /// Blocking: reads files. Call from `spawn_blocking`.
    fn totals(&self, tool: Tool, id: &str) -> Option<Tokens> {
        let path = self.locate(tool, id)?;
        match tool {
            Tool::Claude => {
                let mut scans = lock(&self.claude);
                let scan = scans.entry(path.clone()).or_default();
                scan.update(&path).ok()?;
                Some(scan.total())
            }
            Tool::Codex => codex_totals(&path),
        }
    }
}

async fn session_tokens(st: &AppState) -> BTreeMap<String, SessionTokens> {
    let targets: Vec<(String, Tool, String)> = st
        .sessions
        .iter()
        .filter_map(|e| {
            let m = e.value();
            let id = m.agent_session_id.clone().filter(|id| sessions::valid_agent_id(id))?;
            let tool = match m.agent()? {
                Agent::Claude => Tool::Claude,
                Agent::Codex => Tool::Codex,
                Agent::Other => return None,
            };
            Some((e.key().clone(), tool, id))
        })
        .collect();
    if targets.is_empty() {
        return BTreeMap::new();
    }
    let cache = st.usage.tokens.clone();
    tokio::task::spawn_blocking(move || {
        targets
            .into_iter()
            .filter_map(|(name, tool, id)| {
                let tokens = cache.totals(tool, &id)?;
                Some((name, SessionTokens { tool: tool.key(), tokens }))
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

// -------------------------------------------------------------------- cache

#[derive(Default)]
struct SlotState {
    data: Option<ToolUsage>,
    /// When we last tried (Unix seconds), whether or not it worked.
    checked_at: i64,
}

#[derive(Default)]
struct Slot {
    state: Mutex<SlotState>,
    /// Held while fetching so concurrent callers share one request.
    gate: tokio::sync::Mutex<()>,
}

impl Slot {
    fn fresh(&self, max_age: i64, now: i64) -> Option<ToolUsage> {
        let s = lock(&self.state);
        if now - s.checked_at < max_age { s.data.clone() } else { None }
    }

    fn data(&self) -> Option<ToolUsage> {
        lock(&self.state).data.clone()
    }

    fn store(&self, data: ToolUsage, now: i64) {
        *lock(&self.state) = SlotState { data: Some(data), checked_at: now };
    }
}

pub struct Usage {
    http: reqwest::Client,
    claude: Slot,
    codex: Slot,
    /// The reset time of the window we last warned about, per tool and window.
    alerted: Mutex<HashMap<(Tool, Span), i64>>,
    pub tokens: Arc<TokenCache>,
}

impl Usage {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .user_agent(concat!("ttm/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            claude: Slot::default(),
            codex: Slot::default(),
            alerted: Mutex::default(),
            tokens: Arc::default(),
        }
    }

    fn slot(&self, tool: Tool) -> &Slot {
        match tool {
            Tool::Claude => &self.claude,
            Tool::Codex => &self.codex,
        }
    }

    /// Windows at or over the alert level that we have not warned about yet, as
    /// `5h 92% (resets in 1h20m)`. A reset more than `NEW_WINDOW_SLACK_SECS` after the one we
    /// warned about means a new window, which can warn again.
    fn alerts_due(&self, tool: Tool, u: &ToolUsage, now: i64) -> Vec<String> {
        let mut alerted = lock(&self.alerted);
        let mut due = Vec::new();
        for span in [Span::FiveHour, Span::SevenDay] {
            let Some(w) = u.span(span).filter(|w| w.used_pct >= ALERT_PCT) else { continue };
            let reset = w.resets_at.unwrap_or(0);
            if alerted.get(&(tool, span)).is_some_and(|&prev| reset <= prev + NEW_WINDOW_SLACK_SECS) {
                continue;
            }
            alerted.insert((tool, span), reset);
            let when = w.resets_at.map(|r| format!(" (resets in {})", fmt_remaining(r - now))).unwrap_or_default();
            due.push(format!("{} {:.0}%{when}", span.label(), w.used_pct));
        }
        due
    }
}

/// One push per tool per update, listing the windows that just crossed the line.
fn warn_if_needed(st: &Arc<AppState>, tool: Tool, u: &ToolUsage, now: i64) {
    let due = st.usage.alerts_due(tool, u, now);
    if due.is_empty() {
        return;
    }
    let message = format!("{} is nearly out: {}", tool.label(), due.join(", "));
    let alert = notify::Alert::new(&format!("usage-{}", tool.key()), notify::Kind::Limit, Some(message));
    tokio::spawn(notify::send_arc(st.clone(), alert));
}

async fn current(st: &Arc<AppState>, tool: Tool, max_age: i64) -> ToolUsage {
    let slot = st.usage.slot(tool);
    if let Some(u) = slot.fresh(max_age, now_secs()) {
        return u;
    }
    let _gate = slot.gate.lock().await;
    // Someone else may have refreshed while we waited for the gate.
    let now = now_secs();
    if let Some(u) = slot.fresh(max_age, now) {
        return u;
    }
    let fetched = match tool {
        Tool::Claude => fetch_claude(&st.usage.http, now).await,
        Tool::Codex => fetch_codex_or_log(&st.usage.http, now).await,
    };
    let usage = match fetched {
        Ok(u) => {
            warn_if_needed(st, tool, &u, now);
            u
        }
        Err(e) => {
            // Keep showing the last good numbers, with the reason they are not newer.
            let mut stale = slot.data().unwrap_or_else(|| ToolUsage::new(0, Source::Api));
            stale.error = Some(format!("{e:#}"));
            stale
        }
    };
    slot.store(usage.clone(), now);
    usage
}

/// What a Claude session's status line says about the session itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusLive {
    /// The reasoning level, e.g. `medium`.
    pub effort: Option<String>,
}

/// The session-specific parts of Claude's status JSON.
pub fn status_live(payload: &Value) -> StatusLive {
    StatusLive { effort: payload["effort"]["level"].as_str().filter(|e| !e.is_empty()).map(str::to_owned) }
}

/// A statusLine report from a Claude Code session inside ttm. Fresher than the API, so it
/// replaces what we have, keeping the parts it does not carry (plan, per-model caps).
pub fn ingest_statusline(st: &Arc<AppState>, payload: &Value) {
    // What this session says about itself, whether or not it carries plan limits.
    if let Some(uid) = payload["ttm_uid"].as_str().filter(|u| !u.is_empty()) {
        st.statusline.insert(uid.to_owned(), status_live(payload));
    }
    let now = now_secs();
    let Some(mut u) = from_statusline(payload, now) else { return };
    let slot = st.usage.slot(Tool::Claude);
    if let Some(prev) = slot.data() {
        u.plan = u.plan.or(prev.plan);
        u.per_model = prev.per_model;
    }
    warn_if_needed(st, Tool::Claude, &u, now);
    // The hook cannot say which plan this is. Until the API has, count the numbers as not yet
    // checked so the next request makes one API call and fills the plan in.
    let checked_at = if u.plan.is_some() { now } else { 0 };
    slot.store(u, checked_at);
}

#[derive(Serialize)]
pub struct Snapshot {
    pub claude: ToolUsage,
    pub codex: ToolUsage,
    pub sessions: BTreeMap<String, SessionTokens>,
    /// One per OpenRouter account that has a key. Filled by `with_openrouter`, because it asks over the network.
    pub openrouter: Vec<OrUsage>,
}

/// What an OpenRouter key has spent and has left, in dollars.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrUsage {
    pub id: String,
    pub name: String,
    /// Spent in all, and today and this month when OpenRouter says.
    pub spent: Option<f64>,
    pub daily: Option<f64>,
    pub monthly: Option<f64>,
    /// The key's credit limit and what is left of it; none for a key with no limit.
    pub limit: Option<f64>,
    pub remaining: Option<f64>,
    pub error: Option<String>,
    pub fetched_at: i64,
}

/// How long an account's numbers are reused, so the page's polling does not become a request to OpenRouter each time.
const OR_MAX_AGE_SECS: i64 = 45;
const OR_REFRESH_AGE_SECS: i64 = 8;

static OR_CACHE: std::sync::Mutex<Option<BTreeMap<String, OrUsage>>> = std::sync::Mutex::new(None);

/// The snapshot with each OpenRouter account's balance added. A failure is kept on the account, not raised.
pub async fn with_openrouter(st: &Arc<AppState>, mut snap: Snapshot, refresh: bool) -> Snapshot {
    let accounts = crate::secrets::load(&st.data_dir).await;
    let max_age = if refresh { OR_REFRESH_AGE_SECS } else { OR_MAX_AGE_SECS };
    let now = now_secs();
    let mut out = Vec::new();
    for a in accounts.accounts.iter().filter(|a| a.kind == crate::secrets::Kind::Openrouter && !a.key.is_empty()) {
        let held = OR_CACHE.lock().unwrap().as_ref().and_then(|m| m.get(&a.id).cloned());
        let u = match held {
            Some(u) if now - u.fetched_at < max_age && u.error.is_none() => u,
            _ => {
                let got = crate::openrouter::catalog::remote(a).await;
                let u = or_usage(a, got, now);
                OR_CACHE.lock().unwrap().get_or_insert_with(BTreeMap::new).insert(a.id.clone(), u.clone());
                u
            }
        };
        out.push(u);
    }
    snap.openrouter = out;
    snap
}

fn or_usage(a: &crate::secrets::Account, got: anyhow::Result<crate::openrouter::catalog::Remote>, now: i64) -> OrUsage {
    match got {
        Ok(r) => OrUsage { id: a.id.clone(), name: a.name.clone(), spent: r.usage, daily: r.usage_daily, monthly: r.usage_monthly, limit: r.limit, remaining: r.remaining, error: None, fetched_at: now },
        Err(e) => OrUsage { id: a.id.clone(), name: a.name.clone(), spent: None, daily: None, monthly: None, limit: None, remaining: None, error: Some(format!("{e:#}")), fetched_at: now },
    }
}

fn pool_window(bucket: &Value) -> Option<Window> {
    let used = bucket.get("utilization")?.as_f64()?;
    let resets_at = bucket.get("nextResetAt").and_then(Value::as_i64).map(|ms| ms / 1000);
    Some(Window { used_pct: (used * 100.0).clamp(0.0, 100.0), resets_at })
}

/// Team Claude's `aggregate` quota (0-1 utilisation, reset times in ms) as Claude usage.
fn from_pool(aggregate: &Value, now: i64) -> Option<ToolUsage> {
    let five_hour = aggregate.get("fiveHour").and_then(pool_window);
    let seven_day = aggregate.get("weeklyShared").and_then(pool_window);
    if five_hour.is_none() && seven_day.is_none() {
        return None;
    }
    let mut out = ToolUsage::new(now, Source::TeamClaude);
    out.five_hour = five_hour;
    out.seven_day = seven_day;
    for (key, name) in [("weeklySonnet", "sonnet"), ("weeklyFable", "fable")] {
        // A model window that matches the shared one says nothing more.
        if let Some(w) = aggregate.get(key).and_then(pool_window).filter(|w| seven_day.is_none_or(|s| (s.used_pct - w.used_pct).abs() > 0.05)) {
            out.per_model.push(ModelWindow { name: name.to_owned(), window: w });
        }
    }
    Some(out)
}

/// When Claude Code goes through Team Claude, the limit that matters is the pool's, not the one
/// signed-in account's. Falls back to `own` when Team Claude cannot be asked.
async fn pooled(st: &Arc<AppState>, own: ToolUsage) -> ToolUsage {
    let Some(origin) = crate::teamclaude::detect_global(st, crate::config::Agent::Claude).await else { return own };
    let Some(aggregate) = crate::teamclaude::pool(st, &origin).await else { return own };
    from_pool(&aggregate, now_secs()).unwrap_or(own)
}

/// Everything the strip and panel show. `refresh` is the panel being opened.
pub async fn snapshot(st: &Arc<AppState>, refresh: bool) -> Snapshot {
    let max_age = if refresh { REFRESH_AGE_SECS } else { MAX_AGE_SECS };
    let (claude, codex, sessions) = tokio::join!(
        async { pooled(st, current(st, Tool::Claude, max_age).await).await },
        current(st, Tool::Codex, max_age),
        session_tokens(st),
    );
    Snapshot { claude, codex, sessions, openrouter: Vec::new() }
}

// ---------------------------------------------------------------- statusline

/// `ttm statusline`: Claude Code runs this to draw its footer and pipes it a JSON status.
/// Print the footer first, then report the limits to the server. Never fails the footer.
pub fn run_statusline_cli() {
    use std::io::Write;
    let mut raw = String::new();
    let _ = std::io::stdin().take(256 * 1024).read_to_string(&mut raw);
    let Ok(mut payload) = serde_json::from_str::<Value>(&raw) else { return };
    // Say which ttm session this is, so the server can keep each one's details apart.
    if let (Some(obj), Ok(uid)) = (payload.as_object_mut(), std::env::var("TTM_SESSION_UID")) {
        obj.insert("ttm_uid".into(), Value::String(uid));
    }
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{}", status_line(&payload, now_secs()));
    let _ = out.flush();
    if from_statusline(&payload, 0).is_none() && payload["ttm_uid"].is_null() {
        return;
    }
    if let (Ok(url), Ok(token)) = (std::env::var("TTM_NOTIFY_URL"), std::env::var("TTM_NOTIFY_TOKEN")) {
        let _ = notify::post_local(&url, &token, "/api/internal/usage", &payload.to_string(), Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_791_500_000;

    #[test]
    fn an_openrouter_key_becomes_dollars_and_a_failure_stays_on_its_account() {
        let a = crate::secrets::Account { id: "a1".into(), name: "Main".into(), kind: crate::secrets::Kind::Openrouter, base_url: String::new(), key: "k".into(), use_in_orchestrator: true, models: Vec::new(), prefs: Default::default() };
        let ok = crate::openrouter::catalog::Remote { usage: Some(12.5), usage_daily: Some(0.4), usage_monthly: Some(7.0), limit: Some(50.0), remaining: Some(37.5), ..Default::default() };
        let u = or_usage(&a, Ok(ok), 100);
        assert_eq!((u.spent, u.daily, u.monthly, u.limit, u.remaining, u.error.clone()), (Some(12.5), Some(0.4), Some(7.0), Some(50.0), Some(37.5), None));
        assert_eq!((u.id.as_str(), u.name.as_str(), u.fetched_at), ("a1", "Main", 100));
        let bad = or_usage(&a, Err(anyhow::anyhow!("OpenRouter answered 401")), 100);
        assert_eq!((bad.spent, bad.remaining), (None, None));
        assert!(bad.error.unwrap().contains("401"));
    }

    #[test]
    fn team_claudes_pool_becomes_claude_usage() {
        let agg = json!({
            "fiveHour": {"utilization": 0.82, "nextResetAt": 1_791_994_200_000_i64},
            "weeklyShared": {"utilization": 0.085, "nextResetAt": 1_792_180_800_000_i64},
            "weeklySonnet": {"utilization": 0.085, "nextResetAt": 1_792_180_800_000_i64},
            "weeklyFable": {"utilization": 0.30, "nextResetAt": null},
        });
        let u = from_pool(&agg, NOW).unwrap();
        assert_eq!(u.source, Source::TeamClaude);
        assert_eq!(u.five_hour, Some(Window { used_pct: 82.0, resets_at: Some(1_791_994_200) }));
        assert_eq!(u.seven_day.map(|w| w.used_pct), Some(8.5));
        assert_eq!(u.per_model.len(), 1, "sonnet matches the shared window and is left out");
        assert_eq!(u.per_model[0].name, "fable");
        assert_eq!(u.per_model[0].window.resets_at, None);
        assert_eq!(u.plan, None);
        assert!(from_pool(&json!({}), NOW).is_none());
        assert!(from_pool(&json!({"fiveHour": {"utilization": null}}), NOW).is_none());
    }

    #[test]
    fn parses_times_in_every_shape() {
        assert_eq!(parse_time(&json!(1_791_501_082)), Some(1_791_501_082));
        assert_eq!(parse_time(&json!(1_791_501_082_000_i64)), Some(1_791_501_082));
        assert_eq!(parse_time(&json!("1791501082")), Some(1_791_501_082));
        // The real API sends a fraction and an offset.
        assert_eq!(parse_time(&json!("2026-10-09T02:19:59.696806+00:00")), Some(1_791_512_399));
        assert_eq!(parse_time(&json!("soon")), None);
        assert_eq!(parse_time(&Value::Null), None);
    }

    #[test]
    fn claude_api_response() {
        // Shape taken from a real response; the opus/sonnet caps are null on this plan.
        let body = json!({
            "five_hour": { "utilization": 19.0, "resets_at": "2026-10-09T02:19:59.696806+00:00",
                           "limit_dollars": null, "used_dollars": null },
            "seven_day": { "utilization": 29.0, "resets_at": "2026-10-14T01:59:59.696829+00:00" },
            "seven_day_oauth_apps": null,
            "seven_day_opus": null,
            "seven_day_sonnet": { "utilization": 4.5, "resets_at": "2026-10-14T01:59:59+00:00" },
            "seven_day_breakdown": { "as_of": "x", "rows": [], "window_started_at": "y" }
        });
        let u = parse_claude_api(&body, Some("pro".into()), NOW);
        assert_eq!(u.five_hour, Some(Window { used_pct: 19.0, resets_at: Some(1_791_512_399) }));
        assert_eq!(u.seven_day.map(|w| w.used_pct), Some(29.0));
        assert_eq!(u.per_model.len(), 1);
        assert_eq!(u.per_model[0].name, "sonnet");
        assert_eq!(u.plan.as_deref(), Some("pro"));
        assert_eq!(u.source, Source::Api);
    }

    #[test]
    fn codex_api_response() {
        let body = json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": { "used_percent": 98, "limit_window_seconds": 18000, "reset_at": 1_791_501_082 },
                "secondary_window": { "used_percent": 15, "limit_window_seconds": 604800, "reset_at": 1_792_087_882 }
            }
        });
        let u = parse_codex_api(&body, NOW);
        assert_eq!(u.five_hour, Some(Window { used_pct: 98.0, resets_at: Some(1_791_501_082) }));
        assert_eq!(u.seven_day, Some(Window { used_pct: 15.0, resets_at: Some(1_792_087_882) }));
        assert_eq!(u.plan.as_deref(), Some("plus"));
    }

    #[test]
    fn a_weekly_only_window_lands_in_the_weekly_slot() {
        // A plan with no 5h window reports its weekly one as "primary".
        let body = json!({ "rate_limit": { "primary_window": {
            "used_percent": 40, "limit_window_seconds": 604800, "reset_at": 1_792_087_882 } } });
        let u = parse_codex_api(&body, NOW);
        assert_eq!(u.five_hour, None);
        assert_eq!(u.seven_day.map(|w| w.used_pct), Some(40.0));
    }

    #[test]
    fn codex_log_entry() {
        let limits = json!({
            "primary": { "used_percent": 98.0, "window_minutes": 300, "resets_at": 1_791_501_082 },
            "secondary": { "used_percent": 15.0, "window_minutes": 10080, "resets_at": 1_792_087_882 },
            "plan_type": null
        });
        let u = parse_codex_log(&limits, NOW);
        assert_eq!(u.five_hour.map(|w| w.used_pct), Some(98.0));
        assert_eq!(u.seven_day.map(|w| w.used_pct), Some(15.0));
        assert_eq!(u.source, Source::SessionLog);
        assert_eq!(u.plan, None);
    }

    #[test]
    fn statusline_input_and_footer() {
        let v = json!({
            "model": { "display_name": "Opus" },
            "rate_limits": {
                "five_hour": { "used_percentage": 42.4, "resets_at": NOW + 2 * 3600 + 10 * 60 },
                "seven_day": { "used_percentage": 18.0, "resets_at": NOW + 86_400 }
            }
        });
        assert_eq!(status_line(&v, NOW), "5h 42% (2h10m) · 7d 18%");
        let u = from_statusline(&v, NOW).unwrap();
        assert_eq!(u.source, Source::Hook);
        assert_eq!(u.seven_day.map(|w| w.used_pct), Some(18.0));
    }

    #[test]
    fn a_sessions_own_details_are_read_from_its_status_json() {
        let v = json!({ "effort": { "level": "high" }, "model": { "display_name": "Opus" }, "ttm_uid": "abc" });
        assert_eq!(status_live(&v), StatusLive { effort: Some("high".into()) });
        assert_eq!(status_live(&json!({})), StatusLive::default());
        assert_eq!(status_live(&json!({ "effort": { "level": "" } })).effort, None);
    }

    #[test]
    fn statusline_without_limits_falls_back_to_the_model() {
        let v = json!({ "model": { "display_name": "Opus" }, "rate_limits": null });
        assert!(from_statusline(&v, NOW).is_none());
        assert_eq!(status_line(&v, NOW), "Opus");
        assert_eq!(status_line(&json!({}), NOW), "");
    }

    #[test]
    fn remaining_time_reads_naturally() {
        assert_eq!(fmt_remaining(-5), "now");
        assert_eq!(fmt_remaining(61), "2m");
        assert_eq!(fmt_remaining(3600 + 5 * 60), "1h05m");
        assert_eq!(fmt_remaining(2 * 86_400 + 3 * 3600), "2d3h");
    }

    #[test]
    fn credentials_never_leak_into_errors() {
        let ok = parse_claude_creds(
            r#"{"claudeAiOauth":{"accessToken":"sk-secret","expiresAt":1791500000000,"subscriptionType":"pro"}}"#,
        )
        .unwrap();
        assert_eq!(ok.token, "sk-secret");
        assert_eq!(ok.expires_at, Some(1_791_500_000));
        assert_eq!(ok.plan.as_deref(), Some("pro"));
        let err = parse_claude_creds(r#"{"claudeAiOauth":{"accessToken":""}}"#).err().unwrap();
        assert!(!format!("{err:#}").contains("sk-"));
        assert!(parse_claude_creds(r#"{"other":1}"#).is_err());
        assert!(parse_claude_creds("not json").is_err());
    }

    fn at(pct: f64, reset: i64) -> ToolUsage {
        ToolUsage {
            five_hour: Some(Window { used_pct: pct, resets_at: Some(reset) }),
            ..ToolUsage::new(NOW, Source::Api)
        }
    }

    #[test]
    fn alerts_once_per_window() {
        let usage = Usage::new();
        let reset = NOW + 3600;
        assert!(usage.alerts_due(Tool::Claude, &at(80.0, reset), NOW).is_empty());
        let first = usage.alerts_due(Tool::Claude, &at(91.0, reset), NOW);
        assert_eq!(first, vec!["5h 91% (resets in 1h00m)".to_owned()]);
        assert!(usage.alerts_due(Tool::Claude, &at(95.0, reset), NOW).is_empty());
        // The hook reports the same reset a second off: still the same window.
        assert!(usage.alerts_due(Tool::Claude, &at(96.0, reset + 1), NOW).is_empty());
        // Codex has its own state.
        assert_eq!(usage.alerts_due(Tool::Codex, &at(91.0, reset), NOW).len(), 1);
        // Five hours later it is a new window and may warn again.
        assert_eq!(usage.alerts_due(Tool::Claude, &at(92.0, reset + 5 * 3600), NOW).len(), 1);
    }

    #[test]
    fn claude_transcript_counts_each_message_once() {
        let mut scan = ClaudeScan::default();
        let line = |id: &str, out: u64| {
            json!({ "type": "assistant", "message": { "id": id, "usage": {
                "input_tokens": 10, "output_tokens": out,
                "cache_read_input_tokens": 100, "cache_creation_input_tokens": 5 } } })
            .to_string()
        };
        // Message `a` streams twice; only its final usage counts.
        scan.ingest(line("a", 1).as_bytes());
        scan.ingest(line("a", 20).as_bytes());
        scan.ingest(line("b", 7).as_bytes());
        scan.ingest(br#"{"type":"user","message":{"content":"hi"}}"#);
        scan.ingest(b"not json at all \"usage\"");
        assert_eq!(scan.total(), Tokens { input: 20, output: 27, cache_read: 200, cache_write: 10 });
    }

    #[test]
    fn transcript_scan_is_incremental_and_waits_for_whole_lines() {
        let dir = std::env::temp_dir().join(format!("ttm-usage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let msg = |id: &str| {
            json!({ "type": "assistant", "message": { "id": id, "usage": { "output_tokens": 3 } } }).to_string()
        };
        std::fs::write(&path, format!("{}\n{}", msg("a"), msg("b"))).unwrap(); // `b` has no newline yet
        let mut scan = ClaudeScan::default();
        scan.update(&path).unwrap();
        assert_eq!(scan.total().output, 3);
        std::fs::write(&path, format!("{}\n{}\n{}\n", msg("a"), msg("b"), msg("c"))).unwrap();
        scan.update(&path).unwrap();
        assert_eq!(scan.total().output, 9);
        // A rewritten, shorter file starts over.
        std::fs::write(&path, format!("{}\n", msg("z"))).unwrap();
        scan.update(&path).unwrap();
        assert_eq!(scan.total().output, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_total_splits_out_cached_input() {
        let dir = std::env::temp_dir().join(format!("ttm-usage-codex-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rollout.jsonl");
        let entry = |input: u64| {
            json!({ "type": "event_msg", "payload": { "type": "token_count", "info": {
                "total_token_usage": { "input_tokens": input, "cached_input_tokens": 40, "output_tokens": 9 } } } })
                .to_string()
        };
        std::fs::write(&path, format!("{}\n{}\n{{\"truncated\n", entry(100), entry(150))).unwrap();
        assert_eq!(
            codex_totals(&path),
            Some(Tokens { input: 110, output: 9, cache_read: 40, cache_write: 0 })
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
