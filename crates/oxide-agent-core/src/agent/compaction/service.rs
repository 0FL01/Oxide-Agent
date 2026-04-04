//! Stage 1 compaction orchestration facade.

use super::archive::{persist_compacted_history_chunk, ArchiveSink, NoopArchiveSink};
use super::budget::estimate_request_budget;
use super::classifier::classify_hot_memory_with_policy;
use super::dedup_superseded::{dedup_superseded_tool_results, DedupSupersededContract};
use super::error_retry_collapse::collapse_error_retries;
use super::externalize::{externalize_hot_memory, NoopPayloadSink, PayloadSink};
use super::prune::prune_hot_memory;
use super::rebuild::rebuild_hot_context;
use super::summarizer::CompactionSummarizer;
use super::types::{
    ArchivePersistenceOutcome, BudgetEstimate, CompactionOutcome, CompactionPolicy,
    CompactionRequest, CompactionSnapshot, DedupSupersededOutcome, ErrorRetryCollapseOutcome,
    ExternalizationOutcome, PruneOutcome, RebuildOutcome, SummaryGenerationOutcome,
};
use crate::agent::context::AgentContext;
use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, warn};

struct CheckpointMetrics<'a> {
    budget: &'a BudgetEstimate,
    snapshot: &'a CompactionSnapshot,
    externalization: &'a ExternalizationOutcome,
    error_retry_collapse: &'a ErrorRetryCollapseOutcome,
    dedup_superseded: &'a DedupSupersededOutcome,
    archive_persistence: &'a ArchivePersistenceOutcome,
    pruning: &'a PruneOutcome,
    summary_generation: &'a SummaryGenerationOutcome,
    rebuild: &'a RebuildOutcome,
}

/// Entry point for Agent Mode context compaction orchestration.
#[derive(Clone)]
pub struct CompactionService {
    policy: CompactionPolicy,
    archive_sink: Arc<dyn ArchiveSink>,
    payload_sink: Arc<dyn PayloadSink>,
    summarizer: Option<CompactionSummarizer>,
}

impl Default for CompactionService {
    fn default() -> Self {
        Self::new(CompactionPolicy::default())
    }
}

impl CompactionService {
    /// Create a new compaction service with the provided policy.
    #[must_use]
    pub fn new(policy: CompactionPolicy) -> Self {
        Self {
            policy,
            archive_sink: Arc::new(NoopArchiveSink),
            payload_sink: Arc::new(NoopPayloadSink),
            summarizer: None,
        }
    }

    /// Replace the archive sink used for future cold-context persistence.
    #[must_use]
    pub fn with_archive_sink(mut self, archive_sink: Arc<dyn ArchiveSink>) -> Self {
        self.archive_sink = archive_sink;
        self
    }

    /// Replace the payload sink used for large tool outputs.
    #[must_use]
    pub fn with_payload_sink(mut self, payload_sink: Arc<dyn PayloadSink>) -> Self {
        self.payload_sink = payload_sink;
        self
    }

    /// Attach a structured-history summarizer for Stage 7 compaction summaries.
    #[must_use]
    pub fn with_summarizer(mut self, summarizer: CompactionSummarizer) -> Self {
        self.summarizer = Some(summarizer);
        self
    }

    /// Access the active compaction policy.
    #[must_use]
    pub const fn policy(&self) -> &CompactionPolicy {
        &self.policy
    }

