//! Unauthorized access flood protection mechanism
//!
//! This module provides a cache-based cooldown system to prevent
//! flooding Telegram with "Access Denied" messages, which could
//! result in bot rate limiting or ban.

use moka::future::Cache;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;

/// Cache for tracking the last "Access Denied" response time to users
///
/// This cache implements a silent cooldown mechanism where unauthorized users
/// receive "Access Denied" messages only once per cooldown period, while all
/// attempts are still logged (with throttling).
#[derive(Clone)]
pub struct UnauthorizedCache {
    /// Moka cache storing user_id -> () mappings with automatic TTL
    cache: Cache<i64, ()>,
    /// Cooldown duration between messages to the same user
    cooldown: Duration,
    /// Counter for silenced attempts (for logging throttling)
    silenced_count: Arc<AtomicU64>,
}

impl UnauthorizedCache {
    /// Creates a new `UnauthorizedCache` with specified parameters
    ///
    /// # Arguments
    ///
    /// * `cooldown_secs` - Seconds between "Access Denied" messages to same user
    /// * `ttl_secs` - Time-to-live for cache entries (auto-cleanup)
    /// * `max_capacity` - Maximum number of entries in cache
    ///
    /// # Examples
    ///
    /// ```
    /// use oxide_agent::bot::UnauthorizedCache;
    ///
    /// let cache = UnauthorizedCache::new(
    ///     1200,   // 20 minutes cooldown
    ///     7200,   // 2 hours TTL
    ///     10_000  // max 10k entries
    /// );
    /// ```
    #[must_use]
    pub fn new(cooldown_secs: u64, ttl_secs: u64, max_capacity: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(Duration::from_secs(ttl_secs))
            .build();

        Self {
            cache,
            cooldown: Duration::from_secs(cooldown_secs),
            silenced_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Checks if an "Access Denied" message should be sent to the user
    ///
    /// Returns `true` if the cooldown period has passed or this is the first attempt.
    /// Returns `false` if the user is still in cooldown period.
    ///
    /// This method also implements log throttling: only every 100th silenced
    /// attempt is logged to prevent log flooding.
    ///
    /// # Arguments
    ///
    /// * `user_id` - Telegram user ID
    /// * `user_name` - User's display name (for debug logging)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use oxide_agent::bot::UnauthorizedCache;
    /// # async fn example() {
    /// let cache = UnauthorizedCache::new(1200, 7200, 10_000);
    ///
    /// if cache.should_send(12345, "John").await {
    ///     // Send "Access Denied" message
    /// }
    /// # }
    /// ```
    pub async fn should_send(&self, user_id: i64, user_name: &str) -> bool {
        // If key is not in cache, we should send
        if self.cache.get(&user_id).await.is_none() {
            return true;
        }

        // User is in cooldown, increment silenced counter
        let count = self.silenced_count.fetch_add(1, Ordering::Relaxed) + 1;

        // Log only every 100th silenced attempt to prevent log flooding
        if count.is_multiple_of(100) {
            debug!(
                "⛔️ Silenced {} unauthorized attempts (recent: user {} - {})",
                count, user_id, user_name
            );
        }

        false
    }

    /// Marks that an "Access Denied" message was successfully sent to the user
    ///
    /// This inserts the user ID into the cache, starting the cooldown period.
    ///
    /// # Arguments
    ///
    /// * `user_id` - Telegram user ID
    pub async fn mark_sent(&self, user_id: i64) {
        self.cache.insert(user_id, ()).await;
    }

    /// Returns the current number of entries in the cache
    ///
    /// Useful for monitoring and health checks.
    #[must_use]
    pub fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }

    /// Returns the total number of silenced unauthorized attempts
    ///
    /// Useful for monitoring and statistics.
    #[must_use]
    pub fn silenced_count(&self) -> u64 {
        self.silenced_count.load(Ordering::Relaxed)
    }

    /// Returns the configured cooldown duration
    ///
    /// Useful for displaying in statistics.
    #[must_use]
    pub fn cooldown(&self) -> Duration {
        self.cooldown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_first_attempt_should_send() {
        let cache = UnauthorizedCache::new(60, 120, 100);

        // First attempt should always return true
        assert!(cache.should_send(12345, "TestUser").await);
    }

    #[tokio::test]
    async fn test_cooldown_blocks_second_attempt() {
        let cache = UnauthorizedCache::new(60, 120, 100);

        // First attempt
        assert!(cache.should_send(12345, "TestUser").await);
        cache.mark_sent(12345).await;

        // Immediate second attempt should be blocked
        assert!(!cache.should_send(12345, "TestUser").await);
    }

    #[tokio::test]
    async fn test_different_users_independent() {
        let cache = UnauthorizedCache::new(60, 120, 100);

        assert!(cache.should_send(111, "User1").await);
        cache.mark_sent(111).await;

        // Different user should not be affected
        assert!(cache.should_send(222, "User2").await);
    }

    #[tokio::test]
    async fn test_silenced_count_increments() {
        let cache = UnauthorizedCache::new(60, 120, 100);

        cache.mark_sent(12345).await;

        // Multiple blocked attempts
        for _ in 0..5 {
            cache.should_send(12345, "TestUser").await;
        }

        assert_eq!(cache.silenced_count(), 5);
    }

    #[tokio::test]
    async fn test_entry_count() {
        let cache = UnauthorizedCache::new(60, 120, 100);

        cache.mark_sent(111).await;
        cache.mark_sent(222).await;

        // Manually run pending tasks to update the entry count
        cache.cache.run_pending_tasks().await;

        assert_eq!(cache.entry_count(), 2);
    }
}
