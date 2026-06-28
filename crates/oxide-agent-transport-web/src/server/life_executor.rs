//! Real `LifeRunExecutor` adapter that runs the ordinary `AgentExecutor`
//! with a stable life memory scope `(principal, "life", "main")`.
//!
//! This adapter bridges the transport-neutral `oxide-agent-life` worker seam
//! to the `oxide-agent-core` agent runtime. It reuses the same tool
//! registration, hydration, and checkpoint path as ordinary web sessions —
//! the only difference is the stable scope and the absence of a per-session
//! `SessionRegistry` entry (life runs are ephemeral executions, not
//! long-lived sessions).

use crate::session::WebSessionManager;
use async_trait::async_trait;
use oxide_agent_core::agent::providers::ReminderContext;
use oxide_agent_core::agent::{
    AgentExecutionOptions, AgentExecutionOutcome, AgentExecutor, AgentMemory,
    AgentMemoryCheckpoint, AgentMemoryScope, AgentSession, AgentUserInput, SessionId,
};
use oxide_agent_core::sandbox::SandboxScope;
use oxide_agent_core::storage::ReminderThreadKind;
use oxide_agent_core::storage::StorageProvider;
use oxide_agent_life::domain::{
    LifeSourceTransport, LifeTurn, LifeTurnRole, PrincipalUserId, RedactionState, RunId,
    TimestampMillis, TurnId,
};
use oxide_agent_life::storage::LifeStorageRepository;
use oxide_agent_life::worker::{
    LIFE_CONTEXT_KEY, LIFE_FLOW_ID, LifeRunExecutionOutcome, LifeRunExecutor, LifeWorkerError,
    LifeWorkerResult, LifeWorkerRunContext,
};
use std::sync::Arc;
use tracing::{info, warn};

/// Durable memory checkpoint that delegates to the configured storage provider.
///
/// Identical in behavior to `StorageFlowCheckpoint` in `session.rs` — both
/// write to `agent_memory_snapshots(user_id, context_key, flow_id)` via
/// `StorageProvider::save_agent_memory_for_flow`. Duplicated here because
/// `StorageFlowCheckpoint` is private to `session.rs` and the life executor
/// does not go through `WebSessionManager::create_session_with_model_selection`.
struct LifeMemoryCheckpoint {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: String,
    flow_id: String,
}

#[async_trait]
impl AgentMemoryCheckpoint for LifeMemoryCheckpoint {
    async fn persist(&self, memory: &AgentMemory) -> Result<(), anyhow::Error> {
        self.storage
            .save_agent_memory_for_flow(
                self.user_id,
                self.context_key.clone(),
                self.flow_id.clone(),
                memory,
            )
            .await?;
        Ok(())
    }
}

/// Real `LifeRunExecutor` that delegates to the ordinary `AgentExecutor`.
///
/// Constructed once at server startup and shared across all life runs via
/// `Arc<dyn LifeRunExecutor>`. Each `execute_life_run` call builds a fresh
/// `AgentExecutor` with the stable life scope, hydrates memory from durable
/// storage, runs the agent, persists the assistant turn, and forces a final
/// memory checkpoint.
pub struct LifeAgentExecutor {
    llm: Arc<oxide_agent_core::llm::LlmClient>,
    settings: Arc<oxide_agent_core::config::AgentSettings>,
    storage: Arc<dyn StorageProvider>,
    life_storage: Arc<oxide_agent_life::storage::SqlxLifeStorage>,
}

impl LifeAgentExecutor {
    /// Create a new life executor adapter.
    ///
    /// The `WebSessionManager` provides the LLM client, agent settings, and
    /// storage provider — the same infrastructure used by ordinary web
    /// sessions. The `SqlxLifeStorage` is used for assistant turn persistence.
    #[must_use]
    pub fn new(
        session_manager: &WebSessionManager,
        life_storage: Arc<oxide_agent_life::storage::SqlxLifeStorage>,
    ) -> Self {
        Self {
            llm: session_manager.llm_client(),
            settings: session_manager.agent_settings(),
            storage: session_manager.storage(),
            life_storage,
        }
    }

