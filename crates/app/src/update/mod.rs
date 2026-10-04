//! Updates: find a newer release on GitHub, download it, check it, put it in place.

mod check;
mod install;
mod kind;
mod release;
mod verify;

pub use check::{Checker, fetch_latest};
pub use install::{
    Fetcher, MAX_DOWNLOAD, Step, can_replace, cleanup, download_verified, install, old_path,
    replace_plan, run_plan, work_dir,
};
pub use kind::{InstallKind, asset_for, install_kind};
pub use release::{Asset, Release, newer, parse_release};
pub use verify::{allowed_url, parse_sums, sha256_hex};

/// The release has what this installation needs to update itself (its file and the
/// checksums); otherwise the update window offers Download.
pub fn installable(kind: &InstallKind, release: &Release) -> bool {
    asset_for(kind).is_some_and(|name| release.asset(name).is_some())
        && release.asset(SUMS).is_some()
}

/// GitHub repository of the releases.
pub const REPO: &str = "MadjidSahki/RetroGit";
/// Name of the checksums file published with each release.
pub const SUMS: &str = "SHA256SUMS.txt";
