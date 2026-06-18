use std::time::Duration;

use super::super::LlmError;

/// Maximum number of retry attempts for LLM calls.
pub const MAX_RETRIES: usize = 15;

/// Maximum backoff cap for rate limit errors (seconds).
/// Rate limit indicates the API is overloaded; give it time to recover.
const RATE_LIMIT_BACKOFF_CAP_SECS: u64 = 120;

/// Maximum backoff cap for network / transient errors (seconds).
/// Network errors are usually momentary; retry quickly.
const TRANSIENT_BACKOFF_CAP_SECS: u64 = 30;

/// Returns `true` for HTTP status codes that indicate a transient server-side condition
/// worth retrying: 5xx and 408 (Request Timeout).
pub(crate) const fn is_transient_server_status(status: u16) -> bool {
    status >= 500 && status <= 599 || status == 408
}

/// Calculates the delay before the next retry attempt based on the error type.
/// Returns `None` if the error is not retryable.
pub fn get_retry_delay(error: &LlmError, attempt: usize) -> Option<Duration> {
    match error {
        LlmError::RateLimit { wait_secs, .. } => {
            if let Some(secs) = wait_secs {
                return Some(Duration::from_secs(*secs + 1));
            }
            let backoff_secs = 10u64 * 2u64.pow((attempt - 1) as u32);
            Some(Duration::from_secs(
                backoff_secs.min(RATE_LIMIT_BACKOFF_CAP_SECS),
            ))
        }
        LlmError::ApiError {
            status: Some(429), ..
        } => {
            let backoff_secs = 10u64 * 2u64.pow((attempt - 1) as u32);
            Some(Duration::from_secs(
                backoff_secs.min(RATE_LIMIT_BACKOFF_CAP_SECS),
            ))
        }
        LlmError::ApiError {
            status: Some(status),
            ..
        } if is_transient_server_status(*status) => Some(capped_transient_backoff(attempt)),
        LlmError::EmptyResponse(_) => Some(capped_transient_backoff(attempt)),
        LlmError::NetworkError(_) => Some(capped_transient_backoff(attempt)),
        LlmError::RequestBuilder(_) => None,
        LlmError::JsonError(_) => Some(capped_transient_backoff(attempt)),
        _ => None,
    }
}

/// Exponential backoff with cap for transient (network/5xx/json) errors.
/// 1s -> 2s -> 4s -> 8s -> 16s -> 30s -> 30s -> ...
fn capped_transient_backoff(attempt: usize) -> Duration {
    const INITIAL_BACKOFF_MS: u64 = 1000;
    let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1) as u32);
    Duration::from_millis(backoff_ms.min(TRANSIENT_BACKOFF_CAP_SECS * 1000))
}

#[must_use]
pub fn is_retryable_error(error: &LlmError) -> bool {
    get_retry_delay(error, 1).is_some()
}

#[must_use]
pub fn is_rate_limit_error(error: &LlmError) -> bool {
    match error {
        LlmError::RateLimit { .. } => true,
        LlmError::ApiError {
            status: Some(429), ..
        } => true,
        LlmError::EmptyResponse(_) => false,
        _ => false,
    }
}

#[must_use]
pub fn get_rate_limit_wait_secs(error: &LlmError) -> Option<u64> {
    match error {
        LlmError::RateLimit { wait_secs, .. } => *wait_secs,
        _ => None,
    }
}

pub(crate) fn get_retry_delay_with_initial(
    error: &LlmError,
    attempt: usize,
    initial_backoff_ms: u64,
) -> Option<Duration> {
    let capped_rate_limit =
        |backoff_ms: u64| Duration::from_millis(backoff_ms.min(RATE_LIMIT_BACKOFF_CAP_SECS * 1000));
    let capped_transient =
        |backoff_ms: u64| Duration::from_millis(backoff_ms.min(TRANSIENT_BACKOFF_CAP_SECS * 1000));
    let backoff = || initial_backoff_ms * 2u64.pow((attempt - 1) as u32);

    match error {
        LlmError::RateLimit { wait_secs, .. } => {
            if let Some(secs) = wait_secs {
                return Some(Duration::from_secs(*secs + 1));
            }
            Some(capped_rate_limit(backoff()))
        }
        LlmError::ApiError {
            status: Some(429), ..
        } => Some(capped_rate_limit(backoff())),
        LlmError::ApiError {
            status: Some(status),
            ..
        } if is_transient_server_status(*status) => Some(capped_transient(backoff())),
        LlmError::EmptyResponse(_) => Some(capped_transient(backoff())),
        LlmError::NetworkError(_) => Some(capped_transient(backoff())),
        LlmError::RequestBuilder(_) => None,
        LlmError::JsonError(_) => Some(capped_transient(backoff())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::get_retry_delay;
    use crate::llm::LlmError;
    use std::time::Duration;

    #[test]
    fn retry_delay_treats_503_service_unavailable_as_retryable() {
        let error = LlmError::api_error_status(503, "LLM API error: service unavailable");
        let delay = get_retry_delay(&error, 2).expect("retry delay");

        assert_eq!(delay, Duration::from_millis(2000));
    }

    #[test]
    fn transient_backoff_caps_at_30s() {
        let error = LlmError::NetworkError("connection timeout".to_string());

        // attempt 6 would be 32s without cap
        let delay = get_retry_delay(&error, 6).expect("retry delay");
        assert_eq!(delay, Duration::from_secs(30));

        // attempt 15 should still be 30s
        let delay = get_retry_delay(&error, 15).expect("retry delay");
        assert_eq!(delay, Duration::from_secs(30));
    }

    #[test]
    fn rate_limit_backoff_caps_at_120s() {
        let error = LlmError::RateLimit {
            wait_secs: None,
            message: "too many requests".to_string(),
        };

        // attempt 5 would be 10*16=160s without cap
        let delay = get_retry_delay(&error, 5).expect("retry delay");
        assert_eq!(delay, Duration::from_secs(120));

        // attempt 15 should still be 120s
        let delay = get_retry_delay(&error, 15).expect("retry delay");
        assert_eq!(delay, Duration::from_secs(120));
    }

    #[test]
    fn rate_limit_respects_server_wait_secs() {
        let error = LlmError::RateLimit {
            wait_secs: Some(60),
            message: "retry after 60s".to_string(),
        };

        let delay = get_retry_delay(&error, 1).expect("retry delay");
        assert_eq!(delay, Duration::from_secs(61)); // wait_secs + 1
    }

    #[test]
    fn transient_backoff_sequence() {
        let error = LlmError::NetworkError("timeout".to_string());

        assert_eq!(
            get_retry_delay(&error, 1).expect("retry delay exists"),
            Duration::from_secs(1)
        );
        assert_eq!(
            get_retry_delay(&error, 2).expect("retry delay exists"),
            Duration::from_secs(2)
        );
        assert_eq!(
            get_retry_delay(&error, 3).expect("retry delay exists"),
            Duration::from_secs(4)
        );
        assert_eq!(
            get_retry_delay(&error, 4).expect("retry delay exists"),
            Duration::from_secs(8)
        );
        assert_eq!(
            get_retry_delay(&error, 5).expect("retry delay exists"),
            Duration::from_secs(16)
        );
        assert_eq!(
            get_retry_delay(&error, 6).expect("retry delay exists"),
            Duration::from_secs(30)
        );
        // capped
    }

    #[test]
    fn empty_response_is_retryable() {
        let error = LlmError::EmptyResponse(" (provider=chatgpt)".to_string());

        let delay = get_retry_delay(&error, 2).expect("retry delay exists");
        assert_eq!(delay, Duration::from_secs(2));
    }
}
