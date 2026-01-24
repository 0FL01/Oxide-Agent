#![deny(missing_docs)]
//! Telegram transport adapter for Oxide Agent.

/// Telegram-specific bot/transport implementation.
pub mod bot;
/// Telegram transport configuration.
pub mod config;
/// Telegram runtime entrypoint.
pub mod runner;
