//! Agent-facing `compress` tool provider.

use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::json;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
