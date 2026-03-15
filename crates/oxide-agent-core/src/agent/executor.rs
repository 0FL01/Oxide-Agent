//! Agent executor module
//!
//! Handles orchestration around the core agent runner, including
//! session lifecycle, skill prompts, and tool registry setup.

use super::hooks::{
    CompletionCheckHook, DelegationGuardHook, SearchBudgetHook, TimeoutReportHook,
    ToolAccessPolicyHook, WorkloadDistributorHook,
};
use super::memory::AgentMessage;
use super::profile::{AgentExecutionProfile, ToolAccessPolicy};
use super::prompt::create_agent_system_prompt;
use super::providers::{
    DelegationProvider, FileHosterProvider, ManagerControlPlaneProvider, ManagerTopicLifecycle,
    SandboxProvider, TodosProvider, YtdlpProvider,
};
use super::registry::ToolRegistry;
use super::runner::{AgentRunner, AgentRunnerConfig, AgentRunnerContext};
use super::session::AgentSession;
use super::skills::SkillRegistry;
use crate::agent::progress::AgentEvent;
use crate::config::{get_agent_search_limit, AGENT_TIMEOUT_SECS};
use crate::llm::LlmClient;
use crate::storage::StorageProvider;
use anyhow::{anyhow, Result};
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

#[cfg(feature = "crawl4ai")]
use super::providers::Crawl4aiProvider;
#[cfg(feature = "tavily")]
use super::providers::TavilyProvider;

// Re-export sanitize_xml_tags for backward compatibility
pub use super::recovery::sanitize_xml_tags as public_sanitize_xml_tags;

/// Agent executor that runs tasks iteratively
pub struct AgentExecutor {
    runner: AgentRunner,
    session: AgentSession,
    skill_registry: Option<SkillRegistry>,
    settings: Arc<crate::config::AgentSettings>,
    manager_control_plane: Option<ManagerControlPlaneContext>,
    execution_profile: AgentExecutionProfile,
    tool_policy_state: Arc<RwLock<ToolAccessPolicy>>,
}

#[derive(Clone)]
struct ManagerControlPlaneContext {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_lifecycle: Option<Arc<dyn ManagerTopicLifecycle>>,
}

impl AgentExecutor {
    /// Create a new agent executor
    #[must_use]
    pub fn new(
        llm_client: Arc<LlmClient>,
        session: AgentSession,
        settings: Arc<crate::config::AgentSettings>,
    ) -> Self {
        let tool_policy_state = Arc::new(RwLock::new(ToolAccessPolicy::default()));
        let mut runner = AgentRunner::new(llm_client.clone());
        runner.register_hook(Box::new(CompletionCheckHook::new()));
        runner.register_hook(Box::new(WorkloadDistributorHook::new()));
        runner.register_hook(Box::new(DelegationGuardHook::new()));
        runner.register_hook(Box::new(SearchBudgetHook::new(get_agent_search_limit())));
        runner.register_hook(Box::new(ToolAccessPolicyHook::new(Arc::clone(
            &tool_policy_state,
        ))));
        runner.register_hook(Box::new(TimeoutReportHook::new()));

        let skill_registry = match SkillRegistry::from_env(llm_client.clone()) {
            Ok(Some(registry)) => {
                info!(
                    skills_dir = %registry.skills_dir().display(),
                    "Skills system active"
                );
                Some(registry)
            }
            Ok(None) => {
                info!("Skills system inactive, will use AGENT.md or fallback prompt");
                None
            }
            Err(err) => {
                warn!(error = %err, "Failed to initialize skills registry, falling back to AGENT.md");
                None
            }
        };

        Self {
            runner,
            session,
            skill_registry,
            settings,
            manager_control_plane: None,
            execution_profile: AgentExecutionProfile::default(),
            tool_policy_state,
        }
    }

