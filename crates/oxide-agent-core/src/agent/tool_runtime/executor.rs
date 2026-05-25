//! Typed executor contract for the async tool runtime.

use super::invocation::ToolInvocation;
use super::normalizer::ToolRuntimeError;
use super::output::ToolOutput;
use super::types::ToolName;
use crate::llm::ToolDefinition;
use async_trait::async_trait;

/// Tool-specific business logic. Runtime wrappers own timeout/cancel/history guarantees.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Canonical exact-match tool name.
    fn name(&self) -> ToolName;

    /// Model-visible tool definition.
    fn spec(&self) -> ToolDefinition;

    /// Execute one invocation.
    ///
    /// # Errors
    ///
    /// Returns typed runtime errors that the caller must normalize into a
    /// paired `ToolOutput`.
    async fn execute(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolRuntimeError>;
}
