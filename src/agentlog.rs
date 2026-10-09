//! Where Claude Code and Codex keep their files, and how to find the log of one conversation.
//! Shared by plan-usage (token totals) and the chat view.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use crate::config::Agent;

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn claude_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| home().map(|h| h.join(".claude")))
}

pub fn codex_dir() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".codex")))
}

/// The last `max` bytes of `path`.
pub fn read_tail(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(max)))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Subdirectories of `dir`, newest name first (Codex names them 2026 / 10 / 08).
pub fn subdirs_newest_first(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    v.sort_unstable_by(|a, b| b.cmp(a));
    v
}

/// Codex's rollout logs, newest first.
pub fn rollouts_newest_first(root: &Path) -> impl Iterator<Item = PathBuf> {
    subdirs_newest_first(root)
        .into_iter()
        .flat_map(|y| subdirs_newest_first(&y))
        .flat_map(|m| subdirs_newest_first(&m))
        .flat_map(|d| {
            let mut files: Vec<PathBuf> = std::fs::read_dir(d)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
                .collect();
            files.sort_unstable_by(|a, b| b.cmp(a));
            files
        })
}

/// The log file of conversation `id` for `agent`, if it exists. Blocking: walks directories.
/// `id` must already be checked with `sessions::valid_agent_id`.
pub fn find_log(agent: Agent, id: &str) -> Option<PathBuf> {
    match agent {
        Agent::Claude => {
            let projects = claude_dir()?.join("projects");
            subdirs_newest_first(&projects)
                .into_iter()
                .map(|d| d.join(format!("{id}.jsonl")))
                .find(|p| p.is_file())
        }
        Agent::Codex => find_rollout(&codex_dir()?.join("sessions"), id),
        Agent::Other => None,
    }
}

/// The Codex rollout log of thread `id` under `sessions_root` (`YYYY/MM/DD/rollout-…-<id>.jsonl`).
pub fn find_rollout(sessions_root: &Path, id: &str) -> Option<PathBuf> {
    let suffix = format!("-{id}.jsonl");
    rollouts_newest_first(sessions_root)
        .find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(&suffix)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rollout_is_found_by_its_thread_id_and_a_helper_thread_has_none() {
        let root = std::env::temp_dir().join(format!("ttm-agentlog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let day = root.join("2026/10/09");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join("rollout-2026-10-09T03-06-20-01a11e69-4e78-7233-8832-99e39d90e161.jsonl"), "{}").unwrap();
        std::fs::write(day.join("notes.txt"), "x").unwrap();
        assert!(find_rollout(&root, "01a11e69-4e78-7233-8832-99e39d90e161").is_some());
        // The id a background helper thread reports has no rollout of its own.
        assert!(find_rollout(&root, "01a11e6a-72e1-7ad2-bafc-2d567f1cf9d0").is_none());
        // An id that is merely a suffix of another must not match.
        assert!(find_rollout(&root, "9e39d90e161").is_none());
        assert!(find_rollout(&root.join("nope"), "x").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
