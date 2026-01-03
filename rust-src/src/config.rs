use config::{Config, ConfigError, Environment, File};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Settings {
    pub telegram_token: String,

    #[serde(rename = "allowed_users")]
    pub allowed_users_str: Option<String>,

    #[serde(rename = "agent_access_ids")]
    pub agent_allowed_users_str: Option<String>,

    // API Keys
    pub groq_api_key: Option<String>,
    pub mistral_api_key: Option<String>,
    pub zai_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
    pub openrouter_api_key: Option<String>,
    pub tavily_api_key: Option<String>,

    // R2 Storage
    pub r2_access_key_id: Option<String>,
    pub r2_secret_access_key: Option<String>,
    pub r2_endpoint_url: Option<String>,
    pub r2_bucket_name: Option<String>,

    // OpenRouter configuration
    #[serde(default = "default_openrouter_site_url")]
    pub openrouter_site_url: String,
    #[serde(default = "default_openrouter_site_name")]
    pub openrouter_site_name: String,

    // System message
    pub system_message: Option<String>,
}

fn default_openrouter_site_url() -> String {
    "".to_string()
}

fn default_openrouter_site_name() -> String {
    "Another Chat TG Bot".to_string()
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let run_mode = std::env::var("RUN_MODE").unwrap_or_else(|_| "development".into());

        let s = Config::builder()
            // Start off by merging in the "default" configuration file
            .add_source(File::with_name("config/default").required(false))
            // Add in the current environment file
            .add_source(File::with_name(&format!("config/{}", run_mode)).required(false))
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

        let mut settings: Settings = s.try_deserialize()?;

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
    fn test_config_env_loading() {
        // 1. Test standard loading
        env::set_var("R2_ENDPOINT_URL", "https://example.com");
        env::set_var("TELEGRAM_TOKEN", "dummy_token");

        let settings = Settings::new().expect("Failed to create settings");
        assert_eq!(
            settings.r2_endpoint_url,
            Some("https://example.com".to_string())
        );

        env::remove_var("R2_ENDPOINT_URL");
        env::remove_var("TELEGRAM_TOKEN");

        // 2. Test empty env var
        env::set_var("R2_ENDPOINT_URL", "");
        env::set_var("TELEGRAM_TOKEN", "dummy_token");

        let settings = Settings::new().expect("Failed to create settings");
        // With our fallback logic, if it's empty in env, config might ignore it (or treating as unset).
        // Our fallback only sets if !val.is_empty().
        // So it should be None.
        assert_eq!(settings.r2_endpoint_url, None);

        env::remove_var("R2_ENDPOINT_URL");
        env::remove_var("TELEGRAM_TOKEN");

        // 3. Test explicit mapping case (Upper to lower)
        env::set_var("R2_ENDPOINT_URL", "https://mapping.test");
        env::set_var("TELEGRAM_TOKEN", "dummy");

        let settings = Settings::new().expect("Failed to create settings");
        assert_eq!(
            settings.r2_endpoint_url,
            Some("https://mapping.test".to_string())
        );

        env::remove_var("R2_ENDPOINT_URL");
        env::remove_var("TELEGRAM_TOKEN");
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
            openrouter_site_url: "".to_string(),
            openrouter_site_name: "".to_string(),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: &'static str,
    pub max_tokens: u32,
    pub provider: &'static str,
}

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
        "ZAI GLM-4.6-Flash",
        ModelInfo {
            id: "GLM-4.6V-Flash",
            max_tokens: 4095,
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

pub const DEFAULT_MODEL: &str = "OR Gemini 3 Flash";

// Agent Mode configuration
pub const AGENT_MODEL: &str = "Devstral 2512";
pub const AGENT_TIMEOUT_SECS: u64 = 1800; // 30 minutes
pub const AGENT_MAX_TOKENS: usize = 200_000;
pub const AGENT_COMPACT_THRESHOLD: usize = 180_000; // 90% of max, triggers auto-compact

// Sandbox configuration
pub const SANDBOX_IMAGE: &str = "agent-sandbox:latest";
pub const SANDBOX_MEMORY_LIMIT: i64 = 1024 * 1024 * 1024; // 1GB
pub const SANDBOX_CPU_PERIOD: i64 = 100_000;
pub const SANDBOX_CPU_QUOTA: i64 = 200_000; // 2 CPUs (200% of period)
pub const SANDBOX_EXEC_TIMEOUT_SECS: u64 = 60; // 1 minute per command
