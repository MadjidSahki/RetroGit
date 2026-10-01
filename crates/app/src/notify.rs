//! Notifications: the text of each pull request event, and the system notification.

use github::{PrEvent, PrEventKind};

use crate::strings as s;

/// Title and body of the notification for `e`.
pub fn notification_text(e: &PrEvent) -> (String, String) {
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
    (
        format!("{} #{}", e.repo, e.number),
        format!("{}: {what}", e.title),
    )
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

/// Show a system notification without blocking the caller. Failures are only logged
/// (notifications disabled for the app, no notification service).
pub fn show(title: &str, body: &str) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        if let Err(e) = show_now(&title, &body) {
            log::info!("system notification not shown: {e}");
        }
    });
}

#[cfg(target_os = "macos")]
fn show_now(title: &str, body: &str) -> Result<(), String> {
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
fn show_now(title: &str, body: &str) -> Result<(), String> {
    use tauri_winrt_notification::Toast;
    // Without an installer there is no registered app id: PowerShell's is used.
    Toast::new(Toast::POWERSHELL_APP_ID)
        .title(&format!("RetroGit - {title}"))
        .text1(body)
        .show()
        .map_err(|e| e.to_string())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn show_now(_title: &str, _body: &str) -> Result<(), String> {
    Err("not supported on this platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: PrEventKind) -> PrEvent {
        PrEvent {
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
        let body = |k| notification_text(&event(k)).1;
        assert_eq!(notification_text(&event(PrEventKind::Merged)).0, "o/r #7");
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
