//! Minimal Markdown for pull request descriptions and comments: headings, paragraphs,
//! lists (task boxes too), code blocks, quotes, rules, and inline emphasis, code and links.
//! HTML is not interpreted (comments are dropped, other tags shown as text).

use std::sync::Arc;

use egui::{Color32, RichText, Ui};

use crate::theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Bold(String),
    Italic(String),
    Code(String),
    Link { text: String, url: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(u8, Vec<Inline>),
    Paragraph(Vec<Inline>),
    /// `depth` 0 for top-level items; `marker` is "-", "1." or a task box "[ ]" / "[x]".
    ListItem {
        depth: usize,
        marker: String,
        content: Vec<Inline>,
    },
    Code(String),
    Quote(Vec<Inline>),
    Rule,
}

/// Remove `<!-- ... -->` comments (pull request templates are full of them).
fn strip_html_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// `- item`, `* item`, `+ item`, `1. item` => (marker, content).
fn list_marker(line: &str) -> Option<(String, &str)> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(bullet) {
            for (task, marker) in [("[ ] ", "[ ]"), ("[x] ", "[x]"), ("[X] ", "[x]")] {
                if let Some(r) = rest.strip_prefix(task) {
                    return Some((marker.to_string(), r));
                }
            }
            return Some(("-".to_string(), rest));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && digits <= 9 {
        let rest = &line[digits..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((format!("{}.", &line[..digits]), r));
        }
    }
    None
}

fn is_rule(line: &str) -> bool {
    let t: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3
        && (t.chars().all(|c| c == '-')
            || t.chars().all(|c| c == '*')
            || t.chars().all(|c| c == '_'))
}

