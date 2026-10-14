//! Reading a file for the page's read-only viewer: a path the terminal printed, tapped.
//!
//! This is a seatbelt, not a sandbox: whoever is logged in can already read anything their
//! account can through the terminal. The rules exist so a stray or hostile path in some output
//! cannot make one tap show something it should not: the file must be inside the folder the
//! session works in (symlinks are followed first, so one cannot lead out), and ttm's own secrets
//! are never shown.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use serde::Serialize;

/// Most of a file the viewer will take. A bigger one is cut here, on a line.
pub const MAX_BYTES: u64 = 1024 * 1024;
const MAX_PATH: usize = 4096;
/// Directories whose contents are credentials, wherever they are.
const SECRET_DIRS: [&str; 4] = [".ssh", ".gnupg", ".aws", ".kube"];

#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    NotFound,
    /// Resolves to somewhere outside the project folder (possibly through a symlink).
    Outside,
    /// Credentials, or ttm's own config and data.
    Secret,
    /// A directory, socket, device or the like.
    NotAFile,
    Binary,
}

impl Refused {
    pub fn message(&self) -> &'static str {
        match self {
            Self::NotFound => "no such file in this project",
            Self::Outside => "that path is outside the project folder",
            Self::Secret => "that file is private",
            Self::NotAFile => "not a regular file",
            Self::Binary => "binary file",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct FileView {
    /// Where it really is.
    pub path: String,
    /// Relative to the project folder, for display.
    pub rel: String,
    pub name: String,
    pub size: u64,
    /// The file is longer than `MAX_BYTES` and this is its start.
    pub truncated: bool,
    pub lines: usize,
    pub text: String,
}

/// The folder a session's paths are resolved in: the repository its working folder is in, else
/// the working folder itself. `stop_at` (the home folder) is never taken as a repository, so a
/// dotfiles repo does not make every shell's whole home folder readable.
pub fn project_root(cwd: &Path, stop_at: Option<&Path>) -> PathBuf {
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let home = stop_at.and_then(|h| h.canonicalize().ok());
    cwd.ancestors()
        .find(|d| Some(*d) != home.as_deref() && d.join(".git").exists())
        .unwrap_or(&cwd)
        .to_path_buf()
}

fn is_secret(real: &Path, secrets: &[PathBuf]) -> bool {
    secrets.iter().any(|s| real.starts_with(s))
        || real.components().any(|c| SECRET_DIRS.iter().any(|d| c.as_os_str() == *d))
}

/// Resolve `requested` (absolute, `~/…`, or relative to the working folder, then to the project
/// root) to a real file inside `root`. `secrets` must already be canonical.
pub fn resolve(root: &Path, cwd: &Path, home: Option<&Path>, requested: &str, secrets: &[PathBuf]) -> Result<PathBuf, Refused> {
    let req = requested.trim();
    if req.is_empty() || req.len() > MAX_PATH || req.contains('\0') {
        return Err(Refused::NotFound);
    }
    let path = match (req.strip_prefix("~/"), home) {
        (Some(rest), Some(h)) => h.join(rest),
        _ => PathBuf::from(req),
    };
    let candidates = if path.is_absolute() { vec![path] } else { vec![cwd.join(&path), root.join(&path)] };
    for c in candidates {
        let Ok(real) = c.canonicalize() else { continue };
        if !real.starts_with(root) {
            return Err(Refused::Outside);
        }
        if is_secret(&real, secrets) {
            return Err(Refused::Secret);
        }
        return if real.is_file() { Ok(real) } else { Err(Refused::NotAFile) };
    }
    Err(Refused::NotFound)
}

/// Read up to `MAX_BYTES` of `path` as text. Refuses anything that looks binary.
pub fn read(path: &Path, root: &Path) -> Result<FileView, Refused> {
    let file = File::open(path).map_err(|_| Refused::NotFound)?;
    let meta = file.metadata().map_err(|_| Refused::NotFound)?;
    if !meta.is_file() {
        return Err(Refused::NotAFile);
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|_| Refused::NotFound)?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return Err(Refused::Binary);
    }
    let truncated = bytes.len() as u64 > MAX_BYTES;
    if truncated {
        bytes.truncate(MAX_BYTES as usize);
        // End on a whole line, not in the middle of one.
        if let Some(nl) = bytes.iter().rposition(|b| *b == b'\n') {
            bytes.truncate(nl + 1);
        }
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let lines = text.lines().count();
    let rel = path.strip_prefix(root).unwrap_or(path).display().to_string();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(FileView { path: path.display().to_string(), rel, name, size: meta.len(), truncated, lines, text })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A scratch tree: `<tmp>/proj/{.git, src/a.rs, notes.txt, link-out -> ../outside, link-in -> src}`,
    /// `<tmp>/outside/secret.txt`, `<tmp>/home/.ssh/id_rsa`, `<tmp>/ttm/config.toml`.
    struct Tree {
        base: PathBuf,
        root: PathBuf,
    }

    impl Tree {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!("ttm-files-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&base);
            let base = {
                fs::create_dir_all(&base).unwrap();
                base.canonicalize().unwrap()
            };
            for d in ["proj/.git", "proj/src", "outside", "home/.ssh", "ttm"] {
                fs::create_dir_all(base.join(d)).unwrap();
            }
            fs::write(base.join("proj/src/a.rs"), "fn main() {}\nlet x = 1;\n").unwrap();
            fs::write(base.join("proj/notes.txt"), "hello\n").unwrap();
            fs::write(base.join("outside/secret.txt"), "nope\n").unwrap();
            fs::write(base.join("home/.ssh/id_rsa"), "KEY\n").unwrap();
            fs::write(base.join("ttm/config.toml"), "password = \"x\"\n").unwrap();
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(base.join("outside"), base.join("proj/link-out")).unwrap();
                std::os::unix::fs::symlink(base.join("proj/src"), base.join("proj/link-in")).unwrap();
            }
            let root = base.join("proj");
            Self { base, root }
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    fn res(t: &Tree, cwd: &str, req: &str) -> Result<PathBuf, Refused> {
        let secrets = vec![t.base.join("ttm")];
        resolve(&t.root, &t.root.join(cwd), Some(&t.base.join("home")), req, &secrets)
    }

    #[test]
    fn root_is_the_repository_not_the_subfolder() {
        let t = Tree::new("root");
        assert_eq!(project_root(&t.root.join("src"), None), t.root);
        assert_eq!(project_root(&t.root, None), t.root);
    }

    #[test]
    fn root_without_a_repository_is_the_working_folder() {
        let t = Tree::new("norepo");
        assert_eq!(project_root(&t.base.join("outside"), None), t.base.join("outside"));
    }

    #[test]
    fn a_repository_in_the_home_folder_is_not_a_root() {
        let t = Tree::new("dotfiles");
        fs::create_dir_all(t.base.join("home/.git")).unwrap();
        fs::create_dir_all(t.base.join("home/work")).unwrap();
        let home = t.base.join("home");
        assert_eq!(project_root(&home.join("work"), Some(&home)), home.join("work"));
        // Without the guard the dotfiles repo would be the root.
        assert_eq!(project_root(&home.join("work"), None), home);
    }

    #[test]
    fn finds_files_relative_to_the_working_folder_then_the_root() {
        let t = Tree::new("rel");
        let a = t.root.join("src/a.rs");
        assert_eq!(res(&t, "src", "a.rs"), Ok(a.clone())); // from the working folder
        assert_eq!(res(&t, "src", "src/a.rs"), Ok(a.clone())); // falls back to the root
        assert_eq!(res(&t, "", "./src/a.rs"), Ok(a.clone()));
        assert_eq!(res(&t, "", a.to_str().unwrap()), Ok(a));
        assert_eq!(res(&t, "", "src/missing.rs"), Err(Refused::NotFound));
    }

    #[test]
    fn tilde_means_home() {
        let t = Tree::new("tilde");
        assert_eq!(res(&t, "", "~/.ssh/id_rsa"), Err(Refused::Outside)); // home is not in the project
    }

    #[test]
    fn refuses_what_is_outside_the_project() {
        let t = Tree::new("outside");
        assert_eq!(res(&t, "", "../outside/secret.txt"), Err(Refused::Outside));
        assert_eq!(res(&t, "", t.base.join("outside/secret.txt").to_str().unwrap()), Err(Refused::Outside));
        assert_eq!(res(&t, "", "/etc/hosts"), Err(Refused::Outside));
        assert_eq!(res(&t, "src", "../../outside/secret.txt"), Err(Refused::Outside));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_cannot_lead_out() {
        let t = Tree::new("symlink");
        assert_eq!(res(&t, "", "link-out/secret.txt"), Err(Refused::Outside));
        // One that stays inside is fine, and shows the real path.
        assert_eq!(res(&t, "", "link-in/a.rs"), Ok(t.root.join("src/a.rs")));
    }

    #[test]
    fn credentials_and_ttms_own_files_are_never_shown() {
        let t = Tree::new("secret");
        // A project that sits inside a secrets folder, or contains one.
        fs::create_dir_all(t.root.join(".aws")).unwrap();
        fs::write(t.root.join(".aws/credentials"), "x").unwrap();
        assert_eq!(res(&t, "", ".aws/credentials"), Err(Refused::Secret));
        // ttm's data lives under the root here, as it would for a project that holds it.
        let secrets = vec![t.root.join("src")];
        assert_eq!(resolve(&t.root, &t.root, None, "src/a.rs", &secrets), Err(Refused::Secret));
    }

    #[test]
    fn directories_and_odd_input_are_refused() {
        let t = Tree::new("odd");
        assert_eq!(res(&t, "", "src"), Err(Refused::NotAFile));
        assert_eq!(res(&t, "", ""), Err(Refused::NotFound));
        assert_eq!(res(&t, "", "  "), Err(Refused::NotFound));
        assert_eq!(res(&t, "", "a\0b"), Err(Refused::NotFound));
        assert_eq!(res(&t, "", &"x/".repeat(3000)), Err(Refused::NotFound));
    }

    #[test]
    fn reads_text_with_its_name_and_relative_path() {
        let t = Tree::new("read");
        let v = read(&t.root.join("src/a.rs"), &t.root).unwrap();
        assert_eq!((v.name.as_str(), v.rel.as_str(), v.lines, v.truncated), ("a.rs", "src/a.rs", 2, false));
        assert!(v.text.starts_with("fn main"));
    }

    #[test]
    fn refuses_binary_files() {
        let t = Tree::new("binary");
        fs::write(t.root.join("img.bin"), [0x89, b'P', b'N', b'G', 0, 1, 2]).unwrap();
        assert_eq!(read(&t.root.join("img.bin"), &t.root).unwrap_err(), Refused::Binary);
    }

    #[test]
    fn a_long_file_is_cut_on_a_line() {
        let t = Tree::new("long");
        let line = "0123456789".repeat(9) + "\n"; // 91 bytes
        let n = (MAX_BYTES as usize / line.len()) + 50;
        fs::write(t.root.join("big.txt"), line.repeat(n)).unwrap();
        let v = read(&t.root.join("big.txt"), &t.root).unwrap();
        assert!(v.truncated);
        assert!(v.text.len() as u64 <= MAX_BYTES && v.text.ends_with('\n'));
        assert!(v.size > MAX_BYTES);
        assert!(v.text.lines().all(|l| l.len() == 90), "no half line at the end");
    }

    #[test]
    fn invalid_utf8_is_shown_not_refused() {
        let t = Tree::new("utf8");
        fs::write(t.root.join("latin.txt"), b"caf\xe9\n").unwrap();
        assert!(read(&t.root.join("latin.txt"), &t.root).unwrap().text.starts_with("caf"));
    }
}
