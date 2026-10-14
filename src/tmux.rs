//! Thin wrapper around a dedicated tmux server (`tmux -L ttm`).
//! tmux is the source of truth for session state.

use std::path::Path;

use anyhow::{Context, Result};
use tokio::process::Command;

pub const SOCKET: &str = "ttm";

/// Session ids end up on a tmux command line, so keep them boring.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn base() -> Command {
    let mut c = Command::new("tmux");
    // `-u`: a daemon started by launchd or systemd has no LANG, and tmux then treats the
    // terminal as non-UTF-8 and mangles the tabs in `list-panes -F` output (so no session parses).
    c.args(["-u", "-L", SOCKET]);
    c
}

/// `-e` on new-session (used for per-session env) needs tmux 3.2.
const MIN_TMUX: (u32, u32) = (3, 2);

/// Parse the output of `tmux -V`: "tmux 3.8", "tmux 3.3a", "tmux next-3.5".
fn parse_version(out: &str) -> Option<(u32, u32)> {
    let v = out.trim().strip_prefix("tmux ")?;
    let v = v.strip_prefix("next-").unwrap_or(v);
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor: String = parts.next()?.chars().take_while(char::is_ascii_digit).collect();
    Some((major, minor.parse().ok()?))
}

/// Fail early, with a useful message, when tmux is missing or too old.
pub async fn check_installed() -> Result<()> {
    let out = Command::new("tmux")
        .arg("-V")
        .output()
        .await
        .context("tmux not found on PATH; install it (e.g. `brew install tmux` or `apt install tmux`)")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let found = parse_version(&text).with_context(|| format!("unrecognised `tmux -V` output: {text:?}"))?;
    anyhow::ensure!(
        found >= MIN_TMUX,
        "tmux {}.{} is too old; ttm needs {}.{} or newer",
        found.0,
        found.1,
        MIN_TMUX.0,
        MIN_TMUX.1
    );
    Ok(())
}

/// Exact-match session target for window/pane commands (`=` avoids prefix matching).
fn target(id: &str) -> String {
    format!("={id}:")
}

/// Argument vector for `tmux … new-session -A`: attach if the session exists,
/// otherwise create a plain shell. The trailing `set` commands apply
/// server-wide options on every attach. `id` is checked by `valid_session_id`
/// (no `;`), because tmux treats trailing-`;` arguments as separators.
///
/// The `terminal-features` line has to run *before* `new-session`: tmux reads it when the
/// client attaches, so after is too late. It lets the browser terminal receive OSC 52 copies and
/// OSC 8 links. `set-clipboard on` and `allow-passthrough on` forward the rest: OSC 52, and the
/// DCS-wrapped OSC 9;4 progress that Claude Code sends when it sees tmux. Index 10 keeps the
/// line from piling up a new entry on every attach. `focus-events` lets the agents see whether the
/// pane is focused; `set-titles` hands the program's own window title (OSC 0/2) to the browser,
/// which tmux otherwise keeps to itself.
pub fn attach_args(id: &str) -> Vec<String> {
    let mut a: Vec<String> = ["-u", "-L", SOCKET].map(String::from).into();
    for opt in [
        "start-server",
        ";", "set", "-s", "terminal-features[10]", "*:clipboard:hyperlinks",
        ";", "new-session", "-A", "-s", id,
        ";", "set", "-g", "window-size", "latest",
        ";", "set", "-g", "status", "off",
        ";", "set", "-g", "history-limit", "50000",
        ";", "set", "-g", "set-clipboard", "on",
        ";", "set", "-g", "focus-events", "on",
        ";", "set", "-g", "set-titles", "on",
        ";", "set", "-g", "set-titles-string", "#{pane_title}",
        ";", "set", "-g", "allow-passthrough", "on",
    ] {
        a.push(opt.into());
    }
    a
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub created: u64,
    pub attached: u32,
    /// Unix time of the last output in the session's window.
    pub activity: u64,
    pub width: u32,
    pub height: u32,
    pub dead: bool,
    pub exit_code: Option<u32>,
    pub command: String,
    /// Working directory of the active pane.
    pub cwd: String,
}

const LIST_FORMAT: &str = "#{session_name}\t#{session_created}\t#{session_attached}\t#{window_activity}\t#{window_width}\t#{window_height}\t#{pane_dead}\t#{pane_dead_status}\t#{pane_current_command}\t#{pane_current_path}";

/// One entry per session (its first pane). Empty when no server is running.
pub async fn list() -> Result<Vec<SessionInfo>> {
    let out = base()
        .args(["list-panes", "-a", "-F", LIST_FORMAT])
        .output()
        .await
        .context("running tmux (is it installed?)")?;
    if !out.status.success() {
        // "no server running" simply means there are no sessions yet.
        let err = String::from_utf8_lossy(&out.stderr);
        if !err.contains("no server running") {
            tracing::warn!("tmux list-panes failed: {}", err.trim());
        }
        return Ok(Vec::new());
    }
    let mut seen = std::collections::HashSet::new();
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(parse_line)
        .filter(|s| seen.insert(s.id.clone()))
        .collect())
}

