//! GitHub GraphQL API (`POST /graphql`): used for reads that need many related objects.

use serde_json::{Value, json};

use crate::GithubError;
use crate::client::{Client, check, header};
use crate::error::oauth_restriction;

/// Data of a GraphQL response, and the errors GitHub returned alongside it.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphqlResponse {
    pub data: Value,
    pub errors: Vec<GithubError>,
}

impl Client {
    /// Run `query` with `variables`. Any error in the response fails the call (typed:
    /// OAuth restriction, rate limit, otherwise `Graphql(message)`).
    pub fn graphql(
        &self,
        token: &str,
        query: &str,
        variables: Value,
    ) -> Result<Value, GithubError> {
        let r = self.graphql_partial(token, query, variables)?;
        match r.errors.into_iter().next() {
            Some(e) => Err(e),
            None => Ok(r.data),
        }
    }

    /// Like `graphql`, but returns the data even when some parts failed (searches across
    /// organizations, some of which may refuse the token). Fails only without data.
    pub fn graphql_partial(
        &self,
        token: &str,
        query: &str,
        variables: Value,
    ) -> Result<GraphqlResponse, GithubError> {
        let resp = self
            .agent
            .post(format!("{}/graphql", self.api_base))
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {token}"))
            .send_json(json!({ "query": query, "variables": variables }))?;
        let mut resp = check(resp)?;
        // GraphQL answers 200 even when SSO is missing; the header carries the link.
        let sso_url = header(&resp, "x-github-sso")
            .and_then(|h| h.split("url=").nth(1))
            .map(|u| u.trim().to_string());
        let body: Value = resp.body_mut().read_json()?;
        let errors: Vec<GithubError> = body["errors"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| typed_error(e, sso_url.as_deref()))
            .collect();
        let data = body.get("data").cloned().unwrap_or(Value::Null);
        if data.is_null() {
            return Err(errors
                .into_iter()
                .next()
                .unwrap_or_else(|| GithubError::Decode("no data in GraphQL response".into())));
        }
        Ok(GraphqlResponse { data, errors })
    }
}

fn typed_error(e: &Value, sso_url: Option<&str>) -> GithubError {
    let message = e["message"].as_str().unwrap_or("unknown error");
    if let Some(r) = oauth_restriction(message) {
        return r;
    }
    if message.contains("SAML enforcement") {
        return GithubError::SsoRequired {
            url: sso_url.unwrap_or_default().to_string(),
        };
    }
    if e["type"].as_str() == Some("NOT_FOUND") {
        return GithubError::NotFound(message.to_string());
    }
    if e["type"].as_str() == Some("RATE_LIMITED") {
        return GithubError::RateLimited;
    }
    GithubError::Graphql(message.to_string())
}
