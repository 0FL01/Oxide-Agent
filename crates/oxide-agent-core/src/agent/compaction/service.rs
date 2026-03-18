//! Stage 1 compaction orchestration facade.

use super::archive::{ArchiveSink, NoopArchiveSink};
use super::budget::estimate_request_budget;
use super::classifier::classify_hot_memory;
use super::externalize::{externalize_hot_memory, NoopPayloadSink, PayloadSink};
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

    /// Access the active compaction policy.
    #[must_use]
    pub const fn policy(&self) -> &CompactionPolicy {
        &self.policy
    }

    /// Stage 4 checkpoint hook.
    ///
    /// The service still does not mutate hot memory yet, but it now collects a
    /// full request budget and a deterministic hot-memory classification
    /// snapshot so later stages can prune and compact from orchestration.
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

        let budget = estimate_request_budget(&self.policy, request, agent);
        let snapshot = classify_hot_memory(agent.memory().get_messages());
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
            reclaimed_tokens = externalization.reclaimed_tokens,
            pinned_messages = snapshot.pinned.message_count,
            protected_live_messages = snapshot.protected_live.message_count,
            prunable_artifact_messages = snapshot.prunable_artifacts.message_count,
            compactable_history_messages = snapshot.compactable_history.message_count,
            "Compaction checkpoint reached"
        );

        if !externalization.applied {
            return Ok(CompactionOutcome::noop(request.trigger, budget, snapshot));
        }

        Ok(CompactionOutcome {
            trigger: request.trigger,
            applied: true,
            token_count_before: budget_before.hot_memory.total_tokens,
            token_count_after: budget.hot_memory.total_tokens,
            budget,
            snapshot,
            externalization,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::BudgetState;
    use crate::agent::memory::AgentMessage;
    use crate::agent::{AgentContext, EphemeralSession};

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
    }

    #[test]
    fn default_policy_uses_legacy_threshold() {
        let service = CompactionService::default();
        assert!(service.policy().legacy_compact_threshold > 0);
    }
}
