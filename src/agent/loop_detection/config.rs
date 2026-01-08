//! Configuration for loop detection.

use config::{Config, ConfigError, Environment, File};
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Loop detection configuration loaded from env/files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopDetectionConfig {
    /// Global toggle
    #[serde(rename = "loop_detection_enabled")]
    pub enabled: bool,

    /// Tool call repetition threshold
    #[serde(rename = "loop_tool_call_threshold")]
    pub tool_call_threshold: usize,

    /// Content chunk size (characters)
    #[serde(rename = "loop_content_chunk_size")]
    pub content_chunk_size: usize,
    /// Content loop repetition threshold
    #[serde(rename = "loop_content_threshold")]
    pub content_loop_threshold: usize,
    /// Max content history length (characters)
    #[serde(rename = "loop_max_history_length")]
    pub max_history_length: usize,

    /// Min turns before LLM checks start
    #[serde(rename = "loop_llm_check_after_turns")]
    pub llm_check_after_turns: usize,
    /// Initial LLM check interval
    #[serde(rename = "loop_llm_check_interval")]
    pub llm_check_interval: usize,
    /// Confidence threshold for LLM loop detection
    #[serde(rename = "loop_llm_confidence_threshold")]
    pub llm_confidence_threshold: f64,
    /// Number of history messages to send to LLM
    #[serde(rename = "loop_llm_history_count")]
    pub llm_history_count: usize,
    /// Model name for loop scouting
    #[serde(rename = "loop_scout_model")]
    pub scout_model: String,
}

impl Default for LoopDetectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            tool_call_threshold: 5,
            content_chunk_size: 50,
            content_loop_threshold: 10,
            max_history_length: 5000,
            llm_check_after_turns: 30,
            llm_check_interval: 3,
            llm_confidence_threshold: 0.95,
            llm_history_count: 20,
            scout_model: "magistral-medium-2509".to_string(),
        }
    }
}

impl LoopDetectionConfig {
    /// Load loop detection settings from config files and environment variables.
    ///
    /// Priority: env vars → config files → defaults.
    #[must_use]
    pub fn from_env() -> Self {
        let defaults = Self::default();
        let run_mode = std::env::var("RUN_MODE").unwrap_or_else(|_| "development".to_string());

        let builder = Config::builder()
            .set_default("loop_detection_enabled", defaults.enabled)
            .and_then(|b| {
                b.set_default(
                    "loop_tool_call_threshold",
                    defaults.tool_call_threshold as u64,
                )
            })
            .and_then(|b| {
                b.set_default(
                    "loop_content_chunk_size",
                    defaults.content_chunk_size as u64,
                )
            })
            .and_then(|b| {
                b.set_default(
                    "loop_content_threshold",
                    defaults.content_loop_threshold as u64,
                )
            })
            .and_then(|b| {
                b.set_default(
                    "loop_max_history_length",
                    defaults.max_history_length as u64,
                )
            })
            .and_then(|b| {
                b.set_default(
                    "loop_llm_check_after_turns",
                    defaults.llm_check_after_turns as u64,
                )
            })
            .and_then(|b| {
                b.set_default(
                    "loop_llm_check_interval",
                    defaults.llm_check_interval as u64,
                )
            })
            .and_then(|b| {
                b.set_default(
                    "loop_llm_confidence_threshold",
                    defaults.llm_confidence_threshold,
                )
            })
            .and_then(|b| {
                b.set_default("loop_llm_history_count", defaults.llm_history_count as u64)
            })
            .and_then(|b| b.set_default("loop_scout_model", defaults.scout_model.clone()))
            .map(|b| {
                b.add_source(File::with_name("config/default").required(false))
                    .add_source(File::with_name(&format!("config/{run_mode}")).required(false))
                    .add_source(File::with_name("config/local").required(false))
                    .add_source(Environment::default().ignore_empty(true))
            });

        let config = match builder {
            Ok(builder) => builder.build(),
            Err(err) => return Self::warn_and_default(err),
        };

        match config.and_then(Config::try_deserialize) {
            Ok(settings) => settings,
            Err(err) => Self::warn_and_default(err),
        }
    }

    fn warn_and_default(err: ConfigError) -> Self {
        warn!(error = %err, "Failed to load loop detection config, using defaults");
        Self::default()
    }
}
