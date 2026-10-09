//! Needs-input alerts: Web Push delivery, per-session hook shims, and the
//! tiny HTTP client the `ttm notify` hook command uses to reach the server.

use std::{
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use serde::{Deserialize, Serialize};
use web_push::{
    ContentEncoding, HyperWebPushClient, SubscriptionInfo, Urgency, VapidSignatureBuilder,
    WebPushClient, WebPushError, WebPushMessageBuilder,
};

use crate::AppState;

const PUSH_TTL_SECS: u32 = 3600;
const SEND_TIMEOUT: Duration = Duration::from_secs(15);
/// Don't repeat the same kind of alert for a session more often than this.
const MIN_GAP: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[value(rename_all = "snake_case")]
pub enum Kind {
    NeedsInput,
    Done,
    Idle,
    Exited,
    Test,
    /// Not an alert: the hook only reports the agent's conversation id.
    Session,
    /// A plan limit (5h or weekly) is nearly used up. Not tied to a tmux session.
    Limit,
    /// The tunnel's public address changed. Not tied to a tmux session.
    Tunnel,
}

#[derive(Debug, Clone, Serialize)]
pub struct Alert {
    pub session: String,
    pub kind: Kind,
    pub message: Option<String>,
    /// The question the agent is asking, when we could read it off the screen.
    pub prompt: Option<crate::screen::Prompt>,
}

/// A button on the notification.
#[derive(Serialize)]
struct PushAction {
    /// `choice:<n>`; the service worker turns it into an answer request.
    action: String,
    title: String,
}

#[derive(Serialize)]
struct PushPayload<'a> {
    title: String,
    body: String,
    session: &'a str,
    prompt_id: Option<&'a str>,
    actions: Vec<PushAction>,
    /// A usage-limit warning: `session` is not a real session, so clicking must not open it.
    limit: bool,
    /// The tunnel's new address: clicking opens it instead of a session.
    open_url: Option<String>,
}

fn short(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_owned(),
    }
}

impl Alert {
    pub fn new(session: &str, kind: Kind, message: Option<String>) -> Self {
        Self { session: session.to_owned(), kind, message, prompt: None }
    }

    /// Notification buttons: approve and deny, taken from the screen's own wording.
    fn actions(&self) -> Vec<PushAction> {
        let Some(p) = self.prompt.as_ref().filter(|_| self.kind == Kind::NeedsInput) else {
            return Vec::new();
        };
        let (yes, no) = (crate::screen::approve_index(p), crate::screen::deny_index(p));
        let label = |i: usize| p.choices.iter().find(|c| c.index == i).map(|c| short(&c.label, 24));
        let mut out: Vec<PushAction> = Vec::new();
        for i in [yes, no] {
            if let Some(title) = label(i)
                && !out.iter().any(|a| a.action == format!("choice:{i}"))
            {
                out.push(PushAction { action: format!("choice:{i}"), title });
            }
        }
        out
    }
}

impl Alert {
    fn title(&self, agent: &str) -> String {
        if self.kind == Kind::Test || self.kind == Kind::Session {
            return "ttm test notification".into();
        }
        if self.kind == Kind::Limit {
            return "Usage limit".into();
        }
        if self.kind == Kind::Tunnel {
            return "ttm has a new address".into();
        }
        let what = match self.kind {
            Kind::NeedsInput => "needs your input",
            Kind::Done => "finished",
            Kind::Idle => "went quiet",
            Kind::Exited => "exited",
            Kind::Test | Kind::Session | Kind::Limit | Kind::Tunnel => unreachable!("handled above"),
        };
        format!("{} {what}", if agent.is_empty() { &self.session } else { agent })
    }
}

// ---------------------------------------------------------------- VAPID keys

/// Raw base64url P-256 private key, created on first run (mode 0600).
pub fn load_or_create_vapid(data_dir: &Path) -> Result<String> {
    let path = data_dir.join("vapid.key");
    if let Ok(k) = std::fs::read_to_string(&path) {
        let k = k.trim().to_owned();
        if VapidSignatureBuilder::from_base64_no_sub(&k).is_ok() {
            return Ok(k);
        }
    }
    // Not every 32-byte string is a valid scalar; retry until one is.
    let key = loop {
        let mut raw = [0u8; 32];
        rand::fill(&mut raw[..]);
        let enc = B64.encode(raw);
        if VapidSignatureBuilder::from_base64_no_sub(&enc).is_ok() {
            break enc;
        }
    };
    write_secret(&path, &key)?;
    Ok(key)
}

