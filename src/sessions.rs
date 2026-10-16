//! Session registry. tmux decides which sessions are *running*; this remembers
//! everything else: how each was started (so it can be restored after a reboot),
//! the agent's own conversation id (so it can be resumed), notes, restart counts.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    config::{Agent, Preset, RestartPolicy},
    limits,
    notify::{self, Alert, Kind},
    ops,
    openrouter::launch::{self as orl, Driver, OrSpec},
    screen::Activity,
    scrollback, tmux,
};

const RECONCILE_EVERY: Duration = Duration::from_secs(2);
/// A session that stays up this long is considered healthy again.
const HEALTHY_AFTER: Duration = Duration::from_secs(300);
/// Passed as the resume id when an agent was started but we never learned its conversation id.
pub const RESUME_LAST: &str = "__last__";

/// Everything needed to start a session again.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Spec {
    /// Run through `sh -c`; `None` means the default shell.
    pub command: Option<String>,
    pub dir: Option<PathBuf>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub agent: Option<Agent>,
    /// Started with the agent's "skip all permission checks" options (see `apply_yolo`).
    #[serde(default)]
    pub yolo: bool,
    /// Runs on an OpenRouter model through ttm's proxy. The model flags are added when it starts,
    /// from the session's lease, so a model switched mid-session stays switched after a restore.
    #[serde(default)]
    pub openrouter: Option<OrSpec>,
}

/// Options that make Claude Code or Codex skip every permission check. They are only ever applied
/// when asked for, per session (or by `yolo = true` on a preset).
const CLAUDE_YOLO_FLAG: &str = "--dangerously-skip-permissions";
const CODEX_YOLO_FLAG: &str = "--yolo";

/// `cmd` with `flag` added, unless it is already there. Added at the end of the command line.
fn with_flag(cmd: &str, flag: &str) -> String {
    if cmd.split_whitespace().any(|w| w == flag) { cmd.to_owned() } else { format!("{} {flag}", cmd.trim_end()) }
}

/// `cmd` without Codex's own approval and sandbox options, which `--yolo` replaces and rejects.
fn without_codex_safety_flags(cmd: &str) -> String {
    const WITH_VALUE: &[&str] = &["-a", "--ask-for-approval", "-s", "--sandbox"];
    let mut out: Vec<&str> = Vec::new();
    let mut words = cmd.split_whitespace();
    while let Some(w) = words.next() {
        if WITH_VALUE.contains(&w) {
            words.next(); // its value
        } else if w == "--full-auto" || WITH_VALUE.iter().any(|f| w.starts_with(&format!("{f}="))) {
            // dropped
        } else {
            out.push(w);
        }
    }
    out.join(" ")
}

