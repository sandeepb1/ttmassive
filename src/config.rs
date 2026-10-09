use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub bind: SocketAddr,
    /// Argon2 PHC string, generate with `ttm hash-password`.
    pub password_hash: Option<String>,
    /// Plaintext alternative, normally supplied through `TTM_PASSWORD`.
    pub password: Option<String>,
    /// Honor `X-Forwarded-For` / `X-Forwarded-Proto` from a reverse proxy.
    pub trust_proxy: bool,
    pub session_ttl_hours: u64,
    pub data_dir: Option<PathBuf>,
    /// Contact URI sent to push services (`mailto:` or `https:`). Apple is strict about it.
    pub vapid_subject: String,
    /// Where hook processes reach this server. Defaults to loopback on the bind port.
    pub notify_url: Option<String>,
    /// Control socket for the `ttm` CLI. Default: `<data_dir>/ttm.sock`.
    pub socket: Option<PathBuf>,
    pub presets: Vec<Preset>,
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    #[default]
    Never,
    OnFailure,
}

/// Which agent a preset runs; decides how needs-input alerts are detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    /// Claude Code: Notification/Stop hooks.
    Claude,
    /// Codex CLI: `notify` program.
    Codex,
    /// Anything else: alert when output goes quiet.
    Other,
}

impl Agent {
    /// Guess from the command line: the first word that is `claude` or `codex`.
    pub fn infer(command: &str) -> Self {
        command
            .split(|c: char| c.is_whitespace() || ";&|()".contains(c))
            .map(|w| w.rsplit('/').next().unwrap_or(w))
            .find_map(|w| match w {
                "claude" => Some(Self::Claude),
                "codex" => Some(Self::Codex),
                _ => None,
            })
            .unwrap_or(Self::Other)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Preset {
    pub name: String,
    /// Run through `sh -c`.
    pub command: String,
    pub dir: Option<PathBuf>,
    pub env: std::collections::BTreeMap<String, String>,
    pub restart: RestartPolicy,
    pub max_restarts: u32,
    pub backoff_secs: u64,
    /// Linux only (systemd cgroup `MemoryMax`); ignored with a warning elsewhere.
    pub memory_mb: Option<u64>,
    /// Linux only (systemd `CPUQuota`, 100 = one core).
    pub cpu_percent: Option<u32>,
    /// Overrides the guess from `command`.
    pub agent: Option<Agent>,
    /// For `other` agents: alert after this many quiet seconds following activity.
    pub idle_alert_secs: u64,
    /// Start this preset when the daemon starts (restoring it if it was stopped).
    pub autostart: bool,
    /// When `restart = on_failure` respawns a Claude/Codex session, resume its conversation.
    pub resume_on_restart: bool,
    /// Always start this preset with its agent's "skip all permission checks" options
    /// (Claude: `IS_SANDBOX=1` and `--dangerously-skip-permissions`; Codex: `--yolo`).
    pub yolo: bool,
}

/// Several presets started together.
#[derive(Debug, Clone, Deserialize)]
pub struct Group {
    pub name: String,
    pub presets: Vec<String>,
}

impl Preset {
    pub fn agent(&self) -> Agent {
        self.agent.unwrap_or_else(|| Agent::infer(&self.command))
    }
}

impl Default for Preset {
    fn default() -> Self {
        Self {
            name: String::new(),
            command: String::new(),
            dir: None,
            env: Default::default(),
            restart: RestartPolicy::Never,
            max_restarts: 5,
            backoff_secs: 2,
            memory_mb: None,
            cpu_percent: None,
            agent: None,
            idle_alert_secs: 20,
            autostart: false,
            resume_on_restart: true,
            yolo: false,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: ([127, 0, 0, 1], 7681).into(),
            password_hash: None,
            password: None,
            trust_proxy: false,
            session_ttl_hours: 24 * 30,
            data_dir: None,
            vapid_subject: "mailto:ttm@example.com".into(),
            notify_url: None,
            socket: None,
            presets: Vec::new(),
            groups: Vec::new(),
        }
    }
}

impl Config {
    /// For the server: a password must be configured.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        Self::load_inner(path, true)
    }

    /// For CLI commands that only talk to a running daemon.
    pub fn load_for_cli(path: Option<&Path>) -> Result<Self> {
        Self::load_inner(path, false)
    }

    fn load_inner(path: Option<&Path>, need_password: bool) -> Result<Self> {
        let path = path.map(Path::to_path_buf).or_else(default_config_path);
        let mut cfg = match path.as_deref().filter(|p| p.exists()) {
            Some(p) => {
                let raw = std::fs::read_to_string(p)
                    .with_context(|| format!("reading config {}", p.display()))?;
                toml::from_str(&raw).with_context(|| format!("parsing config {}", p.display()))?
            }
            None => Self::default(),
        };
        if let Ok(pw) = std::env::var("TTM_PASSWORD") {
            cfg.password = Some(pw);
        }
        if let Ok(h) = std::env::var("TTM_PASSWORD_HASH") {
            cfg.password_hash = Some(h);
        }
        for p in &mut cfg.presets {
            anyhow::ensure!(
                crate::tmux::valid_session_id(&p.name),
                "preset name {:?} must be 1-64 chars of [A-Za-z0-9_-]",
                p.name
            );
            anyhow::ensure!(!p.command.trim().is_empty(), "preset {} has an empty command", p.name);
            p.dir = p.dir.take().map(|d| expand_tilde(&d));
        }
        for g in &cfg.groups {
            anyhow::ensure!(crate::tmux::valid_session_id(&g.name), "group name {:?} is invalid", g.name);
            for name in &g.presets {
                anyhow::ensure!(
                    cfg.presets.iter().any(|p| &p.name == name),
                    "group {} refers to unknown preset {name:?}",
                    g.name
                );
            }
        }
        anyhow::ensure!(
            !need_password || cfg.password_hash.is_some() || cfg.password.is_some(),
            "no password configured: set password_hash in config or TTM_PASSWORD in env"
        );
        Ok(cfg)
    }

    pub fn notify_url(&self) -> String {
        self.notify_url
            .clone()
            .unwrap_or_else(|| format!("http://127.0.0.1:{}", self.bind.port()))
    }

    pub fn socket_path(&self) -> Result<PathBuf> {
        Ok(self.socket.clone().unwrap_or(self.data_dir()?.join("ttm.sock")))
    }

    pub fn data_dir(&self) -> Result<PathBuf> {
        let dir = self
            .data_dir
            .clone()
            .or_else(|| dirs_data().map(|d| d.join("ttm")))
            .context("cannot determine data dir; set data_dir in config")?;
        if !dir.exists() {
            std::fs::create_dir_all(&dir)?;
            // Only tighten a directory we just created; never chmod one the user pointed us at.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(dir)
    }
}

pub fn expand_tilde(p: &Path) -> PathBuf {
    match (p.strip_prefix("~"), home()) {
        (Ok(rest), Some(h)) => h.join(rest),
        _ => p.to_path_buf(),
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn default_config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".config")))?;
    Some(base.join("ttm/config.toml"))
}

