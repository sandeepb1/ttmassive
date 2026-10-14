//! Pictures in a conversation. Claude's log holds an attached or returned image as base64 inside
//! the line, and Codex's holds a path or a data URL; the browser can read neither. Each picture is
//! decoded once into `<data>/chat-images/<hash>.<ext>`, and the chat refers to it by URL.
//!
//! Only PNG, JPEG, GIF and WebP are kept, and the bytes are checked rather than the name or the
//! declared type: an SVG, or a file that merely has an image extension, is never served.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};

/// Larger pictures are not kept (Claude resizes what it is sent well below this).
const MAX_BYTES: usize = 16 * 1024 * 1024;
pub const URL_PREFIX: &str = "/api/chat-images/";
/// Pictures older than this are removed when ttm starts; a conversation that is read again saves
/// the ones it still shows afresh.
pub const KEEP: Duration = Duration::from_secs(14 * 24 * 3600);

const EXTS: &[&str] = &["png", "jpg", "gif", "webp"];

pub struct Store {
    dir: PathBuf,
}

/// The extension of the image format `b` starts like, if it is one we keep.
fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("gif")
    } else if b.len() > 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

impl Store {
    pub fn new(data_dir: &Path) -> Self {
        Self { dir: data_dir.join("chat-images") }
    }

