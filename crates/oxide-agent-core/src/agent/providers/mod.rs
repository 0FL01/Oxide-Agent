//! Tool providers module
//!
//! Contains implementations of `ToolProvider` for different tool sources.

/// Topic-scoped self-editing tools for `AGENTS.md`.
pub mod agents_md;
pub mod compression;
pub mod delegation;
mod file_delivery;
pub mod filehoster;
pub mod manager_control_plane;
pub mod media_file;
pub mod reminder;
pub mod sandbox;
pub mod silero_tts;
pub mod ssh_mcp;
pub mod stack_logs;
pub mod todos;
pub mod tts;
pub mod ytdlp;

mod path;

#[cfg(feature = "tavily")]
pub mod tavily;

#[cfg(feature = "searxng")]
pub mod searxng;

#[cfg(feature = "crawl4ai")]
pub mod crawl4ai;

#[cfg(feature = "browser_use")]
pub mod browser_use;

#[cfg(feature = "jira")]
pub mod jira_mcp;

#[cfg(feature = "mattermost")]
pub mod mattermost_mcp;

pub use agents_md::{agents_md_tool_names, AgentsMdProvider};
pub use compression::{compress_tool_names, CompressionProvider, TOOL_COMPRESS};
pub use delegation::DelegationProvider;
pub use filehoster::FileHosterProvider;
pub use manager_control_plane::{
    manager_control_plane_tool_names, ForumTopicActionResult, ForumTopicCreateRequest,
    ForumTopicCreateResult, ForumTopicEditRequest, ForumTopicEditResult, ForumTopicThreadRequest,
    ManagerControlPlaneProvider, ManagerTopicLifecycle, ManagerTopicSandboxCleanup,
};
pub use media_file::MediaFileProvider;
pub use reminder::{
    reminder_tool_names, ReminderContext, ReminderProvider, ReminderScheduleEvent,
    ReminderScheduleNotifier,
};
pub use sandbox::SandboxProvider;
pub use silero_tts::{
    SileroSampleRate, SileroTtsConfig, SileroTtsFormat, SileroTtsProvider, SileroTtsRequest,
    SileroTtsSpeaker,
};
pub use ssh_mcp::{
    cleanup_stale_private_key_tempfiles, inject_approval_credentials,
    inject_ssh_approval_system_message, inject_topic_infra_preflight_system_message,
    inspect_topic_infra_config, probe_secret_ref, SecretProbeKind, SecretProbeReport,
    SshApprovalGrant, SshApprovalRegistry, SshApprovalRequestView, SshMcpProvider,
    TopicInfraPreflightReport,
};
pub use stack_logs::StackLogsProvider;
pub use todos::{TodoItem, TodoList, TodoStatus, TodosProvider};
pub use tts::{KokoroTtsProvider, TtsConfig, TtsVoice};
pub use ytdlp::YtdlpProvider;

#[cfg(feature = "tavily")]
pub use tavily::TavilyProvider;

#[cfg(feature = "searxng")]
pub use searxng::SearxngProvider;

#[cfg(feature = "crawl4ai")]
pub use crawl4ai::Crawl4aiProvider;

#[cfg(feature = "browser_use")]
pub use browser_use::BrowserUseProvider;

#[cfg(feature = "jira")]
pub use jira_mcp::{JiraMcpConfig, JiraMcpProvider};

#[cfg(feature = "mattermost")]
pub use mattermost_mcp::{MattermostMcpConfig, MattermostMcpProvider};
