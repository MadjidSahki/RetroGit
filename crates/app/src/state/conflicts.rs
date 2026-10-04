//! State of the conflict editor (sub-project 6a).

use gitcore::{
    Change, Choice, ConflictFile, DiffLine, FileDiff, Hunk, LineKind, Pane, Pick, Segment, Side,
    apply_choice, locate_blocks, parse_conflicts,
};

use crate::highlight::{Colors, MAX_BYTES, MAX_LINES, Target};

use super::{AppState, ChangesView};
use crate::protocol::{AppError, Severity};
use crate::strings as s;

/// A whole text as a one-hunk diff of unchanged lines: what the background highlighter
/// colors (the conflict panes reuse the diff highlighting service).
pub fn text_as_diff(path: &str, text: &str) -> FileDiff {
    let lines = text
        .split_inclusive('\n')
        .enumerate()
        .map(|(i, l)| DiffLine {
            kind: LineKind::Context,
            old_no: Some(i as u32 + 1),
            new_no: Some(i as u32 + 1),
            text: l.to_string(),
            raw: l.as_bytes().to_vec(),
            no_newline_at_eof: !l.ends_with('\n'),
        })
        .collect::<Vec<_>>();
    let n = lines.len() as u32;
    FileDiff {
        path: path.to_string(),
        side: Side::Unstaged,
        binary: false,
        hunks: vec![Hunk {
            header: String::new(),
            old_start: 1,
            old_lines: n,
            new_start: 1,
            new_lines: n,
            lines,
        }],
    }
}

/// What waits for the user's confirmation in the conflict editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictConfirm {
    /// Keep one side for the whole file.
    WholeFile(Pick),
    /// Mark resolved although conflict markers remain.
    ResolveWithMarkers,
    /// Leave the edited file for another one (`Some(path)`) or close the editor (`None`).
    Discard(Option<String>),
    /// Abort the merge / rebase although the file was edited.
    Abort,
    /// Open another repository although the file was edited.
    OpenRepo(std::path::PathBuf),
}

/// The file being resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictEditor {
    pub file: ConflictFile,
    /// The result pane's text (starts as the file Git left, with its markers).
    pub result: String,
    /// Conflict block shown as current (0-based, among those left in `result`).
    pub current: usize,
    /// The user changed `result`: reloads must not overwrite it.
    pub edited: bool,
    /// The file changed on disk while edited: offered with Reload.
    pub on_disk: Option<ConflictFile>,
    pub confirm: Option<ConflictConfirm>,
    /// A resolution was sent and Git has not answered yet: the buttons are greyed.
    pub resolving: bool,
    /// Syntax colors of each pane (computed in the background; `result_colors` again after
    /// every change of the result).
    pub mine_colors: Colors,
    pub theirs_colors: Colors,
    pub result_colors: Colors,
    /// Last colors computed for the result, still drawn (line by line, where the text did
    /// not change) while new ones are computed: no flicker while typing.
    pub result_colors_shown: Colors,
    /// `result` cut into blocks, kept in step with it (not parsed again every frame).
    pub segments: Vec<Segment>,
    /// The file as Git left it, cut into blocks (maps blocks to the full versions).
    original: Vec<Segment>,
    /// Where each original block is in the full mine / theirs versions.
    mine_blocks: Vec<Option<(usize, usize)>>,
    theirs_blocks: Vec<Option<(usize, usize)>>,
}

/// How many `\r` turning lone `\n` into `\r\n` go before char `cursor` of `text`.
pub fn added_cr_before(text: &str, cursor: usize) -> usize {
    let mut prev = None;
    let mut added = 0;
    for c in text.chars().take(cursor) {
        if c == '\n' && prev != Some('\r') {
            added += 1;
        }
        prev = Some(c);
    }
    added
}

