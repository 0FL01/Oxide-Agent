//! Brave Search provider.
//!
//! Provides URL discovery via Brave Web Search. Page fetching remains the
//! responsibility of `crawl4ai_markdown` or another explicit page opener.

#![allow(dead_code)]

mod client;
mod error;
mod format;
mod provider;
mod types;

pub use provider::{BraveSearchProvider, BraveSearchProviderConfig};
