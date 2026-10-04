//! The `retrogit` command line: arguments, and installing the command in the PATH.

use std::path::{Path, PathBuf};

/// How the binary was started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// Normal window, optionally opening a folder at start (`--open <dir>`).
    Gui { open: Option<PathBuf> },
    /// From the installed `retrogit` command: `--cli <cwd> [path]`.
    Cli { target: PathBuf },
    /// A clicked notification (`retrogit://...`, Windows): open its pull request.
    Link { link: String },
}

/// Parse `std::env::args()` (unknown arguments, such as macOS `-psn_...`, are ignored).
pub fn parse(args: &[String]) -> Launch {
    // A `retrogit:` link (Windows hands it to us raw, quotes included) decides alone: the
    // other arguments may have been smuggled into the link (`retrogit:" --open "\\host\x`).
    if let Some(link) = args.iter().skip(1).find(|a| is_link(a)) {
        return if link.to_ascii_lowercase().starts_with("retrogit://") {
            Launch::Link { link: link.clone() }
        } else {
            Launch::Gui { open: None }
        };
    }
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--open" => {
                return Launch::Gui {
                    open: it.next().map(PathBuf::from),
                };
            }
            "--cli" => {
                let cwd = PathBuf::from(it.next().map(String::as_str).unwrap_or("."));
                let target = match it.next() {
                    Some(p) => cwd.join(p),
                    None => cwd,
                };
                return Launch::Cli { target };
            }
            _ => {}
        }
    }
    Launch::Gui { open: None }
}

fn is_link(a: &str) -> bool {
    a.get(..9)
        .is_some_and(|p| p.eq_ignore_ascii_case("retrogit:"))
}

/// What another instance (or the command line) asked to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requested {
    Folder(PathBuf),
    /// A notification link (`retrogit://...`).
    Link(String),
}

/// A link travels as the "folder" of the single-instance channel: tell them apart.
pub fn route(path: PathBuf) -> Requested {
    let text = path.to_string_lossy();
    if is_link(&text) {
        Requested::Link(text.into_owned())
    } else {
        Requested::Folder(path)
    }
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `/usr/local/bin/retrogit` on macOS: runs the app's binary with the current directory.
pub fn mac_install_script(exe: &Path) -> String {
    format!(
        "#!/bin/sh\nexec {} --cli \"$PWD\" \"$@\"\n",
        sh_quote(&exe.display().to_string())
    )
}

/// AppleScript that installs `script_file` as `/usr/local/bin/retrogit` (asks for an admin password).
pub fn mac_install_applescript(script_file: &Path) -> String {
    let shell = format!(
        "mkdir -p /usr/local/bin && cp {} /usr/local/bin/retrogit && chmod 755 /usr/local/bin/retrogit",
        sh_quote(&script_file.display().to_string())
    );
    let escaped = shell.replace('\\', "\\\\").replace('"', "\\\"");
    format!("do shell script \"{escaped}\" with administrator privileges")
}

/// `retrogit.cmd` placed in a folder on the user's PATH (Windows). ASCII only: cmd reads
/// batch files in the OEM code page, so a path under `%LOCALAPPDATA%` is written with the
/// variable (expanded by cmd in UTF-16). `"%CD%\."` stays valid at a drive root (`C:\`).
pub fn windows_shim(exe: &Path, local_app_data: Option<&Path>) -> String {
    let exe = exe.display().to_string();
    // Compared as text, case-insensitively (Windows paths).
    let rest = local_app_data.and_then(|l| {
        let prefix = format!("{}\\", l.display().to_string().trim_end_matches('\\'));
        exe.to_lowercase()
            .starts_with(&prefix.to_lowercase())
            .then(|| exe[prefix.len()..].to_string())
    });
    let exe_text = match rest {
        Some(rest) => format!("%LOCALAPPDATA%\\{}", rest.replace('%', "%%")),
        None => exe.replace('%', "%%"),
    };
    format!("@\"{exe_text}\" --cli \"%CD%\\.\" %*\r\n")
}

/// PowerShell that appends `$env:RETROGIT_BIN` to the *user* PATH in the registry, keeping
/// `%VARS%` unexpanded and the value type `REG_EXPAND_SZ`; stops on any error (never writes
/// a PATH it could not read).
pub fn windows_path_script() -> &'static str {
    "$ErrorActionPreference = 'Stop'\n\
     $key = Get-Item -Path 'HKCU:\\Environment'\n\
     $path = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')\n\
     $dir = $env:RETROGIT_BIN\n\
     $parts = @($path -split ';' | Where-Object { $_ -ne '' })\n\
     if ($parts | Where-Object { $_.TrimEnd('\\').ToLower() -eq $dir.TrimEnd('\\').ToLower() }) { exit 0 }\n\
     Set-ItemProperty -Path 'HKCU:\\Environment' -Name 'Path' -Type ExpandString -Value (($parts + $dir) -join ';')\n\
     [Environment]::SetEnvironmentVariable('RETROGIT_PATH_REFRESH', '1', 'User')\n\
     [Environment]::SetEnvironmentVariable('RETROGIT_PATH_REFRESH', $null, 'User')\n"
}

/// Why the command cannot be installed from where RetroGit runs (macOS temporary locations).
pub fn install_blocker(exe: &Path) -> Option<&'static str> {
    let p = exe.to_string_lossy();
    (p.contains("/AppTranslocation/") || p.starts_with("/Volumes/"))
        .then_some(crate::strings::ERR_INSTALL_FROM_TEMP)
}

