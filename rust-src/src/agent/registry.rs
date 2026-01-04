//! Tool Registry - manages all tool providers
//!
//! Collects tools from all registered providers and routes tool calls appropriately.

use super::provider::ToolProvider;
use crate::llm::ToolDefinition;
use anyhow::{anyhow, Result};
use tracing::{debug, info, warn};

/// Registry that manages multiple tool providers
pub struct ToolRegistry {
    providers: Vec<Box<dyn ToolProvider>>,
}

impl ToolRegistry {
    /// Create a new empty registry
    #[must_use]
    pub const fn new() -> Self {
        Self { providers: Vec::new() }
    }

    /// Register a new tool provider
    pub fn register(&mut self, provider: Box<dyn ToolProvider>) {
        info!(provider = provider.name(), "Registered tool provider");
        self.providers.push(provider);
    }

    /// Get all tools from all registered providers
    #[must_use]
    pub fn all_tools(&self) -> Vec<ToolDefinition> {
        self.providers.iter().flat_map(|p| p.tools()).collect()
    }

    /// Find a provider and execute the tool
    ///
    /// # Errors
    ///
    /// Returns an error if no provider can handle the tool or if execution fails.
    pub async fn execute(&self, tool_name: &str, arguments: &str) -> Result<String> {
        debug!(tool = tool_name, "Looking for provider to handle tool");

        for provider in &self.providers {
            if provider.can_handle(tool_name) {
                debug!(
                    tool = tool_name,
                    provider = provider.name(),
                    "Found provider for tool"
                );
                return provider.execute(tool_name, arguments).await;
            }
        }

        warn!(tool = tool_name, "No provider found for tool");
        Err(anyhow!("Unknown tool: {tool_name}"))
    }

    /// Check if any provider can handle the tool
    #[must_use]
    pub fn can_handle(&self, tool_name: &str) -> bool {
        self.providers.iter().any(|p| p.can_handle(tool_name))
    }

    /// Get provider names
    #[must_use]
    pub fn provider_names(&self) -> Vec<&str> {
        self.providers.iter().map(|p| p.name()).collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
