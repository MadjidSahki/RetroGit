//! Persisted settings (JSON in the OS config dir). Never contains the token.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

pub const MAX_RECENT: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentRepo {
    pub name: String,
    pub path: PathBuf,
}

/// Window size and position in unzoomed points (what the window is created with).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub width: f32,
    pub height: f32,
    pub x: Option<f32>,
    pub y: Option<f32>,
}

/// Smallest window, in points at the current zoom (the layout needs this much room).
pub const MIN_WINDOW: [f32; 2] = [520.0, 360.0];

impl WindowGeometry {
    /// From egui's viewport rectangles, which are in zoomed points.
    pub fn from_viewport(inner: egui::Rect, outer: Option<egui::Rect>, zoom: f32) -> Self {
        WindowGeometry {
            width: inner.width() * zoom,
            height: inner.height() * zoom,
            x: outer.map(|r| r.left() * zoom),
            y: outer.map(|r| r.top() * zoom),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(deserialize_with = "lenient")]
    pub recent: Vec<RecentRepo>,
    #[serde(deserialize_with = "lenient")]
    pub last_clone_dir: Option<PathBuf>,
    #[serde(deserialize_with = "lenient")]
    pub window: Option<WindowGeometry>,
    /// IDE chosen for each repository ("Open in IDE"), by IDE id.
    #[serde(deserialize_with = "lenient")]
    pub ide_by_repo: std::collections::BTreeMap<PathBuf, String>,
    /// Last IDE chosen: the default for repositories without a choice.
    #[serde(deserialize_with = "lenient")]
    pub default_ide: Option<String>,
    /// GitHub logins of the signed-in accounts, in order (tokens are in the keychain).
    #[serde(deserialize_with = "lenient")]
    pub accounts: Vec<String>,
    /// Account of each repository (`owner/repo`, lowercase): chosen or learned.
    #[serde(deserialize_with = "lenient")]
    pub repo_accounts: std::collections::BTreeMap<String, github::RepoAccount>,
    /// View > Appearance.
    #[serde(deserialize_with = "lenient")]
    pub appearance: AppearanceConfig,
    /// Update checks (6g).
    #[serde(deserialize_with = "lenient")]
    pub updates: UpdatesConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdatesConfig {
    /// Check for a new version at start and every day.
    #[serde(deserialize_with = "lenient_check")]
    pub check: bool,
    /// Version the user chose to skip.
    #[serde(deserialize_with = "lenient")]
    pub skipped: Option<String>,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        UpdatesConfig {
            check: default_check(),
            skipped: None,
        }
    }
}

/// Saved appearance, by name (unknown names fall back to the defaults).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    #[serde(deserialize_with = "lenient_scheme")]
    pub scheme: String,
    #[serde(deserialize_with = "lenient_font")]
    pub font: String,
    /// Interface zoom (1.0 = 100 %).
    #[serde(deserialize_with = "lenient_zoom")]
    pub zoom: f32,
}

pub const MIN_ZOOM: f32 = 0.8;
pub const MAX_ZOOM: f32 = 2.0;

impl Default for AppearanceConfig {
    fn default() -> Self {
        AppearanceConfig {
            scheme: default_scheme(),
            font: default_font(),
            zoom: default_zoom(),
        }
    }
}

// Field defaults: what `Default` gives, and what a wrong value falls back to.
fn default_check() -> bool {
    true
}

fn default_scheme() -> String {
    win95::theme::Appearance::default()
        .scheme
        .name()
        .to_string()
}

fn default_font() -> String {
    win95::theme::Appearance::default().font.name().to_string()
}

fn default_zoom() -> f32 {
    1.0
}

/// A value of the wrong type only resets its own field (to `default`), not the whole file.
fn lenient_or<'de, D: Deserializer<'de>, T: DeserializeOwned>(
    d: D,
    default: fn() -> T,
) -> Result<T, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_else(|e| {
        log::warn!("ignoring a wrong config value: {e}");
        default()
    }))
}

fn lenient<'de, D: Deserializer<'de>, T: DeserializeOwned + Default>(d: D) -> Result<T, D::Error> {
    lenient_or(d, T::default)
}

fn lenient_check<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    lenient_or(d, default_check)
}

fn lenient_scheme<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    lenient_or(d, default_scheme)
}

fn lenient_font<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    lenient_or(d, default_font)
}

fn lenient_zoom<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    lenient_or(d, default_zoom)
}

impl AppearanceConfig {
    pub fn from_choice(a: win95::theme::Appearance, zoom: f32) -> AppearanceConfig {
        AppearanceConfig {
            scheme: a.scheme.name().to_string(),
            font: a.font.name().to_string(),
            zoom,
        }
    }

    pub fn appearance(&self) -> win95::theme::Appearance {
        win95::theme::Appearance {
            scheme: win95::Scheme::from_name(&self.scheme).unwrap_or_default(),
            font: win95::theme::Font::from_name(&self.font).unwrap_or_default(),
        }
    }

    /// The zoom, kept within 80-200 % (100 % if it is not a number).
    pub fn zoom(&self) -> f32 {
        if self.zoom.is_finite() {
            self.zoom.clamp(MIN_ZOOM, MAX_ZOOM)
        } else {
            1.0
        }
    }
}

impl Config {
    /// `<config dir>/RetroGit/config.json`
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("RetroGit").join("config.json"))
    }

    /// Missing or unreadable file => default config. A wrong value only resets its own
    /// field; a file that is not a config at all is logged and kept as `config.json.bad`
    /// (the next save overwrites `config.json`).
    pub fn load_from(path: &Path) -> Config {
        let Ok(bytes) = std::fs::read(path) else {
            return Config::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            let bad = path.with_extension("json.bad");
            log::warn!(
                "ignoring corrupt config {} (kept as {}): {e}",
                path.display(),
                bad.display()
            );
            if let Err(e) = std::fs::write(&bad, &bytes) {
                log::warn!("cannot keep {}: {e}", bad.display());
            }
            Config::default()
        })
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
