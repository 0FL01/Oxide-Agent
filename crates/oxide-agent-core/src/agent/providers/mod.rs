//! Tool providers module
//!
//! Contains implementations of `ToolProvider` for different tool sources.

/// Topic-scoped self-editing tools for `AGENTS.md`.
pub mod agents_md;
pub mod compression;
pub mod delegation;
mod file_delivery;
pub mod filehoster;
pub mod filter;
pub mod manager_control_plane;
#[cfg(any(
    feature = "tool-media-audio",
    feature = "tool-media-image",
    feature = "tool-media-video"
))]
pub mod media_file;
#[cfg(any(
    feature = "tool-media-audio",
    feature = "tool-media-image",
    feature = "tool-media-video"
))]
mod path;
pub mod reminder;
pub mod sandbox;
#[cfg(feature = "tool-tts-silero")]
pub mod silero_tts;
#[cfg(feature = "integration-ssh-mcp")]
pub mod ssh_mcp;
#[cfg(not(feature = "integration-ssh-mcp"))]
mod ssh_mcp_stub;
#[cfg(feature = "tool-stack-logs")]
pub mod stack_logs;
pub mod todos;
#[cfg(feature = "tool-tts-kokoro")]
pub mod tts;
#[cfg(feature = "tool-webfetch-md")]
pub mod webfetch_md;
pub mod wiki_memory;
pub mod ytdlp;

#[cfg(feature = "tool-tavily")]
pub mod tavily;

#[cfg(feature = "tool-searxng")]
pub mod searxng;

#[cfg(feature = "tool-browser-use")]
pub mod browser_use;

#[cfg(feature = "integration-mcp-jira")]
pub mod jira_mcp;

#[cfg(feature = "integration-mcp-mattermost")]
pub mod mattermost_mcp;

pub use agents_md::{agents_md_tool_names, AgentsMdProvider};
pub use compression::{compress_tool_names, CompressionProvider, TOOL_COMPRESS};
pub use delegation::DelegationProvider;
pub use filehoster::FileHosterProvider;
pub use filter::FilteredToolProvider;
pub use manager_control_plane::{
    manager_control_plane_tool_names, ForumTopicActionResult, ForumTopicCreateRequest,
    ForumTopicCreateResult, ForumTopicEditRequest, ForumTopicEditResult, ForumTopicThreadRequest,
    ManagerControlPlaneProvider, ManagerTopicLifecycle, ManagerTopicSandboxCleanup,
};
#[cfg(any(
    feature = "tool-media-audio",
    feature = "tool-media-image",
    feature = "tool-media-video"
))]
pub use media_file::MediaFileProvider;
pub use reminder::{
    reminder_tool_names, ReminderContext, ReminderProvider, ReminderScheduleEvent,
    ReminderScheduleNotifier,
};
pub use sandbox::{
    SandboxExecProvider, SandboxFileOpsProvider, SandboxLifecycleProvider, SandboxRuntime,
};
#[cfg(feature = "tool-tts-silero")]
pub use silero_tts::{
    SileroSampleRate, SileroTtsConfig, SileroTtsFormat, SileroTtsProvider, SileroTtsRequest,
    SileroTtsSpeaker,
};
#[cfg(feature = "integration-ssh-mcp")]
pub use ssh_mcp::{
    cleanup_stale_private_key_tempfiles, inject_approval_credentials,
    inject_ssh_approval_system_message, inject_topic_infra_preflight_system_message,
    inspect_topic_infra_config, probe_secret_ref, SecretProbeKind, SecretProbeReport,
    SshApprovalGrant, SshApprovalRegistry, SshApprovalRequestView, SshMcpProvider,
    TopicInfraPreflightReport,
};
#[cfg(not(feature = "integration-ssh-mcp"))]
pub use ssh_mcp_stub::{
    cleanup_stale_private_key_tempfiles, inject_approval_credentials,
    inject_ssh_approval_system_message, inject_topic_infra_preflight_system_message,
    inspect_topic_infra_config, probe_secret_ref, SecretProbeKind, SecretProbeReport,
    SshApprovalGrant, SshApprovalRegistry, SshApprovalRequestView, TopicInfraPreflightReport,
};
#[cfg(feature = "tool-stack-logs")]
pub use stack_logs::StackLogsProvider;
pub use todos::{TodoItem, TodoList, TodoStatus, TodosProvider};
#[cfg(feature = "tool-tts-kokoro")]
pub use tts::{KokoroTtsProvider, TtsConfig, TtsVoice};
#[cfg(feature = "tool-webfetch-md")]
pub use webfetch_md::WebFetchMdProvider;
pub use wiki_memory::WikiMemoryProvider;
pub use ytdlp::YtdlpProvider;

#[cfg(feature = "tool-tavily")]
pub use tavily::TavilyProvider;

#[cfg(feature = "tool-searxng")]
pub use searxng::SearxngProvider;

#[cfg(feature = "tool-browser-use")]
pub use browser_use::BrowserUseProvider;

#[cfg(feature = "integration-mcp-jira")]
pub use jira_mcp::{JiraMcpConfig, JiraMcpProvider};

#[cfg(feature = "integration-mcp-mattermost")]
pub use mattermost_mcp::{MattermostMcpConfig, MattermostMcpProvider};
