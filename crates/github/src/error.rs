#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum GithubError {
    #[error("GitHub rejected the token (401)")]
    Unauthorized,
    #[error("this organization requires SSO authorization for the token: {url}")]
    SsoRequired { url: String },
    #[error("GitHub API rate limit exceeded")]
    RateLimited,
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected HTTP status {0}")]
    Http(u16),
    #[error("could not decode GitHub response: {0}")]
    Decode(String),
}

impl From<ureq::Error> for GithubError {
    fn from(e: ureq::Error) -> Self {
        match e {
            ureq::Error::StatusCode(s) => GithubError::Http(s),
            ureq::Error::Json(e) => GithubError::Decode(e.to_string()),
            other => GithubError::Network(other.to_string()),
        }
    }
}
