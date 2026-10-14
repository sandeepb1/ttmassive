//! Reading things off the disk for a request that comes round again and again (the session list is
//! polled every few seconds), without ever risking the server.
//!
//! A file open can block with no end: a folder the system has not let this program into yet, or a
//! network or cloud drive that has gone away. Done on the async runtime, one such open froze the
//! whole server, including new connections, because the worker that drives network I/O was the one
//! stuck. So the read runs on the blocking pool, a request waits only `wait` for it, an answer is
//! kept for `fresh`, and a key whose lookup is still going is not looked up again: a stuck folder
//! holds one thread, and its callers just go without the answer.

use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

struct State<K, V> {
    seen: HashMap<K, (V, Instant)>,
    /// Keys being looked up right now, which may never finish.
    pending: HashSet<K>,
}

pub struct Lookups<K, V> {
    state: Arc<Mutex<State<K, V>>>,
    fresh: Duration,
    wait: Duration,
}

impl<K, V> Clone for Lookups<K, V> {
    fn clone(&self) -> Self {
        Self { state: self.state.clone(), fresh: self.fresh, wait: self.wait }
    }
}

fn lock<K, V>(m: &Mutex<State<K, V>>) -> MutexGuard<'_, State<K, V>> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl<K: Hash + Eq + Clone + Send + 'static, V: Clone + Send + 'static> Lookups<K, V> {
    pub fn new(fresh: Duration, wait: Duration) -> Self {
        Self { state: Arc::new(Mutex::new(State { seen: HashMap::new(), pending: HashSet::new() })), fresh, wait }
    }

    /// The answer for `key`: a recent one, a new one if `read` comes back quickly, else the last one
    /// known, else `None`. `read` runs at most once at a time per key.
    pub async fn get(&self, key: K, read: impl FnOnce() -> V + Send + 'static) -> Option<V> {
        {
            let mut st = lock(&self.state);
            let known = st.seen.get(&key).cloned();
            if let Some((v, at)) = &known
                && at.elapsed() < self.fresh
            {
                return Some(v.clone());
            }
            // Someone is already reading, and may be stuck: do not start another.
            if !st.pending.insert(key.clone()) {
                return known.map(|(v, _)| v);
            }
        }
        let (state, k) = (self.state.clone(), key.clone());
        let job = tokio::task::spawn_blocking(move || {
            let v = read();
            let mut st = lock(&state);
            st.pending.remove(&k);
            st.seen.insert(k, (v.clone(), Instant::now()));
            v
        });
        match tokio::time::timeout(self.wait, job).await {
            Ok(Ok(v)) => Some(v),
            // Still going, or it panicked: the last answer will do.
            _ => lock(&self.state).seen.get(&key).map(|(v, _)| v.clone()),
        }
    }
}

/// Run `read` on the blocking pool for a request that is made once, not polled, giving up after
/// `limit`. The thread is not freed if the read never returns, but the server is.
pub async fn off<T: Send + 'static>(limit: Duration, read: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::time::timeout(limit, tokio::task::spawn_blocking(read)).await.ok()?.ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    fn lookups() -> Lookups<PathBuf, Option<String>> {
        Lookups::new(Duration::from_secs(5), Duration::from_millis(400))
    }

    #[tokio::test]
    async fn a_read_that_never_returns_does_not_hold_up_the_caller() {
        let calls = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(AtomicBool::new(false));
        let l = lookups();
        let key = PathBuf::from("/stuck/folder");
        let read = |calls: Arc<AtomicUsize>, release: Arc<AtomicBool>| {
            move || {
                calls.fetch_add(1, Ordering::SeqCst);
                while !release.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(10)); // an open that does not come back
                }
                Some("main".to_owned())
            }
        };

        let t = Instant::now();
        assert_eq!(l.get(key.clone(), read(calls.clone(), release.clone())).await, None, "no answer yet, and no waiting for one");
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());

        let t = Instant::now();
        for _ in 0..20 {
            assert_eq!(l.get(key.clone(), read(calls.clone(), release.clone())).await, None);
        }
        assert!(t.elapsed() < Duration::from_millis(500), "a key already being read is not waited for again");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "and no new thread is spent on it");

        release.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(l.get(key.clone(), read(calls.clone(), release.clone())).await, Some(Some("main".into())), "once the read returns, its answer shows up");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "from what it found, not another read");
    }

    #[tokio::test]
    async fn answers_are_remembered_for_a_few_seconds() {
        let calls = Arc::new(AtomicUsize::new(0));
        let l = lookups();
        let read = |c: Arc<AtomicUsize>| {
            move || {
                c.fetch_add(1, Ordering::SeqCst);
                Some("trunk".to_owned())
            }
        };
        assert_eq!(l.get("/a".into(), read(calls.clone())).await, Some(Some("trunk".into())));
        assert_eq!(l.get("/a".into(), read(calls.clone())).await, Some(Some("trunk".into())));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(l.get("/b".into(), read(calls.clone())).await.is_some(), "another key is its own read");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_old_answer_is_read_again() {
        let calls = Arc::new(AtomicUsize::new(0));
        let l: Lookups<u8, usize> = Lookups::new(Duration::from_millis(30), Duration::from_millis(400));
        let c = calls.clone();
        let n = move || c.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(l.get(1, n.clone()).await, Some(1));
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(l.get(1, n).await, Some(2));
    }

    #[tokio::test]
    async fn a_stuck_key_gives_its_last_answer() {
        let l: Lookups<u8, &'static str> = Lookups::new(Duration::from_millis(10), Duration::from_millis(50));
        assert_eq!(l.get(1, || "first").await, Some("first"));
        tokio::time::sleep(Duration::from_millis(30)).await;
        // The next read hangs: callers keep getting what was known.
        assert_eq!(l.get(1, || { std::thread::sleep(Duration::from_millis(400)); "second" }).await, Some("first"));
        assert_eq!(l.get(1, || "never started").await, Some("first"));
    }

    #[tokio::test]
    async fn one_off_reads_give_up() {
        assert_eq!(off(Duration::from_millis(500), || 7).await, Some(7));
        let t = Instant::now();
        assert_eq!(off(Duration::from_millis(50), || { std::thread::sleep(Duration::from_millis(600)); 1 }).await, None);
        assert!(t.elapsed() < Duration::from_millis(400), "{:?}", t.elapsed());
        assert_eq!(off(Duration::from_millis(500), || -> u8 { panic!("boom") }).await, None, "a panic is an absent answer, not a crash");
    }
}
