//! Agent-specific bot logic
//!
//! Contains transport-specific handlers for agent mode.

/// Media extraction from Telegram messages
pub mod media;

pub use media::extract_agent_input;
