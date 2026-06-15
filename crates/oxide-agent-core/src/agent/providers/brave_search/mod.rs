//! Brave Search provider.
//!
//! Provides URL discovery via Brave Web Search. Page fetching remains the
//! responsibility of `web_crawler`, `web_markdown`, or another explicit page opener.

mod client;
mod error;
mod format;
mod provider;
mod types;

pub use provider::{BraveSearchProvider, BraveSearchProviderConfig};
