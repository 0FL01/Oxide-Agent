use super::AgentExecutor;
use crate::agent::compaction::{
    BudgetEstimate, BudgetState, CompactRequestContext, CompactRunOutcome, CompactionBackend,
    CompactionOutcome, CompactionPhase, CompactionReason, CompactionSnapshot, CompactionTrigger,
    HotMemoryBudget, RebuildOutcome, SummaryGenerationOutcome,
};
use crate::agent::progress::AgentEvent;
use anyhow::{anyhow, Result};
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
        self.compact_current_context_codex_style(&task, progress_tx)
            .await
    }

    async fn compact_current_context_codex_style(
        &mut self,
        task: &str,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<CompactionOutcome> {
        warn!(
            hot_memory_tokens = self.session.memory.token_count(),
            hot_memory_items = self.session.memory.get_messages().len(),
            task_len = task.len(),
            "Manual Codex-style compaction requested"
        );
        Self::emit_runtime_manual_compaction_started(progress_tx.as_ref(), &self.session.memory)
            .await;

        let cancellation_token = self.session.cancellation_token.clone();
        let context = CompactRequestContext {
            task: task.to_string(),
            reason: CompactionReason::Manual,
            phase: CompactionPhase::Manual,
            target_token_budget: self.session.memory.max_tokens(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        let outcome = match Self::await_until_cancelled(cancellation_token, async {
            self.compaction_controller
                .manual_compact(&mut self.session.memory, context)
                .await
                .map_err(anyhow::Error::from)
        })
        .await
        {
            Some(Ok(outcome)) => outcome,
            Some(Err(error)) => {
                warn!(error = %error, "Manual Codex-style compaction failed");
                Self::emit_runtime_manual_compaction_failed(
                    progress_tx.as_ref(),
                    error.to_string(),
                )
                .await;
                return Err(error);
            }
            None => {
                if let Some(tx) = progress_tx.as_ref() {
                    let _ = tx.send(AgentEvent::Cancelled).await;
                }
                return Err(anyhow!("Task cancelled by user"));
            }
        };

        self.session.persist_memory_checkpoint_background();
        let legacy_outcome =
            Self::codex_style_manual_outcome_as_legacy(&outcome, self.session.memory.max_tokens());
        warn!(
            hot_memory_tokens_before = legacy_outcome.token_count_before,
            hot_memory_tokens_after = legacy_outcome.token_count_after,
            history_items_before = outcome.replacement.history_items_before,
            history_items_after = outcome.replacement.history_items_after,
            provider = %outcome.metadata.provider,
            route = %outcome.metadata.route,
            generation = outcome.metadata.generation,
            "Manual Codex-style compaction completed"
        );
        Self::emit_runtime_manual_compaction_completed(progress_tx.as_ref(), &outcome).await;

        Ok(legacy_outcome)
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

    async fn emit_runtime_manual_compaction_started(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        memory: &crate::agent::memory::AgentMemory,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::RuntimeCompactionStarted {
                    reason: CompactionReason::Manual,
                    phase: CompactionPhase::Manual,
                    backend: CompactionBackend::LocalLlmSummary,
                    provider: None,
                    route: None,
                    token_before: memory.token_count(),
                    history_items_before: memory.get_messages().len(),
                })
                .await;
        }
    }

    async fn emit_runtime_manual_compaction_completed(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        outcome: &CompactRunOutcome,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::RuntimeCompactionCompleted {
                    reason: outcome.metadata.reason,
                    phase: outcome.metadata.phase,
                    backend: outcome.metadata.backend,
                    provider: outcome.metadata.provider.clone(),
                    route: outcome.metadata.route.clone(),
                    token_before: outcome.replacement.token_before,
                    token_after: outcome.replacement.token_after,
                    history_items_before: outcome.replacement.history_items_before,
                    history_items_after: outcome.replacement.history_items_after,
                    generation: outcome.metadata.generation,
                    repair_applied: outcome.metadata.repair_applied,
                })
                .await;
        }
    }

    async fn emit_runtime_manual_compaction_failed(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        error: String,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::RuntimeCompactionFailed {
                    reason: CompactionReason::Manual,
                    phase: CompactionPhase::Manual,
                    backend: CompactionBackend::LocalLlmSummary,
                    provider: None,
                    route: None,
                    error,
                })
                .await;
        }
    }

    fn codex_style_manual_outcome_as_legacy(
        outcome: &CompactRunOutcome,
        context_window_tokens: usize,
    ) -> CompactionOutcome {
        let token_before = outcome.replacement.token_before;
        let token_after = outcome.replacement.token_after;
        let context_window_tokens = context_window_tokens
            .max(token_before)
            .max(token_after)
            .max(1);
        CompactionOutcome {
            trigger: CompactionTrigger::Manual,
            applied: true,
            token_count_before: token_before,
            token_count_after: token_after,
            budget: BudgetEstimate {
                context_window_tokens,
                system_prompt_tokens: 0,
                tool_schema_tokens: 0,
                hot_memory: HotMemoryBudget {
                    total_tokens: token_before,
                    total_messages: outcome.replacement.history_items_before,
                    pinned_tokens: 0,
                    protected_live_tokens: 0,
                    prunable_artifact_tokens: 0,
                    compactable_history_tokens: token_before,
                    skill_context_tokens: 0,
                    runtime_context_tokens: 0,
                },
                loaded_skill_tokens: 0,
                reserved_output_tokens: 0,
                hard_reserve_tokens: 0,
                total_input_tokens: token_before,
                projected_total_tokens: token_before,
                headroom_tokens: context_window_tokens.saturating_sub(token_after),
                warning_threshold_tokens: context_window_tokens.saturating_mul(65) / 100,
                prune_threshold_tokens: context_window_tokens.saturating_mul(75) / 100,
                compact_threshold_tokens: context_window_tokens.saturating_mul(85) / 100,
                over_limit_threshold_tokens: context_window_tokens.saturating_mul(95) / 100,
                state: BudgetState::Healthy,
            },
            snapshot: CompactionSnapshot::default(),
            externalization: Default::default(),
            error_retry_collapse: Default::default(),
            dedup_superseded: Default::default(),
            archive_persistence: Default::default(),
            pruning: Default::default(),
            summary_generation: SummaryGenerationOutcome {
                attempted: true,
                used_fallback: false,
                model_name: Some(outcome.metadata.route.clone()),
                summary: None,
            },
            rebuild: RebuildOutcome {
                applied: true,
                inserted_summary: true,
                inserted_breadcrumb: false,
                inserted_archive_reference: false,
                dropped_message_count: outcome
                    .replacement
                    .history_items_before
                    .saturating_sub(outcome.replacement.history_items_after),
                dropped_indices: Vec::new(),
                preserved_recent_indices: Vec::new(),
            },
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