/// `existing` user PATH plus `dir`, or `None` if `dir` is already there.
pub fn merge_path(existing: &str, dir: &str) -> Option<String> {
    let norm = |p: &str| p.trim().trim_end_matches(['\\', '/']).to_ascii_lowercase();
    let parts: Vec<&str> = existing
        .split(';')
        .filter(|p| !p.trim().is_empty())
        .collect();
    if parts.iter().any(|p| norm(p) == norm(dir)) {
        return None;
    }
    let mut out: Vec<&str> = parts;
    out.push(dir);
    Some(out.join(";"))
}

/// Install the `retrogit` command for the current user. `Ok(None)` = cancelled by the user.
/// Blocking (password prompt, PowerShell): run it off the UI thread.
pub fn install_command_line_tool() -> Result<Option<String>, String> {
    use crate::strings as s;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if let Some(why) = install_blocker(&exe) {
        return Err(why.to_string());
    }
    #[cfg(target_os = "macos")]
    {
        let tmp = std::env::temp_dir().join(format!("retrogit-cli-{}", std::process::id()));
        std::fs::write(&tmp, mac_install_script(&exe)).map_err(|e| e.to_string())?;
        let out = std::process::Command::new("osascript")
            .args(["-e", &mac_install_applescript(&tmp)])
            .output()
            .map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&tmp);
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if out.status.success() {
            Ok(Some(s::INSTALLED_CLI_MAC.to_string()))
        } else if err.contains("-128") {
            Ok(None) // "User canceled."
        } else {
            Err(err)
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| s::ERR_NO_LOCALAPPDATA.to_string())?;
        let bin = local.join("RetroGit").join("bin");
        std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
        std::fs::write(bin.join("retrogit.cmd"), windows_shim(&exe, Some(&local)))
            .map_err(|e| e.to_string())?;
        let out = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                windows_path_script(),
            ])
            .env("RETROGIT_BIN", &bin)
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(Some(s::INSTALLED_CLI_WINDOWS.to_string()))
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = exe;
        Err(s::ERR_PLATFORM.to_string())
    }
}

/// Repository root containing `path` (searching parent folders like git does).
pub fn repo_root(path: &Path) -> Result<PathBuf, String> {
    gitcore::Repo::discover(path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn arguments() {
        let a = |v: &[&str]| parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&["retrogit"]), Launch::Gui { open: None });
        assert_eq!(
            a(&["retrogit", "--open", "/r"]),
            Launch::Gui {
                open: Some(PathBuf::from("/r"))
            }
        );
        assert_eq!(
            a(&["retrogit", "--cli", "/cwd"]),
            Launch::Cli {
                target: PathBuf::from("/cwd")
            }
        );
        assert_eq!(
            a(&["retrogit", "--cli", "/cwd", "sub/dir"]),
            Launch::Cli {
                target: PathBuf::from("/cwd/sub/dir")
            }
        );
        assert_eq!(
            a(&["retrogit", "--cli", "/cwd", "/abs"]),
            Launch::Cli {
                target: PathBuf::from("/abs")
            }
        );
        // macOS passes -psn_... when started by the Finder: ignored.
        assert_eq!(a(&["retrogit", "-psn_0_12345"]), Launch::Gui { open: None });
    }

    #[test]
    fn mac_installer_quotes_paths_safely() {
        let script = mac_install_script(Path::new("/Apps/Retro Git's/retrogit"));
        assert_eq!(
            script,
            "#!/bin/sh\nexec '/Apps/Retro Git'\\''s/retrogit' --cli \"$PWD\" \"$@\"\n"
        );
        let osa = mac_install_applescript(Path::new("/tmp/x y/retrogit-cli"));
        assert!(osa.starts_with("do shell script \""), "{osa}");
        assert!(osa.ends_with("\" with administrator privileges"), "{osa}");
        assert!(
            osa.contains("cp '/tmp/x y/retrogit-cli' /usr/local/bin/retrogit"),
            "{osa}"
        );
    }

    #[test]
    fn windows_shim_works_at_a_drive_root_and_with_accented_user_folders() {
        let local = Path::new(r"C:\Users\Hélène\AppData\Local");
        let shim = windows_shim(
            Path::new(r"C:\Users\Hélène\AppData\Local\RetroGit\retrogit.exe"),
            Some(local),
        );
        assert_eq!(
            shim,
            "@\"%LOCALAPPDATA%\\RetroGit\\retrogit.exe\" --cli \"%CD%\\.\" %*\r\n"
        );
        assert!(
            shim.is_ascii(),
            "cmd reads batch files in the OEM code page"
        );
        let other = windows_shim(Path::new(r"D:\Tools\100%\retrogit.exe"), Some(local));
        assert_eq!(
            other,
            "@\"D:\\Tools\\100%%\\retrogit.exe\" --cli \"%CD%\\.\" %*\r\n"
        );
    }

    #[test]
    fn windows_path_update_never_round_trips_the_path_through_text() {
        let script = windows_path_script();
        assert!(
            script.contains("DoNotExpandEnvironmentNames"),
            "keep %VARS% unexpanded"
        );
        assert!(script.contains("-Type ExpandString"), "keep REG_EXPAND_SZ");
        assert!(
            script.contains("$env:RETROGIT_BIN"),
            "folder passed by environment, not interpolated"
        );
        assert!(
            script.starts_with("$ErrorActionPreference = 'Stop'"),
            "abort if the PATH cannot be read"
        );
    }

    #[test]
    fn installing_from_a_temporary_location_is_refused() {
        assert!(
            install_blocker(Path::new(
                "/private/var/folders/x/T/AppTranslocation/U/d/RetroGit.app/Contents/MacOS/retrogit"
            ))
            .is_some()
        );
        assert!(install_blocker(Path::new("/Volumes/RetroGit/retrogit")).is_some());
        assert!(install_blocker(Path::new("/Applications/RetroGit/retrogit")).is_none());
    }
}
