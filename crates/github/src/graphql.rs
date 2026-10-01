//! GitHub GraphQL API (`POST /graphql`): used for reads that need many related objects.

use serde_json::{Value, json};

use crate::GithubError;
use crate::client::{Client, check};
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
        let body: Value = resp.body_mut().read_json()?;
        let errors: Vec<GithubError> = body["errors"]
            .as_array()
            .into_iter()
            .flatten()
            .map(typed_error)
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

fn typed_error(e: &Value) -> GithubError {
    let message = e["message"].as_str().unwrap_or("unknown error");
    if let Some(r) = oauth_restriction(message) {
        return r;
    }
    if e["type"].as_str() == Some("RATE_LIMITED") {
        return GithubError::RateLimited;
    }
    GithubError::Graphql(message.to_string())
}
