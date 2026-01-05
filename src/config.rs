//! Configuration and settings management
//!
//! Loads settings from environment variables and defines model constants.

use config::{Config, ConfigError, Environment, File};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Application settings loaded from environment variables
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Settings {
    /// Telegram Bot API token
    pub telegram_token: String,

    /// Comma-separated list of allowed user IDs for normal chat
    #[serde(rename = "allowed_users")]
    pub allowed_users_str: Option<String>,

    /// Comma-separated list of allowed user IDs for agent mode
    #[serde(rename = "agent_access_ids")]
    pub agent_allowed_users_str: Option<String>,

    /// Groq API key
    pub groq_api_key: Option<String>,
    /// Mistral API key
    pub mistral_api_key: Option<String>,
    /// `ZeroAI` API key
    pub zai_api_key: Option<String>,
    /// Gemini API key
    pub gemini_api_key: Option<String>,
    /// `OpenRouter` API key
    pub openrouter_api_key: Option<String>,
    /// Tavily API key
    pub tavily_api_key: Option<String>,

    /// R2 Storage access key ID
    pub r2_access_key_id: Option<String>,
    /// R2 Storage secret access key
    pub r2_secret_access_key: Option<String>,
    /// R2 Storage endpoint URL
    pub r2_endpoint_url: Option<String>,
    /// R2 Storage bucket name
    pub r2_bucket_name: Option<String>,

    /// Site URL for `OpenRouter` identification
    #[serde(default = "default_openrouter_site_url")]
    pub openrouter_site_url: String,
    /// Site name for `OpenRouter` identification
    #[serde(default = "default_openrouter_site_name")]
    pub openrouter_site_name: String,

    /// Default system message
    pub system_message: Option<String>,
}

const fn default_openrouter_site_url() -> String {
    String::new()
}

fn default_openrouter_site_name() -> String {
    "Another Chat TG Bot".to_string()
}

impl Settings {
    /// Create new settings by loading from environment and files
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use another_chat_rs::config::Settings;
    ///
    /// let settings = Settings::new().expect("Failed to load configuration");
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a `ConfigError` if loading fails.
    pub fn new() -> Result<Self, ConfigError> {
        let run_mode = std::env::var("RUN_MODE").unwrap_or_else(|_| "development".into());

        let s = Config::builder()
            // Start off by merging in the "default" configuration file
            .add_source(File::with_name("config/default").required(false))
            // Add in the current environment file
            .add_source(File::with_name(&format!("config/{run_mode}")).required(false))
            // Add in a local configuration file
            // This file shouldn't be checked into git
            .add_source(File::with_name("config/local").required(false))
            // Add in settings from the environment (with a prefix of APP)
            // Eg.. `APP_DEBUG=1 ./target/app` would set the `debug` key
            .add_source(Environment::with_prefix("APP").separator("__"))
            // Also add settings from environment variables directly (without prefix)
            // Note: Environment::default() auto-converts UPPER_SNAKE_CASE to snake_case
            // ignore_empty treats empty env vars as unset
            .add_source(Environment::default().ignore_empty(true))
            .build()?;

        let mut settings: Self = s.try_deserialize()?;

        // Fallback: Check environment variables directly if config didn't pick them up
        // This handles cases where automatic mapping might fail or behavior differs
        if settings.r2_endpoint_url.is_none() {
            if let Ok(val) = std::env::var("R2_ENDPOINT_URL") {
                if !val.is_empty() {
                    settings.r2_endpoint_url = Some(val);
                }
            }
        }
        if settings.r2_access_key_id.is_none() {
            if let Ok(val) = std::env::var("R2_ACCESS_KEY_ID") {
                if !val.is_empty() {
                    settings.r2_access_key_id = Some(val);
                }
            }
        }
        if settings.r2_secret_access_key.is_none() {
            if let Ok(val) = std::env::var("R2_SECRET_ACCESS_KEY") {
                if !val.is_empty() {
                    settings.r2_secret_access_key = Some(val);
                }
            }
        }
        if settings.r2_bucket_name.is_none() {
            if let Ok(val) = std::env::var("R2_BUCKET_NAME") {
                if !val.is_empty() {
                    settings.r2_bucket_name = Some(val);
                }
            }
        }

        Ok(settings)
    }

    /// Returns a set of Telegram IDs that are allowed to use the bot
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

