//! Normalization helpers for converting runtime failures into tool outputs.

use super::config::ToolRuntimeConfig;
use super::invocation::ToolInvocation;
use super::output::{
    CancellationReason, CleanupStatus, OutputPreview, OutputTruncationMetadata, TimeoutReason,
    ToolOutput, ToolOutputStatus,
};
use chrono::{DateTime, Utc};
use thiserror::Error;

/// Error returned by typed executors before the runtime converts it to output.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ToolRuntimeError {
    /// Tool arguments are invalid.
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    /// Provider protocol is malformed but pairable.
    #[error("provider protocol error: {0}")]
    ProviderProtocol(String),
    /// Tool failed at the application/process level.
    #[error("tool execution failed: {0}")]
    Failure(String),
    /// Runtime-internal failure.
    #[error("internal runtime error: {0}")]
    Internal(String),
}

/// Converts terminal states and errors into provider-valid `ToolOutput`s.
#[derive(Debug, Clone)]
pub struct OutputNormalizer {
    config: ToolRuntimeConfig,
}

impl OutputNormalizer {
    /// Create a normalizer with runtime budgets.
    #[must_use]
    pub fn new(config: ToolRuntimeConfig) -> Self {
        Self { config }
    }

    /// Borrow the runtime config.
    #[must_use]
    pub fn config(&self) -> &ToolRuntimeConfig {
        &self.config
    }

    /// Normalize successful executor output streams.
    #[must_use]
    pub fn success(&self, invocation: &ToolInvocation, stdout: &str, stderr: &str) -> ToolOutput {
        self.base_output(invocation, ToolOutputStatus::Success, Utc::now())
            .with_streams(self.stdout_preview(stdout), self.stderr_preview(stderr))
    }

    /// Normalize application/process failure.
    #[must_use]
    pub fn failure(&self, invocation: &ToolInvocation, message: impl Into<String>) -> ToolOutput {
        self.base_output(invocation, ToolOutputStatus::Failure, Utc::now())
            .with_error_message(message)
            .with_cleanup_status(CleanupStatus::NotNeeded)
    }

    /// Normalize a pre-execution policy block without calling an executor.
    #[must_use]
    pub fn policy_blocked(
        &self,
        invocation: &ToolInvocation,
        reason: impl Into<String>,
    ) -> ToolOutput {
        self.base_output(invocation, ToolOutputStatus::Failure, Utc::now())
            .with_error_message(reason)
            .with_cleanup_status(CleanupStatus::NotStarted)
    }

    /// Normalize unknown tool lookup.
    #[must_use]
    pub fn unknown_tool(
        &self,
        invocation: &ToolInvocation,
        available_tools: &[String],
    ) -> ToolOutput {
        let available = if available_tools.is_empty() {
            "no tools are registered".to_string()
        } else {
            format!("available tools: {}", available_tools.join(", "))
        };

        self.base_output(invocation, ToolOutputStatus::UnknownTool, Utc::now())
            .with_error_message(format!(
                "unknown tool '{}'; {available}",
                invocation.tool_name
            ))
            .with_cleanup_status(CleanupStatus::NotStarted)
    }

    /// Normalize invalid arguments without calling an executor.
    #[must_use]
    pub fn invalid_arguments(
        &self,
        invocation: &ToolInvocation,
        message: impl Into<String>,
    ) -> ToolOutput {
        self.base_output(invocation, ToolOutputStatus::InvalidArguments, Utc::now())
            .with_error_message(message)
            .with_cleanup_status(CleanupStatus::NotStarted)
    }

    /// Normalize provider protocol mismatch.
    #[must_use]
    pub fn provider_protocol_error(
        &self,
        invocation: &ToolInvocation,
        message: impl Into<String>,
    ) -> ToolOutput {
        self.base_output(
            invocation,
            ToolOutputStatus::ProviderProtocolError,
            Utc::now(),
        )
        .with_error_message(message)
        .with_cleanup_status(CleanupStatus::NotStarted)
    }

    /// Normalize per-tool hard timeout.
    #[must_use]
    pub fn timeout(
        &self,
        invocation: &ToolInvocation,
        cleanup_status: CleanupStatus,
    ) -> ToolOutput {
        self.base_output(invocation, ToolOutputStatus::Timeout, Utc::now())
            .with_error_message(format!(
                "tool exceeded hard timeout of {}s",
                invocation.timeout.per_tool_hard_timeout.as_secs()
            ))
            .with_timeout_reason(TimeoutReason::PerToolHardTimeout)
            .with_cleanup_status(cleanup_status)
    }

