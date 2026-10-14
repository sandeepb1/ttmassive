//! The git branch a folder is on, read straight from `.git/HEAD` so listing sessions never has to
//! start a `git` process.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

/// How far up from a folder to look for `.git`.
const MAX_DEPTH: usize = 16;

/// The branch checked out in the repository containing `dir`: its name, or the first seven
/// characters of the commit when HEAD is detached. `None` outside a repository.
pub fn branch(dir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(head_file(dir)?).ok()?;
    parse_head(head.trim())
}

/// `HEAD` of the repository containing `dir`, following the `.git` file a worktree has.
fn head_file(dir: &Path) -> Option<PathBuf> {
    let mut cur = Some(dir);
    for _ in 0..MAX_DEPTH {
        let d = cur?;
        let git = d.join(".git");
        if git.is_dir() {
            return Some(git.join("HEAD"));
        }
        if git.is_file() {
            // A worktree or submodule: `gitdir: <path to its own git folder>`.
            let text = std::fs::read_to_string(&git).ok()?;
            let target = text.trim().strip_prefix("gitdir:")?.trim();
            let target = Path::new(target);
            let abs = if target.is_absolute() { target.to_path_buf() } else { d.join(target) };
            return Some(abs.join("HEAD"));
        }
        cur = d.parent();
    }
    None
}

fn parse_head(head: &str) -> Option<String> {
    if let Some(r) = head.strip_prefix("ref:") {
        let r = r.trim();
        let name = r.strip_prefix("refs/heads/").unwrap_or_else(|| r.rsplit('/').next().unwrap_or(r));
        return Some(name.to_owned()).filter(|n| !n.is_empty());
    }
    (head.len() >= 7 && head.bytes().all(|b| b.is_ascii_hexdigit())).then(|| head[..7].to_owned())
}

/// A branch is looked up at most this often per folder.
const FRESH: Duration = Duration::from_secs(5);
/// How long a request waits for a lookup before it carries on without the answer.
const WAIT: Duration = Duration::from_millis(400);

type Lookup = dyn Fn(&Path) -> Option<String> + Send + Sync;

#[derive(Default)]
struct State {
    seen: HashMap<PathBuf, (Option<String>, Instant)>,
    /// Folders being looked up right now, which may never finish.
    pending: HashSet<PathBuf>,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The branch of each session's folder for the list, kept off the async runtime.
///
/// Reading `.git/HEAD` is an ordinary file open, and an open can block with no end (a folder the
/// system has not yet let this program into, a network or cloud drive that has gone away). Done on
/// the runtime, one such open froze the whole server, including new connections. So the read runs on
/// the blocking pool, a request waits only `WAIT` for it, and a folder with a lookup still going is
/// not looked up again: a stuck one holds one thread, and the list just goes without its branch.
#[derive(Clone)]
pub struct Branches {
    state: Arc<Mutex<State>>,
    lookup: Arc<Lookup>,
}

impl Branches {
    pub fn new() -> Self {
        Self::with(branch)
    }

    fn with(lookup: impl Fn(&Path) -> Option<String> + Send + Sync + 'static) -> Self {
        Self { state: Arc::default(), lookup: Arc::new(lookup) }
    }

    /// The branch of `dir`: a recent answer, a new one if it comes quickly, else the last one known.
    pub async fn get(&self, dir: &Path) -> Option<String> {
        {
            let mut st = lock(&self.state);
            let known = st.seen.get(dir).cloned();
            if let Some((b, at)) = &known
                && at.elapsed() < FRESH
            {
                return b.clone();
            }
            // Someone is already looking, and may be stuck: do not start another.
            if !st.pending.insert(dir.to_owned()) {
                return known.and_then(|(b, _)| b);
            }
        }
        let (state, lookup, path) = (self.state.clone(), self.lookup.clone(), dir.to_owned());
        let job = tokio::task::spawn_blocking(move || {
            let found = lookup(&path);
            let mut st = lock(&state);
            st.pending.remove(&path);
            st.seen.insert(path, (found.clone(), Instant::now()));
            found
        });
        match tokio::time::timeout(WAIT, job).await {
            Ok(Ok(found)) => found,
            // Still going, or it panicked: the last answer will do.
            _ => lock(&self.state).seen.get(dir).and_then(|(b, _)| b.clone()),
        }
    }

