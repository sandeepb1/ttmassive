//! Tiled-pane preferences and saved workspaces, kept on the server so a layout saved on the Mac
//! opens on the tablet. One small JSON document, replaced whole by the page and checked here.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::tmux;

const FILE: &str = "layouts.json";
const MAX_WORKSPACES: usize = 24;
const MAX_TILES: usize = 6;
const MAX_NAME: usize = 40;
const PRESETS: [&str; 7] = ["single", "cols2", "rows2", "cols3", "main-side", "grid2x2", "grid3x2"];

/// Serialises writes so two saves cannot interleave their temp files.
static WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Allow more than one pane at a time.
    pub multi: bool,
    /// Most panes open at once (1 to 6).
    pub max_panes: u8,
    /// Preset to start from on a big screen and on a tablet-sized one.
    pub desktop: String,
    pub tablet: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { multi: true, max_panes: 4, desktop: "cols2".into(), tablet: "cols2".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub name: String,
    pub preset: String,
    /// Relative column and row sizes.
    pub cols: Vec<f64>,
    pub rows: Vec<f64>,
    /// Session shown in each tile, in tile order. Empty string for an empty tile.
    pub tiles: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layouts {
    pub settings: Settings,
    pub workspaces: Vec<Workspace>,
}

fn sizes_ok(v: &[f64]) -> bool {
    (1..=3).contains(&v.len()) && v.iter().all(|x| x.is_finite() && *x > 0.0 && *x < 1.0e6)
}

/// Check a document from the page before it is stored.
pub fn validate(l: &Layouts) -> Result<(), &'static str> {
    let s = &l.settings;
    if !(1..=6).contains(&s.max_panes) {
        return Err("max panes must be 1 to 6");
    }
    if !PRESETS.contains(&s.desktop.as_str()) || !PRESETS.contains(&s.tablet.as_str()) {
        return Err("unknown preset");
    }
    if l.workspaces.len() > MAX_WORKSPACES {
        return Err("too many workspaces");
    }
    for (i, w) in l.workspaces.iter().enumerate() {
        let name = w.name.trim();
        if name.is_empty() || w.name.chars().count() > MAX_NAME || w.name.chars().any(char::is_control) {
            return Err("workspace names are 1 to 40 characters");
        }
        if l.workspaces[..i].iter().any(|o| o.name == w.name) {
            return Err("duplicate workspace name");
        }
        if !PRESETS.contains(&w.preset.as_str()) {
            return Err("unknown preset");
        }
        if !sizes_ok(&w.cols) || !sizes_ok(&w.rows) {
            return Err("bad sizes");
        }
        if w.tiles.len() > MAX_TILES || w.tiles.iter().any(|t| !t.is_empty() && !tmux::valid_session_id(t)) {
            return Err("bad session in workspace");
        }
    }
    Ok(())
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// The stored document, or the defaults when there is none (or it cannot be read).
pub async fn load(data_dir: &Path) -> Layouts {
    let Ok(bytes) = tokio::fs::read(path(data_dir)).await else {
        return Layouts::default();
    };
    match serde_json::from_slice::<Layouts>(&bytes) {
        Ok(l) if validate(&l).is_ok() => l,
        _ => {
            tracing::warn!("{FILE} is unreadable; using defaults");
            Layouts::default()
        }
    }
}

pub async fn save(data_dir: &Path, l: &Layouts) -> Result<()> {
    let _guard = WRITE.lock().await;
    let path = path(data_dir);
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, serde_json::to_vec_pretty(l)?).await?;
    tokio::fs::rename(&tmp, &path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(name: &str) -> Workspace {
        Workspace { name: name.into(), preset: "grid2x2".into(), cols: vec![1.0, 1.0], rows: vec![1.0, 1.0], tiles: vec!["a".into(), "".into()] }
    }

    #[test]
    fn defaults_are_valid() {
        assert_eq!(validate(&Layouts::default()), Ok(()));
    }

    #[test]
    fn rejects_bad_settings() {
        let mut l = Layouts::default();
        l.settings.max_panes = 0;
        assert!(validate(&l).is_err());
        l.settings.max_panes = 7;
        assert!(validate(&l).is_err());
        l.settings.max_panes = 3;
        l.settings.desktop = "wild".into();
        assert!(validate(&l).is_err());
    }

    #[test]
    fn accepts_a_good_workspace_and_rejects_bad_ones() {
        let mut l = Layouts { workspaces: vec![ws("Claude + Codex")], ..Default::default() };
        assert_eq!(validate(&l), Ok(()));

        l.workspaces[0].tiles = vec!["a;rm".into()];
        assert!(validate(&l).is_err(), "session ids are checked");

        l.workspaces[0] = ws("");
        assert!(validate(&l).is_err());
        l.workspaces[0] = ws("tab\there");
        assert!(validate(&l).is_err());

        l.workspaces[0] = ws("x");
        l.workspaces[0].cols = vec![f64::NAN, 1.0];
        assert!(validate(&l).is_err());
        l.workspaces[0].cols = vec![1.0, 0.0];
        assert!(validate(&l).is_err());

        l.workspaces = vec![ws("same"), ws("same")];
        assert!(validate(&l).is_err(), "names are unique");
    }

    #[test]
    fn caps_how_much_is_stored() {
        let mut l = Layouts { workspaces: (0..25).map(|i| ws(&format!("w{i}"))).collect(), ..Default::default() };
        assert!(validate(&l).is_err());
        l.workspaces = vec![ws("big")];
        l.workspaces[0].tiles = vec![String::new(); 7];
        assert!(validate(&l).is_err());
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let l: Layouts = serde_json::from_str("{}").unwrap();
        assert_eq!(l, Layouts::default());
        let l: Layouts = serde_json::from_str(r#"{"workspaces":[]}"#).unwrap();
        assert!(l.settings.multi);
    }

    #[tokio::test]
    async fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("ttm-layouts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load(&dir).await, Layouts::default());
        let l = Layouts { workspaces: vec![ws("w")], ..Default::default() };
        save(&dir, &l).await.unwrap();
        assert_eq!(load(&dir).await, l);
        std::fs::write(dir.join(FILE), b"not json").unwrap();
        assert_eq!(load(&dir).await, Layouts::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
