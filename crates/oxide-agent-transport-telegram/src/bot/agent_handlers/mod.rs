//! Agent mode handlers for Telegram bot
//!
//! Provides handlers for activating agent mode, processing messages,
//! and managing agent sessions.

use crate::bot::state::State;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::prelude::*;

mod callbacks;
mod controls;
mod execution_config;
mod input;
mod lifecycle;
mod reminders;
mod session;
mod shared;
mod task_runner;

pub(crate) use callbacks::*;
pub(crate) use controls::*;
pub(crate) use execution_config::*;
pub(crate) use input::*;
pub(crate) use lifecycle::*;
pub(crate) use reminders::*;
pub(crate) use session::*;
pub(crate) use shared::*;
pub(crate) use task_runner::*;

/// Type alias for dialogue
pub type AgentDialogue = Dialogue<State, InMemStorage<State>>;

#[cfg(test)]
mod tests;
