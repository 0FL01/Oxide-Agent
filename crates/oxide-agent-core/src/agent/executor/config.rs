use super::types::{AgentsMdContext, ManagerControlPlaneContext, TopicInfraContext};
use super::AgentExecutor;
use crate::agent::compaction::{
    CompactionService, CompactionSummarizer, CompactionSummarizerConfig,
};
use crate::agent::hooks::{
    CompletionCheckHook, DelegationGuardHook, HotContextHealthHook, RetrievalAdvisorHook,
    SearchBudgetHook, TimeoutReportHook, ToolAccessPolicyHook, WorkloadDistributorHook,
};
use crate::agent::persistent_memory::{
    LlmMemoryEmbeddingGenerator, LlmMemoryTaskClassifier, LlmPostRunMemoryWriter,
    PersistentMemoryCoordinator, PersistentMemoryEmbeddingIndexer, PersistentMemoryStore,
    PostRunMemoryWriterConfig,
};
use crate::agent::providers::{ManagerTopicLifecycle, ReminderContext, SshApprovalRegistry};
use crate::agent::runner::AgentRunner;
use crate::agent::session::AgentSession;
use crate::config::get_agent_search_limit;
use crate::llm::LlmClient;
use crate::storage::{StorageMemoryRepository, StorageProvider, TopicInfraConfigRecord};
use std::sync::Arc;
use tracing::warn;

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
            let (_, _, _, timeout_secs) = settings.get_configured_compaction_model();
            CompactionService::default().with_summarizer(CompactionSummarizer::new(
                llm_client,
                CompactionSummarizerConfig {
                    model_routes: settings.get_configured_compaction_model_routes(false),
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
            memory_store: None,
            memory_artifact_storage: None,
            agents_md: None,
            manager_control_plane: None,
            topic_infra: None,
            reminder_context: None,
            execution_profile: crate::agent::profile::AgentExecutionProfile::default(),
            tool_policy_state,
            hook_policy_state,
            compaction_service,
            persistent_memory: None,
            memory_classifier: None,
            last_topic_infra_preflight_summary: None,
        }
    }

    /// Attach a custom persistent-memory store used for Stage-4 durable writes.
    #[must_use]
    pub fn with_persistent_memory_store(self, store: Arc<dyn PersistentMemoryStore>) -> Self {
        self.configure_persistent_memory(store, None)
    }

    /// Attach a custom persistent-memory store with artifact storage for archive reads.
    #[must_use]
    pub fn with_persistent_memory_store_and_artifact_storage(
        self,
        store: Arc<dyn PersistentMemoryStore>,
        artifact_storage: Arc<dyn StorageProvider>,
    ) -> Self {
        self.configure_persistent_memory(store, Some(artifact_storage))
    }

    /// Attach a storage-backed persistent-memory repository for Stage-4 durable writes.
    #[must_use]
    pub fn with_storage_memory_repository(self, storage: Arc<dyn StorageProvider>) -> Self {
        let repository: Arc<dyn PersistentMemoryStore> =
            Arc::new(StorageMemoryRepository::new(Arc::clone(&storage)));
        self.configure_persistent_memory(repository, Some(storage))
    }

    pub(super) fn configure_persistent_memory(
        mut self,
        store: Arc<dyn PersistentMemoryStore>,
        artifact_storage: Option<Arc<dyn StorageProvider>>,
    ) -> Self {
        self.memory_store = Some(Arc::clone(&store));
        self.memory_artifact_storage = artifact_storage;
        let classifier_model = self.settings.get_configured_memory_classifier_model();
        if !crate::llm::LlmClient::supports_structured_output_for_model(&classifier_model) {
            warn!(
                provider = %classifier_model.provider,
                model = %classifier_model.id,
                "configured memory classifier route lacks structured output; using text JSON fallback"
            );
        }
        self.memory_classifier = Some(Arc::new(LlmMemoryTaskClassifier::new(
            self.runner.llm_client(),
            classifier_model,
        )));
        let mut coordinator = PersistentMemoryCoordinator::new(Arc::clone(&store));
        let (_, _, _, post_run_writer_timeout_secs) =
            self.settings.get_configured_compaction_model();
        coordinator = coordinator.with_memory_writer(Arc::new(LlmPostRunMemoryWriter::new(
            self.runner.llm_client(),
            PostRunMemoryWriterConfig {
                model_routes: self.settings.get_configured_compaction_model_routes(false),
                timeout_secs: post_run_writer_timeout_secs,
                ..PostRunMemoryWriterConfig::default()
            },
        )));
        if let (Some(model_id), true) = (
            self.settings.embedding_model_id.clone(),
            self.runner.llm_client().is_embedding_available(),
        ) {
            coordinator = coordinator.with_embedding_indexer(
                PersistentMemoryEmbeddingIndexer::new_with_store(
                    store,
                    Arc::new(LlmMemoryEmbeddingGenerator::new(self.runner.llm_client())),
                    model_id,
                ),
            );
        }
        self.persistent_memory = Some(coordinator);
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
