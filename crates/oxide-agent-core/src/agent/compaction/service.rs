//! Stage 1 compaction orchestration facade.

use super::archive::{ArchiveSink, NoopArchiveSink};
use super::budget::estimate_request_budget;
use super::classifier::classify_hot_memory;
use super::externalize::{externalize_hot_memory, NoopPayloadSink, PayloadSink};
use super::prune::prune_hot_memory;
use super::summarizer::CompactionSummarizer;
use super::types::{CompactionOutcome, CompactionPolicy, CompactionRequest};
use crate::agent::context::AgentContext;
use anyhow::Result;
use std::sync::Arc;
use tracing::debug;

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

    /// Stage 6 checkpoint hook.
    ///
    /// The service currently runs deterministic externalization and pruning,
    /// then returns an updated budget and hot-memory classification snapshot.
    pub async fn prepare_for_run(
        &self,
        request: &CompactionRequest<'_>,
        agent: &mut dyn AgentContext,
    ) -> Result<CompactionOutcome> {
        let budget_before = estimate_request_budget(&self.policy, request, agent);
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

        let budget = estimate_request_budget(&self.policy, request, agent);
        let snapshot = classify_hot_memory(agent.memory().get_messages());
        let summary_generation = if let Some(summarizer) = &self.summarizer {
            summarizer
                .summarize_if_needed(
                    request,
                    budget.state,
                    &snapshot,
                    agent.memory().get_messages(),
                )
                .await
        } else {
            Default::default()
        };
        let hot_messages = agent.memory().get_messages().len();

        debug!(
            trigger = ?request.trigger,
            task_len = request.task.len(),
            system_prompt_len = request.system_prompt.len(),
            tool_count = request.tools.len(),
            model = request.model_name,
            model_max_output_tokens = request.model_max_output_tokens,
            is_sub_agent = request.is_sub_agent,
            hot_messages,
            memory_threshold = agent.memory().compact_threshold(),
            hot_memory_tokens = budget.hot_memory.total_tokens,
            system_prompt_tokens = budget.system_prompt_tokens,
            tool_schema_tokens = budget.tool_schema_tokens,
            projected_total_tokens = budget.projected_total_tokens,
            reserved_output_tokens = budget.reserved_output_tokens,
            headroom_tokens = budget.headroom_tokens,
            budget_state = ?budget.state,
            externalized_messages = externalization.externalized_count,
            externalized_reclaimed_tokens = externalization.reclaimed_tokens,
            pruned_messages = pruning.pruned_count,
            pruned_reclaimed_tokens = pruning.reclaimed_tokens,
            summary_attempted = summary_generation.attempted,
            summary_used_fallback = summary_generation.used_fallback,
            pinned_messages = snapshot.pinned.message_count,
            protected_live_messages = snapshot.protected_live.message_count,
            prunable_artifact_messages = snapshot.prunable_artifacts.message_count,
            compactable_history_messages = snapshot.compactable_history.message_count,
            "Compaction checkpoint reached"
        );

        if !externalization.applied && !pruning.applied && !summary_generation.attempted {
            return Ok(CompactionOutcome::noop(request.trigger, budget, snapshot));
        }

        Ok(CompactionOutcome {
            trigger: request.trigger,
            applied: externalization.applied || pruning.applied,
            token_count_before: budget_before.hot_memory.total_tokens,
            token_count_after: budget.hot_memory.total_tokens,
            budget,
            snapshot,
            externalization,
            pruning,
            summary_generation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::{BudgetState, CompactionSummarizer, CompactionSummarizerConfig};
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
    async fn prepare_for_run_generates_fallback_summary_when_needed() {
        let mut session = EphemeralSession::new(20_000);
        session.memory_mut().add_message(AgentMessage::user(
            "We must preserve AGENTS.md during compaction.",
        ));
        session.memory_mut().add_message(AgentMessage::assistant(
            "I found `crates/oxide-agent-core/src/agent/compaction/service.rs`.",
        ));

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
            .expect("stage 7 summary checkpoint should succeed");

        assert!(!outcome.applied);
        assert!(outcome.summary_generation.attempted);
        assert!(outcome.summary_generation.used_fallback);
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
