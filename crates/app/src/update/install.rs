//! Installing an update: a download checked against the published checksum, then the
//! replacement of this copy of RetroGit (with the old one put back if anything fails), then
//! a restart.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::{InstallKind, Release, SUMS, allowed_url, asset_for, parse_sums};
use crate::strings as s;

/// Larger downloads are refused.
pub const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;

/// Downloads from allowed hosts only, each redirect checked. No overall time limit (slow
/// connections finish); a download that receives nothing for `stall` fails.
pub struct Fetcher {
    agent: ureq::Agent,
    allow: fn(&str) -> bool,
    stall: Duration,
}

impl Fetcher {
    pub fn new(allow: fn(&str) -> bool) -> Fetcher {
        Fetcher {
            agent: super::check::agent(None, Some(Duration::from_secs(30))),
            allow,
            stall: Duration::from_secs(60),
        }
    }

    /// How long without data before giving up (60 s by default).
    pub fn with_stall(mut self, stall: Duration) -> Fetcher {
        self.stall = stall;
        self
    }

    /// GitHub's hosts only.
    pub fn github() -> Fetcher {
        Fetcher::new(allowed_url)
    }

    /// GET `url` on a helper thread: Cancel is noticed while connecting or waiting for the
    /// headers (the thread is abandoned then; it ends with its own time limits).
    fn call(
        &self,
        url: &str,
        cancel: &AtomicBool,
    ) -> Result<ureq::http::Response<ureq::Body>, String> {
        let (tx, rx) = sync_channel(1);
        let agent = self.agent.clone();
        let target = url.to_string();
        std::thread::Builder::new()
            .name("retrogit-update-request".into())
            .spawn(move || {
                let _ = tx.send(agent.get(&target).call().map_err(|e| e.to_string()));
            })
            .map_err(|e| e.to_string())?;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(s::UPDATE_CANCELLED.into());
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(resp) => return resp,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err(s::ERR_UPDATE_STALLED.into()),
            }
        }
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
                return Err(s::ERR_UPDATE_REFUSED_HOST.replace("{url}", &url));
            }
            if cancel.load(Ordering::SeqCst) {
                return Err(s::UPDATE_CANCELLED.into());
            }
            let resp = self.call(&url, cancel)?;
            let status = resp.status().as_u16();
            if (300..400).contains(&status) {
                let location = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or("redirect without a location")?;
                url = resolve_location(&url, location);
                continue;
            }
            if status != 200 {
                return Err(s::ERR_UPDATE_HTTP.replace("{status}", &status.to_string()));
            }
            let total = resp
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            if total.is_some_and(|t| t > max) {
                return Err(s::ERR_UPDATE_TOO_LARGE.into());
            }
            // Read on a helper thread: Cancel and a stall are noticed while a read blocks
            // (the thread is abandoned then; it ends with the connection).
            let mut reader = resp.into_body().into_with_config().limit(max + 1).reader();
            let (tx, rx) = sync_channel::<Result<Vec<u8>, String>>(4);
            std::thread::Builder::new()
                .name("retrogit-update-download".into())
                .spawn(move || {
                    let mut buf = vec![0u8; 64 * 1024];
                    loop {
                        let chunk = match reader.read(&mut buf) {
                            Ok(0) => return,
                            Ok(n) => Ok(buf[..n].to_vec()),
                            Err(e) => Err(e.to_string()),
                        };
                        let failed = chunk.is_err();
                        if tx.send(chunk).is_err() || failed {
                            return;
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
            let mut done = 0u64;
            let mut last = Instant::now();
            loop {
                if cancel.load(Ordering::SeqCst) {
                    return Err(s::UPDATE_CANCELLED.into());
                }
                let chunk = match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(chunk) => chunk?,
                    Err(RecvTimeoutError::Timeout) => {
                        if last.elapsed() >= self.stall {
                            return Err(s::ERR_UPDATE_STALLED.into());
                        }
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                };
                last = Instant::now();
                let n = chunk.len();
                done += n as u64;
                if done > max {
                    return Err(s::ERR_UPDATE_TOO_LARGE.into());
                }
                sink(&chunk)?;
                progress(done, total);
            }
            return Ok(());
        }
        Err(s::ERR_UPDATE_REDIRECTS.into())
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
    let name = asset_for(kind).ok_or(s::ERR_UPDATE_CANNOT)?;
    let asset = release
        .asset(name)
        .ok_or_else(|| s::ERR_UPDATE_NO_FILE.replace("{name}", name))?;
    let sums_asset = release.asset(SUMS).ok_or(s::ERR_UPDATE_NO_SUMS)?;
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
        .ok_or_else(|| s::ERR_UPDATE_NO_SUM.replace("{name}", name))?
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
            return Err(s::ERR_UPDATE_CHECKSUM.replace("{name}", name));
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

fn step(st: &Step) -> Result<(), String> {
    match st {
        Step::Unzip { zip, into } | Step::ExpandZip { zip, into } => {
            let _ = std::fs::remove_dir_all(into);
            std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
            if matches!(st, Step::Unzip { .. }) {
                run(Command::new("ditto").arg("-x").arg("-k").arg(zip).arg(into))
            } else {
                // Paths through the environment: no quoting to get wrong (curly quotes...).
                let script =
                    "Expand-Archive -LiteralPath $env:RG_ZIP -DestinationPath $env:RG_INTO -Force";
                let mut cmd = Command::new("powershell");
                cmd.args(["-NoProfile", "-NonInteractive", "-Command", script])
                    .env("RG_ZIP", zip)
                    .env("RG_INTO", into);
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
                }
                run(&mut cmd)
            }
        }
        Step::CheckApp { app, version } => {
            run(Command::new("codesign")
                .args(["--verify", "--deep", "--strict"])
                .arg(app))
            .map_err(|e| s::ERR_UPDATE_SIGNATURE.replace("{why}", &e))?;
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
                Err(s::ERR_UPDATE_VERSION
                    .replace("{got}", &got)
                    .replace("{want}", version))
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
        // Started by the app once Git is idle (see `start`).
        Step::Relaunch(_) => Ok(()),
    }
}

/// Run `steps` in order and return the command that starts the new version (left to the
/// app: it waits for Git to be idle). On failure, work folders are removed.
pub fn run_plan(steps: &[Step]) -> Result<Option<Vec<String>>, String> {
    let mut relaunch = None;
    for st in steps {
        if let Step::Relaunch(argv) = st {
            relaunch = Some(argv.clone());
        }
        if let Err(e) = step(st) {
            for st in steps {
                if let Step::Unzip { into, .. } | Step::ExpandZip { into, .. } = st {
                    let _ = std::fs::remove_dir_all(into);
                }
            }
            return Err(e);
        }
    }
    Ok(relaunch)
}

/// Start the new version: `open` waits for macOS to launch the app (an error is seen);
/// a Windows program is started detached.
pub fn start(argv: &[String]) -> Result<(), String> {
    let (prog, args) = argv.split_first().ok_or(s::ERR_UPDATE_CANNOT)?;
    let mut cmd = Command::new(prog);
    cmd.args(args);
    if prog == "open" {
        return run(&mut cmd);
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
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
    // The old exe cannot be deleted while it still runs (it may be exiting while this copy
    // starts): tried again for a minute.
    if matches!(kind, InstallKind::WindowsPortable(_)) && old.exists() {
        let spawned = std::thread::Builder::new()
            .name("retrogit-update-cleanup".into())
            .spawn(move || {
                remove_with_retries(&old, 120, Duration::from_millis(500), |p| {
                    std::fs::remove_file(p)
                });
            });
        if let Err(e) = spawned {
            log::warn!("old copy not removed: {e}");
        }
    }
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
/// Returns the command that starts the new version (`None`: nothing to start).
pub fn install(
    kind: &InstallKind,
    release: &Release,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, Option<u64>),
    downloaded: impl FnOnce(),
) -> Result<Option<Vec<String>>, String> {
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
        downloaded();
        let work = work_dir(kind).unwrap_or_else(|| download.join("work"));
        run_plan(&replace_plan(kind, &file, &work, &release.version))
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

/// A redirect's `Location` made absolute against `base` (the URL that answered):
/// `https://...` as is, `//host/...` with `base`'s scheme, `/path` on `base`'s host,
/// anything else next to `base`'s last path segment.
pub fn resolve_location(base: &str, location: &str) -> String {
    let has_scheme = location.split_once("://").is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
    });
    if has_scheme {
        return location.to_string();
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    if let Some(net) = location.strip_prefix("//") {
        return format!("{scheme}://{net}");
    }
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, "/"),
    };
    if location.starts_with('/') {
        return format!("{scheme}://{authority}{location}");
    }
    let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
    format!("{scheme}://{authority}{dir}{location}")
}

/// Call `remove` on `path` up to `attempts` times, `interval` apart, until it works (or the
/// file is already gone). Returns whether it is gone.
pub fn remove_with_retries(
    path: &Path,
    attempts: u32,
    interval: Duration,
    mut remove: impl FnMut(&Path) -> std::io::Result<()>,
) -> bool {
    for i in 0..attempts {
        match remove(path) {
            Ok(()) => return true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
            Err(_) if i + 1 < attempts => std::thread::sleep(interval),
            Err(_) => {}
        }
    }
    false
}
