//! Running ttm as a background daemon: pidfile, start/stop/status/logs, and
//! generating the systemd / launchd service files.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

use crate::{
    config::Config,
    control::{self, Request},
};

const LOG_ROTATE_BYTES: u64 = 10 * 1024 * 1024;
const START_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

pub const LAUNCHD_LABEL: &str = "com.ttm.server";

pub fn pid_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ttm.pid")
}

pub fn log_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ttm.log")
}

fn pid_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists and we may signal it.
    unsafe { libc::kill(pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

pub fn read_pid(data_dir: &Path) -> Option<i32> {
    fs::read_to_string(pid_path(data_dir)).ok()?.trim().parse().ok().filter(|p| *p > 0)
}

/// The pid of a live daemon, if there is one.
pub fn running_pid(data_dir: &Path) -> Option<i32> {
    read_pid(data_dir).filter(|p| *p != std::process::id() as i32 && pid_alive(*p))
}

/// Held for the life of `serve`: refuses a second daemon and cleans up on exit.
#[derive(Debug)]
pub struct PidGuard {
    path: PathBuf,
}

impl PidGuard {
    pub fn acquire(data_dir: &Path) -> Result<Self> {
        if let Some(pid) = running_pid(data_dir) {
            bail!("ttm is already running (pid {pid}); stop it with `ttm stop`");
        }
        let path = pid_path(data_dir);
        fs::write(&path, std::process::id().to_string()).with_context(|| format!("writing {}", path.display()))?;
        Ok(Self { path })
    }
}

impl Drop for PidGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

// ------------------------------------------------------------------ start

fn rotate_log(path: &Path) {
    if fs::metadata(path).is_ok_and(|m| m.len() > LOG_ROTATE_BYTES) {
        let _ = fs::rename(path, path.with_extension("log.1"));
    }
}

fn last_lines(path: &Path, n: usize) -> Vec<String> {
    let Ok(mut f) = File::open(path) else { return Vec::new() };
    let len = f.metadata().map_or(0, |m| m.len());
    // Read a generous tail rather than the whole file.
    let start = len.saturating_sub(256 * 1024);
    let mut buf = String::new();
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_to_string(&mut buf).is_err() {
        // A cut in the middle of a multi-byte character: fall back to lossy decoding.
        let mut bytes = Vec::new();
        let _ = f.seek(SeekFrom::Start(start));
        let _ = f.read_to_end(&mut bytes);
        buf = String::from_utf8_lossy(&bytes).into_owned();
    }
    let lines: Vec<&str> = buf.lines().collect();
    lines[lines.len().saturating_sub(n)..].iter().map(|s| (*s).to_owned()).collect()
}

pub async fn start(config_path: Option<&Path>, cfg: &Config) -> Result<()> {
    let data_dir = cfg.data_dir()?;
    let sock = cfg.socket_path()?;
    if let Some(pid) = running_pid(&data_dir) {
        println!("ttm is already running (pid {pid})");
        return Ok(());
    }
    let log = log_path(&data_dir);
    rotate_log(&log);
    let out = OpenOptions::new().create(true).append(true).open(&log).with_context(|| format!("opening {}", log.display()))?;
    let err = out.try_clone()?;

    let mut cmd = Command::new(std::env::current_exe().context("locating the ttm executable")?);
    cmd.arg("serve");
    if let Some(p) = config_path {
        cmd.arg("--config").arg(fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    }
    cmd.stdin(Stdio::null()).stdout(out).stderr(err);
    // SAFETY: setsid is async-signal-safe and touches no memory; it detaches the child from
    // our terminal so it survives this shell closing.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().context("starting the daemon")?;

    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if control::call(&sock, &Request::Ping).await.is_ok() {
            println!("ttm started (pid {}), listening on http://{}", child.id(), cfg.bind);
            println!("log: {}", log.display());
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            let tail = last_lines(&log, 12).join("\n");
            bail!("ttm exited right away ({status}). Last log lines:\n{tail}");
        }
        if Instant::now() >= deadline {
            bail!("ttm did not become ready within {}s; see {}", START_TIMEOUT.as_secs(), log.display());
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

// ------------------------------------------------------------------- stop

pub async fn stop(cfg: &Config, kill_sessions: bool, force: bool) -> Result<()> {
    let data_dir = cfg.data_dir()?;
    let sock = cfg.socket_path()?;
    let Some(pid) = running_pid(&data_dir) else {
        println!("ttm is not running");
        return Ok(());
    };
    if !force
        && let Ok(s) = control::call(&sock, &Request::Status).await
        && s["managed"].as_bool() == Some(true)
    {
        bail!(
            "ttm is run by a service manager, which would just restart it.\n\
             Stop it with `systemctl --user stop ttm` (Linux) or \
             `launchctl bootout gui/$(id -u)/{LAUNCHD_LABEL}` (macOS), or pass --force"
        );
    }
    // Ask nicely first so scrollback is saved; fall back to SIGTERM.
    let asked = control::call(&sock, &Request::Shutdown { kill_sessions }).await.is_ok();
    if !asked {
        // SAFETY: plain signal send to a pid we just read from our own pidfile.
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    let deadline = Instant::now() + STOP_TIMEOUT;
    while pid_alive(pid) {
        if Instant::now() >= deadline {
            bail!("ttm (pid {pid}) did not stop within {}s", STOP_TIMEOUT.as_secs());
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    println!("ttm stopped{}", if kill_sessions { " (and its sessions)" } else { "; sessions keep running in tmux" });
    Ok(())
}

// ----------------------------------------------------------------- status

pub async fn status(cfg: &Config) -> Result<bool> {
    let data_dir = cfg.data_dir()?;
    let sock = cfg.socket_path()?;
    match control::call(&sock, &Request::Status).await {
        Ok(s) => {
            let secs = s["uptime_secs"].as_u64().unwrap_or(0);
            println!("ttm {} is running (pid {})", s["version"].as_str().unwrap_or("?"), s["pid"]);
            println!("  listening   http://{}", s["bind"].as_str().unwrap_or("?"));
            println!("  uptime      {}", human_duration(secs));
            println!(
                "  sessions    {} running, {} exited, {} stopped",
                s["sessions"]["running"], s["sessions"]["exited"], s["sessions"]["stopped"]
            );
            println!("  push        {} device(s)", s["push_devices"]);
            println!("  managed     {}", if s["managed"].as_bool() == Some(true) { "by a service manager" } else { "no (self-started)" });
            println!("  data        {}", data_dir.display());
            println!("  log         {}", log_path(&data_dir).display());
            Ok(true)
        }
        Err(_) => {
            match running_pid(&data_dir) {
                Some(pid) => println!("ttm process {pid} exists but is not answering on {}", sock.display()),
                None => println!("ttm is not running"),
            }
            Ok(false)
        }
    }
}

pub fn human_duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {}s", secs / 60, secs % 60),
        3600..=86399 => format!("{}h {}m", secs / 3600, (secs % 3600) / 60),
        _ => format!("{}d {}h", secs / 86400, (secs % 86400) / 3600),
    }
}

// ------------------------------------------------------------------- logs

pub async fn logs(cfg: &Config, lines: usize, follow: bool) -> Result<()> {
    let log = log_path(&cfg.data_dir()?);
    if !log.exists() {
        println!("no log file at {}", log.display());
        println!("if ttm runs under systemd: journalctl --user -u ttm -f");
        return Ok(());
    }
    for l in last_lines(&log, lines) {
        println!("{l}");
    }
    if !follow {
        return Ok(());
    }
    let mut f = File::open(&log)?;
    let mut pos = f.seek(SeekFrom::End(0))?;
    loop {
        tokio::time::sleep(Duration::from_millis(400)).await;
        let len = fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
        if len < pos {
            // Rotated or truncated: start again from the top of the new file.
            f = File::open(&log)?;
            pos = 0;
        }
        if len > pos {
            f.seek(SeekFrom::Start(pos))?;
            let mut chunk = Vec::new();
            f.read_to_end(&mut chunk)?;
            pos += chunk.len() as u64;
            std::io::stdout().write_all(&chunk)?;
            std::io::stdout().flush()?;
        }
    }
}

// ---------------------------------------------------------------- service

/// Quote one argument for a systemd `ExecStart=` line (also doubling `%` specifiers).
pub fn systemd_quote(s: &str) -> String {
    let s = s.replace('%', "%%");
    if s.is_empty() || s.chars().any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | '$')) {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "$$"))
    } else {
        s
    }
}

pub struct ServiceSpec {
    pub exe: PathBuf,
    pub config: Option<PathBuf>,
    pub path_env: String,
    pub log: PathBuf,
    pub env_file: Option<PathBuf>,
}

pub fn systemd_unit(spec: &ServiceSpec) -> String {
    let mut exec = format!("{} serve", systemd_quote(&spec.exe.display().to_string()));
    if let Some(c) = &spec.config {
        exec.push_str(&format!(" --config {}", systemd_quote(&c.display().to_string())));
    }
    let env_file = spec
        .env_file
        .as_ref()
        .map(|p| format!("EnvironmentFile=-{}\n", systemd_quote(&p.display().to_string())))
        .unwrap_or_default();
    format!(
        "[Unit]\n\
         Description=ttm remote terminal for AI agents\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         ExecStart={exec}\n\
         {env_file}\
         Environment={path}\n\
         Environment=TTM_MANAGED=1\n\
         Restart=on-failure\n\
         RestartSec=3\n\
         # Only stop ttm itself: the tmux server and your agents must survive a restart.\n\
         KillMode=process\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        path = systemd_quote(&format!("PATH={}", spec.path_env)),
    )
}

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn launchd_plist(spec: &ServiceSpec) -> String {
    let mut args = vec![spec.exe.display().to_string(), "serve".into()];
    if let Some(c) = &spec.config {
        args.push("--config".into());
        args.push(c.display().to_string());
    }
    let args: String = args.iter().map(|a| format!("    <string>{}</string>\n", xml_escape(a))).collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20 <key>Label</key>\n  <string>{LAUNCHD_LABEL}</string>\n\
         \x20 <key>ProgramArguments</key>\n  <array>\n{args}  </array>\n\
         \x20 <key>EnvironmentVariables</key>\n  <dict>\n\
         \x20   <key>PATH</key>\n    <string>{path}</string>\n\
         \x20   <key>TTM_MANAGED</key>\n    <string>1</string>\n\
         \x20 </dict>\n\
         \x20 <key>RunAtLoad</key>\n  <true/>\n\
         \x20 <key>KeepAlive</key>\n  <true/>\n\
         \x20 <!-- Do not kill the tmux server (and your agents) when ttm stops or restarts. -->\n\
         \x20 <key>AbandonProcessGroup</key>\n  <true/>\n\
         \x20 <key>StandardOutPath</key>\n  <string>{log}</string>\n\
         \x20 <key>StandardErrorPath</key>\n  <string>{log}</string>\n\
         </dict>\n\
         </plist>\n",
        path = xml_escape(&spec.path_env),
        log = xml_escape(&spec.log.display().to_string()),
    )
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).context("HOME is not set")
}

