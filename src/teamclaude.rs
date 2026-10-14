//! Team Claude (`@karpeleslab/teamclaude`, also called TeamRouter): a local proxy that pools
//! several Claude and Codex accounts. Claude Code reaches it through `ANTHROPIC_BASE_URL` and
//! Codex through a `model_providers` entry, so this module works out whether a pane's agent goes
//! through it, then reads its status, quota and request log, and can switch the active account.
//! Nothing here returns a credential: JSON from the proxy is scrubbed and log files are read
//! for a handful of fields only.

use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;

use crate::{AppState, config::Agent, lookups::Lookups};

const PROBE_TTL: Duration = Duration::from_secs(30);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// How much of a request log is read from each end; the head has the request, the tail the status.
const LOG_EDGE: u64 = 16 * 1024;
/// Newest log files looked at per request for the feed.
const LOG_SCAN: usize = 400;
pub const FEED_LIMIT: usize = 60;
/// How long a request waits for the settings files to be read.
const URL_WAIT: Duration = Duration::from_millis(400);

pub fn provider_of(agent: Agent) -> Option<&'static str> {
    match agent {
        Agent::Claude => Some("anthropic"),
        Agent::Codex => Some("codex"),
        Agent::Other => None,
    }
}

// ---------------------------------------------------------------- detection

/// `scheme://host[:port]` of a base URL, or `None` for anything that is not plain http(s).
fn origin_of(url: &str) -> Option<String> {
    let u = reqwest::Url::parse(url.trim()).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let host = u.host_str()?;
    Some(match u.port() {
        Some(p) => format!("{}://{host}:{p}", u.scheme()),
        None => format!("{}://{host}", u.scheme()),
    })
}

pub fn is_loopback(origin: &str) -> bool {
    reqwest::Url::parse(origin)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .is_some_and(|h| matches!(h.as_str(), "localhost" | "127.0.0.1" | "[::1]" | "::1"))
}

fn settings_base_url(path: &Path) -> Option<String> {
    let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let url = v.get("env")?.get("ANTHROPIC_BASE_URL")?.as_str()?.trim();
    (!url.is_empty()).then(|| url.to_owned())
}

/// Where Claude Code will send requests: the process environment, then the project's settings
/// (local before shared, from `cwd` upwards, when there is a project), then the user's own.
pub fn claude_base_url(cwd: Option<&Path>, env: Option<&str>, claude_dir: Option<&Path>) -> Option<String> {
    if let Some(url) = env.map(str::trim).filter(|u| !u.is_empty()) {
        return Some(url.to_owned());
    }
    cwd.into_iter()
        .flat_map(Path::ancestors)
        .flat_map(|d| [d.join(".claude/settings.local.json"), d.join(".claude/settings.json")])
        .chain(claude_dir.map(|d| d.join("settings.json")))
        .find_map(|p| settings_base_url(&p))
}

/// Where Codex will send requests: the `base_url` of the provider `model_provider` names.
pub fn codex_base_url(codex_dir: Option<&Path>) -> Option<String> {
    let text = std::fs::read_to_string(codex_dir?.join("config.toml")).ok()?;
    let t: toml::Table = text.parse().ok()?;
    let provider = t.get("model_provider")?.as_str()?;
    let url = t.get("model_providers")?.get(provider)?.get("base_url")?.as_str()?.trim();
    (!url.is_empty()).then(|| url.to_owned())
}

