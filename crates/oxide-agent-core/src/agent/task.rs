//! Domain types for persistent agent tasks.

use crate::agent::{AgentMemory, SessionId};
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
    /// Task is paused and waiting for user-provided input.
    WaitingInput,
    /// Task finished successfully.
    Completed,
    /// Task finished with a failure.
    Failed,
    /// Task was cancelled before successful completion.
    Cancelled,
    /// Task stopped gracefully with a partial report at a safe point.
    Stopped,
}

impl TaskState {
    /// Return true when the state is terminal.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Stopped
        )
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
    /// Running -> WaitingInput | Completed | Failed | Cancelled | Stopped
    /// WaitingInput -> Running | Cancelled | Stopped
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Pending, Self::Running | Self::Cancelled)
                | (
                    Self::Running,
                    Self::WaitingInput
                        | Self::Completed
                        | Self::Failed
                        | Self::Cancelled
                        | Self::Stopped
                )
                | (
                    Self::WaitingInput,
                    Self::Running | Self::Cancelled | Self::Stopped
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

/// Transport-agnostic pending input request persisted for HITL resume flows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingInput {
    /// Stable request identifier used by runtime/transport mapping layers.
    pub request_id: String,
    /// Human-readable prompt shown to the user.
    pub prompt: String,
    /// Typed payload for user response constraints.
    #[serde(flatten)]
    pub kind: PendingInputKind,
}

impl PendingInput {
    /// Validate request fields and kind-specific constraints.
    pub fn validate(&self) -> Result<(), PendingInputValidationError> {
        if self.request_id.trim().is_empty() {
            return Err(PendingInputValidationError::EmptyRequestId);
        }

        if self.prompt.trim().is_empty() {
            return Err(PendingInputValidationError::EmptyPrompt);
        }

        self.kind.validate()
    }
}

/// Typed pending input payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PendingInputKind {
    /// Choice-based request (poll-like UX).
    Choice(PendingChoiceInput),
    /// Free-form textual response request.
    Text(PendingTextInput),
}

impl PendingInputKind {
    fn validate(&self) -> Result<(), PendingInputValidationError> {
        match self {
            Self::Choice(choice) => choice.validate(),
            Self::Text(text) => text.validate(),
        }
    }
}

/// Constraints for a choice-style input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingChoiceInput {
    /// Available user-visible choices.
    pub options: Vec<String>,
    /// True when multiple options may be selected.
    pub allow_multiple: bool,
    /// Minimum selected options required for a valid answer.
    pub min_choices: u8,
    /// Maximum selected options allowed for a valid answer.
    pub max_choices: u8,
}

const PENDING_CHOICE_MIN_OPTIONS: usize = 2;
const PENDING_CHOICE_MAX_OPTIONS: usize = 10;

impl PendingChoiceInput {
    fn validate(&self) -> Result<(), PendingInputValidationError> {
        if self.options.len() < PENDING_CHOICE_MIN_OPTIONS {
            return Err(PendingInputValidationError::ChoiceOptionsBelowMinimum {
                count: self.options.len(),
                minimum: PENDING_CHOICE_MIN_OPTIONS,
            });
        }

        if self.options.len() > PENDING_CHOICE_MAX_OPTIONS {
            return Err(PendingInputValidationError::ChoiceOptionsAboveMaximum {
                count: self.options.len(),
                maximum: PENDING_CHOICE_MAX_OPTIONS,
            });
        }

        let option_count = u8::try_from(self.options.len()).map_err(|_| {
            PendingInputValidationError::TooManyChoiceOptions {
                count: self.options.len(),
            }
        })?;

        for option in &self.options {
            if option.trim().is_empty() {
                return Err(PendingInputValidationError::EmptyChoiceOptionValue);
            }
        }

        for (index, left) in self.options.iter().enumerate() {
            if self
                .options
                .iter()
                .skip(index + 1)
                .any(|right| left == right)
            {
                return Err(PendingInputValidationError::DuplicateChoiceOptionValue(
                    left.clone(),
                ));
            }
        }

        if !self.allow_multiple {
            if self.min_choices != 1 || self.max_choices != 1 {
                return Err(PendingInputValidationError::SingleChoiceMustBeExactlyOne {
                    min_choices: self.min_choices,
                    max_choices: self.max_choices,
                });
            }
            return Ok(());
        }

        if self.min_choices == 0 {
            return Err(PendingInputValidationError::MinChoicesMustBePositive);
        }

        if self.min_choices > self.max_choices {
            return Err(PendingInputValidationError::InconsistentChoiceBounds {
                min_choices: self.min_choices,
                max_choices: self.max_choices,
            });
        }

        if self.max_choices > option_count {
            return Err(PendingInputValidationError::MaxChoicesExceedsOptions {
                max_choices: self.max_choices,
                options: option_count,
            });
        }

        Ok(())
    }
}

