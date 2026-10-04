#![allow(clippy::unwrap_used)]
//! Updates: the pure parts (release, installation kind, checksums, allowed URLs).

use std::path::{Path, PathBuf};

use retrogit::update::{
    InstallKind, allowed_url, asset_for, install_kind, newer, parse_release, parse_sums,
    remove_with_retries, resolve_location, sha256_hex,
};

const RELEASE: &str = r###"{
  "tag_name": "v0.1.42",
  "name": "RetroGit v0.1.42",
  "html_url": "https://github.com/MadjidSahki/RetroGit/releases/tag/v0.1.42",
  "body": "## What's Changed\n* Explore the code by @MadjidSahki",
  "published_at": "2026-10-05T10:00:00Z",
  "assets": [
    {"name": "RetroGit-macos-arm64.zip", "size": 7000000,
     "browser_download_url": "https://github.com/MadjidSahki/RetroGit/releases/download/v0.1.42/RetroGit-macos-arm64.zip"},
    {"name": "SHA256SUMS.txt", "size": 300,
     "browser_download_url": "https://github.com/MadjidSahki/RetroGit/releases/download/v0.1.42/SHA256SUMS.txt"}
  ]
}"###;

#[test]
fn a_release_is_read_from_the_github_answer() {
    let r = parse_release(RELEASE).unwrap();
    assert_eq!(r.version, "0.1.42");
    assert_eq!(r.tag, "v0.1.42");
    assert!(r.notes.contains("Explore the code"));
    assert!(r.url.ends_with("/tag/v0.1.42"));
    assert_eq!(r.assets.len(), 2);
    assert_eq!(r.asset("SHA256SUMS.txt").unwrap().size, 300);
    assert!(parse_release("{}").is_err());
    assert!(
        parse_release(r#"{"tag_name":"nightly","assets":[]}"#).is_err(),
        "not a version"
    );
}

#[test]
fn only_newer_versions_not_skipped_are_offered() {
    assert!(newer("0.1.39", "0.1.42", None));
    assert!(!newer("0.1.42", "0.1.42", None));
    assert!(
        !newer("0.1.50", "0.1.42", None),
        "a development build ahead of the release"
    );
    assert!(!newer("0.1.39", "0.1.42", Some("0.1.42")), "skipped");
    assert!(
        newer("0.1.39", "0.1.43", Some("0.1.42")),
        "a newer one than the skipped"
    );
    assert!(!newer("garbage", "0.1.42", None));
}

#[test]
fn the_installation_kind_comes_from_where_the_exe_is() {
    let none = |_: &Path| false;
    let mac = Path::new("/Applications/RetroGit.app/Contents/MacOS/retrogit");
    assert_eq!(
        install_kind("macos", mac, none),
        InstallKind::MacApp(PathBuf::from("/Applications/RetroGit.app"))
    );
    assert_eq!(
        install_kind("macos", Path::new("/usr/local/bin/retrogit"), none),
        InstallKind::Development
    );
    assert_eq!(
        install_kind(
            "macos",
            Path::new("/w/RetroGit/target/release/retrogit"),
            none
        ),
        InstallKind::Development
    );
    let installed = Path::new(r"C:\Users\Ada\AppData\Local\Programs\RetroGit\retrogit.exe");
    let has_uninstaller = |p: &Path| p.ends_with("unins000.exe");
    assert_eq!(
        install_kind("windows", installed, has_uninstaller),
        InstallKind::WindowsInstalled
    );
    let portable = Path::new(r"D:\tools\retrogit.exe");
    assert_eq!(
        install_kind("windows", portable, none),
        InstallKind::WindowsPortable(portable.to_path_buf())
    );
    assert_eq!(
        install_kind(
            "windows",
            Path::new(r"C:\w\RetroGit\target\debug\retrogit.exe"),
            none
        ),
        InstallKind::Development
    );
    assert_eq!(
        asset_for(&InstallKind::WindowsInstalled),
        Some("RetroGit-windows-x64-setup.exe")
    );
    assert_eq!(
        asset_for(&InstallKind::WindowsPortable(portable.into())),
        Some("RetroGit-windows-x64.zip")
    );
    assert_eq!(
        asset_for(&InstallKind::MacApp("/A.app".into())),
        Some("RetroGit-macos-arm64.zip")
    );
    assert_eq!(asset_for(&InstallKind::Development), None);
}

#[test]
fn checksums_are_read_and_computed() {
    let sums = parse_sums(
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  RetroGit-macos-arm64.zip\n\
         BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD *RetroGit-windows-x64.zip\n\
         not a line\n",
    );
    assert_eq!(
        sums.get("RetroGit-macos-arm64.zip").map(String::as_str),
        Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
    );
    assert_eq!(
        sums.get("RetroGit-windows-x64.zip").map(String::as_str),
        Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        "lowercased, binary marker removed"
    );
    assert_eq!(sums.len(), 2);
    // FIPS 180-2 test vectors.
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn only_github_download_hosts_are_allowed() {
    for ok in [
        "https://github.com/MadjidSahki/RetroGit/releases/download/v1/x.zip",
        "https://objects.githubusercontent.com/github-production-release-asset/1?x=y",
        "https://release-assets.githubusercontent.com/github-production-release-asset/1",
        "https://api.github.com/repos/MadjidSahki/RetroGit/releases/latest",
    ] {
        assert!(allowed_url(ok), "{ok}");
    }
    for bad in [
        "http://github.com/x",
        "https://github.com.evil.com/x",
        "https://evil.com/github.com",
        "https://user@evil.com/x",
        "https://githubusercontent.com.evil/x",
        "ftp://github.com/x",
    ] {
        assert!(!allowed_url(bad), "{bad}");
    }
}

#[test]
fn redirect_locations_are_resolved_against_the_current_url() {
    let base = "https://github.com/MadjidSahki/RetroGit/releases/download/v1/x.zip?a=1#f";
    // Absolute: unchanged.
    assert_eq!(
        resolve_location(base, "https://objects.githubusercontent.com/a?b=c"),
        "https://objects.githubusercontent.com/a?b=c"
    );
    // Same scheme, other host (still checked by allowed_url).
    assert_eq!(
        resolve_location(base, "//evil.example/x.zip"),
        "https://evil.example/x.zip"
    );
    // Same host.
    assert_eq!(
        resolve_location("http://127.0.0.1:8080/dl/f?x=1", "/storage/f"),
        "http://127.0.0.1:8080/storage/f"
    );
    assert_eq!(
        resolve_location(base, "/storage/y.zip"),
        "https://github.com/storage/y.zip"
    );
    // Relative to the current folder.
    assert_eq!(
        resolve_location(base, "y.zip"),
        "https://github.com/MadjidSahki/RetroGit/releases/download/v1/y.zip"
    );
    assert_eq!(
        resolve_location("https://github.com", "y.zip"),
        "https://github.com/y.zip"
    );
}

#[test]
fn removing_the_old_exe_is_retried_until_it_works_or_time_is_up() {
    let p = Path::new("retrogit.old.exe");
    let tick = std::time::Duration::from_millis(1);
    // Busy three times (the old copy still runs), then removed.
    let mut calls = 0;
    let done = remove_with_retries(p, 10, tick, |_| {
        calls += 1;
        if calls <= 3 {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        } else {
            Ok(())
        }
    });
    assert!(done);
    assert_eq!(calls, 4);
    // Still busy after every attempt: given up.
    let mut calls = 0;
    let done = remove_with_retries(p, 5, tick, |_| {
        calls += 1;
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
    });
    assert!(!done);
    assert_eq!(calls, 5);
    // Already gone: nothing more to do.
    let mut calls = 0;
    let done = remove_with_retries(p, 5, tick, |_| {
        calls += 1;
        Err(std::io::Error::from(std::io::ErrorKind::NotFound))
    });
    assert!(done);
    assert_eq!(calls, 1);
}
