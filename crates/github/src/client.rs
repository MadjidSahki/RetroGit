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
    /// Signed-in accounts that can see it (filled by the app when merging lists).
    pub accounts: Vec<String>,
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
            accounts: Vec::new(),
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

    /// Logins of the organizations the token's user belongs to (first 100).
    pub fn user_orgs(&self, token: &str) -> Result<Vec<String>, GithubError> {
        let mut resp = self.api_get(&format!("{}/user/orgs?per_page=100", self.api_base), token)?;
        let orgs: Vec<RawOwner> = resp.body_mut().read_json()?;
        Ok(orgs.into_iter().map(|o| o.login).collect())
    }

    /// `Ok` if the token can see `owner/repo` (`NotFound` otherwise, as GitHub says).
    pub fn check_repo(&self, token: &str, owner: &str, repo: &str) -> Result<(), GithubError> {
        let data = self.graphql(
            token,
            "query($owner: String!, $name: String!) { repository(owner: $owner, name: $name) { id } }",
            serde_json::json!({ "owner": owner, "name": repo }),
        )?;
        if data["repository"].is_null() {
            return Err(GithubError::NotFound(format!(
                "Could not resolve to a Repository with the name '{owner}/{repo}'."
            )));
        }
        Ok(())
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

    /// `POST` / `PUT` / `PATCH` / `DELETE` of a JSON body to `path` (relative to the API base).
    /// Refusals that GitHub explains (403, 404, 405, 409, 422) become `Rejected { message }`.
    pub(crate) fn api_send(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<Response<Body>, GithubError> {
        let url = format!("{}{path}", self.api_base);
        let auth = format!("Bearer {token}");
        let with_headers = |b: ureq::RequestBuilder<ureq::typestate::WithBody>| {
            b.header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .header("Authorization", auth.as_str())
        };
        let json = body.cloned().unwrap_or(serde_json::Value::Null);
        let resp = match method {
            "POST" => with_headers(self.agent.post(&url)).send_json(&json)?,
            "PUT" => with_headers(self.agent.put(&url)).send_json(&json)?,
            "PATCH" => with_headers(self.agent.patch(&url)).send_json(&json)?,
            _ if body.is_some() => {
                with_headers(self.agent.delete(&url).force_send_body()).send_json(&json)?
            }
            _ => self
                .agent
                .delete(&url)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .header("Authorization", auth.as_str())
                .call()?,
        };
        check_write(resp)
    }
}

/// Like `check`, but a refusal GitHub explains keeps its message.
fn check_write(resp: Response<Body>) -> Result<Response<Body>, GithubError> {
    let status = resp.status().as_u16();
    if (200..300).contains(&status)
        || status == 401
        || status == 429
        || header(&resp, "x-github-sso").is_some()
        || header(&resp, "x-ratelimit-remaining") == Some("0")
    {
        return check(resp);
    }
    let text = resp.into_body().read_to_string().unwrap_or_default();
    if let Some(e) = crate::error::oauth_restriction(&text) {
        return Err(e);
    }
    let message = api_message(&text);
    if matches!(status, 403 | 404 | 405 | 409 | 422) && !message.is_empty() {
        Err(GithubError::Rejected { status, message })
    } else {
        Err(GithubError::Http(status))
    }
}

/// GitHub's explanation in an error body: `message`, plus the validation errors' messages.
pub(crate) fn api_message(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    if let Some(m) = v["message"].as_str() {
        parts.push(m.to_string());
    }
    for e in v["errors"].as_array().into_iter().flatten() {
        if let Some(m) = e["message"].as_str().or_else(|| e.as_str()) {
            parts.push(m.to_string());
        }
    }
    parts.join(": ")
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
