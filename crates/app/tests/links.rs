//! Only web and mail addresses leave the app, whoever asked to open them.

use egui::{OpenUrl, OutputCommand};
use retrogit::app::retain_safe_urls;

#[test]
fn unsafe_open_url_commands_are_dropped() {
    let mut cmds = vec![
        OutputCommand::OpenUrl(OpenUrl::new_tab("https://github.com/o/r")),
        OutputCommand::OpenUrl(OpenUrl::new_tab("file:///etc/passwd")),
        OutputCommand::CopyText("file:///kept".into()),
        OutputCommand::OpenUrl(OpenUrl::same_tab("mailto:a@b.c")),
        OutputCommand::OpenUrl(OpenUrl::new_tab("retrogit://pull/o/r/1")),
        OutputCommand::OpenUrl(OpenUrl::new_tab("javascript:alert(1)")),
        OutputCommand::OpenUrl(OpenUrl::new_tab("HTTP://example.com")),
    ];
    retain_safe_urls(&mut cmds);
    let kept: Vec<String> = cmds
        .iter()
        .map(|c| match c {
            OutputCommand::OpenUrl(o) => o.url.clone(),
            OutputCommand::CopyText(t) => format!("copy {t}"),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        kept,
        [
            "https://github.com/o/r",
            "copy file:///kept",
            "mailto:a@b.c",
            "HTTP://example.com"
        ]
    );
}
