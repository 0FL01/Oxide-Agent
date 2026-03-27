//! SearXNG tool provider.
//!
//! Provides a self-hosted web search tool backed by the SearXNG JSON API.

mod client;
mod error;
mod format;
mod provider;
mod types;

pub use provider::SearxngProvider;
