use thiserror::Error;

/// Errors returned by CRW scrape operations.
#[derive(Debug, Error)]
pub enum CrwError {
    /// CRW scrape requires an explicit token; absence means the backend is not configured.
    #[error("CRW API token is required")]
    MissingApiToken,
    /// URL was empty or invalid.
    #[error("invalid URL")]
    InvalidUrl,
    /// CRW returned a non-success HTTP status.
    #[error("CRW returned HTTP {status}: {body}")]
    HttpStatus {
        /// HTTP status code.
        status: reqwest::StatusCode,
        /// Truncated response body for diagnostics.
        body: String,
    },
    /// CRW returned a JSON failure envelope with HTTP success status.
    #[error("CRW API failure: {message}")]
    ApiFailure {
        /// Provider-supplied error message, truncated by the client.
        message: String,
    },
    /// Underlying reqwest transport error.
    #[error("CRW request failed: {0}")]
    Request(#[from] reqwest::Error),
}

impl CrwError {
    /// Returns a short, agent-friendly error message for the scrape endpoint.
    #[must_use]
    pub fn scrape_agent_message(&self) -> String {
        match self {
            Self::MissingApiToken => "CRW API token is not configured".to_string(),
            Self::InvalidUrl => "Invalid URL".to_string(),
            Self::HttpStatus { status, body } => {
                let code = status.as_u16();
                if code == 401 || code == 403 {
                    "CRW authentication error".to_string()
                } else if code == 502 || code == 503 || code == 504 {
                    if body.contains("error sending request") {
                        "CRW could not reach the target page — the site may be blocking the renderer or is unreachable".to_string()
                    } else {
                        "CRW renderer failed — the target page may be blocking JavaScript rendering"
                            .to_string()
                    }
                } else if status.is_client_error() {
                    "CRW configuration error".to_string()
                } else {
                    "CRW temporarily unavailable, please try again in a moment".to_string()
                }
            }
            Self::ApiFailure { message } => {
                if is_auth_message(message) {
                    "CRW authentication error".to_string()
                } else {
                    "CRW provider returned an error".to_string()
                }
            }
            Self::Request(err) => {
                if err.is_timeout() {
                    "CRW render request timed out — the target page may be too slow or blocking the renderer".to_string()
                } else if err.is_connect() {
                    "CRW is not reachable, please try again in a moment".to_string()
                } else if err.is_decode() {
                    "CRW request failed (invalid provider response format)".to_string()
                } else {
                    "CRW request failed (transport error)".to_string()
                }
            }
        }
    }

    /// Short error-kind string for structured payload metadata.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::MissingApiToken => "crw_not_configured",
            Self::InvalidUrl => "invalid_url",
            Self::HttpStatus { status, .. } => {
                let code = status.as_u16();
                match code {
                    401 | 403 => "crw_auth_failed",
                    408 => "crw_timeout",
                    429 => "crw_rate_limited",
                    502..=504 => "crw_unavailable",
                    _ => "crw_http_status",
                }
            }
            Self::ApiFailure { message } => {
                if is_auth_message(message) {
                    "crw_auth_failed"
                } else {
                    "crw_api_failure"
                }
            }
            Self::Request(err) => {
                if err.is_timeout() {
                    "crw_timeout"
                } else if err.is_connect() {
                    "crw_connect"
                } else {
                    "crw_network"
                }
            }
        }
    }
}

fn is_auth_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("auth")
        || lower.contains("api key")
        || lower.contains("token")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn http_403_is_auth_failed() {
        let err = CrwError::HttpStatus {
            status: StatusCode::FORBIDDEN,
            body: String::new(),
        };
        assert_eq!(err.kind(), "crw_auth_failed");
    }

    #[test]
    fn scrape_503_with_transport_body_indicates_blocked_renderer() {
        let err = CrwError::HttpStatus {
            status: StatusCode::SERVICE_UNAVAILABLE,
            body: "HTTP request failed: error sending request for url (https://www.hp.com/...)"
                .to_string(),
        };
        let msg = err.scrape_agent_message();
        assert!(msg.contains("could not reach the target page"));
    }

    #[test]
    fn scrape_503_without_transport_body_indicates_blocked_renderer() {
        let err = CrwError::HttpStatus {
            status: StatusCode::SERVICE_UNAVAILABLE,
            body: String::new(),
        };
        let msg = err.scrape_agent_message();
        assert!(msg.contains("renderer failed"));
    }
}
