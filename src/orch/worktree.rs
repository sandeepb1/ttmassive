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
