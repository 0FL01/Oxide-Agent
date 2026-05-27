//! Runtime compaction orchestration for the agent runner.

use super::types::{AgentRunnerContext, RunState};
use super::AgentRunner;
use crate::agent::compaction::{
    count_tokens_cached, estimate_request_budget, wiki_memory_lookup_available, BudgetState,
    CompactRequestContext, CompactRunOutcome, CompactionBackend, CompactionPhase, CompactionPolicy,
    CompactionReason, CompactionRequest, CompactionTrigger,
};
use crate::agent::progress::{AgentEvent, RepeatedCompactionKind};
use crate::config::ModelInfo;
use crate::llm::LlmClient;
use anyhow::{anyhow, Result};
use tracing::warn;

#[derive(Clone, Copy)]
struct RuntimeCompactionRequest<'a> {
    route: &'a ModelInfo,
    reason: CompactionReason,
    phase: CompactionPhase,
    force: bool,
    target_token_budget: usize,
}

impl AgentRunner {
    pub(super) async fn run_iteration_compaction(
        &mut self,
        _ctx: &mut AgentRunnerContext<'_>,
        _state: &mut RunState,
        _iteration: usize,
    ) -> Result<()> {
        Ok(())
    }

    pub(super) async fn run_manual_compaction_checkpoint(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) -> Result<()> {
        let route = Self::primary_runtime_route(ctx, &self.llm_client)?;
        self.run_runtime_compaction(
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
        self.run_runtime_compaction(
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
        self.run_runtime_compaction(
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

        let target_budget = Self::route_hot_history_budget(ctx, next_route);
        self.run_runtime_compaction_with_target_budget(
            ctx,
            state,
            RuntimeCompactionRequest {
                route: next_route,
                reason: CompactionReason::ModelDownshift,
                phase: CompactionPhase::ModelSwitch,
                force: true,
                target_token_budget: target_budget,
            },
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

        let projected_total = Self::projected_total_tokens_for_route(ctx, next_route);
        projected_total > next_window as usize
    }

    fn projected_total_tokens_for_route(ctx: &AgentRunnerContext<'_>, route: &ModelInfo) -> usize {
        let policy = CompactionPolicy::default();
        count_tokens_cached(ctx.system_prompt)
            .saturating_add(Self::tool_schema_tokens(ctx.tools))
            .saturating_add(ctx.agent.memory().token_count())
            .saturating_add(route.max_output_tokens as usize)
            .saturating_add(policy.hard_reserve_tokens)
    }

    fn route_hot_history_budget(ctx: &AgentRunnerContext<'_>, route: &ModelInfo) -> usize {
        let context_window = if route.context_window_tokens == 0 {
            ctx.agent.memory().max_tokens()
        } else {
            route.context_window_tokens as usize
        };
        let policy = CompactionPolicy::default();
        context_window
            .saturating_sub(count_tokens_cached(ctx.system_prompt))
            .saturating_sub(Self::tool_schema_tokens(ctx.tools))
            .saturating_sub(route.max_output_tokens as usize)
            .saturating_sub(policy.hard_reserve_tokens)
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
                        crate::llm::LlmError::Unknown(
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

    async fn run_runtime_compaction(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        route: &ModelInfo,
        reason: CompactionReason,
        phase: CompactionPhase,
        force: bool,
    ) -> Result<bool> {
        self.run_runtime_compaction_with_target_budget(
            ctx,
            state,
            RuntimeCompactionRequest {
                route,
                reason,
                phase,
                force,
                target_token_budget: ctx.agent.memory().max_tokens(),
            },
        )
        .await
    }

    async fn run_runtime_compaction_with_target_budget(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        request: RuntimeCompactionRequest<'_>,
    ) -> Result<bool> {
        let Some(controller) = ctx.compaction_controller else {
            return Ok(false);
        };
        if !request.force && !Self::runtime_compaction_threshold_reached(ctx, request.route) {
            return Ok(false);
        }

        let context = CompactRequestContext {
            task: ctx.task.to_string(),
            route: request.route.clone(),
            reason: request.reason,
            phase: request.phase,
            target_token_budget: request.target_token_budget,
            created_at: chrono::Utc::now().to_rfc3339(),
            wiki_memory_lookup_available: wiki_memory_lookup_available(ctx.tools),
        };
        Self::emit_runtime_compaction_started(
            ctx.progress_tx,
            request.reason,
            request.phase,
            ctx.agent.memory().token_count(),
            ctx.agent.memory().get_messages().len(),
        )
        .await;
        let outcome = match Self::execute_runtime_controller_compaction(
            controller,
            ctx.agent.memory_mut(),
            context,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                warn!(
                    task_id = %ctx.task_id,
                    iteration = state.iteration,
                    reason = ?request.reason,
                    phase = ?request.phase,
                    error = %error,
                    "Runtime compaction failed in agent runner"
                );
                Self::emit_runtime_compaction_failed(
                    ctx.progress_tx,
                    request.reason,
                    request.phase,
                    error.to_string(),
                )
                .await;
                return Err(error.into());
            }
        };

        Self::log_runtime_compaction_success(ctx.task_id, state.iteration, &outcome);
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

    async fn execute_runtime_controller_compaction(
        controller: &crate::agent::compaction::CompactionController,
        memory: &mut crate::agent::memory::AgentMemory,
        context: CompactRequestContext,
    ) -> std::result::Result<CompactRunOutcome, crate::agent::compaction::CompactionControllerError>
    {
        match context.reason {
            CompactionReason::ContextLimit => {
                controller.compact_for_context_limit(memory, context).await
            }
            CompactionReason::ModelDownshift => {
                controller.model_downshift_compact(memory, context).await
            }
            _ => controller.manual_compact(memory, context).await,
        }
    }

    fn log_runtime_compaction_success(
        task_id: &str,
        iteration: usize,
        outcome: &CompactRunOutcome,
    ) {
        warn!(
            task_id = %task_id,
            iteration,
            reason = ?outcome.metadata.reason,
            phase = ?outcome.metadata.phase,
            backend = ?outcome.metadata.backend,
            provider = %outcome.metadata.provider,
            route = %outcome.metadata.route,
            generation = outcome.metadata.generation,
            hot_memory_tokens_before = outcome.replacement.token_before,
            hot_memory_tokens_after = outcome.replacement.token_after,
            history_items_before = outcome.replacement.history_items_before,
            history_items_after = outcome.replacement.history_items_after,
            "Agent runner completed runtime compaction"
        );
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
        CompactedSummaryMetadata, CompactionController, OXIDE_COMPACTED_SUMMARY_PREFIX,
    };
    use crate::agent::context::{AgentContext, EphemeralSession};
    use crate::agent::memory::AgentMessage;
    use crate::agent::progress::AgentEvent;
    use crate::agent::runner::test_support::{
        collect_progress_events, context_overflow_then_summary_then_final_provider,
        final_structured_response, pre_sampling_summary_then_final_provider, stub_non_chat_methods,
    };
    use crate::agent::runner::{AgentRunResult, AgentRunnerConfig, AgentRunnerContext};
    use crate::config::{AgentSettings, ModelInfo};
    use crate::llm::{LlmClient, LlmError, MockLlmProvider};
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct StaticRuntimeSummaryBackend;

    fn existing_compacted_summary() -> AgentMessage {
        AgentMessage::compacted_summary(
            "Old current-format state.",
            &CompactedSummaryMetadata {
                generation: 1,
                reason: CompactionReason::Manual,
                phase: CompactionPhase::Manual,
                token_before: 100,
                token_after: 10,
                history_items_before: 3,
                history_items_after: 1,
                provider: "opencode-go".to_string(),
                route: "deepseek-v4-flash".to_string(),
                backend: CompactionBackend::LocalLlmSummary,
                created_at: "2026-05-21T20:10:00+03:00".to_string(),
                previous_summary_detected: false,
                repair_applied: false,
                wiki_memory_lookup_available: false,
            },
        )
    }

    #[async_trait]
    impl CompactSummaryBackend for StaticRuntimeSummaryBackend {
        async fn summarize(
            &self,
            request: CompactSummaryRequest<'_>,
        ) -> std::result::Result<CompactSummaryResult, CompactSummaryError> {
            Ok(CompactSummaryResult {
                summary_text: "Condensed history for smaller fallback route.".to_string(),
                provider: request.route.provider.clone(),
                route: request.route.id.clone(),
            })
        }
    }

    #[tokio::test]
    async fn run_retries_after_context_overflow_with_runtime_context_limit_compaction() {
        let llm_client = crate::agent::runner::test_support::build_llm_client(
            context_overflow_then_summary_then_final_provider(),
        );
        let compaction_controller = CompactionController::local_llm(Arc::clone(&llm_client), 1);
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Retry after overflow"));
        session
            .memory_mut()
            .add_message(existing_compacted_summary());
        session
            .memory_mut()
            .add_message(AgentMessage::user("Recent request."));

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let mut ctx = AgentRunnerContext {
            task: "Retry after overflow",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-overflow-runtime-compaction",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: Some(&compaction_controller),
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 2, 1, 30, 256),
        };

        let result = runner
            .run(&mut ctx)
            .await
            .expect("runner succeeds after retry");

        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));
        assert!(ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .any(|message| message
                .content
                .starts_with(crate::agent::compaction::OXIDE_COMPACTED_SUMMARY_PREFIX)));
        assert!(ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .all(|message| !message.content.contains("[COMPACTION_SUMMARY]")));
        drop(ctx);
        drop(progress_tx);

        let events = collect_progress_events(&mut progress_rx).await;
        let started = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::RuntimeCompactionStarted {
                        reason: CompactionReason::ContextLimit,
                        phase: CompactionPhase::MidTurn,
                        ..
                    }
                )
            })
            .expect("runtime context-limit compaction started");
        let completed = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::RuntimeCompactionCompleted {
                        reason: CompactionReason::ContextLimit,
                        phase: CompactionPhase::MidTurn,
                        ..
                    }
                )
            })
            .expect("runtime context-limit compaction completed");
        assert!(started < completed);
    }

    #[tokio::test]
    async fn run_pre_sampling_uses_runtime_compaction_when_threshold_reached() {
        let llm_client = crate::agent::runner::test_support::build_llm_client(
            pre_sampling_summary_then_final_provider(),
        );
        let compaction_controller = CompactionController::local_llm(Arc::clone(&llm_client), 1);
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(1_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Pre-sampling compact"));
        session
            .memory_mut()
            .add_message(AgentMessage::user("older ".repeat(4_000)));

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let mut ctx = AgentRunnerContext {
            task: "Pre-sampling compact",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-pre-sampling-runtime-compaction",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: Some(&compaction_controller),
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 1, 1, 30, 256),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");

        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));
        assert!(ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .any(|message| message
                .content
                .starts_with(crate::agent::compaction::OXIDE_COMPACTED_SUMMARY_PREFIX)));
        drop(ctx);
        drop(progress_tx);

        let events = collect_progress_events(&mut progress_rx).await;
        assert!(events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::RuntimeCompactionCompleted {
                    reason: CompactionReason::PreTurn,
                    phase: CompactionPhase::PreSampling,
                    ..
                }
            )
        }));
    }

    #[test]
    fn runtime_compaction_threshold_uses_full_request_budget() {
        let tools = Vec::new();
        let mut session = EphemeralSession::new(20_000);
        session.memory_mut().add_message(AgentMessage::user_task(
            "Compact because output reserve consumes the route window",
        ));

        assert!(
            session.memory().token_count().saturating_mul(100)
                < session.memory().max_tokens().saturating_mul(85),
            "test fixture must stay below the old hot-memory-only threshold"
        );

        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let ctx = AgentRunnerContext {
            task: "Compact because output reserve consumes the route window",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-full-budget-threshold",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 1, 1, 30, 16_000),
        };
        let route = ModelInfo {
            id: "deepseek-v4-flash".to_string(),
            provider: "opencode-go".to_string(),
            max_output_tokens: 16_000,
            context_window_tokens: 20_000,
            weight: 1,
        };

        assert!(AgentRunner::runtime_compaction_threshold_reached(
            &ctx, &route
        ));
        drop(ctx);
    }

    #[tokio::test]
    async fn run_compacts_before_downshifting_to_smaller_model_route() {
        let mut primary = MockLlmProvider::new();
        primary
            .expect_chat_with_tools()
            .times(LlmClient::MAX_RETRIES)
            .returning(|_| {
                Err(LlmError::RateLimit {
                    wait_secs: Some(0),
                    message: "primary rate limit".to_string(),
                })
            });
        stub_non_chat_methods(&mut primary);

        let mut backup = MockLlmProvider::new();
        backup
            .expect_chat_with_tools()
            .times(1)
            .withf(|request| {
                request
                    .system_prompt
                    .contains(OXIDE_COMPACTED_SUMMARY_PREFIX)
                    && !request.messages.iter().any(|message| {
                        message
                            .content
                            .trim_start()
                            .starts_with(OXIDE_COMPACTED_SUMMARY_PREFIX)
                    })
            })
            .return_once(|_| Ok(final_structured_response()));
        stub_non_chat_methods(&mut backup);

        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-pro".to_string()),
            agent_model_provider: Some("llm-provider/opencode-go".to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm_client = LlmClient::new(&settings);
        llm_client.register_provider("llm-provider/opencode-go".to_string(), Arc::new(primary));
        llm_client.register_provider("opencode-go".to_string(), Arc::new(backup));
        let llm_client = Arc::new(llm_client);

        let compaction_controller =
            CompactionController::new(Arc::new(StaticRuntimeSummaryBackend));
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(50_000);
        session.memory_mut().add_message(AgentMessage::user_task(
            "Fail over to a smaller model route",
        ));
        for index in 0..24 {
            session
                .memory_mut()
                .add_message(AgentMessage::user_turn(format!(
                    "background-{index}: {}",
                    "route downshift history ".repeat(160)
                )));
        }

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let mut ctx = AgentRunnerContext {
            task: "Fail over to a smaller model route",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-model-downshift",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: Some(&compaction_controller),
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-pro".to_string(), 1, 1, 30, 256)
                .with_model_provider("llm-provider/opencode-go")
                .with_model_routes(vec![
                    ModelInfo {
                        id: "deepseek-v4-pro".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 128_000,
                        provider: "llm-provider/opencode-go".to_string(),
                        weight: 1,
                    },
                    ModelInfo {
                        id: "deepseek-v4-flash".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 15_000,
                        provider: "opencode-go".to_string(),
                        weight: 1,
                    },
                ]),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");
        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));

        assert!(ctx.agent.memory().get_messages().iter().any(|message| {
            message
                .content
                .trim_start()
                .starts_with(OXIDE_COMPACTED_SUMMARY_PREFIX)
        }));

        drop(ctx);
        drop(progress_tx);
        let events = collect_progress_events(&mut progress_rx).await;
        assert!(events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::RuntimeCompactionCompleted {
                    reason: CompactionReason::ModelDownshift,
                    phase: CompactionPhase::ModelSwitch,
                    provider,
                    route,
                    ..
                } if provider == "opencode-go" && route == "deepseek-v4-flash"
            )
        }));
    }
}
