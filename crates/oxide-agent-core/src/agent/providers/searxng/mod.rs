//! SearXNG tool provider.
//!
//! Provides a self-hosted web search tool backed by the SearXNG JSON API.
//! Transient HTTP errors are retried automatically with exponential backoff
//! and jitter — the agent sees only the final result or a clean error message.

mod backoff;
mod client;
mod error;
mod format;
mod provider;
mod types;

pub use provider::SearxngProvider;
