//! The `retrogit` command line: arguments, and installing the command in the PATH.

use std::path::{Path, PathBuf};

/// How the binary was started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// Normal window, optionally opening a folder at start (`--open <dir>`).
    Gui { open: Option<PathBuf> },
    /// From the installed `retrogit` command: `--cli <cwd> [path]`.
    Cli { target: PathBuf },
}

/// Parse `std::env::args()` (unknown arguments, such as macOS `-psn_...`, are ignored).
pub fn parse(args: &[String]) -> Launch {
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

/// `retrogit.cmd` placed in a folder on the user's PATH (Windows).
pub fn windows_shim(exe: &Path) -> String {
    format!("@\"{}\" --cli \"%CD%\" %*\r\n", exe.display())
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

/// Install the `retrogit` command for the current user. Returns a message to show.
pub fn install_command_line_tool() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    {
        let tmp = std::env::temp_dir().join("retrogit-cli");
        std::fs::write(&tmp, mac_install_script(&exe)).map_err(|e| e.to_string())?;
        let out = std::process::Command::new("osascript")
            .args(["-e", &mac_install_applescript(&tmp)])
            .output()
            .map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&tmp);
        if out.status.success() {
            Ok("Installed /usr/local/bin/retrogit. Open a new terminal and type: retrogit".into())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or("LOCALAPPDATA is not set")?;
        let bin = local.join("RetroGit").join("bin");
        std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
        std::fs::write(bin.join("retrogit.cmd"), windows_shim(&exe)).map_err(|e| e.to_string())?;
        // Read and write the *user* PATH (not the merged process PATH).
        let read = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "[Environment]::GetEnvironmentVariable('Path','User')",
            ])
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| e.to_string())?;
        let current = String::from_utf8_lossy(&read.stdout).trim().to_string();
        if let Some(new_path) = merge_path(&current, &bin.display().to_string()) {
            let script = format!(
                "[Environment]::SetEnvironmentVariable('Path', '{}', 'User')",
                new_path.replace('\'', "''")
            );
            let out = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", &script])
                .creation_flags(0x0800_0000)
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
            }
        }
        Ok("Installed the retrogit command. Open a new terminal and type: retrogit".into())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = exe;
        Err("Not supported on this platform.".into())
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
    fn windows_shim_and_path_merge() {
        assert_eq!(
            windows_shim(Path::new(r"C:\Program Files\RetroGit\retrogit.exe")),
            "@\"C:\\Program Files\\RetroGit\\retrogit.exe\" --cli \"%CD%\" %*\r\n"
        );
        assert_eq!(
            merge_path(r"C:\a;C:\b", r"C:\bin"),
            Some(r"C:\a;C:\b;C:\bin".to_string())
        );
        assert_eq!(
            merge_path(r"C:\a;c:\BIN\;C:\b", r"C:\bin"),
            None,
            "already present (case/trailing slash)"
        );
        assert_eq!(merge_path("", r"C:\bin"), Some(r"C:\bin".to_string()));
        assert_eq!(
            merge_path(r"C:\a;", r"C:\bin"),
            Some(r"C:\a;C:\bin".to_string())
        );
    }
}
