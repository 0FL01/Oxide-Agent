//! Runtime compaction orchestration for the agent runner.
//!
//! All automatic triggers (pre-sampling, context-limit, model-downshift,
//! hook/manual) go through `CompactionController::compact_via_engine`, which
//! selects a compressible range, generates an LLM summary, and applies through
//! `CompactionEngine` — creating a block in `CompactionState` without destroying
//! raw memory. The renderer overlays the block at LLM call time.

use super::AgentRunner;
use super::types::{AgentRunnerContext, RunState};
use crate::agent::compaction::{
    BudgetState, CompactionPhase, CompactionPolicy, CompactionReason, CompactionRequest,
    CompactionTrigger, EngineCompactionResult, count_tokens_cached, estimate_request_budget,
};
use crate::agent::progress::{AgentEvent, RepeatedCompactionKind};
use crate::config::ModelInfo;
use crate::llm::LlmClient;
use anyhow::{Result, anyhow};
use tracing::warn;

impl AgentRunner {
    pub(super) async fn run_manual_compaction_checkpoint(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) -> Result<()> {
        let route = Self::primary_runtime_route(ctx, &self.llm_client)?;
        self.run_engine_compaction(
            ctx,
            state,
            &route,
            CompactionReason::Manual,
            CompactionPhase::Manual,
            true,
        )
        .await?;
        Ok(())
    }

    pub(super) async fn maybe_run_runtime_pre_sampling_compaction(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        iteration: usize,
        route: &ModelInfo,
    ) -> Result<bool> {
        if !Self::runtime_compaction_threshold_reached(ctx, route) {
            return Ok(false);
        }

        let reason = if iteration == 0 {
            CompactionReason::PreTurn
        } else {
            CompactionReason::MidTurn
        };
        self.run_engine_compaction(
            ctx,
            state,
            route,
            reason,
            CompactionPhase::PreSampling,
            false,
        )
        .await
    }

    pub(super) async fn run_runtime_context_limit_compaction(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        route: &ModelInfo,
    ) -> Result<bool> {
        self.run_engine_compaction(
            ctx,
            state,
            route,
            CompactionReason::ContextLimit,
            CompactionPhase::MidTurn,
            true,
        )
        .await
    }

    pub(super) async fn maybe_run_runtime_model_downshift_compaction(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        previous_route: &ModelInfo,
        next_route: &ModelInfo,
    ) -> Result<bool> {
        if !Self::model_downshift_requires_compaction(ctx, previous_route, next_route) {
            return Ok(false);
        }

        self.run_engine_compaction(
            ctx,
            state,
            next_route,
            CompactionReason::ModelDownshift,
            CompactionPhase::ModelSwitch,
            true,
        )
        .await
    }

    fn runtime_compaction_threshold_reached(
        ctx: &AgentRunnerContext<'_>,
        route: &ModelInfo,
    ) -> bool {
        let request = CompactionRequest::new(
            CompactionTrigger::PreIteration,
            ctx.task,
            ctx.system_prompt,
            ctx.tools,
            &route.id,
            route.max_output_tokens,
            ctx.config.is_sub_agent,
        );
        let budget = estimate_request_budget(&CompactionPolicy::default(), &request, ctx.agent);
        matches!(
            budget.state,
            BudgetState::ShouldCompact | BudgetState::OverLimit
        )
    }

    fn model_downshift_requires_compaction(
        ctx: &AgentRunnerContext<'_>,
        previous_route: &ModelInfo,
        next_route: &ModelInfo,
    ) -> bool {
        let previous_window = previous_route.context_window_tokens;
        let next_window = next_route.context_window_tokens;
        if previous_window == 0 || next_window == 0 || next_window >= previous_window {
            return false;
        }

        let projected_total = Self::projected_total_tokens_for_route(ctx);
        projected_total > next_window as usize
    }

