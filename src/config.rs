//! Configuration and settings management
//!
//! Loads settings from environment variables and defines model constants.
//!
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
    /// `ZAI` (Zhipu AI) API key
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

    // Dynamic Model Configuration
    /// Chat model ID override
    pub chat_model_id: Option<String>,
    /// Chat model display name override
    pub chat_model_name: Option<String>,
    /// Chat model provider override
    pub chat_model_provider: Option<String>,
    /// Chat model max tokens override
    pub chat_model_max_tokens: Option<u32>,

    /// Agent model ID override
    pub agent_model_id: Option<String>,
    /// Agent model provider override
    pub agent_model_provider: Option<String>,
    /// Agent model max tokens override
    pub agent_model_max_tokens: Option<u32>,

    /// Sub-agent model ID override
    pub sub_agent_model_id: Option<String>,
    /// Sub-agent model provider override
    pub sub_agent_model_provider: Option<String>,
    /// Sub-agent model max tokens override
    pub sub_agent_model_max_tokens: Option<u32>,

    /// Media model ID override (for voice/images)
    pub media_model_id: Option<String>,
    /// Media model provider override
    pub media_model_provider: Option<String>,
}

const fn default_openrouter_site_url() -> String {
    String::new()
}

fn default_openrouter_site_name() -> String {
    "Oxide Agent TG Bot".to_string()
}

impl Settings {
    /// Create new settings by loading from environment and files
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use oxide_agent::config::Settings;
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

