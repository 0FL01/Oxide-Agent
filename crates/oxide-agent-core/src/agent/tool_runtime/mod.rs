//! Async parallel tool runtime.
//!
//! Phase 1 contains typed foundations only. Later phases wire the active
//! runner path through this module and remove legacy execution paths.

pub mod artifacts;
pub mod config;
pub mod invocation;
pub mod normalizer;
pub mod output;
pub mod types;

pub use artifacts::{ArtifactKind, ArtifactRef};
pub use config::{ToolOutputBudget, ToolRuntimeConfig, ToolTimeoutConfig};
pub use invocation::{
    EnvironmentMetadata, ModelMetadata, ProviderMetadata, ToolExecutionContext, ToolInvocation,
};
pub use normalizer::{OutputNormalizer, ToolRuntimeError};
pub use output::{
    CancellationReason, CleanupStatus, OutputPreview, OutputTruncationMetadata, TimeoutReason,
    ToolOutput, ToolOutputIdentity, ToolOutputStatus,
};
pub use types::{ToolBatchId, ToolCallId, ToolName, TurnId};
