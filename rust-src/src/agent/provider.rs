//! Tool Provider trait for extensible agent tools
//!
//! This trait provides a unified interface for all tool providers.
//! Implementations include `SandboxProvider`, `TavilyProvider`, and future MCP providers.

use crate::llm::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;

/// Unified interface for tool providers
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// Provider name for logging and debugging
    fn name(&self) -> &'static str;

    /// Returns the list of tools this provider offers
    fn tools(&self) -> Vec<ToolDefinition>;

    /// Check if this provider can handle the given tool
    fn can_handle(&self, tool_name: &str) -> bool;

    /// Execute a tool and return the result
    async fn execute(&self, tool_name: &str, arguments: &str) -> Result<String>;
}
