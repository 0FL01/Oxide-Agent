//! Tool providers module
//!
//! Contains native typed runtime tool providers for different tool sources.

/// Topic-scoped self-editing tools for `AGENTS.md`.
pub mod agents_md;
pub mod compression;
pub mod delegation;
mod file_delivery;
pub mod filehoster;
pub mod manager_control_plane;
#[cfg(any(
    oxide_module_tool_media_audio,
    oxide_module_tool_media_image,
    oxide_module_tool_media_video
))]
pub mod media_file;
#[cfg(any(
    oxide_module_tool_media_audio,
    oxide_module_tool_media_image,
    oxide_module_tool_media_video
))]
mod path;
pub mod reminder;
pub mod sandbox;
#[cfg(oxide_module_tool_tts_silero)]
pub mod silero_tts;
#[cfg(oxide_module_integration_ssh_mcp)]
pub mod ssh_mcp;
#[cfg(not(oxide_module_integration_ssh_mcp))]
mod ssh_mcp_stub;
#[cfg(oxide_module_tool_stack_logs)]
pub mod stack_logs;
pub mod todos;
#[cfg(oxide_module_tool_tts_kokoro)]
pub mod tts;
#[cfg(oxide_module_tool_webfetch_md)]
pub mod webfetch_md;
pub mod wiki_memory;
pub mod ytdlp;

#[cfg(oxide_module_tool_tavily)]
pub mod tavily;

#[cfg(oxide_module_tool_brave_search)]
pub mod brave_search;

#[cfg(oxide_module_tool_browser_live)]
pub mod browser_live;

#[cfg(oxide_module_tool_crw)]
pub mod crw;

#[cfg(oxide_module_integration_mcp_jira)]
pub mod jira_mcp;

#[cfg(oxide_module_integration_mcp_mattermost)]
pub mod mattermost_mcp;

pub use agents_md::{AgentsMdProvider, agents_md_tool_names};
pub use compression::{CompressionProvider, TOOL_COMPRESS, compress_tool_names};
pub use delegation::DelegationProvider;
pub use filehoster::FileHosterProvider;
pub use manager_control_plane::{
    ForumTopicActionResult, ForumTopicCreateRequest, ForumTopicCreateResult, ForumTopicEditRequest,
    ForumTopicEditResult, ForumTopicThreadRequest, ManagerControlPlaneProvider,
    ManagerTopicLifecycle, ManagerTopicSandboxCleanup, manager_control_plane_tool_names,
};
#[cfg(any(
    oxide_module_tool_media_audio,
    oxide_module_tool_media_image,
    oxide_module_tool_media_video
))]
pub use media_file::MediaFileProvider;
pub use reminder::{
    ReminderContext, ReminderProvider, ReminderScheduleEvent, ReminderScheduleNotifier,
    reminder_tool_names,
};
pub use sandbox::{
    SandboxExecProvider, SandboxFileOpsProvider, SandboxLifecycleProvider, SandboxRuntime,
};
#[cfg(oxide_module_tool_tts_silero)]
pub use silero_tts::{
    SileroSampleRate, SileroTtsConfig, SileroTtsFormat, SileroTtsProvider, SileroTtsRequest,
    SileroTtsSpeaker,
};
#[cfg(oxide_module_integration_ssh_mcp)]
pub use ssh_mcp::{
    SecretProbeKind, SecretProbeReport, SshMcpProvider, TopicInfraPreflightReport,
    inject_topic_infra_preflight_system_message, inspect_topic_infra_config, probe_secret_ref,
};
#[cfg(not(oxide_module_integration_ssh_mcp))]
pub use ssh_mcp_stub::{
    SecretProbeKind, SecretProbeReport, TopicInfraPreflightReport,
    inject_topic_infra_preflight_system_message, inspect_topic_infra_config, probe_secret_ref,
};
#[cfg(oxide_module_tool_stack_logs)]
pub use stack_logs::StackLogsProvider;
pub use todos::{TodoItem, TodoList, TodoStatus, TodosProvider};
#[cfg(oxide_module_tool_tts_kokoro)]
pub use tts::{KokoroTtsProvider, TtsConfig, TtsVoice};
#[cfg(oxide_module_tool_webfetch_md)]
pub use webfetch_md::WebFetchMdProvider;
pub use wiki_memory::WikiMemoryProvider;
pub use ytdlp::YtdlpProvider;

#[cfg(oxide_module_tool_tavily)]
pub use tavily::TavilyProvider;

#[cfg(oxide_module_tool_brave_search)]
pub use brave_search::BraveSearchProvider;

#[cfg(oxide_module_tool_browser_live)]
pub use browser_live::{
    BrowserAction, BrowserArtifactSettings, BrowserLiveProvider, BrowserObservation,
    BrowserSidecarClient, BrowserSidecarError, BrowserSidecarTimeouts, CreateSessionRequest,
    IdempotencyKey, ScreenshotArtifact, SidecarErrorBody, Viewport,
};

#[cfg(oxide_module_tool_crw)]
pub use crw::CrwProvider;

#[cfg(oxide_module_integration_mcp_jira)]
pub use jira_mcp::{JiraMcpConfig, JiraMcpProvider};

#[cfg(oxide_module_integration_mcp_mattermost)]
pub use mattermost_mcp::{MattermostMcpConfig, MattermostMcpProvider};