    /// Current time in milliseconds.
    fn now_millis() -> Result<i64, LifeWorkerError> {
        let duration = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| LifeWorkerError::Clock(error.to_string()))?;
        i64::try_from(duration.as_millis())
            .map_err(|error| LifeWorkerError::Clock(error.to_string()))
    }

    /// Hydrate `AgentMemory` from durable storage for the stable life scope.
    async fn hydrate_memory(&self, session: &mut AgentSession, user_id: i64) {
        match self
            .storage
            .load_agent_memory_for_flow(
                user_id,
                LIFE_CONTEXT_KEY.to_string(),
                LIFE_FLOW_ID.to_string(),
            )
            .await
        {
            Ok(Some(memory)) => {
                let message_count = memory.get_messages().len();
                info!(
                    user_id,
                    message_count,
                    context_key = LIFE_CONTEXT_KEY,
                    flow_id = LIFE_FLOW_ID,
                    "life agent memory hydrated from durable storage"
                );
                session.memory = memory;
                session.restore_last_task_from_memory();
            }
            Ok(None) => {
                info!(
                    user_id,
                    context_key = LIFE_CONTEXT_KEY,
                    flow_id = LIFE_FLOW_ID,
                    "no persisted life agent memory found; starting with empty memory"
                );
            }
            Err(error) => {
                warn!(
                    user_id,
                    context_key = LIFE_CONTEXT_KEY,
                    flow_id = LIFE_FLOW_ID,
                    ?error,
                    "failed to load persisted life agent memory; continuing with empty memory"
                );
            }
        }
    }

    /// Build a fully-configured `AgentExecutor` for a life run.
    ///
    /// Mirrors `WebSessionManager::create_session_with_model_selection` but
    /// with the stable life scope instead of a per-session scope.
    fn build_executor(
        &self,
        principal_user_id: PrincipalUserId,
        session: AgentSession,
    ) -> AgentExecutor {
        let user_id = principal_user_id.get();
        let mut executor =
            AgentExecutor::new(Arc::clone(&self.llm), session, Arc::clone(&self.settings))
                .with_storage(Arc::clone(&self.storage));

        if self.settings.is_wiki_memory_enabled() {
            executor = executor.with_wiki_memory_store(
                oxide_agent_core::agent::WikiStore::from_storage_provider(
                    Arc::clone(&self.storage),
                    "",
                ),
            );
        }

        executor.set_agents_md_context(
            Arc::clone(&self.storage),
            user_id,
            LIFE_CONTEXT_KEY.to_string(),
        );

        executor.set_reminder_context(ReminderContext {
            storage: Arc::clone(&self.storage),
            user_id,
            context_key: LIFE_CONTEXT_KEY.to_string(),
            flow_id: LIFE_FLOW_ID.to_string(),
            chat_id: user_id,
            thread_id: None,
            thread_kind: ReminderThreadKind::None,
            notifier: None,
        });

        executor
    }

    /// Persist the assistant response as a `life_turns` row and link it to the run.
    async fn persist_assistant_turn(
        &self,
        principal_user_id: PrincipalUserId,
        run_id: RunId,
        content: &str,
        now: TimestampMillis,
    ) -> Result<TurnId, LifeWorkerError> {
        let turn_id = TurnId::new_v4();
        let turn = LifeTurn {
            turn_id,
            principal_user_id,
            run_id: Some(run_id),
            role: LifeTurnRole::Assistant,
            source_transport: LifeSourceTransport::Web,
            source_ref: None,
            content: content.to_owned(),
            attachments: serde_json::json!([]),
            transport_metadata: serde_json::json!({}),
            redaction_state: RedactionState::Clean,
            created_at: now,
        };
        self.life_storage
            .append_turn(&turn)
            .await
            .map_err(LifeWorkerError::Storage)?;
        // The assistant turn is created with run_id already set, so no
        // separate link_turn_to_run call is needed.
        Ok(turn_id)
    }
}

