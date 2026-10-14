//! Images attached from the chat view. They are saved on the host so the agent can read them by
//! path (`@/path/to/image.jpg` in the message), and removed after a week.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

/// Largest image accepted. The browser shrinks photos well below this before sending.
pub const MAX_BYTES: usize = 10 * 1024 * 1024;
/// How long an uploaded image is kept.
pub const KEEP: Duration = Duration::from_secs(7 * 24 * 3600);

/// The kind of image these bytes are, from their first bytes rather than from what the client
/// claims: `png`, `jpg`, `gif` or `webp`.
pub fn detect(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

pub fn content_type(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

/// A single path component made of boring characters: no separators, no dots-only names.
pub fn valid_component(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s != "."
        && s != ".."
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("uploads")
}

/// Save an image for the session whose hidden id is `uid`. Returns its absolute path.
pub fn save(data_dir: &Path, uid: &str, bytes: &[u8]) -> Result<PathBuf> {
    ensure!(!bytes.is_empty(), "the image is empty");
    ensure!(bytes.len() <= MAX_BYTES, "the image is too large (max {} MB)", MAX_BYTES / (1024 * 1024));
    let Some(ext) = detect(bytes) else { bail!("that is not a PNG, JPEG, GIF or WebP image") };
    write(data_dir, uid, bytes, ext)
}

/// Save any file dropped into the terminal. Images keep the kind their bytes show; anything else
/// keeps a harmless extension taken from `name` so the agent can tell what it is. The stored name
/// is always generated, never the client's.
pub fn save_file(data_dir: &Path, uid: &str, bytes: &[u8], name: &str) -> Result<PathBuf> {
    ensure!(!bytes.is_empty(), "the file is empty");
    ensure!(bytes.len() <= MAX_BYTES, "the file is too large (max {} MB)", MAX_BYTES / (1024 * 1024));
    let ext = detect(bytes).map_or_else(|| plain_extension(name), str::to_owned);
    write(data_dir, uid, bytes, &ext)
}

/// The extension of `name` if it is short and made of letters and digits, else `bin`. Image
/// extensions are not trusted here: only `detect` may call a file an image.
fn plain_extension(name: &str) -> String {
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e).to_ascii_lowercase();
    let boring = (1..=8).contains(&ext.len()) && ext.bytes().all(|b| b.is_ascii_alphanumeric());
    if boring && !is_image_ext(&ext) { ext } else { "bin".to_owned() }
}

fn is_image_ext(ext: &str) -> bool {
    matches!(ext, "png" | "jpg" | "jpeg" | "gif" | "webp")
}

fn write(data_dir: &Path, uid: &str, bytes: &[u8], ext: &str) -> Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    ensure!(valid_component(uid), "no such session");
    let folder = dir(data_dir).join(uid);
    std::fs::create_dir_all(&folder).with_context(|| format!("creating {}", folder.display()))?;
    let micros = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_micros());
    let name = format!("{micros:x}-{:x}.{ext}", COUNTER.fetch_add(1, Ordering::Relaxed));
    let path = folder.join(name);
    std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        let _ = std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700));
    }
    Ok(path)
}

/// The file for `/api/uploads/{uid}/{file}`, if both parts are plain names and it exists.
pub fn locate(data_dir: &Path, uid: &str, file: &str) -> Option<PathBuf> {
    if !valid_component(uid) || !valid_component(file) {
        return None;
    }
    let path = dir(data_dir).join(uid).join(file);
    path.is_file().then_some(path)
}

/// An image saved for a session.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Stored {
    pub name: String,
    pub size: u64,
    /// Unix seconds it was saved.
    pub ts: u64,
}

/// The images saved for the session whose hidden id is `uid`, newest first. Other files dropped
/// into the terminal are not listed.
pub fn list(data_dir: &Path, uid: &str) -> Vec<Stored> {
    if !valid_component(uid) {
        return Vec::new();
    }
    let mut out: Vec<Stored> = std::fs::read_dir(dir(data_dir).join(uid))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok().filter(|n| valid_component(n))?;
            is_image_ext(name.rsplit_once('.')?.1).then_some(())?;
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            let ts = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
            Some(Stored { name, size: meta.len(), ts })
        })
        .collect();
    out.sort_by(|a, b| b.ts.cmp(&a.ts).then_with(|| b.name.cmp(&a.name)));
    out
}

