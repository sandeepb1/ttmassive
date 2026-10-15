//! Flows and runs on disk: one JSON file each under `<data_dir>/orch/`. Written whole through a
//! temp file so a crash never leaves half a document. Writes are small and synchronous, because the
//! executor saves from inside its change callback.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::graph::{Flow, id_ok};
use super::run::Run;

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// What the run list shows, without every transcript.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub id: String,
    /// The flow this is an execution of. It may no longer be saved (deleted, or run before it was saved).
    pub flow_id: String,
    pub flow_name: String,
    pub goal: String,
    pub status: super::run::RunStatus,
    pub created: i64,
    /// Everything the agents read and wrote, cache included.
    pub tokens: u64,
}

impl Store {
    pub fn new(data_dir: &Path) -> Store {
        Store { root: data_dir.join("orch") }
    }

    pub fn worktrees(&self) -> PathBuf {
        self.root.join("worktrees")
    }

    fn file(&self, kind: &str, id: &str) -> Result<PathBuf> {
        if !id_ok(id) {
            bail!("bad id");
        }
        Ok(self.root.join(kind).join(format!("{id}.json")))
    }

    fn write<T: Serialize>(&self, kind: &str, id: &str, v: &T) -> Result<()> {
        let path = self.file(kind, id)?;
        fs::create_dir_all(path.parent().context("no parent")?)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Every readable document of a kind. An unreadable one is skipped, not fatal.
    fn read_all<T: serde::de::DeserializeOwned>(&self, kind: &str) -> Vec<T> {
        let Ok(dir) = fs::read_dir(self.root.join(kind)) else { return Vec::new() };
        dir.filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_slice(&fs::read(e.path()).ok()?).ok())
            .collect()
    }

    pub fn save_flow(&self, f: &Flow) -> Result<()> {
        f.validate().map_err(anyhow::Error::msg)?;
        self.write("flows", &f.id, f)
    }

    pub fn load_flow(&self, id: &str) -> Option<Flow> {
        serde_json::from_slice(&fs::read(self.file("flows", id).ok()?).ok()?).ok()
    }

    pub fn list_flows(&self) -> Vec<Flow> {
        let mut v: Vec<Flow> = self.read_all("flows");
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        v
    }

