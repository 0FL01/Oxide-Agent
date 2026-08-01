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
use crate::web_transport::map_agent_event_without_file_storage;
use async_trait::async_trait;
use oxide_agent_core::agent::progress::AgentEvent;
use oxide_agent_core::agent::providers::ReminderContext;
use oxide_agent_core::agent::{
    AgentExecutionOptions, AgentExecutionOutcome, AgentExecutor, AgentMemory,
    AgentMemoryCheckpoint, AgentMemoryScope, AgentSession, AgentUserInput, SessionId,
};
use oxide_agent_core::sandbox::SandboxScope;
use oxide_agent_core::storage::ReminderThreadKind;
use oxide_agent_core::storage::StorageProvider;
use oxide_agent_life::domain::{
    EventId, INTERNAL_TRANSPORT_ID, LifeEvent, LifeRunStatus, LifeTransportId, LifeTurn,
    LifeTurnRole, PrincipalUserId, RedactionState, RunId, TimestampMillis, TurnId,
};
use oxide_agent_life::storage::LifeStorageRepository;
use oxide_agent_life::worker::{
    LIFE_CONTEXT_KEY, LIFE_FLOW_ID, LifeRunExecutionOutcome, LifeRunExecutor, LifeWorkerError,
    LifeWorkerResult, LifeWorkerRunContext,
};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

const LIFE_RUN_CANCELLATION_POLL_INTERVAL: Duration = Duration::from_secs(1);

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

