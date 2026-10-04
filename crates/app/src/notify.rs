//! Notifications: the text of each pull request event, and the system notification.

use github::{PrEvent, PrEventKind};

/// The pull request a notification is about: what a click opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullLink {
    /// `owner/repo`.
    pub repo: String,
    pub number: u64,
    /// Account that was notified.
    pub account: String,
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `retrogit://pull?repo=<o%2Fr>&number=<n>&account=<login>`.
pub fn pull_link(l: &PullLink) -> String {
    format!(
        "retrogit://pull?repo={}&number={}&account={}",
        encode(&l.repo),
        l.number,
        encode(&l.account)
    )
}

pub fn parse_pull_link(url: &str) -> Option<PullLink> {
    // Scheme and host in any case; Windows may add a `/` before the query.
    let head = url.get(..15)?;
    if !head.eq_ignore_ascii_case("retrogit://pull") {
        return None;
    }
    let rest = &url[15..];
    let query = rest.strip_prefix("/?").or_else(|| rest.strip_prefix('?'))?;
    let (mut repo, mut number, mut account) = (None, None, String::new());
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        match k {
            "repo" => repo = Some(decode(v)?),
            "number" => number = v.parse().ok(),
            "account" => account = decode(v)?,
            _ => {}
        }
    }
    let repo = repo.filter(|r| valid_repo(r))?;
    Some(PullLink {
        repo,
        number: number.filter(|n| *n >= 1)?,
        account,
    })
}

/// `owner/repo` as GitHub allows them: owner `[A-Za-z0-9-]{1,39}`, repository
/// `[A-Za-z0-9._-]{1,100}` other than `.` and `..`.
fn valid_repo(r: &str) -> bool {
    let Some((owner, name)) = r.split_once('/') else {
        return false;
    };
    let owner_ok = (1..=39).contains(&owner.len())
        && owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    let name_ok = (1..=100).contains(&name.len())
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    owner_ok && name_ok
}

/// The link of a notification for `e`.
pub fn event_link(e: &PrEvent) -> String {
    pull_link(&PullLink {
        repo: e.repo.clone(),
        number: e.number,
        account: e.account.clone(),
    })
}

use crate::strings as s;

/// Title and body of the notification for `e`; the title names the account when several
/// are signed in.
pub fn notification_text(e: &PrEvent, with_account: bool) -> (String, String) {
    let what = match &e.kind {
        PrEventKind::ChecksPassed => s::NOTIFY_CHECKS_PASSED.to_string(),
        PrEventKind::ChecksFailed { failed } if *failed == 1 => {
            s::NOTIFY_ONE_CHECK_FAILED.to_string()
        }
        PrEventKind::ChecksFailed { failed } if *failed > 1 => {
            s::NOTIFY_CHECKS_FAILED.replace("{n}", &failed.to_string())
        }
        PrEventKind::ChecksFailed { .. } => s::NOTIFY_CHECKS_FAILED_PLAIN.to_string(),
        PrEventKind::Approved { by } => s::NOTIFY_APPROVED.replace("{who}", by),
        PrEventKind::ChangesRequested { by } => s::NOTIFY_CHANGES.replace("{who}", by),
        PrEventKind::Commented { by } => s::NOTIFY_COMMENT.replace("{who}", by),
        PrEventKind::Merged => s::NOTIFY_MERGED.to_string(),
        PrEventKind::Closed => s::NOTIFY_CLOSED.to_string(),
    };
    let mut title = format!("{} #{}", e.repo, e.number);
    if with_account {
        title.push_str(&format!(" (@{})", e.account));
    }
    (title, format!("{}: {what}", e.title))
}

/// `text` as an AppleScript string literal.
pub fn applescript_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Show a system notification without blocking the caller; a click opens `link` where the
/// system supports it. Failures are only logged (notifications disabled, no service).
pub fn show(title: &str, body: &str, link: Option<String>) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        if let Err(e) = show_now(&title, &body, link.as_deref()) {
            log::info!("system notification not shown: {e}");
        }
    });
}

pub mod winreg;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
fn show_now(title: &str, body: &str, link: Option<&str>) -> Result<(), String> {
    // Inside RetroGit.app: native, with the app's name and a click back to it.
    if macos::native() {
        return macos::send(title, body, link);
    }
    // `display notification` works from a bare executable (no app bundle needed).
    let script = format!(
        "display notification {} with title {}",
        applescript_string(body),
        applescript_string(&format!("RetroGit - {title}"))
    );
    let status = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("osascript exited with {status}"))
    }
}

#[cfg(windows)]
fn show_now(title: &str, body: &str, link: Option<&str>) -> Result<(), String> {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::core::HSTRING;
    let show = || -> windows::core::Result<()> {
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(winreg::toast_xml(title, body, link)))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(winreg::AUMID))?
            .Show(&toast)
    };
    show().map_err(|e| e.to_string())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn show_now(_title: &str, _body: &str, _link: Option<&str>) -> Result<(), String> {
    Err("not supported on this platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: PrEventKind) -> PrEvent {
        PrEvent {
            account: "me".into(),
            key: "o/r#7".into(),
            repo: "o/r".into(),
            number: 7,
            title: "Fix login".into(),
            url: "https://github.com/o/r/pull/7".into(),
            kind,
        }
    }

    #[test]
    fn notification_text_for_each_event() {
        let body = |k| notification_text(&event(k), false).1;
        assert_eq!(
            notification_text(&event(PrEventKind::Merged), false).0,
            "o/r #7"
        );
        assert_eq!(
            notification_text(&event(PrEventKind::Merged), true).0,
            "o/r #7 (@me)",
            "several accounts"
        );
        assert_eq!(
            body(PrEventKind::ChecksPassed),
            "Fix login: all checks passed"
        );
        assert_eq!(
            body(PrEventKind::ChecksFailed { failed: 1 }),
            "Fix login: 1 check failed"
        );
        assert_eq!(
            body(PrEventKind::ChecksFailed { failed: 3 }),
            "Fix login: 3 checks failed"
        );
        assert_eq!(
            body(PrEventKind::ChecksFailed { failed: 0 }),
            "Fix login: checks failed"
        );
        assert_eq!(
            body(PrEventKind::Approved { by: "ada".into() }),
            "Fix login: approved by ada"
        );
        assert_eq!(
            body(PrEventKind::ChangesRequested { by: "bob".into() }),
            "Fix login: changes requested by bob"
        );
        assert_eq!(
            body(PrEventKind::Commented { by: "eve".into() }),
            "Fix login: new comment from eve"
        );
        assert_eq!(body(PrEventKind::Merged), "Fix login: merged");
        assert_eq!(body(PrEventKind::Closed), "Fix login: closed");
    }

    #[test]
    fn applescript_strings_are_escaped() {
        assert_eq!(
            applescript_string(r#"say "hi" \o/"#),
            r#""say \"hi\" \\o/""#
        );
        assert_eq!(applescript_string("a\nb"), "\"a b\"");
    }
}