    /// Stage 8 checkpoint hook.
    ///
    /// The service runs deterministic externalization and pruning, optionally
    /// summarizes older history with the sidecar model, and rebuilds hot memory
    /// into pinned/live/summary/recent-raw slices before the next model call.
    pub async fn prepare_for_run(
        &self,
        request: &CompactionRequest<'_>,
        agent: &mut dyn AgentContext,
    ) -> Result<CompactionOutcome> {
        let budget_before = estimate_request_budget(&self.policy, request, agent);
        let (error_retry_collapse, dedup_superseded, externalization, pruning) =
            if Self::should_apply_deterministic_stages(request.trigger, budget_before.state) {
                self.apply_deterministic_stages(request.trigger, agent)
            } else {
                (
                    Default::default(),
                    Default::default(),
                    Default::default(),
                    Default::default(),
                )
            };
        let (archive_persistence, summary_generation, rebuild) =
            self.summarize_and_rebuild(request, agent).await;

        let budget = estimate_request_budget(&self.policy, request, agent);
        let snapshot = classify_hot_memory_with_policy(
            agent.memory().get_messages(),
            &self.policy,
            Some(agent.memory().max_tokens()),
        );
        self.log_checkpoint(
            request,
            agent,
            &budget_before,
            &CheckpointMetrics {
                budget: &budget,
                snapshot: &snapshot,
                externalization: &externalization,
                error_retry_collapse: &error_retry_collapse,
                dedup_superseded: &dedup_superseded,
                archive_persistence: &archive_persistence,
                pruning: &pruning,
                summary_generation: &summary_generation,
                rebuild: &rebuild,
            },
        );
        self.warn_checkpoint_if_needed(
            request,
            agent,
            &budget_before,
            &CheckpointMetrics {
                budget: &budget,
                snapshot: &snapshot,
                externalization: &externalization,
                error_retry_collapse: &error_retry_collapse,
                dedup_superseded: &dedup_superseded,
                archive_persistence: &archive_persistence,
                pruning: &pruning,
                summary_generation: &summary_generation,
                rebuild: &rebuild,
            },
        );

        if !error_retry_collapse.applied
            && !dedup_superseded.applied
            && !externalization.applied
            && !pruning.applied
            && !summary_generation.attempted
            && !archive_persistence.attempted
            && !rebuild.applied
        {
            return Ok(CompactionOutcome::noop(request.trigger, budget, snapshot));
        }

        Ok(CompactionOutcome {
            trigger: request.trigger,
            applied: error_retry_collapse.applied
                || dedup_superseded.applied
                || externalization.applied
                || pruning.applied
                || rebuild.applied,
            token_count_before: budget_before.hot_memory.total_tokens,
            token_count_after: budget.hot_memory.total_tokens,
            budget,
            snapshot,
            externalization,
            error_retry_collapse,
            dedup_superseded,
            archive_persistence,
            pruning,
            summary_generation,
            rebuild,
        })
    }

    fn apply_deterministic_stages(
        &self,
        trigger: super::CompactionTrigger,
        agent: &mut dyn AgentContext,
    ) -> (
        ErrorRetryCollapseOutcome,
        DedupSupersededOutcome,
        ExternalizationOutcome,
        PruneOutcome,
    ) {
        let snapshot_before = classify_hot_memory_with_policy(
            agent.memory().get_messages(),
            &self.policy,
            Some(agent.memory().max_tokens()),
        );

        let (collapsed_messages, error_retry_collapse) = collapse_error_retries(
            &snapshot_before,
            agent.memory().get_messages(),
            matches!(
                trigger,
                super::CompactionTrigger::Manual | super::CompactionTrigger::PostRun
            ),
        );
        if error_retry_collapse.applied {
            agent.memory_mut().replace_messages(collapsed_messages);
        }

        let snapshot_after_collapse = classify_hot_memory_with_policy(
            agent.memory().get_messages(),
            &self.policy,
            Some(agent.memory().max_tokens()),
        );
        let stage0_contract = DedupSupersededContract::default();
        let (deduped_messages, dedup_outcome) = dedup_superseded_tool_results(
            &stage0_contract,
            &snapshot_after_collapse,
            agent.memory().get_messages(),
        );
        if dedup_outcome.applied {
            agent.memory_mut().replace_messages(deduped_messages);
        }

        let snapshot_after_dedup = classify_hot_memory_with_policy(
            agent.memory().get_messages(),
            &self.policy,
            Some(agent.memory().max_tokens()),
        );
        let (rewritten_messages, externalization) = externalize_hot_memory(
            &self.policy,
            &agent.compaction_scope(),
            &snapshot_after_dedup,
            agent.memory().get_messages(),
            self.payload_sink.as_ref(),
            self.archive_sink.as_ref(),
        );
        if externalization.applied {
            agent.memory_mut().replace_messages(rewritten_messages);
        }

        let snapshot_after_externalization = classify_hot_memory_with_policy(
            agent.memory().get_messages(),
            &self.policy,
            Some(agent.memory().max_tokens()),
        );
        let (pruned_messages, pruning) = prune_hot_memory(
            &self.policy,
            &snapshot_after_externalization,
            agent.memory().get_messages(),
            matches!(trigger, super::CompactionTrigger::PostRun),
        );
        if pruning.applied {
            agent.memory_mut().replace_messages(pruned_messages);
        }

        (
            error_retry_collapse,
            dedup_outcome,
            externalization,
            pruning,
        )
    }

