//! Markdown links only open web and mail addresses; parsing is linear and cached.

use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use win95::markdown::{self, inlines, safe_link};

#[test]
fn only_web_and_mail_links_are_safe() {
    for (url, safe) in [
        ("https://github.com/o/r", true),
        ("http://example.com", true),
        ("HTTPS://EXAMPLE.COM", true),
        ("Http://x", true),
        ("mailto:a@b.c", true),
        ("MAILTO:a@b.c", true),
        ("mailto:", true),
        ("file:///etc/passwd", false),
        ("javascript:alert(1)", false),
        ("retrogit://pull/o/r/1", false),
        ("/o/r/pull/1", false),
        ("https:/x", false),
        ("httpsx://x", false),
        (" https://x", false),
        ("", false),
        ("ftp://x", false),
    ] {
        assert_eq!(safe_link(url), safe, "{url}");
    }
}

/// Click the link labelled "x" in `md` and return the URLs the frames asked to open.
fn opened_after_click(md: &'static str) -> Vec<String> {
    let mut h = Harness::new_ui(move |ui| win95::markdown_view(ui, md));
    h.run();
    h.get_by_label("x").click();
    let mut urls = Vec::new();
    for _ in 0..4 {
        h.step();
        for c in &h.output().platform_output.commands {
            if let egui::OutputCommand::OpenUrl(o) = c {
                urls.push(o.url.clone());
            }
        }
    }
    urls
}

#[test]
fn clicking_a_web_link_opens_it() {
    assert_eq!(
        opened_after_click("see [x](https://a.b/c)"),
        vec!["https://a.b/c".to_string()]
    );
}

#[test]
fn clicking_a_file_link_opens_nothing() {
    assert_eq!(
        opened_after_click("see [x](file:///etc/passwd)"),
        Vec::<String>::new()
    );
    assert_eq!(
        opened_after_click("see [x](retrogit://pull/o/r/1)"),
        Vec::<String>::new()
    );
}

#[test]
fn an_unsafe_link_keeps_its_text() {
    assert_eq!(
        inlines("[x](file:///etc/passwd)"),
        vec![markdown::Inline::Link {
            text: "x".into(),
            url: "file:///etc/passwd".into()
        }]
    );
}

const N: usize = 100_000;
const BUDGET: Duration = Duration::from_millis(500);

fn parses_quickly(text: &str) {
    let start = Instant::now();
    let out = inlines(text);
    let took = start.elapsed();
    assert!(!out.is_empty());
    assert!(
        took < BUDGET,
        "{:?}... took {took:?}",
        &text[..8.min(text.len())]
    );
}

#[test]
fn many_unmatched_brackets_parse_in_linear_time() {
    parses_quickly(&"[".repeat(N));
    parses_quickly(&"[](".repeat(N / 3));
    parses_quickly(&"![".repeat(N / 2));
}

#[test]
fn many_words_starting_with_h_parse_in_linear_time() {
    parses_quickly(&"h ".repeat(N));
}

#[test]
fn many_stars_and_underscores_parse_in_linear_time() {
    parses_quickly(&"* ".repeat(N));
    parses_quickly(&"** ".repeat(N / 3));
    parses_quickly(&" _ ".repeat(N / 3));
    parses_quickly(&"`".repeat(N));
}

#[test]
fn a_second_frame_does_not_parse_again() {
    let ctx = egui::Context::default();
    let text = "# Cached\n\nOnly **once**.";
    let before = markdown::PARSES.with(std::cell::Cell::get);
    for _ in 0..3 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            win95::markdown_view(ui, text);
        });
        out.textures_delta.clear();
    }
    assert_eq!(markdown::PARSES.with(std::cell::Cell::get) - before, 1);
}
