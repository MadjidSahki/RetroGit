//! State of the pull request additions (sub-project 6b): editing, people, line ranges and
//! suggestions.

use gitcore::FileDiff;
use github::DiffSide;

use crate::strings as s;

/// Which list of people a dialog edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeopleKind {
    Reviewers,
    Assignees,
}

impl PeopleKind {
    pub fn title(self) -> &'static str {
        match self {
            PeopleKind::Reviewers => s::REVIEWERS_TITLE,
            PeopleKind::Assignees => s::ASSIGNEES_TITLE,
        }
    }
}

/// Lines selected in the Files tab: line indexes `from..=to` of hunk `hunk`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineSelection {
    pub hunk: usize,
    pub from: usize,
    pub to: usize,
}

/// Shift+click on line `line` of hunk `hunk`: the selection grows to it, within its hunk.
pub fn extend_selection(sel: LineSelection, hunk: usize, line: usize) -> LineSelection {
    if hunk != sel.hunk {
        return sel;
    }
    LineSelection {
        hunk,
        from: sel.from.min(line),
        to: sel.to.max(line),
    }
}

/// What a selection comments on: lines `start..=end` of one side, and their text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionTarget {
    pub start: u32,
    pub end: u32,
    pub side: DiffSide,
    /// Text of the selected lines of that side (without line endings).
    pub lines: Vec<String>,
}

/// The side of the selection's first line that has a line number on it (the new side
/// for added and context lines); lines of the other side in the range are left out.
pub fn selection_target(diff: &FileDiff, sel: LineSelection) -> Option<SelectionTarget> {
    let hunk = diff.hunks.get(sel.hunk)?;
    let range = hunk
        .lines
        .get(sel.from..=sel.to.min(hunk.lines.len().saturating_sub(1)))?;
    // A range starting on a removed line followed by new lines is about the new side.
    let side = if range.iter().any(|l| l.kind != gitcore::LineKind::Removed) && range.len() > 1 {
        DiffSide::Right
    } else {
        crate::pr_diff::line_target(range.first()?)?.1
    };
    let picked: Vec<(u32, String)> = range
        .iter()
        .filter_map(|l| {
            let (n, sd) = match (side, l.kind) {
                (DiffSide::Right, gitcore::LineKind::Removed) => return None,
                (DiffSide::Right, _) => (l.new_no?, DiffSide::Right),
                (DiffSide::Left, gitcore::LineKind::Added) => return None,
                (DiffSide::Left, _) => (l.old_no?, DiffSide::Left),
            };
            (sd == side).then(|| (n, l.text.trim_end_matches(['\n', '\r']).to_string()))
        })
        .collect();
    Some(SelectionTarget {
        start: picked.first()?.0,
        end: picked.last()?.0,
        side,
        lines: picked.into_iter().map(|(_, t)| t).collect(),
    })
}

/// Body of a "Suggest change": a ```suggestion block of the selected lines (blank lines
/// included).
pub fn suggestion_prefill(t: &SelectionTarget) -> String {
    github::suggestion_block(&t.lines)
}

/// Like `apply_disabled_reason`, also refusing a fork's pull request: its `pr/N` branch
/// cannot be pushed to the fork from here.
pub fn apply_disabled_reason_for(
    cross_repository: bool,
    head: &str,
    number: u64,
    current_branch: Option<&str>,
    outdated: bool,
    side: DiffSide,
) -> Option<&'static str> {
    if cross_repository {
        return Some(s::WHY_FORK_SUGGESTION);
    }
    apply_disabled_reason(head, number, current_branch, outdated, side)
}

/// Why "Apply suggestion" is disabled: the pull request (head branch `head`, number
/// `number`) must be the checked-out branch, and the thread current on the new side.
pub fn apply_disabled_reason(
    head: &str,
    number: u64,
    current_branch: Option<&str>,
    outdated: bool,
    side: DiffSide,
) -> Option<&'static str> {
    if outdated || side == DiffSide::Left {
        return Some(s::WHY_OUTDATED_SUGGESTION);
    }
    let on_it = current_branch.is_some_and(|b| b == head || b == format!("pr/{number}"));
    (!on_it).then_some(s::WHY_CHECKOUT_FIRST)
}
