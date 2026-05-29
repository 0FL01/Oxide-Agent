//! DuckDuckGo search provider.
//!
//! Provides URL discovery tools backed by DuckDuckGo Lite and News. Page
//! fetching remains the responsibility of `web_markdown`.

mod backoff;
mod client;
mod error;
mod format;
mod provider;
mod rate_limit;
mod types;

pub use provider::DuckDuckGoProvider;
