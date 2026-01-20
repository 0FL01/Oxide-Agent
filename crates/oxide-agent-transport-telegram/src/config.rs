//! Telegram transport settings.

use config::ConfigError;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Telegram transport settings loaded from environment variables.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct TelegramSettings {
    /// Telegram Bot API token.
    pub telegram_token: String,
    /// Comma-separated list of allowed user IDs for normal chat.
    #[serde(rename = "allowed_users")]
    pub allowed_users_str: Option<String>,
    /// Comma-separated list of allowed user IDs for agent mode.
    #[serde(rename = "agent_access_ids")]
    pub agent_allowed_users_str: Option<String>,
}

/// Combined settings used by the Telegram transport layer.
#[derive(Clone)]
pub struct BotSettings {
    /// Agent settings shared across transport handlers.
    pub agent: Arc<oxide_agent_core::config::AgentSettings>,
    /// Telegram-specific settings.
    pub telegram: Arc<TelegramSettings>,
}

impl BotSettings {
    /// Create a new combined settings bundle.
    #[must_use]
    pub fn new(
        agent: oxide_agent_core::config::AgentSettings,
        telegram: TelegramSettings,
    ) -> Self {
        Self {
            agent: Arc::new(agent),
            telegram: Arc::new(telegram),
        }
    }
}

impl TelegramSettings {
    /// Create new settings by loading from environment and files.
    ///
    /// # Errors
    ///
    /// Returns a `ConfigError` if loading fails.
    pub fn new() -> Result<Self, ConfigError> {
        oxide_agent_core::config::build_config()?.try_deserialize()
    }

    /// Returns a set of allowed user IDs for normal chat.
    #[must_use]
    pub fn allowed_users(&self) -> HashSet<i64> {
        self.allowed_users_str
            .as_ref()
            .map(|s| {
                s.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
                    .filter(|token| !token.is_empty())
                    .filter_map(|id| id.parse::<i64>().ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns a set of allowed user IDs for agent mode.
    #[must_use]
    pub fn agent_allowed_users(&self) -> HashSet<i64> {
        self.agent_allowed_users_str
            .as_ref()
            .map(|s| {
                s.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
                    .filter(|token| !token.is_empty())
                    .filter_map(|id| id.parse::<i64>().ok())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Cooldown period (seconds) between "Access Denied" messages for same user.
/// Default: 20 minutes.
pub const UNAUTHORIZED_COOLDOWN_SECS: u64 = 1200;
/// Time-to-live (seconds) for cache entries.
/// Default: 2 hours.
pub const UNAUTHORIZED_CACHE_TTL_SECS: u64 = 7200;
/// Maximum cache capacity (number of entries).
pub const UNAUTHORIZED_CACHE_MAX_SIZE: u64 = 10_000;

/// Get unauthorized cooldown from env or default.
///
/// Environment variable: `UNAUTHORIZED_COOLDOWN_SECS`.
#[must_use]
pub fn get_unauthorized_cooldown() -> u64 {
    std::env::var("UNAUTHORIZED_COOLDOWN_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(UNAUTHORIZED_COOLDOWN_SECS)
}

/// Get unauthorized cache TTL from env or default.
///
/// Environment variable: `UNAUTHORIZED_CACHE_TTL_SECS`.
#[must_use]
pub fn get_unauthorized_cache_ttl() -> u64 {
    std::env::var("UNAUTHORIZED_CACHE_TTL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(UNAUTHORIZED_CACHE_TTL_SECS)
}

/// Get unauthorized cache max size from env or default.
///
/// Environment variable: `UNAUTHORIZED_CACHE_MAX_SIZE`.
#[must_use]
pub fn get_unauthorized_cache_max_size() -> u64 {
    std::env::var("UNAUTHORIZED_CACHE_MAX_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(UNAUTHORIZED_CACHE_MAX_SIZE)
}

#[cfg(test)]
mod tests {
    use super::TelegramSettings;

    #[test]
    fn test_list_parsing() {
        let mut settings = TelegramSettings {
            telegram_token: "dummy".to_string(),
            allowed_users_str: None,
            agent_allowed_users_str: None,
        };

        // Test comma
        settings.allowed_users_str = Some("123,456".to_string());
        let allowed = settings.allowed_users();
        assert!(allowed.contains(&123));
        assert!(allowed.contains(&456));
        assert_eq!(allowed.len(), 2);

        // Test space
        settings.allowed_users_str = Some("111 222".to_string());
        let allowed = settings.allowed_users();
        assert!(allowed.contains(&111));
        assert!(allowed.contains(&222));
        assert_eq!(allowed.len(), 2);

        // Test semicolon and mixed
        settings.allowed_users_str = Some("333; 444, 555".to_string());
        let allowed = settings.allowed_users();
        assert!(allowed.contains(&333));
        assert!(allowed.contains(&444));
        assert!(allowed.contains(&555));
        assert_eq!(allowed.len(), 3);

        // Test empty/bad parsing
        settings.allowed_users_str = Some("abc, 777".to_string());
        let allowed = settings.allowed_users();
        assert!(allowed.contains(&777));
        assert_eq!(allowed.len(), 1);
    }
}
