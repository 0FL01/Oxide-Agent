use std::time::Duration;

use super::super::LlmError;

/// Maximum number of retry attempts for LLM calls.
pub const MAX_RETRIES: usize = 5;

/// Calculates the delay before the next retry attempt based on the error type.
/// Returns `None` if the error is not retryable.
pub fn get_retry_delay(error: &LlmError, attempt: usize) -> Option<Duration> {
    const INITIAL_BACKOFF_MS: u64 = 1000;

    match error {
        LlmError::RateLimit { wait_secs, .. } => {
            if let Some(secs) = wait_secs {
                return Some(Duration::from_secs(*secs + 1));
            }
            let backoff_secs = 10u64 * 2u64.pow((attempt - 1) as u32);
            Some(Duration::from_secs(backoff_secs))
        }
        LlmError::ApiError(msg) => {
            let msg_lower = msg.to_lowercase();
            if msg_lower.contains("429") {
                let backoff_secs = 10u64 * 2u64.pow((attempt - 1) as u32);
                return Some(Duration::from_secs(backoff_secs));
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
                let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1) as u32);
                return Some(Duration::from_millis(backoff_ms));
            }

            None
        }
        LlmError::NetworkError(msg) => {
            if msg.to_lowercase().contains("builder") {
                return None;
            }
            let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1) as u32);
            Some(Duration::from_millis(backoff_ms))
        }
        LlmError::JsonError(_) => {
            let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1) as u32);
            Some(Duration::from_millis(backoff_ms))
        }
        _ => None,
    }
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
            Some(Duration::from_millis(backoff_ms))
        }
        LlmError::ApiError(msg) => {
            let msg_lower = msg.to_lowercase();
            if msg_lower.contains("429") {
                let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
                return Some(Duration::from_millis(backoff_ms));
            }
            if msg_lower.contains("500")
                || msg_lower.contains("502")
                || msg_lower.contains("503")
                || msg_lower.contains("504")
                || msg_lower.contains("timeout")
                || msg_lower.contains("overloaded")
            {
                let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
                return Some(Duration::from_millis(backoff_ms));
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
                return Some(Duration::from_millis(backoff_ms));
            }
            let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
            Some(Duration::from_millis(backoff_ms))
        }
        LlmError::JsonError(_) => {
            let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
            Some(Duration::from_millis(backoff_ms))
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
}
