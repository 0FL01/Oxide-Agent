use config::{Config, ConfigError, Environment, File};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Settings {
    pub telegram_token: String,

    #[serde(rename = "allowed_users")]
    pub allowed_users_str: Option<String>,

    // API Keys
    pub groq_api_key: Option<String>,
    pub mistral_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
    pub openrouter_api_key: Option<String>,

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
            // to match the Python behavior where they are often just uppercase
            .add_source(Environment::default())
            .build()?;

        s.try_deserialize()
    }

    pub fn allowed_users(&self) -> HashSet<i64> {
        self.allowed_users_str
            .as_ref()
            .map(|s| {
                s.split(',')
                    .filter_map(|id| id.trim().parse::<i64>().ok())
                    .collect()
            })
            .unwrap_or_default()
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
        "GPT-OSS-120b",
        ModelInfo {
            id: "openai/gpt-oss-120b",
            max_tokens: 64000,
            provider: "groq",
        },
    ),
    (
        "Mistral Large",
        ModelInfo {
            id: "mistral-large-latest",
            max_tokens: 128000,
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
];

pub const DEFAULT_MODEL: &str = "OR Gemini 3 Flash";
