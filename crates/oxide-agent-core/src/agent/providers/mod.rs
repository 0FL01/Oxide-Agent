//! Tool providers module
//!
//! Contains implementations of `ToolProvider` for different tool sources.

pub mod delegation;
pub mod filehoster;
pub mod manager_control_plane;
pub mod reminder;
pub mod sandbox;
pub mod ssh_mcp;
pub mod todos;
pub mod ytdlp;

mod path;

#[cfg(feature = "tavily")]
pub mod tavily;

#[cfg(feature = "crawl4ai")]
pub mod crawl4ai;

pub use delegation::DelegationProvider;
pub use filehoster::FileHosterProvider;
pub use manager_control_plane::{
    manager_control_plane_tool_names, ForumTopicActionResult, ForumTopicCreateRequest,
    ForumTopicCreateResult, ForumTopicEditRequest, ForumTopicEditResult, ForumTopicThreadRequest,
    ManagerControlPlaneProvider, ManagerTopicLifecycle, ManagerTopicSandboxCleanup,
};
pub use reminder::{reminder_tool_names, ReminderContext, ReminderProvider};
pub use sandbox::SandboxProvider;
pub use ssh_mcp::{
    cleanup_stale_private_key_tempfiles, inject_approval_credentials,
    inject_ssh_approval_system_message, inject_topic_infra_preflight_system_message,
    inspect_topic_infra_config, probe_secret_ref, SecretProbeKind, SecretProbeReport,
    SshApprovalGrant, SshApprovalRegistry, SshApprovalRequestView, SshMcpProvider,
    TopicInfraPreflightReport,
};
pub use todos::{TodoItem, TodoList, TodoStatus, TodosProvider};
pub use ytdlp::YtdlpProvider;

#[cfg(feature = "tavily")]
pub use tavily::TavilyProvider;

#[cfg(feature = "crawl4ai")]
pub use crawl4ai::Crawl4aiProvider;
