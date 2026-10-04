#![allow(clippy::unwrap_used)]
//! Windows notifications: toast XML and the per-user registration (pure parts everywhere,
//! the registry itself on Windows only).

use std::path::Path;

use retrogit::notify::winreg::{AUMID, RegEntry, reg_add_args, registry_entries, toast_xml};

#[test]
fn the_toast_opens_its_link_and_escapes_text() {
    let xml = toast_xml(
        "o/r #7 <bug> & \"fix\"",
        "it's done",
        Some("retrogit://pull?repo=o%2Fr&number=7"),
    );
    assert!(
        xml.starts_with(
            "<toast activationType=\"protocol\" launch=\"retrogit://pull?repo=o%2Fr&amp;number=7\">"
        ),
        "{xml}"
    );
    assert!(
        xml.contains("<text>o/r #7 &lt;bug&gt; &amp; &quot;fix&quot;</text>"),
        "{xml}"
    );
    assert!(xml.contains("<text>it&apos;s done</text>"), "{xml}");
    let plain = toast_xml("t", "b", None);
    assert!(plain.starts_with("<toast>"), "{plain}");
}

#[test]
fn the_app_is_registered_for_notifications_and_links() {
    let exe = Path::new(r"C:\Users\Ada Love\AppData\Local\Programs\RetroGit\retrogit.exe");
    let icon = Path::new(r"C:\Users\Ada Love\AppData\Local\RetroGit\icon.png");
    let e = registry_entries(r"HKCU\Software\Classes", exe, icon);
    let find = |key: &str, name: Option<&str>| {
        e.iter()
            .find(|x| x.key == key && x.name.as_deref() == name)
            .map(|x| x.value.clone())
    };
    let aumid = format!(r"HKCU\Software\Classes\AppUserModelId\{AUMID}");
    assert_eq!(
        find(&aumid, Some("DisplayName")).as_deref(),
        Some("RetroGit")
    );
    assert_eq!(
        find(&aumid, Some("IconUri")).as_deref(),
        Some(icon.to_str().unwrap())
    );
    assert_eq!(
        find(r"HKCU\Software\Classes\retrogit", Some("URL Protocol")).as_deref(),
        Some("")
    );
    assert_eq!(
        find(r"HKCU\Software\Classes\retrogit\shell\open\command", None).as_deref(),
        Some(r#""C:\Users\Ada Love\AppData\Local\Programs\RetroGit\retrogit.exe" "%1""#)
    );
    let args = reg_add_args(&RegEntry {
        key: "HKCU\\K".into(),
        name: None,
        value: "v".into(),
    });
    assert_eq!(
        args,
        ["add", "HKCU\\K", "/ve", "/t", "REG_SZ", "/d", "v", "/f"]
    );
    let named = reg_add_args(&RegEntry {
        key: "HKCU\\K".into(),
        name: Some("N".into()),
        value: String::new(),
    });
    assert_eq!(
        named,
        ["add", "HKCU\\K", "/v", "N", "/t", "REG_SZ", "/d", "", "/f"]
    );
}

#[cfg(windows)]
#[test]
fn registration_writes_the_keys() {
    let root = format!(r"HKCU\Software\RetroGitTest{}\Classes", std::process::id());
    let exe = std::env::current_exe().unwrap();
    let icon = std::env::temp_dir().join("icon.png");
    retrogit::notify::winreg::register_under(&root, &exe, &icon).unwrap();
    let out = std::process::Command::new("reg")
        .args([
            "query",
            &format!(r"{root}\retrogit\shell\open\command"),
            "/ve",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let top = root.trim_end_matches(r"\Classes");
    let _ = std::process::Command::new("reg")
        .args(["delete", top, "/f"])
        .output();
    assert!(text.contains(&exe.display().to_string()), "{text}");
}
