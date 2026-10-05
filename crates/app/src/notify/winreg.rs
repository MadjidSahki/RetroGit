//! Windows: what makes notifications carry RetroGit's name and icon and open a link when
//! clicked, for an app that is not packaged: an AppUserModelID declared under
//! `HKCU\Software\Classes\AppUserModelId`, and the `retrogit://` link handler. Per user, no
//! administrator rights. The pure parts build on every platform (tests).

use std::path::Path;

/// AppUserModelID of RetroGit's notifications (also set by the installer).
pub const AUMID: &str = "RetroGit";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegEntry {
    pub key: String,
    /// `None`: the key's default value.
    pub name: Option<String>,
    pub value: String,
}

fn entry(key: String, name: Option<&str>, value: &str) -> RegEntry {
    RegEntry {
        key,
        name: name.map(String::from),
        value: value.to_string(),
    }
}

/// Values to write under `classes` (`HKCU\Software\Classes` for real).
pub fn registry_entries(classes: &str, exe: &Path, icon: &Path) -> Vec<RegEntry> {
    let aumid = format!(r"{classes}\AppUserModelId\{AUMID}");
    let scheme = format!(r"{classes}\retrogit");
    vec![
        entry(aumid.clone(), Some("DisplayName"), "RetroGit"),
        entry(aumid, Some("IconUri"), &icon.display().to_string()),
        entry(scheme.clone(), None, "URL:RetroGit"),
        entry(scheme.clone(), Some("URL Protocol"), ""),
        entry(
            format!(r"{scheme}\shell\open\command"),
            None,
            &format!("\"{}\" \"%1\"", exe.display()),
        ),
    ]
}

/// Whether `exe` may register itself: not when it runs from inside `temp` (a downloaded
/// update, the uninstaller's copy), or the link handler would point at a file about to go.
/// Case-insensitive, `/` and `\` alike (Windows paths).
pub fn should_register(exe: &Path, temp: &Path) -> bool {
    let norm = |p: &Path| {
        let mut s = p.to_string_lossy().replace('\\', "/").to_lowercase();
        while s.ends_with('/') {
            s.pop();
        }
        s
    };
    let (exe, temp) = (norm(exe), norm(temp));
    if temp.is_empty() {
        return true;
    }
    !exe.strip_prefix(&temp)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// Arguments of `reg.exe` writing `e`.
pub fn reg_add_args(e: &RegEntry) -> Vec<String> {
    let mut args = vec!["add".to_string(), e.key.clone()];
    match &e.name {
        Some(n) => args.extend(["/v".to_string(), n.clone()]),
        None => args.push("/ve".to_string()),
    }
    args.extend([
        "/t".to_string(),
        "REG_SZ".to_string(),
        "/d".to_string(),
        e.value.clone(),
        "/f".to_string(),
    ]);
    args
}

/// Escapes markup; characters XML 1.0 forbids (and tabs, line breaks) become a space, or
/// Windows would refuse the whole toast.
fn xml_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{0}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}' => ' ',
            c => c,
        })
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The toast: title and body; a click opens `link` (protocol activation, works even when
/// RetroGit is closed).
pub fn toast_xml(title: &str, body: &str, link: Option<&str>) -> String {
    let open = match link {
        Some(l) => format!(
            "<toast activationType=\"protocol\" launch=\"{}\">",
            xml_escape(l)
        ),
        None => "<toast>".to_string(),
    };
    format!(
        "{open}<visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        xml_escape(title),
        xml_escape(body)
    )
}

/// Write the entries under `classes` with `reg.exe`.
#[cfg(windows)]
pub fn register_under(classes: &str, exe: &Path, icon: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    for e in registry_entries(classes, exe, icon) {
        let out = std::process::Command::new("reg")
            .args(reg_add_args(&e))
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
    }
    Ok(())
}

/// At each start (in the background): the icon file and the registry entries, for this
/// exe's current location (the portable zip may be moved).
#[cfg(windows)]
pub fn register() {
    static ICON: &[u8] = include_bytes!("../../assets/icon-256.png");
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    // Compared as given and canonical too (the temporary folder may come in 8.3 form).
    let temp = std::env::temp_dir();
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if !should_register(&exe, &temp) || !should_register(&canon(&exe), &canon(&temp)) {
        log::info!("run from the temporary folder: notifications not registered");
        return;
    }
    let Some(dir) = dirs::data_local_dir().map(|d| d.join("RetroGit")) else {
        return;
    };
    let icon = dir.join("icon.png");
    let written = std::fs::create_dir_all(&dir).and_then(|()| {
        if std::fs::read(&icon).ok().as_deref() != Some(ICON) {
            std::fs::write(&icon, ICON)?;
        }
        Ok(())
    });
    if let Err(e) = written {
        log::info!("notification icon not written: {e}");
    }
    if let Err(e) = register_under(r"HKCU\Software\Classes", &exe, &icon) {
        log::info!("notifications not registered: {e}");
    }
}
