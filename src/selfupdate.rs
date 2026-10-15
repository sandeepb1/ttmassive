//! Updating ttm from its GitHub Releases. The web UI is embedded in the binary, so installing a
//! release updates the server and the app together.
//!
//! The server asks GitHub which releases are newer than the running one, and installs one when a
//! signed-in client asks: download the tarball for this platform, check its SHA256, smoke-test the
//! new binary, swap it in (keeping the old one as `ttm.prev`) and exit with [`RESTART_CODE`] so the
//! service manager starts the new one. A start that keeps failing is rolled back to `ttm.prev`.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::AppState;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const TARGET: &str = env!("TTM_TARGET");
const DEFAULT_REPO: &str = "sandeepb1/ttmassive";
const DEFAULT_API: &str = "https://api.github.com";

/// Not 0, so a systemd unit with `Restart=on-failure` restarts too; launchd restarts on any exit.
pub const RESTART_CODE: i32 = 75;
/// How often the server asks GitHub on its own, and how old a result may be before a client's
/// visit asks again.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
const FRESH_FOR: Duration = Duration::from_secs(15 * 60);
/// A new binary that has not come up this many times in a row is replaced by the old one.
const MAX_ATTEMPTS: u32 = 2;
/// How long a new binary must keep serving before the update counts as good.
const HEALTHY_AFTER: Duration = Duration::from_secs(30);
const MAX_DOWNLOAD: u64 = 200 << 20;

// ---- versions and releases (pure) ----

/// `v1.2.3` or `1.2.3`; anything else (a prerelease like `1.2.3-rc1`, a name) is not a release.
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s.strip_prefix('v').unwrap_or(s).split('.');
    let v = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    it.next().is_none().then_some(v)
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Release {
    pub version: String,
    pub title: String,
    pub notes: String,
    pub url: String,
    #[serde(rename = "publishedAt")]
    pub published_at: String,
    #[serde(skip)]
    pub tarball: String,
    #[serde(skip)]
    pub checksum: String,
}

/// Releases newer than `current` that ship a build for `target`, newest first.
fn parse_releases(list: &Value, current: &str, target: &str) -> Vec<Release> {
    let Some(have) = parse_version(current) else { return vec![] };
    let mut out: Vec<((u64, u64, u64), Release)> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false) && !r["prerelease"].as_bool().unwrap_or(false))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?;
            let ver = parse_version(tag)?;
            if ver <= have {
                return None;
            }
            let version = tag.trim_start_matches('v').to_string();
            let want = format!("ttm-v{version}-{target}.tar.gz");
            let asset = |name: &str| {
                r["assets"].as_array()?.iter().find(|a| a["name"] == name)?["browser_download_url"].as_str().map(String::from)
            };
            Some((
                ver,
                Release {
                    title: r["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(tag).to_string(),
                    notes: r["body"].as_str().unwrap_or("").to_string(),
                    url: r["html_url"].as_str().unwrap_or("").to_string(),
                    published_at: r["published_at"].as_str().unwrap_or("").to_string(),
                    tarball: asset(&want)?,
                    checksum: asset(&format!("{want}.sha256"))?,
                    version,
                },
            ))
        })
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.0));
    out.into_iter().map(|(_, r)| r).collect()
}

/// The hash in a `.sha256` file: `<64 hex>  <name>` or just the hash.
fn parse_sha256(text: &str) -> Option<String> {
    let h = text.split_whitespace().next()?.to_ascii_lowercase();
    (h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())).then_some(h)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---- rollback bookkeeping (pure decision, small files) ----

/// Written just before the swap, removed once the new binary has served for a while.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub from: String,
    pub to: String,
    pub attempts: u32,
}

#[derive(Debug, PartialEq)]
enum Boot {
    /// No update in flight, or a leftover from something else (a manual install).
    Normal,
    /// The first or second start of a freshly installed binary.
    Trial(Pending),
    /// It never came up: put the old binary back.
    RollBack(Pending),
}

