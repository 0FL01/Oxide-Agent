//! AuDHD task resume/open-loop state.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{
    GenerationScoped, MemoryGenerationId, PrincipalUserId, TaskStateId, TimestampMillis, TurnId,
};

/// Task state lifecycle.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStateStatus {
    /// Active task state.
    Active,
    /// Paused but resumable.
    Paused,
    /// Completed task.
    Completed,
    /// Abandoned task.
    Abandoned,
}

/// Resume packet for an active/paused project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifeTaskState {
    /// Task state id.
    pub task_state_id: TaskStateId,
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Memory generation owner.
    pub memory_generation_id: MemoryGenerationId,
    /// Stable project key.
    pub project_key: String,
    /// Current goal.
    pub current_goal: String,
    /// Why this goal matters.
    pub why: Option<String>,
    /// Current state bullets/payload.
    pub current_state: Value,
    /// Next concrete action.
    pub next_action: Option<String>,
    /// Open loops payload.
    pub open_loops: Value,
    /// Blockers payload.
    pub blockers: Value,
    /// Lifecycle status.
    pub status: TaskStateStatus,
    /// Last source turn.
    pub last_turn_id: Option<TurnId>,
    /// Creation timestamp.
    pub created_at: TimestampMillis,
    /// Last update timestamp.
    pub updated_at: TimestampMillis,
}

impl GenerationScoped for LifeTaskState {
    fn principal_user_id(&self) -> PrincipalUserId {
        self.principal_user_id
    }

    fn memory_generation_id(&self) -> MemoryGenerationId {
        self.memory_generation_id
    }
}
