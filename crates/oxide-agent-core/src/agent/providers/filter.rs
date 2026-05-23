//! Lightweight provider adapter for exposing only a module-owned tool slice.

use crate::agent::progress::AgentEvent;
use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Restricts a legacy provider to a static set of tool names.
pub struct FilteredToolProvider {
    provider: Arc<dyn ToolProvider>,
    allowed_tool_names: &'static [&'static str],
}

impl FilteredToolProvider {
    /// Creates a filtered view over an existing provider.
    #[must_use]
    pub const fn new(
        provider: Arc<dyn ToolProvider>,
        allowed_tool_names: &'static [&'static str],
    ) -> Self {
        Self {
            provider,
            allowed_tool_names,
        }
    }

    fn allows(&self, tool_name: &str) -> bool {
        self.allowed_tool_names.contains(&tool_name)
    }
}

#[async_trait]
impl ToolProvider for FilteredToolProvider {
    fn name(&self) -> &'static str {
        self.provider.name()
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        self.provider
            .tools()
            .into_iter()
            .filter(|tool| self.allows(&tool.name))
            .collect()
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        self.allows(tool_name) && self.provider.can_handle(tool_name)
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        if !self.can_handle(tool_name) {
            return Err(anyhow!("Unknown tool: {tool_name}"));
        }

        self.provider
            .execute(tool_name, arguments, progress_tx, cancellation_token)
            .await
    }
}
