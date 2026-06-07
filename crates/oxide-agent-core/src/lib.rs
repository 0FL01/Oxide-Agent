#![deny(missing_docs)]
// Production: forbid unsafe. Tests: no lint (allows test helpers to wrap unsafe env ops).
#![cfg_attr(not(test), forbid(unsafe_code))]
//! Oxide Agent core library.
//!
//! Shared logic for agent execution, providers, sandboxing, and storage.

/// Agent logic and tools.
pub mod agent;
/// Capability module manifests and registry scaffolding.
pub mod capabilities;
/// Configuration management.
pub mod config;
/// LLM providers and client.
pub mod llm;
/// Docker sandboxing for code execution.
pub mod sandbox;
/// SQL-backed durable storage layer.
pub mod storage;
/// Utility functions.
pub mod utils;

/// Testing helpers: mock providers and env wrappers.
#[cfg(test)]
pub mod testing;