    fn projected_total_tokens_for_route(ctx: &AgentRunnerContext<'_>) -> usize {
        let policy = CompactionPolicy::default();
        count_tokens_cached(ctx.system_prompt)
            .saturating_add(Self::tool_schema_tokens(ctx.tools))
            .saturating_add(ctx.agent.memory().token_count())
            .saturating_add(policy.hard_reserve_tokens)
    }

    fn primary_runtime_route(
        ctx: &AgentRunnerContext<'_>,
        llm_client: &LlmClient,
    ) -> Result<ModelInfo> {
        if let Some(route) = ctx.config.model_routes.first() {
            return Ok(route.clone());
        }

        llm_client
            .get_model_info(&ctx.config.model_name)
            .or_else(|_| {
                ctx.config
                    .model_provider
                    .clone()
                    .map(|provider| ModelInfo {
                        id: ctx.config.model_name.clone(),
                        provider,
                        max_output_tokens: ctx.config.model_max_output_tokens,
                        context_window_tokens: ctx.agent.memory().max_tokens() as u32,
                        weight: 1,
                    })
                    .ok_or_else(|| {
                        crate::llm::LlmError::unknown(
                            "No active model route available for compaction".to_string(),
                        )
                    })
            })
            .map_err(|error| anyhow!("No active model route available for compaction: {error}"))
    }

    fn tool_schema_tokens(tools: &[crate::llm::ToolDefinition]) -> usize {
        tools.iter().fold(0usize, |acc, tool| {
            let parameter_tokens = serde_json::to_string(&tool.parameters)
                .ok()
                .map_or(0, |params| count_tokens_cached(&params));
            acc.saturating_add(count_tokens_cached(&tool.name))
                .saturating_add(count_tokens_cached(&tool.description))
                .saturating_add(parameter_tokens)
        })
    }

    /// Unified engine-based compaction for all automatic triggers.
    ///
    /// Calls `CompactionController::compact_via_engine`, which selects a
    /// compressible range, generates an LLM summary, and applies through
    /// `CompactionEngine`. Emits started/completed/failed/skipped events.
    async fn run_engine_compaction(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        route: &ModelInfo,
        reason: CompactionReason,
        phase: CompactionPhase,
        force: bool,
    ) -> Result<bool> {
        let Some(controller) = ctx.compaction_controller else {
            return Ok(false);
        };

        let token_before = ctx.agent.memory().rendered_token_count();
        let history_items_before = ctx.agent.memory().rendered_item_count();

        Self::emit_runtime_compaction_started(
            ctx.progress_tx,
            reason,
            phase,
            token_before,
            history_items_before,
        )
        .await;

        let result = controller
            .compact_via_engine(
                ctx.agent.memory_mut(),
                route,
                ctx.task,
                ctx.tools,
                ctx.system_prompt,
                reason,
                phase,
                force,
            )
            .await;

        match result {
            Ok(EngineCompactionResult::Applied(outcome)) => {
                warn!(
                    task_id = %ctx.task_id,
                    iteration = state.iteration,
                    reason = ?outcome.reason,
                    phase = ?outcome.phase,
                    block_ref = %outcome.block_ref,
                    provider = %outcome.provider,
                    route = %outcome.route,
                    rendered_tokens_before = outcome.token_before,
                    rendered_tokens_after = outcome.token_after,
                    rendered_items_before = outcome.history_items_before,
                    rendered_items_after = outcome.history_items_after,
                    "Agent runner completed engine compaction"
                );
                ctx.agent.persist_memory_checkpoint_background();
                Self::refresh_messages_from_memory(ctx);
                state.compaction_count = state.compaction_count.saturating_add(1);
                if state.compaction_count > 1 {
                    Self::emit_repeated_compaction_warning(
                        ctx.progress_tx,
                        RepeatedCompactionKind::Compaction,
                        state.compaction_count,
                    )
                    .await;
                }
                Self::emit_runtime_compaction_completed(ctx.progress_tx, &outcome).await;
                Ok(true)
            }
            Ok(EngineCompactionResult::Skipped(skipped)) => {
                warn!(
                    task_id = %ctx.task_id,
                    iteration = state.iteration,
                    reason = ?skipped.reason,
                    phase = ?skipped.phase,
                    skipped_reason = %skipped.skipped_reason,
                    hot_memory_tokens = ctx.agent.memory().token_count(),
                    rendered_tokens = ctx.agent.memory().rendered_token_count(),
                    history_items = ctx.agent.memory().get_messages().len(),
                    "Engine compaction skipped"
                );
                Self::emit_runtime_compaction_skipped(
                    ctx.progress_tx,
                    skipped.reason,
                    skipped.phase,
                    skipped.skipped_reason,
                )
                .await;
                Ok(false)
            }
            Err(error) => {
                warn!(
                    task_id = %ctx.task_id,
                    iteration = state.iteration,
                    reason = ?reason,
                    phase = ?phase,
                    error = %error,
                    "Engine compaction failed in agent runner"
                );
                Self::emit_runtime_compaction_failed(
                    ctx.progress_tx,
                    reason,
                    phase,
                    error.to_string(),
                )
                .await;
                Err(error.into())
            }
        }
    }

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
                    backend: crate::agent::compaction::CompactionBackend::LocalLlmSummary,
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
                    backend: crate::agent::compaction::CompactionBackend::LocalLlmSummary,
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
                    backend: crate::agent::compaction::CompactionBackend::LocalLlmSummary,
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

