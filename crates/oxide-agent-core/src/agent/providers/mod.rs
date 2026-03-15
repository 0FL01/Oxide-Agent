//! Tool providers module
//!
//! Contains implementations of `ToolProvider` for different tool sources.

pub mod delegation;
pub mod filehoster;
pub mod manager_control_plane;
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
    ForumTopicActionResult, ForumTopicCreateRequest, ForumTopicCreateResult, ForumTopicEditRequest,
    ForumTopicEditResult, ForumTopicThreadRequest, ManagerControlPlaneProvider,
    ManagerTopicLifecycle, ManagerTopicSandboxCleanup,
};
pub use sandbox::SandboxProvider;
pub use ssh_mcp::{
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