    /// Apply the latest execution profile for the next task run.
    pub fn set_execution_profile(&mut self, execution_profile: AgentExecutionProfile) {
        if let Ok(mut policy) = self.tool_policy_state.write() {
            *policy = execution_profile.tool_policy().clone();
        }
        self.execution_profile = execution_profile;
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

    /// Get a reference to the session
    #[must_use]
    pub const fn session(&self) -> &AgentSession {
        &self.session
    }

    /// Get a mutable reference to the session
    pub const fn session_mut(&mut self) -> &mut AgentSession {
        &mut self.session
    }

    /// Disable loop detection for the next execution attempt.
    pub fn disable_loop_detection_next_run(&mut self) {
        self.runner.disable_loop_detection_next_run();
    }

    /// Whether manager control-plane tools are enabled for this executor.
    #[must_use]
    pub fn manager_control_plane_enabled(&self) -> bool {
        self.manager_control_plane.is_some()
    }

    /// Get the last task text, if available.
    #[must_use]
    pub fn last_task(&self) -> Option<&str> {
        self.session.last_task.as_deref()
    }

    fn build_tool_registry(
        &self,
        todos_arc: Arc<Mutex<crate::agent::providers::TodoList>>,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TodosProvider::new(Arc::clone(&todos_arc))));

        let sandbox_scope = self.session.sandbox_scope().clone();
        let sandbox_provider = if let Some(tx) = progress_tx {
            SandboxProvider::new(sandbox_scope.clone()).with_progress_tx(tx.clone())
        } else {
            SandboxProvider::new(sandbox_scope.clone())
        };
        registry.register(Box::new(sandbox_provider));
        registry.register(Box::new(FileHosterProvider::new(sandbox_scope.clone())));

        let ytdlp_provider = if let Some(tx) = progress_tx {
            YtdlpProvider::new(sandbox_scope.clone()).with_progress_tx(tx.clone())
        } else {
            YtdlpProvider::new(sandbox_scope.clone())
        };
        registry.register(Box::new(ytdlp_provider));

        registry.register(Box::new(DelegationProvider::new(
            self.runner.llm_client(),
            sandbox_scope,
            self.settings.clone(),
        )));

        if let Some(control_plane) = &self.manager_control_plane {
            let mut manager_provider = ManagerControlPlaneProvider::new(
                Arc::clone(&control_plane.storage),
                control_plane.user_id,
            );
            if let Some(topic_lifecycle) = &control_plane.topic_lifecycle {
                manager_provider =
                    manager_provider.with_topic_lifecycle(Arc::clone(topic_lifecycle));
            }
            registry.register(Box::new(manager_provider));
        }

        // Register web search provider based on configuration
        let search_provider = crate::config::get_search_provider();
        match search_provider.as_str() {
            "tavily" => {
                #[cfg(feature = "tavily")]
                if let Ok(tavily_key) = std::env::var("TAVILY_API_KEY") {
                    if !tavily_key.is_empty() {
                        if let Ok(p) = TavilyProvider::new(&tavily_key) {
                            registry.register(Box::new(p));
                        }
                    }
                }
                #[cfg(not(feature = "tavily"))]
                warn!("Tavily requested but feature not enabled");
            }
            "crawl4ai" => {
                #[cfg(feature = "crawl4ai")]
                if let Ok(url) = std::env::var("CRAWL4AI_URL") {
                    if !url.is_empty() {
                        registry.register(Box::new(Crawl4aiProvider::new(&url)));
                    }
                }
                #[cfg(not(feature = "crawl4ai"))]
                warn!("Crawl4AI requested but feature not enabled");
            }
            _ => unreachable!(), // get_search_provider() guarantees valid value
        }