/// Delete uploads older than `max_age`, and any folder left empty. Run at start-up.
pub fn cleanup(data_dir: &Path, max_age: Duration) {
    let now = SystemTime::now();
    for folder in std::fs::read_dir(dir(data_dir)).into_iter().flatten().flatten() {
        let folder = folder.path();
        for f in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let old = f
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age > max_age);
            if old {
                let _ = std::fs::remove_file(f.path());
            }
        }
        let _ = std::fs::remove_dir(&folder); // fails (harmlessly) unless it is empty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ttm-uploads-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_kind_comes_from_the_bytes() {
        assert_eq!(detect(PNG), Some("png"));
        assert_eq!(detect(&[0xff, 0xd8, 0xff, 0xe0, 0]), Some("jpg"));
        assert_eq!(detect(b"GIF89a...."), Some("gif"));
        assert_eq!(detect(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(detect(b"<svg onload=alert(1)>"), None);
        assert_eq!(detect(b"#!/bin/sh\nrm -rf ~"), None);
        assert_eq!(detect(b""), None);
    }

    #[test]
    fn names_that_could_climb_out_of_the_folder_are_refused() {
        for bad in ["", ".", "..", "a/b", "../x", "a\\b", "a b", "é", &"x".repeat(81)] {
            assert!(!valid_component(bad), "{bad:?}");
        }
        for good in ["abc-123_x.jpg", "7f3a"] {
            assert!(valid_component(good), "{good:?}");
        }
    }

    #[test]
    fn saving_checks_size_and_kind_and_keeps_the_file_private() {
        let d = temp("save");
        let p = save(&d, "uid1", PNG).unwrap();
        assert!(p.starts_with(d.join("uploads").join("uid1")));
        assert_eq!(p.extension().unwrap(), "png");
        assert_eq!(std::fs::read(&p).unwrap(), PNG);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let two = save(&d, "uid1", PNG).unwrap();
        assert_ne!(p, two, "two uploads must not overwrite each other");
        assert!(save(&d, "uid1", b"not an image").unwrap_err().to_string().contains("not a PNG"));
        assert!(save(&d, "../evil", PNG).is_err());
        assert!(save(&d, "uid1", &[]).is_err());
        let mut big = PNG.to_vec();
        big.resize(MAX_BYTES + 1, 0);
        assert!(save(&d, "uid1", &big).unwrap_err().to_string().contains("too large"));
        // What was saved can be served back by its two-part name, and nothing else can.
        let name = p.file_name().unwrap().to_str().unwrap();
        assert_eq!(locate(&d, "uid1", name), Some(p.clone()));
        assert_eq!(locate(&d, "uid1", "../../uploads/uid1/x"), None);
        assert_eq!(locate(&d, "nope", name), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_session_lists_only_its_own_images_newest_first() {
        let d = temp("list");
        assert!(list(&d, "u1").is_empty(), "no folder yet");
        let a = save(&d, "u1", PNG).unwrap();
        std::thread::sleep(Duration::from_millis(1100)); // timestamps are whole seconds
        let b = save(&d, "u1", PNG).unwrap();
        save(&d, "u2", PNG).unwrap();
        let names: Vec<_> = list(&d, "u1").into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec![b.file_name().unwrap().to_str().unwrap(), a.file_name().unwrap().to_str().unwrap()]);
        assert_eq!(list(&d, "u1")[0].size, PNG.len() as u64);
        assert!(list(&d, "../u1").is_empty(), "a hostile id lists nothing");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn any_file_can_be_saved_but_only_images_are_listed() {
        let d = temp("file");
        let note = save_file(&d, "u1", b"hello", "My Notes.TXT").unwrap();
        assert_eq!(note.extension().unwrap(), "txt");
        assert_eq!(std::fs::read(&note).unwrap(), b"hello");
        // The client's name and its image extensions are never trusted.
        assert_eq!(save_file(&d, "u1", b"hello", "../../x.sh").unwrap().extension().unwrap(), "sh");
        assert_eq!(save_file(&d, "u1", b"<svg onload=1>", "pic.png").unwrap().extension().unwrap(), "bin");
        assert_eq!(save_file(&d, "u1", b"x", "noext").unwrap().extension().unwrap(), "bin");
        assert_eq!(save_file(&d, "u1", b"x", "a.tar.verylongext").unwrap().extension().unwrap(), "bin");
        assert_eq!(save_file(&d, "u1", PNG, "whatever.txt").unwrap().extension().unwrap(), "png");
        assert!(save_file(&d, "u1", &[], "a.txt").is_err());
        assert!(save_file(&d, "../u1", b"x", "a.txt").is_err());
        let names: Vec<_> = list(&d, "u1").into_iter().map(|s| s.name).collect();
        assert_eq!(names.len(), 1, "only the PNG is an image: {names:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cleanup_removes_old_files_and_empty_folders_only() {
        let d = temp("clean");
        let keep = save(&d, "u1", PNG).unwrap();
        let drop = save(&d, "u2", PNG).unwrap();
        // Zero tolerance deletes everything that exists "before now".
        std::thread::sleep(Duration::from_millis(20));
        cleanup(&d, Duration::from_secs(3600));
        assert!(keep.exists() && drop.exists(), "recent files stay");
        cleanup(&d, Duration::ZERO);
        assert!(!keep.exists() && !drop.exists());
        assert!(!d.join("uploads").join("u2").exists(), "the emptied folder goes too");
        let _ = std::fs::remove_dir_all(&d);
    }
}
