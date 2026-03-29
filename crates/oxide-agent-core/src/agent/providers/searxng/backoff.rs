//! Retry backoff for SearXNG requests.
//!
//! Search should be fast — aggressive backoff caps keep total latency bounded.

/// Maximum number of retry attempts (total requests = MAX_RETRIES + 1).
pub const MAX_RETRIES: usize = 3;

/// Initial backoff delay in milliseconds.
const INITIAL_BACKOFF_MS: u64 = 500;

/// Maximum backoff cap in seconds.
const BACKOFF_CAP_SECS: u64 = 10;

/// Jitter range as a fraction (±20%).
const JITTER_FRACTION: f64 = 0.2;

/// Returns the delay before the next retry attempt, or `None` if not retryable.
///
/// Uses exponential backoff with jitter: `base * 2^(attempt-1) ± 20%`, capped at 10 s.
#[must_use]
pub fn retry_delay(attempt: usize) -> std::time::Duration {
    let base_ms = INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1) as u32);
    let capped_ms = base_ms.min(BACKOFF_CAP_SECS * 1000);

    let jitter_ms = (capped_ms as f64 * JITTER_FRACTION) as u64;
    let jitter = if jitter_ms == 0 {
        0
    } else {
        fastrand::u64(..=jitter_ms * 2).saturating_sub(jitter_ms)
    };

    std::time::Duration::from_millis(capped_ms.saturating_add(jitter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn first_retry_is_around_500ms() {
        let delay = retry_delay(1);
        // 500ms ± 20% → [400, 600] ms
        assert!(delay >= Duration::from_millis(400));
        assert!(delay <= Duration::from_millis(600));
    }

    #[test]
    fn second_retry_is_around_1s() {
        let delay = retry_delay(2);
        // 1000ms ± 20% → [800, 1200] ms
        assert!(delay >= Duration::from_millis(800));
        assert!(delay <= Duration::from_millis(1200));
    }

    #[test]
    fn fourth_retry_caps_at_10s() {
        let delay = retry_delay(4);
        // 4000ms < 10s cap, so [3200, 4800] ms
        assert!(delay >= Duration::from_millis(3200));
        assert!(delay <= Duration::from_millis(4800));
    }

    #[test]
    fn high_attempt_stays_under_cap() {
        let delay = retry_delay(10);
        // 500*512 = 256s → capped to 10s ± 20% → [8000, 12000] ms
        assert!(delay >= Duration::from_millis(8000));
        assert!(delay <= Duration::from_millis(12000));
    }
}
