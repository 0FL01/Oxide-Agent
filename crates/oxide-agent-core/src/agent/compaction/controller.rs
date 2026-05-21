//! Runtime/session-level compaction controller.

use super::{
    build_compacted_history, extract_previous_compacted_summary, BuildCompactedHistoryRequest,
    CompactSummaryBackend, CompactSummaryError, CompactSummaryRequest, CompactedHistoryBuildError,
    CompactedSummaryMetadata, CompactionBackend, CompactionPhase, CompactionReason,
    LocalLlmSummary,
};
use crate::agent::memory::{
    AgentMemory, AgentMessage, CompactedHistoryReplacementError, CompactedHistoryReplacementOutcome,
};
use crate::config::ModelInfo;
use crate::llm::LlmClient;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

/// Runtime context for one compact operation.
#[derive(Debug, Clone)]
pub struct CompactRequestContext {
    /// User-visible task/current objective.
    pub task: String,
    /// Why compaction is requested.
    pub reason: CompactionReason,
    /// Runtime phase where compaction happens.
    pub phase: CompactionPhase,
    /// Target replacement history budget.
    pub target_token_budget: usize,
    /// Caller-provided timestamp for deterministic tests and event metadata.
    pub created_at: String,
}

/// Result of a successful controller compact run.
#[derive(Debug, Clone)]
pub struct CompactRunOutcome {
    /// Plain text summary returned by the backend.
    pub summary_text: String,
    /// Final metadata embedded in the summary message.
    pub metadata: CompactedSummaryMetadata,
    /// Atomic replacement details.
    pub replacement: CompactedHistoryReplacementOutcome,
}