/// Parse `text` into blocks.
pub fn markdown_blocks(text: &str) -> Vec<Block> {
    let text = strip_html_comments(&text.replace("\r\n", "\n"));
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut code: Option<Vec<&str>> = None;
    let flush = |paragraph: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(inlines(&paragraph.join(" "))));
            paragraph.clear();
        }
    };
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(lines) = code.as_mut() {
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                blocks.push(Block::Code(lines.join("\n")));
                code = None;
            } else {
                lines.push(line);
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush(&mut paragraph, &mut blocks);
            code = Some(Vec::new());
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            flush(&mut paragraph, &mut blocks);
            let title = trimmed[hashes..].trim().trim_end_matches('#').trim();
            blocks.push(Block::Heading(hashes as u8, inlines(title)));
            continue;
        }
        if is_rule(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Quote(inlines(rest.trim())));
            continue;
        }
        if let Some((marker, content)) = list_marker(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let indent = line.len() - trimmed.len();
            blocks.push(Block::ListItem {
                depth: indent / 2,
                marker,
                content: inlines(content.trim()),
            });
            continue;
        }
        paragraph.push(line.trim());
    }
    if let Some(lines) = code {
        blocks.push(Block::Code(lines.join("\n")));
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

/// Only these links open: web pages and mail. Anything else (`file:`, `javascript:`,
/// `retrogit://`, relative paths) is shown as text.
pub fn safe_link(url: &str) -> bool {
    ["https://", "http://", "mailto:"].iter().any(|p| {
        url.get(..p.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(p))
    })
}

/// For each index `j`, the first index `>= j` where `pat` starts in `chars` (if any).
/// Built once per block so an unmatched marker never rescans the rest of the text.
fn next_index(chars: &[char], pat: &[char]) -> Vec<Option<usize>> {
    let mut next = vec![None; chars.len() + 1];
    for j in (0..chars.len()).rev() {
        next[j] = if chars[j..].starts_with(pat) {
            Some(j)
        } else {
            next[j + 1]
        };
    }
    next
}

/// Whether `chars` holds `prefix` at `i`, without allocating.
fn starts_at(chars: &[char], i: usize, prefix: &str) -> bool {
    prefix
        .chars()
        .enumerate()
        .all(|(k, c)| chars.get(i + k) == Some(&c))
}

/// Parse inline markup of one block (linear in the length of `text`).
pub fn inlines(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<Inline> = Vec::new();
    let mut plain = String::new();
    let patterns: [&[char]; 7] = [
        &['`'],
        &['*'],
        &['_'],
        &['*', '*'],
        &['_', '_'],
        &[']', '('],
        &[')'],
    ];
    let tables: Vec<Vec<Option<usize>>> = patterns.iter().map(|p| next_index(&chars, p)).collect();
    let find = |from: usize, pat: &[char]| -> Option<usize> {
        let k = patterns.iter().position(|p| *p == pat)?;
        tables[k].get(from).copied().flatten()
    };
    let collect = |a: usize, b: usize| chars[a..b].iter().collect::<String>();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut token: Option<(Inline, usize)> = None;
        if c == '\\' && i + 1 < chars.len() && chars[i + 1].is_ascii_punctuation() {
            plain.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '`'
            && let Some(end) = find(i + 1, &['`'])
        {
            token = Some((Inline::Code(collect(i + 1, end)), end + 1));
        } else if (c == '*' || c == '_') && chars.get(i + 1) == Some(&c) {
            if let Some(end) = find(i + 2, &[c, c])
                && end > i + 2
            {
                token = Some((Inline::Bold(collect(i + 2, end)), end + 2));
            }
        } else if c == '*' || (c == '_' && (i == 0 || !chars[i - 1].is_alphanumeric())) {
            if let Some(end) = find(i + 1, &[c])
                && end > i + 1
                && !chars[i + 1].is_whitespace()
            {
                token = Some((Inline::Italic(collect(i + 1, end)), end + 1));
            }
        } else if c == '[' || (c == '!' && chars.get(i + 1) == Some(&'[')) {
            let open = if c == '!' { i + 1 } else { i };
            if let Some(close) = find(open + 1, &[']', '('])
                && let Some(end) = find(close + 2, &[')'])
            {
                let label = collect(open + 1, close);
                let label = if c == '!' {
                    format!("image: {label}")
                } else {
                    label
                };
                token = Some((
                    Inline::Link {
                        text: label,
                        url: collect(close + 2, end),
                    },
                    end + 1,
                ));
            }
        } else if c == 'h'
            && (i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == '(')
            && (starts_at(&chars, i, "https://") || starts_at(&chars, i, "http://"))
        {
            let len = chars[i..]
                .iter()
                .take_while(|c| !c.is_whitespace() && **c != ')')
                .count();
            let mut url = collect(i, i + len);
            while url.ends_with(['.', ',', ';', ':']) {
                url.pop();
            }
            let n = url.chars().count();
            token = Some((
                Inline::Link {
                    text: url.clone(),
                    url,
                },
                i + n,
            ));
        }
        match token {
            Some((t, next)) => {
                if !plain.is_empty() {
                    out.push(Inline::Text(std::mem::take(&mut plain)));
                }
                out.push(t);
                i = next;
            }
            None => {
                plain.push(c);
                i += 1;
            }
        }
    }
    if !plain.is_empty() {
        out.push(Inline::Text(plain));
    }
    out
}

fn show_inlines(ui: &mut Ui, items: &[Inline], size: f32, color: Color32) {
    let pal = theme::palette(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for item in items {
            match item {
                Inline::Text(t) => {
                    ui.label(RichText::new(t).size(size).color(color));
                }
                // W95FA has no bold face: bold text is drawn in navy.
                Inline::Bold(t) => {
                    ui.label(RichText::new(t).size(size).color(pal.link));
                }
                Inline::Italic(t) => {
                    ui.label(RichText::new(t).size(size).italics().color(color));
                }
                Inline::Code(t) => {
                    ui.label(
                        RichText::new(t)
                            .font(egui::FontId::monospace(theme::FONT_SIZE))
                            .background_color(pal.code_bg)
                            .color(pal.window_text),
                    );
                }
                Inline::Link { text, url } if safe_link(url) => {
                    ui.hyperlink_to(RichText::new(text).size(size), url);
                }
                Inline::Link { text, url } => {
                    ui.label(RichText::new(text).size(size).color(color))
                        .on_hover_text(url);
                }
            }
        }
    });
}

thread_local! {
    /// Number of texts parsed by `markdown_view` on this thread (tests check the cache).
    #[doc(hidden)]
    pub static PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Default)]
struct Parser;

impl egui::cache::ComputerMut<&str, Arc<Vec<Block>>> for Parser {
    fn compute(&mut self, text: &str) -> Arc<Vec<Block>> {
        PARSES.with(|p| p.set(p.get() + 1));
        Arc::new(markdown_blocks(text))
    }
}

type ParseCache = egui::cache::FrameCache<Arc<Vec<Block>>, Parser>;

