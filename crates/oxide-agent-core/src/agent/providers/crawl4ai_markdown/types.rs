//! Shared domain types for the crawl4ai_markdown module.

use serde::Deserialize;

#[derive(Debug, Clone)]
pub(crate) struct Crawl4AiMarkdownConfig {
    pub base_url: reqwest::Url,
    pub api_token: Option<String>,
    pub default_timeout_secs: u64,
    pub max_timeout_secs: u64,
    pub max_output_chars: usize,
    pub health_timeout_ms: u64,
    pub jitter_min_ms: u64,
    pub jitter_max_ms: u64,
    pub max_retries: usize,
    pub text_mode: bool,
    pub light_mode: bool,
    pub avoid_ads: bool,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct Crawl4AiMarkdownArgs {
    pub url: String,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub wait_for: Option<String>,
    #[serde(default)]
    pub fresh: bool,
    #[serde(default)]
    pub max_chars: Option<usize>,
}

pub(crate) struct CrawlResult {
    pub final_url: Option<reqwest::Url>,
    pub status_code: Option<u16>,
    pub markdown_kind: &'static str,
    pub content_mode: &'static str,
    pub source_kind: &'static str,
    pub markdown: String,
    pub raw_chars: usize,
    pub selected_chars: usize,
    pub elapsed_ms: Option<u64>,
    pub entries_count: Option<usize>,
    pub noise_filtered: bool,
}

pub(crate) struct MarkdownSelection {
    pub kind: &'static str,
    pub content_mode: &'static str,
    pub text: String,
    pub raw_chars: usize,
    pub selected_chars: usize,
    pub noise_filtered: bool,
}

pub(crate) struct RedditAtomEntry {
    pub title: String,
    pub author: Option<String>,
    pub markdown: String,
}
