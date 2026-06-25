//! Typed dynamic prompt context blocks and providers.
//!
//! The prompt composer owns block ordering/rendering, while each bounded context
//! owns how its dynamic context is built.

use crate::storage::StorageError;
use async_trait::async_trait;

/// Meaning of a dynamic prompt context block for downstream prompt assembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptContextSemantics {
    /// Deterministic rule-like context that constrains runtime behavior.
    DeterministicRuleLike,
    /// Authoritative user default or preference confirmed outside semantic recall.
    AuthoritativeUserDefault,
    /// Confirmed operating contract for how to work with this user.
    OperatingContract,
    /// Current task resume/open-loop state.
    TaskResume,
    /// Reusable support protocol selected for the current turn.
    SupportProtocol,
    /// Evidence/background context, not an instruction source.
    EvidenceOnly,
}

/// One typed dynamic context block inserted into the agent system prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptContextBlock {
    /// Stable provider-local block name for diagnostics and tests.
    pub name: String,
    /// Markdown body inserted into the system prompt when non-empty.
    pub body: String,
    /// Semantics of this block in the prompt precedence contract.
    pub semantics: PromptContextSemantics,
}

impl PromptContextBlock {
    /// Create one dynamic prompt context block.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        body: impl Into<String>,
        semantics: PromptContextSemantics,
    ) -> Self {
        Self {
            name: name.into(),
            body: body.into(),
            semantics,
        }
    }
}

/// Inputs available to a dynamic prompt context provider for one agent turn.
#[derive(Debug, Clone, Copy)]
pub struct PromptContextRequest<'a> {
    /// Stable user/principal id for this context request.
    pub user_id: i64,
    /// Stable context key for scoped dynamic context.
    pub context_key: &'a str,
    /// Current user task/input used for context selection.
    pub task: &'a str,
    /// Number of hot-memory messages currently available to the run.
    pub memory_message_count: usize,
}

/// Error returned by dynamic prompt context providers.
#[derive(Debug, thiserror::Error)]
pub enum PromptContextError {
    /// Dynamic context provider failed while reading durable storage.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// Result type returned by dynamic prompt context providers.
pub type PromptContextResult<T> = Result<T, PromptContextError>;

/// Source of dynamic prompt context blocks for an agent run.
#[async_trait]
pub trait DynamicPromptContextProvider: Send + Sync {
    /// Build ordered context blocks for one prompt assembly.
    async fn build_blocks(
        &self,
        request: PromptContextRequest<'_>,
    ) -> PromptContextResult<Vec<PromptContextBlock>>;
}
