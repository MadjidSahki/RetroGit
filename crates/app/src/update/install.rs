//! Installing an update: a download checked against the published checksum, then the
//! replacement of this copy of RetroGit (with the old one put back if anything fails), then
//! a restart.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::{InstallKind, Release, SUMS, allowed_url, asset_for, parse_sums};

/// Larger downloads are refused.
pub const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;

/// Downloads from allowed hosts only, each redirect checked.
pub struct Fetcher {
    agent: ureq::Agent,
    allow: fn(&str) -> bool,
}

impl Fetcher {
    pub fn new(allow: fn(&str) -> bool) -> Fetcher {
        Fetcher {
            agent: super::check::agent(Duration::from_secs(600)),
            allow,
        }
    }

    /// GitHub's hosts only.
    pub fn github() -> Fetcher {
        Fetcher::new(allowed_url)
    }

    /// Stream `url` (redirects followed by hand) into `sink`, `max` bytes at most.
    fn get(
        &self,
        url: &str,
        max: u64,
        cancel: &AtomicBool,
        mut sink: impl FnMut(&[u8]) -> Result<(), String>,
        mut progress: impl FnMut(u64, Option<u64>),
    ) -> Result<(), String> {
        let mut url = url.to_string();
        for _ in 0..5 {
            if !(self.allow)(&url) {
                return Err(format!("refused download from {url}"));
            }
            if cancel.load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            let mut resp = self.agent.get(&url).call().map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            if (300..400).contains(&status) {
                url = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or("redirect without a location")?
                    .to_string();
                continue;
            }
            if status != 200 {
                return Err(format!("download failed: HTTP {status}"));
            }
            let total = resp
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            if total.is_some_and(|t| t > max) {
                return Err("download too large".into());
            }
            let mut reader = resp.body_mut().with_config().limit(max + 1).reader();
            let mut buf = vec![0u8; 64 * 1024];
            let mut done = 0u64;
            loop {
                if cancel.load(Ordering::SeqCst) {
                    return Err("cancelled".into());
                }
                let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                done += n as u64;
                if done > max {
                    return Err("download too large".into());
                }
                sink(&buf[..n])?;
                progress(done, total);
            }
            return Ok(());
        }
        Err("too many redirects".into())
    }
}

