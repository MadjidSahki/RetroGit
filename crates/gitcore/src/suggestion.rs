//! Applying a pull request suggestion: replace lines of a file and commit.

use crate::{CommitBackend, GitError, Repo};

/// Pure: `content` with lines `start..=end` (1-based) replaced by `replacement`, if they are
/// still `expected`. Every other line keeps its own ending; new lines take the ending of
/// the lines they replace (the last one keeps the last replaced line's ending, so a file
/// without a final newline stays so).
pub fn replace_lines(
    content: &str,
    start: usize,
    end: usize,
    expected: &[String],
    replacement: &str,
) -> Option<String> {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    if start == 0 || end < start || end > lines.len() {
        return None;
    }
    let body = |l: &str| l.trim_end_matches(['\n', '\r']).to_string();
    let current = &lines[start - 1..end];
    let same = current.len() == expected.len()
        && current
            .iter()
            .zip(expected)
            .all(|(a, b)| body(a) == b.trim_end_matches(['\n', '\r']));
    if !same {
        return None;
    }
    let ending = |l: &str| l[l.trim_end_matches(['\n', '\r']).len()..].to_string();
    let first_eol = {
        let e = ending(current[0]);
        if e.is_empty() { "\n".to_string() } else { e }
    };
    let last_eol = ending(current[current.len() - 1]);
    let new: Vec<String> = replacement
        .split_inclusive('\n')
        .map(|l| l.trim_end_matches(['\n', '\r']).to_string())
        .collect();
    let mut out = String::with_capacity(content.len() + replacement.len());
    for l in &lines[..start - 1] {
        out.push_str(l);
    }
    for (i, l) in new.iter().enumerate() {
        out.push_str(l);
        out.push_str(if i + 1 == new.len() {
            &last_eol
        } else {
            &first_eol
        });
    }
    for l in &lines[end..] {
        out.push_str(l);
    }
    Some(out)
}

impl Repo {
    /// Replace lines `start..=end` of `path` by `replacement` and commit with `message`
    /// (through `git`: hooks and signing apply). Refused when the lines are no longer
    /// `expected` (`SuggestionOutdated`), or when there are local changes (the commit must
    /// hold the suggestion only). A refused commit puts the file back.
    pub fn apply_suggestion(
        &self,
        path: &str,
        start: usize,
        end: usize,
        expected: &[String],
        replacement: &str,
        message: &str,
    ) -> Result<(), GitError> {
        let changes = self.git_ok(&["--literal-pathspecs", "status", "--porcelain", "-uno"])?;
        if !changes.stdout.trim().is_empty() {
            return Err(GitError::Unsupported(
                "commit or stash your local changes first".into(),
            ));
        }
        let file = self.workdir()?.join(path);
        let content = std::fs::read_to_string(&file)
            .map_err(|e| GitError::Other(format!("cannot read {path}: {e}")))?;
        let new = replace_lines(&content, start, end, expected, replacement)
            .ok_or(GitError::SuggestionOutdated)?;
        std::fs::write(&file, &new)
            .map_err(|e| GitError::Other(format!("cannot write {path}: {e}")))?;
        let committed = self
            .git_ok(&["--literal-pathspecs", "add", "--", path])
            .and_then(|_| {
                self.commit(message, false, CommitBackend::PreferCli)
                    .map(|_| ())
            });
        if let Err(e) = committed {
            let _ = self.run_git(&["--literal-pathspecs", "reset", "-q", "--", path]);
            let _ = std::fs::write(&file, &content);
            return Err(e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::replace_lines;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn lines_are_replaced_only_if_unchanged() {
        assert_eq!(
            replace_lines("a\nb\nc\n", 2, 2, &s(&["b"]), "x\ny\n").as_deref(),
            Some("a\nx\ny\nc\n")
        );
        assert_eq!(replace_lines("a\nb\n", 2, 2, &s(&["B"]), "x\n"), None);
        assert_eq!(replace_lines("a\n", 0, 1, &s(&["a"]), "x\n"), None);
        assert_eq!(
            replace_lines("a\r\nb", 2, 2, &s(&["b"]), "c\n").as_deref(),
            Some("a\r\nc")
        );
        assert_eq!(
            replace_lines("a\nb\n", 1, 2, &s(&["a", "b"]), "").as_deref(),
            Some("")
        );
    }
}
