//! Typed terminal output for one tool invocation.

use super::artifacts::ArtifactRef;
use super::types::{ToolCallId, ToolName};
use crate::llm::{InvocationId, ProviderToolCallId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Required terminal states for a tool invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutputStatus {
    /// Tool completed successfully.
    Success,
    /// Tool completed but failed at the application/process level.
    Failure,
    /// Per-tool hard timeout fired.
    Timeout,
    /// User/runtime cancellation won the terminal race.
    Cancelled,
    /// Soft hung detector fired before hard timeout.
    HungTimeout,
    /// Cleanup failure dominates the terminal state.
    ProcessCleanupFailed,
    /// Tool arguments could not be parsed or validated.
    InvalidArguments,
    /// Tool name was not registered.
    UnknownTool,
    /// Provider returned malformed tool-call protocol data.
    ProviderProtocolError,
    /// Runtime task panicked, join failed, or another internal invariant broke.
    InternalRuntimeError,
}

impl ToolOutputStatus {
    /// String used in model-facing JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::HungTimeout => "hung_timeout",
            Self::ProcessCleanupFailed => "process_cleanup_failed",
            Self::InvalidArguments => "invalid_arguments",
            Self::UnknownTool => "unknown_tool",
            Self::ProviderProtocolError => "provider_protocol_error",
            Self::InternalRuntimeError => "internal_runtime_error",
        }
    }

    /// Whether the terminal state is successful.
    #[must_use]
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }
}

/// Timeout or hung reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeoutReason {
    /// Per-tool hard timeout fired.
    PerToolHardTimeout,
    /// Optional batch timeout fired.
    BatchTimeout,
    /// Hung detector saw no output for too long.
    HungNoOutput,
    /// Hung detector saw no progress for too long.
    HungNoProgress,
    /// Outer agent timeout overlapped tool cleanup.
    AgentTimeoutOverlap,
}

/// Cancellation source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationReason {
    /// User cancelled the current turn.
    User,
    /// Runtime/session shutdown cancelled the call.
    RuntimeShutdown,
    /// Batch-level cancellation cancelled the call.
    BatchCancelled,
}

/// Process cleanup outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupStatus {
    /// Cleanup was not required.
    NotNeeded,
    /// Process never started.
    NotStarted,
    /// Process already exited before cleanup.
    AlreadyExited,
    /// Process group terminated after graceful signal.
    TerminatedGracefully,
    /// Process group was killed.
    KilledProcessGroup,
    /// Individual process was killed.
    KilledProcess,
    /// Remote cleanup was best-effort and cannot be proven.
    BestEffortRemoteCleanup,
    /// Cleanup failed.
    Failed,
}

/// Model-facing preview for one output stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OutputPreview {
    /// Complete inline text when not truncated and not binary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Head preview when truncated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Tail preview when truncated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail: Option<String>,
    /// Bytes captured into the preview.
    pub bytes_captured: usize,
    /// Known total bytes, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_total_known: Option<usize>,
    /// Whether stream content was truncated.
    pub truncated: bool,
    /// Whether stream was detected as binary and omitted from inline text.
    pub binary: bool,
    /// Optional artifact for full stream content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactRef>,
}

impl OutputPreview {
    /// Empty non-truncated preview.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            text: Some(String::new()),
            head: None,
            tail: None,
            bytes_captured: 0,
            bytes_total_known: Some(0),
            truncated: false,
            binary: false,
            artifact: None,
        }
    }
}

/// Summary of output truncation decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputTruncationMetadata {
    /// Configured stdout budget.
    pub max_stdout_bytes: usize,
    /// Configured stderr budget.
    pub max_stderr_bytes: usize,
    /// Configured model content budget.
    pub max_tool_output_content_bytes: usize,
    /// Whether stdout was truncated.
    pub stdout_truncated: bool,
    /// Whether stderr was truncated.
    pub stderr_truncated: bool,
    /// Whether final provider content was truncated.
    pub content_truncated: bool,
    /// Whether artifact writing failed.
    pub artifact_write_failed: bool,
}

impl OutputTruncationMetadata {
    /// Build metadata for an output with no truncation.
    #[must_use]
    pub const fn new(
        max_stdout_bytes: usize,
        max_stderr_bytes: usize,
        max_tool_output_content_bytes: usize,
    ) -> Self {
        Self {
            max_stdout_bytes,
            max_stderr_bytes,
            max_tool_output_content_bytes,
            stdout_truncated: false,
            stderr_truncated: false,
            content_truncated: false,
            artifact_write_failed: false,
        }
    }
}

/// Stable identity fields shared by output normalizer and history writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutputIdentity {
    /// Provider-visible tool call id used for history pairing.
    pub tool_call_id: ToolCallId,
    /// Provider-owned id when distinct from local id.
    pub provider_tool_call_id: Option<ProviderToolCallId>,
    /// Internal runtime invocation id.
    pub invocation_id: InvocationId,
    /// Exact tool name.
    pub tool_name: ToolName,
    /// Original assistant batch index.
    pub batch_index: usize,
}

