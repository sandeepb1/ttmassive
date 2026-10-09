//! A public HTTPS address for ttm, made by running the system `ssh` as a reverse tunnel to
//! localhost.run or pinggy.io. No account, no extra binary, no new crates: every Mac and Linux
//! machine already has OpenSSH. We read the address off ssh's output and keep the tunnel up.

use std::{
    path::Path,
    process::Stdio,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};

use crate::AppState;

const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A connection that lasted this long counts as healthy: the next failure starts the backoff over.
const HEALTHY_AFTER: Duration = Duration::from_secs(30);
/// How many of ssh's last lines to keep for `status`.
const ERROR_LINES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    #[serde(rename = "localhost.run")]
    LocalhostRun,
    #[serde(rename = "pinggy")]
    Pinggy,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::LocalhostRun => "localhost.run",
            Self::Pinggy => "pinggy",
        }
    }

    /// Hostnames the public address can have. Everything else ssh prints (docs, admin pages,
    /// upgrade nags) is ignored.
    fn domains(self) -> &'static [&'static str] {
        match self {
            Self::LocalhostRun => &["lhr.life", "localhost.run"],
            Self::Pinggy => &["pinggy.link", "pinggy-free.link", "pinggy.io"],
        }
    }

    /// Hosts that serve documentation, not a tunnel.
    fn ignored_hosts(self) -> &'static [&'static str] {
        &["localhost.run", "admin.localhost.run", "pinggy.io", "dashboard.pinggy.io", "www.pinggy.io"]
    }

    /// The free Pinggy tunnel is closed by the server after an hour.
    pub fn expires(self, token: Option<&str>) -> bool {
        self == Self::Pinggy && token.is_none()
    }

    /// Everything after `ssh` on the command line. Our own options come first so that nothing in
    /// the user's `~/.ssh` can change how the tunnel behaves.
    pub fn ssh_args(self, port: u16, token: Option<&str>, known_hosts: &Path) -> Vec<String> {
        let mut a: Vec<String> = [
            "-F",
            "none",
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "IdentityAgent=none",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ServerAliveInterval=30",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "ConnectTimeout=15",
        ]
        .map(String::from)
        .into();
        a.push("-o".into());
        a.push(format!("UserKnownHostsFile={}", known_hosts.display()));
        match self {
            Self::LocalhostRun => {
                a.extend(["-R".into(), format!("80:127.0.0.1:{port}"), "nokey@localhost.run".into()]);
            }
            Self::Pinggy => {
                let dest = match token {
                    Some(t) => format!("{t}@pro.pinggy.io"),
                    None => "free.pinggy.io".into(),
                };
                a.extend(["-p".into(), "443".into(), format!("-R0:127.0.0.1:{port}"), dest]);
            }
        }
        a
    }

    /// The public https address in one line of ssh's output, if there is one.
    pub fn parse_url(self, line: &str) -> Option<String> {
        line.split(|c: char| c.is_whitespace() || matches!(c, ',' | '"' | '\'' | '<' | '>' | '(' | ')'))
            .filter_map(|w| w.strip_prefix("https://"))
            .find_map(|rest| {
                let host = rest.split(['/', ':', '?']).next()?.to_ascii_lowercase();
                let is_ours = self.domains().iter().any(|d| host == *d || host.ends_with(&format!(".{d}")));
                let is_docs = self.ignored_hosts().contains(&host.as_str());
                (is_ours && !is_docs && host.len() > 8).then(|| format!("https://{host}"))
            })
    }
}

impl FromStr for Provider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "localhost.run" | "localhost-run" | "lhr" => Ok(Self::LocalhostRun),
            "pinggy" | "pinggy.io" => Ok(Self::Pinggy),
            other => Err(format!("unknown tunnel {other:?}: use `localhost.run` or `pinggy`")),
        }
    }
}

/// What the tunnel is doing right now.
#[derive(Debug, Default)]
pub struct TunnelState {
    pub provider: Option<Provider>,
    pub url: Option<String>,
    /// When the current connection came up.
    pub since: Option<Instant>,
    pub reconnects: u32,
    pub last_error: Option<String>,
    /// The free Pinggy tunnel will be closed after an hour, so its address will change.
    pub expires: bool,
}

impl TunnelState {
    pub fn enabled(&self) -> bool {
        self.provider.is_some()
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": self.provider.map(Provider::name),
            "url": self.url,
            "up_secs": self.since.map(|t| t.elapsed().as_secs()),
            "reconnects": self.reconnects,
            "last_error": self.last_error,
            "expires": self.expires,
        })
    }
}

/// Refuse a public tunnel that the password alone would not protect.
pub fn check_config(cfg: &crate::config::Config) -> Result<()> {
    if cfg.tunnel.is_none() {
        return Ok(());
    }
    anyhow::ensure!(
        cfg.password_hash.is_some(),
        "a public tunnel needs `password_hash` (make one with `ttm hash-password`); a plaintext \
         `password` or TTM_PASSWORD is not accepted when ttm is reachable from the internet"
    );
    Ok(())
}

