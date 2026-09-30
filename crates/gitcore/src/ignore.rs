use crate::{GitError, Repo};

impl Repo {
    /// Append `pattern` to the root `.gitignore` (created if needed, no duplicates).
    pub fn add_to_gitignore(&self, pattern: &str) -> Result<(), GitError> {
        let pattern = pattern.trim();
        if pattern.is_empty() || pattern.contains('\n') {
            return Err(GitError::Unsupported("invalid .gitignore pattern".into()));
        }
        let path = self.workdir()?.join(".gitignore");
        let io = |e: std::io::Error| GitError::Other(format!("cannot update .gitignore: {e}"));
        let mut text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(io(e)),
        };
        if text.lines().any(|l| l.trim() == pattern) {
            return Ok(());
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(pattern);
        text.push('\n');
        std::fs::write(&path, text).map_err(io)
    }
}
