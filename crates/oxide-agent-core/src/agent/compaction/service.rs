//! Stage 1 compaction orchestration facade.

use super::archive::{ArchiveSink, NoopArchiveSink};
use super::budget::estimate_request_budget;
use super::classifier::classify_hot_memory;
use super::externalize::{externalize_hot_memory, NoopPayloadSink, PayloadSink};
use super::prune::prune_hot_memory;
use super::rebuild::rebuild_hot_context;
use super::summarizer::CompactionSummarizer;
use super::types::{
    BudgetEstimate, CompactionOutcome, CompactionPolicy, CompactionRequest, CompactionSnapshot,
    ExternalizationOutcome, PruneOutcome, RebuildOutcome, SummaryGenerationOutcome,
};
use crate::agent::context::AgentContext;
use anyhow::Result;
use std::sync::Arc;
use tracing::debug;

struct CheckpointMetrics<'a> {
    budget: &'a BudgetEstimate,
    snapshot: &'a CompactionSnapshot,
    externalization: &'a ExternalizationOutcome,
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
        let (externalization, pruning) = self.apply_deterministic_stages(agent);
        let (summary_generation, rebuild) = self.summarize_and_rebuild(request, agent).await;

        let budget = estimate_request_budget(&self.policy, request, agent);
        let snapshot = classify_hot_memory(agent.memory().get_messages());
        self.log_checkpoint(
            request,
            agent,
            &CheckpointMetrics {
                budget: &budget,
                snapshot: &snapshot,
                externalization: &externalization,
                pruning: &pruning,
                summary_generation: &summary_generation,
                rebuild: &rebuild,
            },
        );

        if !externalization.applied
            && !pruning.applied
            && !summary_generation.attempted
            && !rebuild.applied
        {
            return Ok(CompactionOutcome::noop(request.trigger, budget, snapshot));
        }

        Ok(CompactionOutcome {
            trigger: request.trigger,
            applied: externalization.applied || pruning.applied || rebuild.applied,
            token_count_before: budget_before.hot_memory.total_tokens,
            token_count_after: budget.hot_memory.total_tokens,
            budget,
            snapshot,
            externalization,
            pruning,
            summary_generation,
            rebuild,
        })
    }

    fn apply_deterministic_stages(
        &self,
        agent: &mut dyn AgentContext,
    ) -> (ExternalizationOutcome, PruneOutcome) {
        let snapshot_before = classify_hot_memory(agent.memory().get_messages());
        let (rewritten_messages, externalization) = externalize_hot_memory(
            &self.policy,
            &agent.compaction_scope(),
            &snapshot_before,
            agent.memory().get_messages(),
            self.payload_sink.as_ref(),
            self.archive_sink.as_ref(),
        );
        if externalization.applied {
            agent.memory_mut().replace_messages(rewritten_messages);
        }

        let snapshot_after_externalization = classify_hot_memory(agent.memory().get_messages());
        let (pruned_messages, pruning) = prune_hot_memory(
            &self.policy,
            &snapshot_after_externalization,
            agent.memory().get_messages(),
        );
        if pruning.applied {
            agent.memory_mut().replace_messages(pruned_messages);
        }

        (externalization, pruning)
    }

    async fn summarize_and_rebuild(
        &self,
        request: &CompactionRequest<'_>,
        agent: &mut dyn AgentContext,
    ) -> (SummaryGenerationOutcome, RebuildOutcome) {
        let budget_before_summary = estimate_request_budget(&self.policy, request, agent);
        let snapshot_before_summary = classify_hot_memory(agent.memory().get_messages());
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
        let (rebuilt_messages, rebuild) = if let Some(summary) = summary_generation.summary.clone()
        {
            rebuild_hot_context(
                &snapshot_before_summary,
                agent.memory().get_messages(),
                Some(summary),
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

        (summary_generation, rebuild)
    }

    fn log_checkpoint(
        &self,
        request: &CompactionRequest<'_>,
        agent: &dyn AgentContext,
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
            memory_threshold = agent.memory().compact_threshold(),
            hot_memory_tokens = metrics.budget.hot_memory.total_tokens,
            system_prompt_tokens = metrics.budget.system_prompt_tokens,
            tool_schema_tokens = metrics.budget.tool_schema_tokens,
            projected_total_tokens = metrics.budget.projected_total_tokens,
            reserved_output_tokens = metrics.budget.reserved_output_tokens,
            headroom_tokens = metrics.budget.headroom_tokens,
            budget_state = ?metrics.budget.state,
            externalized_messages = metrics.externalization.externalized_count,
            externalized_reclaimed_tokens = metrics.externalization.reclaimed_tokens,
            pruned_messages = metrics.pruning.pruned_count,
            pruned_reclaimed_tokens = metrics.pruning.reclaimed_tokens,
            summary_attempted = metrics.summary_generation.attempted,
            summary_used_fallback = metrics.summary_generation.used_fallback,
            rebuild_applied = metrics.rebuild.applied,
            rebuild_inserted_summary = metrics.rebuild.inserted_summary,
            rebuild_dropped_messages = metrics.rebuild.dropped_message_count,
            pinned_messages = metrics.snapshot.pinned.message_count,
            protected_live_messages = metrics.snapshot.protected_live.message_count,
            prunable_artifact_messages = metrics.snapshot.prunable_artifacts.message_count,
            compactable_history_messages = metrics.snapshot.compactable_history.message_count,
            "Compaction checkpoint reached"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::{
        AgentMessageKind, BudgetState, CompactionSummarizer, CompactionSummarizerConfig,
    };
    use crate::agent::memory::AgentMessage;
    use crate::agent::{AgentContext, EphemeralSession};
    use crate::llm::LlmClient;
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
        assert_eq!(outcome.pruning.pruned_count, 0);
        assert!(!outcome.rebuild.applied);
    }

    #[tokio::test]
    async fn prepare_for_run_externalizes_large_tool_payloads() {
        let mut session = EphemeralSession::new(20_000);
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

        assert!(outcome.applied);
        assert_eq!(outcome.externalization.externalized_count, 1);
        assert!(outcome.token_count_after < outcome.token_count_before);
        assert!(session.memory().get_messages()[0].is_externalized());
        assert!(session.memory().get_messages()[0]
            .content
            .contains("[externalized tool result]"));
        assert_eq!(outcome.pruning.pruned_count, 0);
    }

    #[tokio::test]
    async fn prepare_for_run_prunes_old_tool_payloads_outside_recent_window() {
        let mut session = EphemeralSession::new(20_000);
        for index in 0..5 {
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
            ..CompactionPolicy::default()
        });
        let request = CompactionRequest::new(
            crate::agent::compaction::CompactionTrigger::PreRun,
            "Review search results",
            "system prompt",
            &[],
            "demo-model",
            512,
            false,
        );

        let outcome = service
            .prepare_for_run(&request, &mut session)
            .await
            .expect("stage 6 pruning checkpoint should succeed");

        assert!(outcome.applied);
        assert_eq!(outcome.externalization.externalized_count, 0);
        assert_eq!(outcome.pruning.pruned_indices, vec![0]);
        assert!(session.memory().get_messages()[0].is_pruned());
        assert!(!session.memory().get_messages()[1].is_pruned());
        assert!(session.memory().get_messages()[0]
            .content
            .contains("[pruned tool result]"));
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

        let llm_client = Arc::new(LlmClient::new(&crate::config::AgentSettings::default()));
        let service = CompactionService::default().with_summarizer(CompactionSummarizer::new(
            llm_client,
            CompactionSummarizerConfig {
                model_name: String::new(),
                provider_name: String::new(),
                timeout_secs: 1,
            },
        ));
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
        assert!(outcome.rebuild.applied);
        assert!(outcome.rebuild.inserted_summary);
        assert_eq!(outcome.rebuild.dropped_indices, vec![0, 1]);
        assert_eq!(session.memory().get_messages().len(), 5);
        assert_eq!(
            session.memory().get_messages()[0].resolved_kind(),
            AgentMessageKind::Summary
        );
        assert!(session.memory().get_messages()[0]
            .summary_payload()
            .is_some());
        assert_eq!(
            session.memory().get_messages()[1].content,
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

    #[test]
    fn default_policy_uses_legacy_threshold() {
        let service = CompactionService::default();
        assert!(service.policy().legacy_compact_threshold > 0);
    }
}
