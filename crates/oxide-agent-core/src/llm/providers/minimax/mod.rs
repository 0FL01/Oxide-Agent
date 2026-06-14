//! MiniMax AI provider implementation using reqwest + shared Anthropic Messages helpers.
//!
//! Uses the Anthropic-compatible API endpoint: https://api.minimax.io/anthropic

use crate::llm::providers::anthropic_messages;

mod client;
pub(crate) mod module;

pub use client::MiniMaxProvider;
pub(crate) use module::MiniMaxProviderModule;
