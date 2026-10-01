//! Syntax highlighting of file contents shown in diffs (syntect, pure-Rust regex engine).
//!
//! Highlighting can be slow on long lines (fancy regexes), so it runs on its own thread
//! (`Service`): the UI draws plain text until the colors arrive, and a newer request
//! cancels the one in progress.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, OnceLock};

use egui::Color32;
use gitcore::{FileDiff, LineKind};
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme;
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// No highlighting above this many lines.
pub const MAX_LINES: usize = 5_000;
/// Lines longer than this (minified code, long literals) make their hunk plain text.
pub const MAX_LINE_LEN: usize = 500;
/// No highlighting above this many bytes of diff text.
pub const MAX_BYTES: usize = 256 * 1024;
/// A diff that takes longer than this to highlight stays plain text (pathological regexes).
pub const TIME_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);

/// A run of text drawn in one color.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub color: Color32,
}

/// Per hunk: colored spans of each line, or `None` when that hunk stays plain text.
pub type DiffColors = Vec<Option<Vec<Vec<Span>>>>;

/// Colors of the displayed diff, as seen by the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Colors {
    /// Not asked yet (the UI requests them on the next frame).
    #[default]
    NotRequested,
    /// Being computed in the background: plain text meanwhile.
    Pending,
    /// Unknown language, too large, or cancelled.
    Plain,
    Ready(DiffColors),
}

impl Colors {
    /// Spans of one line, if ready.
    pub fn line(&self, hunk: usize, line: usize) -> Option<&[Span]> {
        match self {
            Colors::Ready(c) => c.get(hunk)?.as_ref()?.get(line).map(Vec::as_slice),
            _ => None,
        }
    }
}

/// Which diff view a highlighting result is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Changes,
    History,
    Pull,
    /// The three panes of the conflict editor.
    ConflictMine,
    ConflictTheirs,
    ConflictResult,
}

/// A finished highlighting job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlighted {
    pub target: Target,
    /// The diff the colors belong to (the UI ignores results for a diff it no longer shows).
    pub diff: FileDiff,
    pub colors: Option<DiffColors>,
}

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

