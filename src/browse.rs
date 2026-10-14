//! Folder completion for the command bar's "Open folder": type part of a path, get the folders
//! that fit. Like the file viewer this is a seatbelt, not a sandbox (whoever is logged in can run
//! `ls` anywhere their account can). It keeps the picker to the home folder, with symlinks
//! followed first so one cannot lead out, and to folders only.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// The most folders returned for one request.
const MAX_ENTRIES: usize = 50;
/// The most directory entries looked at, so a folder with a million files does not stall a keystroke.
const MAX_SCANNED: usize = 5000;

#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Resolves to somewhere outside the home folder.
    Outside,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: String,
    /// The path with the home folder as `~`.
    pub short: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Listing {
    /// The folder whose subfolders are listed, with the home folder as `~`. Empty if it does not exist (yet).
    pub dir: String,
    /// The same folder as an absolute path, to start a session in. Empty if it does not exist (yet).
    pub path: String,
    pub entries: Vec<Entry>,
}

impl Listing {
    fn none() -> Self {
        Listing { dir: String::new(), path: String::new(), entries: vec![] }
    }
}

fn short(home: &Path, p: &Path) -> String {
    match p.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

/// `typed` as an absolute path: `~` and `~/…` mean the home folder, a bare name is relative to it.
fn expand(home: &Path, typed: &str) -> PathBuf {
    if typed == "~" {
        home.to_owned()
    } else if let Some(rest) = typed.strip_prefix("~/") {
        home.join(rest)
    } else if typed.starts_with('/') {
        PathBuf::from(typed)
    } else {
        home.join(typed)
    }
}

/// The subfolders of the folder `typed` names, or, when it ends mid-name, the siblings that start
/// with that name (ignoring case). Hidden folders are listed only when the name being typed starts
/// with a dot.
pub fn list(home: &Path, typed: &str) -> Result<Listing, Refused> {
    let Ok(home) = home.canonicalize() else { return Ok(Listing::none()) };
    let typed = typed.trim();
    // Split the text as typed: `Path` would drop a trailing `.` and has no name for `..`.
    let (dir_part, name) = match typed.rfind('/') {
        Some(i) => (&typed[..=i], &typed[i + 1..]),
        None if typed == "~" => ("~", ""),
        None => ("", typed),
    };
    let (dir, prefix) = if name == ".." { (expand(&home, typed), "") } else { (expand(&home, dir_part), name) };
    // A folder that is not there yet is not an error while the path is still being typed.
    let Ok(dir) = dir.canonicalize() else { return Ok(Listing::none()) };
    if !dir.starts_with(&home) {
        return Err(Refused::Outside);
    }
    let prefix = prefix.to_lowercase();
    let hidden_ok = prefix.starts_with('.');
    let mut entries: Vec<Entry> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .take(MAX_SCANNED)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if (name.starts_with('.') && !hidden_ok) || !name.to_lowercase().starts_with(&prefix) {
                return None;
            }
            // Follow a symlink first: a link out of the home folder is not listed.
            let real = e.path().canonicalize().ok()?;
            (real.is_dir() && real.starts_with(&home)).then(|| Entry { name, short: short(&home, &real), path: real.display().to_string() })
        })
        .collect();
    entries.sort_by_key(|e| e.name.to_lowercase());
    entries.truncate(MAX_ENTRIES);
    Ok(Listing { dir: short(&home, &dir), path: dir.display().to_string(), entries })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Tree {
        base: PathBuf,
        home: PathBuf,
    }

    impl Tree {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!("ttm-browse-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&base);
            let base = {
                fs::create_dir_all(&base).unwrap();
                base.canonicalize().unwrap()
            };
            let home = base.join("home");
            for d in ["home/Docs", "home/docs2", "home/Projects/app", "home/.hidden", "outside/secret"] {
                fs::create_dir_all(base.join(d)).unwrap();
            }
            fs::write(home.join("file.txt"), "x").unwrap();
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(base.join("outside"), home.join("escape")).unwrap();
                std::os::unix::fs::symlink(home.join("Projects"), home.join("shortcut")).unwrap();
            }
            Tree { base, home }
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    fn names(l: &Listing) -> Vec<&str> {
        l.entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn a_trailing_slash_lists_the_folders_inside_and_skips_files_hidden_ones_and_links_out() {
        let t = Tree::new("slash");
        let l = list(&t.home, "~/").unwrap();
        assert_eq!(names(&l), ["Docs", "docs2", "Projects", "shortcut"]);
        assert_eq!(l.dir, "~");
        assert_eq!(list(&t.home, "").unwrap(), l, "nothing typed means the home folder");
        assert_eq!(list(&t.home, "~").unwrap(), l);
        assert_eq!(names(&list(&t.home, "~/Projects/").unwrap()), ["app"]);
    }

    #[test]
    fn a_partial_name_lists_matching_siblings_ignoring_case() {
        let t = Tree::new("partial");
        assert_eq!(names(&list(&t.home, "~/do").unwrap()), ["Docs", "docs2"]);
        assert_eq!(names(&list(&t.home, "Pro").unwrap()), ["Projects"], "a bare name is relative to home");
        assert!(list(&t.home, "~/nothing").unwrap().entries.is_empty());
        assert!(list(&t.home, "~/missing/dir/").unwrap().entries.is_empty(), "not there yet is not an error");
    }

    #[test]
    fn hidden_folders_appear_only_when_a_dot_is_typed() {
        let t = Tree::new("hidden");
        assert!(!names(&list(&t.home, "~/").unwrap()).contains(&".hidden"));
        assert_eq!(names(&list(&t.home, "~/.").unwrap()), [".hidden"]);
    }

    #[test]
    fn nothing_outside_the_home_folder_is_listed() {
        let t = Tree::new("outside");
        for typed in ["/", "/etc", "/etc/", "~/..", "~/../", "~/../outside/", "..", "../outside/", "~/escape/"] {
            assert_eq!(list(&t.home, typed), Err(Refused::Outside), "{typed:?}");
        }
        // A link that stays inside home is followed and shown by where it really goes.
        let l = list(&t.home, "~/short").unwrap();
        assert_eq!(l.entries[0].short, "~/Projects");
        assert_eq!(l.entries[0].path, t.home.join("Projects").display().to_string());
    }
}
