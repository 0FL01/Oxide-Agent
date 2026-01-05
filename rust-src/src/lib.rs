#![deny(missing_docs)]
//! Another Chat with LLM - Rust implementation
//!
//! A Telegram bot that supports multiple LLM providers, multimodal input,
//! and an advanced agent mode with tool execution and sandboxing.

/// Agent logic and tools
pub mod agent;
/// Telegram bot implementation
pub mod bot;
/// Configuration management
pub mod config;
/// LLM providers and client
pub mod llm;
/// Docker sandboxing for code execution
pub mod sandbox;
/// Storage layer (R2/S3)
pub mod storage;
pub mod utils;
