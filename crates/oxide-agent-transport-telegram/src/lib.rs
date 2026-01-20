#![deny(missing_docs)]
//! Telegram transport adapter for Oxide Agent.

/// Telegram transport configuration.
pub mod config;
/// Telegram-specific bot/transport implementation.
pub mod bot;
/// Telegram runtime entrypoint.
pub mod runner;