    /// Returns a set of Telegram IDs that are allowed to use Agent Mode
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    // Tests run sequentially to avoid environment variable race conditions
    #[test]
    fn test_config_env_loading() -> Result<(), Box<dyn std::error::Error>> {
        // 1. Test standard loading
        env::set_var("R2_ENDPOINT_URL", "https://example.com");
        env::set_var("TELEGRAM_TOKEN", "dummy_token");

        let settings = Settings::new()?;
        assert_eq!(
            settings.r2_endpoint_url,
            Some("https://example.com".to_string())
        );

        env::remove_var("R2_ENDPOINT_URL");
        env::remove_var("TELEGRAM_TOKEN");

        // 2. Test empty env var
        env::set_var("R2_ENDPOINT_URL", "");
        env::set_var("TELEGRAM_TOKEN", "dummy_token");

        let settings = Settings::new()?;
        // With our fallback logic, if it's empty in env, config might ignore it (or treating as unset).
        // Our fallback only sets if !val.is_empty().
        // So it should be None.
        assert_eq!(settings.r2_endpoint_url, None);

        env::remove_var("R2_ENDPOINT_URL");
        env::remove_var("TELEGRAM_TOKEN");

        // 3. Test explicit mapping case (Upper to lower)
        env::set_var("R2_ENDPOINT_URL", "https://mapping.test");
        env::set_var("TELEGRAM_TOKEN", "dummy");

        let settings = Settings::new()?;
        assert_eq!(
            settings.r2_endpoint_url,
            Some("https://mapping.test".to_string())
        );

        env::remove_var("R2_ENDPOINT_URL");
        env::remove_var("TELEGRAM_TOKEN");
        Ok(())
    }

    #[test]
    fn test_list_parsing() {
        let mut settings = Settings {
            telegram_token: "dummy".to_string(),
            allowed_users_str: None,
            agent_allowed_users_str: None,
            groq_api_key: None,
            mistral_api_key: None,
            zai_api_key: None,
            gemini_api_key: None,
            openrouter_api_key: None,
            tavily_api_key: None,
            r2_access_key_id: None,
            r2_secret_access_key: None,
            r2_endpoint_url: None,
            r2_bucket_name: None,
            openrouter_site_url: String::new(),
            openrouter_site_name: String::new(),
            system_message: None,
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

/// Information about a supported LLM model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Internal model identifier
    pub id: &'static str,
    /// Maximum allowed output tokens
    pub max_tokens: u32,
    /// Provider name
    pub provider: &'static str,
}

/// List of all supported models and their configurations
pub const MODELS: &[(&str, ModelInfo)] = &[
    (
        "OR Gemini 3 Flash",
        ModelInfo {
            id: "google/gemini-3-flash-preview",
            max_tokens: 64000,
            provider: "openrouter",
        },
    ),
    (
        "ZAI GLM-4.7",
        ModelInfo {
            id: "glm-4.7",
            max_tokens: 128000,
            provider: "zai",
        },
    ),
    (
        "Mistral Large",
        ModelInfo {
            id: "mistral-large-latest",
            max_tokens: 64000,
            provider: "mistral",
        },
    ),
    (
        "Gemini 2.5 Flash Lite",
        ModelInfo {
            id: "gemini-2.5-flash-lite",
            max_tokens: 64000,
            provider: "gemini",
        },
    ),
    (
        "Devstral 2512",
        ModelInfo {
            id: "devstral-2512",
            max_tokens: 64000,
            provider: "mistral",
        },
    ),
];

/// Default model for chat
pub const DEFAULT_MODEL: &str = "OR Gemini 3 Flash";

// Agent Mode configuration
/// Model used for agent tasks
pub const AGENT_MODEL: &str = "Devstral 2512";
/// Maximum iterations for agent loop
pub const AGENT_MAX_ITERATIONS: usize = 100;
/// Agent task timeout in seconds
pub const AGENT_TIMEOUT_SECS: u64 = 1800; // 30 minutes
/// Agent memory token limit
pub const AGENT_MAX_TOKENS: usize = 200_000;
/// Threshold to trigger memory compaction
pub const AGENT_COMPACT_THRESHOLD: usize = 180_000; // 90% of max, triggers auto-compact
/// Max forced continuations when todos incomplete
pub const AGENT_CONTINUATION_LIMIT: usize = 5; // Max forced continuations when todos incomplete

// Sandbox configuration
/// Docker image for the sandbox
pub const SANDBOX_IMAGE: &str = "agent-sandbox:latest";
/// Memory limit for sandbox container (1GB)
pub const SANDBOX_MEMORY_LIMIT: i64 = 1024 * 1024 * 1024; // 1GB
/// CPU period for sandbox container
pub const SANDBOX_CPU_PERIOD: i64 = 100_000;
/// CPU quota for sandbox container (2 CPUs)
pub const SANDBOX_CPU_QUOTA: i64 = 200_000; // 2 CPUs (200% of period)
/// Timeout for individual command execution in sandbox
pub const SANDBOX_EXEC_TIMEOUT_SECS: u64 = 60; // 1 minute per command
