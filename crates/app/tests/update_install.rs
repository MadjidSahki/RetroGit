#![allow(clippy::unwrap_used)]
//! Updates: verified download (mock server), the replacement plan, and replacing a real
//! RetroGit.app on macOS.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use retrogit::update::{
    Asset, Fetcher, InstallKind, Release, Step, download_verified, replace_plan, run_plan,
    sha256_hex,
};

static NO: AtomicBool = AtomicBool::new(false);

fn release_on(server: &mockito::ServerGuard, file: &str) -> Release {
    Release {
        version: "0.1.99".into(),
        tag: "v0.1.99".into(),
        notes: String::new(),
        url: String::new(),
        assets: vec![
            Asset {
                name: file.into(),
                url: format!("{}/dl/{file}", server.url()),
                size: 4,
            },
            Asset {
                name: "SHA256SUMS.txt".into(),
                url: format!("{}/dl/sums", server.url()),
                size: 99,
            },
        ],
    }
}

fn local_only(url: &str) -> bool {
    url.starts_with("http://127.0.0.1:")
}

#[test]
fn the_download_is_checked_against_the_published_checksum() {
    let mut server = mockito::Server::new();
    let file = "RetroGit-windows-x64-setup.exe";
    let r = release_on(&server, file);
    server
        .mock("GET", "/dl/sums")
        .with_body(format!("{}  {file}\n", sha256_hex(b"good")))
        .create();
    // The asset is served after a redirect, like GitHub does.
    server
        .mock("GET", format!("/dl/{file}").as_str())
        .with_status(302)
        .with_header("location", &format!("{}/storage/{file}", server.url()))
        .create();
    let body = server
        .mock("GET", format!("/storage/{file}").as_str())
        .with_body("good")
        .create();
    let dir = tempfile::tempdir().unwrap();
    let fetcher = Fetcher::new(local_only);
    let mut seen = Vec::new();
    let path = download_verified(
        &fetcher,
        &r,
        &InstallKind::WindowsInstalled,
        dir.path(),
        &NO,
        |done, _| seen.push(done),
    )
    .unwrap();
    body.assert();
    assert_eq!(std::fs::read(&path).unwrap(), b"good");
    assert_eq!(seen.last(), Some(&4));
    // Another body: refused, and nothing left behind.
    server.reset();
    server
        .mock("GET", "/dl/sums")
        .with_body(format!("{}  {file}\n", sha256_hex(b"good")))
        .create();
    server
        .mock("GET", format!("/dl/{file}").as_str())
        .with_body("evil")
        .create();
    let dir2 = tempfile::tempdir().unwrap();
    let err = download_verified(
        &fetcher,
        &r,
        &InstallKind::WindowsInstalled,
        dir2.path(),
        &NO,
        |_, _| {},
    )
    .unwrap_err();
    assert!(err.contains("checksum"), "{err}");
    assert!(
        err.starts_with(&retrogit::strings::ERR_UPDATE_CHECKSUM.replace("{name}", file)),
        "{err}"
    );
    assert_eq!(std::fs::read_dir(dir2.path()).unwrap().count(), 0);
}

#[test]
fn redirects_outside_the_allowed_hosts_and_missing_checksums_are_refused() {
    let mut server = mockito::Server::new();
    let file = "RetroGit-windows-x64.zip";
    let r = release_on(&server, file);
    server
        .mock("GET", "/dl/sums")
        .with_body(format!("{}  {file}\n", sha256_hex(b"x")))
        .create();
    server
        .mock("GET", format!("/dl/{file}").as_str())
        .with_status(302)
        .with_header("location", "https://evil.example/x.zip")
        .create();
    let dir = tempfile::tempdir().unwrap();
    let fetcher = Fetcher::new(local_only);
    let kind = InstallKind::WindowsPortable(PathBuf::from(r"D:\r\retrogit.exe"));
    let err = download_verified(&fetcher, &r, &kind, dir.path(), &NO, |_, _| {}).unwrap_err();
    assert!(err.contains("evil.example"), "{err}");
    // No checksum for the file: refused before downloading it.
    server.reset();
    server.mock("GET", "/dl/sums").with_body("").create();
    let err = download_verified(&fetcher, &r, &kind, dir.path(), &NO, |_, _| {}).unwrap_err();
    assert!(err.contains("no checksum"), "{err}");
    let cancelled = AtomicBool::new(true);
    assert!(download_verified(&fetcher, &r, &kind, dir.path(), &cancelled, |_, _| {}).is_err());
}