    const fn should_apply_deterministic_stages(
        trigger: super::CompactionTrigger,
        budget_state: super::BudgetState,
    ) -> bool {
        matches!(
            trigger,
            super::CompactionTrigger::Manual | super::CompactionTrigger::PostRun
        ) || matches!(
            budget_state,
            super::BudgetState::ShouldPrune
                | super::BudgetState::ShouldCompact
                | super::BudgetState::OverLimit
        )
    }

    async fn summarize_and_rebuild(
        &self,
        request: &CompactionRequest<'_>,
        agent: &mut dyn AgentContext,
    ) -> (
        ArchivePersistenceOutcome,
        SummaryGenerationOutcome,
        RebuildOutcome,
    ) {
        let budget_before_summary = estimate_request_budget(&self.policy, request, agent);
        let snapshot_before_summary = classify_hot_memory_with_policy(
            agent.memory().get_messages(),
            &self.policy,
            Some(agent.memory().max_tokens()),
        );
        let summary_generation = if let Some(summarizer) = &self.summarizer {
            summarizer
                .summarize_if_needed(
                    request,
                    budget_before_summary.state,
                    &snapshot_before_summary,
                    agent.memory().get_messages(),
                )
                .await
        } else {
            Default::default()
        };
        let archive_persistence = summary_generation
            .summary
            .as_ref()
            .map(|summary| {
                persist_compacted_history_chunk(
                    &agent.compaction_scope(),
                    request.trigger,
                    &snapshot_before_summary,
                    agent.memory().get_messages(),
                    summary,
                    self.archive_sink.as_ref(),
                )
            })
            .unwrap_or_default();
        let (rebuilt_messages, rebuild) = if let Some(summary) = summary_generation.summary.clone()
        {
            rebuild_hot_context(
                &snapshot_before_summary,
                agent.memory().get_messages(),
                Some(summary),
                archive_persistence.archive_refs.first().cloned(),
            )
        } else {
            (
                agent.memory().get_messages().to_vec(),
                RebuildOutcome::default(),
            )
        };
        if rebuild.applied {
            agent.memory_mut().replace_messages(rebuilt_messages);
        }

        (archive_persistence, summary_generation, rebuild)
    }

