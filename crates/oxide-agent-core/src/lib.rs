#![deny(missing_docs)]
//! Oxide Agent core library.
//!
//! Shared logic for agent execution, providers, sandboxing, and storage.

/// Agent logic and tools.
pub mod agent;
/// Configuration management.
pub mod config;
/// LLM providers and client.
pub mod llm;
/// Docker sandboxing for code execution.
pub mod sandbox;
/// Storage layer (R2/S3).
pub mod storage;
/// Utility functions.
pub mod utils;

#[cfg(test)]
pub mod testing;