        registry
    }

    /// Execute a task with iterative tool calling (agentic loop)
    ///
    /// # Errors
    ///
    /// Returns an error if the LLM call fails, tool execution fails, or the iteration/timeout limits are exceeded.
    #[tracing::instrument(skip(self, progress_tx), fields(session_id = %self.session.session_id))]
    pub async fn execute(
        &mut self,
        task: &str,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<String> {
        self.session.start_task();
        let task_id = self.session.current_task_id.clone().unwrap_or_default();
        self.session.remember_task(task);
        info!(
            task = %task,
            task_id = %task_id,
            memory_messages = self.session.memory.get_messages().len(),
            memory_tokens = self.session.memory.token_count(),
            "Starting agent task"
        );

        self.session.memory.add_message(AgentMessage::user(task));

        let todos_arc = Arc::new(Mutex::new(self.session.memory.todos.clone()));

        let registry = self.build_tool_registry(Arc::clone(&todos_arc), progress_tx.as_ref());

        let tools = self
            .execution_profile
            .tool_policy()
            .filter_definitions(registry.all_tools());
        let (_, provider, _) = self.settings.get_configured_agent_model();
        let structured_output = !provider.eq_ignore_ascii_case("zai");
        let system_prompt = create_agent_system_prompt(
            task,
            &tools,
            structured_output,
            self.skill_registry.as_mut(),
            &mut self.session,
            self.execution_profile.prompt_instructions(),
        )
        .await;
        let mut messages =
            AgentRunner::convert_memory_to_messages(self.session.memory.get_messages());

        let mut ctx = AgentRunnerContext {
            task,
            system_prompt: &system_prompt,
            tools: &tools,
            registry: &registry,
            progress_tx: progress_tx.as_ref(),
            todos_arc: &todos_arc,
            task_id: &task_id,
            messages: &mut messages,
            agent: &mut self.session,
            skill_registry: self.skill_registry.as_mut(),
            config: {
                let (model_id, _, _) = self.settings.get_configured_agent_model();
                AgentRunnerConfig::new(
                    model_id,
                    crate::config::AGENT_MAX_ITERATIONS,
                    crate::config::AGENT_CONTINUATION_LIMIT,
                    self.settings.get_agent_timeout_secs(),
                )
            },
        };

        let timeout_duration = Duration::from_secs(AGENT_TIMEOUT_SECS);
        match timeout(timeout_duration, self.runner.run(&mut ctx)).await {
            Ok(inner) => match inner {
                Ok(res) => {
                    self.session.complete();
                    Ok(res)
                }
                Err(e) => {
                    self.session.fail(e.to_string());
                    Err(e)
                }
            },
            Err(_) => {
                self.session.timeout();
                let limit_mins = self.settings.get_agent_timeout_secs() / 60;
                Err(anyhow!(
                    "Task exceeded timeout limit ({} minutes)",
                    limit_mins
                ))
            }
        }
    }

    /// Check if the task has been cancelled
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.session.cancellation_token.is_cancelled()
    }

    /// Reset the executor and session
    pub fn reset(&mut self) {
        self.session.reset();
        self.runner.reset();
    }

    /// Check if the session is timed out
    #[must_use]
    pub fn is_timed_out(&self) -> bool {
        self.session.elapsed_secs() >= self.settings.get_agent_timeout_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::AgentExecutor;
    use crate::agent::providers::TodoList;
    use crate::agent::providers::{
        ForumTopicActionResult, ForumTopicCreateRequest, ForumTopicCreateResult,
        ForumTopicEditRequest, ForumTopicEditResult, ForumTopicThreadRequest,
        ManagerTopicLifecycle,
    };
    use crate::agent::session::AgentSession;
    use crate::llm::LlmClient;
    use crate::storage::{
        AppendAuditEventOptions, AuditEventRecord, MockStorageProvider, TopicBindingKind,
        TopicBindingRecord,
    };
    use anyhow::{bail, Result};
    use mockall::predicate::eq;
    use serde_json::json;
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::sync::Mutex;

    struct RecordingTopicLifecycle {
        create_calls: StdMutex<Vec<ForumTopicCreateRequest>>,
    }

    impl RecordingTopicLifecycle {
        fn new() -> Self {
            Self {
                create_calls: StdMutex::new(Vec::new()),
            }
        }

        fn create_calls(&self) -> Vec<ForumTopicCreateRequest> {
            match self.create_calls.lock() {
                Ok(calls) => calls.clone(),
                Err(_) => Vec::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl ManagerTopicLifecycle for RecordingTopicLifecycle {
        async fn forum_topic_create(
            &self,
            request: ForumTopicCreateRequest,
        ) -> Result<ForumTopicCreateResult> {
            if let Ok(mut calls) = self.create_calls.lock() {
                calls.push(request.clone());
            }
            Ok(ForumTopicCreateResult {
                chat_id: request.chat_id.unwrap_or(-100_555),
                thread_id: 313,
                name: request.name,
                icon_color: request.icon_color.unwrap_or(9_367_192),
                icon_custom_emoji_id: request.icon_custom_emoji_id,
            })
        }

        async fn forum_topic_edit(
            &self,
            _request: ForumTopicEditRequest,
        ) -> Result<ForumTopicEditResult> {
            bail!("forum_topic_edit is not used by this test lifecycle")
        }

        async fn forum_topic_close(
            &self,
            _request: ForumTopicThreadRequest,
        ) -> Result<ForumTopicActionResult> {
            bail!("forum_topic_close is not used by this test lifecycle")
        }

        async fn forum_topic_reopen(
            &self,
            _request: ForumTopicThreadRequest,
        ) -> Result<ForumTopicActionResult> {
            bail!("forum_topic_reopen is not used by this test lifecycle")
        }

        async fn forum_topic_delete(
            &self,
            _request: ForumTopicThreadRequest,
        ) -> Result<ForumTopicActionResult> {
            bail!("forum_topic_delete is not used by this test lifecycle")
        }
    }

    fn build_executor() -> AgentExecutor {
        let settings = Arc::new(crate::config::AgentSettings::default());
        let llm = Arc::new(LlmClient::new(settings.as_ref()));
        let session = AgentSession::new(9_i64.into());
        AgentExecutor::new(llm, session, settings)
    }

    fn build_audit_record(options: AppendAuditEventOptions) -> AuditEventRecord {
        AuditEventRecord {
            schema_version: 1,
            version: 1,
            event_id: "evt-1".to_string(),
            user_id: options.user_id,
            topic_id: options.topic_id,
            agent_id: options.agent_id,
            action: options.action,
            payload: options.payload,
            created_at: 100,
        }
    }

    #[tokio::test]
    async fn manager_enabled_registry_executes_manager_tool() {
        let mut mock = MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|user_id, topic_id| {
                Ok(Some(TopicBindingRecord {
                    schema_version: 1,
                    version: 3,
                    user_id,
                    topic_id,
                    agent_id: "agent-a".to_string(),
                    binding_kind: TopicBindingKind::Manual,
                    chat_id: None,
                    thread_id: None,
                    expires_at: None,
                    last_activity_at: Some(20),
                    created_at: 10,
                    updated_at: 20,
                }))
            });

        let executor = build_executor().with_manager_control_plane(Arc::new(mock), 77);
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let response = registry
            .execute("topic_binding_get", r#"{"topic_id":"topic-a"}"#, None, None)
            .await
            .expect("manager-enabled registry must route manager tool");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("manager tool response must be valid json");
        assert_eq!(parsed["found"], true);
        assert_eq!(parsed["binding"]["agent_id"], "agent-a");
    }

    #[tokio::test]
    async fn manager_disabled_registry_rejects_manager_tool() {
        let executor = build_executor();
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let err = registry
            .execute("topic_binding_get", r#"{"topic_id":"topic-a"}"#, None, None)
            .await
            .expect_err("manager-disabled registry must reject manager tools");

        assert!(err.to_string().contains("Unknown tool"));
    }

    #[tokio::test]
    async fn manager_dry_run_mutation_does_not_persist_via_executor_registry() {
        let mut mock = MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));
        mock.expect_upsert_topic_binding().times(0);
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.action == "topic_binding_set"
                    && options.payload.get("outcome") == Some(&json!("dry_run"))
            })
            .returning(|options| Ok(build_audit_record(options)));

        let executor = build_executor().with_manager_control_plane(Arc::new(mock), 77);
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let response = registry
            .execute(
                "topic_binding_set",
                r#"{"topic_id":"topic-a","agent_id":"agent-a","dry_run":true}"#,
                None,
                None,
            )
            .await
            .expect("dry-run manager mutation must succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("dry-run response must be valid json");
        assert_eq!(parsed["dry_run"], true);
        assert_eq!(parsed["preview"]["topic_id"], "topic-a");
    }

    #[tokio::test]
    async fn manager_dry_run_mutation_reports_audit_write_failure_non_fatally() {
        let mut mock = MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));
        mock.expect_upsert_topic_binding().times(0);
        mock.expect_append_audit_event().returning(|_| {
            Err(crate::storage::StorageError::Config(
                "audit unavailable".to_string(),
            ))
        });

        let executor = build_executor().with_manager_control_plane(Arc::new(mock), 77);
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let response = registry
            .execute(
                "topic_binding_set",
                r#"{"topic_id":"topic-a","agent_id":"agent-a","dry_run":true}"#,
                None,
                None,
            )
            .await
            .expect("dry-run manager mutation must remain non-fatal when audit write fails");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("dry-run response must be valid json");
        assert_eq!(parsed["dry_run"], true);
        assert_eq!(parsed["audit_status"], "write_failed");
        assert_eq!(parsed["preview"]["topic_id"], "topic-a");
    }

    #[tokio::test]
    async fn manager_executor_forum_topic_create_uses_lifecycle_with_non_fatal_audit() {
        let mut mock = MockStorageProvider::new();
        mock.expect_get_user_config()
            .returning(|_| Ok(crate::storage::UserConfig::default()));
        mock.expect_update_user_config().returning(|_, _| Ok(()));
        mock.expect_append_audit_event().returning(|_| {
            Err(crate::storage::StorageError::Config(
                "audit unavailable".to_string(),
            ))
        });

        let lifecycle = Arc::new(RecordingTopicLifecycle::new());
        let executor = build_executor()
            .with_manager_control_plane(Arc::new(mock), 77)
            .with_manager_topic_lifecycle(lifecycle.clone());
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let response = registry
            .execute(
                "forum_topic_create",
                r#"{"chat_id":-100777,"name":"runtime-topic"}"#,
                None,
                None,
            )
            .await
            .expect("forum_topic_create must succeed when lifecycle succeeds");

        let parsed: serde_json::Value = serde_json::from_str(&response)
            .expect("forum topic create response must be valid json");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["topic"]["thread_id"], 313);
        assert_eq!(parsed["topic"]["name"], "runtime-topic");
        assert_eq!(parsed["audit_status"], "write_failed");

        let calls = lifecycle.create_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].chat_id, Some(-100_777));
        assert_eq!(calls[0].name, "runtime-topic");
    }

    #[tokio::test]
    async fn manager_executor_forum_topic_create_dry_run_skips_lifecycle() {
        let mut mock = MockStorageProvider::new();
        mock.expect_append_audit_event()
            .returning(|options| Ok(build_audit_record(options)));

        let lifecycle = Arc::new(RecordingTopicLifecycle::new());
        let executor = build_executor()
            .with_manager_control_plane(Arc::new(mock), 77)
            .with_manager_topic_lifecycle(lifecycle.clone());
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let response = registry
            .execute(
                "forum_topic_create",
                r#"{"chat_id":-100777,"name":"dry-run","dry_run":true}"#,
                None,
                None,
            )
            .await
            .expect("dry-run forum_topic_create must succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("dry-run response must be valid json");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["dry_run"], true);
        assert_eq!(parsed["audit_status"], "written");
        assert!(lifecycle.create_calls().is_empty());
    }

    #[tokio::test]
    async fn manager_rollback_restores_snapshot_via_executor_registry() {
        let mut mock = MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|user_id, topic_id| {
                Ok(Some(TopicBindingRecord {
                    schema_version: 1,
                    version: 5,
                    user_id,
                    topic_id,
                    agent_id: "agent-current".to_string(),
                    binding_kind: TopicBindingKind::Manual,
                    chat_id: None,
                    thread_id: None,
                    expires_at: None,
                    last_activity_at: Some(20),
                    created_at: 10,
                    updated_at: 20,
                }))
            });
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(None), eq(200_usize))
            .returning(|_, _, _| {
                Ok(vec![AuditEventRecord {
                    schema_version: 1,
                    version: 4,
                    event_id: "evt-4".to_string(),
                    user_id: 77,
                    topic_id: Some("topic-a".to_string()),
                    agent_id: Some("agent-previous".to_string()),
                    action: "topic_binding_set".to_string(),
                    payload: json!({
                        "topic_id": "topic-a",
                        "previous": {
                            "schema_version": 1,
                            "version": 2,
                            "user_id": 77,
                            "topic_id": "topic-a",
                            "agent_id": "agent-previous",
                            "created_at": 1,
                            "updated_at": 2
                        },
                        "outcome": "applied"
                    }),
                    created_at: 30,
                }])
            });
        mock.expect_upsert_topic_binding()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.agent_id == "agent-previous"
            })
            .returning(|options| {
                Ok(TopicBindingRecord {
                    schema_version: 1,
                    version: 6,
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                    chat_id: options.chat_id.for_new_record(),
                    thread_id: options.thread_id.for_new_record(),
                    expires_at: options.expires_at.for_new_record(),
                    last_activity_at: options.last_activity_at,
                    created_at: 40,
                    updated_at: 50,
                })
            });
        mock.expect_delete_topic_binding().times(0);
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.action == "topic_binding_rollback"
                    && options.payload.get("operation") == Some(&json!("restore"))
            })
            .returning(|options| Ok(build_audit_record(options)));

        let executor = build_executor().with_manager_control_plane(Arc::new(mock), 77);
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let response = registry
            .execute(
                "topic_binding_rollback",
                r#"{"topic_id":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect("rollback restore path must succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("rollback response must be valid json");
        assert_eq!(parsed["operation"], "restore");
        assert_eq!(parsed["binding"]["agent_id"], "agent-previous");
    }

    #[tokio::test]
    async fn manager_rollback_deletes_when_snapshot_is_empty_via_executor_registry() {
        let mut mock = MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|user_id, topic_id| {
                Ok(Some(TopicBindingRecord {
                    schema_version: 1,
                    version: 5,
                    user_id,
                    topic_id,
                    agent_id: "agent-current".to_string(),
                    binding_kind: TopicBindingKind::Manual,
                    chat_id: None,
                    thread_id: None,
                    expires_at: None,
                    last_activity_at: Some(20),
                    created_at: 10,
                    updated_at: 20,
                }))
            });
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(None), eq(200_usize))
            .returning(|_, _, _| {
                Ok(vec![AuditEventRecord {
                    schema_version: 1,
                    version: 4,
                    event_id: "evt-4".to_string(),
                    user_id: 77,
                    topic_id: Some("topic-a".to_string()),
                    agent_id: Some("agent-current".to_string()),
                    action: "topic_binding_delete".to_string(),
                    payload: json!({
                        "topic_id": "topic-a",
                        "previous": null,
                        "outcome": "applied"
                    }),
                    created_at: 30,
                }])
            });
        mock.expect_upsert_topic_binding().times(0);
        mock.expect_delete_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(()));
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.action == "topic_binding_rollback"
                    && options.payload.get("operation") == Some(&json!("delete"))
            })
            .returning(|options| Ok(build_audit_record(options)));

        let executor = build_executor().with_manager_control_plane(Arc::new(mock), 77);
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let response = registry
            .execute(
                "topic_binding_rollback",
                r#"{"topic_id":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect("rollback delete path must succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("rollback response must be valid json");
        assert_eq!(parsed["operation"], "delete");
        assert!(parsed["binding"].is_null());
    }
}
