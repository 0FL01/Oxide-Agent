//! Agent executor module
//!
//! Handles orchestration around the core agent runner, including
//! session lifecycle and tool registry setup.

mod compaction;
mod config;
mod execution;
mod policy_hooks;
mod registry;
#[cfg(test)]
mod tests;
mod types;

use self::types::{AgentsMdContext, ManagerControlPlaneContext, TopicInfraContext};
use crate::agent::compaction::CompactionController;
use crate::agent::profile::{AgentExecutionProfile, HookAccessPolicy, ToolAccessPolicy};
use crate::agent::providers::ReminderContext;
use crate::agent::runner::AgentRunner;
use crate::agent::session::{AgentSession, PendingUserInput};
use crate::agent::wiki_memory::WikiStore;
use std::sync::{Arc, RwLock};

// Re-export sanitize_xml_tags for backward compatibility
pub use super::recovery::sanitize_xml_tags as public_sanitize_xml_tags;

/// Agent executor that runs tasks iteratively
pub struct AgentExecutor {
    runner: AgentRunner,
    session: AgentSession,
    settings: Arc<crate::config::AgentSettings>,
    agents_md: Option<AgentsMdContext>,
    manager_control_plane: Option<ManagerControlPlaneContext>,
    topic_infra: Option<TopicInfraContext>,
    reminder_context: Option<ReminderContext>,
    execution_profile: AgentExecutionProfile,
    tool_policy_state: Arc<RwLock<ToolAccessPolicy>>,
    hook_policy_state: Arc<RwLock<HookAccessPolicy>>,
    compaction_controller: CompactionController,
    wiki_memory_store: Option<WikiStore>,
    last_topic_infra_preflight_summary: Option<String>,
}

/// Terminal outcome of an agent execution request.
pub enum AgentExecutionOutcome {
    /// Agent finished and produced a final response.
    Completed(String),
    /// Agent paused because it is waiting for an external approval.
    WaitingForApproval,
    /// Agent paused because it is waiting for additional user input.
    WaitingForUserInput(PendingUserInput),
}