    /// Keep `bytes` if they are a picture; returns the URL to show it at.
    pub fn save_bytes(&self, bytes: &[u8]) -> Option<String> {
        let ext = sniff(bytes).filter(|_| bytes.len() <= MAX_BYTES)?;
        let hash: String = Sha256::digest(bytes)[..16].iter().map(|b| format!("{b:02x}")).collect();
        let name = format!("{hash}.{ext}");
        let path = self.dir.join(&name);
        if !path.is_file() {
            self.write(&path, bytes).ok()?;
        }
        Some(format!("{URL_PREFIX}{name}"))
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Pictures from private conversations: owner-only, set before the rename shows them.
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
            let _ = std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700));
        }
        std::fs::rename(&tmp, path)
    }

    /// A picture given as base64 (Claude's `source.data`).
    pub fn save_base64(&self, data: &str) -> Option<String> {
        let data = data.trim();
        if data.len() > MAX_BYTES / 3 * 4 + 8 {
            return None;
        }
        self.save_bytes(&STANDARD.decode(data).ok()?)
    }

    /// A picture given as a `data:image/...;base64,` URL (Codex's `image_url`).
    pub fn save_data_url(&self, url: &str) -> Option<String> {
        let (head, data) = url.strip_prefix("data:")?.split_once(',')?;
        head.ends_with(";base64").then(|| self.save_base64(data)).flatten()
    }

    /// A picture named by a path in the log (Codex's `local_image`), if it is still there.
    pub fn save_file(&self, path: &str) -> Option<String> {
        let path = Path::new(path);
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        if !path.is_absolute() || !(EXTS.contains(&ext.as_str()) || ext == "jpeg") {
            return None;
        }
        // A real file: not a link that points somewhere else, not a device, not a huge one.
        let meta = std::fs::symlink_metadata(path).ok()?;
        if !meta.is_file() || meta.len() > MAX_BYTES as u64 {
            return None;
        }
        self.save_bytes(&std::fs::read(path).ok()?)
    }

    /// The file for `name` (as in a URL this store handed out), if it exists.
    pub fn locate(&self, name: &str) -> Option<PathBuf> {
        let (hash, ext) = name.split_once('.')?;
        if hash.len() != 32 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) || !EXTS.contains(&ext) {
            return None;
        }
        let path = self.dir.join(name);
        path.is_file().then_some(path)
    }

    /// Remove pictures not written for `max_age`.
    pub fn cleanup(&self, max_age: Duration) {
        let Some(cutoff) = SystemTime::now().checked_sub(max_age) else { return };
        for entry in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < cutoff);
            if old {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1x1 PNG.
    const PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89,
    ];

    fn store(name: &str) -> (Store, PathBuf) {
        let d = std::env::temp_dir().join(format!("ttm-chatimg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        (Store::new(&d), d)
    }

    #[test]
    fn formats_are_recognised_by_their_bytes() {
        assert_eq!(sniff(PNG), Some("png"));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Some("jpg"));
        assert_eq!(sniff(b"GIF89a\x01\0"), Some("gif"));
        assert_eq!(sniff(b"RIFF\x24\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(sniff(b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>"), None);
        assert_eq!(sniff(b"<html>"), None);
        assert_eq!(sniff(b""), None);
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVE...."), None);
    }

    #[test]
    fn a_picture_is_kept_once_and_found_by_its_url() {
        let (s, dir) = store("keep");
        let url = s.save_bytes(PNG).unwrap();
        assert!(url.starts_with(URL_PREFIX) && url.ends_with(".png"), "{url}");
        assert_eq!(s.save_bytes(PNG).as_deref(), Some(url.as_str()), "the same picture has the same URL");
        let name = url.strip_prefix(URL_PREFIX).unwrap();
        assert_eq!(std::fs::read(s.locate(name).unwrap()).unwrap(), PNG);
        assert_eq!(std::fs::read_dir(dir.join("chat-images")).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(s.locate(name).unwrap()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn base64_and_data_urls_are_decoded() {
        let (s, _d) = store("b64");
        let b64 = STANDARD.encode(PNG);
        let a = s.save_base64(&b64).unwrap();
        assert_eq!(s.save_base64(&format!("  {b64}\n")).unwrap(), a, "whitespace around it is fine");
        assert_eq!(s.save_data_url(&format!("data:image/png;base64,{b64}")).unwrap(), a);
        assert_eq!(s.save_data_url(&format!("data:image/png,{b64}")), None, "not base64");
        assert_eq!(s.save_data_url("https://example.com/x.png"), None);
        assert_eq!(s.save_base64("!!!not base64!!!"), None);
        assert_eq!(s.save_base64(&STANDARD.encode(b"just text")), None, "base64 of something else");
    }

    #[test]
    fn only_real_image_files_are_copied() {
        let (s, d) = store("file");
        let img = d.join("shot.png");
        std::fs::write(&img, PNG).unwrap();
        assert!(s.save_file(img.to_str().unwrap()).is_some());

        let fake = d.join("secrets.png");
        std::fs::write(&fake, "password=hunter2").unwrap();
        assert_eq!(s.save_file(fake.to_str().unwrap()), None, "an image name is not enough");

        let text = d.join("notes.txt");
        std::fs::write(&text, PNG).unwrap();
        assert_eq!(s.save_file(text.to_str().unwrap()), None, "nor is image content in another file");
        assert_eq!(s.save_file("relative/shot.png"), None);
        assert_eq!(s.save_file(d.join("gone.png").to_str().unwrap()), None);

        #[cfg(unix)]
        {
            let link = d.join("link.png");
            std::os::unix::fs::symlink(&img, &link).unwrap();
            assert_eq!(s.save_file(link.to_str().unwrap()), None, "a link is not followed");
        }
    }

    #[test]
    fn urls_cannot_reach_other_files() {
        let (s, _d) = store("locate");
        let url = s.save_bytes(PNG).unwrap();
        let name = url.strip_prefix(URL_PREFIX).unwrap().to_owned();
        assert!(s.locate(&name).is_some());
        for bad in ["../state.json", "..%2Fx.png", &format!("../{name}"), "x.png", &format!("{}.svg", &name[..32]), &format!("{}.png", &name[..31]), "", "."] {
            assert!(s.locate(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn old_pictures_are_removed() {
        let (s, _d) = store("clean");
        let url = s.save_bytes(PNG).unwrap();
        let name = url.strip_prefix(URL_PREFIX).unwrap();
        s.cleanup(Duration::from_secs(3600));
        assert!(s.locate(name).is_some(), "a fresh picture stays");
        std::thread::sleep(Duration::from_millis(20));
        s.cleanup(Duration::from_millis(5));
        assert!(s.locate(name).is_none(), "an old one goes");
    }
}
