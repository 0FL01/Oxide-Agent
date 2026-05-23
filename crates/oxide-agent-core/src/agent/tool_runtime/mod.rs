//! Async parallel tool runtime.
//!
//! Phase 1 contains typed foundations only. Later phases wire the active
//! runner path through this module and remove legacy execution paths.

pub mod artifacts;
pub mod config;
pub mod executor;
pub mod history;
pub mod invocation;
pub mod modules;
pub mod normalizer;
pub mod output;
pub mod process;
pub mod provider_opencode_go;
pub mod registry;
pub mod runtime;
pub mod types;

pub use artifacts::{ArtifactKind, ArtifactRef};
pub use config::{
    v1_tool_runtime_enabled_for_model, ToolOutputBudget, ToolRuntimeConfig, ToolTimeoutConfig,
};
pub use executor::ToolExecutor;
pub use history::{ToolHistoryError, ToolHistoryWriter};
pub use invocation::{
    EnvironmentMetadata, ModelMetadata, ProviderMetadata, ToolExecutionContext, ToolInvocation,
};
#[cfg(feature = "tool-compression")]
pub use modules::CompressionToolModule;
#[cfg(feature = "tool-sandbox-exec")]
pub use modules::SandboxExecToolModule;
#[cfg(feature = "tool-sandbox-fileops")]
pub use modules::SandboxFileOpsToolModule;
#[cfg(feature = "tool-sandbox-recreate")]
pub use modules::SandboxRecreateToolModule;
#[cfg(feature = "tool-todos")]
pub use modules::TodosToolModule;
pub use modules::{ToolModule, ToolModuleContext};
pub use normalizer::{OutputNormalizer, ToolRuntimeError};
pub use output::{
    CancellationReason, CleanupStatus, OutputPreview, OutputTruncationMetadata, TimeoutReason,
    ToolOutput, ToolOutputIdentity, ToolOutputStatus,
};
pub use process::ProcessManager;
pub use provider_opencode_go::{
    OpenCodeGoParsedToolCall, OpenCodeGoProtocolIssue, OpenCodeGoToolCallBatch,
    OpenCodeGoToolCallParser, OpenCodeGoToolOutputEncoder, OpenCodeGoToolParseError,
};
pub use registry::{RegistryError, ToolRegistry};
pub use runtime::{ToolCallRuntime, ToolRuntimeFatal, ToolTurnContext};
pub use types::{ToolBatchId, ToolCallId, ToolName, TurnId};
