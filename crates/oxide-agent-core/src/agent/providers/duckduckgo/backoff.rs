use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct BackoffConfig {
    pub initial: Duration,
    pub max: Duration,
}

#[must_use]
pub fn retry_delay(attempt: usize, config: BackoffConfig) -> Duration {
    let multiplier = 2u32.saturating_pow(attempt.saturating_sub(1) as u32);
    let initial_ms = u64::try_from(config.initial.as_millis()).unwrap_or(u64::MAX);
    let max_ms = u64::try_from(config.max.as_millis()).unwrap_or(u64::MAX);
    let base_ms = initial_ms.saturating_mul(u64::from(multiplier));
    let capped_ms = base_ms.min(max_ms);
    let jitter_ms = capped_ms / 5;
    let jitter = if jitter_ms == 0 {
        0
    } else {
        fastrand::u64(..=jitter_ms.saturating_mul(2)).saturating_sub(jitter_ms)
    };

    Duration::from_millis(capped_ms.saturating_add(jitter))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_applies_exponential_backoff_with_jitter_bounds() {
        let config = BackoffConfig {
            initial: Duration::from_millis(1_000),
            max: Duration::from_secs(30),
        };

        let delay = retry_delay(2, config);

        assert!(delay >= Duration::from_millis(1_600));
        assert!(delay <= Duration::from_millis(2_400));
    }

    #[test]
    fn retry_delay_caps_at_max_with_jitter() {
        let config = BackoffConfig {
            initial: Duration::from_secs(10),
            max: Duration::from_secs(30),
        };

        let delay = retry_delay(10, config);

        assert!(delay >= Duration::from_secs(24));
        assert!(delay <= Duration::from_secs(36));
    }
}
