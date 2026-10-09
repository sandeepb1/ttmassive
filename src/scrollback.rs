//! On-disk copies of each session's recent output, so you can still read what
//! an agent last showed after it exits or the machine reboots.

use std::{
    path::PathBuf,
    time::{Duration, UNIX_EPOCH},
};

use anyhow::Result;

use crate::AppState;

/// Lines of history captured per snapshot.
pub const SNAPSHOT_LINES: usize = 2000;
/// Hard cap per file.
const MAX_BYTES: usize = 512 * 1024;

fn path(st: &AppState, uid: &str) -> Option<PathBuf> {
    // uids are our own hex ids; refuse anything that could escape the directory.
    (!uid.is_empty() && uid.bytes().all(|b| b.is_ascii_hexdigit())).then(|| st.data_dir.join("scrollback").join(format!("{uid}.txt")))
}

/// Keep the tail of `text`, within `MAX_BYTES`, cut on a line boundary.
fn clamp(text: &str) -> &str {
    if text.len() <= MAX_BYTES {
        return text;
    }
    let mut start = text.len() - MAX_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let cut = &text[start..];
    cut.find('\n').map_or(cut, |i| &cut[i + 1..])
}

/// Trailing blank lines are just empty screen; drop them.
fn trim_blank_tail(text: &str) -> &str {
    text.trim_end_matches(['\n', ' ', '\r'])
}

pub async fn save(st: &AppState, uid: &str, text: &str) -> Result<()> {
    let Some(path) = path(st, uid) else { return Ok(()) };
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let body = format!("{}\n", clamp(trim_blank_tail(text)));
    let tmp = path.with_extension("txt.tmp");
    tokio::fs::write(&tmp, body).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).await?;
    }
    tokio::fs::rename(&tmp, &path).await?;
    Ok(())
}

/// The last `lines` lines of the saved output, if any.
pub async fn read_tail(st: &AppState, uid: &str, lines: usize) -> Option<String> {
    let text = tokio::fs::read_to_string(path(st, uid)?).await.ok()?;
    let all: Vec<&str> = text.lines().collect();
    Some(all[all.len().saturating_sub(lines)..].join("\n"))
}

pub async fn remove(st: &AppState, uid: &str) {
    if let Some(p) = path(st, uid) {
        let _ = tokio::fs::remove_file(p).await;
    }
}

/// Unix time the snapshot was last written.
pub async fn modified(st: &AppState, uid: &str) -> Option<u64> {
    let m = tokio::fs::metadata(path(st, uid)?).await.ok()?.modified().ok()?;
    m.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// Snapshot one live session (best effort).
pub async fn snapshot(st: &AppState, id: &str, uid: &str) {
    match crate::tmux::capture(id, SNAPSHOT_LINES).await {
        Ok(text) => {
            if let Err(e) = save(st, uid, &text).await {
                tracing::warn!("saving scrollback for {id}: {e:#}");
            }
        }
        Err(e) => tracing::debug!("scrollback capture {id}: {e:#}"),
    }
}

/// Snapshot every live session; used on shutdown.
pub async fn snapshot_all(st: &AppState) {
    let targets: Vec<(String, String)> =
        st.sessions.iter().map(|e| (e.key().clone(), e.value().uid.clone())).collect();
    for (id, uid) in targets {
        if crate::tmux::exists(&id).await {
            snapshot(st, &id, &uid).await;
        }
    }
}

/// Minimum gap between snapshots of one session.
pub const MIN_GAP: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_tail_on_line_boundary() {
        let line = "x".repeat(99) + "\n";
        let big = line.repeat(MAX_BYTES / 100 + 50);
        let out = clamp(&big);
        assert!(out.len() <= MAX_BYTES);
        assert!(out.starts_with('x') && out.ends_with('\n'));
        assert_eq!(out.len() % 100, 0, "must cut on a line boundary");
        assert_eq!(clamp("short"), "short");
    }

    #[test]
    fn blank_tail_is_trimmed() {
        assert_eq!(trim_blank_tail("a\nb\n\n   \n"), "a\nb");
    }
}
