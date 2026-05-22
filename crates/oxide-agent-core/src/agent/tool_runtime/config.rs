//! Runtime configuration for async parallel tool execution.

use crate::config::ModelInfo;
use std::path::PathBuf;
use std::time::Duration;

/// Per-tool timeout and hung-detection configuration copied into each invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolTimeoutConfig {
    /// Hard execution timeout for one tool call.
    pub per_tool_hard_timeout: Duration,
    /// Grace period for async executors to observe cancellation before abort.
    pub cancellation_grace_period: Duration,
    /// Grace period after SIGTERM before SIGKILL.
    pub terminate_grace_period: Duration,
    /// Grace period after SIGKILL before cleanup is considered failed.
    pub kill_grace_period: Duration,
    /// Whether the soft hung detector is enabled.
    pub hung_detection_enabled: bool,
    /// Startup grace before hung checks begin.
    pub hung_startup_grace: Duration,
    /// No-output threshold for process-like tools.
    pub hung_no_output_threshold: Duration,
    /// No-progress threshold for tools that emit progress events.
    pub hung_no_progress_threshold: Duration,
}

impl Default for ToolTimeoutConfig {
    fn default() -> Self {
        Self {
            per_tool_hard_timeout: Duration::from_secs(300),
            cancellation_grace_period: Duration::from_secs(2),
            terminate_grace_period: Duration::from_secs(5),
            kill_grace_period: Duration::from_secs(2),
            hung_detection_enabled: true,
            hung_startup_grace: Duration::from_secs(30),
            hung_no_output_threshold: Duration::from_secs(120),
            hung_no_progress_threshold: Duration::from_secs(180),
        }
    }
}

/// Output and artifact budget configuration for one runtime instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutputBudget {
    /// Maximum stdout bytes retained for model-facing preview decisions.
    pub max_captured_stdout_bytes: usize,
    /// Maximum stderr bytes retained for model-facing preview decisions.
    pub max_captured_stderr_bytes: usize,
    /// Head bytes included when an output stream is truncated.
    pub output_head_bytes: usize,
    /// Tail bytes included when an output stream is truncated.
    pub output_tail_bytes: usize,
    /// Maximum final provider-visible tool output content bytes.
    pub max_tool_output_content_bytes: usize,
}

impl Default for ToolOutputBudget {
    fn default() -> Self {
        Self {
            max_captured_stdout_bytes: 65_536,
            max_captured_stderr_bytes: 65_536,
            output_head_bytes: 16_384,
            output_tail_bytes: 32_768,
            max_tool_output_content_bytes: 131_072,
        }
    }
}

/// Complete v1 runtime config. Policy/safety gates are intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRuntimeConfig {
    /// Timeout and hung-detection settings.
    pub timeout: ToolTimeoutConfig,
    /// Model-facing output budgets.
    pub output: ToolOutputBudget,
    /// Internal artifact root.
    pub artifact_dir: PathBuf,
    /// Internal tool log root.
    pub log_dir: PathBuf,
    /// Best-effort artifact retention.
    pub artifact_retention: Duration,
    /// Best-effort log retention.
    pub log_retention: Duration,
    /// Optional deployment-local storage soft cap.
    pub storage_soft_cap_bytes: Option<u64>,
    /// Optional technical backpressure cap, not a policy restriction.
    pub max_in_flight_tools: Option<usize>,
}

impl Default for ToolRuntimeConfig {
    fn default() -> Self {
        Self {
            timeout: ToolTimeoutConfig::default(),
            output: ToolOutputBudget::default(),
            artifact_dir: PathBuf::from(".oxide/tool-artifacts"),
            log_dir: PathBuf::from(".oxide/tool-logs"),
            artifact_retention: Duration::from_secs(7 * 24 * 60 * 60),
            log_retention: Duration::from_secs(30 * 24 * 60 * 60),
            storage_soft_cap_bytes: Some(1_073_741_824),
            max_in_flight_tools: None,
        }
    }
}

/// Whether a model route is supported by the v1 typed tool runtime.
#[must_use]
pub fn v1_tool_runtime_enabled_for_model(model: &ModelInfo) -> bool {
    let provider = normalize_tool_runtime_route_part(&model.provider);
    if provider != "opencode-go" {
        return false;
    }

    let model_id = model
        .id
        .rsplit_once('/')
        .map_or(model.id.as_str(), |(_, tail)| tail);
    normalize_tool_runtime_route_part(model_id) == "deepseek-v4-flash"
}

fn normalize_tool_runtime_route_part(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|ch| {
            if ch == '_' || ch.is_whitespace() {
                '-'
            } else {
                ch.to_ascii_lowercase()
            }
        })
        .collect()
}