/// Fail early, with a useful message, if there is no `ssh` to run.
pub async fn check_ssh() -> Result<()> {
    Command::new("ssh")
        .arg("-V")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|_| ())
        .context("--tunnel needs `ssh` (OpenSSH) on your PATH")
}

/// Keep the tunnel connected until ttm shuts down, reconnecting with a growing pause.
pub async fn run(st: Arc<AppState>, provider: Provider, token: Option<String>) {
    let known_hosts = st.data_dir.join("tunnel_known_hosts");
    let port = st.cfg.bind.port();
    let expires = provider.expires(token.as_deref());
    if expires {
        tracing::warn!(
            "pinggy's free tunnel closes after an hour and the address changes; \
             set `tunnel_token` for one that lasts"
        );
    }
    let mut backoff = BACKOFF_START;
    loop {
        let started = Instant::now();
        let result = connect(&st, provider, port, token.as_deref(), &known_hosts, expires).await;
        let healthy = started.elapsed() >= HEALTHY_AFTER;
        {
            let mut t = st.tunnel.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            t.since = None;
            t.reconnects += 1;
            if let Err(e) = &result {
                t.last_error = Some(format!("{e:#}"));
            }
        }
        match &result {
            Ok(()) => tracing::warn!("tunnel closed; reconnecting in {}s", backoff.as_secs()),
            Err(e) => tracing::warn!("tunnel failed: {e:#}; retrying in {}s", backoff.as_secs()),
        }
        if healthy {
            backoff = BACKOFF_START;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// One ssh connection, from spawn until it exits.
async fn connect(
    st: &Arc<AppState>,
    provider: Provider,
    port: u16,
    token: Option<&str>,
    known_hosts: &Path,
    expires: bool,
) -> Result<()> {
    let mut child = Command::new("ssh")
        .args(provider.ssh_args(port, token, known_hosts))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("starting ssh")?;
    let out = child.stdout.take().context("ssh stdout")?;
    let err = child.stderr.take().context("ssh stderr")?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(bool, String)>();
    forward_lines(out, false, tx.clone());
    forward_lines(err, true, tx.clone());
    drop(tx);

    let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut announced = false;
    let mut expires = expires;
    while let Some((is_err, line)) = rx.recv().await {
        let clean = strip_ansi(&line);
        // A token the service does not accept is not an error: it hands out a free tunnel.
        if says_free_tier(&clean) && !expires {
            expires = true;
            if token.is_some() {
                tracing::warn!("{} did not accept the tunnel_token: this is a free tunnel and its address will change", provider.name());
            }
        }
        if let Some(url) = provider.parse_url(&clean) {
            // Some services print more than one address; the first is the one to use.
            if !announced {
                announced = true;
                announce(st, provider, url, expires).await;
            }
            continue;
        }
        // Only ssh's own complaints say why it stopped; the banner on stdout does not.
        if is_err && is_error_line(&clean) {
            tail.push_back(clean.trim().to_owned());
            if tail.len() > ERROR_LINES {
                tail.pop_front();
            }
        }
    }
    let status = child.wait().await.context("waiting for ssh")?;
    let why = tail.iter().cloned().collect::<Vec<_>>().join(" / ");
    anyhow::bail!("ssh exited ({status}){}", if why.is_empty() { String::new() } else { format!(": {why}") })
}

/// Send each line of `r` to `tx`, tagged with whether it came from stderr.
fn forward_lines<R>(r: R, is_err: bool, tx: tokio::sync::mpsc::UnboundedSender<(bool, String)>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(r).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send((is_err, line)).is_err() {
                break;
            }
        }
    });
}

/// A new address: remember it, log it, and tell the user's devices if it changed.
async fn announce(st: &Arc<AppState>, provider: Provider, url: String, expires: bool) {
    let changed = {
        let mut t = st.tunnel.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let changed = t.url.as_deref() != Some(url.as_str());
        t.provider = Some(provider);
        t.url = Some(url.clone());
        t.since = Some(Instant::now());
        t.last_error = None;
        t.expires = expires;
        changed
    };
    tracing::info!("public address: {url} ({})", provider.name());
    if changed {
        let mut alert = crate::notify::Alert::new("tunnel", crate::notify::Kind::Tunnel, Some(url));
        alert.message = alert.message.take().map(|u| format!("ttm is now at {u}"));
        crate::notify::send(st, alert).await;
    }
}

/// The banner lines pinggy prints when the tunnel is on its free tier.
fn says_free_tier(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("you are not authenticated") || l.contains("tunnel will expire")
}

/// Whether a line on ssh's stderr explains a failure, as opposed to routine notices.
fn is_error_line(line: &str) -> bool {
    let l = line.trim();
    !l.is_empty()
        && !l.starts_with("**")
        && !l.starts_with("Warning: Permanently added")
        && !l.starts_with("Allocated port")
}