    pub fn delete_flow(&self, id: &str) -> Result<()> {
        match fs::remove_file(self.file("flows", id)?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// The plan limits, or the defaults when none were saved (or the file cannot be read).
    pub fn load_limits(&self) -> super::graph::Budget {
        std::fs::read(self.file("limits", "global").unwrap_or_default())
            .ok()
            .and_then(|b| serde_json::from_slice::<super::graph::Budget>(&b).ok())
            .filter(|b| b.validate().is_ok())
            .unwrap_or_default()
    }

    pub fn save_limits(&self, b: &super::graph::Budget) -> Result<()> {
        b.validate().map_err(anyhow::Error::msg)?;
        self.write("limits", "global", b)
    }

    pub fn save_run(&self, r: &Run) -> Result<()> {
        self.write("runs", &r.id, r)
    }

    pub fn load_run(&self, id: &str) -> Option<Run> {
        serde_json::from_slice(&fs::read(self.file("runs", id).ok()?).ok()?).ok()
    }

    pub fn delete_run(&self, id: &str) -> Result<()> {
        match fs::remove_file(self.file("runs", id)?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    pub fn all_runs(&self) -> Vec<Run> {
        self.read_all("runs")
    }

    /// Newest first.
    pub fn list_runs(&self) -> Vec<RunSummary> {
        let mut v: Vec<RunSummary> = self
            .all_runs()
            .into_iter()
            .map(|r| {
                let tokens = r.tokens().total();
                RunSummary { id: r.id, flow_id: r.flow.id, flow_name: r.flow.name, goal: r.goal, status: r.status, created: r.created, tokens }
            })
            .collect();
        v.sort_by(|a, b| b.created.cmp(&a.created));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orch::run::RunStatus;
    use crate::orch::starters;

    /// A fresh temp folder that is removed when the test ends, pass or fail.
    struct Tmp(PathBuf);

    impl std::ops::Deref for Tmp {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp(name: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("ttm-orch-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }

    #[test]
    fn a_flow_is_saved_listed_loaded_and_deleted() {
        let dir = tmp("flow");
        let s = Store::new(&dir);
        assert!(s.list_flows().is_empty());
        let f = starters::all().remove(0);
        s.save_flow(&f).unwrap();
        assert_eq!(s.load_flow(&f.id), Some(f.clone()));
        assert_eq!(s.list_flows().len(), 1);
        s.delete_flow(&f.id).unwrap();
        assert_eq!(s.load_flow(&f.id), None);
        s.delete_flow(&f.id).unwrap(); // already gone is fine
    }

    #[test]
    fn an_invalid_flow_is_not_saved() {
        let dir = tmp("invalid");
        let s = Store::new(&dir);
        let mut f = starters::all().remove(0);
        f.nodes.clear();
        assert!(s.save_flow(&f).is_err());
        assert!(s.list_flows().is_empty());
    }

    #[test]
    fn ids_cannot_escape_the_folder() {
        let dir = tmp("ids");
        let s = Store::new(&dir);
        assert!(s.file("runs", "../../etc/passwd").is_err());
        assert!(s.file("runs", "a/b").is_err());
        assert!(s.file("runs", "").is_err());
        assert!(s.load_run("../x").is_none());
    }

    #[test]
    fn limits_default_until_saved_and_bad_ones_are_refused() {
        let dir = tmp("limits");
        let s = Store::new(&dir);
        assert_eq!(s.load_limits(), crate::orch::graph::Budget::default());
        let b = crate::orch::graph::Budget { claude_pct: 40.0, codex_pct: 30.0, stop_window_pct: 90.0 };
        s.save_limits(&b).unwrap();
        assert_eq!(s.load_limits(), b);
        assert!(s.save_limits(&crate::orch::graph::Budget { claude_pct: 0.0, ..b }).is_err());
        assert!(s.save_limits(&crate::orch::graph::Budget { codex_pct: 101.0, ..b }).is_err());
        assert_eq!(s.load_limits(), b, "a refused save leaves the old limits");
    }

    #[test]
    fn a_deleted_run_is_gone_and_deleting_twice_is_fine() {
        let dir = tmp("rundel");
        let s = Store::new(&dir);
        let flow = starters::all().remove(0);
        s.save_run(&Run::new("a".into(), flow.clone(), "x".into(), 1)).unwrap();
        s.save_run(&Run::new("b".into(), flow, "y".into(), 2)).unwrap();
        s.delete_run("a").unwrap();
        s.delete_run("a").unwrap();
        assert!(s.load_run("a").is_none());
        assert_eq!(s.list_runs().len(), 1);
        assert!(s.delete_run("../b").is_err());
    }

    #[test]
    fn runs_round_trip_and_list_newest_first() {
        let dir = tmp("runs");
        let s = Store::new(&dir);
        let flow = starters::all().remove(0);
        let mut old = Run::new("old".into(), flow.clone(), "first".into(), 10);
        old.status = RunStatus::Done;
        let new = Run::new("new".into(), flow, "second".into(), 20);
        s.save_run(&old).unwrap();
        s.save_run(&new).unwrap();
        assert_eq!(s.load_run("new"), Some(new));
        let list = s.list_runs();
        assert_eq!(list.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["new", "old"]);
        assert_eq!(list[1].goal, "first");
        assert_eq!(list[0].flow_id, "starter-feature", "a run says which flow it belongs to");
        assert_eq!(list[0].tokens, 0);
    }

    #[test]
    fn the_summary_carries_the_flow_id_and_token_total_under_the_names_the_page_reads() {
        let dir = tmp("summary");
        let s = Store::new(&dir);
        let mut r = Run::new("r1".into(), starters::all().remove(0), "g".into(), 5);
        r.overhead = crate::orch::usage::Tokens { input: 1, output: 2, cache_read: 3, cache_write: 4 };
        s.save_run(&r).unwrap();
        let v = serde_json::to_value(s.list_runs()).unwrap();
        assert_eq!(v[0]["flowId"], "starter-feature");
        assert_eq!(v[0]["tokens"], 10);
    }

    #[test]
    fn a_corrupt_file_is_skipped() {
        let dir = tmp("corrupt");
        let s = Store::new(&dir);
        s.save_flow(&starters::all().remove(0)).unwrap();
        fs::write(dir.join("orch/flows/broken.json"), "{ nope").unwrap();
        assert_eq!(s.list_flows().len(), 1);
    }
}
