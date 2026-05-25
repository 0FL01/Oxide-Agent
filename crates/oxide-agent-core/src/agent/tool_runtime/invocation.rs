//! Canonical invocation passed to typed tool executors.

use super::config::ToolTimeoutConfig;
use super::output::ToolOutputIdentity;
use super::types::{ToolBatchId, ToolCallId, ToolName, TurnId};
use crate::agent::identity::SessionId;
use crate::llm::{InvocationId, ProviderToolCallId};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

/// Runtime execution context available to executors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionContext {
    /// Current working directory for local/process tools.
    pub cwd: Option<PathBuf>,
    /// Artifact root available to the invocation.
    pub artifact_dir: PathBuf,
    /// Whether quiet process execution is expected and should not trigger hung detection.
    pub quiet_ok: bool,
}

impl ToolExecutionContext {
    /// Build a minimal execution context rooted at the given artifact directory.
    #[must_use]
    pub fn new(artifact_dir: impl Into<PathBuf>) -> Self {
        Self {
            cwd: None,
            artifact_dir: artifact_dir.into(),
            quiet_ok: false,
        }
    }
}

/// Provider metadata captured for diagnostics and output normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderMetadata {
    /// LLM provider identifier.
    pub provider: String,
    /// Provider protocol family, usually `chat_like` for v1.
    pub protocol: String,
}

/// Model metadata captured for diagnostics and v1 provider/model validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelMetadata {
    /// Model id used for the current turn.
    pub model: String,
}

/// Optional runtime environment metadata.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EnvironmentMetadata {
    /// Sandbox/container scope when applicable.
    pub sandbox_scope: Option<String>,
    /// Remote target label when applicable.
    pub remote_target: Option<String>,
}

/// Canonical input for one tool executor invocation.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    /// Transport/session id.
    pub session_id: SessionId,
    /// Agent turn id.
    pub turn_id: TurnId,
    /// Tool-call batch id.
    pub batch_id: ToolBatchId,
    /// Original assistant batch index.
    pub batch_index: usize,
    /// Internal runtime invocation id.
    pub invocation_id: InvocationId,
    /// Provider-visible history pairing id.
    pub tool_call_id: ToolCallId,
    /// Provider-owned id when present.
    pub provider_tool_call_id: Option<ProviderToolCallId>,
    /// Exact tool name.
    pub tool_name: ToolName,
    /// Raw provider payload for diagnostics.
    pub raw_provider_payload: Value,
    /// Raw JSON arguments string.
    pub raw_arguments: String,
    /// Parsed/normalized JSON arguments.
    pub normalized_arguments: Value,
    /// Invocation cancellation token.
    pub cancellation_token: CancellationToken,
    /// Timeout and hung-detection settings.
    pub timeout: ToolTimeoutConfig,
    /// Execution context for tool-specific behavior.
    pub execution_context: ToolExecutionContext,
    /// Provider metadata.
    pub provider_metadata: ProviderMetadata,
    /// Model metadata.
    pub model_metadata: ModelMetadata,
    /// Optional working directory override.
    pub working_directory: Option<PathBuf>,
    /// Optional environment metadata.
    pub environment_metadata: Option<EnvironmentMetadata>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Execution start timestamp.
    pub started_at: Option<DateTime<Utc>>,
}

impl ToolInvocation {
    /// Return output identity fields for this invocation.
    #[must_use]
    pub fn output_identity(&self) -> ToolOutputIdentity {
        ToolOutputIdentity {
            tool_call_id: self.tool_call_id.clone(),
            provider_tool_call_id: self.provider_tool_call_id.clone(),
            invocation_id: self.invocation_id.clone(),
            tool_name: self.tool_name.clone(),
            batch_index: self.batch_index,
        }
    }

    /// Return the effective start time, falling back to creation time.
    #[must_use]
    pub fn effective_started_at(&self) -> DateTime<Utc> {
        self.started_at.unwrap_or(self.created_at)
    }
}