/// Constraints for a text-style input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingTextInput {
    /// Minimum input length in UTF-8 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u16>,
    /// Maximum input length in UTF-8 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u16>,
    /// True when multi-line responses are acceptable.
    pub multiline: bool,
}

impl PendingTextInput {
    fn validate(&self) -> Result<(), PendingInputValidationError> {
        if let Some(max_length) = self.max_length {
            if max_length == 0 {
                return Err(PendingInputValidationError::TextMaxLengthMustBePositive);
            }
        }

        if let (Some(min_length), Some(max_length)) = (self.min_length, self.max_length) {
            if min_length > max_length {
                return Err(PendingInputValidationError::InconsistentTextBounds {
                    min_length,
                    max_length,
                });
            }
        }

        Ok(())
    }
}

/// Errors returned by pending input payload validation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PendingInputValidationError {
    /// Request identifier cannot be empty.
    #[error("pending input request id cannot be empty")]
    EmptyRequestId,
    /// Prompt cannot be empty.
    #[error("pending input prompt cannot be empty")]
    EmptyPrompt,
    /// Choice input must contain at least the required minimum options.
    #[error("choice input must contain at least {minimum} options (got {count})")]
    ChoiceOptionsBelowMinimum {
        /// Number of options provided.
        count: usize,
        /// Minimum allowed option count.
        minimum: usize,
    },
    /// Choice input cannot exceed the supported maximum option count.
    #[error("choice input must contain at most {maximum} options (got {count})")]
    ChoiceOptionsAboveMaximum {
        /// Number of options provided.
        count: usize,
        /// Maximum allowed option count.
        maximum: usize,
    },
    /// Choice option value cannot be empty.
    #[error("choice option value cannot be empty")]
    EmptyChoiceOptionValue,
    /// Choice options must be unique.
    #[error("duplicate choice option value: {0}")]
    DuplicateChoiceOptionValue(String),
    /// Choice option count exceeds representable bounds for max choice constraints.
    #[error("choice option count is too large: {count}")]
    TooManyChoiceOptions {
        /// Number of options provided.
        count: usize,
    },
    /// Single-choice requests must enforce exactly one selection.
    #[error(
        "single-choice request must use min_choices=1 and max_choices=1 (got min={min_choices}, max={max_choices})"
    )]
    SingleChoiceMustBeExactlyOne {
        /// Minimum selected options required.
        min_choices: u8,
        /// Maximum selected options allowed.
        max_choices: u8,
    },
    /// Multi-select request must require at least one choice.
    #[error("min_choices must be positive")]
    MinChoicesMustBePositive,
    /// Min/max choice constraints are invalid.
    #[error("invalid choice bounds: min_choices={min_choices}, max_choices={max_choices}")]
    InconsistentChoiceBounds {
        /// Minimum selected options required.
        min_choices: u8,
        /// Maximum selected options allowed.
        max_choices: u8,
    },
    /// Max choices cannot exceed number of available options.
    #[error("max_choices={max_choices} exceeds options={options}")]
    MaxChoicesExceedsOptions {
        /// Maximum selected options allowed.
        max_choices: u8,
        /// Available options count.
        options: u8,
    },
    /// Max text length must be positive when configured.
    #[error("text max_length must be positive")]
    TextMaxLengthMustBePositive,
    /// Min/max text constraints are invalid.
    #[error("invalid text bounds: min_length={min_length}, max_length={max_length}")]
    InconsistentTextBounds {
        /// Minimum input length.
        min_length: u16,
        /// Maximum input length.
        max_length: u16,
    },
}

/// Contract for graceful stop requests separated from hard cancellation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopSignal {
    /// Desired stop behavior.
    pub mode: StopSignalMode,
    /// Safe point where the worker should stop and produce report.
    pub safe_point: StopSafePoint,
}

/// Supported graceful stop mode(s).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopSignalMode {
    /// Soft-stop execution and emit a partial report.
    StopAndReport,
}

/// Safe points where a graceful stop can be observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopSafePoint {
    /// Stop at loop boundary while task is actively running.
    LoopBoundary,
    /// Stop while task is paused waiting for user input.
    WaitingInput,
}

impl StopSafePoint {
    /// Return true when this safe point is valid for the observed task state.
    #[must_use]
    pub const fn supports_state(self, state: TaskState) -> bool {
        matches!(
            (self, state),
            (Self::LoopBoundary, TaskState::Running)
                | (Self::WaitingInput, TaskState::WaitingInput)
        )
    }
}

/// Transport-agnostic partial report produced by graceful stop flow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopReport {
    /// Human-readable summary generated from current task progress.
    pub summary: String,
    /// Safe point where stop was observed.
    pub safe_point: StopSafePoint,
    /// Task state observed when stop signal was handled.
    pub observed_state: TaskState,
}