    fn log_checkpoint(
        &self,
        request: &CompactionRequest<'_>,
        agent: &dyn AgentContext,
        budget_before: &BudgetEstimate,
        metrics: &CheckpointMetrics<'_>,
    ) {
        debug!(
            trigger = ?request.trigger,
            task_len = request.task.len(),
            system_prompt_len = request.system_prompt.len(),
            tool_count = request.tools.len(),
            model = request.model_name,
            model_max_output_tokens = request.model_max_output_tokens,
            is_sub_agent = request.is_sub_agent,
            hot_messages = agent.memory().get_messages().len(),
            hot_messages_before = budget_before.hot_memory.total_messages,
            hot_memory_tokens = metrics.budget.hot_memory.total_tokens,
            hot_memory_tokens_before = budget_before.hot_memory.total_tokens,
            system_prompt_tokens = metrics.budget.system_prompt_tokens,
            tool_schema_tokens = metrics.budget.tool_schema_tokens,
            projected_total_tokens = metrics.budget.projected_total_tokens,
            projected_total_tokens_before = budget_before.projected_total_tokens,
            reserved_output_tokens = metrics.budget.reserved_output_tokens,
            headroom_tokens = metrics.budget.headroom_tokens,
            headroom_tokens_before = budget_before.headroom_tokens,
            warning_threshold_tokens = metrics.budget.warning_threshold_tokens,
            prune_threshold_tokens = metrics.budget.prune_threshold_tokens,
            compact_threshold_tokens = metrics.budget.compact_threshold_tokens,
            over_limit_threshold_tokens = metrics.budget.over_limit_threshold_tokens,
            budget_state = ?metrics.budget.state,
            budget_state_before = ?budget_before.state,
            externalized_messages = metrics.externalization.externalized_count,
            externalized_reclaimed_tokens = metrics.externalization.reclaimed_tokens,
            collapsed_retry_attempts = metrics.error_retry_collapse.collapsed_attempt_count,
            collapsed_retry_messages = metrics.error_retry_collapse.dropped_message_count,
            collapsed_retry_reclaimed_tokens = metrics.error_retry_collapse.reclaimed_tokens,
            deduplicated_superseded_messages = metrics.dedup_superseded.deduplicated_count,
            deduplicated_superseded_reclaimed_tokens = metrics.dedup_superseded.reclaimed_tokens,
            archived_chunks = metrics.archive_persistence.archived_chunk_count,
            archived_messages = metrics.archive_persistence.archived_message_count,
            pruned_messages = metrics.pruning.pruned_count,
            pruned_reclaimed_tokens = metrics.pruning.reclaimed_tokens,
            total_reclaimed_tokens = budget_before
                .hot_memory
                .total_tokens
                .saturating_sub(metrics.budget.hot_memory.total_tokens),
            summary_attempted = metrics.summary_generation.attempted,
            summary_used_fallback = metrics.summary_generation.used_fallback,
            rebuild_applied = metrics.rebuild.applied,
            rebuild_inserted_summary = metrics.rebuild.inserted_summary,
            rebuild_inserted_archive_reference = metrics.rebuild.inserted_archive_reference,
            rebuild_dropped_messages = metrics.rebuild.dropped_message_count,
            pinned_messages = metrics.snapshot.pinned.message_count,
            protected_live_messages = metrics.snapshot.protected_live.message_count,
            prunable_artifact_messages = metrics.snapshot.prunable_artifacts.message_count,
            compactable_history_messages = metrics.snapshot.compactable_history.message_count,
            "Compaction checkpoint reached"
        );
    }

