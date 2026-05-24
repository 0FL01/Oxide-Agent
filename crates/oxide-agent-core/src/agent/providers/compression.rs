//! Agent-facing `compress` tool provider.

use crate::agent::provider::ToolProvider;
use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

/// Stable tool name for agent-triggered context compression.
pub const TOOL_COMPRESS: &str = "compress";

/// Tool names exposed for agent-triggered context compression.
///
/// This keeps the tool name in one place for registry and runner checks.
#[must_use]
pub fn compress_tool_names() -> Vec<String> {
    vec![TOOL_COMPRESS.to_string()]
}

/// Minimal provider that only advertises the `compress` tool.
pub struct CompressionProvider;

impl CompressionProvider {
    /// Create a new compression tool provider.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn tools_definitions() -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: TOOL_COMPRESS.to_string(),
            description: "Compress the current Agent Mode hot context using the built-in compaction pipeline.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
        }]
    }

    /// Build native typed runtime executors for manual context compression.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        Self::tools_definitions()
            .into_iter()
            .map(|spec| {
                Arc::new(CompressionToolExecutor {
                    provider: Arc::clone(self),
                    name: ToolName::from(spec.name.clone()),
                    spec,
                }) as Arc<dyn ToolExecutor>
            })
            .collect()
    }

    fn validate_arguments(arguments: &str) -> Result<()> {
        let value = if arguments.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(arguments)?
        };

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(()),
            _ => Err(anyhow!("compress arguments must be an empty JSON object")),
        }
    }

    fn execute_tool(&self, tool_name: &str, arguments: &str) -> Result<String> {
        if tool_name != TOOL_COMPRESS {
            return Err(anyhow!("Unknown compression tool: {tool_name}"));
        }
        Self::validate_arguments(arguments)?;
        serde_json::to_string_pretty(&json!({
            "ok": true,
            "scheduled": true,
            "message": "Manual context compression has been scheduled before the next model call."
        }))
        .map_err(Into::into)
    }
}

impl Default for CompressionProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for CompressionProvider {
    fn name(&self) -> &'static str {
        "compression"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        Self::tools_definitions()
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        tool_name == TOOL_COMPRESS
    }

    async fn execute(
        &self,
        tool_name: &str,
        _arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        Err(anyhow!(
            "{tool_name} is handled directly by the agent runner"
        ))
    }
}

struct CompressionToolExecutor {
    provider: Arc<CompressionProvider>,
    name: ToolName,
    spec: ToolDefinition,
}

#[async_trait]
impl ToolExecutor for CompressionToolExecutor {
    fn name(&self) -> ToolName {
        self.name.clone()
    }

    fn spec(&self) -> ToolDefinition {
        self.spec.clone()
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            artifact_dir: invocation.execution_context.artifact_dir.clone(),
            ..ToolRuntimeConfig::default()
        });
        self.provider
            .execute_tool(self.name.as_str(), &invocation.raw_arguments)
            .map(|output| normalizer.success(&invocation, &output, ""))
            .map_err(compression_runtime_error)
    }
}

fn compression_runtime_error(error: anyhow::Error) -> ToolRuntimeError {
    let message = error.to_string();
    if error.downcast_ref::<serde_json::Error>().is_some()
        || message.contains("compress arguments must be")
    {
        ToolRuntimeError::InvalidArguments(message)
    } else {
        ToolRuntimeError::Failure(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolInvocation, ToolOutputStatus, ToolRuntimeError, ToolTimeoutConfig, TurnId,
    };
    use crate::llm::InvocationId;
    use chrono::Utc;
    use tokio_util::sync::CancellationToken;

    fn runtime_invocation(tool_name: &str, raw_arguments: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(77),
            turn_id: TurnId::from("turn-compression"),
            batch_id: ToolBatchId::from("batch-compression"),
            batch_index: 0,
            invocation_id: InvocationId::from(format!("invoke-{tool_name}")),
            tool_call_id: ToolCallId::from(format!("call-{tool_name}")),
            provider_tool_call_id: None,
            tool_name: ToolName::from(tool_name),
            raw_provider_payload: json!({}),
            raw_arguments: raw_arguments.to_string(),
            normalized_arguments: serde_json::Value::Null,
            cancellation_token: CancellationToken::new(),
            timeout: ToolTimeoutConfig::default(),
            execution_context: ToolExecutionContext::new(std::env::temp_dir()),
            provider_metadata: ProviderMetadata {
                provider: "test".to_string(),
                protocol: "chat_like".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "test-model".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: now,
            started_at: Some(now),
        }
    }

    #[test]
    fn exposes_compress_tool_definition() {
        let provider = CompressionProvider::new();
        assert!(provider.can_handle(TOOL_COMPRESS));

        let tools = provider.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, TOOL_COMPRESS);
        assert!(tools[0].description.contains("compaction pipeline"));
    }

    #[test]
    fn tool_name_list_contains_compress() {
        assert_eq!(compress_tool_names(), vec![TOOL_COMPRESS.to_string()]);
    }

    #[tokio::test]
    async fn execute_is_handled_by_runner() {
        let provider = CompressionProvider::new();

        let error = provider
            .execute(TOOL_COMPRESS, "{}", None, None)
            .await
            .expect_err("compress should be handled by the runner");

        assert!(error
            .to_string()
            .contains("handled directly by the agent runner"));
    }

    #[test]
    fn typed_runtime_executors_register_compress_tool() {
        let provider = Arc::new(CompressionProvider::new());
        let executors = provider.tool_runtime_executors();

        assert_eq!(executors.len(), 1);
        assert_eq!(executors[0].name().as_str(), TOOL_COMPRESS);
    }

    #[tokio::test]
    async fn typed_runtime_executor_schedules_manual_compression() {
        let provider = Arc::new(CompressionProvider::new());
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("compress typed executor registered");

        let output = executor
            .execute(runtime_invocation(TOOL_COMPRESS, "{}"))
            .await
            .expect("compress schedule succeeds");

        assert_eq!(output.status, ToolOutputStatus::Success);
        let stdout = output.stdout.text.as_deref().expect("stdout text");
        assert!(stdout.contains(r#""scheduled": true"#));
    }

    #[tokio::test]
    async fn typed_runtime_executor_rejects_non_empty_arguments() {
        let provider = Arc::new(CompressionProvider::new());
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("compress typed executor registered");

        let error = executor
            .execute(runtime_invocation(TOOL_COMPRESS, r#"{"force":true}"#))
            .await
            .expect_err("non-empty compress args must be invalid");

        assert!(matches!(error, ToolRuntimeError::InvalidArguments(_)));
    }
}
