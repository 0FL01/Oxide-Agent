use thiserror::Error;

/// Errors returned by Browser Live sidecar operations.
#[derive(Debug, Error)]
pub enum BrowserSidecarError {
    /// Sidecar base URL is empty or invalid.
    #[error("invalid browser sidecar base URL: {0}")]
    InvalidBaseUrl(String),
    /// Browser sidecar token is required when the client is constructed.
    #[error("browser sidecar token is required")]
    MissingToken,
    /// Idempotency key is required for mutating sidecar requests.
    #[error("browser sidecar idempotency key is required for mutating requests")]
    MissingIdempotencyKey,
    /// Session id cannot be represented safely as one URL path segment.
    #[error("invalid browser sidecar session id")]
    InvalidSessionId,
    /// Sidecar returned a non-success HTTP status without a parseable envelope.
    #[error("browser sidecar returned HTTP {status}: {body}")]
    HttpStatus {
        /// HTTP status code.
        status: reqwest::StatusCode,
        /// Truncated response body for diagnostics.
        body: String,
    },
    /// Sidecar returned a stable failure envelope.
    #[error("browser sidecar API failure {code}: {message}")]
    ApiFailure {
        /// Stable sidecar error code.
        code: String,
        /// Human-readable message from the sidecar.
        message: String,
        /// Whether the sidecar says retry is safe.
        retryable: bool,
        /// Optional recovery hint from the sidecar.
        hint: Option<String>,
    },
    /// Underlying reqwest transport or JSON error.
    #[error("browser sidecar request failed: {0}")]
    Request(#[from] reqwest::Error),
    /// Response body was valid JSON but not the expected sidecar contract.
    #[error("browser sidecar returned invalid response: {0}")]
    InvalidResponse(String),
}

impl BrowserSidecarError {
    /// Classifies whether the operation can be retried without violating the
    /// sidecar contract. Mutating callers must still re-observe before retrying
    /// browser actions whose previous outcome is unknown.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::InvalidBaseUrl(_)
            | Self::MissingToken
            | Self::MissingIdempotencyKey
            | Self::InvalidSessionId
            | Self::InvalidResponse(_) => false,
            Self::HttpStatus { status, .. } => matches!(status.as_u16(), 408 | 429 | 502..=504),
            Self::ApiFailure { retryable, .. } => *retryable,
            Self::Request(err) => is_retryable_reqwest(err),
        }
    }

    /// Stable error-kind string for structured tool payloads and metrics.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl(_) => "browser_sidecar_invalid_base_url",
            Self::MissingToken => "browser_sidecar_missing_token",
            Self::MissingIdempotencyKey => "browser_sidecar_missing_idempotency_key",
            Self::InvalidSessionId => "browser_sidecar_invalid_session_id",
            Self::HttpStatus { status, .. } => match status.as_u16() {
                401 | 403 => "browser_sidecar_auth_failed",
                408 => "browser_sidecar_timeout",
                429 => "browser_sidecar_rate_limited",
                502..=504 => "browser_sidecar_unavailable",
                _ => "browser_sidecar_http_status",
            },
            Self::ApiFailure { code, .. } => stable_code_kind(code),
            Self::Request(err) => {
                if err.is_timeout() {
                    "browser_sidecar_timeout"
                } else if err.is_connect() {
                    "browser_sidecar_connect"
                } else {
                    "browser_sidecar_network"
                }
            }
            Self::InvalidResponse(_) => "browser_sidecar_invalid_response",
        }
    }

    /// Short user-facing diagnostic. It never includes response bodies or tokens.
    #[must_use]
    pub fn agent_message(&self) -> String {
        match self {
            Self::InvalidBaseUrl(_) => "Browser sidecar URL is invalid".to_string(),
            Self::MissingToken => "Browser sidecar token is not configured".to_string(),
            Self::MissingIdempotencyKey => "Browser sidecar idempotency key is missing".to_string(),
            Self::InvalidSessionId => "Browser session id is invalid".to_string(),
            Self::HttpStatus { status, .. } => status_agent_message(*status),
            Self::ApiFailure { message, hint, .. } => hint
                .as_ref()
                .filter(|value| !value.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| message.clone()),
            Self::Request(err) => {
                if err.is_timeout() || err.is_connect() {
                    "Browser sidecar is temporarily unavailable".to_string()
                } else if err.is_decode() {
                    "Browser sidecar returned invalid response format".to_string()
                } else {
                    "Browser sidecar request failed".to_string()
                }
            }
            Self::InvalidResponse(_) => "Browser sidecar returned invalid response".to_string(),
        }
    }
}

fn stable_code_kind(code: &str) -> &'static str {
    match code {
        "timeout" => "browser_sidecar_timeout",
        "not_found" => "browser_sidecar_not_found",
        "invalid_action" => "browser_sidecar_invalid_action",
        "policy_denied" => "browser_sidecar_policy_denied",
        "browser_crashed" => "browser_sidecar_browser_crashed",
        "cdp_error" => "browser_sidecar_cdp_error",
        "stale_session" => "browser_sidecar_stale_session",
        "rate_limited" => "browser_sidecar_rate_limited",
        "sidecar_at_capacity" => "browser_sidecar_at_capacity",
        _ => "browser_sidecar_api_failure",
    }
}

fn status_agent_message(status: reqwest::StatusCode) -> String {
    if status.as_u16() == 401 || status.as_u16() == 403 {
        "Browser sidecar authentication failed".to_string()
    } else if status.as_u16() == 429 || status.is_server_error() {
        "Browser sidecar is temporarily unavailable".to_string()
    } else {
        "Browser sidecar request was rejected".to_string()
    }
}

fn is_retryable_reqwest(err: &reqwest::Error) -> bool {
    if err.is_builder() {
        return false;
    }
    if err.is_timeout() || err.is_connect() {
        return true;
    }
    let msg = err.to_string().to_lowercase();
    msg.contains("connection reset")
        || msg.contains("connection refused")
        || msg.contains("broken pipe")
        || msg.contains("eof")
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn retry_classification_uses_status_and_envelope() {
        let rate_limited = BrowserSidecarError::HttpStatus {
            status: StatusCode::TOO_MANY_REQUESTS,
            body: String::new(),
        };
        assert!(rate_limited.is_retryable());
        assert_eq!(rate_limited.kind(), "browser_sidecar_rate_limited");

        let auth = BrowserSidecarError::HttpStatus {
            status: StatusCode::UNAUTHORIZED,
            body: String::new(),
        };
        assert!(!auth.is_retryable());
        assert_eq!(auth.kind(), "browser_sidecar_auth_failed");

        let api = BrowserSidecarError::ApiFailure {
            code: "browser_crashed".to_string(),
            message: "browser crashed".to_string(),
            retryable: true,
            hint: Some("restart session".to_string()),
        };
        assert!(api.is_retryable());
        assert_eq!(api.kind(), "browser_sidecar_browser_crashed");
        assert_eq!(api.agent_message(), "restart session");
    }

    #[test]
    fn local_config_errors_are_not_retryable() {
        for error in [
            BrowserSidecarError::MissingToken,
            BrowserSidecarError::MissingIdempotencyKey,
            BrowserSidecarError::InvalidSessionId,
        ] {
            assert!(!error.is_retryable());
        }
    }
}
