//! Browser-rendered Markdown via a configured Crawl4AI REST service.
//!
//! Provides `crawl4ai_markdown`: one validated public URL, one `POST /crawl`,
//! bounded Markdown output. Oxide does not manage Crawl4AI lifecycle.

pub(crate) mod constants;
pub(crate) mod crawl;
pub(crate) mod env_helpers;
pub(crate) mod errors;
pub(crate) mod executor;
pub(crate) mod reddit_rss;
pub(crate) mod response;
pub(crate) mod types;
pub(crate) mod url_validation;

use constants::*;
use env_helpers::*;
use errors::*;
use response::*;
use types::*;
use url_validation::*;

use crate::agent::tool_runtime::{ToolExecutor, ToolName, ToolRuntimeConfig};
use crate::llm::ToolDefinition;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

use crawl::read_limited_body;
use executor::Crawl4AiMarkdownToolExecutor;

/// Native provider for browser-rendered Markdown through Crawl4AI REST.
pub struct Crawl4AiMarkdownProvider {
    client: reqwest::Client,
    config: Crawl4AiMarkdownConfig,
}

impl Crawl4AiMarkdownProvider {
    /// Create a provider from environment configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(Crawl4AiMarkdownConfig::from_env())
    }

    pub(super) fn with_config(config: Crawl4AiMarkdownConfig) -> Self {
        let client_timeout = Duration::from_secs(config.max_timeout_secs.saturating_add(5));
        let client = reqwest::Client::builder()
            .timeout(client_timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client, config }
    }

    /// Build native typed runtime executors for the Crawl4AI markdown tool.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        let spec = Self::tool_definition();
        vec![Arc::new(Crawl4AiMarkdownToolExecutor {
            provider: Arc::clone(self),
            name: ToolName::from(spec.name.clone()),
            spec,
        })]
    }

    fn tool_definition() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_CRAWL4AI_MARKDOWN.to_string(),
            description: concat!(
                "Open one http/https URL with the configured Crawl4AI REST service and return bounded Markdown. ",
                "Use after selecting specific URLs from brave_search or searxng_search. Do not crawl every search result. ",
                "For Reddit thread URLs, omit max_chars or use 15000-30000 so comments and benchmarks are not prematurely truncated. ",
                "Use for pages that need browser rendering, JavaScript, overlay/consent handling, or when web_markdown fails. ",
                "Browser-level optimizations are enabled by default: images blocked (text_mode), background features disabled (light_mode), ad/tracker domains blocked (avoid_ads). ",
                "This tool does not crawl multiple pages, execute JavaScript, run hooks, use LLM extraction, or return screenshots/PDFs."
            ).to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Fully-qualified public http/https URL to open"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Optional request timeout in seconds, clamped to configured bounds"
                    },
                    "wait_for": {
                        "type": "string",
                        "description": "Optional CSS selector to wait for before extracting Markdown; JavaScript conditions are not allowed"
                    },
                    "fresh": {
                        "type": "boolean",
                        "description": "If true, bypass Crawl4AI content cache for this crawl; default false"
                    },
                    "max_chars": {
                        "type": "integer",
                        "description": "Optional maximum Markdown characters to return, clamped to configured hard cap"
                    }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
        }
    }
}

impl Default for Crawl4AiMarkdownProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Crawl4AiMarkdownConfig {
    fn from_env() -> Self {
        let max_timeout_secs = env_u64("OXIDE_CRAWL4AI_MAX_TIMEOUT_SECS", DEFAULT_MAX_TIMEOUT_SECS);
        let default_timeout_secs =
            env_u64("OXIDE_CRAWL4AI_DEFAULT_TIMEOUT_SECS", DEFAULT_TIMEOUT_SECS)
                .clamp(1, max_timeout_secs);
        let jitter_min_ms = env_u64("OXIDE_CRAWL4AI_JITTER_MIN_MS", DEFAULT_JITTER_MIN_MS);
        let jitter_max_ms =
            env_u64("OXIDE_CRAWL4AI_JITTER_MAX_MS", DEFAULT_JITTER_MAX_MS).max(jitter_min_ms);

        Self {
            base_url: env_url("OXIDE_CRAWL4AI_BASE_URL", DEFAULT_BASE_URL),
            api_token: env_non_empty("OXIDE_CRAWL4AI_API_TOKEN"),
            default_timeout_secs,
            max_timeout_secs,
            max_output_chars: env_usize(
                "OXIDE_CRAWL4AI_MAX_OUTPUT_CHARS",
                DEFAULT_MAX_OUTPUT_CHARS,
            )
            .max(1),
            health_timeout_ms: env_u64(
                "OXIDE_CRAWL4AI_HEALTH_TIMEOUT_MS",
                DEFAULT_HEALTH_TIMEOUT_MS,
            )
            .max(1),
            jitter_min_ms,
            jitter_max_ms,
            max_retries: env_usize("OXIDE_CRAWL4AI_MAX_RETRIES", DEFAULT_MAX_RETRIES),
            text_mode: env_bool("OXIDE_CRAWL4AI_TEXT_MODE", true),
            light_mode: env_bool("OXIDE_CRAWL4AI_LIGHT_MODE", true),
            avoid_ads: env_bool("OXIDE_CRAWL4AI_AVOID_ADS", true),
        }
    }
}

#[cfg(test)]
mod tests;