    /// Normalize user/runtime cancellation.
    #[must_use]
    pub fn cancelled(
        &self,
        invocation: &ToolInvocation,
        reason: CancellationReason,
        cleanup_status: CleanupStatus,
    ) -> ToolOutput {
        self.base_output(invocation, ToolOutputStatus::Cancelled, Utc::now())
            .with_error_message("tool invocation was cancelled")
            .with_cancellation_reason(reason)
            .with_cleanup_status(cleanup_status)
    }

    /// Normalize a hung detector terminal state.
    #[must_use]
    pub fn hung_timeout(
        &self,
        invocation: &ToolInvocation,
        reason: TimeoutReason,
        cleanup_status: CleanupStatus,
    ) -> ToolOutput {
        self.base_output(invocation, ToolOutputStatus::HungTimeout, Utc::now())
            .with_error_message("tool invocation was considered hung")
            .with_timeout_reason(reason)
            .with_cleanup_status(cleanup_status)
    }

    /// Normalize an executor panic, join error, or internal runtime fault.
    #[must_use]
    pub fn internal_runtime_error(
        &self,
        invocation: &ToolInvocation,
        message: impl Into<String>,
    ) -> ToolOutput {
        self.base_output(
            invocation,
            ToolOutputStatus::InternalRuntimeError,
            Utc::now(),
        )
        .with_error_message(message)
    }

    /// Normalize typed executor errors.
    #[must_use]
    pub fn executor_error(
        &self,
        invocation: &ToolInvocation,
        error: ToolRuntimeError,
    ) -> ToolOutput {
        match error {
            ToolRuntimeError::InvalidArguments(message) => {
                self.invalid_arguments(invocation, message)
            }
            ToolRuntimeError::ProviderProtocol(message) => {
                self.provider_protocol_error(invocation, message)
            }
            ToolRuntimeError::Failure(message) => self.failure(invocation, message),
            ToolRuntimeError::Internal(message) => self.internal_runtime_error(invocation, message),
        }
    }

    /// Build a preview using stdout budget.
    #[must_use]
    pub fn stdout_preview(&self, text: &str) -> OutputPreview {
        self.preview_text(text, self.config.output.max_captured_stdout_bytes)
    }

    /// Build a preview using stderr budget.
    #[must_use]
    pub fn stderr_preview(&self, text: &str) -> OutputPreview {
        self.preview_text(text, self.config.output.max_captured_stderr_bytes)
    }

    fn base_output(
        &self,
        invocation: &ToolInvocation,
        status: ToolOutputStatus,
        ended_at: DateTime<Utc>,
    ) -> ToolOutput {
        ToolOutput::terminal(
            invocation.output_identity(),
            status,
            invocation.effective_started_at(),
            ended_at,
            self.truncation_metadata(),
        )
    }

    fn truncation_metadata(&self) -> OutputTruncationMetadata {
        OutputTruncationMetadata::new(
            self.config.output.max_captured_stdout_bytes,
            self.config.output.max_captured_stderr_bytes,
            self.config.output.max_tool_output_content_bytes,
        )
    }

    fn preview_text(&self, text: &str, max_bytes: usize) -> OutputPreview {
        if text.len() <= max_bytes {
            return OutputPreview {
                text: Some(text.to_string()),
                bytes_captured: text.len(),
                bytes_total_known: Some(text.len()),
                ..OutputPreview::default()
            };
        }

        let head = take_head_by_bytes(text, self.config.output.output_head_bytes);
        let tail = take_tail_by_bytes(text, self.config.output.output_tail_bytes);

        OutputPreview {
            text: None,
            bytes_captured: head.len() + tail.len(),
            bytes_total_known: Some(text.len()),
            head: Some(head),
            tail: Some(tail),
            truncated: true,
            binary: false,
            artifact: None,
        }
    }
}

fn take_head_by_bytes(text: &str, max_bytes: usize) -> String {
    let end = boundary_forward(text, max_bytes.min(text.len()));
    text[..end].to_string()
}

fn take_tail_by_bytes(text: &str, max_bytes: usize) -> String {
    let start_hint = text.len().saturating_sub(max_bytes);
    let start = boundary_backward(text, start_hint);
    text[start..].to_string()
}