fn dirs_data() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return home().map(|h| h.join("Library/Application Support"));
    }
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".local/share")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_presets_and_defaults() {
        let cfg: Config = toml::from_str(
            r#"
            password = "x"
            [[presets]]
            name = "claude"
            command = "claude"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.bind.port(), 7681);
        assert_eq!(cfg.presets[0].name, "claude");
        assert_eq!(cfg.presets[0].restart, RestartPolicy::Never);
        assert_eq!(cfg.presets[0].max_restarts, 5);
    }

    #[test]
    fn parses_restart_and_limits() {
        let cfg: Config = toml::from_str(
            r#"
            [[presets]]
            name = "codex"
            command = "codex"
            restart = "on_failure"
            memory_mb = 2048
            [presets.env]
            FOO = "bar"
            "#,
        )
        .unwrap();
        let p = &cfg.presets[0];
        assert_eq!(p.restart, RestartPolicy::OnFailure);
        assert_eq!(p.memory_mb, Some(2048));
        assert_eq!(p.env["FOO"], "bar");
    }

    #[test]
    fn example_config_stays_valid() {
        let cfg: Config = toml::from_str(include_str!("../config.example.toml")).unwrap();
        assert_eq!(cfg.presets.len(), 3);
        assert_eq!(cfg.presets[2].restart, RestartPolicy::OnFailure);
        assert_eq!(cfg.presets[0].agent(), Agent::Claude);
        assert!(cfg.password_hash.is_some());
        assert_eq!(cfg.groups.len(), 1);
        assert_eq!(cfg.groups[0].presets, ["claude", "build-watch"]);
        assert!(!cfg.presets[0].autostart && cfg.presets[0].resume_on_restart);
    }

    #[test]
    fn groups_must_name_real_presets() {
        let dir = std::env::temp_dir().join(format!("ttm-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "password = \"x\"\n[[groups]]\nname = \"g\"\npresets = [\"missing\"]\n").unwrap();
        let err = Config::load(Some(&path)).unwrap_err().to_string();
        assert!(err.contains("unknown preset"), "{err}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn agent_inference() {
        assert_eq!(Agent::infer("claude"), Agent::Claude);
        assert_eq!(Agent::infer("cd ~/p && /usr/local/bin/claude --model x"), Agent::Claude);
        assert_eq!(Agent::infer("exec codex"), Agent::Codex);
        assert_eq!(Agent::infer("aider --yes"), Agent::Other);
        assert_eq!(Agent::infer("echo claudette"), Agent::Other);
    }

    #[test]
    fn tilde_expansion() {
        let h = home().unwrap();
        assert_eq!(expand_tilde(Path::new("~/proj")), h.join("proj"));
        assert_eq!(expand_tilde(Path::new("/abs")), PathBuf::from("/abs"));
    }
}
