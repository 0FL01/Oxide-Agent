//! Async parallel tool runtime.
//!
//! Tool capability modules own typed executor construction; callers register
//! executors in the catalog rather than provider-specific paths.

pub(crate) mod arguments;
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
#[cfg(oxide_module_tool_retrieve_tools)]
pub mod retrieve_tools;
pub mod runtime;
pub mod surface;
pub mod types;

pub use artifacts::{ArtifactKind, ArtifactRef};
pub use config::{
    ToolOutputBudget, ToolRuntimeConfig, ToolTimeoutConfig, v1_tool_runtime_enabled_for_model,
};
pub use executor::ToolExecutor;
pub use history::{ToolHistoryError, ToolHistoryWriter};
pub use invocation::{
    EnvironmentMetadata, ModelMetadata, ProviderMetadata, ToolExecutionContext, ToolInvocation,
};
pub use modules::AgentsMdModuleContext;
#[cfg(oxide_module_tool_agents_md)]
pub use modules::AgentsMdToolModule;
#[cfg(oxide_module_tool_audio_stt)]
pub use modules::AudioSttToolModule;
pub use modules::BrowserLiveModuleContext;
#[cfg(oxide_module_tool_browser_live)]
pub use modules::BrowserLiveToolModule;
pub use modules::BrowserSessionCleanup;
#[cfg(oxide_module_tool_compression)]
pub use modules::CompressionToolModule;
#[cfg(oxide_module_tool_delegation)]
pub use modules::DelegationToolModule;
#[cfg(oxide_module_tool_file_delivery)]
pub use modules::FileDeliveryToolModule;
#[cfg(oxide_module_integration_mcp_jira)]
pub use modules::JiraMcpToolModule;
#[cfg(oxide_module_tool_tts_kokoro)]
pub use modules::KokoroTtsToolModule;
pub use modules::ManagerControlPlaneModuleContext;
#[cfg(oxide_module_manager_control_plane)]
pub use modules::ManagerControlPlaneToolModule;
#[cfg(oxide_module_integration_mcp_mattermost)]
pub use modules::MattermostMcpToolModule;
#[cfg(oxide_module_tool_reminder)]
pub use modules::ReminderToolModule;
#[cfg(oxide_module_tool_sandbox_exec)]
pub use modules::SandboxExecToolModule;
#[cfg(oxide_module_tool_sandbox_fileops)]
pub use modules::SandboxFileOpsToolModule;
#[cfg(oxide_module_tool_sandbox_recreate)]
pub use modules::SandboxRecreateToolModule;
#[cfg(oxide_module_tool_tts_silero)]
pub use modules::SileroTtsToolModule;
pub use modules::SshMcpModuleContext;
#[cfg(oxide_module_integration_ssh_mcp)]
pub use modules::SshMcpToolModule;
#[cfg(oxide_module_tool_stack_logs)]
pub use modules::StackLogsToolModule;
#[cfg(oxide_module_tool_todos)]
pub use modules::TodosToolModule;
pub use modules::ToolModuleContextParts;
#[cfg(oxide_module_tool_vision_image)]
pub use modules::VisionImageToolModule;
#[cfg(oxide_module_tool_vision_video)]
pub use modules::VisionVideoToolModule;
#[cfg(oxide_module_tool_webfetch_md)]
pub use modules::WebCrawlerToolModule;
#[cfg(oxide_module_tool_webfetch_md)]
pub use modules::WebFetchMdToolModule;
#[cfg(oxide_module_tool_web_search)]
pub use modules::WebSearchToolModule;
#[cfg(oxide_module_tool_ytdlp)]
pub use modules::YtdlpToolModule;
pub use modules::{ToolModule, ToolModuleContext};
pub use normalizer::{OutputNormalizer, ToolRuntimeError};
pub use output::{
    CancellationReason, CleanupStatus, OutputPreview, OutputTruncationMetadata, TimeoutReason,
    ToolOutput, ToolOutputIdentity, ToolOutputImageAttachment, ToolOutputStatus,
};
pub use process::ProcessManager;
pub use provider_opencode_go::{
    OpenCodeGoParsedToolCall, OpenCodeGoProtocolIssue, OpenCodeGoToolCallBatch,
    OpenCodeGoToolCallParser, OpenCodeGoToolOutputEncoder, OpenCodeGoToolParseError,
};
#[cfg(oxide_module_tool_retrieve_tools)]
pub use retrieve_tools::RetrieveToolsToolModule;
pub use runtime::{ToolCallRuntime, ToolRuntimeFatal, ToolTurnContext};
pub use surface::{
    ActivationResult, CapabilityGroup, CatalogError, ToolCatalog, ToolCatalogEntry, ToolSurface,
    ToolSurfaceHandle, ToolVisibility,
};
pub use types::{ToolBatchId, ToolCallId, ToolName, TurnId};
