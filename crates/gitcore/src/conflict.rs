//! Conflicted files: the text Git left with conflict markers, cut into blocks, and the
//! ways to resolve a file.

use crate::{GitError, Operation, Repo};

const OPEN: &str = "<<<<<<<";
const BASE: &str = "|||||||";
const SEP: &str = "=======";
const CLOSE: &str = ">>>>>>>";

/// A piece of a conflicted file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Text both sides agree on (line endings included).
    Common(String),
    /// A conflict block. `raw` is the block as written, markers included.
    Conflict {
        mine: String,
        /// The common ancestor's text (`diff3` / `zdiff3` styles only).
        base: Option<String>,
        theirs: String,
        mine_label: String,
        theirs_label: String,
        raw: String,
    },
}

/// What to keep of a conflict block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Mine,
    Theirs,
    /// Mine, then theirs.
    Both,
}

/// `marker` at the start of `line`, followed by a space or the end of the line.
fn marker_label<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(marker)?;
    let rest = rest.trim_end_matches(['\n', '\r']);
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix(' ')
    }
}

/// Cut `text` into common parts and conflict blocks. An incomplete block (no closing
/// marker, or no separator) stays common text.
pub fn parse_conflicts(text: &str) -> Vec<Segment> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out: Vec<Segment> = Vec::new();
    let mut common = String::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(mine_label) = marker_label(lines[i], OPEN) else {
            common.push_str(lines[i]);
            i += 1;
            continue;
        };
        // Look for the rest of the block.
        let (mut base_at, mut sep_at, mut close_at) = (None, None, None);
        for (j, line) in lines.iter().enumerate().skip(i + 1) {
            if marker_label(line, OPEN).is_some() {
                break;
            }
            if sep_at.is_none() && base_at.is_none() && marker_label(line, BASE).is_some() {
                base_at = Some(j);
            } else if sep_at.is_none() && line.trim_end_matches(['\n', '\r']) == SEP {
                sep_at = Some(j);
            } else if sep_at.is_some() && marker_label(line, CLOSE).is_some() {
                close_at = Some(j);
                break;
            }
        }
        let (Some(sep), Some(close)) = (sep_at, close_at) else {
            common.push_str(lines[i]);
            i += 1;
            continue;
        };
        if !common.is_empty() {
            out.push(Segment::Common(std::mem::take(&mut common)));
        }
        let join = |a: usize, b: usize| lines[a..b].concat();
        let mine_end = base_at.unwrap_or(sep);
        out.push(Segment::Conflict {
            mine: join(i + 1, mine_end),
            base: base_at.map(|b| join(b + 1, sep)),
            theirs: join(sep + 1, close),
            mine_label: mine_label.to_string(),
            theirs_label: marker_label(lines[close], CLOSE)
                .unwrap_or_default()
                .to_string(),
            raw: join(i, close + 1),
        });
        i = close + 1;
    }
    if !common.is_empty() {
        out.push(Segment::Common(common));
    }
    out
}

/// Number of conflict blocks left in `text`.
pub fn conflict_count(text: &str) -> usize {
    parse_conflicts(text)
        .iter()
        .filter(|s| matches!(s, Segment::Conflict { .. }))
        .count()
}

/// Whether a line still starts with a conflict marker (even in a broken block).
pub fn has_marker_lines(text: &str) -> bool {
    text.lines()
        .any(|l| marker_label(l, OPEN).is_some() || marker_label(l, CLOSE).is_some())
}

/// `text` with conflict block number `block` replaced by `choice`; everything else (other
/// blocks, edits made by hand) is kept as is.
pub fn apply_choice(text: &str, block: usize, choice: Choice) -> String {
    let crlf = text.contains("\r\n");
    let mut out = String::with_capacity(text.len());
    let mut n = 0;
    for seg in parse_conflicts(text) {
        match seg {
            Segment::Common(t) => out.push_str(&t),
            Segment::Conflict {
                mine, theirs, raw, ..
            } => {
                if n == block {
                    match choice {
                        Choice::Mine => out.push_str(&mine),
                        Choice::Theirs => out.push_str(&theirs),
                        Choice::Both => {
                            out.push_str(&mine);
                            if !mine.is_empty() && !mine.ends_with('\n') && !theirs.is_empty() {
                                out.push_str(if crlf { "\r\n" } else { "\n" });
                            }
                            out.push_str(&theirs);
                        }
                    }
                } else {
                    out.push_str(&raw);
                }
                n += 1;
            }
        }
    }
    out
}

/// Which file a range is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Mine,
    Result,
    Theirs,
}