fn parse_line(line: &str) -> Option<SessionInfo> {
    let mut f = line.split('\t');
    Some(SessionInfo {
        id: f.next()?.to_owned(),
        created: f.next()?.parse().ok()?,
        attached: f.next()?.parse().ok()?,
        activity: f.next()?.parse().ok()?,
        width: f.next()?.parse().ok()?,
        height: f.next()?.parse().ok()?,
        dead: f.next()? == "1",
        exit_code: f.next()?.parse().ok(),
        command: f.next()?.to_owned(),
        cwd: f.next().unwrap_or_default().to_owned(),
    })
}

async fn run(cmd: &mut Command, what: &str) -> Result<()> {
    let out = cmd.output().await.with_context(|| format!("running tmux {what}"))?;
    anyhow::ensure!(
        out.status.success(),
        "tmux {what} failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(())
}

/// Create a detached session running `argv` (no shell involved; pass
/// `["sh","-c",cmd]` for shell strings). Empty `argv` starts the default shell.
pub async fn create(
    id: &str,
    dir: Option<&Path>,
    env: &[(String, String)],
    argv: &[String],
    remain_on_exit: bool,
) -> Result<()> {
    let mut c = base();
    c.args(["new-session", "-d", "-s", id, "-x", "120", "-y", "40"]);
    if let Some(d) = dir {
        c.arg("-c").arg(d);
    }
    for (k, v) in env {
        c.arg("-e").arg(format!("{k}={v}"));
    }
    if !argv.is_empty() {
        c.arg("--").args(argv);
    }
    run(&mut c, "new-session").await?;
    if remain_on_exit {
        // Keep a dead pane around so you can read why the agent stopped.
        // Best effort: a command that exits instantly may already be gone.
        let _ = run(
            base().args(["set-option", "-w", "-t", &target(id), "remain-on-exit", "on"]),
            "set-option",
        )
        .await;
    }
    Ok(())
}

/// Stop the whole tmux server (every session and agent in it).
pub async fn kill_server() -> Result<()> {
    run(base().arg("kill-server"), "kill-server").await
}

pub async fn exists(id: &str) -> bool {
    base()
        .args(["has-session", "-t", &format!("={id}")])
        .output()
        .await
        .is_ok_and(|o| o.status.success())
}

/// The visible screen plus up to `lines` of scrollback, as plain text.
pub async fn capture(id: &str, lines: usize) -> Result<String> {
    let out = base()
        .args(["capture-pane", "-p", "-J", "-S", &format!("-{lines}"), "-t", &target(id)])
        .output()
        .await
        .context("running tmux capture-pane")?;
    anyhow::ensure!(out.status.success(), "no such session: {id}");
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Everything in the session, history and screen, as plain text with wrapped lines joined.
pub async fn capture_all(id: &str) -> Result<String> {
    let out = base()
        .args(["capture-pane", "-p", "-J", "-S", "-", "-t", &target(id)])
        .output()
        .await
        .context("running tmux capture-pane")?;
    anyhow::ensure!(out.status.success(), "no such session: {id}");
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Put the session in copy-mode on its `n`th match for `pattern`, counted from the bottom (the
/// newest is 1). tmux scrolls to it and highlights the matches. `pattern` must come from
/// `search::pattern`: it is a regex, and must not end in `;`, which tmux would take as a command
/// separator.
pub async fn search_jump(id: &str, pattern: &str, n: usize) -> Result<()> {
    anyhow::ensure!(n >= 1 && !pattern.ends_with(';'), "bad search");
    let t = target(id);
    let mut steps: Vec<Vec<String>> = vec![
        vec!["copy-mode".into(), "-t".into(), t.clone()],
        // Start from the very end, so a match on the last line is the first one found.
        vec!["send-keys".into(), "-X".into(), "-t".into(), t.clone(), "history-bottom".into()],
        vec!["send-keys".into(), "-X".into(), "-t".into(), t.clone(), "end-of-line".into()],
        vec!["send-keys".into(), "-X".into(), "-t".into(), t.clone(), "search-backward".into(), pattern.into()],
    ];
    if n > 1 {
        steps.push(vec!["send-keys".into(), "-X".into(), "-N".into(), (n - 1).to_string(), "-t".into(), t, "search-again".into()]);
    }
    let mut cmd = base();
    for (i, step) in steps.iter().enumerate() {
        if i > 0 {
            cmd.arg(";");
        }
        cmd.args(step);
    }
    let out = cmd.output().await.context("running tmux copy-mode")?;
    anyhow::ensure!(out.status.success(), "tmux: {}", String::from_utf8_lossy(&out.stderr).trim());
    Ok(())
}

/// Leave copy-mode. Fine when the session is not in it (the user may have pressed `q` already).
pub async fn search_close(id: &str) -> Result<()> {
    let out = base().args(["send-keys", "-X", "-t", &target(id), "cancel"]).output().await.context("running tmux send-keys")?;
    let err = String::from_utf8_lossy(&out.stderr);
    anyhow::ensure!(out.status.success() || err.contains("not in a mode"), "no such session: {id}");
    Ok(())
}

/// The visible screen with its escape codes (colours, faint, bold), for the few things plain text
/// can't tell apart.
pub async fn capture_styled(id: &str) -> Result<String> {
    let out = base()
        .args(["capture-pane", "-p", "-e", "-J", "-t", &target(id)])
        .output()
        .await
        .context("running tmux capture-pane")?;
    anyhow::ensure!(out.status.success(), "no such session: {id}");
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Keys tmux understands by name that we let callers send.
pub fn key_name(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => "Enter",
        "esc" | "escape" => "Escape",
        "tab" => "Tab",
        "btab" | "shift-tab" => "BTab",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "home" => "Home",
        "end" => "End",
        "pageup" | "pgup" => "PageUp",
        "pagedown" | "pgdn" => "PageDown",
        "backspace" | "bspace" => "BSpace",
        "space" => "Space",
        "c-c" | "ctrl-c" => "C-c",
        "c-d" | "ctrl-d" => "C-d",
        "c-z" | "ctrl-z" => "C-z",
        "c-l" | "ctrl-l" => "C-l",
        _ => return None,
    })
}

/// Type into a session without attaching: literal text and named keys, in order.
pub async fn send_steps(id: &str, steps: &[crate::screen::Step]) -> Result<()> {
    use crate::screen::Step;
    for step in steps {
        let mut c = base();
        c.args(["send-keys", "-t", &target(id)]);
        match step {
            Step::Text(t) => c.arg("-l").arg("--").arg(t),
            Step::Key(k) => c.arg(k),
        };
        run(&mut c, "send-keys").await?;
    }
    Ok(())
}

/// Paste `text` as a bracketed paste. Agents treat that as typed text with its newlines kept,
/// instead of submitting at the first one.
pub async fn paste(id: &str, text: &str) -> Result<()> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    // A buffer of our own, deleted by the paste, so concurrent pastes never share one.
    let buffer = format!("ttm-paste-{}", N.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    run(base().args(["set-buffer", "-b", &buffer, "--", text]), "set-buffer").await?;
    run(base().args(["paste-buffer", "-p", "-d", "-b", &buffer, "-t", &target(id)]), "paste-buffer").await
}

pub async fn set_env(id: &str, key: &str, value: &str) -> Result<()> {
    run(base().args(["set-environment", "-t", &format!("={id}"), key, value]), "set-environment").await
}

pub async fn unset_env(id: &str, key: &str) -> Result<()> {
    run(base().args(["set-environment", "-t", &format!("={id}"), "-u", key]), "set-environment").await
}

pub async fn kill(id: &str) -> Result<()> {
    run(base().args(["kill-session", "-t", &format!("={id}")]), "kill-session").await
}

pub async fn rename(id: &str, new: &str) -> Result<()> {
    run(base().args(["rename-session", "-t", &format!("={id}"), new]), "rename-session").await
}

/// Restart the (dead or alive) pane's original command.
pub async fn respawn(id: &str) -> Result<()> {
    run(base().args(["respawn-pane", "-k", "-t", &target(id)]), "respawn-pane").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_validation() {
        assert!(valid_session_id("claude-1_a"));
        assert!(!valid_session_id(""));
        assert!(!valid_session_id("a b"));
        assert!(!valid_session_id("a;rm"));
        assert!(!valid_session_id("a.b"));
    }

    #[test]
    fn attach_sets_terminal_features_before_the_client_attaches() {
        let a = attach_args("work");
        let at = |needle: &str| a.iter().position(|x| x == needle).unwrap();
        assert!(at("terminal-features[10]") < at("new-session"));
        assert_eq!(a[at("new-session") + 3], "work");
        assert!(a.windows(2).any(|w| w[0] == "allow-passthrough" && w[1] == "on"));
        assert!(a.windows(2).any(|w| w[0] == "set-clipboard" && w[1] == "on"));
    }

    #[test]
    fn parses_tmux_versions() {
        assert_eq!(parse_version("tmux 3.8\n"), Some((3, 8)));
        assert_eq!(parse_version("tmux 3.3a"), Some((3, 3)));
        assert_eq!(parse_version("tmux next-3.5"), Some((3, 5)));
        assert_eq!(parse_version("tmux 2.9a"), Some((2, 9)));
        assert!(parse_version("screen 4.9").is_none());
        assert!((2, 9) < MIN_TMUX && (3, 2) >= MIN_TMUX);
    }

    #[test]
    fn parses_live_and_dead_lines() {
        let s = parse_line("work\t1700000000\t1\t1700000050\t120\t40\t0\t\tzsh\t/home/me").unwrap();
        assert_eq!((s.id.as_str(), s.width, s.height), ("work", 120, 40));
        assert!(!s.dead);
        assert_eq!(s.exit_code, None);
        assert_eq!(s.command, "zsh");
        assert_eq!(s.cwd, "/home/me");

        let d = parse_line("agent\t1700000000\t0\t1700000099\t120\t40\t1\t3\tclaude").unwrap();
        assert!(d.dead);
        assert_eq!(d.exit_code, Some(3));
        assert!(parse_line("garbage").is_none());
    }
}
