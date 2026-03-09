//! Telegram transport settings.

use config::ConfigError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

fn default_topic_enabled() -> bool {
    true
}

/// Telegram per-topic configuration.
#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TelegramTopicSettings {
    /// Telegram chat identifier.
    pub chat_id: i64,
    /// Telegram thread/topic identifier in forum chats.
    #[serde(default)]
    pub thread_id: Option<i32>,
    /// Agent profile id to use for this topic.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Enables topic-level override routing.
    #[serde(default = "default_topic_enabled")]
    pub enabled: bool,
    /// Require explicit bot mention in this topic.
    #[serde(default)]
    pub require_mention: bool,
    /// Skills whitelist for this topic.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Optional topic-level system prompt.
    #[serde(default)]
    pub system_prompt: Option<String>,
}

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
    /// Per-topic overrides loaded from structured config.
    #[serde(default, rename = "topicConfigs", alias = "topic_configs")]
    pub topic_configs: Vec<TelegramTopicSettings>,
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
    pub fn new(agent: oxide_agent_core::config::AgentSettings, telegram: TelegramSettings) -> Self {
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

    /// Resolves per-topic settings by chat and thread identifiers.
    #[must_use]
    pub fn resolve_topic_config(
        &self,
        chat_id: i64,
        thread_id: Option<i32>,
    ) -> Option<&TelegramTopicSettings> {
        self.topic_configs
            .iter()
            .find(|cfg| cfg.chat_id == chat_id && cfg.thread_id == thread_id)
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
    use config::{Config, File, FileFormat};

    #[test]
    fn test_list_parsing() {
        let mut settings = TelegramSettings {
            telegram_token: "dummy".to_string(),
            allowed_users_str: None,
            agent_allowed_users_str: None,
            topic_configs: Vec::new(),
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

    #[test]
    fn deserializes_topic_configs_with_camel_case_keys() {
        let raw = r#"
        {
          "telegram_token": "dummy",
          "topicConfigs": [
            {
              "chatId": -10001,
              "threadId": 42,
              "agentId": "support-agent",
              "enabled": true,
              "requireMention": true,
              "skills": ["faq", "billing"],
              "systemPrompt": "Use support tone"
            },
            {
              "chatId": -10001,
              "agentId": "fallback-agent"
            },
            {
              "chatId": -10001,
              "threadId": 77
            }
          ]
        }
        "#;

        let loaded = Config::builder()
            .add_source(File::from_str(raw, FileFormat::Json))
            .build();
        let cfg = match loaded {
            Ok(config) => config.try_deserialize::<TelegramSettings>(),
            Err(err) => panic!("failed to build config: {err}"),
        };
        let settings = match cfg {
            Ok(settings) => settings,
            Err(err) => panic!("failed to deserialize settings: {err}"),
        };

        assert_eq!(settings.topic_configs.len(), 3);

        let first = &settings.topic_configs[0];
        assert_eq!(first.chat_id, -10001);
        assert_eq!(first.thread_id, Some(42));
        assert_eq!(first.agent_id.as_deref(), Some("support-agent"));
        assert!(first.enabled);
        assert!(first.require_mention);
        assert_eq!(first.skills, vec!["faq", "billing"]);
        assert_eq!(first.system_prompt.as_deref(), Some("Use support tone"));

        let second = &settings.topic_configs[1];
        assert_eq!(second.chat_id, -10001);
        assert_eq!(second.thread_id, None);
        assert_eq!(second.agent_id.as_deref(), Some("fallback-agent"));
        assert!(second.enabled);
        assert!(!second.require_mention);
        assert!(second.skills.is_empty());
        assert_eq!(second.system_prompt, None);

        let third = &settings.topic_configs[2];
        assert_eq!(third.chat_id, -10001);
        assert_eq!(third.thread_id, Some(77));
        assert_eq!(third.agent_id, None);
        assert!(third.enabled);
        assert!(!third.require_mention);
        assert!(third.skills.is_empty());
        assert_eq!(third.system_prompt, None);
    }

    #[test]
    fn resolves_topic_config_by_chat_and_thread() {
        let raw = r#"
        {
          "telegram_token": "dummy",
          "topicConfigs": [
            {"chatId": -10001, "threadId": 10, "agentId": "forum-agent"},
            {"chatId": -10001, "agentId": "default-chat-agent"}
          ]
        }
        "#;

        let loaded = Config::builder()
            .add_source(File::from_str(raw, FileFormat::Json))
            .build();
        let cfg = match loaded {
            Ok(config) => config.try_deserialize::<TelegramSettings>(),
            Err(err) => panic!("failed to build config: {err}"),
        };
        let settings = match cfg {
            Ok(settings) => settings,
            Err(err) => panic!("failed to deserialize settings: {err}"),
        };

        let forum = settings.resolve_topic_config(-10001, Some(10));
        match forum {
            Some(topic) => assert_eq!(topic.agent_id.as_deref(), Some("forum-agent")),
            None => panic!("expected forum topic config"),
        }

        let chat_default = settings.resolve_topic_config(-10001, None);
        match chat_default {
            Some(topic) => assert_eq!(topic.agent_id.as_deref(), Some("default-chat-agent")),
            None => panic!("expected chat-level topic config"),
        }

        assert!(settings.resolve_topic_config(-10001, Some(99)).is_none());
        assert!(settings.resolve_topic_config(-20002, None).is_none());
    }

    #[test]
    fn deserializes_topic_config_without_agent_id_as_none() {
        let raw = r#"
        {
          "telegram_token": "dummy",
          "topicConfigs": [
            {"chatId": -30003, "threadId": 5}
          ]
        }
        "#;

        let loaded = Config::builder()
            .add_source(File::from_str(raw, FileFormat::Json))
            .build();
        let cfg = match loaded {
            Ok(config) => config.try_deserialize::<TelegramSettings>(),
            Err(err) => panic!("failed to build config: {err}"),
        };
        let settings = match cfg {
            Ok(settings) => settings,
            Err(err) => panic!("failed to deserialize settings: {err}"),
        };

        assert_eq!(settings.topic_configs.len(), 1);
        assert_eq!(settings.topic_configs[0].agent_id, None);
    }
}