        if settings
            .zai_api_key
            .as_ref()
            .is_none_or(|key| key.trim().is_empty())
        {
            return Err(ConfigError::Message(
                "Critical: ZAI_API_KEY is required for operation".to_string(),
            ));
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

    /// Returns a list of available models, merging defaults with environment overrides
    pub fn get_available_models(&self) -> Vec<(String, ModelInfo)> {
        let mut models = default_models();

        // If CHAT_MODEL_ID and CHAT_MODEL_NAME are set, add/replace the model
        if let (Some(id), Some(name)) = (&self.chat_model_id, &self.chat_model_name) {
            let provider = self
                .chat_model_provider
                .clone()
                .unwrap_or_else(|| "openrouter".to_string());
            let max_tokens = self.chat_model_max_tokens.unwrap_or(64000);

            let new_model = ModelInfo {
                id: id.clone(),
                max_tokens,
                provider,
            };

            // Check if model with this name already exists
            if let Some(pos) = models.iter().position(|(n, _)| n == name) {
                models[pos] = (name.clone(), new_model);
            } else {
                models.push((name.clone(), new_model));
            }
        }

        models
    }

    /// Returns the configured agent model (id, provider, max_tokens)
    pub fn get_configured_agent_model(&self) -> (String, String, u32) {
        if let (Some(id), Some(provider)) = (&self.agent_model_id, &self.agent_model_provider) {
            return (
                id.clone(),
                provider.clone(),
                self.agent_model_max_tokens.unwrap_or(128000),
            );
        }
        // Default: ZAI GLM-4.7
        ("glm-4.7".to_string(), "zai".to_string(), 128000)
    }

    /// Returns the configured sub-agent model (id, provider, max_tokens)
    pub fn get_configured_sub_agent_model(&self) -> (String, String, u32) {
        if let (Some(id), Some(provider)) =
            (&self.sub_agent_model_id, &self.sub_agent_model_provider)
        {
            return (
                id.clone(),
                provider.clone(),
                self.sub_agent_model_max_tokens.unwrap_or(64000),
            );
        }
        // Default: ZAI GLM-4.5-Air
        ("glm-4.5-air".to_string(), "zai".to_string(), 64000)
    }

    /// Returns the configured media model (id, provider)
    pub fn get_media_model(&self) -> (String, String) {
        if let (Some(id), Some(provider)) = (&self.media_model_id, &self.media_model_provider) {
            return (id.clone(), provider.clone());
        }
        // Default: OpenRouter Gemini 3 Flash
        (
            "google/gemini-3-flash-preview".to_string(),
            "openrouter".to_string(),
        )
    }

    /// Returns model info by its display name
    pub fn get_model_info_by_name(&self, name: &str) -> Option<ModelInfo> {
        self.get_available_models()
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, info)| info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    // Tests run sequentially to avoid environment variable race conditions
    #[test]
    fn test_config_env_loading() -> Result<(), Box<dyn std::error::Error>> {
        env::set_var("ZAI_API_KEY", "dummy_zai_key");

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

        env::remove_var("ZAI_API_KEY");
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
            chat_model_id: None,
            chat_model_name: None,
            chat_model_provider: None,
            chat_model_max_tokens: None,
            agent_model_id: None,
            agent_model_provider: None,
            agent_model_max_tokens: None,
            sub_agent_model_id: None,
            sub_agent_model_provider: None,
            sub_agent_model_max_tokens: None,
            media_model_id: None,
            media_model_provider: None,
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
    pub id: String,
    /// Maximum allowed output tokens
    pub max_tokens: u32,
    /// Provider name
    pub provider: String,
}

/// Returns the default list of supported models and their configurations
pub fn default_models() -> Vec<(String, ModelInfo)> {
    vec![
        (
            "OR Gemini 3 Flash".to_string(),
            ModelInfo {
                id: "google/gemini-3-flash-preview".to_string(),
                max_tokens: 64000,
                provider: "openrouter".to_string(),
            },
        ),
        (
            "ZAI GLM-4.7".to_string(),
            ModelInfo {
                id: "glm-4.7".to_string(),
                max_tokens: 128000,
                provider: "zai".to_string(),
            },
        ),
        (
            "ZAI GLM-4.5-Air".to_string(),
            ModelInfo {
                id: "glm-4.5-air".to_string(),
                max_tokens: 64000,
                provider: "zai".to_string(),
            },
        ),
        (
            "Mistral Large".to_string(),
            ModelInfo {
                id: "mistral-large-latest".to_string(),
                max_tokens: 64000,
                provider: "mistral".to_string(),
            },
        ),
        (
            "Gemini 2.5 Flash Lite".to_string(),
            ModelInfo {
                id: "gemini-2.5-flash-lite".to_string(),
                max_tokens: 64000,
                provider: "gemini".to_string(),
            },
        ),
        (
            "Devstral 2512".to_string(),
            ModelInfo {
                id: "devstral-2512".to_string(),
                max_tokens: 64000,
                provider: "mistral".to_string(),
            },
        ),
        (
            "labs-devstral-small-2512".to_string(),
            ModelInfo {
                id: "labs-devstral-small-2512".to_string(),
                max_tokens: 64000,
                provider: "mistral".to_string(),
            },
        ),
        (
            "labs-mistral-small-creative".to_string(),
            ModelInfo {
                id: "labs-mistral-small-creative".to_string(),
                max_tokens: 32000,
                provider: "mistral".to_string(),
            },
        ),
    ]
}

/// Default model for chat
pub const DEFAULT_MODEL: &str = "OR Gemini 3 Flash";

// Agent Mode configuration
/// Model used for agent tasks (ZAI GLM-4.7)
pub const AGENT_MODEL_ZAI: &str = "ZAI GLM-4.7";
/// Model used for sub-agent tasks (ZAI GLM-4.5-Air)
pub const SUB_AGENT_MODEL_ZAI: &str = "ZAI GLM-4.5-Air";
/// Model used for agent tasks (Mistral Devstral 2512)
pub const AGENT_MODEL_MISTRAL: &str = "Devstral 2512";

/// Get the agent model based on environment variable or default to ZAI
///
/// Set `AGENT_MODEL_PROVIDER=mistral` to use Devstral 2512 instead of GLM-4.7
#[must_use]
pub fn get_agent_model() -> &'static str {
    match std::env::var("AGENT_MODEL_PROVIDER") {
        Ok(ref val) if val == "mistral" => AGENT_MODEL_MISTRAL,
        _ => AGENT_MODEL_ZAI, // Default to ZAI GLM-4.7
    }
}

/// Default agent model constant (deprecated, use `get_agent_model()` instead)
#[deprecated(since = "0.2.0", note = "Use `get_agent_model()` instead")]
pub const AGENT_MODEL: &str = "ZAI GLM-4.7";

/// Maximum iterations for agent loop
pub const AGENT_MAX_ITERATIONS: usize = 200;
/// Maximum iterations for sub-agent loop
pub const SUB_AGENT_MAX_ITERATIONS: usize = 60;
/// Agent task timeout in seconds
pub const AGENT_TIMEOUT_SECS: u64 = 1800; // 30 minutes
/// Sub-agent task timeout in seconds
pub const SUB_AGENT_TIMEOUT_SECS: u64 = 600;
/// Maximum timeout for individual tool call (in seconds)
/// This prevents a single tool from blocking the agent indefinitely
pub const AGENT_TOOL_TIMEOUT_SECS: u64 = 300; // 5 minutes
/// Agent memory token limit
pub const AGENT_MAX_TOKENS: usize = 200_000;
/// Sub-agent memory token limit (lighter context)
pub const SUB_AGENT_MAX_TOKENS: usize = 64_000;
/// Threshold to trigger memory compaction
pub const AGENT_COMPACT_THRESHOLD: usize = 180_000; // 90% of max, triggers auto-compact
/// Max forced continuations when todos incomplete
pub const AGENT_CONTINUATION_LIMIT: usize = 20; // Max forced continuations when todos incomplete

// Narrator system configuration
/// Model used for narrative generation (sidecar LLM)
pub const NARRATOR_MODEL: &str = "labs-mistral-small-creative";
/// Provider for narrator model
pub const NARRATOR_PROVIDER: &str = "mistral";
/// Maximum tokens for narrator response (concise output)
pub const NARRATOR_MAX_TOKENS: u32 = 256;

/// Get narrator model from env or default
#[must_use]
pub fn get_narrator_model() -> &'static str {
    // Static string required, so we don't support env override for now
    NARRATOR_MODEL
}

