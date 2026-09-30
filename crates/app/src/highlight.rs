//! Syntax highlighting of file contents shown in diffs (syntect, pure-Rust regex engine).

use std::path::Path;
use std::sync::OnceLock;

use egui::Color32;
use gitcore::FileDiff;
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme;
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// No highlighting above this many lines (keeps the UI responsive).
pub const MAX_LINES: usize = 5_000;
/// No highlighting when a line is longer than this (minified files).
pub const MAX_LINE_LEN: usize = 2_000;

/// A run of text drawn in one color.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub color: Color32,
}

/// Colored spans for each line, per hunk: `[hunk][line][span]`.
pub type DiffColors = Vec<Vec<Vec<Span>>>;

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    // bat's extended set: TypeScript, TOML, Terraform/HCL, Dockerfile... (200+ languages).
    SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    // Light theme, readable on the white (and pale green/red) diff background.
    THEME.get_or_init(|| {
        two_face::theme::extra()
            .get(two_face::theme::EmbeddedThemeName::InspiredGithub)
            .clone()
    })
}

fn syntax_for(path: &str, first_line: &str) -> Option<&'static SyntaxReference> {
    let set = syntaxes();
    let p = Path::new(path);
    let by_ext = p
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|e| set.find_syntax_by_extension(e));
    let by_name = || {
        p.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| set.find_syntax_by_extension(n))
    };
    by_ext
        .or_else(by_name)
        .or_else(|| set.find_syntax_by_first_line(first_line))
        .filter(|s| s.name != "Plain Text")
}

/// Name of the language used for `path` (first line helps for shebang scripts).
pub fn language_of(path: &str, first_line: &str) -> Option<String> {
    syntax_for(path, first_line).map(|s| s.name.clone())
}

/// Highlight consecutive lines of one file (each line may end with its newline).
/// `None` when the language is unknown or the input is too large.
pub fn highlight(path: &str, lines: &[&str]) -> Option<Vec<Vec<Span>>> {
    if lines.len() > MAX_LINES || lines.iter().any(|l| l.len() > MAX_LINE_LEN) {
        return None;
    }
    let syntax = syntax_for(path, lines.first().copied().unwrap_or(""))?;
    let mut h = HighlightLines::new(syntax, theme());
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        // The parser expects a trailing newline; it is not part of the displayed text.
        let body = line.trim_end_matches(['\n', '\r']);
        let fed = format!("{body}\n");
        let ranges = h.highlight_line(&fed, syntaxes()).ok()?;
        let spans = ranges
            .into_iter()
            .filter_map(|(style, text)| {
                let text = text.trim_end_matches('\n');
                (!text.is_empty()).then(|| Span {
                    text: text.to_string(),
                    color: Color32::from_rgb(
                        style.foreground.r,
                        style.foreground.g,
                        style.foreground.b,
                    ),
                })
            })
            .collect();
        out.push(spans);
    }
    Some(out)
}

/// A monospace label: `prefix` in black, then the line either in its syntax colors or in
/// black (`spans` = `None`), with an optional background and suffix.
pub fn colored_line(
    prefix: &str,
    text: &str,
    spans: Option<&[Span]>,
    suffix: &str,
    font: egui::FontId,
    background: Color32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let fmt = |color: Color32| egui::TextFormat {
        font_id: font.clone(),
        color,
        background,
        ..Default::default()
    };
    job.append(prefix, 0.0, fmt(win95::theme::BLACK));
    match spans {
        Some(spans) => {
            for span in spans {
                job.append(&span.text, 0.0, fmt(span.color));
            }
        }
        None => job.append(text, 0.0, fmt(win95::theme::BLACK)),
    }
    if !suffix.is_empty() {
        job.append(suffix, 0.0, fmt(win95::theme::GRAY));
    }
    job
}

/// Colors for one line of a diff, if highlighting succeeded for that diff.
pub fn line_spans(
    colors: Option<&Option<DiffColors>>,
    hunk: usize,
    line: usize,
) -> Option<&[Span]> {
    colors?.as_ref()?.get(hunk)?.get(line).map(Vec::as_slice)
}

