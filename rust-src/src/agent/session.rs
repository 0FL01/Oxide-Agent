//! Agent session management
//!
//! Manages the lifecycle of an agent session for a user, including
//! timeout tracking, progress message tracking, session state, and sandbox.

use super::memory::AgentMemory;
// use super::providers::TodoList;
use crate::config::{AGENT_MAX_TOKENS, AGENT_TIMEOUT_SECS};
use crate::sandbox::SandboxManager;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::{debug, info};

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
    /// Docker sandbox for code execution (lazily initialized)
    sandbox: Option<SandboxManager>,
    /// When the current task started
    started_at: Option<Instant>,
    /// Unique ID for the current task execution (for log correlation)
    pub current_task_id: Option<String>,
    /// Current status
    pub status: AgentStatus,
}

impl AgentSession {
    /// Create a new agent session for a user
    #[must_use]
    pub fn new(user_id: i64, chat_id: i64) -> Self {
        Self {
            user_id,
            chat_id,
            progress_message_id: None,
            memory: AgentMemory::new(AGENT_MAX_TOKENS),
            sandbox: None,
            started_at: None,
            current_task_id: None,
            status: AgentStatus::Idle,
        }
    }

    /// Start a new task, resetting the timer and generating a task ID
    pub fn start_task(&mut self) {
        self.started_at = Some(Instant::now());
        self.current_task_id = Some(uuid::Uuid::new_v4().to_string());
        self.status = AgentStatus::Processing {
            step: "Инициализация...".to_string(),
            progress_percent: 0,
        };
    }

    /// Check if the session has exceeded the timeout limit
    #[must_use]
    pub fn is_timed_out(&self) -> bool {
        self.started_at
            .is_some_and(|start| start.elapsed().as_secs() > AGENT_TIMEOUT_SECS)
    }

    /// Get elapsed time in seconds since task start
    #[must_use]
    pub fn elapsed_secs(&self) -> u64 {
        self.started_at.map_or(0, |start| start.elapsed().as_secs())
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

    /// Reset the session (clear memory, todos, reset status)
    /// Note: Sandbox is persistent and not destroyed here
    pub fn reset(&mut self) {
        self.memory.clear();
        self.status = AgentStatus::Idle;
        self.started_at = None;
        self.current_task_id = None;
        self.progress_message_id = None;

        // Sandbox is persistent, do NOT destroy it here
        // if let Some(mut sandbox) = self.sandbox.take() { ... }
    }

    /// Clear only the todos list (keeps memory intact)
    pub fn clear_todos(&mut self) {
        self.memory.todos.clear();
    }

    /// Check if the session is currently processing a task
    #[must_use]
    pub const fn is_processing(&self) -> bool {
        matches!(self.status, AgentStatus::Processing { .. })
    }

    /// Check if sandbox is available
    #[must_use]
    pub fn has_sandbox(&self) -> bool {
        self.sandbox
            .as_ref()
            .is_some_and(SandboxManager::is_running)
    }

    /// Ensure sandbox is running, creating it if necessary
    ///
    /// # Errors
    ///
    /// Returns an error if sandbox creation fails.
    pub async fn ensure_sandbox(&mut self) -> Result<&mut SandboxManager> {
        let needs_new = self.sandbox.as_ref().is_none_or(|s| !s.is_running());

        if needs_new {
            debug!(user_id = self.user_id, "Creating new sandbox");
            let mut sandbox = SandboxManager::new(self.user_id).await?;
            sandbox.create_sandbox().await?;
            self.sandbox = Some(sandbox);
            info!(user_id = self.user_id, "Sandbox created for session");
        }

        self.sandbox
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Sandbox not initialized"))
    }

    /// Get sandbox reference if running
    #[must_use]
    pub fn sandbox(&self) -> Option<&SandboxManager> {
        self.sandbox.as_ref().filter(|s| s.is_running())
    }

    /// Get mutable sandbox reference if running
    pub fn sandbox_mut(&mut self) -> Option<&mut SandboxManager> {
        self.sandbox.as_mut().filter(|s| s.is_running())
    }

    /// Destroy sandbox if running
    ///
    /// # Errors
    ///
    /// Returns an error if sandbox destruction fails.
    pub async fn destroy_sandbox(&mut self) -> Result<()> {
        if let Some(mut sandbox) = self.sandbox.take() {
            sandbox.destroy().await?;
            info!(user_id = self.user_id, "Sandbox destroyed");
        }
        Ok(())
    }
}
