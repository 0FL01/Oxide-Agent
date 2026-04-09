use super::AgentExecutor;
use crate::agent::compaction::{CompactionOutcome, CompactionRequest, CompactionTrigger};
use crate::agent::progress::AgentEvent;
use crate::agent::prompt::create_agent_system_prompt;
use anyhow::{anyhow, Result};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::Duration;
use tracing::warn;

impl AgentExecutor {
    /// Manually compact the current Agent Mode hot context without running a task iteration.
    pub async fn compact_current_context(
        &mut self,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<CompactionOutcome> {
        let task = self
            .last_task()
            .map(str::to_string)
            .unwrap_or_else(|| "Continue the current Agent Mode session".to_string());
        let todos_arc = Arc::new(Mutex::new(self.session.memory.todos.clone()));
        let registry = self.build_tool_registry(Arc::clone(&todos_arc), progress_tx.as_ref());
        let tools = self
            .execution_profile
            .tool_policy()
            .filter_definitions(registry.all_tools());
        let model = self.settings.get_configured_agent_model();
        let structured_output = crate::llm::LlmClient::supports_structured_output_for_model(&model);
        let system_prompt = create_agent_system_prompt(
            &task,
            &tools,
            structured_output,
            self.skill_registry.as_mut(),
            &mut self.session,
            self.execution_profile.prompt_instructions(),
        )
        .await;
        let request = CompactionRequest::new(
            CompactionTrigger::Manual,
            &task,
            &system_prompt,
            &tools,
            &model.id,
            model.max_output_tokens,
            false,
        );

        warn!(
            model = %model.id,
            tool_count = tools.len(),
            task_len = task.len(),
            system_prompt_len = system_prompt.len(),
            "Manual compaction requested"
        );
        Self::emit_manual_compaction_started(progress_tx.as_ref()).await;
        let cancellation_token = self.session.cancellation_token.clone();
        let outcome = match Self::await_until_cancelled(
            cancellation_token,
            self.compaction_service
                .prepare_for_run(&request, &mut self.session),
        )
        .await
        {
            Some(Ok(outcome)) => outcome,
            Some(Err(error)) => {
                warn!(error = %error, "Manual compaction failed");
                Self::emit_manual_compaction_failed(progress_tx.as_ref(), error.to_string()).await;
                return Err(error);
            }
            None => {
                if let Some(tx) = progress_tx.as_ref() {
                    let _ = tx.send(AgentEvent::Cancelled).await;
                }
                return Err(anyhow!("Task cancelled by user"));
            }
        };
        warn!(
            applied = outcome.applied,
            budget_state = ?outcome.budget.state,
            hot_memory_tokens_before = outcome.token_count_before,
            hot_memory_tokens_after = outcome.token_count_after,
            collapsed_retry_attempts = outcome.error_retry_collapse.collapsed_attempt_count,
            collapsed_retry_messages = outcome.error_retry_collapse.dropped_message_count,
            externalized_count = outcome.externalization.externalized_count,
            pruned_count = outcome.pruning.pruned_count,
            reclaimed_tokens = outcome.reclaimed_hot_memory_tokens(),
            cleanup_reclaimed_tokens = outcome.reclaimed_cleanup_tokens(),
            summary_attempted = outcome.summary_generation.attempted,
            summary_used_fallback = outcome.summary_generation.used_fallback,
            archived_chunk_count = outcome.archive_persistence.archived_chunk_count,
            summary_updated = outcome.rebuild.inserted_summary,
            "Manual compaction completed"
        );
        if outcome.pruning.applied {
            Self::emit_manual_pruning_applied(progress_tx.as_ref(), &outcome).await;
        }
        Self::emit_manual_compaction_completed(progress_tx.as_ref(), &outcome).await;
        Ok(outcome)
    }

    /// Check if the task has been cancelled
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.session.cancellation_token.is_cancelled()
    }

    pub(super) fn agent_timeout_duration(&self) -> Duration {
        Duration::from_secs(self.settings.get_agent_timeout_secs())
    }

    pub(super) fn agent_timeout_error_message(&self) -> String {
        let limit_mins = self.settings.get_agent_timeout_secs() / 60;
        format!("Task exceeded timeout limit ({limit_mins} minutes)")
    }

    /// Reset the executor and session
    pub fn reset(&mut self) {
        self.session.reset();
        self.runner.reset();
    }

    /// Check if the session is timed out
    #[must_use]
    pub fn is_timed_out(&self) -> bool {
        self.session.is_processing()
            && self.session.elapsed_secs() >= self.settings.get_agent_timeout_secs()
    }

    async fn emit_manual_compaction_started(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::CompactionStarted {
                    trigger: CompactionTrigger::Manual,
                })
                .await;
        }
    }

    async fn emit_manual_pruning_applied(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        outcome: &CompactionOutcome,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::PruningApplied {
                    pruned_count: outcome.pruning.pruned_count,
                    reclaimed_tokens: outcome.pruning.reclaimed_tokens,
                })
                .await;
        }
    }

    async fn emit_manual_compaction_completed(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        outcome: &CompactionOutcome,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::CompactionCompleted {
                    trigger: CompactionTrigger::Manual,
                    applied: outcome.applied,
                    externalized_count: outcome.externalization.externalized_count,
                    pruned_count: outcome.pruning.pruned_count,
                    reclaimed_tokens: outcome.reclaimed_hot_memory_tokens(),
                    archived_chunk_count: outcome.archive_persistence.archived_chunk_count,
                    summary_updated: outcome.rebuild.inserted_summary,
                })
                .await;
        }
    }

    async fn emit_manual_compaction_failed(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        error: String,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::CompactionFailed {
                    trigger: CompactionTrigger::Manual,
                    error,
                })
                .await;
        }
    }

    /// Emit a milestone event for latency tracking.
    pub(super) async fn emit_milestone(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        name: &str,
    ) {
        if let Some(tx) = progress_tx {
            let timestamp_ms = chrono::Utc::now().timestamp_millis();
            let _ = tx
                .send(AgentEvent::Milestone {
                    name: name.to_string(),
                    timestamp_ms,
                })
                .await;
        }
    }
}