    fn warn_checkpoint_if_needed(
        &self,
        request: &CompactionRequest<'_>,
        agent: &dyn AgentContext,
        budget_before: &BudgetEstimate,
        metrics: &CheckpointMetrics<'_>,
    ) {
        if !should_warn_checkpoint(request, budget_before, metrics) {
            return;
        }

        let scope = agent.compaction_scope();
        warn!(
            trigger = ?request.trigger,
            context_key = %scope.context_key,
            flow_id = %scope.flow_id,
            model = request.model_name,
            is_sub_agent = request.is_sub_agent,
            tool_count = request.tools.len(),
            task_len = request.task.len(),
            system_prompt_len = request.system_prompt.len(),
            budget_state_before = ?budget_before.state,
            budget_state_after = ?metrics.budget.state,
            hot_messages_before = budget_before.hot_memory.total_messages,
            hot_messages_after = metrics.budget.hot_memory.total_messages,
            hot_memory_tokens_before = budget_before.hot_memory.total_tokens,
            hot_memory_tokens_after = metrics.budget.hot_memory.total_tokens,
            projected_total_tokens_before = budget_before.projected_total_tokens,
            projected_total_tokens_after = metrics.budget.projected_total_tokens,
            headroom_tokens_before = budget_before.headroom_tokens,
            headroom_tokens_after = metrics.budget.headroom_tokens,
            collapsed_retry_attempts = metrics.error_retry_collapse.collapsed_attempt_count,
            collapsed_retry_messages = metrics.error_retry_collapse.dropped_message_count,
            deduplicated_superseded_count = metrics.dedup_superseded.deduplicated_count,
            externalized_count = metrics.externalization.externalized_count,
            pruned_count = metrics.pruning.pruned_count,
            reclaimed_tokens = budget_before
                .hot_memory
                .total_tokens
                .saturating_sub(metrics.budget.hot_memory.total_tokens),
            cleanup_reclaimed_tokens = metrics
                .dedup_superseded
                .reclaimed_tokens
                .saturating_add(metrics.error_retry_collapse.reclaimed_tokens)
                .saturating_add(metrics.externalization.reclaimed_tokens)
                .saturating_add(metrics.pruning.reclaimed_tokens),
            archived_chunk_count = metrics.archive_persistence.archived_chunk_count,
            archived_message_count = metrics.archive_persistence.archived_message_count,
            summary_attempted = metrics.summary_generation.attempted,
            summary_used_fallback = metrics.summary_generation.used_fallback,
            summary_model = metrics.summary_generation.model_name.as_deref().unwrap_or(""),
            rebuild_applied = metrics.rebuild.applied,
            rebuild_inserted_summary = metrics.rebuild.inserted_summary,
            rebuild_inserted_archive_reference = metrics.rebuild.inserted_archive_reference,
            rebuild_dropped_messages = metrics.rebuild.dropped_message_count,
            pinned_messages = metrics.snapshot.pinned.message_count,
            protected_live_messages = metrics.snapshot.protected_live.message_count,
            prunable_artifact_messages = metrics.snapshot.prunable_artifacts.message_count,
            compactable_history_messages = metrics.snapshot.compactable_history.message_count,
            "Compaction checkpoint executed"
        );
    }
}