/// ssh and the services behind it sometimes colour their banners.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LHR: &str = include_str!("testdata/localhost_run.txt");
    const PINGGY: &str = include_str!("testdata/pinggy.txt");

    fn urls(p: Provider, text: &str) -> Vec<String> {
        text.lines().filter_map(|l| p.parse_url(&strip_ansi(l))).collect()
    }

    #[test]
    fn localhost_run_banner_gives_only_the_tunnel_address() {
        assert_eq!(urls(Provider::LocalhostRun, LHR), ["https://b78e2f4d1619ec.lhr.life"]);
    }

    #[test]
    fn pinggy_banner_gives_the_https_address() {
        // It prints a second address too (`*.free.pinggy.net`); we only ever use the first.
        assert_eq!(urls(Provider::Pinggy, PINGGY), ["https://rnqwx-203-0-113-7.run.pinggy-free.link"]);
    }

    #[test]
    fn docs_and_look_alikes_are_not_addresses() {
        let p = Provider::LocalhostRun;
        assert_eq!(p.parse_url("see https://localhost.run/docs for more"), None);
        assert_eq!(p.parse_url("https://admin.localhost.run/"), None);
        assert_eq!(p.parse_url("https://lhr.life.evil.example/"), None);
        assert_eq!(p.parse_url("https://evil-lhr.life/"), None);
        assert_eq!(p.parse_url("http://abc123def456.lhr.life"), None);
        assert_eq!(Provider::Pinggy.parse_url("https://dashboard.pinggy.io/x"), None);
    }

    #[test]
    fn address_is_cut_at_the_path_and_lowercased() {
        assert_eq!(
            Provider::LocalhostRun.parse_url("tunneled with tls termination, https://ABCDEF123456.lhr.life/x?y=1"),
            Some("https://abcdef123456.lhr.life".into())
        );
    }

    #[test]
    fn names_parse() {
        assert_eq!("localhost.run".parse(), Ok(Provider::LocalhostRun));
        assert_eq!("PINGGY".parse(), Ok(Provider::Pinggy));
        assert!("ngrok".parse::<Provider>().is_err());
    }

    #[test]
    fn ssh_ignores_the_users_own_setup() {
        let a = Provider::LocalhostRun.ssh_args(7681, None, Path::new("/d/kh"));
        let j = a.join(" ");
        assert!(j.starts_with("-F none"), "{j}");
        for opt in ["BatchMode=yes", "StrictHostKeyChecking=accept-new", "IdentityAgent=none", "UserKnownHostsFile=/d/kh"] {
            assert!(j.contains(opt), "{opt} missing from {j}");
        }
        assert!(j.ends_with("-R 80:127.0.0.1:7681 nokey@localhost.run"), "{j}");
    }

    #[test]
    fn pinggy_uses_the_token_host_only_with_a_token() {
        let free = Provider::Pinggy.ssh_args(1, None, Path::new("/k")).join(" ");
        assert!(free.ends_with("-p 443 -R0:127.0.0.1:1 free.pinggy.io"), "{free}");
        let pro = Provider::Pinggy.ssh_args(1, Some("tok"), Path::new("/k")).join(" ");
        assert!(pro.ends_with("tok@pro.pinggy.io"), "{pro}");
        assert!(Provider::Pinggy.expires(None));
        assert!(!Provider::Pinggy.expires(Some("tok")));
        assert!(!Provider::LocalhostRun.expires(None));
    }

    #[test]
    fn pinggy_free_tier_is_recognised_from_its_banner() {
        assert!(PINGGY.lines().any(says_free_tier));
        assert!(!LHR.lines().any(says_free_tier));
    }

    #[test]
    fn routine_ssh_notices_are_not_errors() {
        for l in [
            "** WARNING: connection is not using a post-quantum key exchange algorithm.",
            "Warning: Permanently added 'localhost.run' (ED25519) to the list of known hosts.",
            "Allocated port 2 for remote forward to 127.0.0.1:18765",
            "   ",
        ] {
            assert!(!is_error_line(l), "{l}");
        }
        assert!(is_error_line("ssh: connect to host localhost.run port 22: Connection refused"));
        assert!(is_error_line("Connection to localhost.run closed by remote host."));
    }

    #[test]
    fn ansi_colours_are_removed() {
        assert_eq!(strip_ansi("\u{1b}[1;32mhttps://x\u{1b}[0m"), "https://x");
    }

    #[test]
    fn a_plaintext_password_is_refused_for_a_tunnel() {
        let mut cfg = crate::config::Config {
            tunnel: Some(Provider::LocalhostRun),
            password: Some("x".into()),
            ..Default::default()
        };
        assert!(check_config(&cfg).is_err());
        cfg.password_hash = Some("$argon2id$v=19$m=19456,t=2,p=1$x$y".into());
        assert!(check_config(&cfg).is_ok());
        cfg.tunnel = None;
        cfg.password_hash = None;
        assert!(check_config(&cfg).is_ok());
    }
}
