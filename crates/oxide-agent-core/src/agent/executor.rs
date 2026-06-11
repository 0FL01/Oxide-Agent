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
use crate::agent::memory::AgentMessageAttachment;
use crate::agent::profile::{AgentExecutionProfile, HookAccessPolicy, ToolAccessPolicy};
use crate::agent::providers::ReminderContext;
use crate::agent::runner::AgentRunner;
use crate::agent::session::{AgentSession, PendingUserInput};
use crate::agent::wiki_memory::WikiStore;
use crate::config::ModelInfo;
use std::sync::{Arc, RwLock};

/// Per-run effort preset for agent execution budgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentExecutionEffort {
    /// Use configured/default runtime budgets.
    #[default]
    Standard,
    /// Allow more continuation/iteration/time budget for deeper research.
    Extended,
    /// Highest built-in budget for long-running, tool-heavy work.
    Heavy,
}

/// Optional per-run execution controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AgentExecutionOptions {
    /// Effort preset applied to runner budgets.
    pub effort: AgentExecutionEffort,
    /// Exact per-run timeout override in seconds.
    pub timeout_secs: Option<u64>,
    /// Exact per-run search tool call limit override.
    pub search_limit: Option<usize>,
    /// Explicit per-run provider reasoning effort override.
    pub reasoning_effort_override: Option<&'static str>,
}

impl AgentExecutionOptions {
    /// Create options for a specific effort preset.
    #[must_use]
    pub const fn with_effort(effort: AgentExecutionEffort) -> Self {
        Self {
            effort,
            timeout_secs: None,
            search_limit: None,
            reasoning_effort_override: None,
        }
    }

    /// Set an exact per-run timeout override in seconds.
    #[must_use]
    pub const fn with_timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = Some(timeout_secs);
        self
    }

    /// Set an exact per-run search tool call limit override.
    #[must_use]
    pub const fn with_search_limit(mut self, search_limit: usize) -> Self {
        self.search_limit = Some(search_limit);
        self
    }

    /// Set an explicit per-run provider reasoning effort override.
    #[must_use]
    pub const fn with_reasoning_effort(mut self, reasoning_effort: &'static str) -> Self {
        self.reasoning_effort_override = Some(reasoning_effort);
        self
    }

    pub(crate) const fn min_max_iterations(self) -> Option<usize> {
        match self.effort {
            AgentExecutionEffort::Standard => None,
            AgentExecutionEffort::Extended => Some(400),
            AgentExecutionEffort::Heavy => Some(512),
        }
    }

    pub(crate) const fn min_continuation_limit(self) -> Option<usize> {
        match self.effort {
            AgentExecutionEffort::Standard => None,
            AgentExecutionEffort::Extended => Some(50),
            AgentExecutionEffort::Heavy => Some(150),
        }
    }

    pub(crate) const fn min_timeout_secs(self) -> Option<u64> {
        match self.effort {
            AgentExecutionEffort::Standard => None,
            AgentExecutionEffort::Extended => Some(90 * 60),
            AgentExecutionEffort::Heavy => Some(180 * 60),
        }
    }

    pub(crate) const fn min_search_limit(self) -> Option<usize> {
        match self.effort {
            AgentExecutionEffort::Standard => None,
            AgentExecutionEffort::Extended => Some(30),
            AgentExecutionEffort::Heavy => Some(80),
        }
    }

    pub(crate) const fn reasoning_effort(self) -> Option<&'static str> {
        match self.reasoning_effort_override {
            Some(reasoning_effort) => Some(reasoning_effort),
            None => match self.effort {
                AgentExecutionEffort::Standard => None,
                AgentExecutionEffort::Extended | AgentExecutionEffort::Heavy => Some("high"),
            },
        }
    }
}

/// User input for one agent execution turn.
///
/// `content` remains the stable text projection. Attachments are safe refs only;
/// raw bytes are resolved later and never stored in hot memory.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentUserInput {
    /// Stable text projection used by prompts, compaction, and text-only routes.
    pub content: String,
    /// Safe attachment refs associated with this user turn.
    pub attachments: Vec<AgentMessageAttachment>,
}

impl AgentUserInput {
    /// Create text-only user input.
    #[must_use]
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            attachments: Vec::new(),
        }
    }

    /// Attach safe refs without changing the text projection.
    #[must_use]
    pub fn with_attachments(mut self, attachments: Vec<AgentMessageAttachment>) -> Self {
        self.attachments = attachments;
        self
    }

    /// Return the stable text projection.
    #[must_use]
    pub fn text_projection(&self) -> &str {
        &self.content
    }
}

/// Agent executor that runs tasks iteratively
pub struct AgentExecutor {
    runner: AgentRunner,
    session: AgentSession,
    settings: Arc<crate::config::AgentSettings>,
    model_routes_override: Option<Vec<ModelInfo>>,
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
    /// Agent paused because it is waiting for additional user input.
    WaitingForUserInput(PendingUserInput),
}