fn boot_decision(pending: Option<Pending>, running: &str) -> Boot {
    match pending {
        Some(p) if p.to == running && p.attempts >= MAX_ATTEMPTS => Boot::RollBack(p),
        Some(p) if p.to == running => Boot::Trial(Pending { attempts: p.attempts + 1, ..p }),
        _ => Boot::Normal,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LastResult {
    ok: bool,
    from: String,
    to: String,
    at: u64,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn pending_file(data: &Path) -> PathBuf {
    data.join("update-pending.json")
}

fn last_file(data: &Path) -> PathBuf {
    data.join("update-last.json")
}

fn record(data: &Path, p: &Pending, ok: bool) {
    let last = LastResult { ok, from: p.from.clone(), to: p.to.clone(), at: now_secs() };
    if let Ok(s) = serde_json::to_string(&last) {
        let _ = std::fs::write(last_file(data), s);
    }
    let _ = std::fs::remove_file(pending_file(data));
}

fn prev_of(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".prev");
    exe.with_file_name(name)
}

/// Run first thing in `serve`. Counts this start of a just-installed binary, or, when it has
/// failed too often, puts the old one back and exits so the service manager starts that instead.
pub fn on_boot(data: &Path) -> Option<Pending> {
    let pending: Option<Pending> =
        std::fs::read_to_string(pending_file(data)).ok().and_then(|s| serde_json::from_str(&s).ok());
    match boot_decision(pending, CURRENT) {
        Boot::Normal => {
            // A sentinel for some other version is stale.
            let _ = std::fs::remove_file(pending_file(data));
            None
        }
        Boot::Trial(p) => {
            if let Ok(s) = serde_json::to_string(&p) {
                let _ = std::fs::write(pending_file(data), s);
            }
            Some(p)
        }
        Boot::RollBack(p) => {
            tracing::error!("ttm {} did not start {} times; restoring {}", p.to, p.attempts, p.from);
            let restored = std::env::current_exe().map_err(anyhow::Error::from).and_then(|exe| {
                std::fs::rename(prev_of(&exe), &exe).context("restoring ttm.prev")
            });
            if let Err(e) = &restored {
                tracing::error!("rollback failed: {e:#}");
            }
            record(data, &p, false);
            if restored.is_ok() {
                std::process::exit(RESTART_CODE);
            }
            None
        }
    }
}

/// After [`on_boot`] said this is a trial: once the server has been up a while, the update is good.
pub fn confirm_later(data: std::path::PathBuf, p: Pending) {
    tokio::spawn(async move {
        tokio::time::sleep(HEALTHY_AFTER).await;
        record(&data, &p, true);
        tracing::info!("update to {} confirmed", p.to);
    });
}

// ---- checking GitHub ----

#[derive(Default)]
struct Cache {
    releases: Vec<Release>,
    checked: Option<(Instant, u64)>,
    error: Option<String>,
}

#[derive(Default)]
pub struct Updater {
    cache: tokio::sync::Mutex<Cache>,
    installing: AtomicBool,
    restart: AtomicBool,
}

fn repo() -> String {
    std::env::var("TTM_UPDATE_REPO").unwrap_or_else(|_| DEFAULT_REPO.into())
}

/// Overridable so tests can point at a fixture server.
fn api_base() -> String {
    std::env::var("TTM_UPDATE_API").unwrap_or_else(|_| DEFAULT_API.into())
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent(format!("ttm/{CURRENT}"))
        .build()?)
}

/// Why this server cannot replace itself, or the path of the binary it would replace.
fn install_target() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate the ttm binary: {e}"))?;
    // launchd names the job in XPC_SERVICE_NAME and is the parent of its jobs; systemd sets INVOCATION_ID.
    // Pointing the update check at another server (`TTM_UPDATE_API`) is only done in tests, which have neither.
    let by_service = std::env::var("XPC_SERVICE_NAME").is_ok_and(|v| v == "com.ttm.server")
        || (cfg!(target_os = "macos") && std::os::unix::process::parent_id() == 1)
        || std::env::var_os("INVOCATION_ID").is_some()
        || std::env::var_os("TTM_UPDATE_API").is_some();
    if !by_service {
        return Err("ttm is not running under launchd or systemd, so nothing would restart it. Update with `make install` and restart it yourself.".into());
    }
    let dir = exe.parent().ok_or("the ttm binary has no folder")?;
    let c = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: `c` is a valid NUL-terminated path.
    if unsafe { libc::access(c.as_ptr(), libc::W_OK) } != 0 {
        return Err(format!("{} is not writable by this user. Update with `make install`.", dir.display()));
    }
    Ok(exe)
}

impl Updater {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn restart_requested(&self) -> bool {
        self.restart.load(Ordering::Relaxed)
    }