/// `text` with every lone `\n` turned into `\r\n`.
fn to_crlf(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev = None;
    for c in text.chars() {
        if c == '\n' && prev != Some('\r') {
            out.push('\r');
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

impl ConflictEditor {
    /// Send `cmd` (a resolution): the buttons stay greyed until Git answers.
    pub fn resolve(&mut self, cmd: crate::protocol::Command) -> crate::protocol::Command {
        self.resolving = true;
        cmd
    }

    /// The working file uses CRLF line endings: what the user types follows them.
    pub fn crlf(&self) -> bool {
        self.file
            .working
            .as_deref()
            .is_some_and(|w| w.contains("\r\n"))
    }

    pub fn new(file: ConflictFile) -> ConflictEditor {
        let result = file.working.clone().unwrap_or_default();
        let original = parse_conflicts(&result);
        let locate = |text: &Option<String>, pane| {
            text.as_deref()
                .map(|t| locate_blocks(&original, t, pane))
                .unwrap_or_default()
        };
        ConflictEditor {
            mine_blocks: locate(&file.mine, Pane::Mine),
            theirs_blocks: locate(&file.theirs, Pane::Theirs),
            segments: original.clone(),
            original,
            result,
            file,
            current: 0,
            edited: false,
            on_disk: None,
            confirm: None,
            resolving: false,
            mine_colors: Colors::NotRequested,
            theirs_colors: Colors::NotRequested,
            result_colors: Colors::NotRequested,
            result_colors_shown: Colors::NotRequested,
        }
    }

    /// Colors computed for `diff`, kept only if `diff` is still what the pane shows.
    pub fn colors_loaded(&mut self, target: Target, diff: &FileDiff, colors: Colors) {
        let path = &self.file.path;
        let (text, slot) = match target {
            Target::ConflictMine => (self.file.mine.as_deref(), &mut self.mine_colors),
            Target::ConflictTheirs => (self.file.theirs.as_deref(), &mut self.theirs_colors),
            Target::ConflictResult => (Some(self.result.as_str()), &mut self.result_colors),
            _ => return,
        };
        if text.is_some_and(|t| text_as_diff(path, t) == *diff) {
            if target == Target::ConflictResult {
                self.result_colors_shown = colors.clone();
            }
            *slot = colors;
        }
    }

    pub fn conflicts_left(&self) -> usize {
        self.segments
            .iter()
            .filter(|s| matches!(s, Segment::Conflict { .. }))
            .count()
    }

    fn set_result(&mut self, text: String) {
        self.segments = parse_conflicts(&text);
        self.result = text;
        self.result_colors = Colors::NotRequested;
        self.edited = true;
        self.clamp();
    }

    /// Lines of the current block in the full version of `pane` (Mine or Theirs), found by
    /// matching it with the block Git wrote.
    pub fn current_side_block(&self, pane: Pane) -> Option<(usize, usize)> {
        let raw = self
            .segments
            .iter()
            .filter_map(|s| match s {
                Segment::Conflict { raw, .. } => Some(raw),
                Segment::Common(_) => None,
            })
            .nth(self.current)?;
        let index = self
            .original
            .iter()
            .filter_map(|s| match s {
                Segment::Conflict { raw, .. } => Some(raw),
                Segment::Common(_) => None,
            })
            .position(|r| r == raw)?;
        let blocks = match pane {
            Pane::Mine => &self.mine_blocks,
            Pane::Theirs => &self.theirs_blocks,
            Pane::Result => return None,
        };
        blocks.get(index).copied().flatten()
    }

    /// Highlighting jobs for the panes still without colors (each asked once); texts too
    /// large for highlighting stay plain without being sent.
    pub fn colors_to_request(&mut self) -> Vec<(Target, FileDiff)> {
        let path = self.file.path.clone();
        let too_big = |t: &str| t.len() > MAX_BYTES || t.lines().count() > MAX_LINES;
        let mut out = Vec::new();
        let panes = [
            (
                Target::ConflictMine,
                self.file.mine.as_deref(),
                &mut self.mine_colors,
            ),
            (
                Target::ConflictTheirs,
                self.file.theirs.as_deref(),
                &mut self.theirs_colors,
            ),
            (
                Target::ConflictResult,
                Some(self.result.as_str()),
                &mut self.result_colors,
            ),
        ];
        for (target, text, slot) in panes {
            if *slot != Colors::NotRequested {
                continue;
            }
            match text {
                Some(t) if !too_big(t) => {
                    out.push((target, text_as_diff(&path, t)));
                    *slot = Colors::Pending;
                }
                _ => *slot = Colors::Plain,
            }
        }
        out
    }

    /// Replace the current block by `choice`; the next block becomes current.
    pub fn choose(&mut self, choice: Choice) {
        if self.current >= self.conflicts_left() {
            return;
        }
        let text = apply_choice(&self.result, self.current, choice);
        self.set_result(text);
    }

    pub fn next(&mut self) {
        self.current += 1;
        self.clamp();
    }

    pub fn previous(&mut self) {
        self.current = self.current.saturating_sub(1);
    }

    /// Keep `current` on an existing block (after edits).
    pub fn clamp(&mut self) {
        self.current = self.current.min(self.conflicts_left().saturating_sub(1));
    }

    /// The result pane's text after the user typed in it.
    pub fn typed(&mut self, text: String) {
        let text = if self.crlf() { to_crlf(&text) } else { text };
        self.set_result(text);
    }

    /// The user typed in the result pane.
    pub fn edit(&mut self, text: String) {
        if text != self.result {
            self.set_result(text);
        }
    }

    /// Take the version found on disk, dropping the edits.
    pub fn reload(&mut self) {
        if let Some(file) = self.on_disk.take() {
            *self = ConflictEditor::new(file);
        }
    }
}

impl ChangesView {
    /// Conflicted files, in status order.
    pub fn conflicted(&self) -> impl Iterator<Item = &gitcore::FileStatus> {
        self.files
            .iter()
            .filter(|f| f.unstaged == Some(Change::Conflicted))
    }

    /// Ask to show `path` in the conflict editor (the UI then loads it). With unsaved edits
    /// on another file, asks first.
    pub fn open_conflict(&mut self, path: &str) -> bool {
        if let Some(ed) = self.conflict.as_mut()
            && ed.edited
            && ed.file.path != path
        {
            ed.confirm = Some(ConflictConfirm::Discard(Some(path.to_string())));
            return false;
        }
        self.conflict_path = Some(path.to_string());
        self.conflict_error = None;
        self.shown = None;
        self.diff = None;
        true
    }

    /// Close the editor (asks first if there are unsaved edits).
    pub fn close_conflict(&mut self) {
        if let Some(ed) = self.conflict.as_mut()
            && ed.edited
        {
            ed.confirm = Some(ConflictConfirm::Discard(None));
            return;
        }
        self.conflict = None;
        self.conflict_path = None;
    }

    /// Open another repository: asks first if the open conflict was edited. Returns
    /// whether it can be opened now.
    pub fn request_open_repo(&mut self, path: &std::path::Path) -> bool {
        match self.conflict.as_mut() {
            Some(ed) if ed.edited => {
                ed.confirm = Some(ConflictConfirm::OpenRepo(path.to_path_buf()));
                false
            }
            _ => true,
        }
    }

    /// Abort the operation: asks first if the open conflict was edited. Returns whether it
    /// can be aborted now.
    pub fn request_abort(&mut self) -> bool {
        if let Some(ed) = self.conflict.as_mut()
            && ed.edited
        {
            ed.confirm = Some(ConflictConfirm::Abort);
            return false;
        }
        self.conflict = None;
        self.conflict_path = None;
        true
    }

    /// The user confirmed dropping the edits: go where they wanted.
    pub fn discard_conflict_edits(&mut self) -> Option<String> {
        let target = match self.conflict.as_ref().and_then(|e| e.confirm.clone()) {
            Some(ConflictConfirm::Discard(t)) => t,
            _ => return None,
        };
        self.conflict = None;
        self.conflict_path = None;
        match target {
            Some(path) => {
                self.open_conflict(&path);
                Some(path)
            }
            None => None,
        }
    }
}

impl AppState {
    pub(super) fn conflict_loaded(&mut self, file: ConflictFile) {
        let c = &mut self.changes;
        if c.conflict_path.as_deref() != Some(file.path.as_str()) {
            return;
        }
        c.conflict_error = None;
        if let Some(ed) = c.conflict.as_mut() {
            ed.resolving = false;
        }
        match c.conflict.as_mut() {
            Some(ed) if ed.file.path == file.path && ed.edited => {
                // Never overwrite what the user typed; offer the new version instead.
                if file.working != ed.file.working {
                    ed.on_disk = Some(file);
                }
            }
            Some(ed) if ed.file == file => {}
            _ => c.conflict = Some(ConflictEditor::new(file)),
        }
    }

    /// `path` was resolved: show the next conflicted file, or say it is over.
    pub(super) fn conflict_resolved(&mut self, path: &str) {
        let c = &mut self.changes;
        if let Some(ed) = c.conflict.as_mut() {
            ed.resolving = false;
        }
        if c.conflict.as_ref().is_some_and(|e| e.file.path == path) {
            c.conflict = None;
            c.conflict_path = None;
        }
        let next = c.conflicted().map(|f| f.path.clone()).find(|p| p != path);
        match next {
            Some(p) => {
                c.open_conflict(&p);
                c.load_conflict = true;
            }
            None => {
                let message = match self.operation {
                    Some(gitcore::Operation::Rebase) => s::ALL_RESOLVED_REBASE,
                    Some(gitcore::Operation::Merge) => s::ALL_RESOLVED_MERGE,
                    Some(gitcore::Operation::CherryPick | gitcore::Operation::Revert) => {
                        s::ALL_RESOLVED_CONTINUE
                    }
                    None => s::ALL_RESOLVED,
                };
                self.messages
                    .push_back(AppError::new(Severity::Info, message));
            }
        }
    }

    /// After a status refresh: an editor whose file is no longer in conflict closes.
    pub(super) fn sync_conflict_editor(&mut self) {
        let c = &mut self.changes;
        if let Some(path) = c.conflict_path.clone()
            && !c.conflicted().any(|f| f.path == path)
        {
            c.conflict = None;
            c.conflict_path = None;
        }
    }
}