    /// `get` for several folders at once, so slow ones wait together rather than one after another.
    pub async fn get_all(&self, dirs: Vec<PathBuf>) -> HashMap<PathBuf, Option<String>> {
        let mut set = tokio::task::JoinSet::new();
        for d in dirs {
            let me = self.clone();
            set.spawn(async move {
                let b = me.get(&d).await;
                (d, b)
            });
        }
        let mut out = HashMap::new();
        while let Some(done) = set.join_next().await {
            if let Ok((d, b)) = done {
                out.insert(d, b);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ttm-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn head_text_is_read_as_a_branch_or_a_short_commit() {
        assert_eq!(parse_head("ref: refs/heads/main"), Some("main".into()));
        assert_eq!(parse_head("ref: refs/heads/feature/web-redesign"), Some("feature/web-redesign".into()));
        assert_eq!(parse_head("ref: refs/remotes/origin/dev"), Some("dev".into()));
        assert_eq!(parse_head("0123456789abcdef0123456789abcdef01234567"), Some("0123456".into()));
        assert_eq!(parse_head("not a head"), None);
        assert_eq!(parse_head(""), None);
    }

    #[test]
    fn a_repository_is_found_from_a_subfolder() {
        let d = temp("repo");
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::write(d.join(".git/HEAD"), "ref: refs/heads/trunk\n").unwrap();
        std::fs::create_dir_all(d.join("src/deep")).unwrap();
        assert_eq!(branch(&d), Some("trunk".into()));
        assert_eq!(branch(&d.join("src/deep")), Some("trunk".into()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_worktree_follows_its_gitdir_file() {
        let d = temp("worktree");
        let real = d.join("main/.git/worktrees/wt");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("HEAD"), "ref: refs/heads/hotfix\n").unwrap();
        std::fs::create_dir_all(d.join("wt")).unwrap();
        std::fs::write(d.join("wt/.git"), format!("gitdir: {}\n", real.display())).unwrap();
        assert_eq!(branch(&d.join("wt")), Some("hotfix".into()));
        // A relative gitdir is relative to the folder holding the `.git` file.
        std::fs::write(d.join("wt/.git"), "gitdir: ../main/.git/worktrees/wt\n").unwrap();
        assert_eq!(branch(&d.join("wt")), Some("hotfix".into()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn outside_a_repository_there_is_no_branch() {
        let d = temp("none");
        assert_eq!(branch(&d), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn a_lookup_that_never_returns_does_not_hold_up_the_list() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let r = release.clone();
        let b = Branches::with(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
            while !r.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10)); // an open that does not come back
            }
            Some("main".into())
        });
        let dir = PathBuf::from("/stuck/folder");

        let t = Instant::now();
        assert_eq!(b.get(&dir).await, None, "no answer yet, and no waiting for one");
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());

        let t = Instant::now();
        for _ in 0..20 {
            assert_eq!(b.get(&dir).await, None);
        }
        assert!(t.elapsed() < Duration::from_millis(500), "a folder already being looked up is not waited for again");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "and no new thread is spent on it");

        release.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(b.get(&dir).await, Some("main".into()), "once the open returns, the answer shows up");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "from what it found, not another lookup");
    }

    #[tokio::test]
    async fn answers_are_remembered_for_a_few_seconds() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let b = Branches::with(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
            Some("trunk".into())
        });
        let d = PathBuf::from("/a");
        assert_eq!(b.get(&d).await, Some("trunk".into()));
        assert_eq!(b.get(&d).await, Some("trunk".into()));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(b.get(Path::new("/b")).await, Some("trunk".into()), "another folder is its own lookup");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_stuck_folder_does_not_hide_the_others() {
        let b = Branches::with(|p| {
            if p.ends_with("stuck") {
                std::thread::sleep(Duration::from_secs(3));
            }
            Some(p.file_name().unwrap().to_string_lossy().into_owned())
        });
        let t = Instant::now();
        let got = b.get_all(vec!["/x/ok".into(), "/x/stuck".into(), "/x/fine".into()]).await;
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        assert_eq!(got[Path::new("/x/ok")], Some("ok".into()));
        assert_eq!(got[Path::new("/x/fine")], Some("fine".into()));
        assert_eq!(got[Path::new("/x/stuck")], None);
    }

    #[tokio::test]
    async fn a_real_repository_is_read_through_the_cache() {
        let d = temp("cache");
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::write(d.join(".git/HEAD"), "ref: refs/heads/dev\n").unwrap();
        assert_eq!(Branches::new().get(&d).await, Some("dev".into()));
        let _ = std::fs::remove_dir_all(&d);
    }
}
