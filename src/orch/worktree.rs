//! A git worktree for each node that writes files, so builders never touch the user's checkout and
//! two builders never touch each other. The branch and folder are kept after the run for the user
//! to look at and merge; nothing here deletes them.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tokio::process::Command;

use super::graph::id_ok;

pub fn branch_name(run_id: &str, node_id: &str) -> String {
    format!("ttm-orch/{run_id}-{node_id}")
}

/// Where a node's worktree is (or will be), without creating it.
pub fn path(base: &Path, run_id: &str, node_id: &str) -> PathBuf {
    base.join(run_id).join(node_id)
}

async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .context("could not run git")?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Whether a folder is ready for builders, which work on copies made from its latest commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RepoState {
    Ready,
    /// A git repository with nothing committed yet.
    NoCommits,
    NotARepo,
}

pub async fn repo_state(project: &Path) -> RepoState {
    if git(project, &["rev-parse", "--is-inside-work-tree"]).await.is_err() {
        RepoState::NotARepo
    } else if git(project, &["rev-parse", "--verify", "HEAD"]).await.is_err() {
        RepoState::NoCommits
    } else {
        RepoState::Ready
    }
}

/// Make a folder ready: `git init` if it is not a repository, then commit what is in it so builders have something to
/// copy. Does nothing to a repository that already has a commit. Everything in the folder that its ignore rules do not
/// exclude goes into that first commit.
pub async fn init(project: &Path) -> Result<RepoState> {
    let state = repo_state(project).await;
    if state == RepoState::Ready {
        return Ok(state);
    }
    if state == RepoState::NotARepo {
        git(project, &["init"]).await?;
    }
    git(project, &["add", "-A"]).await?;
    let mut args = identity(project).await;
    args.extend(["commit", "--allow-empty", "--no-verify", "-m", "Starting point, made by ttm so agents can work on copies"]);
    git(project, &args).await?;
    Ok(repo_state(project).await)
}

/// A machine with no git identity would refuse to commit, so give ttm's own commits a stand-in.
async fn identity(dir: &Path) -> Vec<&'static str> {
    let has = git(dir, &["config", "user.email"]).await.is_ok_and(|e| !e.is_empty());
    if has { vec![] } else { vec!["-c", "user.name=ttm", "-c", "user.email=ttm@localhost"] }
}

/// Commit everything a builder changed in its copy, so its branch holds the work: the diff and merge commands the
/// page offers, and the user's own `git merge`, depend on it. `node_modules` is left out even when the project does
/// not ignore it. Returns whether anything was committed.
pub async fn commit_all(dir: &Path, message: &str) -> Result<bool> {
    git(dir, &["add", "-A", "--", ".", ":(exclude,glob)**/node_modules/**"]).await?;
    if git(dir, &["diff", "--cached", "--name-only"]).await?.is_empty() {
        return Ok(false);
    }
    let mut args = identity(dir).await;
    args.extend(["commit", "--no-verify", "-m", message]);
    git(dir, &args).await?;
    Ok(true)
}