/// Highlight consecutive lines with one syntax. `None` if a line is too long or
/// `cancelled()` becomes true.
fn highlight_lines(
    syntax: &SyntaxReference,
    lines: &[&str],
    cancelled: &dyn Fn() -> bool,
) -> Option<Vec<Vec<Span>>> {
    if lines.iter().any(|l| l.len() > MAX_LINE_LEN) {
        return None;
    }
    let mut h = HighlightLines::new(syntax, theme());
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        if cancelled() {
            return None;
        }
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

/// Highlight consecutive lines of one file (each line may end with its newline).
/// `None` when the language is unknown or the input is too large.
pub fn highlight(path: &str, lines: &[&str]) -> Option<Vec<Vec<Span>>> {
    if lines.len() > MAX_LINES {
        return None;
    }
    let syntax = syntax_for(path, lines.first().copied().unwrap_or(""))?;
    highlight_lines(syntax, lines, &|| false)
}

/// Highlight a diff. The language is chosen once (path, or first line of a hunk that starts
/// the file). Each hunk is parsed twice — old side (context + removed) and new side
/// (context + added) — so an unclosed comment on one side cannot color the other.
pub fn highlight_diff(diff: &FileDiff, cancelled: &dyn Fn() -> bool) -> Option<DiffColors> {
    let bytes: usize = diff
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .map(|l| l.text.len())
        .sum();
    if diff.binary || diff.line_count() > MAX_LINES || bytes > MAX_BYTES {
        return None;
    }
    let first_line = diff
        .hunks
        .iter()
        .find(|h| h.new_start <= 1 || h.old_start <= 1)
        .and_then(|h| h.lines.first())
        .map(|l| l.text.as_str())
        .unwrap_or("");
    let syntax = syntax_for(&diff.path, first_line)?;
    let mut out = Vec::with_capacity(diff.hunks.len());
    for hunk in &diff.hunks {
        let side = |skip: LineKind| -> (Vec<usize>, Vec<&str>) {
            hunk.lines
                .iter()
                .enumerate()
                .filter(|(_, l)| l.kind != skip)
                .map(|(i, l)| (i, l.text.as_str()))
                .unzip()
        };
        let (old_idx, old_lines) = side(LineKind::Added);
        let (new_idx, new_lines) = side(LineKind::Removed);
        let old = highlight_lines(syntax, &old_lines, cancelled);
        let new = highlight_lines(syntax, &new_lines, cancelled);
        if cancelled() {
            return None;
        }
        let colored = match (old, new) {
            (Some(old), Some(new)) => {
                let mut lines = vec![Vec::new(); hunk.lines.len()];
                // Context lines take their colors from the new side.
                for (i, spans) in old_idx.into_iter().zip(old) {
                    lines[i] = spans;
                }
                for (i, spans) in new_idx.into_iter().zip(new) {
                    lines[i] = spans;
                }
                Some(lines)
            }
            _ => None,
        };
        out.push(colored);
    }
    Some(out)
}

/// Background highlighting thread. Only the latest request of each target matters: an
/// older one for the same target still running is cancelled as soon as a newer one
/// arrives; requests for other targets (the panes of the conflict editor) are all served.
pub struct Service {
    tx: Sender<(u64, Target, FileDiff)>,
    latest: Arc<std::sync::Mutex<std::collections::HashMap<Target, u64>>>,
    next: Arc<AtomicU64>,
}

impl Service {
    /// `deliver` is called on the highlighting thread for each finished request.
    pub fn start(deliver: impl Fn(Highlighted) + Send + 'static) -> Service {
        let (tx, rx) = channel::<(u64, Target, FileDiff)>();
        let latest: Arc<std::sync::Mutex<std::collections::HashMap<Target, u64>>> = Arc::default();
        let current = latest.clone();
        let spawned = std::thread::Builder::new()
            .name("retrogit-highlight".into())
            .spawn(move || {
                let is_latest = |target: Target, id: u64| {
                    current
                        .lock()
                        .map(|m| m.get(&target) == Some(&id))
                        .unwrap_or(false)
                };
                while let Ok(first) = rx.recv() {
                    // Everything queued, keeping only the newest request of each target.
                    let mut jobs = vec![first];
                    while let Ok(more) = rx.try_recv() {
                        jobs.push(more);
                    }
                    jobs.retain(|(id, target, _)| is_latest(*target, *id));
                    for (id, target, diff) in jobs {
                        let started = std::time::Instant::now();
                        let superseded = || !is_latest(target, id);
                        let cancelled = || superseded() || started.elapsed() > TIME_BUDGET;
                        let colors = highlight_diff(&diff, &cancelled);
                        if !superseded() {
                            deliver(Highlighted {
                                target,
                                diff,
                                colors,
                            });
                        }
                    }
                }
            });
        if let Err(e) = spawned {
            log::warn!("syntax highlighting disabled: {e}");
        }
        Service {
            tx,
            latest,
            next: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn request(&self, target: Target, diff: FileDiff) {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut m) = self.latest.lock() {
            m.insert(target, id);
        }
        let _ = self.tx.send((id, target, diff));
    }
}

/// Replace tabs by spaces up to the next multiple of 4 (egui draws a tab as a single
/// small gap, which breaks indentation in a fixed-width view).
pub fn expand_tabs(text: &str) -> String {
    expand_tabs_from(text, 0).0
}

/// Like `expand_tabs`, starting at column `col`; returns the text and the column after it.
/// Columns count `char`s: wide characters (CJK, emoji) are not measured.
fn expand_tabs_from(text: &str, mut col: usize) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\t' {
            let n = 4 - col % 4;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(c);
            col += 1;
        }
    }
    (out, col)
}

/// One full-width diff row: `background` over the whole row, the text left-aligned,
/// unwrapped and selectable (it can be copied).
pub fn diff_row(
    ui: &mut egui::Ui,
    job: egui::text::LayoutJob,
    height: f32,
    background: Color32,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(0.0, height), egui::Sense::hover());
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, egui::vec2(width, height)),
        0.0,
        background,
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_size(
                rect.min,
                egui::vec2(f32::INFINITY, height),
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let resp = child.add(egui::Label::new(job).extend().selectable(true));
    ui.allocate_exact_size(
        egui::vec2(resp.rect.width().max(width) - 0.0, 0.0),
        egui::Sense::hover(),
    );
    resp
}

