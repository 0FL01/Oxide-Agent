//! CRW web research provider.
//!
//! Provides `web_search` backed by CRW `POST /v1/search` and a scrape
//! client used by `web_crawler` fallback via CRW `POST /v1/scrape`.
//! Transient HTTP errors are retried automatically with exponential backoff.

/// CRW HTTP client for search and scrape endpoints.
pub mod client;
/// Error types for CRW operations.
pub mod error;
/// Markdown formatting for CRW search results.
pub mod format;
/// CRW provider and tool executor implementations.
pub mod provider;
/// Request and response types for CRW API.
pub mod types;

pub use provider::CrwProvider;
pub use types::CrwScrapeArgs;
