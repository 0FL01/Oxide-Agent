//! Client shell for the shared Chat Completions wire path.

use reqwest::Client as HttpClient;
use serde_json::Value;

use super::profile::ChatCompletionsProfile;
use super::response as chat_response;
use crate::llm::LlmError;
use crate::llm::support::http::send_json_request;

#[derive(Debug, Clone)]
pub(crate) struct ChatCompletionsClientConfig {
    pub(crate) http_client: HttpClient,
    pub(crate) endpoint: String,
    pub(crate) api_key: Option<String>,
    pub(crate) profile: ChatCompletionsProfile,
}

#[derive(Debug, Clone)]
pub(crate) struct ChatCompletionsClient {
    http_client: HttpClient,
    endpoint: String,
    api_key: Option<String>,
    profile: ChatCompletionsProfile,
}

impl ChatCompletionsClient {
    #[must_use]
    pub(crate) fn new(
        http_client: HttpClient,
        endpoint: impl Into<String>,
        api_key: Option<String>,
        profile: ChatCompletionsProfile,
    ) -> Self {
        Self::from_config(ChatCompletionsClientConfig {
            http_client,
            endpoint: endpoint.into(),
            api_key,
            profile,
        })
    }

    #[must_use]
    pub(crate) fn from_config(config: ChatCompletionsClientConfig) -> Self {
        Self {
            http_client: config.http_client,
            endpoint: config.endpoint,
            api_key: config.api_key,
            profile: config.profile,
        }
    }

    #[must_use]
    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    #[must_use]
    pub(crate) fn profile(&self) -> ChatCompletionsProfile {
        self.profile
    }

    #[must_use]
    pub(crate) fn auth_header(&self) -> Option<String> {
        self.api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(|key| format!("Bearer {key}"))
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn extra_headers(&self) -> &'static [(&'static str, &'static str)] {
        self.profile.extra_headers
    }

    pub(crate) async fn post_json(&self, body: &Value) -> Result<Value, LlmError> {
        let auth = self.auth_header();
        send_json_request(
            &self.http_client,
            &self.endpoint,
            body,
            auth.as_deref(),
            self.profile.extra_headers,
        )
        .await
        .map_err(|error| apply_profile_rate_limit_wait(error, self.profile))
    }

    #[must_use]
    pub(crate) fn http_client(&self) -> &HttpClient {
        &self.http_client
    }
}

fn apply_profile_rate_limit_wait(error: LlmError, profile: ChatCompletionsProfile) -> LlmError {
    match error {
        LlmError::RateLimit { wait_secs, message } => LlmError::RateLimit {
            wait_secs: chat_response::parse_rate_limit_wait_secs(profile, &message, wait_secs),
            message,
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_shell_keeps_endpoint_profile_and_bearer_auth() {
        let client = ChatCompletionsClient::new(
            crate::llm::support::http::create_http_client(),
            "https://example.test/v1/chat/completions",
            Some(" token ".to_string()),
            ChatCompletionsProfile::generic(),
        );

        assert_eq!(
            client.endpoint(),
            "https://example.test/v1/chat/completions"
        );
        assert_eq!(client.profile().label, "generic");
        assert_eq!(client.auth_header().as_deref(), Some("Bearer token"));
        assert_eq!(client.extra_headers(), &[]);
    }
}
