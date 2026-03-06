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

/// Schema version for persisted task snapshots.
pub const TASK_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Schema version for persisted task event logs.
pub const TASK_EVENT_LOG_SCHEMA_VERSION: u32 = 1;

/// Persisted checkpoint used for restart-safe task recovery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskCheckpoint {
    /// Schema version for checkpoint compatibility.
    pub schema_version: u32,
    /// Current lifecycle state persisted at checkpoint time.
    pub state: TaskState,
    /// Latest task event sequence included in the checkpoint.
    pub last_event_sequence: u64,
    /// Timestamp when the checkpoint was written.
    pub persisted_at: DateTime<Utc>,
}

impl TaskCheckpoint {
    /// Create a new checkpoint for the current task state.
    #[must_use]
    pub fn new(state: TaskState, last_event_sequence: u64) -> Self {
        Self {
            schema_version: TASK_SNAPSHOT_SCHEMA_VERSION,
            state,
            last_event_sequence,
            persisted_at: Utc::now(),
        }
    }
}

/// Restart-safe persisted task snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    /// Schema version for snapshot compatibility.
    pub schema_version: u32,
    /// Stable metadata for the task instance.
    pub metadata: TaskMetadata,
    /// Transport-agnostic task input payload.
    pub task: String,
    /// Latest persisted recovery checkpoint.
    pub checkpoint: TaskCheckpoint,
}

impl TaskSnapshot {
    /// Create a new persisted snapshot for a task.
    #[must_use]
    pub fn new(metadata: TaskMetadata, task: String, last_event_sequence: u64) -> Self {
        let checkpoint = TaskCheckpoint::new(metadata.state, last_event_sequence);

        Self {
            schema_version: TASK_SNAPSHOT_SCHEMA_VERSION,
            metadata,
            task,
            checkpoint,
        }
    }
}

/// Baseline persisted task event entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEvent {
    /// Event log schema version.
    pub schema_version: u32,
    /// Task identifier that owns this event log entry.
    pub task_id: TaskId,
    /// Monotonic sequence number within the task event log.
    pub sequence: u64,
    /// Event classification for replay and audit.
    pub kind: TaskEventKind,
    /// Task state after this event is applied.
    pub state: TaskState,
    /// Transport-agnostic event details.
    pub message: Option<String>,
    /// Timestamp when the event was recorded.
    pub recorded_at: DateTime<Utc>,
}

impl TaskEvent {
    /// Create a new task event.
    #[must_use]
    pub fn new(
        task_id: TaskId,
        sequence: u64,
        kind: TaskEventKind,
        state: TaskState,
        message: Option<String>,
    ) -> Self {
        Self {
            schema_version: TASK_EVENT_LOG_SCHEMA_VERSION,
            task_id,
            sequence,
            kind,
            state,
            message,
            recorded_at: Utc::now(),
        }
    }
}

/// Baseline task event kinds persisted for recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEventKind {
    /// Task registration has been persisted.
    Created,
    /// Task lifecycle state changed.
    StateChanged,
    /// Recovery checkpoint was persisted.
    CheckpointSaved,
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
    use super::{
        TaskEvent, TaskEventKind, TaskId, TaskMetadata, TaskSnapshot, TaskState,
        TaskStateTransitionError, TASK_EVENT_LOG_SCHEMA_VERSION, TASK_SNAPSHOT_SCHEMA_VERSION,
    };
    use chrono::Utc;

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

    #[test]
    fn task_snapshot_roundtrip_preserves_recovery_contract() {
        let metadata = TaskMetadata::new();
        let snapshot = TaskSnapshot::new(metadata.clone(), "rebuild index".to_string(), 3);

        let json = serde_json::to_string(&snapshot);
        assert!(json.is_ok());

        let parsed: Result<TaskSnapshot, serde_json::Error> =
            serde_json::from_str(&json.unwrap_or_default());
        assert!(parsed.is_ok());

        let parsed = parsed.unwrap_or_else(|_| snapshot.clone());
        assert_eq!(parsed.schema_version, TASK_SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(parsed.metadata, metadata);
        assert_eq!(parsed.task, "rebuild index");
        assert_eq!(parsed.checkpoint.state, TaskState::Pending);
        assert_eq!(parsed.checkpoint.last_event_sequence, 3);
    }

    #[test]
    fn task_events_roundtrip_preserves_baseline_event_log_contract() {
        let task_id = TaskId::new();
        let event = TaskEvent::new(
            task_id,
            7,
            TaskEventKind::StateChanged,
            TaskState::Running,
            Some("worker lease renewed".to_string()),
        );

        let json = serde_json::to_string(&event);
        assert!(json.is_ok());

        let parsed: Result<TaskEvent, serde_json::Error> =
            serde_json::from_str(&json.unwrap_or_default());
        assert!(parsed.is_ok());

        let parsed = parsed.unwrap_or(event.clone());
        assert_eq!(parsed.schema_version, TASK_EVENT_LOG_SCHEMA_VERSION);
        assert_eq!(parsed.task_id, task_id);
        assert_eq!(parsed.sequence, 7);
        assert_eq!(parsed.kind, TaskEventKind::StateChanged);
        assert_eq!(parsed.state, TaskState::Running);
        assert_eq!(parsed.message.as_deref(), Some("worker lease renewed"));
        assert!(parsed.recorded_at <= Utc::now());
    }
}
