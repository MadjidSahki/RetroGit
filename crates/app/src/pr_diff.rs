//! Pull request file patches (from the GitHub API) as `FileDiff`s, so the diff view and
//! syntax highlighting of the History tab can show them, and where line comments go.

use gitcore::{DiffLine, FileDiff, Hunk, LineKind, Side};
use github::{DiffSide, ReviewThread};

/// `@@ -1,3 +1,4 @@ fn x()` => (1, 3, 1, 4). A missing count means 1.
fn parse_header(line: &str) -> Option<(u32, u32, u32, u32)> {
    let inner = line.strip_prefix("@@ ")?;
    let (ranges, _) = inner.split_once(" @@")?;
    let (old, new) = ranges.split_once(' ')?;
    let range = |r: &str, sign: char| -> Option<(u32, u32)> {
        let r = r.strip_prefix(sign)?;
        match r.split_once(',') {
            Some((s, n)) => Some((s.parse().ok()?, n.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (os, on) = range(old, '-')?;
    let (ns, nn) = range(new, '+')?;
    Some((os, on, ns, nn))
}

/// Convert GitHub's `patch` text (hunks only, no file header) into a `FileDiff`.
/// `None` (binary file, or too large for GitHub) gives a binary diff.
pub fn parse_patch(path: &str, patch: Option<&str>) -> FileDiff {
    let mut diff = FileDiff {
        path: path.to_string(),
        side: Side::Staged,
        binary: patch.is_none(),
        hunks: Vec::new(),
    };
    let Some(patch) = patch else {
        return diff;
    };
    let (mut old_no, mut new_no) = (0u32, 0u32);
    for line in patch.split('\n') {
        if line.starts_with("@@") {
            if let Some((os, on, ns, nn)) = parse_header(line) {
                diff.hunks.push(Hunk {
                    header: line.trim_end().to_string(),
                    old_start: os,
                    old_lines: on,
                    new_start: ns,
                    new_lines: nn,
                    lines: Vec::new(),
                });
                (old_no, new_no) = (os, ns);
            }
            continue;
        }
        let Some(hunk) = diff.hunks.last_mut() else {
            continue;
        };
        if line.starts_with('\\') {
            // "\ No newline at end of file" applies to the previous line.
            if let Some(last) = hunk.lines.last_mut() {
                last.no_newline_at_eof = true;
                let trimmed = last.text.trim_end_matches('\n').to_string();
                last.raw = trimmed.as_bytes().to_vec();
                last.text = trimmed;
            }
            continue;
        }
        let (kind, rest) = match line.chars().next() {
            Some('+') => (LineKind::Added, &line[1..]),
            Some('-') => (LineKind::Removed, &line[1..]),
            Some(' ') => (LineKind::Context, &line[1..]),
            // A trailing empty string after the last '\n', or noise.
            _ => continue,
        };
        let (o, n) = match kind {
            LineKind::Added => {
                new_no += 1;
                (None, Some(new_no - 1))
            }
            LineKind::Removed => {
                old_no += 1;
                (Some(old_no - 1), None)
            }
            LineKind::Context => {
                old_no += 1;
                new_no += 1;
                (Some(old_no - 1), Some(new_no - 1))
            }
        };
        let text = format!("{rest}\n");
        hunk.lines.push(DiffLine {
            kind,
            old_no: o,
            new_no: n,
            raw: text.as_bytes().to_vec(),
            text,
            no_newline_at_eof: false,
        });
    }
    diff
}

/// Where a comment on `line` goes: removed lines on the old side, others on the new side.
pub fn line_target(line: &DiffLine) -> Option<(u32, DiffSide)> {
    match line.kind {
        LineKind::Removed => line.old_no.map(|n| (n, DiffSide::Left)),
        LineKind::Added | LineKind::Context => line.new_no.map(|n| (n, DiffSide::Right)),
    }
}

/// Indexes of the current (not outdated) threads of `path` attached to `line`. Context
/// lines can carry threads on either side (github.com's split view has both columns).
pub fn threads_at(threads: &[ReviewThread], path: &str, line: &DiffLine) -> Vec<usize> {
    let mut targets: Vec<(u32, DiffSide)> = line_target(line).into_iter().collect();
    if line.kind == LineKind::Context
        && let Some(old) = line.old_no
    {
        targets.push((old, DiffSide::Left));
    }
    threads
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            !t.outdated
                && t.path == path
                && targets
                    .iter()
                    .any(|(n, side)| t.line == Some(*n) && t.side == *side)
        })
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const PATCH: &str = "@@ -1,3 +1,4 @@ fn main() {\n a\n-b\n+B\n+c\n d\n@@ -10 +11,2 @@\n x\n+y\n\\ No newline at end of file";

    #[test]
    fn patches_become_hunks_with_line_numbers() {
        let d = parse_patch("src/a.rs", Some(PATCH));
        assert!(!d.binary);
        assert_eq!(d.hunks.len(), 2);
        let h = &d.hunks[0];
        assert_eq!(
            (h.old_start, h.old_lines, h.new_start, h.new_lines),
            (1, 3, 1, 4)
        );
        assert_eq!(h.header, "@@ -1,3 +1,4 @@ fn main() {");
        let nums: Vec<_> = h
            .lines
            .iter()
            .map(|l| (l.kind, l.old_no, l.new_no))
            .collect();
        assert_eq!(
            nums,
            [
                (LineKind::Context, Some(1), Some(1)),
                (LineKind::Removed, Some(2), None),
                (LineKind::Added, None, Some(2)),
                (LineKind::Added, None, Some(3)),
                (LineKind::Context, Some(3), Some(4)),
            ]
        );
        assert_eq!(h.lines[2].text, "B\n");
        let h2 = &d.hunks[1];
        assert_eq!(
            (h2.old_start, h2.old_lines, h2.new_start, h2.new_lines),
            (10, 1, 11, 2)
        );
        assert_eq!(h2.lines[1].new_no, Some(12));
        assert!(h2.lines[1].no_newline_at_eof);
        assert_eq!(h2.lines[1].text, "y");
        assert_eq!(d.line_count(), 7);
    }

    #[test]
    fn missing_or_empty_patches() {
        assert!(parse_patch("logo.png", None).binary);
        let empty = parse_patch("renamed.rs", Some(""));
        assert!(!empty.binary);
        assert!(empty.hunks.is_empty());
    }

    #[test]
    fn line_target_maps_sides() {
        let d = parse_patch("a", Some(PATCH));
        let l = &d.hunks[0].lines;
        assert_eq!(
            line_target(&l[0]),
            Some((1, DiffSide::Right)),
            "context: new side"
        );
        assert_eq!(
            line_target(&l[1]),
            Some((2, DiffSide::Left)),
            "removed: old side"
        );
        assert_eq!(line_target(&l[2]), Some((2, DiffSide::Right)));
    }

    #[test]
    fn threads_are_found_on_their_line_and_side() {
        let d = parse_patch("a", Some(PATCH));
        let t = |line: Option<u32>, side, outdated| ReviewThread {
            path: "a".into(),
            line,
            original_line: line,
            side,
            outdated,
            resolved: false,
            comments: vec![],
        };
        let threads = vec![
            t(Some(2), DiffSide::Left, false),
            t(Some(2), DiffSide::Right, false),
            t(Some(2), DiffSide::Right, true),
            ReviewThread {
                path: "other".into(),
                ..t(Some(2), DiffSide::Right, false)
            },
        ];
        let l = &d.hunks[0].lines;
        // A context line can be commented on either column (split view on github.com).
        let left_context = vec![t(Some(1), DiffSide::Left, false)];
        assert_eq!(threads_at(&left_context, "a", &l[0]), [0]);
        assert_eq!(threads_at(&threads, "a", &l[1]), [0]);
        assert_eq!(threads_at(&threads, "a", &l[2]), [1]);
        assert!(threads_at(&threads, "a", &l[0]).is_empty());
    }
}
