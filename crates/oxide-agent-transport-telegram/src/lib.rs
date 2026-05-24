#![deny(missing_docs)]
//! Telegram transport adapter for Oxide Agent.

/// Telegram-specific bot/transport implementation.
#[cfg(feature = "storage-s3-r2")]
pub mod bot;
/// Telegram transport configuration.
pub mod config;
/// In-memory reminder due queue and notifier integration.
#[cfg(feature = "storage-s3-r2")]
pub mod reminder_scheduler;
/// Telegram runtime entrypoint.
pub mod runner;