fn boundary_forward(text: &str, mut end: usize) -> usize {
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

fn boundary_backward(text: &str, mut start: usize) -> usize {
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::config::{ToolOutputBudget, ToolTimeoutConfig};
    use crate::agent::tool_runtime::invocation::{
        EnvironmentMetadata, ModelMetadata, ProviderMetadata, ToolExecutionContext,
    };
    use crate::agent::tool_runtime::types::{ToolBatchId, ToolCallId, ToolName, TurnId};
    use crate::llm::InvocationId;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn success_output_encodes_compact_json_content() {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig::default());
        let invocation = test_invocation("call_success");

        let output = normalizer.success(&invocation, "ok", "");
        let content = output.encode_model_content().expect("valid JSON");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("JSON object");

        assert_eq!(parsed["tool_call_id"], "call_success");
        assert_eq!(parsed["tool_name"], "execute_command");
        assert_eq!(parsed["status"], "success");
        assert!(parsed["success"].as_bool().unwrap_or(false));
        assert_eq!(parsed["stdout"]["text"], "ok");
    }

    #[test]
    fn unknown_tool_normalizes_to_paired_output() {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig::default());
        let invocation = test_invocation("call_unknown");

        let output = normalizer.unknown_tool(&invocation, &["read_file".to_string()]);

        assert_eq!(output.tool_call_id.as_str(), "call_unknown");
        assert_eq!(output.status, ToolOutputStatus::UnknownTool);
        assert!(!output.success);
        assert_eq!(output.cleanup_status, CleanupStatus::NotStarted);
    }

    #[test]
    fn long_stdout_is_split_into_head_and_tail() {
        let config = ToolRuntimeConfig {
            output: ToolOutputBudget {
                max_captured_stdout_bytes: 10,
                output_head_bytes: 4,
                output_tail_bytes: 3,
                ..ToolOutputBudget::default()
            },
            ..ToolRuntimeConfig::default()
        };
        let normalizer = OutputNormalizer::new(config);

        let preview = normalizer.stdout_preview("0123456789abcdef");

        assert!(preview.truncated);
        assert_eq!(preview.text, None);
        assert_eq!(preview.head.as_deref(), Some("0123"));
        assert_eq!(preview.tail.as_deref(), Some("def"));
        assert_eq!(preview.bytes_total_known, Some(16));
    }

    #[test]
    fn executor_error_maps_every_required_error_class() {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig::default());
        let invocation = test_invocation("call_error");

        let cases = [
            (
                ToolRuntimeError::InvalidArguments("bad json".into()),
                ToolOutputStatus::InvalidArguments,
            ),
            (
                ToolRuntimeError::ProviderProtocol("missing id".into()),
                ToolOutputStatus::ProviderProtocolError,
            ),
            (
                ToolRuntimeError::Failure("exit 1".into()),
                ToolOutputStatus::Failure,
            ),
            (
                ToolRuntimeError::Internal("join failed".into()),
                ToolOutputStatus::InternalRuntimeError,
            ),
        ];

        for (error, status) in cases {
            let output = normalizer.executor_error(&invocation, error);
            assert_eq!(output.status, status);
            assert_eq!(output.tool_call_id.as_str(), "call_error");
        }
    }

    fn test_invocation(tool_call_id: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(42),
            turn_id: TurnId::from("turn_1"),
            batch_id: ToolBatchId::from("batch_1"),
            batch_index: 0,
            invocation_id: InvocationId::from(format!("invocation_{tool_call_id}")),
            tool_call_id: ToolCallId::from(tool_call_id),
            provider_tool_call_id: None,
            tool_name: ToolName::from("execute_command"),
            raw_provider_payload: json!({}),
            raw_arguments: "{}".to_string(),
            normalized_arguments: json!({}),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            timeout: ToolTimeoutConfig::default(),
            execution_context: ToolExecutionContext::new(PathBuf::from(".oxide/tool-artifacts")),
            provider_metadata: ProviderMetadata {
                provider: "opencode-go".to_string(),
                protocol: "chat_like".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "deepseek-v4-flash".to_string(),
            },
            working_directory: None,
            environment_metadata: Some(EnvironmentMetadata::default()),
            created_at: now,
            started_at: Some(now),
        }
    }
}