    /// Ask GitHub unless the last answer is recent (and `force` is off).
    async fn refresh(&self, force: bool) {
        let mut c = self.cache.lock().await;
        if !force && c.checked.is_some_and(|(t, _)| t.elapsed() < FRESH_FOR) {
            return;
        }
        let url = format!("{}/repos/{}/releases?per_page=30", api_base(), repo());
        let got = async {
            let list: Value = client()?
                .get(url)
                .timeout(Duration::from_secs(15))
                .header("Accept", "application/vnd.github+json")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            anyhow::Ok(list)
        }
        .await;
        c.checked = Some((Instant::now(), now_secs()));
        match got {
            Ok(list) => {
                c.releases = parse_releases(&list, CURRENT, TARGET);
                c.error = None;
            }
            Err(e) => {
                tracing::warn!("checking for updates: {e:#}");
                c.error = Some(e.to_string());
            }
        }
    }

    pub async fn status(&self, st: &AppState, force: bool) -> Value {
        self.refresh(force).await;
        let c = self.cache.lock().await;
        let (can, why) = match install_target() {
            Ok(_) => (true, None),
            Err(why) => (false, Some(why)),
        };
        let last: Option<LastResult> =
            std::fs::read_to_string(last_file(&st.data_dir)).ok().and_then(|s| serde_json::from_str(&s).ok());
        json!({
            "current": CURRENT,
            "latest": c.releases.first().map(|r| &r.version),
            "releases": c.releases,
            "checkedAt": c.checked.map(|(_, t)| t),
            "error": c.error,
            "canInstall": can,
            "cannotInstallReason": why,
            "installing": self.installing.load(Ordering::Relaxed),
            "lastResult": last,
        })
    }

    /// Download, verify and swap in `version`, then ask `serve` to shut down and restart.
    pub async fn install(&self, st: &AppState, version: &str) -> Result<()> {
        let exe = install_target().map_err(anyhow::Error::msg)?;
        let rel = {
            let c = self.cache.lock().await;
            c.releases.iter().find(|r| r.version == version).cloned()
        }
        .with_context(|| format!("{version} is not a known newer release; check for updates first"))?;
        if self.installing.swap(true, Ordering::SeqCst) {
            bail!("an update is already being installed");
        }
        let done = self.install_inner(st, &exe, &rel).await;
        if done.is_err() {
            self.installing.store(false, Ordering::SeqCst);
        }
        done
    }

    async fn install_inner(&self, st: &AppState, exe: &Path, rel: &Release) -> Result<()> {
        let testing = std::env::var_os("TTM_UPDATE_API").is_some();
        let trusted = format!("https://github.com/{}/releases/download/", repo());
        if !(testing || (rel.tarball.starts_with(&trusted) && rel.checksum.starts_with(&trusted))) {
            bail!("refusing to download from outside {trusted}");
        }
        let http = client()?;
        let sum_text = http.get(&rel.checksum).send().await?.error_for_status()?.text().await?;
        let want = parse_sha256(&sum_text).context("the checksum file is not a SHA256")?;
        let res = http.get(&rel.tarball).send().await?.error_for_status()?;
        if res.content_length().is_some_and(|n| n > MAX_DOWNLOAD) {
            bail!("the download is unexpectedly large");
        }
        let bytes = res.bytes().await?;
        if bytes.len() as u64 > MAX_DOWNLOAD {
            bail!("the download is unexpectedly large");
        }
        let got = hex(&Sha256::digest(&bytes));
        if got != want {
            bail!("checksum mismatch (expected {want}, got {got}); not installing");
        }
        let (exe, data, version) = (exe.to_path_buf(), st.data_dir.clone(), rel.version.clone());
        let work = tokio::task::spawn_blocking(move || swap_in(&bytes, &exe, &data, &version));
        tokio::time::timeout(Duration::from_secs(60), work).await.context("installing took too long")???;
        tracing::info!("installed ttm {}; restarting", rel.version);
        self.restart.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// Shut `serve` down shortly, so the response to the install request still gets out.
pub fn restart_soon(st: std::sync::Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        st.shutdown.notify_one();
    });
}

