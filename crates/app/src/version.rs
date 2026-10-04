//! RetroGit's version: the release's (`RETROGIT_VERSION`, set by the CI), else Cargo's.

pub fn version() -> &'static str {
    option_env!("RETROGIT_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// `"0.1.42"` or `"v0.1.42"` as numbers, to compare versions (updates, 6g).
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut parts = v.trim().trim_start_matches('v').split('.');
    let mut next = || parts.next()?.parse().ok();
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}