#[async_trait]
impl LifeRunExecutor for LifeAgentExecutor {
    async fn execute_life_run(
        &self,
        context: LifeWorkerRunContext,
    ) -> LifeWorkerResult<LifeRunExecutionOutcome> {
        let principal_user_id = context.run.input.principal_user_id;
        let run_id = context.run.run_id;
        let user_content = context.run.user_content.clone();
        let user_id = principal_user_id.get();

        // 1. Build AgentSession with stable life scope and sandbox scope.
        let session_id = SessionId::from(user_id);
        let sandbox_scope = SandboxScope::new(user_id, LIFE_CONTEXT_KEY.to_string());
        let memory_scope = AgentMemoryScope::new(user_id, LIFE_CONTEXT_KEY, LIFE_FLOW_ID);
        let mut session = AgentSession::new_with_scopes(session_id, sandbox_scope, memory_scope);

        // 2. Hydrate AgentMemory from durable storage (agent_memory_snapshots).
        self.hydrate_memory(&mut session, user_id).await;

        // 3. Install durable memory checkpoint — same mechanism as ordinary
        //    web sessions. This ensures memory survives across runs and
        //    backend restarts.
        session.set_memory_checkpoint(Arc::new(LifeMemoryCheckpoint {
            storage: Arc::clone(&self.storage),
            user_id,
            context_key: LIFE_CONTEXT_KEY.to_string(),
            flow_id: LIFE_FLOW_ID.to_string(),
        }));

        // 4. Build the executor with the same tool registration as ordinary
        //    sessions (wiki memory, AGENTS.md, reminders, storage).
        let mut executor = self.build_executor(principal_user_id, session);

        // 5. Execute the user input. No progress_tx for now — C4 will add
        //    the AgentEvent → life_events bridge.
        let outcome = executor
            .execute_user_input_with_options(
                AgentUserInput::new(user_content),
                None,
                AgentExecutionOptions::default(),
            )
            .await
            .map_err(|error| LifeWorkerError::Executor(error.to_string()))?;

        // 6. Force a synchronous final memory checkpoint so the full
        //    conversation history (including this run's messages) is
        //    durably persisted before the run completes.
        executor
            .session()
            .persist_memory_checkpoint()
            .await
            .map_err(|error| LifeWorkerError::Executor(error.to_string()))?;

        let checkpoint_at_millis = Self::now_millis()?;
        let final_checkpoint_at = TimestampMillis::new(checkpoint_at_millis);

        // 7. Extract the assistant response and persist it as a life_turn.
        let assistant_content = match outcome {
            AgentExecutionOutcome::Completed(response) => response,
            AgentExecutionOutcome::WaitingForUserInput(_) => {
                // Life mode does not support pausing yet — treat as an error
                // so the run is marked failed and the user can retry.
                return Err(LifeWorkerError::Executor(
                    "life run paused for user input — not yet supported in permanent chat"
                        .to_owned(),
                ));
            }
        };

        let now = TimestampMillis::new(Self::now_millis()?);
        self.persist_assistant_turn(principal_user_id, run_id, &assistant_content, now)
            .await?;

        // 8. Serialize the final memory snapshot for the outcome (informational).
        let final_memory =
            serde_json::to_value(&executor.session().memory).unwrap_or(serde_json::json!({}));

        info!(
            user_id,
            run_id = %run_id,
            context_key = LIFE_CONTEXT_KEY,
            flow_id = LIFE_FLOW_ID,
            "life run completed: assistant turn persisted, memory checkpoint saved"
        );

        Ok(LifeRunExecutionOutcome {
            final_checkpoint_at,
            final_memory,
            final_memory_schema_version: 1,
        })
    }
}
