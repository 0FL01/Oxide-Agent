//! Shared constants for the Crawl4AI Markdown provider.

pub(crate) const TOOL_CRAWL4AI_MARKDOWN: &str = "crawl4ai_markdown";
pub(crate) const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11235";
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 60;
pub(crate) const DEFAULT_MAX_TIMEOUT_SECS: u64 = 120;
pub(crate) const DEFAULT_OUTPUT_CHARS: usize = 20_000;
pub(crate) const DEFAULT_MAX_OUTPUT_CHARS: usize = 30_000;
pub(crate) const DEFAULT_HEALTH_TIMEOUT_MS: u64 = 1_500;
pub(crate) const DEFAULT_JITTER_MIN_MS: u64 = 250;
pub(crate) const DEFAULT_JITTER_MAX_MS: u64 = 1_500;
pub(crate) const DEFAULT_MAX_RETRIES: usize = 0;
pub(crate) const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
pub(crate) const MAX_WAIT_FOR_CHARS: usize = 256;
pub(crate) const ERROR_MESSAGE_MAX_CHARS: usize = 1_000;
pub(crate) const RESPONSE_TAIL_MAX_CHARS: usize = 2_000;
pub(crate) const LOG_BODY_HEAD_MAX_CHARS: usize = 500;