/// Get narrator provider from env or default
#[must_use]
pub fn get_narrator_provider() -> &'static str {
    NARRATOR_PROVIDER
}

// Skill system configuration
/// Skills directory (contains modular prompt files)
pub const SKILLS_DIR: &str = "skills";
/// Maximum tokens allocated to selected skills
pub const SKILL_TOKEN_BUDGET: usize = 4096;
/// Minimum semantic similarity score to consider a skill relevant
pub const SKILL_EMBEDDING_THRESHOLD: f32 = 0.6;
/// Maximum number of non-core skills to select
pub const SKILL_MAX_SELECTED: usize = 3;
/// TTL for skill metadata cache (seconds)
pub const SKILL_CACHE_TTL_SECS: u64 = 3600;
/// Default embedding model for skills
pub const MISTRAL_EMBED_MODEL: &str = "mistral-embed";
/// Expected embedding vector dimension
pub const EMBEDDING_DIMENSION: usize = 1024;
/// Embedding cache directory
pub const EMBEDDING_CACHE_DIR: &str = ".embeddings_cache/skills";

/// Get embedding dimension from env or default.
#[must_use]
pub fn get_embedding_dimension() -> usize {
    std::env::var("EMBEDDING_DIMENSION")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(EMBEDDING_DIMENSION)
}

/// Get skills directory path from env or default.
#[must_use]
pub fn get_skills_dir() -> String {
    std::env::var("SKILLS_DIR").unwrap_or_else(|_| SKILLS_DIR.to_string())
}

/// Get skill token budget from env or default.
#[must_use]
pub fn get_skill_token_budget() -> usize {
    std::env::var("SKILL_TOKEN_BUDGET")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(SKILL_TOKEN_BUDGET)
}