/// Highlight every hunk of a diff, each from its first line (lines in display order).
pub fn highlight_diff(diff: &FileDiff) -> Option<DiffColors> {
    if diff.binary || diff.line_count() > MAX_LINES {
        return None;
    }
    diff.hunks
        .iter()
        .map(|h| {
            let lines: Vec<&str> = h.lines.iter().map(|l| l.text.as_str()).collect();
            highlight(&diff.path, &lines)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn languages_are_detected_from_the_file_name_or_first_line() {
        assert_eq!(language_of("src/main.rs", "").as_deref(), Some("Rust"));
        assert_eq!(language_of("App/Program.cs", "").as_deref(), Some("C#"));
        assert_eq!(
            language_of("web/app.ts", "")
                .map(|l| l.contains("TypeScript") || l.contains("JavaScript")),
            Some(true)
        );
        assert_eq!(
            language_of("deploy/values.yaml", "").as_deref(),
            Some("YAML")
        );
        assert_eq!(
            language_of("tools/run", "#!/usr/bin/env python3\n").as_deref(),
            Some("Python")
        );
        assert_eq!(language_of("Makefile", "").as_deref(), Some("Makefile"));
        assert_eq!(language_of("notes.unknownext", "just text\n"), None);
    }

    #[test]
    fn spans_rebuild_each_line_exactly() {
        let lines = ["fn main() {\n", "    let x = \"hi\"; // comment\n", "}\n"];
        let out = highlight("main.rs", &lines).unwrap();
        assert_eq!(out.len(), 3);
        for (spans, line) in out.iter().zip(lines) {
            let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
            assert_eq!(joined, line.trim_end_matches(['\n', '\r']));
        }
        // Keywords and strings get different colors.
        let colors: std::collections::HashSet<_> = out[1].iter().map(|s| s.color).collect();
        assert!(colors.len() >= 2, "{:?}", out[1]);
    }

    #[test]
    fn unknown_languages_and_huge_inputs_are_not_highlighted() {
        assert!(highlight("x.unknownext", &["a\n"]).is_none());
        let many = vec!["let a = 1;\n"; MAX_LINES + 1];
        assert!(highlight("a.rs", &many).is_none());
        let long = format!("{}\n", "x".repeat(MAX_LINE_LEN + 1));
        assert!(highlight("a.rs", &[long.as_str()]).is_none());
    }

    #[test]
    fn a_colored_line_keeps_prefix_text_and_suffix() {
        let font = egui::FontId::monospace(13.0);
        let spans = [
            Span {
                text: "let".into(),
                color: Color32::RED,
            },
            Span {
                text: " x".into(),
                color: Color32::BLUE,
            },
        ];
        let job = colored_line(
            "  1 + ",
            "let x",
            Some(&spans),
            " !",
            font.clone(),
            Color32::WHITE,
        );
        assert_eq!(job.text, "  1 + let x !");
        let plain = colored_line("> ", "raw", None, "", font, Color32::WHITE);
        assert_eq!(plain.text, "> raw");
        assert_eq!(line_spans(None, 0, 0), None);
        assert_eq!(line_spans(Some(&None), 0, 0), None);
    }

    #[test]
    fn a_whole_diff_is_highlighted_hunk_by_hunk() {
        use gitcore::{DiffLine, FileDiff, Hunk, LineKind, Side};
        let line = |t: &str| DiffLine {
            kind: LineKind::Added,
            old_no: None,
            new_no: None,
            text: t.into(),
            raw: t.as_bytes().to_vec(),
            no_newline_at_eof: false,
        };
        let hunk = |ls: Vec<DiffLine>| Hunk {
            header: "@@".into(),
            old_start: 1,
            old_lines: 0,
            new_start: 1,
            new_lines: 1,
            lines: ls,
        };
        let diff = FileDiff {
            path: "lib.rs".into(),
            side: Side::Unstaged,
            binary: false,
            hunks: vec![
                hunk(vec![line("fn a() {}\n"), line("// x\n")]),
                hunk(vec![line("struct S;\n")]),
            ],
        };
        let colors = highlight_diff(&diff).unwrap();
        assert_eq!(colors.len(), 2);
        assert_eq!(colors[0].len(), 2);
        assert_eq!(colors[1].len(), 1);
    }
}