pub fn public_key(private_b64: &str) -> Result<String> {
    let b = VapidSignatureBuilder::from_base64_no_sub(private_b64)
        .map_err(|e| anyhow::anyhow!("bad VAPID key: {e}"))?;
    Ok(B64.encode(b.get_public_key()))
}

/// Shared secret between the server and hook processes it launches.
pub fn load_or_create_token(data_dir: &Path) -> Result<String> {
    let path = data_dir.join("notify.token");
    if let Ok(t) = std::fs::read_to_string(&path)
        && t.trim().len() >= 32
    {
        return Ok(t.trim().to_owned());
    }
    let mut raw = [0u8; 32];
    rand::fill(&mut raw[..]);
    let t = B64.encode(raw);
    write_secret(&path, &t)?;
    Ok(t)
}

fn write_secret(path: &Path, value: &str) -> Result<()> {
    std::fs::write(path, value).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

// ------------------------------------------------------------------ delivery

pub async fn send_arc(st: std::sync::Arc<AppState>, alert: Alert) {
    send(&st, alert).await;
}

/// Send `alert` to every subscribed device. Drops subscriptions the push
/// service says are gone. Never blocks the caller for long.
pub async fn send(st: &AppState, alert: Alert) {
    if alert.kind != Kind::Test && !st.rate_limit_ok(&alert) {
        return;
    }
    let subs: Vec<SubscriptionInfo> = st.subscriptions.lock().map(|g| g.clone()).unwrap_or_default();
    tracing::info!("alert {:?} for {} -> {} device(s)", alert.kind, alert.session, subs.len());
    if subs.is_empty() {
        return;
    }
    let agent = st
        .sessions
        .get(&alert.session)
        .and_then(|m| m.preset.clone())
        .unwrap_or_default();
    let body = match (&alert.prompt, &alert.message) {
        (Some(p), _) => {
            let ctx = p.context.last().map(|c| format!("{c}\n")).unwrap_or_default();
            short(&format!("{ctx}{}", p.question), 200)
        }
        (None, m) => m.clone().unwrap_or_default(),
    };
    let payload = match serde_json::to_vec(&PushPayload {
        title: alert.title(&agent),
        body,
        session: &alert.session,
        prompt_id: alert.prompt.as_ref().map(|p| p.id.as_str()),
        actions: alert.actions(),
        limit: alert.kind == Kind::Limit,
        open_url: (alert.kind == Kind::Tunnel)
            .then(|| alert.message.as_deref().and_then(|m| m.rsplit(' ').next()).map(str::to_owned))
            .flatten(),
    }) {
        Ok(p) => p,
        Err(e) => return tracing::warn!("encoding push payload: {e}"),
    };

    let client = HyperWebPushClient::new();
    let mut dead = Vec::new();
    for sub in &subs {
        match tokio::time::timeout(SEND_TIMEOUT, deliver(st, &client, sub, &payload)).await {
            Ok(Ok(())) => {}
            Ok(Err(WebPushError::EndpointNotValid(_) | WebPushError::EndpointNotFound(_))) => {
                dead.push(sub.endpoint.clone());
            }
            Ok(Err(e)) => tracing::warn!("push to {}…: {e}", &sub.endpoint[..sub.endpoint.len().min(40)]),
            Err(_) => tracing::warn!("push timed out"),
        }
    }
    if !dead.is_empty() {
        if let Ok(mut g) = st.subscriptions.lock() {
            g.retain(|s| !dead.contains(&s.endpoint));
        }
        crate::sessions::save(st).await;
    }
}

async fn deliver(
    st: &AppState,
    client: &HyperWebPushClient,
    sub: &SubscriptionInfo,
    payload: &[u8],
) -> Result<(), WebPushError> {
    let mut sig = VapidSignatureBuilder::from_base64(&st.vapid_private, sub)?;
    sig.add_claim("sub", st.cfg.vapid_subject.as_str());
    let mut msg = WebPushMessageBuilder::new(sub);
    msg.set_payload(ContentEncoding::Aes128Gcm, payload);
    msg.set_vapid_signature(sig.build()?);
    msg.set_ttl(PUSH_TTL_SECS);
    msg.set_urgency(Urgency::High);
    client.send(msg.build()?).await
}

impl AppState {
    /// Per-(session, kind) throttle.
    fn rate_limit_ok(&self, a: &Alert) -> bool {
        let key = (a.session.clone(), a.kind);
        let now = Instant::now();
        let mut ok = true;
        self.last_alert
            .entry(key)
            .and_modify(|t| {
                if now.duration_since(*t) < MIN_GAP {
                    ok = false;
                } else {
                    *t = now;
                }
            })
            .or_insert(now);
        ok
    }
}

/// A hook said the agent needs input. Give the screen a moment to draw the prompt,
/// read it, and alert with the actual question and answer buttons.
pub async fn needs_input(st: std::sync::Arc<AppState>, session: String, hook_message: Option<String>) {
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let mut alert = Alert::new(&session, Kind::NeedsInput, hook_message);
    if let Ok(screen) = crate::tmux::capture(&session, 60).await {
        alert.prompt = crate::screen::analyze(&screen).prompt;
    }
    send(&st, alert).await;
}

// ------------------------------------------------------------ hook injection

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Claude Code `--settings` JSON: Notification and Stop hooks calling `ttm notify`.
pub fn claude_settings(ttm: &Path) -> String {
    let cmd = |kind: &str| format!("{} notify --kind {kind} --stdin", sh_quote(&ttm.display().to_string()));
    serde_json::json!({
        // Claude pipes its status (including plan-limit usage) here every few seconds.
        "statusLine": { "type": "command", "command": format!("{} statusline", sh_quote(&ttm.display().to_string())) },
        "hooks": {
            // SessionStart reports the conversation id as soon as it exists, so a resume works
            // even if the agent never needed input.
            "SessionStart": [{ "hooks": [{ "type": "command", "command": cmd("session") }] }],
            "Notification": [{ "hooks": [{ "type": "command", "command": cmd("needs_input") }] }],
            "Stop": [{ "hooks": [{ "type": "command", "command": cmd("done") }] }],
        }
    })
    .to_string()
}

/// Codex `-c` override. Codex appends its JSON payload as the final argument, and
/// `--kind auto` reads the event type from it (turn finished vs approval requested).
pub fn codex_notify_override(ttm: &Path) -> String {
    let argv = serde_json::json!([ttm.display().to_string(), "notify", "--kind", "auto"]);
    format!("notify={argv}")
}

/// A `PATH` shim that finds the real binary and runs it with `extra`
/// arguments prepended. It skips itself by file identity (`-ef`), not by
/// directory spelling, so odd `PATH` entries (`//`, symlinks) can't make it
/// re-exec itself forever.
///
/// `TTM_RESUME_ID` (set when a session is restored) turns the first launch into a
/// resume. The shim unsets it so agents the agent itself spawns don't also resume.
fn shim_script(name: &str, extra: &str) -> String {
    let resume = match name {
        "claude" => {
            r#"if [ "$r" = "__last__" ]; then set -- --continue "$@"; elif [ -n "$r" ]; then set -- --resume "$r" "$@"; fi"#
        }
        _ => {
            r#"if [ "$r" = "__last__" ]; then set -- resume --last "$@"; elif [ -n "$r" ]; then set -- resume "$r" "$@"; fi"#
        }
    };
    format!(
        r#"#!/bin/sh
# Generated by ttm: adds notification hooks, then runs the real {name}.
r="${{TTM_RESUME_ID:-}}"
unset TTM_RESUME_ID
{resume}
IFS=:
for d in $PATH; do
  if [ -x "$d/{name}" ] && ! [ "$d/{name}" -ef "$0" ]; then
    unset IFS
    exec "$d/{name}" {extra} "$@"
  fi
done
echo "ttm shim: {name} not found on PATH" >&2
exit 127
"#
    )
}

/// (Re)write shims into `<data_dir>/shims` and return that directory.
pub fn install_shims(data_dir: &Path, ttm: &Path) -> Result<PathBuf> {
    let dir = data_dir.join("shims");
    std::fs::create_dir_all(&dir)?;
    let shims = [
        ("claude", format!("--settings {}", sh_quote(&claude_settings(ttm)))),
        ("codex", format!("-c {}", sh_quote(&codex_notify_override(ttm)))),
    ];
    for (name, extra) in shims {
        let path = dir.join(name);
        std::fs::write(&path, shim_script(name, &extra))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(dir)
}

// ------------------------------------------------------- `ttm notify` client

#[derive(Deserialize)]
struct HookPayload {
    message: Option<String>,
    #[serde(rename = "last-assistant-message")]
    last_assistant_message: Option<String>,
    /// Claude Code.
    session_id: Option<String>,
    /// Codex.
    #[serde(rename = "thread-id")]
    thread_id: Option<String>,
    cwd: Option<String>,
    /// Codex event name: `agent-turn-complete`, `approval-requested`.
    #[serde(rename = "type")]
    event: Option<String>,
}

/// Body for `POST /api/internal/notify`.
#[derive(Serialize, Deserialize)]
pub struct NotifyBody {
    pub uid: String,
    pub kind: Kind,
    pub message: Option<String>,
    #[serde(default)]
    pub agent_session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

/// What a hook command declares; `Auto` is decided from the payload.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum HookKind {
    NeedsInput,
    Done,
    Session,
    Auto,
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_owned(),
    }
}

/// Run by agent hooks. Must never hang or fail the agent: all errors are swallowed.
pub fn run_notify_cli(kind: HookKind, stdin: bool, payload_arg: Option<String>) {
    let _ = notify_cli(kind, stdin, payload_arg);
}

fn notify_cli(kind: HookKind, stdin: bool, payload_arg: Option<String>) -> Result<()> {
    let uid = std::env::var("TTM_SESSION_UID").context("not inside a ttm session")?;
    let token = std::env::var("TTM_NOTIFY_TOKEN")?;
    let url = std::env::var("TTM_NOTIFY_URL")?;

    let mut raw = payload_arg.unwrap_or_default();
    if stdin {
        let mut buf = String::new();
        std::io::stdin().take(64 * 1024).read_to_string(&mut buf)?;
        raw = buf;
    }
    let payload: Option<HookPayload> = serde_json::from_str(&raw).ok();
    let kind = match kind {
        HookKind::NeedsInput => Kind::NeedsInput,
        HookKind::Done => Kind::Done,
        HookKind::Session => Kind::Session,
        HookKind::Auto => match payload.as_ref().and_then(|p| p.event.as_deref()) {
            Some("approval-requested") => Kind::NeedsInput,
            _ => Kind::Done,
        },
    };
    let (message, agent_session_id, cwd) = match payload {
        Some(p) => (
            p.message.or(p.last_assistant_message).map(|m| clip(&m, 160)),
            p.session_id.or(p.thread_id),
            p.cwd,
        ),
        None => (None, None, None),
    };

    let body = serde_json::to_string(&NotifyBody { uid, kind, message, agent_session_id, cwd })?;
    post_local(&url, &token, "/api/internal/notify", &body, Duration::from_secs(3))
}

/// POST `body` (JSON) to one of the server's internal endpoints, as a hook process does.
/// Plain HTTP to the local server, so no TLS stack is needed here.
pub fn post_local(url: &str, token: &str, path: &str, body: &str, timeout: Duration) -> Result<()> {
    let hostport = url.strip_prefix("http://").context("notify url must be http://")?.trim_end_matches('/');
    let mut conn = TcpStream::connect(hostport)?;
    conn.set_read_timeout(Some(timeout))?;
    conn.set_write_timeout(Some(timeout))?;
    write!(
        conn,
        "POST {path} HTTP/1.1\r\nHost: {hostport}\r\nx-ttm-token: {token}\r\n\
         content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )?;
    let mut resp = String::new();
    let _ = conn.read_to_string(&mut resp);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_settings_shape() {
        let v: serde_json::Value = serde_json::from_str(&claude_settings(Path::new("/opt/it's/ttm"))).unwrap();
        let cmd = v["hooks"]["Notification"][0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, r"'/opt/it'\''s/ttm' notify --kind needs_input --stdin");
        assert!(v["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap().contains("--kind done"));
        assert!(v["hooks"]["SessionStart"][0]["hooks"][0]["command"].as_str().unwrap().contains("--kind session"));
    }

    #[test]
    fn codex_override_is_toml_array() {
        let s = codex_notify_override(Path::new("/usr/bin/ttm"));
        assert_eq!(s, r#"notify=["/usr/bin/ttm","notify","--kind","auto"]"#);
        let parsed: toml::Table = toml::from_str(&s).unwrap();
        assert_eq!(parsed["notify"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn vapid_key_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ttm-vapid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let k1 = load_or_create_vapid(&dir).unwrap();
        let k2 = load_or_create_vapid(&dir).unwrap();
        assert_eq!(k1, k2);
        let public = B64.decode(public_key(&k1).unwrap()).unwrap();
        assert_eq!(public.len(), 65); // uncompressed P-256 point
        assert_eq!(public[0], 4);
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn shim_injects_args_and_skips_itself() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("ttm-shim-{}", std::process::id()));
        let fake = root.join("fake");
        std::fs::create_dir_all(&fake).unwrap();
        std::fs::write(fake.join("claude"), "#!/bin/sh\nprintf '%s|' \"$@\"\n").unwrap();
        std::fs::set_permissions(fake.join("claude"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let shims = install_shims(&root, Path::new("/opt/ttm")).unwrap();

        // A PATH with a doubled slash (as TMPDIR often produces) must not fool the shim.
        let path = format!("{}/{}:{}", shims.display(), "", fake.display());
        let out = std::process::Command::new(shims.join("claude"))
            .args(["hello", "world"])
            .env("PATH", path)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.starts_with("--settings|{"), "got {text:?}");
        assert!(text.ends_with("|hello|world|"), "got {text:?}");
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn shim_turns_resume_env_into_flags_and_does_not_leak_it() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("ttm-shim-resume-{}", std::process::id()));
        let fake = root.join("fake");
        std::fs::create_dir_all(&fake).unwrap();
        for name in ["claude", "codex"] {
            // Print the args and whether the resume variable is still visible to the real binary.
            let body = "#!/bin/sh\nprintf '%s|' \"$@\"\nprintf 'leak=%s' \"$TTM_RESUME_ID\"\n";
            std::fs::write(fake.join(name), body).unwrap();
            std::fs::set_permissions(fake.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let shims = install_shims(&root, Path::new("/opt/ttm")).unwrap();
        let path = format!("{}:{}", shims.display(), fake.display());
        let run = |name: &str, resume: Option<&str>| {
            let mut c = std::process::Command::new(shims.join(name));
            c.arg("go").env("PATH", &path).env_remove("TTM_RESUME_ID");
            if let Some(r) = resume {
                c.env("TTM_RESUME_ID", r);
            }
            String::from_utf8_lossy(&c.output().unwrap().stdout).into_owned()
        };
        let a = run("claude", Some("abc-1"));
        assert!(a.contains("|--resume|abc-1|go|") && a.ends_with("leak="), "{a}");
        assert!(run("claude", Some("__last__")).contains("|--continue|go|"));
        assert!(!run("claude", None).contains("--resume"));
        let c = run("codex", Some("thr-9"));
        assert!(c.contains("|resume|thr-9|go|") && c.ends_with("leak="), "{c}");
        assert!(run("codex", Some("__last__")).contains("|resume|--last|go|"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn needs_input_alert_offers_approve_and_deny_buttons() {
        let screen = "Do you want to proceed?\n❯ 1. Yes\n  2. Yes, and don't ask again\n  3. No, and tell Claude what to do differently (esc)\n";
        let mut alert = Alert::new("agent", Kind::NeedsInput, None);
        alert.prompt = crate::screen::analyze(screen).prompt;
        let actions = alert.actions();
        assert_eq!(actions.len(), 2);
        assert_eq!((actions[0].action.as_str(), actions[0].title.as_str()), ("choice:1", "Yes"));
        assert_eq!(actions[1].action, "choice:3");
        assert!(actions[1].title.ends_with('…') || actions[1].title.starts_with("No"));
        // Only needs-input alerts carry buttons.
        alert.kind = Kind::Done;
        assert!(alert.actions().is_empty());
        // A prompt whose approve and deny are the same choice yields one button, not two.
        let mut one = Alert::new("agent", Kind::NeedsInput, None);
        one.prompt = crate::screen::analyze("Continue?\n❯ 1. Yes\n  2. Yes always\n").prompt;
        assert_eq!(one.actions().iter().filter(|a| a.action == "choice:1").count(), 1);
    }

    #[test]
    fn clip_is_char_safe() {
        assert_eq!(clip("héllo wörld", 5), "héllo…");
        assert_eq!(clip("short", 10), "short");
    }
}
