//! Async parallel tool runtime.
//!
//! Tool capability modules own typed executor construction; callers register
//! executors through the runtime registry rather than provider-specific paths.

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
#[cfg(feature = "tool-agents-md")]
pub use modules::AgentsMdModuleContext;
#[cfg(feature = "tool-agents-md")]
pub use modules::AgentsMdToolModule;
#[cfg(feature = "tool-brave-search")]
pub use modules::BraveSearchToolModule;
#[cfg(feature = "tool-compression")]
pub use modules::CompressionToolModule;
#[cfg(feature = "tool-crawl4ai-markdown")]
pub use modules::Crawl4AiMarkdownToolModule;
#[cfg(feature = "tool-delegation")]
pub use modules::DelegationToolModule;
#[cfg(feature = "tool-duckduckgo")]
pub use modules::DuckDuckGoToolModule;
#[cfg(feature = "tool-file-delivery")]
pub use modules::FileDeliveryToolModule;
#[cfg(feature = "integration-mcp-jira")]
pub use modules::JiraMcpToolModule;
#[cfg(feature = "tool-tts-kokoro")]
pub use modules::KokoroTtsToolModule;
#[cfg(feature = "manager-control-plane")]
pub use modules::ManagerControlPlaneModuleContext;
#[cfg(feature = "manager-control-plane")]
pub use modules::ManagerControlPlaneToolModule;
#[cfg(feature = "integration-mcp-mattermost")]
pub use modules::MattermostMcpToolModule;
#[cfg(feature = "tool-media-audio")]
pub use modules::MediaAudioToolModule;
#[cfg(feature = "tool-media-image")]
pub use modules::MediaImageToolModule;
#[cfg(feature = "tool-media-video")]
pub use modules::MediaVideoToolModule;
#[cfg(feature = "tool-reminder")]
pub use modules::ReminderToolModule;
#[cfg(feature = "tool-sandbox-exec")]
pub use modules::SandboxExecToolModule;
#[cfg(feature = "tool-sandbox-fileops")]
pub use modules::SandboxFileOpsToolModule;
#[cfg(feature = "tool-sandbox-recreate")]
pub use modules::SandboxRecreateToolModule;
#[cfg(feature = "tool-searxng")]
pub use modules::SearxngToolModule;
#[cfg(feature = "tool-tts-silero")]
pub use modules::SileroTtsToolModule;
#[cfg(feature = "integration-ssh-mcp")]
pub use modules::SshMcpModuleContext;
#[cfg(feature = "integration-ssh-mcp")]
pub use modules::SshMcpToolModule;
#[cfg(feature = "tool-stack-logs")]
pub use modules::StackLogsToolModule;
#[cfg(feature = "tool-tavily")]
pub use modules::TavilyToolModule;
#[cfg(feature = "tool-todos")]
pub use modules::TodosToolModule;
pub use modules::ToolModuleContextParts;
#[cfg(feature = "tool-webfetch-md")]
pub use modules::WebFetchMdToolModule;
#[cfg(feature = "tool-wiki-memory")]
pub use modules::WikiMemoryToolModule;
#[cfg(feature = "tool-ytdlp")]
pub use modules::YtdlpToolModule;
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
