//! Persisted settings (JSON in the OS config dir). Never contains the token.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const MAX_RECENT: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentRepo {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub width: f32,
    pub height: f32,
    pub x: Option<f32>,
    pub y: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub recent: Vec<RecentRepo>,
    pub last_clone_dir: Option<PathBuf>,
    pub window: Option<WindowGeometry>,
}

impl Config {
    /// `<config dir>/RetroGit/config.json`
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("RetroGit").join("config.json"))
    }

    /// Missing or corrupt file => default config (a corrupt file is logged, not fatal).
    pub fn load_from(path: &Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                log::warn!("ignoring corrupt config {}: {e}", path.display());
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    /// Atomic write: temp file then rename.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    /// Put `path` first in the recent list (no duplicates, capped).
    pub fn add_recent(&mut self, name: &str, path: &Path) {
        self.recent.retain(|r| r.path != path);
        self.recent.insert(
            0,
            RecentRepo {
                name: name.to_string(),
                path: path.to_path_buf(),
            },
        );
        self.recent.truncate(MAX_RECENT);
    }

    pub fn remove_recent(&mut self, path: &Path) {
        self.recent.retain(|r| r.path != path);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub").join("config.json");
        let mut c = Config::default();
        c.add_recent("demo", Path::new("/tmp/demo"));
        c.last_clone_dir = Some("/tmp".into());
        c.window = Some(WindowGeometry {
            width: 800.0,
            height: 600.0,
            x: Some(10.0),
            y: None,
        });
        c.save_to(&p).unwrap();
        assert_eq!(Config::load_from(&p), c);
    }

    #[test]
    fn missing_or_corrupt_file_gives_default() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.json");
        assert_eq!(Config::load_from(&p), Config::default());
        std::fs::write(&p, "{ not json").unwrap();
        assert_eq!(Config::load_from(&p), Config::default());
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.json");
        std::fs::write(&p, r#"{"last_clone_dir":"/x","future_field":1}"#).unwrap();
        let c = Config::load_from(&p);
        assert_eq!(c.last_clone_dir, Some(PathBuf::from("/x")));
        assert!(c.recent.is_empty());
    }

    #[test]
    fn add_recent_dedupes_moves_to_front_and_caps() {
        let mut c = Config::default();
        for i in 0..25 {
            c.add_recent(&format!("r{i}"), Path::new(&format!("/r{i}")));
        }
        assert_eq!(c.recent.len(), MAX_RECENT);
        assert_eq!(c.recent[0].name, "r24");
        c.add_recent("r10", Path::new("/r10"));
        assert_eq!(c.recent[0].path, PathBuf::from("/r10"));
        assert_eq!(
            c.recent
                .iter()
                .filter(|r| r.path == Path::new("/r10"))
                .count(),
            1
        );
        c.remove_recent(Path::new("/r10"));
        assert!(c.recent.iter().all(|r| r.path != Path::new("/r10")));
    }
}
