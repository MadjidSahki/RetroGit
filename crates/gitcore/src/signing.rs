use crate::{GitError, Repo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningFormat {
    Gpg,
    Ssh,
    X509,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningConfig {
    /// `commit.gpgsign`: commits made with the git CLI will be signed.
    pub enabled: bool,
    pub format: SigningFormat,
    /// `user.signingkey`, if set.
    pub key: Option<String>,
}

impl Repo {
    /// Effective signing configuration (system, global, `includeIf` and repository config).
    pub fn signing_config(&self) -> Result<SigningConfig, GitError> {
        let cfg = self
            .git()
            .config()
            .and_then(|mut c| c.snapshot())
            .map_err(|e| GitError::from_git2(&e))?;
        let format = match cfg.get_string("gpg.format").ok().as_deref() {
            Some("ssh") => SigningFormat::Ssh,
            Some("x509") => SigningFormat::X509,
            _ => SigningFormat::Gpg,
        };
        Ok(SigningConfig {
            enabled: cfg.get_bool("commit.gpgsign").unwrap_or(false),
            format,
            key: cfg
                .get_string("user.signingkey")
                .ok()
                .filter(|k| !k.is_empty()),
        })
    }
}
