#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum GithubError {
    #[error("GitHub rejected the token (401)")]
    Unauthorized,
    #[error("this organization requires SSO authorization for the token: {url}")]
    SsoRequired { url: String },
    /// The organization restricts OAuth App access and has not approved RetroGit.
    #[error("the organization {} restricts third-party application access", org.as_deref().unwrap_or("?"))]
    OAuthRestricted { org: Option<String> },
    #[error("GitHub API rate limit exceeded")]
    RateLimited,
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected HTTP status {0}")]
    Http(u16),
    /// GitHub refused a change and said why (validation failed, not mergeable...).
    #[error("{message}")]
    Rejected { status: u16, message: String },
    #[error("GitHub GraphQL error: {0}")]
    Graphql(String),
    /// GitHub says the object does not exist. Organizations that restrict OAuth Apps make
    /// their repositories look missing to the restricted token.
    #[error("not found: {0}")]
    NotFound(String),
    #[error("could not decode GitHub response: {0}")]
    Decode(String),
    /// People were added, then removing `not_removed` failed (GitHub's `reason`).
    #[error("{reason}")]
    PeopleHalf {
        not_removed: Vec<String>,
        reason: String,
    },
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

/// GitHub's "this repository does not exist" (what a restricted OAuth App sees), as opposed
/// to another missing object (pull request, comment).
pub fn repository_missing(e: &GithubError) -> bool {
    matches!(e, GithubError::NotFound(m) if m.starts_with("Could not resolve to a Repository"))
}

/// Marker GitHub puts in REST and GraphQL messages when an organization blocks the app.
const OAUTH_RESTRICTED: &str = "OAuth App access restrictions";

/// `OAuthRestricted` if `message` is GitHub's "organization restricts OAuth Apps" text.
/// The organization is quoted in backticks: "... the `ExampleOrg` organization has ...".
pub(crate) fn oauth_restriction(message: &str) -> Option<GithubError> {
    if !message.contains(OAUTH_RESTRICTED) {
        return None;
    }
    let org = message
        .split_once("the `")
        .and_then(|(_, rest)| rest.split_once("` organization"))
        .map(|(org, _)| org.to_string());
    Some(GithubError::OAuthRestricted { org })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_restricting_organization() {
        let m = "Although you appear to have the correct authorization credentials, the \
                 `ExampleOrg` organization has enabled OAuth App access restrictions, meaning \
                 that data access to third-parties is limited.";
        assert_eq!(
            oauth_restriction(m),
            Some(GithubError::OAuthRestricted {
                org: Some("ExampleOrg".into())
            })
        );
        assert_eq!(
            oauth_restriction("enabled OAuth App access restrictions"),
            Some(GithubError::OAuthRestricted { org: None })
        );
        assert_eq!(oauth_restriction("Resource not accessible"), None);
    }
}