/// Get semantic threshold from env or default.
#[must_use]
pub fn get_skill_semantic_threshold() -> f32 {
    std::env::var("SKILL_SEMANTIC_THRESHOLD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(SKILL_EMBEDDING_THRESHOLD)
}

/// Get max selected skills from env or default.
#[must_use]
pub fn get_skill_max_selected() -> usize {
    std::env::var("SKILL_MAX_SELECTED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(SKILL_MAX_SELECTED)
}

/// Get skill cache TTL (seconds) from env or default.
#[must_use]
pub fn get_skill_cache_ttl_secs() -> u64 {
    std::env::var("SKILL_CACHE_TTL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(SKILL_CACHE_TTL_SECS)
}

/// Get embedding model name from env or default.
#[must_use]
pub fn get_mistral_embed_model() -> String {
    std::env::var("MISTRAL_EMBED_MODEL").unwrap_or_else(|_| MISTRAL_EMBED_MODEL.to_string())
}

/// Get embedding cache directory from env or default.
#[must_use]
pub fn get_embedding_cache_dir() -> String {
    std::env::var("EMBEDDING_CACHE_DIR").unwrap_or_else(|_| EMBEDDING_CACHE_DIR.to_string())
}

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

// Unauthorized access flood protection
/// Cooldown period (seconds) between "Access Denied" messages for same user
/// Default: 20 minutes
pub const UNAUTHORIZED_COOLDOWN_SECS: u64 = 1200; // 20 minutes
/// Time-to-live (seconds) for cache entries
/// Default: 2 hours
pub const UNAUTHORIZED_CACHE_TTL_SECS: u64 = 7200; // 2 hours
/// Maximum cache capacity (number of entries)
pub const UNAUTHORIZED_CACHE_MAX_SIZE: u64 = 10_000;

/// Get unauthorized cooldown from env or default
///
/// Environment variable: `UNAUTHORIZED_COOLDOWN_SECS`
#[must_use]
pub fn get_unauthorized_cooldown() -> u64 {
    std::env::var("UNAUTHORIZED_COOLDOWN_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(UNAUTHORIZED_COOLDOWN_SECS)
}

/// Get unauthorized cache TTL from env or default
///
/// Environment variable: `UNAUTHORIZED_CACHE_TTL_SECS`
#[must_use]
pub fn get_unauthorized_cache_ttl() -> u64 {
    std::env::var("UNAUTHORIZED_CACHE_TTL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(UNAUTHORIZED_CACHE_TTL_SECS)
}

/// Get unauthorized cache max size from env or default
///
/// Environment variable: `UNAUTHORIZED_CACHE_MAX_SIZE`
#[must_use]
pub fn get_unauthorized_cache_max_size() -> u64 {
    std::env::var("UNAUTHORIZED_CACHE_MAX_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(UNAUTHORIZED_CACHE_MAX_SIZE)
}

// Telegram API retry configuration
/// Maximum number of retry attempts for Telegram API file operations
pub const TELEGRAM_API_MAX_RETRIES: usize = 3;
/// Initial backoff delay in milliseconds for Telegram API retries
pub const TELEGRAM_API_INITIAL_BACKOFF_MS: u64 = 500;
/// Maximum backoff delay in milliseconds for Telegram API retries
pub const TELEGRAM_API_MAX_BACKOFF_MS: u64 = 4000;

// LLM HTTP client configuration
/// Default timeout for LLM API HTTP requests (seconds)
/// Keeps long-running model responses alive while preventing infinite hangs
pub const LLM_HTTP_TIMEOUT_SECS: u64 = 300;

/// Get LLM HTTP timeout from env or default
///
/// Environment variable: `LLM_HTTP_TIMEOUT_SECS`
#[must_use]
pub fn get_llm_http_timeout_secs() -> u64 {
    std::env::var("LLM_HTTP_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(LLM_HTTP_TIMEOUT_SECS)
}
