//! What a long piece of work is doing right now, for a page to show while it waits: each lookup and each call to a model,
//! when it began, whether it finished and what it found. The page polls the list by the id it chose for the work.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;

/// How many pieces of work are remembered. The oldest are forgotten first.
const KEEP: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub id: u32,
    /// Which step of the work it belongs to (1, 2, 3).
    pub step: u8,
    /// What is being done: "Reading your Claude plan usage".
    pub label: String,
    /// Who is being asked, when it is a call out: "claude -p · sonnet", "api.openrouter.ai".
    #[serde(skip_serializing_if = "String::is_empty")]
    pub target: String,
    /// What came of it, once it has.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    pub state: State,
    /// Milliseconds after the work began, when this began and when it ended.
    pub began: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended: Option<u64>,
}

struct Work {
    since: Instant,
    items: Vec<Item>,
}

#[derive(Default)]
pub struct Activity {
    inner: Mutex<(HashMap<String, Work>, VecDeque<String>)>,
}

impl Activity {
    /// A handle to record into the work called `id`. With no id the handle records nothing.
    pub fn track(self: &Arc<Self>, id: Option<&str>) -> Tracker {
        let id = id.map(str::trim).filter(|s| !s.is_empty() && s.len() <= 64).map(str::to_owned);
        if let Some(id) = &id {
            let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if !g.0.contains_key(id) {
                g.0.insert(id.clone(), Work { since: Instant::now(), items: Vec::new() });
                g.1.push_back(id.clone());
                while g.1.len() > KEEP {
                    if let Some(old) = g.1.pop_front() {
                        g.0.remove(&old);
                    }
                }
            }
        }
        Tracker { at: self.clone(), id }
    }

    /// Everything recorded for the work, and how long ago it began, in milliseconds.
    pub fn read(&self, id: &str) -> Option<(Vec<Item>, u64)> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.0.get(id).map(|w| (w.items.clone(), w.since.elapsed().as_millis() as u64))
    }
}

#[derive(Clone)]
pub struct Tracker {
    at: Arc<Activity>,
    id: Option<String>,
}

impl Tracker {
    /// A tracker that records nothing.
    pub fn none() -> Tracker {
        Tracker { at: Arc::new(Activity::default()), id: None }
    }

    /// The name of the work, when it has one.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Note that something has begun.
    pub fn begin(&self, step: u8, label: &str, target: &str) -> Step {
        let Some(id) = &self.id else { return Step { t: None, n: 0 } };
        let mut g = self.at.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(w) = g.0.get_mut(id) else { return Step { t: None, n: 0 } };
        let n = w.items.len() as u32;
        let began = w.since.elapsed().as_millis() as u64;
        w.items.push(Item { id: n, step, label: label.to_owned(), target: target.to_owned(), detail: String::new(), state: State::Running, began, ended: None });
        Step { t: Some(self.clone()), n }
    }
}

/// One thing under way. Finish it with `done` or `fail`; one dropped unfinished is left running.
pub struct Step {
    t: Option<Tracker>,
    n: u32,
}

impl Step {
    pub fn done(self, detail: impl Into<String>) {
        self.end(State::Done, detail.into());
    }

    pub fn fail(self, detail: impl Into<String>) {
        self.end(State::Failed, detail.into());
    }

    fn end(self, state: State, detail: String) {
        let Some(t) = &self.t else { return };
        let Some(id) = &t.id else { return };
        let mut g = t.at.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = g.0.get_mut(id) {
            let at = w.since.elapsed().as_millis() as u64;
            if let Some(i) = w.items.get_mut(self.n as usize) {
                i.state = state;
                i.detail = detail;
                i.ended = Some(at);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_recorded_in_order_with_what_came_of_them() {
        let a = Arc::new(Activity::default());
        let t = a.track(Some("w1"));
        let first = t.begin(1, "Reading plan usage", "");
        assert_eq!(a.read("w1").unwrap().0[0].state, State::Running);
        first.done("5h 12%");
        t.begin(1, "Asking Codex", "codex app-server").fail("not signed in");
        let (items, _) = a.read("w1").unwrap();
        assert_eq!(items.iter().map(|i| (i.label.as_str(), i.state, i.detail.as_str())).collect::<Vec<_>>(), [("Reading plan usage", State::Done, "5h 12%"), ("Asking Codex", State::Failed, "not signed in")]);
        assert!(items[0].ended.is_some() && items[1].target == "codex app-server");
    }

    #[test]
    fn a_tracker_without_an_id_records_nothing() {
        let a = Arc::new(Activity::default());
        a.track(None).begin(1, "x", "").done("y");
        a.track(Some("  ")).begin(1, "x", "").done("y");
        assert!(a.read("").is_none());
        Tracker::none().begin(1, "x", "").fail("y");
    }

    #[test]
    fn only_the_latest_pieces_of_work_are_kept() {
        let a = Arc::new(Activity::default());
        for i in 0..KEEP + 3 {
            a.track(Some(&format!("w{i}"))).begin(1, "x", "");
        }
        assert!(a.read("w0").is_none() && a.read("w2").is_none());
        assert!(a.read("w3").is_some() && a.read(&format!("w{}", KEEP + 2)).is_some());
        // The same id again carries on the same list.
        a.track(Some("w3")).begin(2, "y", "");
        assert_eq!(a.read("w3").unwrap().0.len(), 2);
    }
}
