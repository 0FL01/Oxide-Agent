use super::{AgentExecutionOptions, AgentExecutor};
use crate::agent::compaction::{
    CompactionBackend, CompactionPhase, CompactionReason, EngineCompactionResult,
};
use crate::agent::progress::AgentEvent;
use anyhow::{Result, anyhow};
use tokio::time::Duration;
use tracing::warn;

impl AgentExecutor {
    /// Manually compact the current Agent Mode hot context without running a task iteration.
    ///
    /// Uses the unified engine path (`compact_via_engine`) — creates a block
    /// in `CompactionState` without destroying raw memory.
    pub async fn compact_current_context(
        &mut self,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<()> {
        let task = self
            .last_task()
            .map(str::to_string)
            .unwrap_or_else(|| "Continue the current Agent Mode session".to_string());

        let token_before = self.session.memory.rendered_token_count();
        let history_items_before = self.session.memory.rendered_item_count();

        warn!(
            hot_memory_tokens = self.session.memory.token_count(),
            rendered_tokens = token_before,
            rendered_items = history_items_before,
            task_len = task.len(),
            "Manual engine compaction requested"
        );

        Self::emit_runtime_compaction_started(
            progress_tx.as_ref(),
            CompactionReason::Manual,
            CompactionPhase::Manual,
            token_before,
            history_items_before,
        )
        .await;

        let route = self.settings.get_configured_agent_model();
        let tools = self.current_tool_definitions();
        let cancellation_token = self.session.cancellation_token.clone();

        let result = Self::await_until_cancelled(cancellation_token, async {
            self.compaction_controller
                .compact_via_engine(
                    &mut self.session.memory,
                    &route,
                    &task,
                    &tools,
                    "", // system_prompt — not available outside a run; empty is conservative
                    CompactionReason::Manual,
                    CompactionPhase::Manual,
                    true, // force
                )
                .await
                .map_err(anyhow::Error::from)
        })
        .await;

        match result {
            Some(Ok(EngineCompactionResult::Applied(outcome))) => {
                self.session.persist_memory_checkpoint_background();
                warn!(
                    block_ref = %outcome.block_ref,
                    rendered_tokens_before = outcome.token_before,
                    rendered_tokens_after = outcome.token_after,
                    rendered_items_before = outcome.history_items_before,
                    rendered_items_after = outcome.history_items_after,
                    provider = %outcome.provider,
                    route = %outcome.route,
                    "Manual engine compaction completed"
                );
                Self::emit_runtime_compaction_completed(progress_tx.as_ref(), &outcome).await;
                Ok(())
            }
            Some(Ok(EngineCompactionResult::Skipped(skipped))) => {
                Self::emit_runtime_compaction_skipped(
                    progress_tx.as_ref(),
                    skipped.reason,
                    skipped.phase,
                    skipped.skipped_reason,
                )
                .await;
                Ok(())
            }
            Some(Err(error)) => {
                warn!(error = %error, "Manual engine compaction failed");
                Self::emit_runtime_compaction_failed(
                    progress_tx.as_ref(),
                    CompactionReason::Manual,
                    CompactionPhase::Manual,
                    error.to_string(),
                )
                .await;
                Err(error)
            }
            None => {
                if let Some(tx) = progress_tx.as_ref() {
                    let _ = tx.send(AgentEvent::Cancelled).await;
                }
                Err(anyhow!("Task cancelled by user"))
            }
        }
    }

    /// Check if the task has been cancelled
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.session.cancellation_token.is_cancelled()
    }

    pub(super) fn agent_timeout_secs(&self, options: AgentExecutionOptions) -> u64 {
        options.min_timeout_secs().map_or_else(
            || self.settings.get_agent_timeout_secs(),
            |minimum| self.settings.get_agent_timeout_secs().max(minimum),
        )
    }

    pub(super) fn agent_timeout_duration(&self, options: AgentExecutionOptions) -> Duration {
        Duration::from_secs(self.agent_timeout_secs(options))
    }

    pub(super) fn agent_timeout_error_message(&self, options: AgentExecutionOptions) -> String {
        let limit_mins = self.agent_timeout_secs(options) / 60;
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

    // Unified event emitters — shared between runner and executor paths.
    // These replace the old duplicated emit_runtime_manual_compaction_* methods.

    async fn emit_runtime_compaction_started(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        reason: CompactionReason,
        phase: CompactionPhase,
        token_before: usize,
        history_items_before: usize,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::RuntimeCompactionStarted {
                    reason,
                    phase,
                    backend: CompactionBackend::LocalLlmSummary,
                    provider: None,
                    route: None,
                    token_before,
                    history_items_before,
                })
                .await;
        }
    }

    async fn emit_runtime_compaction_completed(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        outcome: &crate::agent::compaction::EngineCompactionOutcome,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::RuntimeCompactionCompleted {
                    reason: outcome.reason,
                    phase: outcome.phase,
                    backend: CompactionBackend::LocalLlmSummary,
                    provider: outcome.provider.clone(),
                    route: outcome.route.clone(),
                    token_before: outcome.token_before,
                    token_after: outcome.token_after,
                    history_items_before: outcome.history_items_before,
                    history_items_after: outcome.history_items_after,
                    generation: outcome.block_ref.as_u32(),
                    repair_applied: false,
                })
                .await;
        }
    }

    async fn emit_runtime_compaction_failed(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        reason: CompactionReason,
        phase: CompactionPhase,
        error: String,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::RuntimeCompactionFailed {
                    reason,
                    phase,
                    backend: CompactionBackend::LocalLlmSummary,
                    provider: None,
                    route: None,
                    error,
                })
                .await;
        }
    }

    async fn emit_runtime_compaction_skipped(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        reason: CompactionReason,
        phase: CompactionPhase,
        skipped_reason: String,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::RuntimeCompactionSkipped {
                    reason,
                    phase,
                    skipped_reason,
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