/// Blocking part: unpack, smoke-test, swap, leave the sentinel.
fn swap_in(tarball: &[u8], exe: &Path, data: &Path, version: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = exe.parent().context("the ttm binary has no folder")?;
    let work = dir.join(format!(".ttm-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir(&work).context("creating a work folder next to the binary")?;
    let result = (|| {
        let tgz = work.join("ttm.tar.gz");
        std::fs::write(&tgz, tarball)?;
        let out = std::process::Command::new("tar").arg("-xzf").arg(&tgz).arg("-C").arg(&work).arg("ttm").output().context("running tar")?;
        if !out.status.success() {
            bail!("unpacking failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        let new = work.join("ttm");
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
        let probe = std::process::Command::new(&new).arg("--version").output().context("running the new binary")?;
        let said = String::from_utf8_lossy(&probe.stdout);
        if !probe.status.success() || !said.contains(version) {
            bail!("the new binary did not report version {version} (it said {:?})", said.trim());
        }
        let prev = prev_of(exe);
        let _ = std::fs::remove_file(&prev);
        std::fs::rename(exe, &prev).context("moving the current binary aside")?;
        if let Err(e) = std::fs::rename(&new, exe) {
            let _ = std::fs::rename(&prev, exe);
            return Err(anyhow::Error::from(e).context("putting the new binary in place"));
        }
        let pending = Pending { from: CURRENT.into(), to: version.into(), attempts: 0 };
        std::fs::write(pending_file(data), serde_json::to_string(&pending)?)?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Check on startup and then every few hours, so a client opening later has an answer ready.
pub async fn check_loop(st: std::sync::Arc<AppState>) {
    tokio::time::sleep(Duration::from_secs(10)).await;
    loop {
        st.update.refresh(true).await;
        tokio::time::sleep(CHECK_EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, target: &str) -> Value {
        let name = format!("ttm-{tag}-{target}.tar.gz");
        json!({
            "tag_name": tag, "name": format!("{tag} — title"), "body": "- a change", "draft": false, "prerelease": false,
            "html_url": "https://github.com/x/y/releases/tag/".to_string() + tag, "published_at": "2026-10-15T00:00:00Z",
            "assets": [
                {"name": name, "browser_download_url": format!("https://github.com/x/y/releases/download/{tag}/{name}")},
                {"name": format!("{name}.sha256"), "browser_download_url": format!("https://github.com/x/y/releases/download/{tag}/{name}.sha256")},
            ],
        })
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(parse_version("v0.10.0") > parse_version("0.9.9"));
        assert_eq!(parse_version("1.2.3-rc1"), None);
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
    }

    #[test]
    fn newer_releases_for_this_platform_newest_first() {
        let list = json!([release("v0.2.0", "t"), release("v0.3.0", "t"), release("v0.1.0", "t"), release("v0.4.0", "other")]);
        let got = parse_releases(&list, "0.1.0", "t");
        assert_eq!(got.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), ["0.3.0", "0.2.0"]);
        assert!(got[0].tarball.ends_with("ttm-v0.3.0-t.tar.gz"));
        assert!(got[0].checksum.ends_with(".sha256"));
    }

    #[test]
    fn drafts_and_prereleases_are_skipped() {
        let mut draft = release("v0.2.0", "t");
        draft["draft"] = json!(true);
        let mut pre = release("v0.3.0", "t");
        pre["prerelease"] = json!(true);
        assert!(parse_releases(&json!([draft, pre]), "0.1.0", "t").is_empty());
    }

    #[test]
    fn sha256_file_formats() {
        let h = "a".repeat(64);
        assert_eq!(parse_sha256(&format!("{h}  ttm.tar.gz\n")), Some(h.clone()));
        assert_eq!(parse_sha256(&h.to_uppercase()), Some(h));
        assert_eq!(parse_sha256("nope"), None);
    }

    fn pending(to: &str, attempts: u32) -> Option<Pending> {
        Some(Pending { from: "0.1.0".into(), to: to.into(), attempts })
    }

    #[test]
    fn a_failing_new_binary_is_rolled_back_on_the_third_start() {
        assert_eq!(boot_decision(None, "0.2.0"), Boot::Normal);
        assert_eq!(boot_decision(pending("0.2.0", 0), "0.2.0"), Boot::Trial(pending("0.2.0", 1).unwrap()));
        assert_eq!(boot_decision(pending("0.2.0", 1), "0.2.0"), Boot::Trial(pending("0.2.0", 2).unwrap()));
        assert_eq!(boot_decision(pending("0.2.0", 2), "0.2.0"), Boot::RollBack(pending("0.2.0", 2).unwrap()));
    }

    #[test]
    fn a_sentinel_for_another_version_is_ignored() {
        // The old binary is running again after a rollback, or someone installed by hand.
        assert_eq!(boot_decision(pending("0.2.0", 2), "0.1.0"), Boot::Normal);
    }

    #[test]
    fn prev_sits_next_to_the_binary() {
        assert_eq!(prev_of(Path::new("/a/b/ttm")), PathBuf::from("/a/b/ttm.prev"));
    }
}