    async fn emit_repeated_compaction_warning(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        kind: RepeatedCompactionKind,
        count: usize,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::RepeatedCompactionWarning { kind, count })
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::{
        CompactSummaryBackend, CompactSummaryError, CompactSummaryRequest, CompactSummaryResult,
        CompactionController,
    };
    use crate::agent::context::{AgentContext, EphemeralSession};
    use crate::agent::memory::AgentMessage;
    use crate::agent::progress::AgentEvent;
    use crate::agent::runner::test_support::{build_llm_client, collect_progress_events};
    use crate::agent::runner::{AgentRunnerConfig, AgentRunnerContext};
    use crate::config::ModelInfo;
    use crate::llm::MockLlmProvider;
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    /// A summary backend that returns a fixed summary string.
    struct StaticSummaryBackend;

    #[async_trait]
    impl CompactSummaryBackend for StaticSummaryBackend {
        async fn summarize(
            &self,
            request: CompactSummaryRequest<'_>,
        ) -> Result<CompactSummaryResult, CompactSummaryError> {
            Ok(CompactSummaryResult {
                summary_text: "Condensed history of older context.".to_string(),
                provider: request.route.provider.clone(),
                route: request.route.id.clone(),
            })
        }
    }

    #[tokio::test]
    async fn engine_compaction_creates_block_and_preserves_raw_memory() {
        let compaction_controller = CompactionController::new(Arc::new(StaticSummaryBackend));
        let llm_client = build_llm_client(MockLlmProvider::new());
        let mut runner = AgentRunner::new(llm_client);
        let mut session = EphemeralSession::new(100);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Test compaction"));
        // Old messages large enough to exceed the 100-token target.
        for i in 0..5 {
            session
                .memory_mut()
                .add_message(AgentMessage::user_turn(format!(
                    "old {i}: {}",
                    "x".repeat(200)
                )));
        }
        session
            .memory_mut()
            .add_message(AgentMessage::user("recent 1"));
        session
            .memory_mut()
            .add_message(AgentMessage::user("recent 2"));
        session
            .memory_mut()
            .add_message(AgentMessage::user("recent 3"));

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let mut ctx = AgentRunnerContext {
            task: "Test compaction",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "test-engine-compaction",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: Some(&compaction_controller),
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            storage: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 1, 1, 30, 256)
                .with_model_routes(vec![ModelInfo {
                    id: "deepseek-v4-flash".to_string(),
                    provider: "opencode-go".to_string(),
                    max_output_tokens: 256,
                    context_window_tokens: 100,
                    weight: 1,
                }]),
        };
        let mut state = crate::agent::runner::types::RunState::new();

