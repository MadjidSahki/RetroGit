//! `retrogit --cli <cwd> [path]` (what the installed `retrogit` command runs).

use std::process::Command;
use std::sync::mpsc::channel;
use std::time::Duration;

/// Run the binary with its data directory pointed at `data` (HOME on macOS, LOCALAPPDATA
/// on Windows), so it talks to the test's instance, not a real one.
fn cli(data_home: &std::path::Path, args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_retrogit"))
        .args(args)
        .env("HOME", data_home)
        .env("LOCALAPPDATA", data_home)
        .env("XDG_DATA_HOME", data_home)
        .output()
        .unwrap_or_else(|e| panic!("{e}"))
}

fn data_dir(home: &std::path::Path) -> std::path::PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/RetroGit")
    } else {
        home.join("RetroGit")
    }
}

#[test]
fn the_command_hands_the_repository_root_to_the_running_window() {
    let home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let repo = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    git2::Repository::init(repo.path()).unwrap_or_else(|e| panic!("{e}"));
    let sub = repo.path().join("src/deep");
    std::fs::create_dir_all(&sub).unwrap_or_else(|e| panic!("{e}"));

    let (tx, rx) = channel();
    let _server = retrogit::instance::Server::start(&data_dir(home.path()), move |p| {
        let _ = tx.send(p);
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let out = cli(home.path(), &["--cli".as_ref(), sub.as_os_str()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got = rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|e| panic!("{e}"));
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(canon(&got), canon(repo.path()));
}

#[cfg(unix)]
#[test]
fn outside_a_repository_the_command_fails_with_a_message() {
    let home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let plain = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let out = cli(home.path(), &["--cli".as_ref(), plain.path().as_os_str()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not a Git repository"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
