use super::error::DuckDuckGoError;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

static DUCKDUCKGO_LIMITER: OnceLock<Arc<DuckDuckGoRateLimiter>> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct DuckDuckGoRateLimitConfig {
    pub max_concurrent: usize,
    pub min_delay: Duration,
    pub jitter: Duration,
    pub cooldown: Duration,
}

impl DuckDuckGoRateLimitConfig {
    #[must_use]
    pub fn normalized(self) -> Self {
        Self {
            max_concurrent: self.max_concurrent.max(1),
            min_delay: self.min_delay,
            jitter: self.jitter,
            cooldown: self.cooldown,
        }
    }
}

#[derive(Debug)]
pub struct DuckDuckGoRateLimiter {
    gate: Arc<Semaphore>,
    state: Mutex<LimiterState>,
    min_delay: Duration,
    jitter: Duration,
    cooldown: Duration,
}

#[derive(Debug, Default)]
struct LimiterState {
    last_request_at: Option<Instant>,
    cooldown_until: Option<Instant>,
}

impl DuckDuckGoRateLimiter {
    #[must_use]
    pub fn global(config: DuckDuckGoRateLimitConfig) -> Arc<Self> {
        Arc::clone(DUCKDUCKGO_LIMITER.get_or_init(|| Arc::new(Self::new(config))))
    }

    #[must_use]
    pub fn new(config: DuckDuckGoRateLimitConfig) -> Self {
        let config = config.normalized();
        Self {
            gate: Arc::new(Semaphore::new(config.max_concurrent)),
            state: Mutex::new(LimiterState::default()),
            min_delay: config.min_delay,
            jitter: config.jitter,
            cooldown: config.cooldown,
        }
    }

    pub async fn acquire(&self) -> Result<OwnedSemaphorePermit, DuckDuckGoError> {
        let permit = Arc::clone(&self.gate)
            .acquire_owned()
            .await
            .map_err(|_| DuckDuckGoError::RateLimited)?;

        let delay = {
            let mut state = self.state.lock().await;
            let now = Instant::now();
            if state.cooldown_until.is_some_and(|until| until > now) {
                return Err(DuckDuckGoError::RateLimited);
            }

            let spacing = self.min_delay.saturating_add(random_jitter(self.jitter));
            let delay = state
                .last_request_at
                .and_then(|last| last.checked_add(spacing))
                .and_then(|next| next.checked_duration_since(now))
                .unwrap_or_default();
            state.last_request_at = Some(now.checked_add(delay).unwrap_or(now));
            delay
        };

        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }

        Ok(permit)
    }

    pub async fn mark_cooldown(&self) {
        let mut state = self.state.lock().await;
        state.cooldown_until = Some(
            Instant::now()
                .checked_add(self.cooldown)
                .unwrap_or_else(Instant::now),
        );
    }
}

fn random_jitter(jitter: Duration) -> Duration {
    let jitter_ms = u64::try_from(jitter.as_millis()).unwrap_or(u64::MAX);
    if jitter_ms == 0 {
        Duration::ZERO
    } else {
        Duration::from_millis(fastrand::u64(..=jitter_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::timeout;

    #[tokio::test(start_paused = true)]
    async fn limiter_rejects_during_cooldown() {
        let limiter = DuckDuckGoRateLimiter::new(DuckDuckGoRateLimitConfig {
            max_concurrent: 1,
            min_delay: Duration::ZERO,
            jitter: Duration::ZERO,
            cooldown: Duration::from_secs(30),
        });

        limiter.mark_cooldown().await;

        let result = limiter.acquire().await;
        assert!(matches!(result, Err(DuckDuckGoError::RateLimited)));
    }

    #[tokio::test(start_paused = true)]
    async fn limiter_serializes_concurrent_requests() {
        let limiter = Arc::new(DuckDuckGoRateLimiter::new(DuckDuckGoRateLimitConfig {
            max_concurrent: 1,
            min_delay: Duration::from_millis(10),
            jitter: Duration::ZERO,
            cooldown: Duration::from_secs(30),
        }));

        let first = limiter.acquire().await.expect("first permit");
        let second_limiter = Arc::clone(&limiter);
        let second = tokio::spawn(async move { second_limiter.acquire().await });

        assert!(timeout(Duration::from_millis(1), second).await.is_err());
        drop(first);
    }
}
