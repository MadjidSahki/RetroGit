//! How this copy of RetroGit was installed, which decides how to update it.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// macOS: the `RetroGit.app` bundle at this path.
    MacApp(PathBuf),
    /// Windows, with the installer (an uninstaller next to the exe).
    WindowsInstalled,
    /// Windows, the portable exe at this path.
    WindowsPortable(PathBuf),
    /// `cargo run`, a bare binary: updates are only announced.
    Development,
}

/// Built by Cargo: directly in `target/debug|release` or `target/<triple>/debug|release`
/// (any case, `/` or `\` separators), with Cargo's `deps` or `.fingerprint` folder next to
/// the exe. A copy merely placed under folders with those names is not one.
fn in_cargo_target(exe: &Path, exists: &impl Fn(&Path) -> bool) -> bool {
    let text = exe.to_string_lossy().replace('\\', "/");
    let Some((dir, _)) = text.rsplit_once('/') else {
        return false;
    };
    let lower = dir.to_lowercase();
    let parts: Vec<&str> = lower.split('/').collect();
    let profile = matches!(parts.last(), Some(&"debug" | &"release"));
    let n = parts.len();
    let under_target = (n >= 2 && parts[n - 2] == "target") || (n >= 3 && parts[n - 3] == "target");
    profile
        && under_target
        && ["deps", ".fingerprint"]
            .iter()
            .any(|d| exists(Path::new(&format!("{dir}/{d}"))))
}

/// `os`: `std::env::consts::OS`; `exists` tells whether a file is there.
pub fn install_kind(os: &str, exe: &Path, exists: impl Fn(&Path) -> bool) -> InstallKind {
    if in_cargo_target(exe, &exists) {
        return InstallKind::Development;
    }
    // Windows paths are read with `\` too (tests run on every system).
    let text = exe.to_string_lossy().replace('\\', "/");
    match os {
        "macos" => match text.find(".app/Contents/MacOS/") {
            Some(i) => InstallKind::MacApp(PathBuf::from(&text[..i + 4])),
            None => InstallKind::Development,
        },
        "windows" => {
            let dir = text.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            let uninstaller = format!("{dir}/unins000.exe");
            if exists(Path::new(&uninstaller)) {
                InstallKind::WindowsInstalled
            } else {
                InstallKind::WindowsPortable(exe.to_path_buf())
            }
        }
        _ => InstallKind::Development,
    }
}

/// Release file that updates `kind` (`None`: updates are only announced).
pub fn asset_for(kind: &InstallKind) -> Option<&'static str> {
    match kind {
        InstallKind::MacApp(_) => Some("RetroGit-macos-arm64.zip"),
        InstallKind::WindowsInstalled => Some("RetroGit-windows-x64-setup.exe"),
        InstallKind::WindowsPortable(_) => Some("RetroGit-windows-x64.zip"),
        InstallKind::Development => None,
    }
}