fn should_warn_checkpoint(
    request: &CompactionRequest<'_>,
    budget_before: &BudgetEstimate,
    metrics: &CheckpointMetrics<'_>,
) -> bool {
    matches!(
        request.trigger,
        super::CompactionTrigger::Manual | super::CompactionTrigger::PostRun
    ) || budget_before.state.requires_warn_telemetry()
        || metrics.budget.state.requires_warn_telemetry()
        || metrics.error_retry_collapse.applied
        || metrics.dedup_superseded.applied
        || metrics.externalization.applied
        || metrics.pruning.applied
        || metrics.summary_generation.attempted
        || metrics.summary_generation.used_fallback
        || metrics.archive_persistence.attempted
        || metrics.rebuild.applied
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::{
        AgentMessageKind, BudgetState, CompactionSummarizer, CompactionSummarizerConfig,
    };
    use crate::agent::memory::AgentMessage;
    use crate::agent::providers::{TodoItem, TodoStatus};
    use crate::agent::EphemeralSession;
    use crate::llm::{LlmClient, ToolCall, ToolCallFunction};
    use std::sync::Arc;

    #[tokio::test]
    async fn prepare_for_run_is_noop_before_prune_and_compaction() {
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user("Investigate compaction migration"));

        let service = CompactionService::default();
        let request = CompactionRequest::new(
            crate::agent::compaction::CompactionTrigger::PreRun,
            "Investigate compaction migration",
            "system prompt",
            &[],
            "demo-model",
            512,
            false,
        );

        let outcome = service
            .prepare_for_run(&request, &mut session)
            .await
            .expect("stage 4 classifier checkpoint should succeed");

        assert_eq!(outcome.token_count_before, outcome.token_count_after);
        assert!(!outcome.applied);
        assert_eq!(session.memory().get_messages().len(), 1);
        assert_eq!(outcome.budget.state, BudgetState::Healthy);
        assert_eq!(outcome.snapshot.protected_live.message_count, 0);
        assert_eq!(outcome.snapshot.compactable_history.message_count, 1);
        assert_eq!(outcome.externalization.externalized_count, 0);
        assert_eq!(outcome.archive_persistence.archived_chunk_count, 0);
        assert_eq!(outcome.pruning.pruned_count, 0);
        assert!(!outcome.rebuild.applied);
    }

    #[tokio::test]
    async fn prepare_for_run_skips_externalization_for_healthy_budget() {
        let mut session = EphemeralSession::new(20_000);
        // Tool result requires a preceding assistant message with tool call to avoid being dropped by history repair
        session
            .memory_mut()
            .add_message(AgentMessage::assistant_with_tools(
                "Reading file",
                vec![ToolCall::new(
                    "call-1".to_string(),
                    ToolCallFunction {
                        name: "read_file".to_string(),
                        arguments: r#"{"path":"file.txt"}"#.to_string(),
                    },
                    false,
                )],
            ));
        session.memory_mut().add_message(AgentMessage::tool(
            "call-1",
            "read_file",
            &"A".repeat(5_000),
        ));

        let service = CompactionService::default();
        let request = CompactionRequest::new(
            crate::agent::compaction::CompactionTrigger::PreRun,
            "Inspect the file",
            "system prompt",
            &[],
            "demo-model",
            512,
            false,
        );

        let outcome = service
            .prepare_for_run(&request, &mut session)
            .await
            .expect("stage 5 externalization checkpoint should succeed");

        assert!(!outcome.applied);
        assert_eq!(outcome.externalization.externalized_count, 0);
        assert_eq!(outcome.token_count_after, outcome.token_count_before);
        // Now messages[0] is assistant, messages[1] is tool result
        assert!(!session.memory().get_messages()[1].is_externalized());
        assert_eq!(
            session.memory().get_messages()[1].content,
            "A".repeat(5_000)
        );
        assert_eq!(outcome.pruning.pruned_count, 0);
    }

    #[tokio::test]
    async fn prepare_for_run_does_not_prune_without_summary_boundary_under_pressure() {
        let mut session = EphemeralSession::new(2_048);
        // Tool results require preceding assistant messages with tool calls to avoid being dropped by history repair
        for index in 0..5 {
            session
                .memory_mut()
                .add_message(AgentMessage::assistant_with_tools(
                    "Searching",
                    vec![ToolCall::new(
                        format!("call-{index}"),
                        ToolCallFunction {
                            name: "search".to_string(),
                            arguments: serde_json::json!({"query": format!("query-{index}")})
                                .to_string(),
                        },
                        false,
                    )],
                ));
            session.memory_mut().add_message(AgentMessage::tool(
                &format!("call-{index}"),
                "search",
                &format!("result-{index}-{}", "B".repeat(80)),
            ));
        }

        let service = CompactionService::new(CompactionPolicy {
            externalize_threshold_tokens: usize::MAX,
            externalize_threshold_chars: usize::MAX,
            prune_min_tokens: 1,
            prune_min_chars: 16,
            protected_tool_window_tokens: 1,
            ..CompactionPolicy::default()
        });
        let request = CompactionRequest::new(
            crate::agent::compaction::CompactionTrigger::PreRun,
            "Review search results",
            "system prompt",
            &[],
            "demo-model",
            1_024,
            false,
        );

        let outcome = service
            .prepare_for_run(&request, &mut session)
            .await
            .expect("stage 6 pruning checkpoint should succeed");

        assert!(!outcome.applied);
        assert!(matches!(
            outcome.budget.state,
            BudgetState::ShouldPrune | BudgetState::ShouldCompact | BudgetState::OverLimit
        ));
        assert_eq!(outcome.externalization.externalized_count, 0);
        assert!(outcome.pruning.pruned_indices.is_empty());
        assert!(!session.memory().get_messages()[0].is_pruned());
        assert!(!session.memory().get_messages()[1].is_pruned());
    }

    #[tokio::test]
    async fn prepare_for_run_prunes_only_before_summary_boundary_under_pressure() {
        let mut session = EphemeralSession::new(512);
        // Tool results require preceding assistant messages with tool calls to avoid being dropped by history repair
        session
            .memory_mut()
            .add_message(AgentMessage::assistant_with_tools(
                "Searching before summary",
                vec![ToolCall::new(
                    "call-0".to_string(),
                    ToolCallFunction {
                        name: "search".to_string(),
                        arguments: serde_json::json!({"query": "query-0"}).to_string(),
                    },
                    false,
                )],
            ));
        session.memory_mut().add_message(AgentMessage::tool(
            "call-0",
            "search",
            &format!("before-summary-{}", "B".repeat(80)),
        ));
        session.memory_mut().add_message(AgentMessage::summary(
            "[Previous context compressed]\n- preserved",
        ));
        for index in 1..6 {
            session
                .memory_mut()
                .add_message(AgentMessage::assistant_with_tools(
                    "Searching after summary",
                    vec![ToolCall::new(
                        format!("call-{index}"),
                        ToolCallFunction {
                            name: "search".to_string(),
                            arguments: serde_json::json!({"query": format!("query-{index}")})
                                .to_string(),
                        },
                        false,
                    )],
                ));
            session.memory_mut().add_message(AgentMessage::tool(
                &format!("call-{index}"),
                "search",
                &format!("after-summary-{index}-{}", "C".repeat(80)),
            ));
        }

        let service = CompactionService::new(CompactionPolicy {
            externalize_threshold_tokens: usize::MAX,
            externalize_threshold_chars: usize::MAX,
            prune_min_tokens: 1,
            prune_min_chars: 16,
            ..CompactionPolicy::default()
        });
        let request = CompactionRequest::new(
            crate::agent::compaction::CompactionTrigger::PreIteration,
            "Review summarized search results",
            "system prompt",
            &[],
            "demo-model",
            1_024,
            false,
        );

        let outcome = service
            .prepare_for_run(&request, &mut session)
            .await
            .expect("summary-boundary pruning checkpoint should succeed");

        assert!(outcome.applied);
        // Now the structure is: [assistant, tool_result, summary, ...]
        // The tool result at index 1 should be pruned (before summary boundary)
        assert_eq!(outcome.pruning.pruned_indices, vec![1]);
        assert!(session.memory().get_messages()[1].is_pruned());
        assert_eq!(
            session.memory().get_messages()[2].resolved_kind(),
            AgentMessageKind::Summary
        );
        // Tool result after summary should not be pruned
        assert!(!session.memory().get_messages()[4].is_pruned());
    }

    #[tokio::test]
    async fn prepare_for_run_rebuilds_hot_memory_with_fallback_summary() {
        let mut session = EphemeralSession::new(20_000);
        session.memory_mut().add_message(AgentMessage::user(
            "We must preserve AGENTS.md during compaction.",
        ));
        session.memory_mut().add_message(AgentMessage::assistant(
            "I found `crates/oxide-agent-core/src/agent/compaction/service.rs`.",
        ));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Keep the latest turns raw."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Recent response 1."));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Recent response 2 input."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Recent response 2 output."));

        let service = fallback_summarizer_service();
        let request = CompactionRequest::new(
            crate::agent::compaction::CompactionTrigger::Manual,
            "Ship stage 7",
            "system prompt",
            &[],
            "demo-model",
            512,
            false,
        );

        let outcome = service
            .prepare_for_run(&request, &mut session)
            .await
            .expect("stage 8 rebuild checkpoint should succeed");

        assert!(outcome.applied);
        assert!(outcome.summary_generation.attempted);
        assert!(outcome.summary_generation.used_fallback);
        assert_eq!(outcome.archive_persistence.archived_chunk_count, 1);
        assert!(outcome.rebuild.applied);
        assert!(outcome.rebuild.inserted_summary);
        assert!(outcome.rebuild.inserted_archive_reference);
        assert_eq!(outcome.rebuild.dropped_indices, vec![0, 1]);
        assert_eq!(session.memory().get_messages().len(), 6);
        assert_eq!(
            session.memory().get_messages()[0].resolved_kind(),
            AgentMessageKind::Summary
        );
        assert_eq!(
            session.memory().get_messages()[1].resolved_kind(),
            AgentMessageKind::ArchiveReference
        );
        assert!(session.memory().get_messages()[0]
            .summary_payload()
            .is_some());
        assert_eq!(
            session.memory().get_messages()[2].content,
            "Keep the latest turns raw."
        );
        assert!(outcome
            .summary_generation
            .summary
            .as_ref()
            .expect("fallback summary")
            .relevant_files_entities
            .iter()
            .any(|item| item.contains("crates/oxide-agent-core/src/agent/compaction/service.rs")));
    }

    #[tokio::test]
    async fn prepare_for_run_preserves_pinned_live_context_and_todos() {
        let mut session = preservation_session();
        let expected_todos = session.memory().todos.clone();
        let service = fallback_summarizer_service();
        let request = CompactionRequest::new(
            crate::agent::compaction::CompactionTrigger::Manual,
            "Ship stage 12",
            "system prompt",
            &[],
            "demo-model",
            256,
            false,
        );

        let outcome = service
            .prepare_for_run(&request, &mut session)
            .await
            .expect("stage 12 hardening checkpoint should succeed");

        assert!(outcome.applied);
        assert_eq!(session.memory().todos.items.len(), 2);
        assert_eq!(
            session
                .memory()
                .todos
                .current_task()
                .map(|item| item.description.as_str()),
            Some("Keep current task preserved")
        );
        assert_eq!(
            session.memory().todos.items[1].description,
            "Add hardening coverage"
        );
        assert_eq!(
            session.memory().todos.pending_count(),
            expected_todos.pending_count()
        );

        let messages = session.memory().get_messages();
        assert_eq!(messages[0].resolved_kind(), AgentMessageKind::TopicAgentsMd);
        assert_eq!(messages[1].resolved_kind(), AgentMessageKind::SystemContext);
        assert_eq!(messages[2].resolved_kind(), AgentMessageKind::UserTask);
        assert_eq!(
            messages[3].resolved_kind(),
            AgentMessageKind::RuntimeContext
        );
        assert_eq!(
            messages[4].resolved_kind(),
            AgentMessageKind::ApprovalReplay
        );
        assert_eq!(messages[5].resolved_kind(), AgentMessageKind::Summary);
        assert_eq!(
            messages[6].resolved_kind(),
            AgentMessageKind::ArchiveReference
        );
        assert_eq!(messages[7].content, "Recent request 1.");
        assert_eq!(messages[10].content, "Recent response 2.");
        assert!(messages.iter().all(|message| !message
            .content
            .contains("Older request about compaction hardening.")));
    }

    fn fallback_summarizer_service() -> CompactionService {
        let llm_client = Arc::new(LlmClient::new(&crate::config::AgentSettings::default()));
        CompactionService::default().with_summarizer(CompactionSummarizer::new(
            llm_client,
            CompactionSummarizerConfig {
                model_routes: Vec::new(),
                timeout_secs: 1,
                ..CompactionSummarizerConfig::default()
            },
        ))
    }

    fn preservation_session() -> EphemeralSession {
        let mut session = EphemeralSession::new(256);
        session.memory_mut().todos.update(vec![
            TodoItem {
                description: "Keep current task preserved".to_string(),
                status: TodoStatus::InProgress,
            },
            TodoItem {
                description: "Add hardening coverage".to_string(),
                status: TodoStatus::Pending,
            },
        ]);
        for message in [
            AgentMessage::topic_agents_md("# Topic AGENTS\nPreserve operator instructions."),
            AgentMessage::system_context("Base execution policy"),
            AgentMessage::user_task("Ship stage 12"),
            AgentMessage::runtime_context("User asked to keep the active task."),
            AgentMessage::approval_replay("Replay the approved SSH action exactly once."),
            AgentMessage::user("Older request about compaction hardening."),
            AgentMessage::assistant("Older response with findings."),
            AgentMessage::user("Recent request 1."),
            AgentMessage::assistant("Recent response 1."),
            AgentMessage::user("Recent request 2."),
            AgentMessage::assistant("Recent response 2."),
        ] {
            session.memory_mut().add_message(message);
        }
        session
    }

    #[test]
    fn default_policy_uses_threshold_ladder() {
        let service = CompactionService::default();
        assert!(service.policy().warning_threshold_percent > 0);
        assert!(
            service.policy().warning_threshold_percent < service.policy().prune_threshold_percent
        );
    }
}