/// A monospace label: `prefix` in black, then the line either in its syntax colors or in
/// black (`spans` = `None`), and an optional gray suffix.
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
            // Tab stops are counted from the start of the code, after the prefix.
            let mut col = 0;
            for span in spans {
                let (t, next) = expand_tabs_from(&span.text, col);
                col = next;
                job.append(&t, 0.0, fmt(span.color));
            }
        }
        None => job.append(&expand_tabs(text), 0.0, fmt(win95::theme::BLACK)),
    }
    if !suffix.is_empty() {
        job.append(suffix, 0.0, fmt(win95::theme::GRAY));
    }
    job
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
        assert_eq!(Colors::Plain.line(0, 0), None);
        assert_eq!(Colors::Ready(vec![None]).line(0, 0), None);
    }

    #[test]
    fn tabs_become_spaces_to_the_next_stop() {
        assert_eq!(expand_tabs("\tlet x;"), "    let x;");
        assert_eq!(expand_tabs("ab\tc"), "ab  c");
        assert_eq!(expand_tabs("    four"), "    four");
        let spans = [
            Span {
                text: "\tfn".into(),
                color: Color32::RED,
            },
            Span {
                text: "\tx".into(),
                color: Color32::BLUE,
            },
        ];
        let job = colored_line(
            "",
            "",
            Some(&spans),
            "",
            egui::FontId::monospace(13.0),
            Color32::WHITE,
        );
        assert_eq!(job.text, "    fn  x", "tab stops continue across spans");
    }

    #[test]
    fn diff_rows_are_left_aligned_and_keep_indentation() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let font = egui::FontId::monospace(13.0);
            for text in ["x", "        indented"] {
                let job = colored_line("", text, None, "", font.clone(), Color32::WHITE);
                diff_row(ui, job, 17.0, Color32::WHITE);
            }
        });
        out.textures_delta.clear();
        // Where the text was actually drawn.
        let xs: Vec<f32> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.pos.x),
                _ => None,
            })
            .collect();
        assert_eq!(xs.len(), 2, "{xs:?}");
        assert!(
            xs[0] < 20.0,
            "rows must start at the left edge, text drawn at x={}",
            xs[0]
        );
        assert_eq!(
            xs[0], xs[1],
            "every row starts at the same x; spaces carry the indentation"
        );
    }

    use gitcore::{DiffLine, FileDiff, Hunk, LineKind, Side};

    fn dl(kind: LineKind, t: &str) -> DiffLine {
        DiffLine {
            kind,
            old_no: None,
            new_no: None,
            text: t.into(),
            raw: t.as_bytes().to_vec(),
            no_newline_at_eof: false,
        }
    }

    fn hunk(new_start: u32, lines: Vec<DiffLine>) -> Hunk {
        Hunk {
            header: "@@".into(),
            old_start: new_start,
            old_lines: 0,
            new_start,
            new_lines: 0,
            lines,
        }
    }

    fn file(path: &str, hunks: Vec<Hunk>) -> FileDiff {
        FileDiff {
            path: path.into(),
            side: Side::Unstaged,
            binary: false,
            hunks,
        }
    }

    fn never() -> bool {
        false
    }

    #[test]
    fn a_whole_diff_is_highlighted_hunk_by_hunk() {
        let d = file(
            "lib.rs",
            vec![
                hunk(
                    1,
                    vec![
                        dl(LineKind::Added, "fn a() {}\n"),
                        dl(LineKind::Added, "// x\n"),
                    ],
                ),
                hunk(9, vec![dl(LineKind::Added, "struct S;\n")]),
            ],
        );
        let colors = highlight_diff(&d, &never).unwrap();
        assert_eq!(colors.len(), 2);
        assert_eq!(colors[0].as_ref().unwrap().len(), 2);
        assert_eq!(colors[1].as_ref().unwrap().len(), 1);
    }

    #[test]
    fn a_shebang_script_is_colored_in_every_hunk() {
        let d = file(
            "tools/run",
            vec![
                hunk(
                    1,
                    vec![
                        dl(LineKind::Context, "#!/usr/bin/env python3\n"),
                        dl(LineKind::Added, "import os\n"),
                    ],
                ),
                hunk(
                    40,
                    vec![
                        dl(LineKind::Added, "def f(x):\n"),
                        dl(LineKind::Added, "    return x\n"),
                    ],
                ),
            ],
        );
        let colors = highlight_diff(&d, &never).unwrap();
        assert!(colors.iter().all(Option::is_some), "{colors:?}");
    }

    #[test]
    fn one_unhighlightable_hunk_does_not_uncolor_the_others() {
        let long = format!("let s = \"{}\";\n", "x".repeat(MAX_LINE_LEN));
        let d = file(
            "a.rs",
            vec![
                hunk(1, vec![dl(LineKind::Added, "fn a() {}\n")]),
                hunk(50, vec![dl(LineKind::Added, &long)]),
            ],
        );
        let colors = highlight_diff(&d, &never).unwrap();
        assert!(colors[0].is_some());
        assert!(colors[1].is_none());
    }

    #[test]
    fn removed_and_added_lines_are_parsed_on_their_own_side() {
        // The removed line opens a block comment that the new code does not have.
        let d = file(
            "a.rs",
            vec![hunk(
                1,
                vec![
                    dl(LineKind::Context, "fn a() {}\n"),
                    dl(LineKind::Removed, "/* old\n"),
                    dl(LineKind::Added, "let x = 1;\n"),
                    dl(LineKind::Context, "let y = 2;\n"),
                ],
            )],
        );
        let colors = highlight_diff(&d, &never).unwrap();
        let new_side = highlight_lines(
            syntax_for("a.rs", "").unwrap(),
            &["fn a() {}\n", "let x = 1;\n", "let y = 2;\n"],
            &never,
        )
        .unwrap();
        assert_eq!(
            colors[0].as_ref().unwrap()[3],
            new_side[2],
            "context after the change must not look like a comment"
        );
    }

    #[test]
    fn huge_diffs_and_cancelled_work_give_no_colors() {
        let big: Vec<DiffLine> = (0..3000)
            .map(|_| {
                dl(
                    LineKind::Added,
                    &format!("let a = \"{}\";\n", "y".repeat(100)),
                )
            })
            .collect();
        assert!(
            highlight_diff(&file("a.rs", vec![hunk(1, big)]), &never).is_none(),
            "total size cap"
        );
        let d = file(
            "a.rs",
            vec![hunk(1, vec![dl(LineKind::Added, "fn a() {}\n")])],
        );
        assert!(highlight_diff(&d, &|| true).is_none(), "cancelled");
    }

    #[test]
    fn requests_for_different_panes_are_all_delivered() {
        let (tx, rx) = std::sync::mpsc::channel();
        let service = Service::start(move |r| {
            let _ = tx.send(r);
        });
        let d = |t: &str| file("a.rs", vec![hunk(1, vec![dl(LineKind::Added, t)])]);
        // The three panes of the conflict editor ask in the same frame.
        service.request(Target::ConflictMine, d("fn mine() {}\n"));
        service.request(Target::ConflictTheirs, d("fn theirs() {}\n"));
        service.request(Target::ConflictResult, d("fn result() {}\n"));
        let mut got = Vec::new();
        while got.len() < 3 {
            let r = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            got.push(r.target);
        }
        assert!(got.contains(&Target::ConflictMine) && got.contains(&Target::ConflictTheirs));
    }

    #[test]
    fn the_background_service_delivers_the_latest_request() {
        let (tx, rx) = std::sync::mpsc::channel();
        let service = Service::start(move |r| {
            let _ = tx.send(r);
        });
        let old = file(
            "a.rs",
            vec![hunk(1, vec![dl(LineKind::Added, "fn old() {}\n")])],
        );
        let new = file(
            "a.rs",
            vec![hunk(1, vec![dl(LineKind::Added, "fn new() {}\n")])],
        );
        service.request(Target::Changes, old);
        service.request(Target::Changes, new.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut last = None;
        while std::time::Instant::now() < deadline {
            if let Ok(r) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
                let done = r.diff == new;
                last = Some(r);
                if done {
                    break;
                }
            }
        }
        let last = last.unwrap();
        assert_eq!(last.target, Target::Changes);
        assert_eq!(last.diff, new);
        assert!(last.colors.is_some());
    }

    #[test]
    fn a_pathologically_slow_diff_gives_up_within_the_time_budget() {
        let (tx, rx) = std::sync::mpsc::channel();
        let service = Service::start(move |r| {
            let _ = tx.send(r);
        });
        let line = format!("const s = \"{}\";\n", "a".repeat(480));
        let lines = (0..400).map(|_| dl(LineKind::Added, &line)).collect();
        let started = std::time::Instant::now();
        service.request(Target::Changes, file("x.js", vec![hunk(1, lines)]));
        let r = rx
            .recv_timeout(TIME_BUDGET + std::time::Duration::from_secs(5))
            .unwrap();
        assert!(r.colors.is_none(), "gives up and stays plain");
        assert!(
            started.elapsed() < TIME_BUDGET + std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn diff_rows_are_selectable_text() {
        let ctx = egui::Context::default();
        let mut sense = None;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let job = colored_line(
                "",
                "copy me",
                None,
                "",
                egui::FontId::monospace(13.0),
                Color32::TRANSPARENT,
            );
            sense = Some(diff_row(ui, job, 17.0, Color32::WHITE).sense);
        });
        out.textures_delta.clear();
        assert!(
            sense.unwrap().senses_drag(),
            "text selection needs a drag-sensing label"
        );
    }
}
