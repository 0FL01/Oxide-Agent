use super::types::{AgentsMdContext, ManagerControlPlaneContext, TopicInfraContext};
use super::AgentExecutor;
use crate::agent::compaction::{
    CompactionService, CompactionSummarizer, CompactionSummarizerConfig,
};
use crate::agent::hooks::{
    CompletionCheckHook, DelegationGuardHook, HotContextHealthHook, RetrievalAdvisorHook,
    SearchBudgetHook, TimeoutReportHook, ToolAccessPolicyHook, WorkloadDistributorHook,
};
use crate::agent::providers::{ManagerTopicLifecycle, ReminderContext, SshApprovalRegistry};
use crate::agent::runner::AgentRunner;
use crate::agent::session::AgentSession;
use crate::agent::wiki_memory::WikiStore;
use crate::config::get_agent_search_limit;
use crate::config::ModelInfo;
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

fn format_dedicated_model_route(id: &str, provider: &str) -> Option<String> {
    if id.trim().is_empty() || provider.trim().is_empty() {
        None
    } else {
        Some(format!("{provider}/{id}"))
    }
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
        runner.register_hook(Box::new(HotContextHealthHook::with_limits(
            settings.get_hot_context_limits(),
        )));
        runner.register_hook(Box::new(RetrievalAdvisorHook::new()));
        Self::register_policy_controlled_hook(
            &mut runner,
            WorkloadDistributorHook::new(),
            Arc::clone(&hook_policy_state),
        );
        Self::register_policy_controlled_hook(
            &mut runner,
            DelegationGuardHook::new(),
            Arc::clone(&hook_policy_state),
        );
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

        let skill_registry = None;

        let compaction_service = {
            let (compaction_model_id, compaction_model_provider, _, timeout_secs) =
                settings.get_configured_compaction_model();
            let inherited_routes = settings.get_configured_agent_model_routes();
            let model_routes = settings.get_configured_compaction_model_routes(false);

            debug!(
                dedicated_compaction_route = ?format_dedicated_model_route(
                    &compaction_model_id,
                    &compaction_model_provider,
                ),
                inherited_agent_routes = ?format_model_routes(&inherited_routes),
                effective_compaction_routes = ?format_model_routes(&model_routes),
                timeout_secs,
                "Configured compaction summarizer routes"
            );

            CompactionService::default().with_summarizer(CompactionSummarizer::new(
                llm_client,
                CompactionSummarizerConfig {
                    model_routes,
                    timeout_secs,
                    ..CompactionSummarizerConfig::default()
                },
            ))
        };

        Self {
            runner,
            session,
            skill_registry,
            settings,
            agents_md: None,
            manager_control_plane: None,
            topic_infra: None,
            reminder_context: None,
            execution_profile: crate::agent::profile::AgentExecutionProfile::default(),
            tool_policy_state,
            hook_policy_state,
            compaction_service,
            wiki_memory_store: None,
            last_topic_infra_preflight_summary: None,
        }
    }

    /// Attach the durable LLM Wiki memory store used for bounded prompt context.
    #[must_use]
    pub fn with_wiki_memory_store(mut self, store: WikiStore) -> Self {
        self.wiki_memory_store = Some(store);
        self
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
            approvals: self
                .topic_infra
                .as_ref()
                .map_or_else(SshApprovalRegistry::new, |ctx| ctx.approvals.clone()),
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
}