fn line_count(s: &str) -> usize {
    s.split_inclusive('\n').count()
}

/// Lines (`start`, `count`, 0-based) of each conflict block: in the result text itself
/// (markers included), or in the mine / theirs file the blocks were cut from.
pub fn block_lines(segments: &[Segment], pane: Pane) -> Vec<(usize, usize)> {
    let mut line = 0;
    let mut out = Vec::new();
    for seg in segments {
        match seg {
            Segment::Common(t) => line += line_count(t),
            Segment::Conflict {
                mine, theirs, raw, ..
            } => {
                let n = match pane {
                    Pane::Mine => line_count(mine),
                    Pane::Theirs => line_count(theirs),
                    Pane::Result => line_count(raw),
                };
                out.push((line, n));
                line += n;
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// Both sides changed the text: resolve block by block.
    Content,
    /// Not text (or not UTF-8): keep one side.
    Binary,
    /// Deleted on our side, changed on theirs.
    DeletedByUs,
    /// Changed on our side, deleted on theirs.
    DeletedByThem,
    /// Added on both sides with different contents.
    AddedByBoth,
}

/// Everything the conflict editor needs about one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictFile {
    pub path: String,
    pub kind: ConflictKind,
    /// Our version (HEAD for a merge, upstream for a rebase); `None` if absent or binary.
    pub mine: Option<String>,
    pub theirs: Option<String>,
    /// The working-tree file, with Git's markers.
    pub working: Option<String>,
    pub operation: Option<Operation>,
}

/// Whole-file resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Ours,
    Theirs,
}

/// Text of `bytes`, if it is UTF-8 without NUL bytes.
fn as_text(bytes: &[u8]) -> Option<String> {
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes.to_vec()).ok()
}

impl Repo {
    /// The conflict of `path` (stages of the index and the working-tree file).
    pub fn conflict(&self, path: &str) -> Result<ConflictFile, GitError> {
        let repo = self.git();
        let map = |e: git2::Error| GitError::from_git2(&e);
        let mut index = repo.index().map_err(map)?;
        // libgit2 caches the index: `git` (resolving) changes it behind our back.
        index.read(false).map_err(map)?;
        let mut found = None;
        for c in index.conflicts().map_err(map)? {
            let c = c.map_err(map)?;
            let p = [&c.our, &c.their, &c.ancestor]
                .into_iter()
                .flatten()
                .next()
                .map(|e| String::from_utf8_lossy(&e.path).into_owned());
            if p.as_deref() == Some(path) {
                found = Some(c);
                break;
            }
        }
        let c = found.ok_or_else(|| GitError::Other(format!("'{path}' is not in conflict")))?;
        let blob = |e: &Option<git2::IndexEntry>| -> Result<Option<Vec<u8>>, GitError> {
            match e {
                Some(e) => Ok(Some(repo.find_blob(e.id).map_err(map)?.content().to_vec())),
                None => Ok(None),
            }
        };
        let (ours, theirs) = (blob(&c.our)?, blob(&c.their)?);
        let working = std::fs::read(self.workdir()?.join(path))
            .ok()
            .and_then(|b| as_text(&b));
        let mine = ours.as_deref().and_then(as_text);
        let theirs_text = theirs.as_deref().and_then(as_text);
        let kind = match (&ours, &theirs) {
            (None, Some(_)) => ConflictKind::DeletedByUs,
            (Some(_), None) => ConflictKind::DeletedByThem,
            (Some(_), Some(_)) if mine.is_none() || theirs_text.is_none() || working.is_none() => {
                ConflictKind::Binary
            }
            (Some(_), Some(_)) if c.ancestor.is_none() => ConflictKind::AddedByBoth,
            _ => ConflictKind::Content,
        };
        Ok(ConflictFile {
            path: path.to_string(),
            kind,
            mine,
            theirs: theirs_text,
            working,
            operation: self.operation_in_progress(),
        })
    }

    /// Write the resolved `content` of `path` and mark it resolved (`git add`).
    pub fn resolve_with_content(&self, path: &str, content: &str) -> Result<(), GitError> {
        std::fs::write(self.workdir()?.join(path), content)
            .map_err(|e| GitError::Other(format!("cannot write {path}: {e}")))?;
        self.git_ok(&["add", "--", path]).map(|_| ())
    }

    /// Keep one side's whole version of `path`.
    pub fn resolve_with(&self, path: &str, pick: Pick) -> Result<(), GitError> {
        let side = match pick {
            Pick::Ours => "--ours",
            Pick::Theirs => "--theirs",
        };
        self.git_ok(&["checkout", side, "--", path])?;
        self.git_ok(&["add", "--", path]).map(|_| ())
    }