impl StopReport {
    /// Validate report invariants required by graceful stop contract.
    pub fn validate(&self) -> Result<(), StopReportValidationError> {
        if self.summary.trim().is_empty() {
            return Err(StopReportValidationError::EmptySummary);
        }

        if self.observed_state.is_terminal() {
            return Err(StopReportValidationError::TerminalObservedState {
                state: self.observed_state,
            });
        }

        if !self.safe_point.supports_state(self.observed_state) {
            return Err(StopReportValidationError::InvalidSafePointForState {
                safe_point: self.safe_point,
                state: self.observed_state,
            });
        }

        Ok(())
    }
}

/// Errors returned by graceful stop report validation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum StopReportValidationError {
    /// Graceful stop report summary cannot be empty.
    #[error("stop report summary cannot be empty")]
    EmptySummary,
    /// Safe point is inconsistent with observed state.
    #[error("safe point {safe_point:?} is invalid for state {state:?}")]
    InvalidSafePointForState {
        /// Safe point captured in the report.
        safe_point: StopSafePoint,
        /// State observed at stop handling boundary.
        state: TaskState,
    },
    /// Report cannot be produced from a terminal state.
    #[error("graceful stop report cannot observe terminal state {state:?}")]
    TerminalObservedState {
        /// Invalid observed terminal state.
        state: TaskState,
    },
}

/// Schema version for persisted task snapshots.
pub const TASK_SNAPSHOT_SCHEMA_VERSION: u32 = 4;

/// Schema version for persisted task event logs.
pub const TASK_EVENT_LOG_SCHEMA_VERSION: u32 = 2;

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
    /// Owning session persisted for deterministic runtime recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Transport-agnostic task input payload.
    pub task: String,
    /// Latest persisted recovery checkpoint.
    pub checkpoint: TaskCheckpoint,
    /// Optional recovery note written when the runtime cannot safely resume execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_note: Option<String>,
    /// Optional pending HITL request persisted for restart-safe resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_input: Option<PendingInput>,
    /// Optional serialized agent memory captured at the pause boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_memory: Option<String>,
    /// Optional partial report emitted by graceful stop flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_report: Option<StopReport>,
}

impl TaskSnapshot {
    /// Create a new persisted snapshot for a task.
    #[must_use]
    pub fn new(
        metadata: TaskMetadata,
        session_id: SessionId,
        task: String,
        last_event_sequence: u64,
    ) -> Self {
        let checkpoint = TaskCheckpoint::new(metadata.state, last_event_sequence);

        Self {
            schema_version: TASK_SNAPSHOT_SCHEMA_VERSION,
            metadata,
            session_id: Some(session_id),
            task,
            checkpoint,
            recovery_note: None,
            pending_input: None,
            agent_memory: None,
            stop_report: None,
        }
    }

    /// Persist a copy of agent memory in this snapshot.
    pub fn set_agent_memory(&mut self, memory: &AgentMemory) -> Result<(), serde_json::Error> {
        self.agent_memory = Some(serde_json::to_string(memory)?);
        Ok(())
    }

    /// Restore agent memory persisted in this snapshot.
    pub fn parse_agent_memory(&self) -> Result<Option<AgentMemory>, serde_json::Error> {
        self.agent_memory
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
    }

