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
    #[error("DuckDuckGo returned a CAPTCHA or block page: {0}")]
    Blocked(String),
    #[error("DuckDuckGo parser could not recognize the response: {0}")]
    ParserBreak(String),
    #[error("DuckDuckGo request failed: {0}")]
    Request(String),
}

impl DuckDuckGoError {
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::EmptyQuery
            | Self::ClientInit(_)
            | Self::RateLimited
            | Self::Blocked(_)
            | Self::ParserBreak(_) => false,
            Self::Timeout => true,
            Self::Request(message) => is_retryable_message(message),
        }
    }

    #[must_use]
    pub fn should_cooldown(&self) -> bool {
        match self {
            Self::RateLimited | Self::Timeout | Self::Blocked(_) => true,
            Self::Request(message) => is_retryable_message(message) || is_block_message(message),
            Self::EmptyQuery | Self::ClientInit(_) | Self::ParserBreak(_) => false,
        }
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::EmptyQuery => "empty_query",
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
            Self::ClientInit(_) => "client_init",
            Self::Blocked(_) => "blocked",
            Self::ParserBreak(_) => "parser_break",
            Self::Request(_) => "request",
        }
    }

    #[must_use]
    pub fn agent_message(&self) -> String {
        match self {
            Self::EmptyQuery => "DuckDuckGo search query cannot be empty".to_string(),
            Self::RateLimited => concat!(
                "DuckDuckGo is temporarily rate-limited; retry later or use existing results. ",
                "Do not call duckduckgo_search again in this task with rewritten queries. ",
                "Use web_markdown only for already selected URLs or another available source."
            )
            .to_string(),
            Self::Timeout => {
                "DuckDuckGo temporarily unavailable, please try again in a moment".to_string()
            }
            Self::ClientInit(_) => "DuckDuckGo search configuration error".to_string(),
            Self::Blocked(_) => concat!(
                "DuckDuckGo is temporarily blocking or rate-limiting requests; ",
                "do not call duckduckgo_search again in this task with rewritten queries. ",
                "Use existing results or another available source."
            )
            .to_string(),
            Self::Request(message) if is_block_message(message) => concat!(
                "DuckDuckGo is temporarily blocking or rate-limiting requests; ",
                "do not call duckduckgo_search again in this task with rewritten queries. ",
                "Use existing results or another available source."
            )
            .to_string(),
            Self::ParserBreak(_) => concat!(
                "DuckDuckGo response format changed or a block page was not recognized; ",
                "search result parsing failed."
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
