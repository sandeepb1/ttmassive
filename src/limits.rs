//! Resource limits for preset sessions.
//!
//! Linux: wrap the command in a transient systemd user scope (cgroup limits).
//! Elsewhere: limits are not enforced. `setrlimit(RLIMIT_AS)` would be the
//! only portable memory cap, but Node-based agents (Claude Code, Codex) reserve
//! huge virtual address ranges and crash under it, so we don't pretend.

use crate::config::Preset;

pub fn wrap(preset: &Preset, argv: Vec<String>) -> Vec<String> {
    if preset.memory_mb.is_none() && preset.cpu_percent.is_none() {
        return argv;
    }
    if !cfg!(target_os = "linux") {
        tracing::warn!("preset {}: resource limits are only enforced on Linux; ignoring", preset.name);
        return argv;
    }
    let mut out: Vec<String> = ["systemd-run", "--user", "--scope", "--quiet"].map(String::from).into();
    if let Some(mb) = preset.memory_mb {
        out.extend(["-p".into(), format!("MemoryMax={mb}M"), "-p".into(), "MemorySwapMax=0".into()]);
    }
    if let Some(pct) = preset.cpu_percent {
        out.extend(["-p".into(), format!("CPUQuota={pct}%")]);
    }
    out.push("--".into());
    out.extend(argv);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv() -> Vec<String> {
        vec!["sh".into(), "-c".into(), "claude".into()]
    }

    #[test]
    fn no_limits_is_passthrough() {
        assert_eq!(wrap(&Preset::default(), argv()), argv());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_wraps_in_systemd_scope() {
        let p = Preset { memory_mb: Some(512), cpu_percent: Some(200), ..Preset::default() };
        let w = wrap(&p, argv());
        assert_eq!(w[0], "systemd-run");
        assert!(w.contains(&"MemoryMax=512M".to_string()));
        assert!(w.contains(&"CPUQuota=200%".to_string()));
        assert_eq!(&w[w.len() - 3..], &argv()[..]);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_ignores_limits() {
        let p = Preset { memory_mb: Some(512), ..Preset::default() };
        assert_eq!(wrap(&p, argv()), argv());
    }
}
