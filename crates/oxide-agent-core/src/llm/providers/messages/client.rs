//! Thin HTTP client wrapper for Anthropic-compatible Messages requests.

#![allow(dead_code)]

use serde_json::Value;

use super::MessagesProfile;
use super::response;
use crate::llm::support::http::send_json_request;
use crate::llm::{ChatResponse, LlmError};

/// Shared client state for Messages-compatible providers.
#[derive(Debug, Clone)]
pub(crate) struct MessagesClient {
    http_client: reqwest::Client,
    endpoint: String,
    api_key: String,
    profile: MessagesProfile,
}

impl MessagesClient {
    /// Create a Messages client from an exact endpoint.
    pub(crate) fn new(
        http_client: reqwest::Client,
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        profile: MessagesProfile,
    ) -> Self {
        Self {
            http_client,
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            profile,
        }
    }

    /// Create a Messages client from a provider base URL using the profile policy.
    pub(crate) fn from_base_url(
        http_client: reqwest::Client,
        base_url: &str,
        api_key: impl Into<String>,
        profile: MessagesProfile,
    ) -> Self {
        Self::new(
            http_client,
            profile.endpoint_for(base_url),
            api_key,
            profile,
        )
    }

    /// Exact Messages endpoint.
    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Profile attached to this client.
    pub(crate) fn profile(&self) -> MessagesProfile {
        self.profile
    }

    /// Underlying HTTP client.
    pub(crate) fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }

    /// API key used for this client.
    pub(crate) fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Send a JSON request and return the parsed JSON response.
    pub(crate) async fn post_json(&self, body: &Value) -> Result<Value, LlmError> {
        let auth = self.profile.auth_header(&self.api_key);
        let extra_headers = self.profile.extra_headers(&self.api_key);
        send_json_request(
            &self.http_client,
            &self.endpoint,
            body,
            auth.as_deref(),
            &extra_headers,
        )
        .await
    }

    /// Send a JSON request and parse a Messages response.
    pub(crate) async fn send_and_parse(&self, body: &Value) -> Result<ChatResponse, LlmError> {
        let response = self.post_json(body).await?;
        response::parse_response(response, self.profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::support::http::create_http_client;

    #[test]
    fn messages_client_keeps_endpoint_key_and_profile() {
        let client = MessagesClient::from_base_url(
            create_http_client(),
            "https://api.anthropic.com",
            " key ",
            MessagesProfile::anthropic(),
        );

        assert_eq!(client.endpoint(), "https://api.anthropic.com/v1/messages");
        assert_eq!(client.api_key(), " key ");
        assert_eq!(client.profile(), MessagesProfile::anthropic());
        assert!(client.profile().auth_header(client.api_key()).is_none());
    }
}