/// Draw Markdown `text`, parsed once while it stays on screen. Web and mail links open
/// in the browser; other links are shown as text with the address on hover.
pub fn markdown_view(ui: &mut Ui, text: &str) {
    let pal = theme::palette(ui.ctx());
    let base = theme::FONT_SIZE;
    let blocks = ui
        .ctx()
        .memory_mut(|m| m.caches.cache::<ParseCache>().get(text).clone());
    for block in blocks.iter() {
        match block {
            Block::Heading(level, items) => {
                let size = base + (4.0 - f32::from((*level).min(4))).max(0.0) * 2.0;
                ui.add_space(4.0);
                show_inlines(ui, items, size, pal.link);
            }
            Block::Paragraph(items) => show_inlines(ui, items, base, pal.window_text),
            Block::ListItem {
                depth,
                marker,
                content,
            } => {
                ui.horizontal(|ui| {
                    ui.add_space(8.0 + *depth as f32 * 16.0);
                    ui.label(RichText::new(marker.as_str()).color(pal.window_text));
                    show_inlines(ui, content, base, pal.window_text);
                });
            }
            Block::Code(code) => {
                egui::Frame::NONE
                    .fill(pal.code_bg)
                    .inner_margin(egui::Margin::same(4))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(code.as_str())
                                    .font(egui::FontId::monospace(base))
                                    .color(pal.window_text),
                            )
                            .extend(),
                        );
                    });
            }
            Block::Quote(items) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("|").color(pal.gray_text));
                    show_inlines(ui, items, base, pal.gray_text);
                });
            }
            Block::Rule => {
                ui.separator();
            }
        }
        ui.add_space(2.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Inline {
        Inline::Text(s.into())
    }

    #[test]
    fn markdown_headings_lists_code_quotes_links() {
        let md = "## Summary\nFixes the\nlogin page.\n\n- one\n  - nested\n1. first\n- [ ] todo\n- [x] done\n\n```rust\nfn main() {}\n```\n> quoted\n\n---\nSee [docs](https://x.dev/d).";
        assert_eq!(
            markdown_blocks(md),
            vec![
                Block::Heading(2, vec![t("Summary")]),
                Block::Paragraph(vec![t("Fixes the login page.")]),
                Block::ListItem {
                    depth: 0,
                    marker: "-".into(),
                    content: vec![t("one")]
                },
                Block::ListItem {
                    depth: 1,
                    marker: "-".into(),
                    content: vec![t("nested")]
                },
                Block::ListItem {
                    depth: 0,
                    marker: "1.".into(),
                    content: vec![t("first")]
                },
                Block::ListItem {
                    depth: 0,
                    marker: "[ ]".into(),
                    content: vec![t("todo")]
                },
                Block::ListItem {
                    depth: 0,
                    marker: "[x]".into(),
                    content: vec![t("done")]
                },
                Block::Code("fn main() {}".into()),
                Block::Quote(vec![t("quoted")]),
                Block::Rule,
                Block::Paragraph(vec![
                    t("See "),
                    Inline::Link {
                        text: "docs".into(),
                        url: "https://x.dev/d".into()
                    },
                    t("."),
                ]),
            ]
        );
    }

    #[test]
    fn markdown_inline_emphasis_and_code() {
        assert_eq!(
            inlines("a **b** _c_ *d* `e*f` snake_case_name \\*g"),
            vec![
                t("a "),
                Inline::Bold("b".into()),
                t(" "),
                Inline::Italic("c".into()),
                t(" "),
                Inline::Italic("d".into()),
                t(" "),
                Inline::Code("e*f".into()),
                t(" snake_case_name *g"),
            ]
        );
        assert_eq!(inlines("2 * 3 = 6"), vec![t("2 * 3 = 6")], "lone star");
        assert_eq!(inlines("**unclosed"), vec![t("**unclosed")]);
    }

    #[test]
    fn bare_urls_and_images_become_links() {
        assert_eq!(
            inlines("see https://github.com/o/r/pull/1. ![shot](https://i/x.png)"),
            vec![
                t("see "),
                Inline::Link {
                    text: "https://github.com/o/r/pull/1".into(),
                    url: "https://github.com/o/r/pull/1".into()
                },
                t(". "),
                Inline::Link {
                    text: "image: shot".into(),
                    url: "https://i/x.png".into()
                },
            ]
        );
    }

    #[test]
    fn unknown_html_is_shown_as_text_and_comments_are_dropped() {
        assert_eq!(
            markdown_blocks("<!-- template hint -->\n<details>x</details>"),
            vec![Block::Paragraph(vec![t("<details>x</details>")])]
        );
        assert_eq!(
            markdown_blocks("a <!-- never closed"),
            vec![Block::Paragraph(vec![t("a")])]
        );
    }

    #[test]
    fn an_unclosed_code_block_keeps_its_lines() {
        assert_eq!(
            markdown_blocks("```\nx\ny"),
            vec![Block::Code("x\ny".into())]
        );
        assert_eq!(markdown_blocks(""), vec![]);
        assert_eq!(
            markdown_blocks("\r\nWindows\r\nlines\r\n"),
            vec![Block::Paragraph(vec![t("Windows lines")])]
        );
    }
}
