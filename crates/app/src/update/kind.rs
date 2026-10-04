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

/// Inside a Cargo `target/debug` or `target/release` folder (`/` or `\` separators).
fn in_cargo_target(exe: &Path) -> bool {
    let text = exe.to_string_lossy().replace('\\', "/").to_lowercase();
    let parts: Vec<&str> = text.split('/').collect();
    parts
        .windows(2)
        .any(|w| w[0] == "target" && (w[1] == "debug" || w[1] == "release"))
        || parts
            .windows(3)
            .any(|w| w[0] == "target" && (w[2] == "debug" || w[2] == "release"))
}

/// `os`: `std::env::consts::OS`; `exists` tells whether a file is there.
pub fn install_kind(os: &str, exe: &Path, exists: impl Fn(&Path) -> bool) -> InstallKind {
    if in_cargo_target(exe) {
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