pub struct Probes {
    http: reqwest::Client,
    seen: Mutex<HashMap<String, (Instant, bool)>>,
    /// The address each agent is set to use, per folder. Reading the settings files is a file open
    /// in the session's project folder, so it is kept off the runtime and remembered (see `lookups`).
    urls: Lookups<(u8, PathBuf), Option<String>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Probes {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(CALL_TIMEOUT)
                .user_agent(concat!("ttm/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            seen: Mutex::default(),
            urls: Lookups::new(PROBE_TTL / 6, URL_WAIT),
        }
    }

    /// Whether `origin` answers like Team Claude (its status route lists accounts), asked at most
    /// once every `PROBE_TTL`.
    async fn answers(&self, origin: &str) -> bool {
        if let Some(&(at, yes)) = lock(&self.seen).get(origin)
            && at.elapsed() < PROBE_TTL
        {
            return yes;
        }
        let yes = match self.http.get(format!("{origin}/teamclaude/status")).timeout(PROBE_TIMEOUT).send().await {
            Ok(r) if r.status().is_success() => r.json::<Value>().await.is_ok_and(|v| v.get("accounts").is_some_and(Value::is_array)),
            _ => false,
        };
        lock(&self.seen).insert(origin.to_owned(), (Instant::now(), yes));
        yes
    }

    async fn get(&self, origin: &str, path: &str) -> Result<Value> {
        let r = self.http.get(format!("{origin}{path}")).send().await.with_context(|| format!("reaching Team Claude at {origin}"))?;
        if !r.status().is_success() {
            bail!("Team Claude answered {} for {path}", r.status());
        }
        let mut v: Value = r.json().await.with_context(|| format!("reading Team Claude's {path}"))?;
        scrub(&mut v);
        Ok(v)
    }
}

/// The Team Claude `agent` goes through when started in `cwd` (its origin), if it goes through one.
pub async fn detect(st: &AppState, agent: Agent, cwd: &Path) -> Option<String> {
    routed(st, agent, Some(cwd)).await
}

/// The same for an agent started with no project settings: what the machine-wide plan usage uses.
pub async fn detect_global(st: &AppState, agent: Agent) -> Option<String> {
    routed(st, agent, None).await
}

async fn routed(st: &AppState, agent: Agent, cwd: Option<&Path>) -> Option<String> {
    let url = match agent {
        Agent::Claude => {
            let (env, dir, cwd) = (std::env::var("ANTHROPIC_BASE_URL").ok(), crate::agentlog::claude_dir(), cwd.map(Path::to_owned));
            let key = (0, cwd.clone().unwrap_or_default());
            st.teamclaude.urls.get(key, move || claude_base_url(cwd.as_deref(), env.as_deref(), dir.as_deref())).await.flatten()
        }
        Agent::Codex => {
            let dir = crate::agentlog::codex_dir();
            st.teamclaude.urls.get((1, PathBuf::new()), move || codex_base_url(dir.as_deref())).await.flatten()
        }
        Agent::Other => None,
    }?;
    let origin = origin_of(&url)?;
    st.teamclaude.answers(&origin).await.then_some(origin)
}

// ---------------------------------------------------------------- reading

/// Drop string values under keys that look like credentials. Team Claude's status has none
/// today; this is so that a future field cannot reach the browser by accident.
fn scrub(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|k, val| !(val.is_string() && secret_key(k)));
            m.values_mut().for_each(scrub);
        }
        Value::Array(a) => a.iter_mut().for_each(scrub),
        _ => {}
    }
}

fn secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase().replace(['_', '-'], "");
    ["accesstoken", "refreshtoken", "idtoken", "apikey", "secret", "password", "authorization", "cookie", "credential", "bearer"]
        .iter()
        .any(|s| k.contains(s))
}

#[derive(Serialize)]
pub struct Overview {
    pub origin: String,
    pub provider: &'static str,
    /// The proxy runs on this machine, so its dashboard and request log are reachable.
    pub loopback: bool,
    pub dashboard: String,
    pub status: Value,
    pub quota: Value,
}

pub async fn overview(st: &AppState, origin: &str, agent: Agent) -> Result<Overview> {
    let provider = provider_of(agent).context("this pane does not run Claude or Codex")?;
    let (status, quota) = tokio::join!(st.teamclaude.get(origin, "/teamclaude/status"), st.teamclaude.get(origin, "/teamclaude/quota"));
    Ok(Overview {
        origin: origin.to_owned(),
        provider,
        loopback: is_loopback(origin),
        dashboard: format!("{origin}/teamclaude/dashboard"),
        status: status?,
        quota: quota.unwrap_or(Value::Null),
    })
}

/// The pooled limits across every account (`/teamclaude/quota` → `aggregate`).
pub async fn pool(st: &AppState, origin: &str) -> Option<Value> {
    st.teamclaude.get(origin, "/teamclaude/quota").await.ok()?.get("aggregate").cloned()
}

// ---------------------------------------------------------------- switching