/// Canonical terminal output for one tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    /// Provider-visible tool call id used for history pairing.
    pub tool_call_id: ToolCallId,
    /// Provider-owned id when distinct from local id.
    pub provider_tool_call_id: Option<ProviderToolCallId>,
    /// Internal runtime invocation id.
    pub invocation_id: InvocationId,
    /// Exact tool name.
    pub tool_name: ToolName,
    /// Original assistant batch index.
    pub batch_index: usize,
    /// Terminal status.
    pub status: ToolOutputStatus,
    /// Success flag derived from terminal status unless executor overrides later.
    pub success: bool,
    /// Optional process exit code.
    pub exit_code: Option<i32>,
    /// stdout preview.
    pub stdout: OutputPreview,
    /// stderr preview.
    pub stderr: OutputPreview,
    /// Optional structured payload.
    pub structured_payload: Option<Value>,
    /// Concise error message.
    pub error_message: Option<String>,
    /// Timeout/hung reason.
    pub timeout_reason: Option<TimeoutReason>,
    /// Cancellation reason.
    pub cancellation_reason: Option<CancellationReason>,
    /// Cleanup status.
    pub cleanup_status: CleanupStatus,
    /// Start timestamp.
    pub started_at: DateTime<Utc>,
    /// End timestamp.
    pub ended_at: DateTime<Utc>,
    /// Truncation metadata.
    pub truncation: OutputTruncationMetadata,
    /// Artifact references for large/binary outputs.
    pub artifacts: Vec<ArtifactRef>,
}

impl ToolOutput {
    /// Create a terminal output with empty stdout/stderr and default metadata.
    #[must_use]
    pub fn terminal(
        identity: ToolOutputIdentity,
        status: ToolOutputStatus,
        started_at: DateTime<Utc>,
        ended_at: DateTime<Utc>,
        truncation: OutputTruncationMetadata,
    ) -> Self {
        Self {
            tool_call_id: identity.tool_call_id,
            provider_tool_call_id: identity.provider_tool_call_id,
            invocation_id: identity.invocation_id,
            tool_name: identity.tool_name,
            batch_index: identity.batch_index,
            status,
            success: status.is_success(),
            exit_code: None,
            stdout: OutputPreview::empty(),
            stderr: OutputPreview::empty(),
            structured_payload: None,
            error_message: None,
            timeout_reason: None,
            cancellation_reason: None,
            cleanup_status: CleanupStatus::NotNeeded,
            started_at,
            ended_at,
            truncation,
            artifacts: Vec::new(),
        }
    }

    /// Attach a concise error message.
    #[must_use]
    pub fn with_error_message(mut self, message: impl Into<String>) -> Self {
        self.error_message = Some(message.into());
        self
    }

    /// Attach a timeout reason.
    #[must_use]
    pub fn with_timeout_reason(mut self, reason: TimeoutReason) -> Self {
        self.timeout_reason = Some(reason);
        self
    }

    /// Attach a cancellation reason.
    #[must_use]
    pub fn with_cancellation_reason(mut self, reason: CancellationReason) -> Self {
        self.cancellation_reason = Some(reason);
        self
    }

    /// Attach cleanup status.
    #[must_use]
    pub fn with_cleanup_status(mut self, cleanup_status: CleanupStatus) -> Self {
        self.cleanup_status = cleanup_status;
        self
    }

    /// Attach stdout/stderr previews.
    #[must_use]
    pub fn with_streams(mut self, stdout: OutputPreview, stderr: OutputPreview) -> Self {
        self.truncation.stdout_truncated = stdout.truncated;
        self.truncation.stderr_truncated = stderr.truncated;
        self.stdout = stdout;
        self.stderr = stderr;
        self
    }

    /// Attach a process exit code.
    #[must_use]
    pub fn with_exit_code(mut self, exit_code: i32) -> Self {
        self.exit_code = Some(exit_code);
        self
    }

    /// Duration in milliseconds, saturating at zero for skewed clocks.
    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        let duration = self.ended_at.signed_duration_since(self.started_at);
        u64::try_from(duration.num_milliseconds()).unwrap_or(0)
    }

    /// Provider/model-facing JSON value.
    #[must_use]
    pub fn model_content_value(&self) -> Value {
        json!({
            "tool_call_id": self.tool_call_id.as_str(),
            "tool_name": self.tool_name.as_str(),
            "status": self.status.as_str(),
            "success": self.success,
            "duration_ms": self.duration_ms(),
            "exit_code": self.exit_code,
            "stdout": self.stdout,
            "stderr": self.stderr,
            "structured_payload": self.structured_payload,
            "error_message": self.error_message,
            "timeout_reason": self.timeout_reason,
            "cancellation_reason": self.cancellation_reason,
            "cleanup_status": self.cleanup_status,
            "truncation": self.truncation,
            "artifacts": self.artifacts,
        })
    }

    /// Encode the provider/model-facing tool output as a compact JSON string.
    ///
    /// # Errors
    ///
    /// Returns a serialization error if an attached payload cannot be encoded.
    pub fn encode_model_content(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.model_content_value())
    }
}
