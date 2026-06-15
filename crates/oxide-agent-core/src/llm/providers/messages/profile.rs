//! Provider-specific profile data for Anthropic-compatible Messages paths.

#![allow(dead_code)]

use super::ANTHROPIC_VERSION;

/// How a Messages endpoint is derived from provider configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessagesEndpointPolicy {
    /// Append `/v1/messages` to a base provider URL.
    AppendV1Messages,
    /// Use the configured value as the exact Messages endpoint.
    UseConfiguredUrlAsExactEndpoint,
}

/// Authentication/header behavior for a Messages-compatible endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessagesAuthPolicy {
    /// Anthropic-style `x-api-key` plus `anthropic-version` headers.
    XApiKey,
    /// OpenCode-compatible Bearer auth plus Anthropic-compatible headers.
    BearerAndXApiKey,
}

/// Thinking block policy for request construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessagesThinkingPolicy {
    /// Caller controls whether a `thinking` block is emitted.
    CallerProvided,
    /// Enable thinking for known reasoning models unless disabled by caller.
    OpenCodeReasoningModels,
}

/// Usage accounting policy for response parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessagesUsagePolicy {
    /// Anthropic-compatible input/output/cache token accounting.
    AnthropicCacheFields,
}

/// Profile for provider-specific behavior in shared Messages helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MessagesProfile {
    /// Provider label for error messages.
    pub label: &'static str,
    /// Default endpoint or base URL for this profile.
    pub default_endpoint: &'static str,
    /// Endpoint derivation policy.
    pub endpoint_policy: MessagesEndpointPolicy,
    /// Auth/header policy.
    pub auth_policy: MessagesAuthPolicy,
    /// Prefix for generated fallback tool IDs when the provider returns empty IDs.
    /// `None` means empty IDs are kept as-is.
    pub empty_tool_id_fallback_prefix: Option<&'static str>,
    /// Thinking request policy.
    pub thinking_policy: MessagesThinkingPolicy,
    /// Usage accounting policy.
    pub usage_policy: MessagesUsagePolicy,
}

impl MessagesProfile {
    /// Profile for generic Anthropic Messages API provider.
    pub const fn anthropic() -> Self {
        Self {
            label: "Anthropic",
            default_endpoint: "https://api.anthropic.com",
            endpoint_policy: MessagesEndpointPolicy::AppendV1Messages,
            auth_policy: MessagesAuthPolicy::XApiKey,
            empty_tool_id_fallback_prefix: Some("anthropic_fallback_"),
            thinking_policy: MessagesThinkingPolicy::CallerProvided,
            usage_policy: MessagesUsagePolicy::AnthropicCacheFields,
        }
    }

    /// Profile for OpenCode Go/Zen Anthropic Messages branch.
    pub const fn opencode_go() -> Self {
        Self {
            label: "OpenCode Go",
            default_endpoint: "https://opencode.ai/zen/go/v1/messages",
            endpoint_policy: MessagesEndpointPolicy::UseConfiguredUrlAsExactEndpoint,
            auth_policy: MessagesAuthPolicy::BearerAndXApiKey,
            empty_tool_id_fallback_prefix: Some("opencode_go_tool_use_"),
            thinking_policy: MessagesThinkingPolicy::OpenCodeReasoningModels,
            usage_policy: MessagesUsagePolicy::AnthropicCacheFields,
        }
    }

    /// Build the exact Messages endpoint for a configured base or endpoint.
    pub(crate) fn endpoint_for(self, configured: &str) -> String {
        let trimmed = configured.trim().trim_end_matches('/');
        match self.endpoint_policy {
            MessagesEndpointPolicy::UseConfiguredUrlAsExactEndpoint => trimmed.to_string(),
            MessagesEndpointPolicy::AppendV1Messages => format!("{trimmed}/v1/messages"),
        }
    }

    /// Build profile-specific `Authorization` header, if any.
    pub(crate) fn auth_header(self, api_key: &str) -> Option<String> {
        match self.auth_policy {
            MessagesAuthPolicy::XApiKey => None,
            MessagesAuthPolicy::BearerAndXApiKey => Some(format!("Bearer {}", api_key.trim())),
        }
    }

    /// Build profile-specific extra headers.
    pub(crate) fn extra_headers<'a>(self, api_key: &'a str) -> Vec<(&'static str, &'a str)> {
        vec![
            ("anthropic-version", ANTHROPIC_VERSION),
            ("x-api-key", api_key),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_profile_preserves_anthropic_headers_and_endpoint_policy() {
        let profile = MessagesProfile::anthropic();

        assert_eq!(profile.label, "Anthropic");
        assert_eq!(
            profile.endpoint_for("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(profile.auth_header(" key "), None);
        assert_eq!(
            profile.extra_headers("key"),
            vec![
                ("anthropic-version", ANTHROPIC_VERSION),
                ("x-api-key", "key")
            ]
        );
        assert_eq!(
            profile.empty_tool_id_fallback_prefix,
            Some("anthropic_fallback_")
        );
    }

    #[test]
    fn messages_profile_preserves_opencode_bearer_and_fallback_policy() {
        let profile = MessagesProfile::opencode_go();

        assert_eq!(
            profile.endpoint_policy,
            MessagesEndpointPolicy::UseConfiguredUrlAsExactEndpoint
        );
        assert_eq!(
            profile.auth_header(" token ").as_deref(),
            Some("Bearer token")
        );
        assert_eq!(
            profile.empty_tool_id_fallback_prefix,
            Some("opencode_go_tool_use_")
        );
        assert_eq!(
            profile.thinking_policy,
            MessagesThinkingPolicy::OpenCodeReasoningModels
        );
    }
}
