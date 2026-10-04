//! Updates: find a newer release on GitHub, download it, check it, put it in place.

mod kind;
mod release;
mod verify;

pub use kind::{InstallKind, asset_for, install_kind};
pub use release::{Asset, Release, newer, parse_release};
pub use verify::{allowed_url, parse_sums, sha256_hex};

/// GitHub repository of the releases.
pub const REPO: &str = "MadjidSahki/RetroGit";
/// Name of the checksums file published with each release.
pub const SUMS: &str = "SHA256SUMS.txt";