pub enum SwitchError {
    /// Not allowed or not possible; the message says why.
    Refused(String),
    /// The proxy could not be reached or failed.
    Failed(anyhow::Error),
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

/// Team Claude finds the account to switch to by name and takes the first one that matches, so
/// when two providers have an account of the same name only the first can be reached. This makes
/// sure that the first one is a `provider` account, so a Claude pane never moves Codex's account.
fn check_switch(status: &Value, provider: &str, account: &str) -> Result<(), String> {
    let accounts = status.get("accounts").and_then(Value::as_array).ok_or("Team Claude sent no account list")?;
    let wanted = norm(account);
    let first = accounts
        .iter()
        .find(|a| a.get("name").and_then(Value::as_str).is_some_and(|n| norm(n) == wanted))
        .ok_or_else(|| format!("Team Claude has no account called {account}"))?;
    if first.get("provider").and_then(Value::as_str) != Some(provider) {
        return Err(format!(
            "{account} is also the name of an account for another provider that Team Claude finds first, so it cannot switch a {provider} pane to it by name. Use its dashboard."
        ));
    }
    Ok(())
}

pub async fn switch(st: &AppState, origin: &str, agent: Agent, account: &str) -> Result<Value, SwitchError> {
    let provider = provider_of(agent).ok_or_else(|| SwitchError::Refused("this pane does not run Claude or Codex".into()))?;
    if !is_loopback(origin) {
        return Err(SwitchError::Refused("accounts are only switched on the machine Team Claude runs on".into()));
    }
    let status = st.teamclaude.get(origin, "/teamclaude/status").await.map_err(SwitchError::Failed)?;
    check_switch(&status, provider, account).map_err(SwitchError::Refused)?;
    let reply = st
        .teamclaude
        .http
        .post(format!("{origin}/teamclaude/switch"))
        .json(&serde_json::json!({ "account": account }))
        .send()
        .await
        .context("reaching Team Claude")
        .map_err(SwitchError::Failed)?;
    let ok = reply.status().is_success();
    let mut body: Value = reply.json().await.unwrap_or(Value::Null);
    scrub(&mut body);
    if ok {
        Ok(body)
    } else {
        let why = body.get("error").and_then(Value::as_str).unwrap_or("it refused");
        Err(SwitchError::Refused(format!("Team Claude would not switch: {why}")))
    }
}

// ---------------------------------------------------------------- request log

/// Team Claude's config file: `TEAMROUTER_CONFIG` / `TEAMCLAUDE_CONFIG`, else `teamrouter.json`
/// when it exists, else `teamclaude.json`, under `$XDG_CONFIG_HOME` or `~/.config`.
fn config_path() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(p) = var("TEAMROUTER_CONFIG").or_else(|| var("TEAMCLAUDE_CONFIG")) {
        return Some(p);
    }
    let dir = var("XDG_CONFIG_HOME").or_else(|| crate::agentlog::home().map(|h| h.join(".config")))?;
    let renamed = dir.join("teamrouter.json");
    Some(if renamed.exists() { renamed } else { dir.join("teamclaude.json") })
}

/// The directory Team Claude writes one file per request into, and how much it records.
fn log_settings(config: &Path) -> Option<(PathBuf, String)> {
    let v: Value = serde_json::from_slice(&std::fs::read(config).ok()?).ok()?;
    let dir = v.get("logDir")?.as_str()?.trim();
    if dir.is_empty() {
        return None;
    }
    let level = v.get("logLevel").and_then(Value::as_str).unwrap_or("body");
    Some((PathBuf::from(dir), level.to_owned()))
}

