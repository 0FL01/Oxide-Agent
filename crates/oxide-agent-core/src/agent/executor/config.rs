use super::AgentExecutor;
use super::types::{AgentsMdContext, ManagerControlPlaneContext, TopicInfraContext};
use crate::agent::compaction::CompactionController;
use crate::agent::hooks::{
    CompletionCheckHook, HotContextHealthHook, SearchBudgetHook, TimeoutReportHook,
    ToolAccessPolicyHook,
};
use crate::agent::providers::{ManagerTopicLifecycle, ReminderContext};
use crate::agent::runner::AgentRunner;
use crate::agent::session::AgentSession;
use crate::config::ModelInfo;
use crate::config::get_agent_search_limit;
use crate::llm::LlmClient;
use crate::storage::{StorageProvider, TopicInfraConfigRecord};
use std::sync::Arc;
use tracing::debug;

fn format_model_routes(routes: &[ModelInfo]) -> Vec<String> {
    routes
        .iter()
        .map(|route| format!("{}/{}", route.provider, route.id))
        .collect()
}

impl AgentExecutor {
    /// Create a new agent executor
    #[must_use]
    pub fn new(
        llm_client: Arc<LlmClient>,
        mut session: AgentSession,
        settings: Arc<crate::config::AgentSettings>,
    ) -> Self {
        session.set_context_window_tokens(settings.get_agent_internal_context_budget_tokens());
        let tool_policy_state = Arc::new(std::sync::RwLock::new(
            crate::agent::profile::ToolAccessPolicy::default(),
        ));
        let hook_policy_state = Arc::new(std::sync::RwLock::new(
            crate::agent::profile::HookAccessPolicy::default(),
        ));
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        runner.register_hook(Box::new(CompletionCheckHook::new()));
        runner.register_hook(Box::new(HotContextHealthHook::new()));
        Self::register_policy_controlled_hook(
            &mut runner,
            SearchBudgetHook::new(get_agent_search_limit()),
            Arc::clone(&hook_policy_state),
        );
        runner.register_hook(Box::new(ToolAccessPolicyHook::new(Arc::clone(
            &tool_policy_state,
        ))));
        Self::register_policy_controlled_hook(
            &mut runner,
            TimeoutReportHook::new(),
            Arc::clone(&hook_policy_state),
        );

        debug!(
            active_agent_routes = ?format_model_routes(&settings.get_configured_agent_model_routes()),
            "Configured runtime compaction to use active agent routes"
        );
        let compaction_controller = CompactionController::local_llm(
            Arc::clone(&llm_client),
            settings.get_agent_timeout_secs(),
        );

        Self {
            runner,
            session,
            settings,
            model_override: None,
            agents_md: None,
            manager_control_plane: None,
            topic_infra: None,
            reminder_context: None,
            execution_profile: crate::agent::profile::AgentExecutionProfile::default(),
            tool_policy_state,
            hook_policy_state,
            compaction_controller,
            last_topic_infra_preflight_summary: None,
            storage: None,
        }
    }

    /// Apply the latest execution profile for the next task run.
    pub fn set_execution_profile(
        &mut self,
        execution_profile: crate::agent::profile::AgentExecutionProfile,
    ) {
        if let Ok(mut policy) = self.tool_policy_state.write() {
            *policy = execution_profile.tool_policy().clone();
        }
        if let Ok(mut policy) = self.hook_policy_state.write() {
            *policy = execution_profile.hook_policy().clone();
        }
        self.execution_profile = execution_profile;
    }

    /// Override the active model for the next execution.
    pub fn set_model_override(&mut self, model: Option<ModelInfo>) {
        let context_budget = model.as_ref().map_or_else(
            || self.settings.get_agent_internal_context_budget_tokens(),
            |model| self.settings.agent_internal_context_budget_for_model(model),
        );
        self.session.set_context_window_tokens(context_budget);
        self.model_override = model;
    }

    /// Return the currently configured per-executor model override.
    #[must_use]
    pub const fn model_override(&self) -> Option<&ModelInfo> {
        self.model_override.as_ref()
    }

    /// Attach topic-scoped AGENTS.md tooling.
    pub fn set_agents_md_context(
        &mut self,
        storage: Arc<dyn StorageProvider>,
        user_id: i64,
        topic_id: String,
    ) {
        self.agents_md = Some(AgentsMdContext {
            storage,
            user_id,
            topic_id,
        });
    }

    /// Attach or clear topic-scoped infrastructure tooling.
    pub fn set_topic_infra(
        &mut self,
        storage: Arc<dyn StorageProvider>,
        user_id: i64,
        topic_id: String,
        config: Option<TopicInfraConfigRecord>,
    ) {
        self.topic_infra = config.map(|config| TopicInfraContext {
            storage,
            user_id,
            topic_id,
            config,
        });
    }

    /// Attach or clear reminder scheduling context for this executor.
    pub fn set_reminder_context(&mut self, context: ReminderContext) {
        self.reminder_context = Some(context);
    }

    /// Attach user-scoped storage for manager control-plane tools.
    #[must_use]
    pub fn with_manager_control_plane(
        mut self,
        storage: Arc<dyn StorageProvider>,
        user_id: i64,
    ) -> Self {
        self.manager_control_plane = Some(ManagerControlPlaneContext {
            storage,
            user_id,
            topic_lifecycle: None,
        });
        self
    }

    /// Attach transport forum topic lifecycle for manager tools.
    #[must_use]
    pub fn with_manager_topic_lifecycle(
        mut self,
        topic_lifecycle: Arc<dyn ManagerTopicLifecycle>,
    ) -> Self {
        if let Some(control_plane) = self.manager_control_plane.as_mut() {
            control_plane.topic_lifecycle = Some(topic_lifecycle);
        }
        self
    }

    /// Attach durable storage for tool modules that need Postgres
    /// (e.g. browser-live screenshot artifacts).
    #[must_use]
    pub fn with_storage(mut self, storage: Arc<dyn StorageProvider>) -> Self {
        self.storage = Some(storage);
        self
    }
}
