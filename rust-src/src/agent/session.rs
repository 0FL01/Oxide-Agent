//! Agent session management
//!
//! Manages the lifecycle of an agent session for a user, including
//! timeout tracking, progress message tracking, and session state.

use super::memory::AgentMemory;
use crate::config::{AGENT_MAX_TOKENS, AGENT_TIMEOUT_SECS};
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Status of an agent session
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum AgentStatus {
    /// Agent is idle, waiting for a task
    #[default]
    Idle,
    /// Agent is processing a task
    Processing { step: String, progress_percent: u8 },
    /// Agent has completed the task
    Completed,
    /// Agent timed out (30 minute limit)
    TimedOut,
    /// Agent encountered an error
    Error(String),
}

/// Represents an active agent session for a user
pub struct AgentSession {
    /// Telegram user ID
    pub user_id: i64,
    /// Telegram chat ID
    pub chat_id: i64,
    /// Message ID for progress updates (edited in-place)
    pub progress_message_id: Option<i32>,
    /// Conversation memory with auto-compaction
    pub memory: AgentMemory,
    /// When the current task started
    started_at: Option<Instant>,
    /// Current status
    pub status: AgentStatus,
}

impl AgentSession {
    /// Create a new agent session for a user
    pub fn new(user_id: i64, chat_id: i64) -> Self {
        Self {
            user_id,
            chat_id,
            progress_message_id: None,
            memory: AgentMemory::new(AGENT_MAX_TOKENS),
            started_at: None,
            status: AgentStatus::Idle,
        }
    }

    /// Start a new task, resetting the timer
    pub fn start_task(&mut self) {
        self.started_at = Some(Instant::now());
        self.status = AgentStatus::Processing {
            step: "Инициализация...".to_string(),
            progress_percent: 0,
        };
    }

    /// Check if the session has exceeded the timeout limit
    pub fn is_timed_out(&self) -> bool {
        self.started_at
            .map(|start| start.elapsed().as_secs() > AGENT_TIMEOUT_SECS)
            .unwrap_or(false)
    }

    /// Get elapsed time in seconds since task start
    pub fn elapsed_secs(&self) -> u64 {
        self.started_at
            .map(|start| start.elapsed().as_secs())
            .unwrap_or(0)
    }

    /// Update the progress status
    pub fn update_progress(&mut self, step: String, progress_percent: u8) {
        self.status = AgentStatus::Processing {
            step,
            progress_percent: progress_percent.min(100),
        };
    }

    /// Mark the task as completed
    pub fn complete(&mut self) {
        self.status = AgentStatus::Completed;
        self.started_at = None;
    }

    /// Mark the task as timed out
    pub fn timeout(&mut self) {
        self.status = AgentStatus::TimedOut;
        self.started_at = None;
    }

    /// Mark the task as failed with an error
    pub fn fail(&mut self, error: String) {
        self.status = AgentStatus::Error(error);
        self.started_at = None;
    }

    /// Reset the session (clear memory, reset status)
    pub fn reset(&mut self) {
        self.memory.clear();
        self.status = AgentStatus::Idle;
        self.started_at = None;
        self.progress_message_id = None;
    }

    /// Check if the session is currently processing a task
    pub fn is_processing(&self) -> bool {
        matches!(self.status, AgentStatus::Processing { .. })
    }
}