/// Map an `AgentEvent` to a `(kind, payload)` pair for a `life_events` row.
///
/// Reuses the same event-mapping logic as the ordinary web transport
/// (`browser_event_parts`), passing `None` for file-storage params since
/// life mode does not handle file delivery at the event-bridge level.
/// The payload encodes `summary`, `payload`, `redacted`, and `truncated`
/// so the web UI activity panel can render life events using the same
/// `PersistedTaskEvent`-compatible shape.
pub(crate) fn agent_event_to_life_parts(event: &AgentEvent) -> (String, serde_json::Value) {
    let (task_kind, summary, payload, redacted, truncated) =
        map_agent_event_without_file_storage(event);
    let kind_str = serde_json::to_value(&task_kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_string());
    let life_payload = json!({
        "summary": summary,
        "payload": payload,
        "redacted": redacted,
        "truncated": truncated,
    });
    (kind_str, life_payload)
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
            source_transport: LifeTransportId::new(INTERNAL_TRANSPORT_ID)
                .expect("internal transport id"),
            source_ref: None,
            content: content.to_owned(),
            attachments: serde_json::json!([]),
            transport_metadata: serde_json::json!({}),
            redaction_state: RedactionState::Clean,
            created_at: now,
        };
        let deliveries = self
            .life_storage
            .append_assistant_turn_and_enqueue_deliveries(&turn, now)
            .await
            .map_err(LifeWorkerError::Storage)?;
        // The assistant turn is created with run_id already set, so no
        // separate link_turn_to_run call is needed.
        tracing::debug!(
            run_id = %run_id,
            turn_id = %turn_id,
            delivery_count = deliveries.len(),
            "life assistant turn persisted and delivery outbox enqueued"
        );
        Ok(turn_id)
    }

    async fn watch_durable_cancellation(
        life_storage: Arc<oxide_agent_life::storage::SqlxLifeStorage>,
        principal_user_id: PrincipalUserId,
        run_id: RunId,
        cancellation_token: CancellationToken,
    ) {
        let mut interval = tokio::time::interval(LIFE_RUN_CANCELLATION_POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                () = cancellation_token.cancelled() => return,
                _ = interval.tick() => {
                    match life_storage.run_status(principal_user_id, run_id).await {
                        Ok(Some(LifeRunStatus::Running)) => {}
                        Ok(Some(status)) => {
                            info!(run_id = %run_id, ?status, "life run durable status is terminal; cancelling executor token");
                            cancellation_token.cancel();
                            return;
                        }
                        Ok(None) => {
                            warn!(run_id = %run_id, "life run disappeared while executing; cancelling executor token");
                            cancellation_token.cancel();
                            return;
                        }
                        Err(error) => {
                            warn!(run_id = %run_id, ?error, "failed to poll durable life run cancellation status");
                        }
                    }
                }
            }
        }
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
        let cancellation_token = CancellationToken::new();
        session.cancellation_token = cancellation_token.clone();
        let cancellation_watcher = tokio::spawn(Self::watch_durable_cancellation(
            Arc::clone(&self.life_storage),
            principal_user_id,
            run_id,
            cancellation_token.clone(),
        ));

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
        //    sessions (AGENTS.md, reminders, storage).
        let mut executor = self.build_executor(principal_user_id, session);

        // 5. Create the AgentEvent → life_events bridge. The consumer task
        //    receives events from the executor and appends life_events rows
        //    with monotonic seq, scoped by run_id.
        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(128);
        let bridge_storage = Arc::clone(&self.life_storage);
        let bridge_run_id = run_id;
        let event_bridge = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let (kind, payload) = agent_event_to_life_parts(&event);
                let seq = match bridge_storage.next_event_seq(bridge_run_id).await {
                    Ok(seq) => seq,
                    Err(error) => {
                        warn!(run_id = %bridge_run_id, ?error, "failed to get next life event seq");
                        continue;
                    }
                };
                let now = match Self::now_millis() {
                    Ok(ms) => TimestampMillis::new(ms),
                    Err(error) => {
                        warn!(run_id = %bridge_run_id, ?error, "clock error in life event bridge");
                        continue;
                    }
                };
                let life_event = LifeEvent {
                    event_id: EventId::new_v4(),
                    run_id: bridge_run_id,
                    seq,
                    kind,
                    payload,
                    created_at: now,
                };
                if let Err(error) = bridge_storage.append_event(&life_event).await {
                    warn!(run_id = %bridge_run_id, ?error, "failed to append life event");
                }
            }
        });

        // 6. Execute the user input with progress events bridged to
        //    life_events. The executor drops `event_tx` when it finishes,
        //    which closes the channel and ends the consumer loop.
        let execution_result = executor
            .execute_user_input_with_options(
                AgentUserInput::new(user_content),
                Some(event_tx),
                AgentExecutionOptions::default(),
            )
            .await;

        // 7. Await the event bridge — the channel is closed when the
        //    executor drops its sender, ensuring all events are flushed
        //    before the run completes.
        let _ = event_bridge.await;
        cancellation_watcher.abort();

        let outcome = match execution_result {
            Ok(outcome) => outcome,
            Err(_error) if cancellation_token.is_cancelled() => {
                return Err(LifeWorkerError::Cancelled { run_id });
            }
            Err(error) => return Err(LifeWorkerError::Executor(error.to_string())),
        };

        // 8. Force a synchronous final memory checkpoint so the full
        //    conversation history (including this run's messages) is
        //    durably persisted before the run completes.
        executor
            .session()
            .persist_memory_checkpoint()
            .await
            .map_err(|error| LifeWorkerError::Executor(error.to_string()))?;

        let checkpoint_at_millis = Self::now_millis()?;
        let final_checkpoint_at = TimestampMillis::new(checkpoint_at_millis);

        // 9. Extract the assistant response and persist it as a life_turn.
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

        info!(
            user_id,
            run_id = %run_id,
            context_key = LIFE_CONTEXT_KEY,
            flow_id = LIFE_FLOW_ID,
            "life run completed: assistant turn persisted, memory checkpoint saved"
        );

        Ok(LifeRunExecutionOutcome {
            final_checkpoint_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_agent_core::agent::progress::{AgentEvent, AgentEventSource};

    #[test]
    fn maps_tool_call() {
        let event = AgentEvent::ToolCall {
            id: "call-1".to_string(),
            source: AgentEventSource::Root,
            name: "execute_command".to_string(),
            input: "{\"command\":\"ls -la\"}".to_string(),
            command_preview: Some("ls -la".to_string()),
        };
        let (kind, payload) = agent_event_to_life_parts(&event);
        assert_eq!(kind, "tool_call");
        assert_eq!(payload["summary"], "execute_command");
        assert_eq!(payload["payload"]["name"], "execute_command");
        assert_eq!(payload["payload"]["id"], "call-1");
        assert_eq!(payload["payload"]["source"], "root");
        assert_eq!(payload["redacted"], false);
    }

    #[test]
    fn maps_thinking() {
        let event = AgentEvent::Finished;
        let (kind, payload) = agent_event_to_life_parts(&event);
        assert_eq!(kind, "finished");
        assert_eq!(payload["summary"], "Finished");
    }

    #[test]
    fn maps_tool_result() {
        let event = AgentEvent::ToolResult {
            id: "call-1".to_string(),
            source: AgentEventSource::Root,
            name: "execute_command".to_string(),
            output: "{\"success\":true,\"output\":\"done\"}".to_string(),
            success: true,
        };
        let (kind, payload) = agent_event_to_life_parts(&event);
        assert_eq!(kind, "tool_result");
        assert_eq!(payload["payload"]["success"], true);
        assert_eq!(payload["payload"]["name"], "execute_command");
    }

    #[test]
    fn maps_sub_agent_tool_call() {
        let inner = AgentEvent::ToolCall {
            id: "sub-call-1".to_string(),
            source: AgentEventSource::SubAgent,
            name: "read_file".to_string(),
            input: "{\"path\":\"/tmp/test\"}".to_string(),
            command_preview: None,
        };
        let event = AgentEvent::SubAgent {
            sub_agent_id: "sub-1".to_string(),
            sub_agent_name: "research-agent".to_string(),
            event: Box::new(inner),
        };
        let (kind, payload) = agent_event_to_life_parts(&event);
        assert_eq!(kind, "tool_call");
        assert_eq!(payload["payload"]["name"], "read_file");
        assert_eq!(payload["payload"]["source"], "sub_agent");
        assert_eq!(payload["payload"]["source_id"], "sub-1");
        assert_eq!(payload["payload"]["source_name"], "research-agent");
    }

    #[test]
    fn maps_continuation() {
        let event = AgentEvent::Continuation {
            source: AgentEventSource::Root,
            reason: "incomplete todos".to_string(),
            count: 2,
        };
        let (kind, payload) = agent_event_to_life_parts(&event);
        assert_eq!(kind, "continuation");
        assert_eq!(payload["payload"]["count"], 2);
        assert_eq!(payload["payload"]["reason"], "incomplete todos");
    }

    #[test]
    fn maps_todos_updated() {
        use oxide_agent_core::agent::providers::TodoList;
        let event = AgentEvent::TodosUpdated {
            source: AgentEventSource::Root,
            todos: TodoList::default(),
        };
        let (kind, payload) = agent_event_to_life_parts(&event);
        assert_eq!(kind, "todos_updated");
        assert_eq!(payload["payload"]["source"], "root");
    }

    #[test]
    fn maps_error() {
        let event = AgentEvent::Error("something went wrong".to_string());
        let (kind, payload) = agent_event_to_life_parts(&event);
        assert_eq!(kind, "error");
        assert_eq!(payload["summary"], "Error");
        assert_eq!(payload["payload"]["message"], "something went wrong");
    }
}
