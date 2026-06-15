//! Generic Anthropic Messages API provider.
//!
//! Uses the Anthropic Messages v1 API with configurable base URL.
//! Supports any Anthropic-compatible endpoint (Anthropic, MiniMax, etc.).

mod client;
pub(crate) mod module;

pub use client::AnthropicProvider;
pub(crate) use module::AnthropicProviderModule;