/// Download the file `kind` needs into `dir` and check it against the release's
/// `SHA256SUMS.txt`; nothing is left in `dir` if it fails.
pub fn download_verified(
    fetcher: &Fetcher,
    release: &Release,
    kind: &InstallKind,
    dir: &Path,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, String> {
    let name = asset_for(kind).ok_or("this copy of RetroGit cannot update itself")?;
    let asset = release
        .asset(name)
        .ok_or_else(|| format!("the release has no {name}"))?;
    let sums_asset = release.asset(SUMS).ok_or("the release has no checksums")?;
    let mut sums_text = Vec::new();
    fetcher.get(
        &sums_asset.url,
        1024 * 1024,
        cancel,
        |b| {
            sums_text.extend_from_slice(b);
            Ok(())
        },
        |_, _| {},
    )?;
    let sums = parse_sums(&String::from_utf8_lossy(&sums_text));
    let expected = sums
        .get(name)
        .ok_or_else(|| format!("no checksum for {name}"))?
        .clone();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(name);
    let result = (|| {
        let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        fetcher.get(
            &asset.url,
            MAX_DOWNLOAD,
            cancel,
            |b| {
                hasher.update(b);
                file.write_all(b).map_err(|e| e.to_string())
            },
            progress,
        )?;
        let got: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if got != expected {
            return Err(format!("checksum mismatch for {name}"));
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(path),
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            Err(e)
        }
    }
}

/// One operation of a replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// macOS: `ditto -x -k` (keeps the signature).
    Unzip {
        zip: PathBuf,
        into: PathBuf,
    },
    /// Windows: PowerShell's `Expand-Archive`.
    ExpandZip {
        zip: PathBuf,
        into: PathBuf,
    },
    /// macOS: the signature holds and `Info.plist` has this version.
    CheckApp {
        app: PathBuf,
        version: String,
    },
    /// macOS: no Gatekeeper prompt for the updated app.
    RemoveQuarantine(PathBuf),
    /// `current` becomes its `.old` sibling, `new` takes its place; the old one is put back
    /// if that fails, and deleted after (or at the next start when it is still running).
    Swap {
        new: PathBuf,
        current: PathBuf,
    },
    RemoveDir(PathBuf),
    /// Start this command, detached (skipped by tests).
    Relaunch(Vec<String>),
}

/// `RetroGit.app` -> `RetroGit.app.old`; `retrogit.exe` -> `retrogit.old.exe`.
pub fn old_path(current: &Path) -> PathBuf {
    let name = current
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let old = match name.strip_suffix(".exe") {
        Some(stem) => format!("{stem}.old.exe"),
        None => format!("{name}.old"),
    };
    current.with_file_name(old)
}

/// What replacing this copy with the downloaded `file` takes; `work`: a folder next to the
/// installed app (same disk, so renames work).
pub fn replace_plan(kind: &InstallKind, file: &Path, work: &Path, version: &str) -> Vec<Step> {
    match kind {
        InstallKind::MacApp(app) => {
            let new = work.join("RetroGit.app");
            vec![
                Step::Unzip {
                    zip: file.into(),
                    into: work.into(),
                },
                Step::CheckApp {
                    app: new.clone(),
                    version: version.into(),
                },
                Step::RemoveQuarantine(new.clone()),
                Step::Swap {
                    new,
                    current: app.clone(),
                },
                Step::RemoveDir(work.into()),
                Step::Relaunch(vec!["open".into(), "-n".into(), app.display().to_string()]),
            ]
        }
        InstallKind::WindowsInstalled => vec![Step::Relaunch(vec![
            file.display().to_string(),
            "/VERYSILENT".into(),
            "/SUPPRESSMSGBOXES".into(),
            "/NORESTART".into(),
            "/RELAUNCH".into(),
        ])],
        InstallKind::WindowsPortable(exe) => vec![
            Step::ExpandZip {
                zip: file.into(),
                into: work.into(),
            },
            Step::Swap {
                new: work.join("retrogit.exe"),
                current: exe.clone(),
            },
            Step::RemoveDir(work.into()),
            Step::Relaunch(vec![exe.display().to_string()]),
        ],
        InstallKind::Development => Vec::new(),
    }
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn step(s: &Step, relaunch: bool) -> Result<(), String> {
    match s {
        Step::Unzip { zip, into } | Step::ExpandZip { zip, into } => {
            let _ = std::fs::remove_dir_all(into);
            std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
            if matches!(s, Step::Unzip { .. }) {
                run(Command::new("ditto").arg("-x").arg("-k").arg(zip).arg(into))
            } else {
                let script = format!(
                    "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                    zip.display().to_string().replace('\'', "''"),
                    into.display().to_string().replace('\'', "''")
                );
                run(Command::new("powershell").args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &script,
                ]))
            }
        }
        Step::CheckApp { app, version } => {
            run(Command::new("codesign")
                .args(["--verify", "--deep", "--strict"])
                .arg(app))
            .map_err(|e| format!("the new app's signature does not hold: {e}"))?;
            let plist = app.join("Contents/Info.plist");
            let out = Command::new("/usr/libexec/PlistBuddy")
                .args(["-c", "Print CFBundleVersion"])
                .arg(&plist)
                .output()
                .map_err(|e| e.to_string())?;
            let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if got == *version {
                Ok(())
            } else {
                Err(format!("the downloaded app is {got}, not {version}"))
            }
        }
        Step::RemoveQuarantine(app) => run(Command::new("xattr").arg("-cr").arg(app)),
        Step::Swap { new, current } => {
            let old = old_path(current);
            let _ = std::fs::remove_dir_all(&old);
            let _ = std::fs::remove_file(&old);
            std::fs::rename(current, &old).map_err(|e| e.to_string())?;
            if let Err(e) = std::fs::rename(new, current) {
                let _ = std::fs::rename(&old, current);
                return Err(e.to_string());
            }
            // A running exe cannot be deleted on Windows: removed at the next start.
            let _ = std::fs::remove_dir_all(&old);
            let _ = std::fs::remove_file(&old);
            Ok(())
        }
        Step::RemoveDir(dir) => {
            let _ = std::fs::remove_dir_all(dir);
            Ok(())
        }
        Step::Relaunch(argv) if relaunch => {
            let (prog, args) = argv.split_first().ok_or("nothing to start")?;
            Command::new(prog)
                .args(args)
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        Step::Relaunch(_) => Ok(()),
    }
}

/// Run `steps` in order; on failure, work folders are removed and nothing is restarted.
pub fn run_plan(steps: &[Step], relaunch: bool) -> Result<(), String> {
    for s in steps {
        if let Err(e) = step(s, relaunch) {
            for s in steps {
                if let Step::Unzip { into, .. } | Step::ExpandZip { into, .. } = s {
                    let _ = std::fs::remove_dir_all(into);
                }
            }
            return Err(e);
        }
    }
    Ok(())
}

/// At start: what an earlier update left (the `.old` copy, the work folder).
pub fn cleanup(kind: &InstallKind) {
    let current = match kind {
        InstallKind::MacApp(app) => app.clone(),
        InstallKind::WindowsPortable(exe) => exe.clone(),
        _ => return,
    };
    let old = old_path(&current);
    let _ = std::fs::remove_dir_all(&old);
    let _ = std::fs::remove_file(&old);
    if let Some(dir) = work_dir(kind) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// This copy's folder can be written (else the update window offers Download).
pub fn can_replace(kind: &InstallKind) -> bool {
    let dir = match kind {
        InstallKind::MacApp(app) => app.parent().map(Path::to_path_buf),
        InstallKind::WindowsPortable(exe) => exe.parent().map(Path::to_path_buf),
        InstallKind::WindowsInstalled => return true,
        InstallKind::Development => return false,
    };
    let Some(dir) = dir else { return false };
    let probe = dir.join(format!(".retrogit-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Download, check and put in place `release`, then start it (on the calling thread).
pub fn install(
    kind: &InstallKind,
    release: &Release,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<(), String> {
    let download = std::env::temp_dir().join(format!("RetroGit-update-{}", std::process::id()));
    let result = (|| {
        let file = download_verified(
            &Fetcher::github(),
            release,
            kind,
            &download,
            cancel,
            progress,
        )?;
        let work = work_dir(kind).unwrap_or_else(|| download.join("work"));
        run_plan(&replace_plan(kind, &file, &work, &release.version), true)
    })();
    // The Windows installer still reads its file: it lives in the temporary folder.
    if !(result.is_ok() && *kind == InstallKind::WindowsInstalled) {
        let _ = std::fs::remove_dir_all(&download);
    }
    result
}

/// Folder where the new version is unpacked: next to this copy (same disk).
pub fn work_dir(kind: &InstallKind) -> Option<PathBuf> {
    match kind {
        InstallKind::MacApp(app) => app.parent().map(|p| p.join(".RetroGit-update")),
        InstallKind::WindowsPortable(exe) => exe.parent().map(|p| p.join(".retrogit-update")),
        _ => None,
    }
}
