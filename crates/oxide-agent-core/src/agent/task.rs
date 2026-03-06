//! Domain types for persistent agent tasks.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Transport-agnostic task identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(Uuid);

impl TaskId {
    /// Create a new random task identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Return the raw UUID value.
    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Uuid> for TaskId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl From<TaskId> for Uuid {
    fn from(value: TaskId) -> Self {
        value.0
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle state for a background task.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Task is registered but not started.
    Pending,
    /// Task is actively executing.
    Running,
    /// Task finished successfully.
    Completed,
    /// Task finished with a failure.
    Failed,
    /// Task was cancelled before successful completion.
    Cancelled,
}

impl TaskState {
    /// Return true when the state is terminal.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Return true when the state has not reached a terminal outcome yet.
    #[must_use]
    pub const fn is_non_terminal(self) -> bool {
        !self.is_terminal()
    }

    /// Return true when the task is actively executing.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Running)
    }

    /// Return true when the transition is allowed by the task state machine.
    ///
    /// Transition graph:
    /// Pending -> Running | Cancelled
    /// Running -> Completed | Failed | Cancelled
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Pending, Self::Running | Self::Cancelled)
                | (
                    Self::Running,
                    Self::Completed | Self::Failed | Self::Cancelled
                )
        )
    }

    /// Validate a transition and return an explicit error when it is invalid.
    pub fn validate_transition(self, next: Self) -> Result<(), TaskStateTransitionError> {
        if self.can_transition_to(next) {
            Ok(())
        } else {
            Err(TaskStateTransitionError::InvalidTransition {
                from: self,
                to: next,
            })
        }
    }
}

/// Minimal task metadata safe for future persistence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskMetadata {
    /// Stable task identifier.
    pub id: TaskId,
    /// Current task lifecycle state.
    pub state: TaskState,
    /// Task creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Timestamp of the latest state update.
    pub updated_at: DateTime<Utc>,
}

impl TaskMetadata {
    /// Create metadata for a newly registered pending task.
    #[must_use]
    pub fn new() -> Self {
        let now = Utc::now();

        Self {
            id: TaskId::new(),
            state: TaskState::Pending,
            created_at: now,
            updated_at: now,
        }
    }

    /// Transition the task to a new state and update the timestamp.
    pub fn transition_to(&mut self, next: TaskState) -> Result<(), TaskStateTransitionError> {
        self.state.validate_transition(next)?;
        self.state = next;
        self.updated_at = Utc::now();
        Ok(())
    }
}

impl Default for TaskMetadata {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors returned by task state validation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TaskStateTransitionError {
    /// The requested state transition is not permitted.
    #[error("invalid task state transition: {from:?} -> {to:?}")]
    InvalidTransition {
        /// Current task state.
        from: TaskState,
        /// Requested next task state.
        to: TaskState,
    },
}

#[cfg(test)]
mod tests {
    use super::{TaskMetadata, TaskState, TaskStateTransitionError};

    #[test]
    fn task_state_reports_terminal_semantics() {
        assert!(TaskState::Completed.is_terminal());
        assert!(TaskState::Failed.is_terminal());
        assert!(TaskState::Cancelled.is_terminal());
        assert!(TaskState::Pending.is_non_terminal());
        assert!(TaskState::Running.is_non_terminal());
        assert!(!TaskState::Completed.is_non_terminal());
        assert!(!TaskState::Pending.is_active());
        assert!(TaskState::Running.is_active());
        assert!(!TaskState::Cancelled.is_active());
    }

    #[test]
    fn task_state_allows_valid_transitions_from_pending() {
        for next in [TaskState::Running, TaskState::Cancelled] {
            assert_eq!(TaskState::Pending.validate_transition(next), Ok(()));
        }
    }

    #[test]
    fn task_state_allows_valid_transitions_from_running() {
        for next in [
            TaskState::Completed,
            TaskState::Failed,
            TaskState::Cancelled,
        ] {
            assert_eq!(TaskState::Running.validate_transition(next), Ok(()));
        }
    }

    #[test]
    fn task_state_rejects_invalid_transitions() {
        let cases = [
            (TaskState::Pending, TaskState::Pending),
            (TaskState::Pending, TaskState::Completed),
            (TaskState::Pending, TaskState::Failed),
            (TaskState::Running, TaskState::Pending),
            (TaskState::Running, TaskState::Running),
            (TaskState::Completed, TaskState::Pending),
            (TaskState::Completed, TaskState::Running),
            (TaskState::Completed, TaskState::Completed),
            (TaskState::Completed, TaskState::Failed),
            (TaskState::Completed, TaskState::Cancelled),
            (TaskState::Failed, TaskState::Pending),
            (TaskState::Failed, TaskState::Running),
            (TaskState::Failed, TaskState::Completed),
            (TaskState::Failed, TaskState::Failed),
            (TaskState::Failed, TaskState::Cancelled),
            (TaskState::Cancelled, TaskState::Pending),
            (TaskState::Cancelled, TaskState::Running),
            (TaskState::Cancelled, TaskState::Completed),
            (TaskState::Cancelled, TaskState::Failed),
            (TaskState::Cancelled, TaskState::Cancelled),
        ];

        for (from, to) in cases {
            assert_eq!(
                from.validate_transition(to),
                Err(TaskStateTransitionError::InvalidTransition { from, to })
            );
        }
    }

    #[test]
    fn task_state_metadata_transition_updates_state() {
        let mut metadata = TaskMetadata::new();
        let created_at = metadata.created_at;

        assert_eq!(metadata.transition_to(TaskState::Running), Ok(()));
        assert_eq!(metadata.state, TaskState::Running);
        assert!(metadata.updated_at >= created_at);
    }

    #[test]
    fn task_state_metadata_rejects_invalid_transition() {
        let mut metadata = TaskMetadata::new();

        assert_eq!(metadata.transition_to(TaskState::Cancelled), Ok(()));

        let error = metadata.transition_to(TaskState::Running);
        assert_eq!(
            error,
            Err(TaskStateTransitionError::InvalidTransition {
                from: TaskState::Cancelled,
                to: TaskState::Running,
            })
        );
    }
}