/// Controller-level compaction failure.
#[derive(Debug, Error)]
pub enum CompactionControllerError {
    /// Summary generation failed.
    #[error(transparent)]
    Summary(#[from] CompactSummaryError),
    /// Replacement history could not be built safely.
    #[error(transparent)]
    Build(#[from] CompactedHistoryBuildError),
    /// Memory rejected the replacement before mutation.
    #[error(transparent)]
    Replace(#[from] CompactedHistoryReplacementError),
}

/// Single entrypoint for runtime/session-level compaction.
pub struct CompactionController {
    summary_backend: Arc<dyn CompactSummaryBackend>,
}

impl CompactionController {
    /// Create a controller with one provider-agnostic summary backend.
    #[must_use]
    pub fn new(summary_backend: Arc<dyn CompactSummaryBackend>) -> Self {
        Self { summary_backend }
    }

    /// Create the default provider-agnostic local LLM compaction controller.
    #[must_use]
    pub fn local_llm(
        llm_client: Arc<LlmClient>,
        routes: Vec<ModelInfo>,
        timeout_secs: u64,
    ) -> Self {
        Self::new(Arc::new(LocalLlmSummary::new(
            llm_client,
            routes,
            Duration::from_secs(timeout_secs.max(1)),
        )))
    }

    /// Run explicit manual compaction and atomically replace session memory.
    pub async fn manual_compact(
        &self,
        memory: &mut AgentMemory,
        context: CompactRequestContext,
    ) -> Result<CompactRunOutcome, CompactionControllerError> {
        self.compact(memory, context).await
    }

    /// Run a forced mid-turn context-limit compaction.
    pub async fn compact_for_context_limit(
        &self,
        memory: &mut AgentMemory,
        mut context: CompactRequestContext,
    ) -> Result<CompactRunOutcome, CompactionControllerError> {
        context.reason = CompactionReason::ContextLimit;
        context.phase = CompactionPhase::MidTurn;
        self.compact(memory, context).await
    }

    /// Run compaction before switching to a smaller model context window.
    pub async fn model_downshift_compact(
        &self,
        memory: &mut AgentMemory,
        mut context: CompactRequestContext,
    ) -> Result<CompactRunOutcome, CompactionControllerError> {
        context.reason = CompactionReason::ModelDownshift;
        context.phase = CompactionPhase::ModelSwitch;
        self.compact(memory, context).await
    }

    async fn compact(
        &self,
        memory: &mut AgentMemory,
        context: CompactRequestContext,
    ) -> Result<CompactRunOutcome, CompactionControllerError> {
        let source_messages = memory.get_messages().to_vec();
        let previous_summary = extract_previous_compacted_summary(&source_messages);
        let summary_result = self
            .summary_backend
            .summarize(CompactSummaryRequest {
                task: &context.task,
                messages: &source_messages,
                previous_summary: previous_summary.as_ref(),
            })
            .await?;

        let mut metadata = CompactedSummaryMetadata {
            generation: next_generation(previous_summary.as_ref()),
            reason: context.reason,
            phase: context.phase,
            token_before: memory.token_count(),
            token_after: 0,
            history_items_before: source_messages.len(),
            history_items_after: 0,
            provider: summary_result.provider.clone(),
            route: summary_result.route.clone(),
            backend: CompactionBackend::LocalLlmSummary,
            created_at: context.created_at,
            previous_summary_detected: previous_summary.is_some(),
            repair_applied: false,
        };

        let replacement = build_replacement(
            &source_messages,
            &summary_result.summary_text,
            &metadata,
            context.target_token_budget,
        )?;
        metadata.history_items_after = replacement.len();
        metadata.token_after = replacement_tokens(&replacement);

        let replacement = build_replacement(
            &source_messages,
            &summary_result.summary_text,
            &metadata,
            context.target_token_budget,
        )?;
        let replacement_outcome = memory.replace_compacted_history(replacement)?;

        Ok(CompactRunOutcome {
            summary_text: summary_result.summary_text,
            metadata,
            replacement: replacement_outcome,
        })
    }
}

fn build_replacement(
    source_messages: &[AgentMessage],
    summary_text: &str,
    metadata: &CompactedSummaryMetadata,
    target_token_budget: usize,
) -> Result<Vec<AgentMessage>, CompactedHistoryBuildError> {
    build_compacted_history(BuildCompactedHistoryRequest {
        messages: source_messages,
        summary_text,
        metadata,
        target_token_budget,
    })
}

fn replacement_tokens(messages: &[AgentMessage]) -> usize {
    messages
        .iter()
        .map(|message| {
            let mut tokens = super::count_tokens_cached(&message.content);
            if let Some(reasoning) = &message.reasoning {
                tokens = tokens.saturating_add(super::count_tokens_cached(reasoning));
            }
            tokens
        })
        .sum()
}

fn next_generation(previous_summary: Option<&super::PreviousCompactedSummary>) -> u32 {
    if previous_summary.is_some() {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::{CompactSummaryResult, OXIDE_COMPACTED_SUMMARY_PREFIX};
    use crate::agent::providers::{TodoItem, TodoStatus};
    use async_trait::async_trait;

    struct StaticSummaryBackend;
    struct FailingSummaryBackend;

    #[async_trait]
    impl CompactSummaryBackend for StaticSummaryBackend {
        async fn summarize(
            &self,
            _request: CompactSummaryRequest<'_>,
        ) -> Result<CompactSummaryResult, CompactSummaryError> {
            Ok(CompactSummaryResult {
                summary_text: "Current state and remaining work.".to_string(),
                provider: "mock".to_string(),
                route: "mock-compact".to_string(),
            })
        }

        fn selected_route(&self) -> Option<&crate::config::ModelInfo> {
            None
        }
    }

    #[async_trait]
    impl CompactSummaryBackend for FailingSummaryBackend {
        async fn summarize(
            &self,
            _request: CompactSummaryRequest<'_>,
        ) -> Result<CompactSummaryResult, CompactSummaryError> {
            Err(CompactSummaryError::Provider("mock failure".to_string()))
        }

        fn selected_route(&self) -> Option<&crate::config::ModelInfo> {
            None
        }
    }

    fn context() -> CompactRequestContext {
        CompactRequestContext {
            task: "Ship compaction".to_string(),
            reason: CompactionReason::Manual,
            phase: CompactionPhase::Manual,
            target_token_budget: 10_000,
            created_at: "2026-05-21T20:15:00+03:00".to_string(),
        }
    }

    #[tokio::test]
    async fn manual_compact_replaces_memory_with_one_prefixed_summary() {
        let backend = StaticSummaryBackend;
        let controller = CompactionController::new(Arc::new(backend));
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::user_task("Ship compaction"));
        memory.add_message(AgentMessage::summary("[COMPACTION_SUMMARY]\nold"));
        memory.add_message(AgentMessage::user("Continue"));

        let outcome = controller
            .manual_compact(&mut memory, context())
            .await
            .expect("manual compact succeeds");

        assert_eq!(outcome.metadata.generation, 2);
        assert!(outcome.metadata.previous_summary_detected);
        assert_eq!(
            memory
                .get_messages()
                .iter()
                .filter(|message| message.content.starts_with(OXIDE_COMPACTED_SUMMARY_PREFIX))
                .count(),
            1
        );
        assert!(memory
            .get_messages()
            .iter()
            .all(|message| !message.content.contains("[COMPACTION_SUMMARY]")));
    }

    #[tokio::test]
    async fn repeated_manual_compact_keeps_one_prefixed_summary() {
        let backend = StaticSummaryBackend;
        let controller = CompactionController::new(Arc::new(backend));
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::user_task("Ship compaction"));
        memory.add_message(AgentMessage::user("Continue"));

        controller
            .manual_compact(&mut memory, context())
            .await
            .expect("first compact succeeds");
        memory.add_message(AgentMessage::user("Continue after first compact"));
        controller
            .manual_compact(&mut memory, context())
            .await
            .expect("second compact succeeds");

        assert_eq!(
            memory
                .get_messages()
                .iter()
                .filter(|message| message.content.starts_with(OXIDE_COMPACTED_SUMMARY_PREFIX))
                .count(),
            1
        );
        assert!(memory
            .get_messages()
            .iter()
            .any(|message| message.content == "Continue after first compact"));
    }

    #[tokio::test]
    async fn manual_compact_preserves_todos_state() {
        let backend = StaticSummaryBackend;
        let controller = CompactionController::new(Arc::new(backend));
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::user_task("Ship compaction"));
        memory.todos.items.push(TodoItem {
            description: "Keep current todo".to_string(),
            status: TodoStatus::InProgress,
        });

        controller
            .manual_compact(&mut memory, context())
            .await
            .expect("manual compact succeeds");

        assert_eq!(memory.todos.items.len(), 1);
        assert_eq!(memory.todos.items[0].description, "Keep current todo");
        assert_eq!(memory.todos.items[0].status, TodoStatus::InProgress);
    }

    #[tokio::test]
    async fn manual_compact_failure_does_not_mutate_memory() {
        let controller = CompactionController::new(Arc::new(FailingSummaryBackend));
        let mut memory = AgentMemory::new(100_000);
        memory.add_message(AgentMessage::user_task("Ship compaction"));
        memory.add_message(AgentMessage::summary("[COMPACTION_SUMMARY]\nold"));
        memory.add_message(AgentMessage::user("Continue"));
        let before_messages =
            serde_json::to_value(memory.get_messages()).expect("messages serialize");
        let before_tokens = memory.token_count();

        let err = controller
            .manual_compact(&mut memory, context())
            .await
            .expect_err("summary failure should abort compaction");

        assert!(matches!(err, CompactionControllerError::Summary(_)));
        assert_eq!(
            serde_json::to_value(memory.get_messages()).expect("messages serialize"),
            before_messages
        );
        assert_eq!(memory.token_count(), before_tokens);
    }
}