        runner
            .run_manual_compaction_checkpoint(&mut ctx, &mut state)
            .await
            .expect("compaction succeeds");

        // A block should be created.
        assert!(
            ctx.agent.memory().compaction_state().has_active_blocks(),
            "compaction should have created an active block"
        );
        // Raw memory is preserved.
        assert!(
            ctx.agent
                .memory()
                .get_messages()
                .iter()
                .any(|m| m.content.contains("old 0:")),
            "raw memory should be preserved"
        );
        // Rendered context should contain block summary.
        let rendered = ctx.agent.memory().rendered_messages();
        assert!(
            rendered
                .iter()
                .any(|m| m.content.contains("Compressed conversation section")),
            "rendered context should contain block summary"
        );

        drop(ctx);
        drop(progress_tx);
        let events = collect_progress_events(&mut progress_rx).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::RuntimeCompactionCompleted { .. }))
        );
    }

    #[tokio::test]
    async fn engine_compaction_skipped_when_nothing_to_compress() {
        let compaction_controller = CompactionController::new(Arc::new(StaticSummaryBackend));
        let llm_client = build_llm_client(MockLlmProvider::new());
        let mut runner = AgentRunner::new(llm_client);
        let mut session = EphemeralSession::new(100_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Test"));
        session
            .memory_mut()
            .add_message(AgentMessage::user("recent 1"));
        session
            .memory_mut()
            .add_message(AgentMessage::user("recent 2"));
        session
            .memory_mut()
            .add_message(AgentMessage::user("recent 3"));

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let mut ctx = AgentRunnerContext {
            task: "Test",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "test-engine-skip",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: Some(&compaction_controller),
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            storage: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 1, 1, 30, 256),
        };
        let mut state = crate::agent::runner::types::RunState::new();

        runner
            .run_manual_compaction_checkpoint(&mut ctx, &mut state)
            .await
            .expect("compaction call succeeds");

        // No block created — only 3 user turns, all in tail.
        assert!(!ctx.agent.memory().compaction_state().has_active_blocks());

        drop(ctx);
        drop(progress_tx);
        let events = collect_progress_events(&mut progress_rx).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::RuntimeCompactionSkipped { .. })),
            "should emit Skipped event"
        );
    }

    #[test]
    fn threshold_reached_with_large_context() {
        let tools = Vec::new();
        let mut session = EphemeralSession::new(1_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Test"));
        session
            .memory_mut()
            .add_message(AgentMessage::user("large ".repeat(4_000)));

        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let ctx = AgentRunnerContext {
            task: "Test",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "test-threshold",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            storage: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 1, 1, 30, 256),
        };
        let route = ModelInfo {
            id: "deepseek-v4-flash".to_string(),
            provider: "opencode-go".to_string(),
            max_output_tokens: 256,
            context_window_tokens: 1_000,
            weight: 1,
        };

        assert!(AgentRunner::runtime_compaction_threshold_reached(
            &ctx, &route
        ));
    }

    #[test]
    fn threshold_not_reached_with_small_context() {
        let tools = Vec::new();
        let mut session = EphemeralSession::new(100_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Test"));

        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let ctx = AgentRunnerContext {
            task: "Test",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "test-threshold-ok",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            storage: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 1, 1, 30, 16_000),
        };
        let route = ModelInfo {
            id: "deepseek-v4-flash".to_string(),
            provider: "opencode-go".to_string(),
            max_output_tokens: 16_000,
            context_window_tokens: 100_000,
            weight: 1,
        };

        assert!(!AgentRunner::runtime_compaction_threshold_reached(
            &ctx, &route
        ));
    }
}
