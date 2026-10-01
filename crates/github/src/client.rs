use std::time::Duration;

use serde::Deserialize;
use ureq::http::Response;
use ureq::tls::{RootCerts, TlsConfig};
use ureq::{Agent, Body};

use crate::device_flow::{DeviceCode, PollResponse};
use crate::{GithubError, next_link};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct User {
    pub login: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    /// `owner/name`
    pub full_name: String,
    pub name: String,
    pub owner: String,
    pub private: bool,
    /// HTTPS clone URL (no credentials).
    pub clone_url: String,
    /// ISO-8601 timestamp, e.g. `2026-09-30T10:00:00Z`.
    pub updated_at: String,
}

/// Result of listing repositories.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoListing {
    pub repos: Vec<RepoInfo>,
    /// IDs of organizations whose repos GitHub left out because the token is not
    /// SSO-authorized for them (`X-GitHub-SSO: partial-results; organizations=...`).
    pub sso_hidden_orgs: Vec<String>,
}

#[derive(Deserialize)]
struct RawRepo {
    full_name: String,
    name: String,
    owner: RawOwner,
    private: bool,
    clone_url: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct RawOwner {
    login: String,
}

impl From<RawRepo> for RepoInfo {
    fn from(r: RawRepo) -> Self {
        RepoInfo {
            full_name: r.full_name,
            name: r.name,
            owner: r.owner.login,
            private: r.private,
            clone_url: r.clone_url,
            updated_at: r.updated_at,
        }
    }
}

/// Stateless GitHub client: the token is passed to each call.
#[derive(Clone)]
pub struct Client {
    pub(crate) agent: Agent,
    pub(crate) api_base: String,
    pub(crate) web_base: String,
}

const USER_AGENT: &str = concat!("RetroGit/", env!("CARGO_PKG_VERSION"));
const REPOS_PATH: &str =
    "/user/repos?affiliation=owner,collaborator,organization_member&sort=updated&per_page=100";

impl Client {
    /// Client for github.com.
    pub fn github_com() -> Client {
        Client::with_bases("https://api.github.com", "https://github.com")
    }

    /// Client with custom base URLs (tests use a local mock server).
    pub fn with_bases(api_base: &str, web_base: &str) -> Client {
        let agent: Agent = Agent::config_builder()
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(60)))
            // OS trust store: works behind corporate TLS inspection proxies.
            .tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .build()
            .into();
        Client {
            agent,
            api_base: api_base.trim_end_matches('/').to_string(),
            web_base: web_base.trim_end_matches('/').to_string(),
        }
    }

    pub fn request_device_code(
        &self,
        client_id: &str,
        scopes: &[&str],
    ) -> Result<DeviceCode, GithubError> {
        let scope = scopes.join(" ");
        let resp = self
            .agent
            .post(format!("{}/login/device/code", self.web_base))
            .header("Accept", "application/json")
            .send_form([("client_id", client_id), ("scope", scope.as_str())])?;
        let mut resp = check(resp)?;
        Ok(resp.body_mut().read_json::<DeviceCode>()?)
    }

    pub fn poll_token(
        &self,
        client_id: &str,
        device_code: &str,
    ) -> Result<PollResponse, GithubError> {
        let resp = self
            .agent
            .post(format!("{}/login/oauth/access_token", self.web_base))
            .header("Accept", "application/json")
            .send_form([
                ("client_id", client_id),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])?;
        let mut resp = check(resp)?;
        let v: serde_json::Value = resp.body_mut().read_json()?;
        Ok(PollResponse::from_json(&v))
    }

    pub fn current_user(&self, token: &str) -> Result<User, GithubError> {
        let mut resp = self.api_get(&format!("{}/user", self.api_base), token)?;
        Ok(resp.body_mut().read_json::<User>()?)
    }

    /// All repositories the user can access, following pagination.
    pub fn list_repos(&self, token: &str) -> Result<RepoListing, GithubError> {
        let mut url = format!("{}{}", self.api_base, REPOS_PATH);
        let mut out = RepoListing::default();
        loop {
            let mut resp = self.api_get(&url, token)?;
            let next = header(&resp, "link").and_then(next_link);
            for org in header(&resp, "x-github-sso")
                .map(partial_sso_orgs)
                .unwrap_or_default()
            {
                if !out.sso_hidden_orgs.contains(&org) {
                    out.sso_hidden_orgs.push(org);
                }
            }
            let page: Vec<RawRepo> = resp.body_mut().read_json()?;
            out.repos.extend(page.into_iter().map(RepoInfo::from));
            match next {
                Some(n) => url = n,
                None => return Ok(out),
            }
        }
    }

    pub(crate) fn api_get(&self, url: &str, token: &str) -> Result<Response<Body>, GithubError> {
        let resp = self
            .agent
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("Authorization", format!("Bearer {token}"))
            .call()?;
        check(resp)
    }
}

pub(crate) fn header<'a>(resp: &'a Response<Body>, name: &str) -> Option<&'a str> {
    resp.headers().get(name).and_then(|v| v.to_str().ok())
}

/// `partial-results; organizations=1,2` => `["1", "2"]`.
fn partial_sso_orgs(value: &str) -> Vec<String> {
    if !value.trim_start().starts_with("partial-results") {
        return Vec::new();
    }
    value
        .split("organizations=")
        .nth(1)
        .map(|ids| {
            ids.split(',')
                .map(|i| i.trim().to_string())
                .filter(|i| !i.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Map non-2xx statuses to typed errors.
pub(crate) fn check(resp: Response<Body>) -> Result<Response<Body>, GithubError> {
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(resp);
    }
    if status == 401 {
        return Err(GithubError::Unauthorized);
    }
    if status == 403
        && let Some(sso) = header(&resp, "x-github-sso")
    {
        let url = sso.split("url=").nth(1).unwrap_or("").trim().to_string();
        return Err(GithubError::SsoRequired { url });
    }
    if (status == 403 || status == 429) && header(&resp, "x-ratelimit-remaining") == Some("0") {
        return Err(GithubError::RateLimited);
    }
    if status == 403 {
        let text = resp.into_body().read_to_string().unwrap_or_default();
        if let Some(e) = crate::error::oauth_restriction(&text) {
            return Err(e);
        }
    }
    Err(GithubError::Http(status))
}