/// `20261014_135804.123_00042.log`, the only names Team Claude writes.
fn is_log_name(n: &str) -> bool {
    let b = n.as_bytes();
    let digits = |r: std::ops::Range<usize>| b.get(r).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    n.ends_with(".log")
        && b.len() >= "20261014_135804.123_00042.log".len()
        && digits(0..8)
        && b[8] == b'_'
        && digits(9..15)
        && b[15] == b'.'
        && digits(16..19)
        && b[19] == b'_'
        && digits(20..b.len() - 4)
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Request {
    /// When the log was last written, which is when the request ended.
    pub at: u64,
    pub account: String,
    pub retry: u32,
    pub method: String,
    pub path: String,
    pub status: Option<u16>,
    pub error: bool,
    /// Whether the request came from the pane's own agent conversation.
    pub mine: bool,
    #[serde(skip)]
    session: Option<String>,
}

fn url_path(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let path = rest.find('/').map_or("/", |i| &rest[i..]);
    path.split('?').next().unwrap_or("/").to_owned()
}

/// Read what the feed shows from a log's two ends. Headers are only looked at for the session id;
/// nothing else from them (and nothing from a body) is kept.
fn parse_log(head: &str, tail: &str, at: u64) -> Option<Request> {
    let mut lines = head.lines();
    let first = lines.find(|l| l.starts_with("=== REQUEST"))?;
    let inner = first.strip_prefix("=== REQUEST (account: ")?.strip_suffix(") ===")?;
    let (account, retry) = inner.rsplit_once(", retry: ")?;
    let (method, url) = lines.next()?.split_once(' ')?;
    let mut session = None;
    for l in lines.take_while(|l| l.starts_with("  ")) {
        if let Some((k, v)) = l.trim().split_once(": ")
            && (k.eq_ignore_ascii_case("x-claude-code-session-id") || (session.is_none() && k.eq_ignore_ascii_case("session-id")))
        {
            session = Some(v.trim().to_owned());
        }
    }
    let status = [tail, head].iter().find_map(|text| {
        let rest = &text[text.rfind("=== RESPONSE ")? + "=== RESPONSE ".len()..];
        rest.split_whitespace().next()?.parse::<u16>().ok()
    });
    Some(Request {
        at,
        account: account.to_owned(),
        retry: retry.parse().unwrap_or(0),
        method: method.to_owned(),
        path: url_path(url),
        status,
        error: tail.contains("=== ERROR ==="),
        mine: false,
        session,
    })
}

fn read_edges(path: &Path) -> std::io::Result<(String, String, u64)> {
    let mut f = File::open(path)?;
    let meta = f.metadata()?;
    let mut head = Vec::new();
    (&mut f).take(LOG_EDGE).read_to_end(&mut head)?;
    let tail = if meta.len() <= LOG_EDGE { head.clone() } else { crate::agentlog::read_tail(path, LOG_EDGE)? };
    let at = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    Ok((String::from_utf8_lossy(&head).into_owned(), String::from_utf8_lossy(&tail).into_owned(), at))
}

#[derive(Serialize)]
pub struct Feed {
    /// Request logging is configured on the proxy and its folder can be read.
    pub enabled: bool,
    /// `off`, `headers` or `body`, as the proxy is set.
    pub level: Option<String>,
    pub requests: Vec<Request>,
}

/// The newest requests, newest first. `own` is the pane's conversation id: its requests are
/// marked, and with `only_own` they are the only ones returned.
fn read_feed(config: Option<&Path>, own: Option<&str>, only_own: bool, limit: usize) -> Feed {
    let Some((dir, level)) = config.and_then(log_settings) else {
        return Feed { enabled: false, level: None, requests: Vec::new() };
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Feed { enabled: false, level: Some(level), requests: Vec::new() };
    };
    let mut names: Vec<String> = entries.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| is_log_name(n)).collect();
    names.sort_unstable_by(|a, b| b.cmp(a));
    let mut requests = Vec::new();
    for name in names.into_iter().take(LOG_SCAN) {
        let Ok((head, tail, at)) = read_edges(&dir.join(&name)) else { continue };
        let Some(mut r) = parse_log(&head, &tail, at) else { continue };
        r.mine = own.is_some() && r.session.as_deref() == own;
        if only_own && !r.mine {
            continue;
        }
        requests.push(r);
        if requests.len() >= limit {
            break;
        }
    }
    Feed { enabled: true, level: Some(level), requests }
}

