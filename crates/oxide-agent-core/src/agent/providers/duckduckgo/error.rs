use thiserror::Error;

#[derive(Debug, Error)]
pub enum DuckDuckGoError {
    #[error("search query cannot be empty")]
    EmptyQuery,
    #[error("DuckDuckGo is temporarily rate-limited")]
    RateLimited,
    #[error("DuckDuckGo request timed out")]
    Timeout,
    #[error("DuckDuckGo client initialization failed: {0}")]
    ClientInit(String),
    #[error("DuckDuckGo request failed: {0}")]
    Request(String),
}

impl DuckDuckGoError {
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::EmptyQuery | Self::ClientInit(_) | Self::RateLimited => false,
            Self::Timeout => true,
            Self::Request(message) => is_retryable_message(message),
        }
    }

    #[must_use]
    pub fn should_cooldown(&self) -> bool {
        match self {
            Self::RateLimited | Self::Timeout => true,
            Self::Request(message) => is_retryable_message(message) || is_block_message(message),
            Self::EmptyQuery | Self::ClientInit(_) => false,
        }
    }

    #[must_use]
    pub fn agent_message(&self) -> String {
        match self {
            Self::EmptyQuery => "DuckDuckGo search query cannot be empty".to_string(),
            Self::RateLimited => concat!(
                "DuckDuckGo is temporarily rate-limited; retry later or use existing results. ",
                "Use web_markdown only for already selected URLs."
            )
            .to_string(),
            Self::Timeout => {
                "DuckDuckGo temporarily unavailable, please try again in a moment".to_string()
            }
            Self::ClientInit(_) => "DuckDuckGo search configuration error".to_string(),
            Self::Request(message) if is_block_message(message) => concat!(
                "DuckDuckGo is temporarily blocking or rate-limiting requests; ",
                "retry later or use existing results."
            )
            .to_string(),
            Self::Request(_) => "DuckDuckGo search request failed".to_string(),
        }
    }
}

fn is_retryable_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("429")
        || message.contains("too many requests")
        || message.contains("timeout")
        || message.contains("timed out")
        || message.contains("connection reset")
        || message.contains("connection refused")
        || message.contains("broken pipe")
        || message.contains("eof")
        || message.contains("502")
        || message.contains("503")
        || message.contains("504")
}

fn is_block_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("403")
        || message.contains("forbidden")
        || message.contains("captcha")
        || message.contains("blocked")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_messages_include_rate_limits_and_resets() {
        assert!(DuckDuckGoError::Request("HTTP 429".to_string()).is_retryable());
        assert!(DuckDuckGoError::Request("connection reset".to_string()).is_retryable());
        assert!(!DuckDuckGoError::Request("bad input".to_string()).is_retryable());
    }
}
