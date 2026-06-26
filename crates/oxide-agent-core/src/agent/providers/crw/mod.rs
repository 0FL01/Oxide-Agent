//! CRW rendered scrape client.
//!
//! CRW indexed search is owned by the unified `web_search` provider. This
//! module contains only the CRW `POST /v1/scrape` client used by rendered
//! `web_crawler` modes.

/// CRW HTTP client for the scrape endpoint.
pub mod client;
/// Error types for CRW operations.
pub mod error;
/// Request and response types for CRW API.
pub mod types;

pub use client::CrwScrapeClient;
pub use types::CrwScrapeArgs;
