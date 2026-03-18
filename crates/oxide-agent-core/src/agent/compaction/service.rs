//! Stage 1 compaction orchestration facade.

use super::archive::{ArchiveSink, NoopArchiveSink};
use super::budget::estimate_request_budget;
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
        }
    }

    /// Replace the archive sink used for future cold-context persistence.
    #[must_use]
    pub fn with_archive_sink(mut self, archive_sink: Arc<dyn ArchiveSink>) -> Self {
        self.archive_sink = archive_sink;
        self
    }

    /// Access the active compaction policy.
    #[must_use]
    pub const fn policy(&self) -> &CompactionPolicy {
        &self.policy
    }

    /// Stage 1 checkpoint hook.
    ///
    /// The service remains a no-op mutation-wise for now, but Stage 3 collects
    /// a full request budget so later stages can make pruning/compaction
    /// decisions from orchestration instead of hidden memory side effects.
    pub async fn prepare_for_run(
        &self,
        request: &CompactionRequest<'_>,
        agent: &mut dyn AgentContext,
    ) -> Result<CompactionOutcome> {
        let budget = estimate_request_budget(&self.policy, request, agent);
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
            "Compaction checkpoint reached"
        );

        let _ = &self.archive_sink;

        Ok(CompactionOutcome::noop(request.trigger, budget))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::BudgetState;
    use crate::agent::memory::AgentMessage;
    use crate::agent::{AgentContext, EphemeralSession};

    #[tokio::test]
    async fn prepare_for_run_is_noop_in_stage_one() {
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
            .unwrap();

        assert_eq!(outcome.token_count_before, outcome.token_count_after);
        assert!(!outcome.applied);
        assert_eq!(session.memory().get_messages().len(), 1);
        assert_eq!(outcome.budget.state, BudgetState::Healthy);
    }

    #[test]
    fn default_policy_uses_legacy_threshold() {
        let service = CompactionService::default();
        assert!(service.policy().legacy_compact_threshold > 0);
    }
}
