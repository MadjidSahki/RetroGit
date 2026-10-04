//! The latest release, as GitHub's API describes it.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// `0.1.42` (the tag without its `v`).
    pub version: String,
    pub tag: String,
    /// Release notes (Markdown).
    pub notes: String,
    /// Page of the release on github.com.
    pub url: String,
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

#[derive(Deserialize)]
struct RawAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

/// `GET /repos/{repo}/releases/latest` answer.
pub fn parse_release(json: &str) -> Result<Release, String> {
    let raw: RawRelease = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let version = raw.tag_name.trim_start_matches('v').to_string();
    if crate::version::parse_version(&version).is_none() {
        return Err(format!("not a version: {}", raw.tag_name));
    }
    Ok(Release {
        version,
        tag: raw.tag_name,
        notes: raw.body.unwrap_or_default(),
        url: raw.html_url,
        assets: raw
            .assets
            .into_iter()
            .map(|a| Asset {
                name: a.name,
                url: a.browser_download_url,
                size: a.size,
            })
            .collect(),
    })
}

/// `release` is newer than `current` and not the version the user chose to skip.
pub fn newer(current: &str, release: &str, skipped: Option<&str>) -> bool {
    use crate::version::parse_version;
    if skipped == Some(release) {
        return false;
    }
    match (parse_version(current), parse_version(release)) {
        (Some(c), Some(r)) => r > c,
        _ => false,
    }
}