    /// Validate snapshot invariants required for persistence.
    pub fn validate(&self) -> Result<(), TaskSnapshotValidationError> {
        match self.metadata.state {
            TaskState::WaitingInput => {
                let pending_input = self
                    .pending_input
                    .as_ref()
                    .ok_or(TaskSnapshotValidationError::MissingPendingInputForWaitingState)?;
                pending_input
                    .validate()
                    .map_err(TaskSnapshotValidationError::InvalidPendingInput)?;
                let _ = self
                    .agent_memory
                    .as_ref()
                    .ok_or(TaskSnapshotValidationError::MissingAgentMemoryForWaitingState)?;
                self.parse_agent_memory().map(|_| ()).map_err(|error| {
                    TaskSnapshotValidationError::InvalidAgentMemory(error.to_string())
                })?;

                if self.stop_report.is_some() {
                    Err(TaskSnapshotValidationError::UnexpectedStopReportForState {
                        state: TaskState::WaitingInput,
                    })
                } else {
                    Ok(())
                }
            }
            TaskState::Stopped => {
                let stop_report = self
                    .stop_report
                    .as_ref()
                    .ok_or(TaskSnapshotValidationError::MissingStopReportForStoppedState)?;
                stop_report
                    .validate()
                    .map_err(TaskSnapshotValidationError::InvalidStopReport)?;

                if self.pending_input.is_some() {
                    Err(
                        TaskSnapshotValidationError::UnexpectedPendingInputForState {
                            state: TaskState::Stopped,
                        },
                    )
                } else if self.agent_memory.is_some() {
                    Err(TaskSnapshotValidationError::UnexpectedAgentMemoryForState {
                        state: TaskState::Stopped,
                    })
                } else {
                    Ok(())
                }
            }
            state => {
                if self.pending_input.is_some() {
                    Err(TaskSnapshotValidationError::UnexpectedPendingInputForState { state })
                } else if self.agent_memory.is_some() {
                    Err(TaskSnapshotValidationError::UnexpectedAgentMemoryForState { state })
                } else if self.stop_report.is_some() {
                    Err(TaskSnapshotValidationError::UnexpectedStopReportForState { state })
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// Errors returned by task snapshot invariant checks.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TaskSnapshotValidationError {
    /// Waiting input state must always persist a pending input payload.
    #[error("waiting_input state requires pending_input payload")]
    MissingPendingInputForWaitingState,
    /// Pending input payload is only valid while waiting for input.
    #[error("state {state:?} cannot carry pending_input payload")]
    UnexpectedPendingInputForState {
        /// State that carried an unexpected pending payload.
        state: TaskState,
    },
    /// Pending payload does not pass validation constraints.
    #[error("invalid pending input payload: {0}")]
    InvalidPendingInput(PendingInputValidationError),
    /// Waiting input state must persist pre-pause memory.
    #[error("waiting_input state requires agent_memory payload")]
    MissingAgentMemoryForWaitingState,
    /// Agent memory payload is only valid while waiting for input.
    #[error("state {state:?} cannot carry agent_memory payload")]
    UnexpectedAgentMemoryForState {
        /// State that carried an unexpected agent memory payload.
        state: TaskState,
    },
    /// Agent memory payload cannot be deserialized.
    #[error("invalid agent memory payload: {0}")]
    InvalidAgentMemory(String),
    /// Stopped state must always persist a graceful stop report payload.
    #[error("stopped state requires stop_report payload")]
    MissingStopReportForStoppedState,
    /// Stop report payload is only valid for stopped state.
    #[error("state {state:?} cannot carry stop_report payload")]
    UnexpectedStopReportForState {
        /// State that carried an unexpected stop report payload.
        state: TaskState,
    },
    /// Stop report payload does not pass validation constraints.
    #[error("invalid stop report payload: {0}")]
    InvalidStopReport(StopReportValidationError),
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
    /// Graceful stop signal was accepted for safe-point handling.
    StopSignalReceived,
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
        PendingChoiceInput, PendingInput, PendingInputKind, PendingInputValidationError,
        PendingTextInput, StopReport, StopReportValidationError, StopSafePoint, StopSignal,
        StopSignalMode, TaskEvent, TaskEventKind, TaskId, TaskMetadata, TaskSnapshot,
        TaskSnapshotValidationError, TaskState, TaskStateTransitionError,
        TASK_EVENT_LOG_SCHEMA_VERSION, TASK_SNAPSHOT_SCHEMA_VERSION,
    };
    use crate::agent::{AgentMemory, SessionId};
    use chrono::Utc;

    #[test]
    fn task_state_reports_terminal_semantics() {
        assert!(TaskState::Completed.is_terminal());
        assert!(TaskState::Failed.is_terminal());
        assert!(TaskState::Cancelled.is_terminal());
        assert!(TaskState::Stopped.is_terminal());
        assert!(TaskState::Pending.is_non_terminal());
        assert!(TaskState::Running.is_non_terminal());
        assert!(TaskState::WaitingInput.is_non_terminal());
        assert!(!TaskState::Completed.is_non_terminal());
        assert!(!TaskState::Pending.is_active());
        assert!(TaskState::Running.is_active());
        assert!(!TaskState::WaitingInput.is_active());
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
            TaskState::WaitingInput,
            TaskState::Completed,
            TaskState::Failed,
            TaskState::Cancelled,
            TaskState::Stopped,
        ] {
            assert_eq!(TaskState::Running.validate_transition(next), Ok(()));
        }
    }

    #[test]
    fn task_state_allows_valid_transitions_from_waiting_input() {
        for next in [TaskState::Running, TaskState::Cancelled, TaskState::Stopped] {
            assert_eq!(TaskState::WaitingInput.validate_transition(next), Ok(()));
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
            (TaskState::WaitingInput, TaskState::Pending),
            (TaskState::WaitingInput, TaskState::WaitingInput),
            (TaskState::WaitingInput, TaskState::Completed),
            (TaskState::WaitingInput, TaskState::Failed),
            (TaskState::Pending, TaskState::Stopped),
            (TaskState::Completed, TaskState::Stopped),
            (TaskState::Failed, TaskState::Stopped),
            (TaskState::Cancelled, TaskState::Stopped),
            (TaskState::Stopped, TaskState::Pending),
            (TaskState::Stopped, TaskState::Running),
            (TaskState::Stopped, TaskState::WaitingInput),
            (TaskState::Stopped, TaskState::Completed),
            (TaskState::Stopped, TaskState::Failed),
            (TaskState::Stopped, TaskState::Cancelled),
            (TaskState::Stopped, TaskState::Stopped),
            (TaskState::Completed, TaskState::Pending),
            (TaskState::Completed, TaskState::Running),
            (TaskState::Completed, TaskState::WaitingInput),
            (TaskState::Completed, TaskState::Completed),
            (TaskState::Completed, TaskState::Failed),
            (TaskState::Completed, TaskState::Cancelled),
            (TaskState::Failed, TaskState::Pending),
            (TaskState::Failed, TaskState::Running),
            (TaskState::Failed, TaskState::WaitingInput),
            (TaskState::Failed, TaskState::Completed),
            (TaskState::Failed, TaskState::Failed),
            (TaskState::Failed, TaskState::Cancelled),
            (TaskState::Cancelled, TaskState::Pending),
            (TaskState::Cancelled, TaskState::Running),
            (TaskState::Cancelled, TaskState::WaitingInput),
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
        let session_id = SessionId::from(42);
        let snapshot =
            TaskSnapshot::new(metadata.clone(), session_id, "rebuild index".to_string(), 3);

        let json = serde_json::to_string(&snapshot);
        assert!(json.is_ok());

        let parsed: Result<TaskSnapshot, serde_json::Error> =
            serde_json::from_str(&json.unwrap_or_default());
        assert!(parsed.is_ok());

        let parsed = parsed.unwrap_or_else(|_| snapshot.clone());
        assert_eq!(parsed.schema_version, TASK_SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(parsed.metadata, metadata);
        assert_eq!(parsed.session_id, Some(session_id));
        assert_eq!(parsed.task, "rebuild index");
        assert_eq!(parsed.checkpoint.state, TaskState::Pending);
        assert_eq!(parsed.checkpoint.last_event_sequence, 3);
        assert_eq!(parsed.recovery_note, None);
        assert_eq!(parsed.pending_input, None);
        assert_eq!(parsed.stop_report, None);
    }

    #[test]
    fn task_snapshot_roundtrip_preserves_pending_input_payload() {
        let mut metadata = TaskMetadata::new();
        assert_eq!(metadata.transition_to(TaskState::Running), Ok(()));
        assert_eq!(metadata.transition_to(TaskState::WaitingInput), Ok(()));

        let mut snapshot =
            TaskSnapshot::new(metadata, SessionId::from(7), "collect logs".to_string(), 9);
        snapshot.pending_input = Some(PendingInput {
            request_id: "req-123".to_string(),
            prompt: "Choose data sources".to_string(),
            kind: PendingInputKind::Choice(PendingChoiceInput {
                options: vec![
                    "system".to_string(),
                    "application".to_string(),
                    "database".to_string(),
                ],
                allow_multiple: true,
                min_choices: 1,
                max_choices: 2,
            }),
        });
        let mut memory = AgentMemory::new(4_096);
        memory.add_message(crate::agent::memory::AgentMessage::user(
            "collect diagnostics",
        ));
        assert!(snapshot.set_agent_memory(&memory).is_ok());
        snapshot.checkpoint.state = TaskState::WaitingInput;

        let json = serde_json::to_string(&snapshot);
        assert!(json.is_ok());

        let parsed: Result<TaskSnapshot, serde_json::Error> =
            serde_json::from_str(&json.unwrap_or_default());
        assert!(parsed.is_ok());

        let parsed = parsed.unwrap_or(snapshot.clone());
        assert_eq!(parsed.metadata.state, TaskState::WaitingInput);
        assert_eq!(parsed.checkpoint.state, TaskState::WaitingInput);
        assert_eq!(parsed.pending_input, snapshot.pending_input);
        assert_eq!(parsed.agent_memory, snapshot.agent_memory);
        assert_eq!(parsed.stop_report, None);
        assert_eq!(parsed.validate(), Ok(()));
    }

    #[test]
    fn stop_signal_contract_is_transport_agnostic() {
        let signal = StopSignal {
            mode: StopSignalMode::StopAndReport,
            safe_point: StopSafePoint::LoopBoundary,
        };

        let json = serde_json::to_string(&signal);
        assert!(json.is_ok());

        let parsed: Result<StopSignal, serde_json::Error> =
            serde_json::from_str(&json.unwrap_or_default());
        assert!(parsed.is_ok());
        assert_eq!(parsed.unwrap_or(signal.clone()), signal);
    }

    #[test]
    fn stop_signal_safe_point_contract_is_explicit() {
        assert!(StopSafePoint::LoopBoundary.supports_state(TaskState::Running));
        assert!(!StopSafePoint::LoopBoundary.supports_state(TaskState::WaitingInput));
        assert!(StopSafePoint::WaitingInput.supports_state(TaskState::WaitingInput));
        assert!(!StopSafePoint::WaitingInput.supports_state(TaskState::Running));
        assert!(!StopSafePoint::WaitingInput.supports_state(TaskState::Cancelled));
    }

    #[test]
    fn stop_signal_report_validation_enforces_safe_point_rules() {
        let valid = StopReport {
            summary: "Collected logs and generated partial diagnostics".to_string(),
            safe_point: StopSafePoint::LoopBoundary,
            observed_state: TaskState::Running,
        };
        assert_eq!(valid.validate(), Ok(()));

        let invalid = StopReport {
            summary: "Partial report".to_string(),
            safe_point: StopSafePoint::LoopBoundary,
            observed_state: TaskState::WaitingInput,
        };
        assert_eq!(
            invalid.validate(),
            Err(StopReportValidationError::InvalidSafePointForState {
                safe_point: StopSafePoint::LoopBoundary,
                state: TaskState::WaitingInput,
            })
        );
    }

    #[test]
    fn stop_signal_report_validation_rejects_terminal_observed_state() {
        let report = StopReport {
            summary: "Should fail".to_string(),
            safe_point: StopSafePoint::LoopBoundary,
            observed_state: TaskState::Cancelled,
        };

        assert_eq!(
            report.validate(),
            Err(StopReportValidationError::TerminalObservedState {
                state: TaskState::Cancelled,
            })
        );
    }

    #[test]
    fn task_snapshot_validation_requires_stop_report_for_stopped_state() {
        let mut metadata = TaskMetadata::new();
        assert_eq!(metadata.transition_to(TaskState::Running), Ok(()));
        assert_eq!(metadata.transition_to(TaskState::Stopped), Ok(()));

        let snapshot = TaskSnapshot::new(metadata, SessionId::from(15), "collect".to_string(), 1);

        assert_eq!(
            snapshot.validate(),
            Err(TaskSnapshotValidationError::MissingStopReportForStoppedState)
        );
    }

    #[test]
    fn task_snapshot_validation_accepts_stop_report_for_stopped_state() {
        let mut metadata = TaskMetadata::new();
        assert_eq!(metadata.transition_to(TaskState::Running), Ok(()));
        assert_eq!(metadata.transition_to(TaskState::Stopped), Ok(()));

        let mut snapshot =
            TaskSnapshot::new(metadata, SessionId::from(16), "collect".to_string(), 2);
        snapshot.stop_report = Some(StopReport {
            summary: "Collected partial data before graceful stop".to_string(),
            safe_point: StopSafePoint::LoopBoundary,
            observed_state: TaskState::Running,
        });

        assert_eq!(snapshot.validate(), Ok(()));
    }

    #[test]
    fn task_snapshot_validation_rejects_stop_report_outside_stopped_state() {
        let metadata = TaskMetadata::new();
        let mut snapshot =
            TaskSnapshot::new(metadata, SessionId::from(17), "collect".to_string(), 3);
        snapshot.stop_report = Some(StopReport {
            summary: "Partial".to_string(),
            safe_point: StopSafePoint::LoopBoundary,
            observed_state: TaskState::Running,
        });

        assert_eq!(
            snapshot.validate(),
            Err(TaskSnapshotValidationError::UnexpectedStopReportForState {
                state: TaskState::Pending,
            })
        );
    }

    #[test]
    fn task_snapshot_validation_rejects_waiting_input_without_pending_input() {
        let mut metadata = TaskMetadata::new();
        assert_eq!(metadata.transition_to(TaskState::Running), Ok(()));
        assert_eq!(metadata.transition_to(TaskState::WaitingInput), Ok(()));

        let snapshot =
            TaskSnapshot::new(metadata, SessionId::from(10), "collect data".to_string(), 1);

        assert_eq!(
            snapshot.validate(),
            Err(TaskSnapshotValidationError::MissingPendingInputForWaitingState)
        );
    }

    #[test]
    fn task_snapshot_validation_rejects_pending_input_outside_waiting_state() {
        let metadata = TaskMetadata::new();
        let mut snapshot =
            TaskSnapshot::new(metadata, SessionId::from(10), "collect".to_string(), 1);
        snapshot.pending_input = Some(PendingInput {
            request_id: "req-1".to_string(),
            prompt: "Choose one".to_string(),
            kind: PendingInputKind::Choice(PendingChoiceInput {
                options: vec!["a".to_string(), "b".to_string()],
                allow_multiple: false,
                min_choices: 1,
                max_choices: 1,
            }),
        });

        assert_eq!(
            snapshot.validate(),
            Err(
                TaskSnapshotValidationError::UnexpectedPendingInputForState {
                    state: TaskState::Pending,
                }
            )
        );
    }

    #[test]
    fn task_snapshot_validation_rejects_waiting_input_without_agent_memory() {
        let mut metadata = TaskMetadata::new();
        assert_eq!(metadata.transition_to(TaskState::Running), Ok(()));
        assert_eq!(metadata.transition_to(TaskState::WaitingInput), Ok(()));

        let mut snapshot =
            TaskSnapshot::new(metadata, SessionId::from(12), "collect".to_string(), 1);
        snapshot.pending_input = Some(PendingInput {
            request_id: "req-1".to_string(),
            prompt: "Provide confirmation".to_string(),
            kind: PendingInputKind::Text(PendingTextInput {
                min_length: Some(1),
                max_length: Some(64),
                multiline: false,
            }),
        });

        assert_eq!(
            snapshot.validate(),
            Err(TaskSnapshotValidationError::MissingAgentMemoryForWaitingState)
        );
    }

    #[test]
    fn task_snapshot_validation_rejects_agent_memory_outside_waiting_state() {
        let metadata = TaskMetadata::new();
        let mut snapshot =
            TaskSnapshot::new(metadata, SessionId::from(13), "collect".to_string(), 1);
        let mut memory = AgentMemory::new(4_096);
        memory.add_message(crate::agent::memory::AgentMessage::assistant("done"));
        assert!(snapshot.set_agent_memory(&memory).is_ok());

        assert_eq!(
            snapshot.validate(),
            Err(TaskSnapshotValidationError::UnexpectedAgentMemoryForState {
                state: TaskState::Pending,
            })
        );
    }

    #[test]
    fn task_snapshot_validation_rejects_invalid_agent_memory_payload() {
        let mut metadata = TaskMetadata::new();
        assert_eq!(metadata.transition_to(TaskState::Running), Ok(()));
        assert_eq!(metadata.transition_to(TaskState::WaitingInput), Ok(()));

        let mut snapshot =
            TaskSnapshot::new(metadata, SessionId::from(14), "collect".to_string(), 1);
        snapshot.pending_input = Some(PendingInput {
            request_id: "req-3".to_string(),
            prompt: "Provide confirmation".to_string(),
            kind: PendingInputKind::Text(PendingTextInput {
                min_length: Some(1),
                max_length: Some(64),
                multiline: false,
            }),
        });
        snapshot.agent_memory = Some("{not-json".to_string());

        assert!(matches!(
            snapshot.validate(),
            Err(TaskSnapshotValidationError::InvalidAgentMemory(_))
        ));
    }

    #[test]
    fn task_snapshot_validation_rejects_invalid_pending_input_payload() {
        let mut metadata = TaskMetadata::new();
        assert_eq!(metadata.transition_to(TaskState::Running), Ok(()));
        assert_eq!(metadata.transition_to(TaskState::WaitingInput), Ok(()));

        let mut snapshot =
            TaskSnapshot::new(metadata, SessionId::from(11), "collect".to_string(), 2);
        snapshot.pending_input = Some(PendingInput {
            request_id: "req-2".to_string(),
            prompt: "Pick".to_string(),
            kind: PendingInputKind::Choice(PendingChoiceInput {
                options: vec!["only".to_string()],
                allow_multiple: true,
                min_choices: 2,
                max_choices: 2,
            }),
        });

        assert_eq!(
            snapshot.validate(),
            Err(TaskSnapshotValidationError::InvalidPendingInput(
                PendingInputValidationError::ChoiceOptionsBelowMinimum {
                    count: 1,
                    minimum: 2,
                },
            ))
        );
    }

    #[test]
    fn task_snapshot_deserializes_pre_slice_payload_without_pending_input() {
        let snapshot_json = r#"{
            "schema_version": 2,
            "metadata": {
                "id": "11111111-1111-4111-8111-111111111111",
                "state": "running",
                "created_at": "2026-03-07T00:00:00Z",
                "updated_at": "2026-03-07T00:00:00Z"
            },
            "session_id": 42,
            "task": "legacy snapshot",
            "checkpoint": {
                "schema_version": 2,
                "state": "running",
                "last_event_sequence": 5,
                "persisted_at": "2026-03-07T00:00:00Z"
            },
            "recovery_note": null
        }"#;

        let parsed: Result<TaskSnapshot, serde_json::Error> = serde_json::from_str(snapshot_json);
        assert!(parsed.is_ok());
        let parsed = parsed.unwrap_or_else(|_| {
            TaskSnapshot::new(
                TaskMetadata::new(),
                SessionId::from(42),
                "fallback".to_string(),
                0,
            )
        });
        assert_eq!(parsed.schema_version, 2);
        assert_eq!(parsed.pending_input, None);
        assert_eq!(parsed.metadata.state, TaskState::Running);
        assert_eq!(parsed.validate(), Ok(()));
    }

    #[test]
    fn task_event_deserializes_pre_slice_payload_with_legacy_schema_version() {
        let event_json = r#"{
            "schema_version": 1,
            "task_id": "11111111-1111-4111-8111-111111111111",
            "sequence": 1,
            "kind": "state_changed",
            "state": "running",
            "message": "legacy",
            "recorded_at": "2026-03-07T00:00:00Z"
        }"#;

        let parsed: Result<TaskEvent, serde_json::Error> = serde_json::from_str(event_json);
        assert!(parsed.is_ok());
        let parsed = parsed.unwrap_or_else(|_| {
            TaskEvent::new(
                TaskId::new(),
                1,
                TaskEventKind::StateChanged,
                TaskState::Running,
                Some("fallback".to_string()),
            )
        });
        assert_eq!(parsed.schema_version, 1);
        assert_eq!(parsed.state, TaskState::Running);
    }

    #[test]
    fn pending_input_choice_validation_accepts_valid_payload() {
        let input = PendingInput {
            request_id: "choice-1".to_string(),
            prompt: "Select reports to generate".to_string(),
            kind: PendingInputKind::Choice(PendingChoiceInput {
                options: vec![
                    "daily".to_string(),
                    "weekly".to_string(),
                    "monthly".to_string(),
                ],
                allow_multiple: true,
                min_choices: 1,
                max_choices: 2,
            }),
        };

        assert_eq!(input.validate(), Ok(()));
    }

    #[test]
    fn pending_input_choice_validation_rejects_invalid_payloads() {
        let too_few_options = PendingInput {
            request_id: "choice-too-few".to_string(),
            prompt: "Select one".to_string(),
            kind: PendingInputKind::Choice(PendingChoiceInput {
                options: vec!["only".to_string()],
                allow_multiple: false,
                min_choices: 1,
                max_choices: 1,
            }),
        };
        assert_eq!(
            too_few_options.validate(),
            Err(PendingInputValidationError::ChoiceOptionsBelowMinimum {
                count: 1,
                minimum: 2,
            })
        );

        let too_many_options = PendingInput {
            request_id: "choice-too-many".to_string(),
            prompt: "Select targets".to_string(),
            kind: PendingInputKind::Choice(PendingChoiceInput {
                options: (1..=11).map(|index| format!("option-{index}")).collect(),
                allow_multiple: true,
                min_choices: 1,
                max_choices: 3,
            }),
        };
        assert_eq!(
            too_many_options.validate(),
            Err(PendingInputValidationError::ChoiceOptionsAboveMaximum {
                count: 11,
                maximum: 10,
            })
        );

        let invalid_bounds = PendingInput {
            request_id: "choice-2".to_string(),
            prompt: "Select items".to_string(),
            kind: PendingInputKind::Choice(PendingChoiceInput {
                options: vec!["a".to_string(), "b".to_string()],
                allow_multiple: true,
                min_choices: 2,
                max_choices: 1,
            }),
        };
        assert_eq!(
            invalid_bounds.validate(),
            Err(PendingInputValidationError::InconsistentChoiceBounds {
                min_choices: 2,
                max_choices: 1,
            })
        );

        let duplicate_option = PendingInput {
            request_id: "choice-3".to_string(),
            prompt: "Select source".to_string(),
            kind: PendingInputKind::Choice(PendingChoiceInput {
                options: vec!["db".to_string(), "db".to_string()],
                allow_multiple: false,
                min_choices: 1,
                max_choices: 1,
            }),
        };
        assert_eq!(
            duplicate_option.validate(),
            Err(PendingInputValidationError::DuplicateChoiceOptionValue(
                "db".to_string(),
            ))
        );

        let invalid_single = PendingInput {
            request_id: "choice-4".to_string(),
            prompt: "Pick exactly one".to_string(),
            kind: PendingInputKind::Choice(PendingChoiceInput {
                options: vec!["x".to_string(), "y".to_string()],
                allow_multiple: false,
                min_choices: 1,
                max_choices: 2,
            }),
        };
        assert_eq!(
            invalid_single.validate(),
            Err(PendingInputValidationError::SingleChoiceMustBeExactlyOne {
                min_choices: 1,
                max_choices: 2,
            })
        );
    }

    #[test]
    fn pending_input_text_validation_handles_bounds() {
        let valid = PendingInput {
            request_id: "text-1".to_string(),
            prompt: "Describe expected output".to_string(),
            kind: PendingInputKind::Text(PendingTextInput {
                min_length: Some(5),
                max_length: Some(200),
                multiline: true,
            }),
        };
        assert_eq!(valid.validate(), Ok(()));

        let invalid = PendingInput {
            request_id: "text-2".to_string(),
            prompt: "Describe expected output".to_string(),
            kind: PendingInputKind::Text(PendingTextInput {
                min_length: Some(300),
                max_length: Some(200),
                multiline: true,
            }),
        };
        assert_eq!(
            invalid.validate(),
            Err(PendingInputValidationError::InconsistentTextBounds {
                min_length: 300,
                max_length: 200,
            })
        );
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
