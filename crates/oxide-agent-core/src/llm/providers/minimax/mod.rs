//! MiniMax AI provider implementation using claudius SDK
//!
//! Uses the Anthropic-compatible API endpoint: https://api.minimax.io/anthropic

mod client;
mod messages;
mod response;
mod tools;

pub use client::MiniMaxProvider;