fn service_file() -> Result<PathBuf> {
    if cfg!(target_os = "macos") {
        Ok(home()?.join("Library/LaunchAgents").join(format!("{LAUNCHD_LABEL}.plist")))
    } else {
        let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or(home()?.join(".config"));
        Ok(base.join("systemd/user/ttm.service"))
    }
}

fn run_checked(cmd: &mut Command) -> Result<()> {
    let out = cmd.output().with_context(|| format!("running {:?}", cmd.get_program()))?;
    if !out.status.success() {
        bail!("{:?} failed: {}", cmd.get_program(), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

pub async fn install_service(config_path: Option<&Path>, cfg: &Config, now: bool) -> Result<()> {
    let data_dir = cfg.data_dir()?;
    let spec = ServiceSpec {
        exe: std::env::current_exe().context("locating the ttm executable")?,
        config: config_path.map(|p| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())),
        // Capture *this shell's* PATH: that is where your agents and tools are.
        path_env: std::env::var("PATH").unwrap_or_default(),
        log: log_path(&data_dir),
        env_file: dirs_config().map(|d| d.join("ttm/env")),
    };
    let file = service_file()?;
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let body = if cfg!(target_os = "macos") { launchd_plist(&spec) } else { systemd_unit(&spec) };
    fs::write(&file, body).with_context(|| format!("writing {}", file.display()))?;
    println!("wrote {}", file.display());

    if !now {
        if cfg!(target_os = "macos") {
            println!("load it with:  launchctl bootstrap gui/{} {}", uid(), file.display());
        } else {
            println!("enable it with:  systemctl --user daemon-reload && systemctl --user enable --now ttm");
            println!("keep it running after you log out:  loginctl enable-linger $USER");
        }
        return Ok(());
    }
    if let Some(pid) = running_pid(&data_dir) {
        bail!("a self-started ttm is running (pid {pid}); run `ttm stop` first so the service can take over the port");
    }
    if cfg!(target_os = "macos") {
        let target = format!("gui/{}", uid());
        // Reloading an already-loaded job fails, so unload first (ignore "not loaded").
        let _ = Command::new("launchctl").args(["bootout", &format!("{target}/{LAUNCHD_LABEL}")]).output();
        run_checked(Command::new("launchctl").args(["bootstrap", &target]).arg(&file))?;
    } else {
        run_checked(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
        run_checked(Command::new("systemctl").args(["--user", "enable", "--now", "ttm"]))?;
        println!("tip: `loginctl enable-linger $USER` keeps it running after you log out");
    }
    println!("service started; check with `ttm status`");
    Ok(())
}

fn dirs_config() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home().ok().map(|h| h.join(".config")))
}

pub async fn uninstall_service() -> Result<()> {
    let file = service_file()?;
    if cfg!(target_os = "macos") {
        let _ = Command::new("launchctl").args(["bootout", &format!("gui/{}/{LAUNCHD_LABEL}", uid())]).output();
    } else {
        let _ = Command::new("systemctl").args(["--user", "disable", "--now", "ttm"]).output();
    }
    match fs::remove_file(&file) {
        Ok(()) => println!("removed {}", file.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => println!("no service file at {}", file.display()),
        Err(e) => return Err(e).with_context(|| format!("removing {}", file.display())),
    }
    if !cfg!(target_os = "macos") {
        let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).output();
    }
    println!("sessions keep running in tmux");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ServiceSpec {
        ServiceSpec {
            exe: "/opt/my apps/ttm".into(),
            config: Some("/home/me/.config/ttm/config.toml".into()),
            path_env: "/home/me/.local/bin:/usr/bin".into(),
            log: "/home/me/.local/share/ttm/ttm.log".into(),
            env_file: Some("/home/me/.config/ttm/env".into()),
        }
    }

    #[test]
    fn systemd_quoting() {
        assert_eq!(systemd_quote("/usr/bin/ttm"), "/usr/bin/ttm");
        assert_eq!(systemd_quote("/opt/my apps/ttm"), "\"/opt/my apps/ttm\"");
        assert_eq!(systemd_quote("100%"), "100%%");
        assert_eq!(systemd_quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(systemd_quote("$HOME"), "\"$$HOME\"");
    }

    #[test]
    fn unit_has_the_important_lines() {
        let u = systemd_unit(&spec());
        assert!(u.contains("ExecStart=\"/opt/my apps/ttm\" serve --config /home/me/.config/ttm/config.toml\n"), "{u}");
        assert!(u.contains("Environment=PATH=/home/me/.local/bin:/usr/bin\n"));
        assert!(u.contains("Environment=TTM_MANAGED=1\n"));
        assert!(u.contains("KillMode=process\n"));
        assert!(u.contains("EnvironmentFile=-/home/me/.config/ttm/env\n"));
    }

    #[test]
    fn plist_is_escaped_and_complete() {
        let mut s = spec();
        s.path_env = "/a&b:/c<d".into();
        let p = launchd_plist(&s);
        assert!(p.contains("<string>/a&amp;b:/c&lt;d</string>"), "{p}");
        assert!(p.contains("<string>/opt/my apps/ttm</string>"));
        assert!(p.contains("<key>AbandonProcessGroup</key>\n  <true/>"));
        assert!(p.contains("<key>TTM_MANAGED</key>"));
        assert!(p.contains(LAUNCHD_LABEL));
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(human_duration(5), "5s");
        assert_eq!(human_duration(125), "2m 5s");
        assert_eq!(human_duration(7260), "2h 1m");
        assert_eq!(human_duration(90000), "1d 1h");
    }

    #[test]
    fn last_lines_tails_a_file() {
        let p = std::env::temp_dir().join(format!("ttm-log-{}", std::process::id()));
        fs::write(&p, "a\nb\nc\nd\n").unwrap();
        assert_eq!(last_lines(&p, 2), vec!["c", "d"]);
        assert_eq!(last_lines(&p, 10).len(), 4);
        fs::remove_file(&p).ok();
        assert!(last_lines(&p, 3).is_empty());
    }

    #[test]
    fn pidfile_guard_blocks_a_second_instance_and_cleans_up() {
        let dir = std::env::temp_dir().join(format!("ttm-pid-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // A stale pidfile (no such process) is taken over.
        fs::write(pid_path(&dir), "999999").unwrap();
        let g = PidGuard::acquire(&dir).unwrap();
        assert_eq!(read_pid(&dir), Some(std::process::id() as i32));
        drop(g);
        assert!(!pid_path(&dir).exists());
        // A live foreign pid (pid 1) blocks it.
        fs::write(pid_path(&dir), "1").unwrap();
        assert!(PidGuard::acquire(&dir).unwrap_err().to_string().contains("already running"));
        fs::remove_dir_all(&dir).ok();
    }
}
