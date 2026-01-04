//! Agent session management
//!
//! Manages the lifecycle of an agent session for a user, including
//! timeout tracking, progress message tracking, session state, and sandbox.

use super::memory::AgentMemory;
use super::providers::TodoList;
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
    /// Todo list for multi-step tasks
    pub todos: TodoList,
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
    pub fn new(user_id: i64, chat_id: i64) -> Self {
        Self {
            user_id,
            chat_id,
            progress_message_id: None,
            memory: AgentMemory::new(AGENT_MAX_TOKENS),
            todos: TodoList::new(),
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

    /// Reset the session (clear memory, todos, reset status)
    /// Note: Sandbox is persistent and not destroyed here
    pub async fn reset(&mut self) {
        self.memory.clear();
        self.todos.clear();
        self.status = AgentStatus::Idle;
        self.started_at = None;
        self.current_task_id = None;
        self.progress_message_id = None;

        // Sandbox is persistent, do NOT destroy it here
        // if let Some(mut sandbox) = self.sandbox.take() { ... }
    }

    /// Clear only the todos list (keeps memory intact)
    pub fn clear_todos(&mut self) {
        self.todos.clear();
    }

    /// Check if the session is currently processing a task
    pub fn is_processing(&self) -> bool {
        matches!(self.status, AgentStatus::Processing { .. })
    }

    /// Check if sandbox is available
    pub fn has_sandbox(&self) -> bool {
        self.sandbox.as_ref().is_some_and(|s| s.is_running())
    }

    /// Ensure sandbox is running, creating it if necessary
    pub async fn ensure_sandbox(&mut self) -> Result<&mut SandboxManager> {
        if self.sandbox.is_none() || !self.sandbox.as_ref().unwrap().is_running() {
            debug!(user_id = self.user_id, "Creating new sandbox");
            let mut sandbox = SandboxManager::new(self.user_id).await?;
            sandbox.create_sandbox().await?;
            self.sandbox = Some(sandbox);
            info!(user_id = self.user_id, "Sandbox created for session");
        }

        Ok(self.sandbox.as_mut().unwrap())
    }

    /// Get sandbox reference if running
    pub fn sandbox(&self) -> Option<&SandboxManager> {
        self.sandbox.as_ref().filter(|s| s.is_running())
    }

    /// Get mutable sandbox reference if running
    pub fn sandbox_mut(&mut self) -> Option<&mut SandboxManager> {
        self.sandbox.as_mut().filter(|s| s.is_running())
    }

    /// Destroy sandbox if running
    pub async fn destroy_sandbox(&mut self) -> Result<()> {
        if let Some(mut sandbox) = self.sandbox.take() {
            sandbox.destroy().await?;
            info!(user_id = self.user_id, "Sandbox destroyed");
        }
        Ok(())
    }
}
