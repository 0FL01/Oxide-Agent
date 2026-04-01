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
        LlmError::ApiError(msg) => {
            let msg_lower = msg.to_lowercase();
            if msg_lower.contains("429") {
                let backoff_secs = 10u64 * 2u64.pow((attempt - 1) as u32);
                return Some(Duration::from_secs(
                    backoff_secs.min(RATE_LIMIT_BACKOFF_CAP_SECS),
                ));
            }

            if msg_lower.contains("500")
                || msg_lower.contains("internal server error")
                || msg_lower.contains("502")
                || msg_lower.contains("bad gateway")
                || msg_lower.contains("503")
                || msg_lower.contains("service unavailable")
                || msg_lower.contains("504")
                || msg_lower.contains("gateway timeout")
                || msg_lower.contains("temporarily unavailable")
                || msg_lower.contains("timeout")
                || msg_lower.contains("overloaded")
            {
                return Some(capped_transient_backoff(attempt));
            }

            None
        }
        LlmError::NetworkError(msg) => {
            if msg.to_lowercase().contains("builder") {
                return None;
            }
            Some(capped_transient_backoff(attempt))
        }
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
        LlmError::ApiError(msg) => msg.to_lowercase().contains("429"),
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
    match error {
        LlmError::RateLimit { wait_secs, .. } => {
            if let Some(secs) = wait_secs {
                return Some(Duration::from_secs(*secs + 1));
            }
            let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
            Some(Duration::from_millis(
                backoff_ms.min(RATE_LIMIT_BACKOFF_CAP_SECS * 1000),
            ))
        }
        LlmError::ApiError(msg) => {
            let msg_lower = msg.to_lowercase();
            if msg_lower.contains("429") {
                let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
                return Some(Duration::from_millis(
                    backoff_ms.min(RATE_LIMIT_BACKOFF_CAP_SECS * 1000),
                ));
            }
            if msg_lower.contains("500")
                || msg_lower.contains("502")
                || msg_lower.contains("503")
                || msg_lower.contains("504")
                || msg_lower.contains("timeout")
                || msg_lower.contains("overloaded")
            {
                let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
                return Some(Duration::from_millis(
                    backoff_ms.min(TRANSIENT_BACKOFF_CAP_SECS * 1000),
                ));
            }
            None
        }
        LlmError::NetworkError(msg) => {
            let msg_lower = msg.to_lowercase();
            if msg_lower.contains("dns")
                || msg_lower.contains("refused")
                || msg_lower.contains("reset")
            {
                let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
                return Some(Duration::from_millis(
                    backoff_ms.min(TRANSIENT_BACKOFF_CAP_SECS * 1000),
                ));
            }
            let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
            Some(Duration::from_millis(
                backoff_ms.min(TRANSIENT_BACKOFF_CAP_SECS * 1000),
            ))
        }
        LlmError::JsonError(_) => {
            let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
            Some(Duration::from_millis(
                backoff_ms.min(TRANSIENT_BACKOFF_CAP_SECS * 1000),
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::get_retry_delay;
    use crate::llm::LlmError;
    use std::time::Duration;

    #[test]
    fn retry_delay_treats_service_unavailable_text_as_retryable() {
        let error = LlmError::ApiError("NVIDIA NIM API error: service unavailable".to_string());
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
}
