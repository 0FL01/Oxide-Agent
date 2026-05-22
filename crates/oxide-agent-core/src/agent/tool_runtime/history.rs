//! History write facade and invariants for tool-call batches.

use super::output::ToolOutput;
use super::provider_opencode_go::OpenCodeGoToolCallBatch;
use async_trait::async_trait;
use thiserror::Error;

/// History persistence failure.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ToolHistoryError {
    /// Assistant tool-call message could not be recorded.
    #[error("failed to record assistant tool-call batch: {0}")]
    AssistantWriteFailed(String),
    /// Tool output message could not be recorded.
    #[error("failed to record tool output: {0}")]
    OutputWriteFailed(String),
}

/// Runtime-owned history writer. Executors must never call this directly.
#[async_trait]
pub trait ToolHistoryWriter: Send + Sync {
    /// Record the assistant tool-call batch before execution starts.
    async fn record_assistant_tool_calls(
        &self,
        batch: &OpenCodeGoToolCallBatch,
    ) -> Result<(), ToolHistoryError>;

    /// Record exactly one tool output in deterministic batch order.
    async fn record_tool_output(&self, output: &ToolOutput) -> Result<(), ToolHistoryError>;
}