    /// Resolve by deleting `path`.
    pub fn resolve_delete(&self, path: &str) -> Result<(), GitError> {
        self.git_ok(&["rm", "-q", "--", path]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: &str = "a\n<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> feat/x\nz\n";

    fn conflicts(text: &str) -> Vec<(String, Option<String>, String, String, String)> {
        parse_conflicts(text)
            .into_iter()
            .filter_map(|s| match s {
                Segment::Conflict {
                    mine,
                    base,
                    theirs,
                    mine_label,
                    theirs_label,
                    ..
                } => Some((mine, base, theirs, mine_label, theirs_label)),
                Segment::Common(_) => None,
            })
            .collect()
    }

    #[test]
    fn blocks_and_labels() {
        let segs = parse_conflicts(ONE);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0], Segment::Common("a\n".into()));
        assert_eq!(
            conflicts(ONE),
            [(
                "mine\n".to_string(),
                None,
                "theirs\n".to_string(),
                "HEAD".to_string(),
                "feat/x".to_string()
            )]
        );
        assert_eq!(segs[2], Segment::Common("z\n".into()));
        assert_eq!(conflict_count(ONE), 1);
    }

    #[test]
    fn several_blocks_diff3_and_empty_sides() {
        let text = "<<<<<<< ours\nm1\n||||||| base\nb1\n=======\nt1\n>>>>>>> theirs\nmid\n<<<<<<< ours\n=======\nonly theirs\n>>>>>>> theirs\n";
        let c = conflicts(text);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].1.as_deref(), Some("b1\n"));
        assert_eq!(c[0].0, "m1\n");
        assert_eq!(c[1].0, "", "mine deleted the lines");
        assert_eq!(c[1].2, "only theirs\n");
    }

    #[test]
    fn crlf_and_missing_final_newline_are_kept() {
        let text = "a\r\n<<<<<<< HEAD\r\nmine\r\n=======\r\ntheirs\r\n>>>>>>> x\r\nend";
        assert_eq!(conflicts(text)[0].0, "mine\r\n");
        assert_eq!(apply_choice(text, 0, Choice::Theirs), "a\r\ntheirs\r\nend");
        let eof = "<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> x";
        assert_eq!(apply_choice(eof, 0, Choice::Mine), "mine\n");
    }

    #[test]
    fn incomplete_markers_stay_text() {
        let broken = "a\n<<<<<<< HEAD\nmine\n=======\ntheirs\nno end\n";
        assert_eq!(conflict_count(broken), 0);
        assert_eq!(parse_conflicts(broken), [Segment::Common(broken.into())]);
        assert!(has_marker_lines(broken));
        assert!(!has_marker_lines("a\n======= not a marker? fine\n"));
        // "=======" alone is a separator only inside a block.
        assert_eq!(conflict_count("=======\n"), 0);
    }

    #[test]
    fn choices_replace_one_block_and_keep_the_rest() {
        let two = format!("{ONE}{ONE}");
        let mine_first = apply_choice(&two, 0, Choice::Mine);
        assert!(
            mine_first.starts_with("a\nmine\nz\na\n<<<<<<< HEAD\n"),
            "{mine_first}"
        );
        assert_eq!(conflict_count(&mine_first), 1);
        assert_eq!(
            apply_choice(&mine_first, 0, Choice::Both),
            "a\nmine\nz\na\nmine\ntheirs\nz\n",
            "the remaining block is now number 0"
        );
        // Hand edits outside the block survive.
        let edited = ONE.replace("a\n", "edited\n");
        assert_eq!(
            apply_choice(&edited, 0, Choice::Theirs),
            "edited\ntheirs\nz\n"
        );
        // A block number past the end changes nothing.
        assert_eq!(apply_choice(ONE, 5, Choice::Mine), ONE);
    }

    #[test]
    fn block_lines_in_each_pane() {
        let text = "a\nb\n<<<<<<< HEAD\nm1\nm2\n=======\nt1\n>>>>>>> x\nc\n<<<<<<< HEAD\n=======\nt2\nt3\n>>>>>>> x\n";
        let segs = parse_conflicts(text);
        assert_eq!(block_lines(&segs, Pane::Result), [(2, 6), (9, 5)]);
        assert_eq!(block_lines(&segs, Pane::Mine), [(2, 2), (5, 0)]);
        assert_eq!(block_lines(&segs, Pane::Theirs), [(2, 1), (4, 2)]);
    }
}