/// The worktree for `node_id` in `run_id`, created from the project's current commit. Asking again
/// for one that exists returns it, which is how a resumed run finds its builder's work.
pub async fn ensure(project: &Path, base: &Path, run_id: &str, node_id: &str) -> Result<PathBuf> {
    if !id_ok(run_id) || !id_ok(node_id) {
        bail!("bad id");
    }
    let dir = path(base, run_id, node_id);
    if dir.join(".git").exists() {
        return Ok(dir);
    }
    git(project, &["rev-parse", "--verify", "HEAD"])
        .await
        .context("the project must be a git repository with at least one commit")?;
    tokio::fs::create_dir_all(dir.parent().context("no parent")?).await?;
    let dir_str = dir.to_string_lossy().to_string();
    git(project, &["worktree", "add", "-b", &branch_name(run_id, node_id), &dir_str, "HEAD"]).await?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let d = std::env::temp_dir().join(format!("ttm-orch-wt-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }

    async fn repo(dir: &Path) {
        git(dir, &["init", "-q"]).await.unwrap();
        git(dir, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "first"])
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_builders_work_is_committed_to_its_branch_and_nothing_else_is() {
        let root = tmp("commit");
        let (project, base) = (root.join("project"), root.join("wt"));
        std::fs::create_dir_all(&project).unwrap();
        repo(&project).await;
        let dir = ensure(&project, &base, "r", "b").await.unwrap();
        std::fs::write(dir.join("app.ts"), "export {}").unwrap();
        std::fs::create_dir_all(dir.join("node_modules/x")).unwrap();
        std::fs::write(dir.join("node_modules/x/i.js"), "x").unwrap();
        assert!(commit_all(&dir, "Builder").await.unwrap());
        assert_eq!(git(&dir, &["ls-files"]).await.unwrap(), "app.ts", "dependencies stay out of the branch");
        // The work is on the branch, so the project can see and merge it.
        let shown = git(&project, &["diff", "--stat", "HEAD...ttm-orch/r-b"]).await.unwrap();
        assert!(shown.contains("app.ts"), "{shown}");
        // Nothing new: no empty commit.
        assert!(!commit_all(&dir, "Builder again").await.unwrap());
        assert_eq!(git(&dir, &["rev-list", "--count", "HEAD"]).await.unwrap(), "2");
        // Later changes make a second commit.
        std::fs::write(dir.join("more.ts"), "1").unwrap();
        assert!(commit_all(&dir, "Builder, second go").await.unwrap());
        git(&project, &["-c", "user.name=t", "-c", "user.email=t@t", "merge", "-q", "ttm-orch/r-b"]).await.unwrap();
        assert!(project.join("app.ts").exists() && project.join("more.ts").exists());
    }

    #[tokio::test]
    async fn a_plain_folder_is_set_up_with_its_files_in_a_first_commit() {
        let root = tmp("init");
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("main.txt"), "hello").unwrap();
        assert_eq!(repo_state(&project).await, RepoState::NotARepo);
        assert_eq!(init(&project).await.unwrap(), RepoState::Ready);
        assert_eq!(git(&project, &["ls-files"]).await.unwrap(), "main.txt");
        // Builders can now get a copy, and it has the file.
        let dir = ensure(&project, &root.join("wt"), "r", "b").await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("main.txt")).unwrap(), "hello");
    }

    #[tokio::test]
    async fn an_empty_folder_and_a_repository_with_no_commits_can_be_set_up_too() {
        let root = tmp("init2");
        let (empty, bare) = (root.join("empty"), root.join("fresh"));
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&bare).unwrap();
        assert_eq!(init(&empty).await.unwrap(), RepoState::Ready);
        git(&bare, &["init", "-q"]).await.unwrap();
        std::fs::write(bare.join("a.txt"), "x").unwrap();
        assert_eq!(repo_state(&bare).await, RepoState::NoCommits);
        assert_eq!(init(&bare).await.unwrap(), RepoState::Ready);
        assert_eq!(git(&bare, &["ls-files"]).await.unwrap(), "a.txt");
    }

    #[tokio::test]
    async fn setting_up_a_ready_repository_changes_nothing() {
        let root = tmp("init3");
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        repo(&project).await;
        std::fs::write(project.join("later.txt"), "x").unwrap();
        let before = git(&project, &["rev-parse", "HEAD"]).await.unwrap();
        assert_eq!(init(&project).await.unwrap(), RepoState::Ready);
        assert_eq!(git(&project, &["rev-parse", "HEAD"]).await.unwrap(), before);
        assert!(git(&project, &["status", "--porcelain"]).await.unwrap().contains("later.txt"), "uncommitted work is left alone");
    }

    #[tokio::test]
    async fn a_builder_gets_its_own_worktree_and_branch() {
        let root = tmp("make");
        let (project, base) = (root.join("project"), root.join("wt"));
        std::fs::create_dir_all(&project).unwrap();
        repo(&project).await;

        let dir = ensure(&project, &base, "run1", "build").await.unwrap();
        assert_eq!(dir, base.join("run1/build"));
        assert!(dir.join(".git").exists());
        let branches = git(&project, &["branch", "--list", "ttm-orch/*"]).await.unwrap();
        assert!(branches.contains("ttm-orch/run1-build"), "{branches}");
        // The main checkout is untouched: no branch switch, nothing new in it.
        assert!(git(&project, &["status", "--porcelain"]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn asking_again_returns_the_same_worktree() {
        let root = tmp("again");
        let (project, base) = (root.join("project"), root.join("wt"));
        std::fs::create_dir_all(&project).unwrap();
        repo(&project).await;
        let a = ensure(&project, &base, "r", "b").await.unwrap();
        std::fs::write(a.join("work.txt"), "kept").unwrap();
        let b = ensure(&project, &base, "r", "b").await.unwrap();
        assert_eq!(a, b);
        assert_eq!(std::fs::read_to_string(b.join("work.txt")).unwrap(), "kept");
    }

    #[tokio::test]
    async fn a_folder_that_is_not_a_repo_is_refused_with_a_reason() {
        let root = tmp("norepo");
        let project = root.join("plain");
        std::fs::create_dir_all(&project).unwrap();
        let e = ensure(&project, &root.join("wt"), "r", "b").await.unwrap_err();
        assert!(format!("{e:#}").contains("git repository"), "{e:#}");
    }

    #[tokio::test]
    async fn ids_cannot_escape_the_folder() {
        let root = tmp("ids");
        assert!(ensure(&root, &root, "../x", "b").await.is_err());
        assert!(ensure(&root, &root, "r", "a/b").await.is_err());
    }
}