#[test]
fn each_installation_has_its_plan() {
    let work = Path::new("/w");
    let mac = InstallKind::MacApp(PathBuf::from("/Applications/RetroGit.app"));
    let plan = replace_plan(&mac, Path::new("/t/u.zip"), work, "0.1.99");
    assert_eq!(
        plan.first(),
        Some(&Step::Unzip {
            zip: "/t/u.zip".into(),
            into: "/w".into()
        })
    );
    assert!(plan.contains(&Step::CheckApp {
        app: "/w/RetroGit.app".into(),
        version: "0.1.99".into()
    }));
    let swap = plan
        .iter()
        .position(|s| matches!(s, Step::Swap { .. }))
        .unwrap();
    let clean = plan
        .iter()
        .position(|s| matches!(s, Step::RemoveQuarantine(_)))
        .unwrap();
    assert!(clean < swap, "quarantine removed before the swap");
    assert_eq!(
        plan.last(),
        Some(&Step::Relaunch(vec![
            "open".into(),
            "-n".into(),
            "/Applications/RetroGit.app".into()
        ]))
    );
    let setup = replace_plan(
        &InstallKind::WindowsInstalled,
        Path::new(r"C:\t\setup.exe"),
        work,
        "0.1.99",
    );
    assert_eq!(
        setup,
        [Step::Relaunch(vec![
            r"C:\t\setup.exe".into(),
            "/VERYSILENT".into(),
            "/SUPPRESSMSGBOXES".into(),
            "/NORESTART".into(),
            "/RELAUNCH".into()
        ])]
    );
    let exe = PathBuf::from(r"D:\r\retrogit.exe");
    let portable = replace_plan(
        &InstallKind::WindowsPortable(exe.clone()),
        Path::new(r"C:\t\p.zip"),
        work,
        "0.1.99",
    );
    assert!(matches!(portable.first(), Some(Step::ExpandZip { .. })));
    assert!(
        portable
            .iter()
            .any(|s| matches!(s, Step::Swap { current, .. } if *current == exe))
    );
    assert_eq!(
        portable.last(),
        Some(&Step::Relaunch(vec![exe.display().to_string()]))
    );
    assert!(replace_plan(&InstallKind::Development, Path::new("/x"), work, "1").is_empty());
}

/// A RetroGit.app of `version` made by the release script around a stand-in binary.
#[cfg(target_os = "macos")]
fn packaged(version: &str, out: &Path) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::create_dir_all(out).unwrap();
    let bin = out.join("fake-retrogit");
    std::fs::copy("/usr/bin/true", &bin).unwrap();
    let status = std::process::Command::new(root.join("scripts/package-macos.sh"))
        .args([bin.to_str().unwrap(), version, out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    out.join("RetroGit-macos-arm64.zip")
}

#[cfg(target_os = "macos")]
fn bundle_version(app: &Path) -> String {
    let out = std::process::Command::new("/usr/libexec/PlistBuddy")
        .args([
            "-c",
            "Print CFBundleVersion",
            app.join("Contents/Info.plist").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[cfg(target_os = "macos")]
#[test]
fn a_mac_app_is_replaced_in_place_or_left_untouched() {
    let d = tempfile::tempdir().unwrap();
    let installed_dir = d.path().join("Applications");
    std::fs::create_dir_all(&installed_dir).unwrap();
    let old_zip = packaged("0.1.98", &d.path().join("old"));
    std::process::Command::new("ditto")
        .args([
            "-x",
            "-k",
            old_zip.to_str().unwrap(),
            installed_dir.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    let app = installed_dir.join("RetroGit.app");
    assert_eq!(bundle_version(&app), "0.1.98");
    let new_zip = packaged("0.1.99", &d.path().join("new"));
    let kind = InstallKind::MacApp(app.clone());
    // Wrong version announced: stopped before touching the installed app.
    let work = installed_dir.join(".RetroGit-update");
    let wrong = replace_plan(&kind, &new_zip, &work, "0.1.100");
    assert!(run_plan(&wrong).is_err());
    assert_eq!(bundle_version(&app), "0.1.98");
    assert!(!work.exists(), "work folder cleaned");
    // Right one: replaced, nothing left beside it.
    let plan = replace_plan(&kind, &new_zip, &work, "0.1.99");
    let relaunch = run_plan(&plan).unwrap();
    assert_eq!(
        relaunch.unwrap()[0],
        "open",
        "the restart is left to the app"
    );
    assert_eq!(bundle_version(&app), "0.1.99");
    let names: Vec<String> = std::fs::read_dir(&installed_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["RetroGit.app"]);
}

#[test]
fn a_failed_swap_puts_the_current_file_back() {
    let d = tempfile::tempdir().unwrap();
    let current = d.path().join("retrogit.exe");
    std::fs::write(&current, "old").unwrap();
    let missing = d.path().join("nowhere").join("retrogit.exe");
    let steps = [Step::Swap {
        new: missing,
        current: current.clone(),
    }];
    assert!(run_plan(&steps).is_err());
    assert_eq!(std::fs::read_to_string(&current).unwrap(), "old");
    let new = d.path().join("new.exe");
    std::fs::write(&new, "new").unwrap();
    run_plan(&[Step::Swap {
        new,
        current: current.clone(),
    }])
    .unwrap();
    assert_eq!(std::fs::read_to_string(&current).unwrap(), "new");
}