/// The request log only exists on the machine Team Claude runs on.
pub async fn feed(origin: &str, own: Option<String>, only_own: bool, limit: usize) -> Feed {
    if !is_loopback(origin) {
        return Feed { enabled: false, level: None, requests: Vec::new() };
    }
    tokio::task::spawn_blocking(move || read_feed(config_path().as_deref(), own.as_deref(), only_own, limit))
        .await
        .unwrap_or(Feed { enabled: false, level: None, requests: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ttm-tc-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    fn write_settings(dir: &Path, file: &str, url: &str) {
        fs::create_dir_all(dir.join(".claude")).unwrap();
        fs::write(dir.join(".claude").join(file), format!(r#"{{"env":{{"ANTHROPIC_BASE_URL":"{url}"}}}}"#)).unwrap();
    }

    #[test]
    fn claude_settings_layers_are_tried_in_order() {
        let base = tmp("claude");
        let user = base.join("user");
        let proj = base.join("proj");
        let sub = proj.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::create_dir_all(&user).unwrap();
        assert_eq!(claude_base_url(Some(&sub), None, Some(&user)), None);

        fs::write(user.join("settings.json"), r#"{"env":{"ANTHROPIC_BASE_URL":"http://user:1"}}"#).unwrap();
        assert_eq!(claude_base_url(Some(&sub), None, Some(&user)).as_deref(), Some("http://user:1"));

        write_settings(&proj, "settings.json", "http://project:2");
        assert_eq!(claude_base_url(Some(&sub), None, Some(&user)).as_deref(), Some("http://project:2"));

        write_settings(&proj, "settings.local.json", "http://local:3");
        assert_eq!(claude_base_url(Some(&sub), None, Some(&user)).as_deref(), Some("http://local:3"));

        assert_eq!(claude_base_url(Some(&sub), Some(" http://env:4 "), Some(&user)).as_deref(), Some("http://env:4"));
        assert_eq!(claude_base_url(Some(&sub), Some("  "), Some(&user)).as_deref(), Some("http://local:3"), "a blank variable is unset");
    }

    #[test]
    fn codex_base_url_follows_the_chosen_provider() {
        let dir = tmp("codex");
        fs::write(
            dir.join("config.toml"),
            "model_provider = \"teamclaude\"\n[model_providers.teamclaude]\nname = \"t\"\nbase_url = \"http://127.0.0.1:3456/backend-api/codex\"\n[model_providers.other]\nbase_url = \"http://other\"\n",
        )
        .unwrap();
        assert_eq!(codex_base_url(Some(&dir)).as_deref(), Some("http://127.0.0.1:3456/backend-api/codex"));
        fs::write(dir.join("config.toml"), "model = \"x\"\n").unwrap();
        assert_eq!(codex_base_url(Some(&dir)), None, "no provider chosen means OpenAI's own");
        assert_eq!(codex_base_url(None), None);
    }

    #[test]
    fn origins_drop_the_path() {
        assert_eq!(origin_of("http://127.0.0.1:3456/backend-api/codex").as_deref(), Some("http://127.0.0.1:3456"));
        assert_eq!(origin_of("https://proxy.example.com/v1").as_deref(), Some("https://proxy.example.com"));
        assert_eq!(origin_of("ftp://x"), None);
        assert_eq!(origin_of("not a url"), None);
        assert!(is_loopback("http://127.0.0.1:3456"));
        assert!(is_loopback("http://localhost:3456"));
        assert!(!is_loopback("https://proxy.example.com"));
    }

    #[test]
    fn credentials_are_scrubbed_but_token_counts_stay() {
        let mut v = json!({
            "accounts": [{"name": "a", "accessToken": "sk-1", "refresh_token": "r", "apiKey": "k",
                          "usage": {"totalInputTokens": 5, "tokensRemaining": 7}}],
            "sessions": {"tokens": {"input": 1}},
            "x": {"Authorization": "Bearer z", "cookie": "c"},
        });
        scrub(&mut v);
        let text = v.to_string();
        for leaked in ["sk-1", "\"r\"", "Bearer", "\"c\""] {
            assert!(!text.contains(leaked), "{leaked} in {text}");
        }
        assert_eq!(v["accounts"][0]["usage"]["totalInputTokens"], 5);
        assert_eq!(v["accounts"][0]["usage"]["tokensRemaining"], 7);
        assert_eq!(v["sessions"]["tokens"]["input"], 1);
    }

    fn status() -> Value {
        json!({"accounts": [
            {"name": "s@ndeep.me", "provider": "anthropic"},
            {"name": "s@ndeep.me", "provider": "codex"},
            {"name": "claude@sndp.me", "provider": "anthropic"},
        ]})
    }

    #[test]
    fn switching_never_crosses_providers() {
        let s = status();
        assert!(check_switch(&s, "anthropic", "claude@sndp.me").is_ok());
        assert!(check_switch(&s, "anthropic", " S@ndeep.me ").is_ok(), "the first of that name is Anthropic's");
        assert!(check_switch(&s, "codex", "s@ndeep.me").is_err(), "by name Team Claude would pick Anthropic's");
        assert!(check_switch(&s, "codex", "claude@sndp.me").is_err(), "that is an Anthropic account");
        assert!(check_switch(&s, "anthropic", "nobody").is_err());
        assert!(check_switch(&json!({}), "anthropic", "x").is_err());
    }

    #[test]
    fn log_names_are_recognised() {
        assert!(is_log_name("20261014_135804.123_00042.log"));
        assert!(is_log_name("20261014_135804.123_123456.log"));
        assert!(!is_log_name("20261014_135804.123_00042.txt"));
        assert!(!is_log_name("notes.log"));
        assert!(!is_log_name("../20261014_135804.123_00042.log"));
    }

    const HEAD: &str = "=== REQUEST (account: s@ndeep.me (Org), retry: 1) ===
POST https://api.anthropic.com/v1/messages?beta=true
  accept: application/json
  authorization: Bearer sk-ant-oat01-SECRET...
  x-claude-code-session-id: 795aaf4d-7f97
  user-agent: claude-cli

=== REQUEST BODY ===
{\"secret prompt\": true}";

    #[test]
    fn a_log_is_read_for_the_feed_only() {
        let tail = "\n\n=== RESPONSE 429 ===\n  retry-after: 3\n";
        let r = parse_log(HEAD, tail, 99).unwrap();
        assert_eq!(r.account, "s@ndeep.me (Org)");
        assert_eq!((r.retry, r.method.as_str(), r.path.as_str()), (1, "POST", "/v1/messages"));
        assert_eq!(r.status, Some(429));
        assert_eq!(r.session.as_deref(), Some("795aaf4d-7f97"));
        assert!(!r.error);
        let json = serde_json::to_string(&r).unwrap();
        for leaked in ["SECRET", "sk-ant", "authorization", "795aaf4d", "secret prompt", "beta=true"] {
            assert!(!json.contains(leaked), "{leaked} in {json}");
        }
    }

    #[test]
    fn codex_requests_use_session_id_and_failures_are_flagged() {
        let head = "=== REQUEST (account: s@ndeep.me, retry: 0) ===\nPOST https://chatgpt.com/backend-api/codex/responses\n  session-id: thr-1\n";
        let r = parse_log(head, "\n\n=== ERROR ===\nboom", 1).unwrap();
        assert_eq!(r.session.as_deref(), Some("thr-1"));
        assert_eq!(r.status, None);
        assert!(r.error);
        assert!(parse_log("garbage", "", 0).is_none());
    }

    #[test]
    fn the_feed_filters_to_one_conversation() {
        let base = tmp("feed");
        let logs = base.join("logs");
        fs::create_dir_all(&logs).unwrap();
        let cfg = base.join("teamclaude.json");
        fs::write(&cfg, format!(r#"{{"logDir":"{}","logLevel":"headers"}}"#, logs.display())).unwrap();
        let log = |n: &str, sid: &str, status: u16| {
            fs::write(
                logs.join(n),
                format!("=== REQUEST (account: a, retry: 0) ===\nPOST https://api.anthropic.com/v1/messages\n  x-claude-code-session-id: {sid}\n\n=== RESPONSE {status} ===\n"),
            )
            .unwrap();
        };
        log("20261014_100000.000_00001.log", "mine", 200);
        log("20261014_100001.000_00002.log", "theirs", 200);
        log("20261014_100002.000_00003.log", "mine", 529);
        fs::write(logs.join("stray.txt"), "ignore me").unwrap();

        let mine = read_feed(Some(&cfg), Some("mine"), true, 10);
        assert!(mine.enabled);
        assert_eq!(mine.level.as_deref(), Some("headers"));
        assert_eq!(mine.requests.iter().map(|r| r.status).collect::<Vec<_>>(), [Some(529), Some(200)], "newest first");
        assert!(mine.requests.iter().all(|r| r.mine));

        let all = read_feed(Some(&cfg), Some("mine"), false, 2);
        assert_eq!(all.requests.len(), 2, "the limit applies");
    }

    #[test]
    fn a_missing_log_dir_means_logging_is_off() {
        let base = tmp("nolog");
        let cfg = base.join("teamclaude.json");
        fs::write(&cfg, r#"{"accounts":[]}"#).unwrap();
        assert!(!read_feed(Some(&cfg), None, false, 5).enabled);
        fs::write(&cfg, format!(r#"{{"logDir":"{}"}}"#, base.join("gone").display())).unwrap();
        let f = read_feed(Some(&cfg), None, false, 5);
        assert!(!f.enabled);
        assert_eq!(f.level.as_deref(), Some("body"));
        assert!(!read_feed(None, None, false, 5).enabled);
    }
}