/// Turn `spec` into a "skip all permission checks" launch for its agent. Other agents are left alone.
pub fn apply_yolo(spec: &mut Spec) {
    let Some(cmd) = spec.command.clone() else { return };
    match spec.agent {
        Some(Agent::Claude) => {
            spec.env.insert("IS_SANDBOX".into(), "1".into());
            spec.command = Some(with_flag(&cmd, CLAUDE_YOLO_FLAG));
            spec.yolo = true;
        }
        Some(Agent::Codex) => {
            spec.command = Some(with_flag(&without_codex_safety_flags(&cmd), CODEX_YOLO_FLAG));
            spec.yolo = true;
        }
        _ => {}
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub preset: Option<String>,
    /// Stable id handed to hooks via env; survives renames.
    #[serde(default)]
    pub uid: String,
    pub restarts: u32,
    #[serde(default)]
    pub spec: Spec,
    /// The agent's own conversation id (Claude `session_id`, Codex `thread-id`), learned from hooks.
    #[serde(default)]
    pub agent_session_id: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub created: u64,
    /// The folder the session was started in. It decides which project the web list files the session
    /// under, and unlike `spec.dir` it does not follow a shell that `cd`s elsewhere.
    #[serde(default)]
    pub project: Option<PathBuf>,
    #[serde(default)]
    pub last_exit: Option<u32>,
    #[serde(skip)]
    pub restart_at: Option<Instant>,
    #[serde(skip)]
    pub last_restart: Option<Instant>,
    /// Idle-alert tracking: last seen tmux activity time, and whether output
    /// happened since the last alert.
    #[serde(skip)]
    pub last_activity: u64,
    #[serde(skip)]
    pub armed: bool,
    #[serde(skip)]
    pub exit_notified: bool,
    /// Whether this daemon run has seen the session running. Sessions stopped before the
    /// daemon started are never forgotten automatically.
    #[serde(skip)]
    pub seen_live: bool,
    /// Scrollback snapshot tracking.
    #[serde(skip)]
    pub snap_activity: u64,
    #[serde(skip)]
    pub snap_dead: bool,
    #[serde(skip)]
    pub snap_at: Option<Instant>,
}

impl SessionMeta {
    pub fn agent(&self) -> Option<Agent> {
        self.spec.agent.filter(|a| *a != Agent::Other)
    }

    /// What to hand the shim so the agent resumes instead of starting fresh.
    pub fn resume_value(&self) -> Option<String> {
        self.agent()?;
        Some(self.agent_session_id.clone().unwrap_or_else(|| RESUME_LAST.into()))
    }
}

pub type Registry = DashMap<String, SessionMeta>;

fn state_file(data_dir: &Path) -> PathBuf {
    data_dir.join("state.json")
}

#[derive(Default, Serialize, Deserialize)]
struct StateFile {
    #[serde(default)]
    sessions: HashMap<String, SessionMeta>,
    #[serde(default)]
    subscriptions: Vec<web_push::SubscriptionInfo>,
}

pub fn new_uid() -> String {
    let mut b = [0u8; 8];
    rand::fill(&mut b[..]);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn load(data_dir: &Path) -> (Registry, Vec<web_push::SubscriptionInfo>) {
    let mut f: StateFile = std::fs::read(state_file(data_dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    for m in f.sessions.values_mut() {
        if m.uid.is_empty() {
            m.uid = new_uid();
        }
    }
    (f.sessions.into_iter().collect(), f.subscriptions)
}

/// Atomic write: temp file in the same directory, then rename.
pub async fn save(st: &AppState) {
    let snapshot = StateFile {
        sessions: st.sessions.iter().map(|e| (e.key().clone(), e.value().clone())).collect(),
        subscriptions: st.subscriptions.lock().map(|g| g.clone()).unwrap_or_default(),
    };
    let path = state_file(&st.data_dir);
    let tmp = path.with_extension("json.tmp");
    let res: Result<()> = async {
        tokio::fs::write(&tmp, serde_json::to_vec_pretty(&snapshot)?).await?;
        // Holds push-subscription keys: owner-only, set before the rename makes it visible.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).await?;
        }
        tokio::fs::rename(&tmp, &path).await?;
        Ok(())
    }
    .await;
    if let Err(e) = res {
        tracing::warn!("saving state: {e:#}");
    }
}

/// Current name of the session that owns hook uid `uid`.
pub fn name_for_uid(st: &AppState, uid: &str) -> Option<String> {
    if uid.is_empty() {
        return None;
    }
    st.sessions.iter().find(|e| e.value().uid == uid).map(|e| e.key().clone())
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Agent conversation ids end up in argv and on disk: keep them boring.
pub fn valid_agent_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

// ---------------------------------------------------------------- listing

#[derive(Serialize)]
pub struct SessionView {
    pub id: String,
    pub preset: Option<String>,
    /// "running", "exited" (process ended, pane kept) or "stopped" (tmux session gone).
    pub status: &'static str,
    pub exit_code: Option<u32>,
    pub idle_secs: u64,
    pub attached: u32,
    pub created: u64,
    pub command: String,
    pub cols: u32,
    pub rows: u32,
    pub restarts: u32,
    pub restart_in_secs: Option<u64>,
    pub agent: Option<Agent>,
    /// A Claude/Codex session that can be resumed.
    pub resumable: bool,
    /// Whether we have learned the agent's conversation id.
    pub has_agent_session: bool,
    pub group: Option<String>,
    pub note: Option<String>,
    pub tags: Vec<String>,
    pub pinned: bool,
    /// Started with permission checks skipped (Claude: `--dangerously-skip-permissions`; Codex: `--yolo`).
    pub yolo: bool,
    /// The working folder: where the pane is now while it runs, where it was started otherwise.
    pub cwd: Option<String>,
    /// The folder it was started in: its project in the web list. `None` for sessions not seen running yet.
    pub project: Option<String>,
    /// The git branch checked out in `cwd`.
    pub branch: Option<String>,
    /// What a running Claude or Codex session is doing. Filled in by `enrich`, for the web list.
    pub activity: Option<Activity>,
    /// One line of the latest message of an agent session. Filled in by `enrich`.
    pub preview: Option<String>,
    /// How many helper agents the session has running. Filled in by `enrich`.
    pub running_subagents: u32,
    /// The proxy the session's agent goes through (`"teamclaude"`), if it does. Filled in by `enrich`.
    pub routed_via: Option<&'static str>,
    /// The OpenRouter model, CLI and spend, when the session runs on OpenRouter.
    pub openrouter: Option<serde_json::Value>,
}

pub async fn snapshot(st: &AppState) -> Result<Vec<SessionView>> {
    let now = now_secs();
    let live = tmux::list().await?;
    let home = crate::agentlog::home();
    let mut out: Vec<SessionView> = Vec::new();
    for s in &live {
        let meta = st.sessions.get(&s.id);
        let m = meta.as_deref();
        out.push(SessionView {
            preset: m.and_then(|m| m.preset.clone()),
            restarts: m.map_or(0, |m| m.restarts),
            restart_in_secs: m
                .and_then(|m| m.restart_at)
                .map(|t| t.saturating_duration_since(Instant::now()).as_secs()),
            status: if s.dead { "exited" } else { "running" },
            exit_code: s.exit_code,
            idle_secs: now.saturating_sub(s.activity),
            attached: s.attached,
            created: s.created,
            command: s.command.clone(),
            cols: s.width,
            rows: s.height,
            agent: m.and_then(SessionMeta::agent),
            resumable: m.is_some_and(|m| m.agent().is_some()),
            has_agent_session: m.is_some_and(|m| m.agent_session_id.is_some()),
            group: m.and_then(|m| m.group.clone()),
            note: m.and_then(|m| m.note.clone()),
            tags: m.map(|m| m.tags.clone()).unwrap_or_default(),
            pinned: m.is_some_and(|m| m.pinned),
            yolo: m.is_some_and(|m| m.spec.yolo),
            cwd: Some(s.cwd.clone()).filter(|c| !c.is_empty()),
            project: m.and_then(|m| m.project.as_deref()).map(|d| expand_home(d, home.as_deref()).display().to_string()),
            branch: None,
            activity: None,
            preview: None,
            running_subagents: 0,
            routed_via: None,
            openrouter: m.and_then(|m| m.spec.openrouter.as_ref()).map(|or| orl::summary(or, st.leases.by_tag(&s.id).map(|(_, l)| l).as_ref())),
            id: s.id.clone(),
        });
    }
    // Remembered sessions whose tmux session is gone.
    let stopped: Vec<(String, SessionMeta)> = st
        .sessions
        .iter()
        .filter(|e| !live.iter().any(|s| &s.id == e.key()))
        .map(|e| (e.key().clone(), e.value().clone()))
        .collect();
    for (id, m) in stopped {
        let saved = scrollback::modified(st, &m.uid).await;
        out.push(SessionView {
            status: "stopped",
            exit_code: m.last_exit,
            idle_secs: saved.map_or(0, |t| now.saturating_sub(t)),
            attached: 0,
            created: m.created,
            command: m.spec.command.clone().unwrap_or_default(),
            cols: 0,
            rows: 0,
            restarts: m.restarts,
            restart_in_secs: None,
            agent: m.agent(),
            resumable: m.agent().is_some(),
            has_agent_session: m.agent_session_id.is_some(),
            preset: m.preset,
            group: m.group,
            note: m.note,
            tags: m.tags,
            pinned: m.pinned,
            yolo: m.spec.yolo,
            cwd: m.spec.dir.as_deref().map(|d| expand_home(d, home.as_deref()).display().to_string()),
            project: m.project.as_deref().map(|d| expand_home(d, home.as_deref()).display().to_string()),
            branch: None,
            activity: None,
            preview: None,
            running_subagents: 0,
            routed_via: None,
            openrouter: m.spec.openrouter.as_ref().map(|or| orl::summary(or, st.leases.by_tag(&id).map(|(_, l)| l).as_ref())),
            id,
        });
    }
    let mut dirs: Vec<PathBuf> = out.iter().filter_map(|v| v.cwd.as_deref().map(PathBuf::from)).collect();
    dirs.sort();
    dirs.dedup();
    let branches = st.branches.get_all(dirs).await;
    for v in &mut out {
        v.branch = v.cwd.as_deref().and_then(|c| branches.get(Path::new(c)).cloned().flatten());
    }
    out.sort_by(|a, b| {
        b.pinned.cmp(&a.pinned).then(b.created.cmp(&a.created)).then_with(|| a.id.cmp(&b.id))
    });
    Ok(out)
}

/// Add what the web list shows but the CLI has no use for: what each running agent is doing, and
/// the latest message of every agent session. One pane capture and one incremental log read each.
pub async fn enrich(st: &Arc<AppState>, views: &mut [SessionView]) {
    let mut tasks = tokio::task::JoinSet::new();
    for (i, v) in views.iter().enumerate() {
        if !matches!(v.agent, Some(Agent::Claude | Agent::Codex)) {
            continue;
        }
        let (st, id, running) = (st.clone(), v.id.clone(), v.status == "running");
        let (agent, cwd) = (v.agent, v.cwd.clone());
        tasks.spawn(async move {
            let activity = if running {
                ops::screen(&id).await.ok().map(|a| if a.prompt.is_some() { Activity::Waiting } else { a.activity })
            } else {
                None
            };
            let (preview, running) = crate::chat::overview(&st, &id).await;
            let routed = match (agent, cwd) {
                _ if runs_on_openrouter(&st, &id) => None,
                (Some(a), Some(c)) => crate::teamclaude::detect(&st, a, Path::new(&c)).await.map(|_| "teamclaude"),
                _ => None,
            };
            (i, activity, preview, running, routed)
        });
    }
    while let Some(done) = tasks.join_next().await {
        if let Ok((i, activity, preview, running, routed)) = done {
            views[i].activity = activity;
            views[i].preview = preview;
            views[i].running_subagents = running;
            views[i].routed_via = routed;
        }
    }
}

/// Whether the session's agent talks to OpenRouter through ttm's proxy. Such a session is not routed
/// by anything configured around it, such as a Team Claude `ANTHROPIC_BASE_URL` in your settings.
pub fn runs_on_openrouter(st: &AppState, id: &str) -> bool {
    st.sessions.get(id).is_some_and(|m| m.spec.openrouter.is_some())
}

/// What the details sidebar needs to know about one session's pane.
pub struct PaneContext {
    /// Where the pane is now while it runs, where it was started otherwise.
    pub cwd: PathBuf,
    pub agent: Option<Agent>,
    /// The agent's own conversation id, once a hook has reported it.
    pub conversation: Option<String>,
}

pub async fn pane_context(st: &AppState, id: &str) -> Option<PaneContext> {
    let meta = st.sessions.get(id).map(|m| m.clone());
    let live = tmux::list().await.ok()?.into_iter().find(|s| s.id == id).map(|s| s.cwd).filter(|c| !c.is_empty());
    let home = crate::agentlog::home();
    let cwd = live
        .map(PathBuf::from)
        .or_else(|| meta.as_ref()?.spec.dir.as_deref().map(|d| expand_home(d, home.as_deref())))?;
    Some(PaneContext {
        cwd,
        agent: meta.as_ref().and_then(SessionMeta::agent),
        conversation: meta.and_then(|m| m.agent_session_id).filter(|c| valid_agent_id(c)),
    })
}

// -------------------------------------------------------------- lifecycle

/// First free name derived from `base`: `base`, `base-2`, `base-3`, …
fn unique_name(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == base) {
        return base.to_owned();
    }
    (2u32..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken.contains(c))
        .expect("an unbounded range always yields a free name")
}

fn spec_for(preset: &Preset, yolo: bool) -> Spec {
    let mut spec = Spec {
        command: Some(preset.command.clone()),
        dir: preset.dir.clone(),
        env: preset.env.clone(),
        agent: Some(preset.agent()),
        yolo: false,
        openrouter: None,
    };
    if yolo || preset.yolo {
        apply_yolo(&mut spec);
    }
    spec
}

/// Start `spec` as tmux session `id`. `resume` is handed to the claude/codex shims.
async fn launch(
    st: &AppState,
    id: &str,
    uid: &str,
    spec: &Spec,
    preset: Option<&Preset>,
    resume: Option<&str>,
) -> Result<()> {
    let mut env: Vec<(String, String)> = vec![
        ("TTM_SESSION_UID".into(), uid.to_owned()),
        ("TTM_NOTIFY_URL".into(), st.cfg.notify_url()),
        ("TTM_NOTIFY_TOKEN".into(), st.notify_token.clone()),
    ];
    // A service has no locale; without one the agent's TUI and your shell can't draw Unicode.
    if ["LC_ALL", "LC_CTYPE", "LANG"].iter().all(|k| std::env::var_os(k).is_none()) {
        env.push(("LANG".into(), "en_US.UTF-8".into()));
    }
    // No folder given: your home, not the service's working directory (`/` under launchd).
    let home = crate::agentlog::home();
    let dir = spec.dir.as_deref().filter(|d| d.is_dir()).or(home.as_deref());
    let routed = match &spec.openrouter {
        Some(or) => Some(or_flags(st, id, uid, or)?),
        None => None,
    };
    match &spec.command {
        Some(cmd) => {
            let cmd = &match &routed {
                Some(f) => format!("{cmd}{}", f.args),
                None => cmd.clone(),
            };
            env.extend(routed.iter().flat_map(|f| f.env.clone()));
            // tmux ignores `-e PATH=…` for the pane process, so set PATH via `env`. The
            // shims then add notification hooks whenever `claude` / `codex` runs, and
            // consume TTM_RESUME_ID so only the first agent started resumes.
            let path = format!("{}:{}", st.shim_dir.display(), std::env::var("PATH").unwrap_or_default());
            let mut argv: Vec<String> = vec!["env".into(), format!("PATH={path}")];
            if let Some(r) = resume {
                argv.push(format!("TTM_RESUME_ID={r}"));
            }
            // Unless the preset restarts itself, leave you in a shell when the command ends
            // (Ctrl+C out of Claude, say) instead of a dead pane you can't type into.
            let restarts = preset.is_some_and(|p| p.restart == RestartPolicy::OnFailure);
            if restarts {
                argv.extend(["sh".into(), "-c".into(), cmd.clone()]);
            } else {
                argv.extend(shell_after(cmd));
            }
            if let Some(p) = preset {
                argv = limits::wrap(p, argv);
            }
            env.extend(spec.env.iter().map(|(k, v)| (k.clone(), v.clone())));
            tmux::create(id, dir, &env, &argv, true).await
        }
        // Plain shells: your shell's rc files rearrange PATH and define aliases, so
        // shims can't be relied on here. Use a preset for hook-based alerts.
        None => tmux::create(id, dir, &env, &[], false).await,
    }
}

/// The flags and environment that point an agent at OpenRouter. The lease is the session's own:
/// found again after a restore (spend and the model it was switched to are kept), made if gone.
fn or_flags(st: &AppState, id: &str, uid: &str, or: &OrSpec) -> Result<orl::Flags> {
    let (token, model) = match st.leases.by_tag(id) {
        Some((token, lease)) => (token, lease.model),
        None => (st.leases.mint(&or.account, &or.model, id, or.cap_usd), or.model.clone()),
    };
    anyhow::ensure!(orl::valid_model(&model), "{model:?} is not a usable model id");
    let origin = orl::origin(st.cfg.bind.port());
    Ok(match or.driver {
        Driver::Claude => {
            let exe = std::env::current_exe().context("locating the ttm executable")?;
            let path = orl::write_claude_settings(&st.data_dir, uid, &notify::claude_settings(&exe), &origin, &token)?;
            orl::claude_flags(&path, &model)
        }
        Driver::Codex => orl::codex_flags(&origin, &token, &model),
    })
}

/// `sh -c <script> <cmd>`: run `cmd`, then replace the wrapper with your shell. The trap keeps a
/// Ctrl+C that reaches the wrapper from ending it (a handler, not "ignore", so `cmd` still gets
/// the default action back when it is exec'd).
fn shell_after(cmd: &str) -> Vec<String> {
    const SCRIPT: &str = r#"trap : INT; sh -c "$0"; s=$?; printf '\n[ttm] command exited (status %s). This is a shell now.\n' "$s"; exec "${SHELL:-/bin/sh}""#;
    vec!["sh".into(), "-c".into(), SCRIPT.into(), cmd.into()]
}

pub struct NewSession<'a> {
    pub preset: Option<&'a str>,
    pub name: Option<&'a str>,
    pub group: Option<&'a str>,
    /// Use `name` as given instead of making it unique.
    pub exact: bool,
    /// Start Claude or Codex with permission checks skipped. Ignored for other presets.
    pub yolo: bool,
    /// Start in this folder instead of the preset's. Must exist.
    pub dir: Option<PathBuf>,
}

/// Folders worth launching an agent in: where your tmux panes are now, then where saved sessions
/// were started. Most recently active first, only folders that still exist.
pub async fn recent_dirs(st: &AppState, limit: usize) -> Vec<RecentDir> {
    let mut found: Vec<(u64, PathBuf)> = Vec::new();
    if let Ok(live) = tmux::list().await {
        found.extend(live.into_iter().filter(|s| !s.cwd.is_empty()).map(|s| (s.activity, PathBuf::from(s.cwd))));
    }
    for e in st.sessions.iter() {
        if let Some(d) = &e.value().spec.dir {
            found.push((e.value().created, d.clone()));
        }
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    let home = crate::agentlog::home();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (_, d) in found {
        let d = expand_home(&d, home.as_deref());
        if !d.is_dir() || d.parent().is_none() || !seen.insert(d.clone()) {
            continue;
        }
        let name = d.file_name().map_or_else(|| "/".to_owned(), |n| n.to_string_lossy().into_owned());
        let short = match home.as_deref().and_then(|h| d.strip_prefix(h).ok()) {
            Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Some(rest) => format!("~/{}", rest.display()),
            None => d.display().to_string(),
        };
        out.push(RecentDir { path: d.display().to_string(), name, short });
        if out.len() >= limit {
            break;
        }
    }
    out
}

#[derive(Serialize)]
pub struct RecentDir {
    pub path: String,
    /// The last part, for a title.
    pub name: String,
    /// The path with your home folder as `~`.
    pub short: String,
}

fn expand_home(d: &std::path::Path, home: Option<&std::path::Path>) -> PathBuf {
    match (d.strip_prefix("~"), home) {
        (Ok(rest), Some(h)) => h.join(rest),
        _ => d.to_path_buf(),
    }
}

/// A session name for an agent started in a folder: `claude-ttmassive`.
fn name_for_dir(preset: &str, dir: &std::path::Path) -> Option<String> {
    let tail: String = dir
        .file_name()?
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let name = format!("{preset}-{}", tail.trim_matches('-'));
    (tmux::valid_session_id(&name) && name.len() <= 40).then_some(name)
}

/// Start a new detached session, from a preset or as a plain shell.
pub async fn create(st: &AppState, req: NewSession<'_>) -> Result<String> {
    create_resuming(st, req, None).await
}

/// Like `create`, but a Claude or Codex agent picks up the conversation `resume` instead of starting fresh.
pub async fn create_resuming(st: &AppState, req: NewSession<'_>, resume: Option<&str>) -> Result<String> {
    let preset: Option<&Preset> = match req.preset {
        Some(p) => Some(
            st.cfg.presets.iter().find(|x| x.name == p).with_context(|| format!("unknown preset {p:?}"))?,
        ),
        None => None,
    };
    let dir_name = match (req.dir.as_deref(), preset, req.name) {
        (Some(d), Some(p), None) => name_for_dir(&p.name, d),
        _ => None,
    };
    let base = req.name.or(dir_name.as_deref()).or(preset.map(|p| p.name.as_str())).unwrap_or("shell");
    anyhow::ensure!(tmux::valid_session_id(base), "invalid session name");
    let taken: Vec<String> = tmux::list()
        .await?
        .into_iter()
        .map(|s| s.id)
        .chain(st.sessions.iter().map(|e| e.key().clone()))
        .collect();
    let id = if req.exact {
        anyhow::ensure!(!taken.iter().any(|t| t == base), "a session named {base:?} already exists");
        base.to_owned()
    } else {
        unique_name(base, &taken)
    };

    let uid = new_uid();
    let mut spec = preset.map_or_else(Spec::default, |p| spec_for(p, req.yolo));
    if let Some(d) = req.dir {
        anyhow::ensure!(d.is_dir(), "{} is not a folder", d.display());
        spec.dir = Some(d);
    }
    register(st, id, uid, spec, preset, req.group, resume).await
}

/// Start `spec` as session `id` and remember it.
async fn register(st: &AppState, id: String, uid: String, spec: Spec, preset: Option<&Preset>, group: Option<&str>, resume: Option<&str>) -> Result<String> {
    if let Err(e) = launch(st, &id, &uid, &spec, preset, resume).await {
        // The lease was made for a session that never started.
        if spec.openrouter.is_some() {
            st.leases.revoke_tag(&id);
            orl::remove_claude_settings(&st.data_dir, &uid);
        }
        return Err(e);
    }
    let project = spec.dir.clone();
    st.sessions.insert(
        id.clone(),
        SessionMeta {
            preset: preset.map(|p| p.name.clone()),
            uid,
            project,
            spec,
            group: group.map(str::to_owned),
            created: now_secs(),
            // So a later restore carries on the same conversation.
            agent_session_id: resume.map(str::to_owned),
            ..Default::default()
        },
    );
    save(st).await;
    Ok(id)
}

/// A session that runs Claude Code or Codex on an OpenRouter model.
pub struct OrNew<'a> {
    pub account: &'a str,
    pub model: &'a str,
    /// The CLI to run it with; `None` picks one from the model (see `orl::driver_for`).
    pub driver: Option<Driver>,
    pub cap_usd: Option<f64>,
    pub name: Option<&'a str>,
    pub dir: Option<PathBuf>,
    pub yolo: bool,
    /// Carry on this conversation of the agent instead of starting a new one.
    pub resume: Option<&'a str>,
}

pub async fn create_openrouter(st: &AppState, req: OrNew<'_>) -> Result<String> {
    anyhow::ensure!(orl::valid_model(req.model), "that is not a usable model id");
    if let Some(c) = req.cap_usd {
        anyhow::ensure!(c.is_finite() && c > 0.0 && c < 1.0e6, "the dollar cap must be more than zero");
    }
    let accounts = crate::secrets::load(&st.data_dir).await;
    let account = accounts.get(req.account).context("no such account")?;
    anyhow::ensure!(account.kind == crate::secrets::Kind::Openrouter, "that account is not an OpenRouter account");
    anyhow::ensure!(!account.key.is_empty(), "the account has no key");

    let driver = req.driver.unwrap_or_else(|| orl::driver_for_with(req.model, &accounts.drivers));
    let base = match (req.name, req.dir.as_deref()) {
        (Some(n), _) => n.to_owned(),
        (None, Some(d)) => name_for_dir("or", d).unwrap_or_else(|| "or".into()),
        (None, None) => "or".into(),
    };
    anyhow::ensure!(tmux::valid_session_id(&base), "invalid session name");
    let taken: Vec<String> = tmux::list().await?.into_iter().map(|s| s.id).chain(st.sessions.iter().map(|e| e.key().clone())).collect();
    let id = unique_name(&base, &taken);

    let command = driver.command().to_owned();
    let mut spec = Spec {
        agent: Some(Agent::infer(&command)),
        command: Some(command),
        openrouter: Some(OrSpec { account: account.id.clone(), model: req.model.to_owned(), driver, cap_usd: req.cap_usd }),
        ..Default::default()
    };
    if req.yolo {
        apply_yolo(&mut spec);
    }
    if let Some(d) = req.dir {
        anyhow::ensure!(d.is_dir(), "{} is not a folder", d.display());
        spec.dir = Some(d);
    }
    register(st, id, new_uid(), spec, None, None, req.resume).await
}

/// Switch the model of an OpenRouter session, from its next request on. Remembered, so a restore keeps it.
pub async fn set_or_model(st: &AppState, id: &str, model: &str) -> Result<()> {
    anyhow::ensure!(orl::valid_model(model), "that is not a usable model id");
    {
        let mut m = st.sessions.get_mut(id).with_context(|| format!("no such session: {id}"))?;
        let or = m.spec.openrouter.as_mut().context("that session does not run on OpenRouter")?;
        or.model = model.to_owned();
    }
    anyhow::ensure!(st.leases.set_model(id, model), "the session has no lease");
    save(st).await;
    Ok(())
}

/// Bring a stopped (or exited) session back. Claude/Codex sessions resume their
/// conversation unless `fresh` is set.
pub async fn restore(st: &AppState, id: &str, fresh: bool) -> Result<()> {
    let (uid, spec, preset_name, resume) = {
        let m = st.sessions.get(id).with_context(|| format!("no such session: {id}"))?;
        (m.uid.clone(), m.spec.clone(), m.preset.clone(), if fresh { None } else { m.resume_value() })
    };
    // An exited agent keeps its dead pane; snapshot it, then replace it.
    if tmux::exists(id).await {
        scrollback::snapshot(st, id, &uid).await;
        tmux::kill(id).await?;
    }
    let preset = preset_name.as_deref().and_then(|n| st.cfg.presets.iter().find(|p| p.name == n));
    launch(st, id, &uid, &spec, preset, resume.as_deref()).await?;
    if let Some(mut m) = st.sessions.get_mut(id) {
        m.restarts = 0;
        m.restart_at = None;
        m.exit_notified = false;
        m.last_exit = None;
    }
    save(st).await;
    tracing::info!("restored {id}{}", if resume.is_some() { " (resuming)" } else { "" });
    Ok(())
}

/// Remove a session and everything remembered about it.
pub async fn kill(st: &AppState, id: &str) -> Result<()> {
    anyhow::ensure!(st.sessions.contains_key(id) || tmux::exists(id).await, "no such session: {id}");
    if tmux::exists(id).await {
        tmux::kill(id).await?;
    }
    if let Some((_, m)) = st.sessions.remove(id) {
        scrollback::remove(st, &m.uid).await;
        if m.spec.openrouter.is_some() {
            orl::remove_claude_settings(&st.data_dir, &m.uid);
        }
    }
    st.leases.revoke_tag(id);
    save(st).await;
    Ok(())
}

pub async fn rename(st: &AppState, id: &str, new: &str) -> Result<()> {
    anyhow::ensure!(tmux::valid_session_id(new), "invalid session name");
    let taken = tmux::exists(new).await || st.sessions.contains_key(new);
    anyhow::ensure!(!taken, "a session named {new:?} already exists");
    anyhow::ensure!(st.sessions.contains_key(id) || tmux::exists(id).await, "no such session: {id}");
    if tmux::exists(id).await {
        tmux::rename(id, new).await?;
    }
    if let Some((_, meta)) = st.sessions.remove(id) {
        st.sessions.insert(new.to_owned(), meta);
    }
    st.leases.rename_tag(id, new);
    save(st).await;
    Ok(())
}

// ------------------------------------------------------------- reconcile

/// Background loop: adopt new sessions, keep stopped ones, apply restart
/// policies, raise idle/exit alerts and snapshot scrollback.
pub async fn reconcile_loop(st: Arc<AppState>) {
    let mut tick = tokio::time::interval(RECONCILE_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut first = true;
    loop {
        tick.tick().await;
        match tmux::list().await {
            Ok(live) => {
                reconcile_once(&st, &live, first).await;
                if first {
                    autostart(&st, &live).await;
                    first = false;
                }
            }
            Err(e) => tracing::debug!("reconcile: {e:#}"),
        }
    }
}

/// Presets marked `autostart` get a session when the daemon comes up: restored
/// (resuming) if one was stopped, created if none exists.
async fn autostart(st: &Arc<AppState>, live: &[tmux::SessionInfo]) {
    for preset in st.cfg.presets.iter().filter(|p| p.autostart) {
        let existing: Option<String> = st
            .sessions
            .iter()
            .find(|e| e.value().preset.as_deref() == Some(preset.name.as_str()))
            .map(|e| e.key().clone());
        let res = match existing {
            Some(id) if live.iter().any(|s| s.id == id) => continue,
            Some(id) => restore(st, &id, false).await,
            None => create(
                st,
                NewSession { preset: Some(&preset.name), name: None, group: None, exact: false, yolo: false, dir: None },
            )
            .await
            .map(|_| ()),
        };
        if let Err(e) = res {
            tracing::warn!("autostart {}: {e:#}", preset.name);
        }
    }
}

async fn reconcile_once(st: &Arc<AppState>, live: &[tmux::SessionInfo], first: bool) {
    let mut dirty = false;
    let mut alerts: Vec<Alert> = Vec::new();
    let mut snaps: Vec<(String, String)> = Vec::new();
    let now = now_secs();

    // Sessions we didn't start (e.g. made with tmux directly) are adopted so they can be restored.
    for s in live {
        if !st.sessions.contains_key(&s.id) {
            st.sessions.insert(
                s.id.clone(),
                SessionMeta {
                    uid: new_uid(),
                    created: s.created,
                    spec: Spec { dir: (!s.cwd.is_empty()).then(|| PathBuf::from(&s.cwd)), ..Default::default() },
                    ..Default::default()
                },
            );
            dirty = true;
        }
    }

    // Vanished sessions: after a daemon (re)start they are the ones lost to a reboot, so keep
    // them as "stopped". At runtime a plain shell that disappears was exited on purpose.
    let stopping = st.stopping.load(std::sync::atomic::Ordering::Relaxed);
    let gone: Vec<(String, String, bool)> = st
        .sessions
        .iter()
        .filter(|e| !live.iter().any(|s| &s.id == e.key()))
        .map(|e| {
            let m = e.value();
            let forget = forget_when_gone(first, stopping, live.is_empty(), m.spec.command.is_some(), m.seen_live);
            (e.key().clone(), m.uid.clone(), forget)
        })
        .collect();
    for (id, uid, forget) in gone {
        if forget {
            st.sessions.remove(&id);
            scrollback::remove(st, &uid).await;
            dirty = true;
        }
    }

    for s in live {
        let Some(mut meta) = st.sessions.get_mut(&s.id) else { continue };
        meta.seen_live = true;

        // A session nobody filed under a project yet (started by hand, or before projects existed)
        // belongs to the folder it is in now. It stays there after that.
        if meta.project.is_none() && !s.cwd.is_empty() {
            meta.project = Some(PathBuf::from(&s.cwd));
            dirty = true;
        }

        // Remember where a plain shell wandered to, so a restore lands in the same place.
        if meta.spec.command.is_none() && !s.cwd.is_empty() && meta.spec.dir.as_deref() != Some(Path::new(&s.cwd)) {
            meta.spec.dir = Some(PathBuf::from(&s.cwd));
            dirty = true;
        }
        if s.dead && meta.last_exit != s.exit_code {
            meta.last_exit = s.exit_code;
            dirty = true;
        }

        // Keep an on-disk copy of recent output.
        if (s.activity != meta.snap_activity || (s.dead && !meta.snap_dead))
            && meta.snap_at.is_none_or(|t| t.elapsed() >= scrollback::MIN_GAP)
        {
            meta.snap_activity = s.activity;
            meta.snap_dead = s.dead;
            meta.snap_at = Some(Instant::now());
            snaps.push((s.id.clone(), meta.uid.clone()));
        }

        let preset = meta.preset.as_deref().and_then(|n| st.cfg.presets.iter().find(|p| p.name == n));
        if !s.dead {
            meta.exit_notified = false;
            if let Some(p) = preset
                && p.agent() == Agent::Other
            {
                // Output since the last alert re-arms it; quiet for long enough fires it.
                if meta.last_activity == 0 {
                    meta.last_activity = s.activity; // first sighting: no alert for old output
                } else if s.activity != meta.last_activity {
                    meta.last_activity = s.activity;
                    meta.armed = true;
                } else if meta.armed && s.attached == 0 && now.saturating_sub(s.activity) >= p.idle_alert_secs {
                    meta.armed = false;
                    alerts.push(Alert::new(&s.id, Kind::Idle, None));
                }
            }
            if meta.restarts > 0 && meta.last_restart.is_some_and(|t| t.elapsed() > HEALTHY_AFTER) {
                meta.restarts = 0;
                dirty = true;
            }
            continue;
        }

        let Some(preset) = preset else { continue };
        // A missing exit status means the process was killed by a signal.
        let failed = s.exit_code != Some(0);
        if preset.restart != RestartPolicy::OnFailure || !failed || meta.restarts >= preset.max_restarts {
            meta.restart_at = None;
            if !meta.exit_notified {
                meta.exit_notified = true;
                let message = s.exit_code.map(|c| format!("exit code {c}"));
                alerts.push(Alert::new(&s.id, Kind::Exited, message));
            }
            continue;
        }
        match meta.restart_at {
            None => {
                let delay = preset.backoff_secs.saturating_mul(1u64 << meta.restarts.min(10));
                meta.restart_at = Some(Instant::now() + Duration::from_secs(delay));
            }
            Some(due) if Instant::now() >= due => {
                meta.restart_at = None;
                meta.restarts += 1;
                meta.last_restart = Some(Instant::now());
                let resume = if preset.resume_on_restart { meta.resume_value() } else { None };
                let (id, n) = (s.id.clone(), meta.restarts);
                drop(meta); // never hold a map guard across an await
                match respawn(&id, resume.as_deref()).await {
                    Ok(()) => tracing::info!("restarted {id} (attempt {n}{})", if resume.is_some() { ", resuming" } else { "" }),
                    Err(e) => tracing::warn!("restart {id}: {e:#}"),
                }
                dirty = true;
            }
            Some(_) => {}
        }
    }

    if dirty {
        save(st).await;
    }
    for (id, uid) in snaps {
        let st = st.clone();
        tokio::spawn(async move { scrollback::snapshot(&st, &id, &uid).await });
    }
    for a in alerts {
        // Delivery can be slow; don't hold up the next reconcile tick.
        tokio::spawn(notify::send_arc(st.clone(), a));
    }
}

/// Should a session whose tmux session vanished be forgotten (rather than kept as "stopped")?
///
/// Only when it looks like the user closed that one shell: the daemon watched it run, and now
/// it is gone while others live on. Everything else is kept and can be restored: the daemon
/// just started (reboot), it is shutting down, *every* session is gone at once (tmux died), the
/// session was started from a command (presets), or it was already stopped before this run.
fn forget_when_gone(first_tick: bool, stopping: bool, no_live_sessions: bool, has_command: bool, seen_live: bool) -> bool {
    seen_live && !(first_tick || stopping || no_live_sessions || has_command)
}

/// Rerun a dead pane's command, optionally resuming the agent. `respawn-pane`
/// uses the session environment, so the resume id is set there just for the spawn.
async fn respawn(id: &str, resume: Option<&str>) -> Result<()> {
    if let Some(r) = resume {
        tmux::set_env(id, "TTM_RESUME_ID", r).await?;
    }
    let res = tmux::respawn(id).await;
    if resume.is_some() {
        let _ = tmux::unset_env(id, "TTM_RESUME_ID").await;
    }
    res
}

#[cfg(test)]
mod tests {
    fn spec(cmd: &str, agent: Agent) -> Spec {
        Spec { command: Some(cmd.into()), agent: Some(agent), ..Default::default() }
    }

    #[test]
    fn claude_yolo_sets_the_sandbox_variable_and_the_flag() {
        let mut s = spec("claude --model opus", Agent::Claude);
        apply_yolo(&mut s);
        assert_eq!(s.command.as_deref(), Some("claude --model opus --dangerously-skip-permissions"));
        assert_eq!(s.env.get("IS_SANDBOX").map(String::as_str), Some("1"));
        assert!(s.yolo);
        // Asking twice changes nothing more, and an existing flag is not repeated.
        let before = s.clone();
        apply_yolo(&mut s);
        assert_eq!((s.command, s.env), (before.command, before.env));
        let mut has = spec("claude --dangerously-skip-permissions", Agent::Claude);
        apply_yolo(&mut has);
        assert_eq!(has.command.as_deref(), Some("claude --dangerously-skip-permissions"));
    }

    #[test]
    fn codex_yolo_replaces_its_approval_and_sandbox_options() {
        let mut s = spec("codex -a on-request -s read-only -m gpt-6", Agent::Codex);
        apply_yolo(&mut s);
        // `--yolo` is refused next to these, so they go.
        assert_eq!(s.command.as_deref(), Some("codex -m gpt-6 --yolo"));
        assert!(s.yolo && s.env.is_empty(), "Codex needs no environment variable");
        let mut long = spec("codex --ask-for-approval never --sandbox=workspace-write --full-auto", Agent::Codex);
        apply_yolo(&mut long);
        assert_eq!(long.command.as_deref(), Some("codex --yolo"));
        let mut plain = spec("codex", Agent::Codex);
        apply_yolo(&mut plain);
        assert_eq!(plain.command.as_deref(), Some("codex --yolo"));
    }

    #[test]
    fn other_programs_and_plain_shells_are_never_changed() {
        let mut other = spec("cargo watch -x test", Agent::Other);
        apply_yolo(&mut other);
        assert_eq!(other.command.as_deref(), Some("cargo watch -x test"));
        assert!(!other.yolo && other.env.is_empty());
        let mut shell = Spec::default();
        apply_yolo(&mut shell);
        assert!(!shell.yolo && shell.command.is_none());
    }

    #[test]
    fn a_preset_can_always_start_yolo_and_a_request_can_ask_for_it() {
        let preset = Preset { name: "c".into(), command: "claude".into(), ..Default::default() };
        assert!(!spec_for(&preset, false).yolo);
        assert!(spec_for(&preset, true).yolo);
        let always = Preset { yolo: true, ..preset };
        assert!(spec_for(&always, false).yolo);
    }

    use super::*;

    #[test]
    fn folder_sessions_are_named_after_the_folder() {
        let d = std::path::Path::new("/Users/me/code/my app");
        assert_eq!(name_for_dir("claude", d).as_deref(), Some("claude-my-app"));
        assert_eq!(name_for_dir("codex", std::path::Path::new("/")), None);
        let long = std::path::Path::new("/x/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(name_for_dir("claude", long), None);
    }

    #[test]
    fn tilde_dirs_expand() {
        let home = std::path::Path::new("/Users/me");
        assert_eq!(expand_home(std::path::Path::new("~/code"), Some(home)), PathBuf::from("/Users/me/code"));
        assert_eq!(expand_home(std::path::Path::new("/tmp"), Some(home)), PathBuf::from("/tmp"));
    }

    #[test]
    fn a_command_that_ends_leaves_a_shell() {
        // Even `exec cmd` (which replaces the inner shell) falls through to $SHELL.
        for cmd in ["exit 3", "exec sh -c 'exit 3'"] {
            let argv = shell_after(cmd);
            let out = std::process::Command::new(&argv[0])
                .args(&argv[1..])
                .env("SHELL", "/usr/bin/true")
                .output()
                .expect("run wrapper");
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(text.contains("status 3"), "{cmd}: {text}");
            assert!(out.status.success(), "{cmd}: the shell should have been exec'd");
        }
    }

    #[test]
    fn unique_names() {
        let taken = vec!["claude".to_string(), "claude-2".to_string()];
        assert_eq!(unique_name("codex", &taken), "codex");
        assert_eq!(unique_name("claude", &taken), "claude-3");
    }

    #[test]
    fn only_a_watched_lone_shell_that_closed_is_forgotten() {
        // (first tick, stopping, nothing live, has command, seen live)
        assert!(!forget_when_gone(true, false, false, false, true), "after a restart/reboot");
        assert!(!forget_when_gone(false, true, false, false, true), "while shutting down");
        assert!(!forget_when_gone(false, false, true, false, true), "tmux died: nothing is live");
        assert!(!forget_when_gone(false, false, false, true, true), "presets are always restorable");
        assert!(!forget_when_gone(false, false, false, false, false), "stopped before this run: never auto-forgotten");
        assert!(forget_when_gone(false, false, false, false, true), "one plain shell the daemon watched exit");
    }

    #[test]
    fn agent_id_validation() {
        assert!(valid_agent_id("b5f6c1c2-1d3e-4f5a-9b8c-0123456789ab"));
        assert!(valid_agent_id("thread_1.2:3"));
        assert!(!valid_agent_id(""));
        assert!(!valid_agent_id("a b"));
        assert!(!valid_agent_id("x;rm -rf"));
        assert!(!valid_agent_id(&"a".repeat(129)));
    }

    #[test]
    fn resume_value_depends_on_agent() {
        let mut m = SessionMeta::default();
        assert_eq!(m.resume_value(), None);
        m.spec.agent = Some(Agent::Other);
        assert_eq!(m.resume_value(), None);
        m.spec.agent = Some(Agent::Claude);
        assert_eq!(m.resume_value().as_deref(), Some(RESUME_LAST));
        m.agent_session_id = Some("abc-123".into());
        assert_eq!(m.resume_value().as_deref(), Some("abc-123"));
    }

    #[test]
    fn old_state_files_still_load() {
        // A state.json written before specs, notes and ids existed.
        let old = r#"{"sessions":{"agent-a":{"preset":"claude","restarts":2}},"subscriptions":[]}"#;
        let f: StateFile = serde_json::from_str(old).unwrap();
        let m = &f.sessions["agent-a"];
        assert_eq!((m.preset.as_deref(), m.restarts, m.pinned), (Some("claude"), 2, false));
        assert!(m.spec.command.is_none() && m.tags.is_empty());
    }
}
