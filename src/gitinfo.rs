//! The git branch a folder is on, read straight from `.git/HEAD` so listing sessions never has to
//! start a `git` process.

use std::path::{Path, PathBuf};

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
}
