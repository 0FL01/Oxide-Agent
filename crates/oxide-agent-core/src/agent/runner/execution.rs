//! Core execution loop for the agent runner.

use super::types::{
    AgentRunResult, AgentRunnerContext, FinalResponseInput, RunState, StructuredOutputFailure,
};
use super::AgentRunner;
use crate::agent::compaction::{estimate_request_budget, CompactionRequest, CompactionTrigger};
use crate::agent::memory::AgentMessage;
use crate::agent::progress::{AgentEvent, RepeatedCompactionKind, TokenSnapshot};
use crate::agent::recovery::sanitize_tool_calls;
use crate::agent::structured_output::parse_structured_output;
use crate::llm::{ChatResponse, LlmError};
use anyhow::{anyhow, Result};
use tracing::{debug, info, warn};

impl AgentRunner {
    /// Execute the agent loop until completion or error.
    pub async fn run(&mut self, ctx: &mut AgentRunnerContext<'_>) -> Result<AgentRunResult> {
        self.reset_loop_detector(ctx).await;
        self.apply_before_agent_hooks(ctx)?;
        self.run_loop(ctx).await
    }

    async fn run_loop(&mut self, ctx: &mut AgentRunnerContext<'_>) -> Result<AgentRunResult> {
        let mut state = RunState::new();

        for iteration in 0..ctx.config.max_iterations {
            state.iteration = iteration;

            if ctx.agent.cancellation_token().is_cancelled() {
                return Err(self.cancelled_error(ctx).await);
            }

            if ctx.agent.elapsed_secs() >= ctx.config.timeout_secs {
                if let Some(res) = self.apply_timeout_hook(ctx, &state)? {
                    return Ok(AgentRunResult::Final(res));
                }
            }

            self.apply_pending_runtime_context(ctx, &mut state).await;

            self.apply_before_iteration_hooks(ctx, &state)?;
            self.run_iteration_compaction(ctx, &mut state, iteration)
                .await?;

            debug!(task_id = %ctx.task_id, iteration = iteration, "Agent loop iteration");

            let snapshot_trigger = if iteration == 0 {
                CompactionTrigger::PreRun
            } else {
                CompactionTrigger::PreIteration
            };
            let snapshot = Self::build_token_snapshot(ctx, snapshot_trigger);
            Self::log_token_snapshot(ctx, iteration, "before_llm_call", &snapshot);
            if let Some(tx) = ctx.progress_tx {
                let _ = tx.send(AgentEvent::Thinking { snapshot }).await;
            }

            if self.llm_loop_detected(ctx, &state).await {
                return Err(self
                    .loop_detected_error(
                        ctx,
                        &state,
                        crate::agent::loop_detection::LoopType::CognitiveLoop,
                    )
                    .await);
            }

            let response = self.call_llm_with_tools(ctx, &mut state).await?;
            if let Some(result) = self.handle_llm_response(response, ctx, &mut state).await? {
                return Ok(result);
            }
        }

        Err(anyhow!(
            "Agent exceeded iteration limit ({}).",
            ctx.config.max_iterations
        ))
    }

    async fn call_llm_with_tools(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) -> Result<ChatResponse> {
        match self.chat_with_tools_once(ctx).await {
            Ok(response) => Ok(response),
            Err(error) if Self::llm_error_suggests_context_overflow(&error) => {
                let retried = self
                    .run_compaction_checkpoint(ctx, state, CompactionTrigger::Manual)
                    .await?;
                if retried {
                    match self.chat_with_tools_once(ctx).await {
                        Ok(response) => return Ok(response),
                        Err(retry_error) => {
                            Self::emit_llm_error(ctx.progress_tx, &retry_error).await;
                            return Err(anyhow!(
                                "LLM call failed after compaction retry: {retry_error}"
                            ));
                        }
                    }
                }

                Self::emit_llm_error(ctx.progress_tx, &error).await;
                Err(anyhow!("LLM call failed: {error}"))
            }
            Err(error) => {
                Self::emit_llm_error(ctx.progress_tx, &error).await;
                Err(anyhow!("LLM call failed: {error}"))
            }
        }
    }

    async fn chat_with_tools_once(
        &self,
        ctx: &mut AgentRunnerContext<'_>,
    ) -> Result<ChatResponse, LlmError> {
        let json_mode = self.requires_structured_output(&ctx.config.model_name);
        self.llm_client
            .chat_with_tools(
                ctx.system_prompt,
                ctx.messages,
                ctx.tools,
                &ctx.config.model_name,
                json_mode,
            )
            .await
    }

    async fn handle_llm_response(
        &mut self,
        mut response: ChatResponse,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) -> Result<Option<AgentRunResult>> {
        self.preprocess_llm_response(&mut response, ctx).await;

        let raw_json = response
            .content
            .clone()
            .unwrap_or_default()
            .trim()
            .to_string();

        if !response.tool_calls.is_empty() {
            return self
                .handle_tool_calls_response(&mut response, &raw_json, ctx, state)
                .await;
        }

        if !self.requires_structured_output(&ctx.config.model_name) {
            let reasoning = response.reasoning_content.take();
            return self
                .handle_unstructured_response(reasoning, raw_json.clone(), ctx, state)
                .await;
        }

        let parsed = match parse_structured_output(&raw_json, ctx.tools) {
            Ok(parsed) => {
                state.structured_output_failures = 0;
                parsed
            }
            Err(error) => {
                let failure = StructuredOutputFailure { error, raw_json };
                return self
                    .handle_structured_output_error(ctx, state, failure)
                    .await;
            }
        };

        let tool_calls = parsed
            .tool_call
            .map(|tool_call| vec![self.build_tool_call(tool_call)])
            .unwrap_or_default();

        self.spawn_narrative_task(
            response.reasoning_content.as_deref(),
            &tool_calls,
            ctx.progress_tx,
        );

        if tool_calls.is_empty() {
            let final_answer = parsed
                .final_answer
                .unwrap_or_else(|| "Task completed, but answer is empty.".to_string());

            if self.content_loop_detected(final_answer.as_str()).await {
                return Err(self
                    .loop_detected_error(
                        ctx,
                        state,
                        crate::agent::loop_detection::LoopType::ContentLoop,
                    )
                    .await);
            }

            let input = FinalResponseInput {
                final_answer,
                raw_json,
                reasoning: response.reasoning_content,
            };

            return self.handle_final_response(ctx, state, input).await;
        }

        if self.tool_loop_detected(&tool_calls).await {
            return Err(self
                .loop_detected_error(
                    ctx,
                    state,
                    crate::agent::loop_detection::LoopType::ToolCallLoop,
                )
                .await);
        }

        self.record_assistant_tool_call(ctx, &raw_json, &tool_calls);
        if let Some(res) = self.execute_tools(ctx, state, tool_calls).await? {
            return Ok(Some(res));
        }
        Ok(None)
    }

    async fn apply_pending_runtime_context(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) {
        let pending_context = ctx.agent.drain_runtime_context();
        if pending_context.is_empty() {
            return;
        }

        state.continuation_count += 1;
        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::Continuation {
                    reason: "New user context received, adapting the plan.".to_string(),
                    count: state.continuation_count,
                })
                .await;
        }

        for injection in pending_context {
            ctx.messages
                .push(crate::llm::Message::user(&injection.content));
            ctx.agent
                .memory_mut()
                .add_message(AgentMessage::runtime_context(injection.content));
        }
    }

    async fn run_iteration_compaction(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        iteration: usize,
    ) -> Result<()> {
        let trigger = if iteration == 0 {
            CompactionTrigger::PreRun
        } else {
            CompactionTrigger::PreIteration
        };
        let _ = self.run_compaction_checkpoint(ctx, state, trigger).await?;
        Ok(())
    }

    async fn run_compaction_checkpoint(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        trigger: CompactionTrigger,
    ) -> Result<bool> {
        let Some(compaction_service) = ctx.compaction_service else {
            return Ok(false);
        };

        let request = CompactionRequest::new(
            trigger,
            ctx.task,
            ctx.system_prompt,
            ctx.tools,
            &ctx.config.model_name,
            ctx.config.model_max_output_tokens,
            ctx.config.is_sub_agent,
        );
        let outcome = match compaction_service
            .prepare_for_run(&request, ctx.agent)
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                Self::log_compaction_failure(ctx.task_id, state.iteration, trigger, &error);
                Self::emit_compaction_failed(ctx.progress_tx, trigger, error.to_string()).await;
                return Err(error);
            }
        };
        Self::log_compaction_success(ctx.task_id, state.iteration, trigger, &outcome);
        let should_emit_progress = matches!(trigger, CompactionTrigger::Manual)
            || outcome.applied
            || outcome.summary_generation.attempted
            || outcome.archive_persistence.attempted;
        if should_emit_progress {
            Self::emit_compaction_started(ctx.progress_tx, trigger).await;
            if outcome.pruning.applied {
                Self::emit_pruning_applied(ctx.progress_tx, &outcome).await;
            }
            Self::emit_compaction_completed(ctx.progress_tx, trigger, &outcome).await;
        }
        if outcome.applied {
            Self::refresh_messages_from_memory(ctx);
            Self::track_repeated_compaction(ctx, state, trigger, &outcome).await;
        }

        Ok(outcome.applied)
    }

    fn log_compaction_failure(
        task_id: &str,
        iteration: usize,
        trigger: CompactionTrigger,
        error: &anyhow::Error,
    ) {
        warn!(
            task_id = %task_id,
            iteration,
            trigger = ?trigger,
            error = %error,
            "Compaction checkpoint failed in agent runner"
        );
    }

    fn log_compaction_success(
        task_id: &str,
        iteration: usize,
        trigger: CompactionTrigger,
        outcome: &crate::agent::CompactionOutcome,
    ) {
        if !outcome.requires_warn_log() {
            return;
        }

        warn!(
            task_id = %task_id,
            iteration,
            trigger = ?trigger,
            applied = outcome.applied,
            budget_state = ?outcome.budget.state,
            hot_memory_tokens_before = outcome.token_count_before,
            hot_memory_tokens_after = outcome.token_count_after,
            externalized_count = outcome.externalization.externalized_count,
            pruned_count = outcome.pruning.pruned_count,
            reclaimed_tokens = outcome.reclaimed_hot_memory_tokens(),
            cleanup_reclaimed_tokens = outcome.reclaimed_cleanup_tokens(),
            summary_attempted = outcome.summary_generation.attempted,
            summary_used_fallback = outcome.summary_generation.used_fallback,
            archived_chunk_count = outcome.archive_persistence.archived_chunk_count,
            summary_updated = outcome.rebuild.inserted_summary,
            "Agent runner completed compaction checkpoint"
        );
    }

    async fn track_repeated_compaction(
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        trigger: CompactionTrigger,
        outcome: &crate::agent::CompactionOutcome,
    ) {
        if outcome.rebuild.inserted_summary {
            state.compaction_count = state.compaction_count.saturating_add(1);
            if state.compaction_count > 1 {
                Self::log_repeated_compaction(
                    ctx.task_id,
                    state.iteration,
                    trigger,
                    RepeatedCompactionKind::Compaction,
                    state.compaction_count,
                );
                Self::emit_repeated_compaction_warning(
                    ctx.progress_tx,
                    RepeatedCompactionKind::Compaction,
                    state.compaction_count,
                )
                .await;
            }
            return;
        }

        if outcome.externalization.applied || outcome.pruning.applied {
            state.cleanup_count = state.cleanup_count.saturating_add(1);
            if state.cleanup_count > 1 {
                Self::log_repeated_compaction(
                    ctx.task_id,
                    state.iteration,
                    trigger,
                    RepeatedCompactionKind::Cleanup,
                    state.cleanup_count,
                );
                Self::emit_repeated_compaction_warning(
                    ctx.progress_tx,
                    RepeatedCompactionKind::Cleanup,
                    state.cleanup_count,
                )
                .await;
            }
        }
    }

    fn log_repeated_compaction(
        task_id: &str,
        iteration: usize,
        trigger: CompactionTrigger,
        kind: RepeatedCompactionKind,
        count: usize,
    ) {
        let message = match kind {
            RepeatedCompactionKind::Cleanup => "Agent run required repeated deterministic cleanup",
            RepeatedCompactionKind::Compaction => "Agent run required repeated summary compaction",
        };
        warn!(
            task_id = %task_id,
            iteration,
            trigger = ?trigger,
            kind = ?kind,
            count,
            "{message}"
        );
    }

    async fn handle_tool_calls_response(
        &mut self,
        response: &mut ChatResponse,
        raw_json: &str,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) -> Result<Option<AgentRunResult>> {
        let tool_calls = sanitize_tool_calls(std::mem::take(&mut response.tool_calls));

        self.spawn_narrative_task(
            response.reasoning_content.as_deref(),
            &tool_calls,
            ctx.progress_tx,
        );

        if self.tool_loop_detected(&tool_calls).await {
            return Err(self
                .loop_detected_error(
                    ctx,
                    state,
                    crate::agent::loop_detection::LoopType::ToolCallLoop,
                )
                .await);
        }

        self.record_assistant_tool_call(ctx, raw_json, &tool_calls);
        if let Some(res) = self.execute_tools(ctx, state, tool_calls).await? {
            return Ok(Some(res));
        }
        Ok(None)
    }

    async fn handle_unstructured_response(
        &mut self,
        reasoning: Option<String>,
        raw_output: String,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) -> Result<Option<AgentRunResult>> {
        self.spawn_narrative_task(reasoning.as_deref(), &[], ctx.progress_tx);

        let final_answer = if raw_output.trim().is_empty() {
            "Task completed, but answer is empty.".to_string()
        } else {
            raw_output.clone()
        };

        if self.content_loop_detected(final_answer.as_str()).await {
            return Err(self
                .loop_detected_error(
                    ctx,
                    state,
                    crate::agent::loop_detection::LoopType::ContentLoop,
                )
                .await);
        }

        let input = FinalResponseInput {
            final_answer,
            raw_json: raw_output,
            reasoning,
        };

        self.handle_final_response(ctx, state, input).await
    }

    fn requires_structured_output(&self, model_name: &str) -> bool {
        match self.llm_client.get_model_info(model_name) {
            Ok(info) => !info.provider.eq_ignore_ascii_case("zai"),
            Err(error) => {
                warn!(
                    model = model_name,
                    error = %error,
                    "Failed to resolve model info; defaulting to structured output"
                );
                true
            }
        }
    }

    async fn preprocess_llm_response(
        &mut self,
        response: &mut ChatResponse,
        ctx: &mut AgentRunnerContext<'_>,
    ) {
        if let Some(u) = &response.usage {
            ctx.agent.memory_mut().sync_api_usage(u.clone());
            let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
            Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;
        }

        if let Some(ref reasoning) = response.reasoning_content {
            debug!(reasoning_len = reasoning.len(), "Model reasoning received");

            if let Some(tx) = ctx.progress_tx {
                let summary = crate::agent::thoughts::extract_reasoning_summary(reasoning, 100);
                let _ = tx.send(AgentEvent::Reasoning { summary }).await;
            }
        }

        let content_empty = response
            .content
            .as_deref()
            .map(|content| content.trim().is_empty())
            .unwrap_or(true);

        if content_empty && response.tool_calls.is_empty() {
            warn!(model = %ctx.config.model_name, "Model returned empty content");
        }
    }

    async fn emit_llm_error(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        error: &LlmError,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::Error(format!("LLM call failed: {error}")))
                .await;
        }
    }

    async fn emit_compaction_started(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        trigger: CompactionTrigger,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx.send(AgentEvent::CompactionStarted { trigger }).await;
        }
    }

    async fn emit_pruning_applied(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        outcome: &crate::agent::CompactionOutcome,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::PruningApplied {
                    pruned_count: outcome.pruning.pruned_count,
                    reclaimed_tokens: outcome.pruning.reclaimed_tokens,
                })
                .await;
        }
    }

    async fn emit_compaction_completed(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        trigger: CompactionTrigger,
        outcome: &crate::agent::CompactionOutcome,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::CompactionCompleted {
                    trigger,
                    applied: outcome.applied,
                    externalized_count: outcome.externalization.externalized_count,
                    pruned_count: outcome.pruning.pruned_count,
                    reclaimed_tokens: outcome.reclaimed_hot_memory_tokens(),
                    archived_chunk_count: outcome.archive_persistence.archived_chunk_count,
                    summary_updated: outcome.rebuild.inserted_summary,
                })
                .await;
        }
    }

    async fn emit_compaction_failed(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        trigger: CompactionTrigger,
        error: String,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::CompactionFailed { trigger, error })
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

    pub(super) fn build_token_snapshot(
        ctx: &AgentRunnerContext<'_>,
        trigger: CompactionTrigger,
    ) -> TokenSnapshot {
        let request = CompactionRequest::new(
            trigger,
            ctx.task,
            ctx.system_prompt,
            ctx.tools,
            &ctx.config.model_name,
            ctx.config.model_max_output_tokens,
            ctx.config.is_sub_agent,
        );
        let policy = ctx
            .compaction_service
            .map(|service| service.policy().clone())
            .unwrap_or_default();
        let budget = estimate_request_budget(&policy, &request, ctx.agent);

        TokenSnapshot {
            hot_memory_tokens: budget.hot_memory.total_tokens,
            system_prompt_tokens: budget.system_prompt_tokens,
            tool_schema_tokens: budget.tool_schema_tokens,
            loaded_skill_tokens: budget.loaded_skill_tokens,
            total_input_tokens: budget.total_input_tokens,
            reserved_output_tokens: budget.reserved_output_tokens,
            projected_total_tokens: budget.projected_total_tokens,
            context_window_tokens: budget.context_window_tokens,
            headroom_tokens: budget.headroom_tokens,
            budget_state: budget.state,
            last_api_usage: ctx.agent.memory().api_usage().cloned(),
        }
    }

    pub(super) async fn emit_token_snapshot_update(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        snapshot: TokenSnapshot,
    ) {
        let Some(tx) = progress_tx else { return };
        let _ = tx.send(AgentEvent::TokenSnapshotUpdated { snapshot }).await;
    }

    fn log_token_snapshot(
        ctx: &AgentRunnerContext<'_>,
        iteration: usize,
        phase: &str,
        snapshot: &TokenSnapshot,
    ) {
        info!(
            task_id = %ctx.task_id,
            iteration,
            phase,
            hot_memory_tokens = snapshot.hot_memory_tokens,
            system_prompt_tokens = snapshot.system_prompt_tokens,
            tool_schema_tokens = snapshot.tool_schema_tokens,
            loaded_skill_tokens = snapshot.loaded_skill_tokens,
            total_input_tokens = snapshot.total_input_tokens,
            reserved_output_tokens = snapshot.reserved_output_tokens,
            projected_total_tokens = snapshot.projected_total_tokens,
            context_window_tokens = snapshot.context_window_tokens,
            headroom_tokens = snapshot.headroom_tokens,
            budget_state = ?snapshot.budget_state,
            last_api_prompt_tokens = snapshot.last_api_usage.as_ref().map(|usage| usage.prompt_tokens),
            last_api_completion_tokens = snapshot
                .last_api_usage
                .as_ref()
                .map(|usage| usage.completion_tokens),
            last_api_total_tokens = snapshot.last_api_usage.as_ref().map(|usage| usage.total_tokens),
            "Agent request token snapshot"
        );
    }

    fn llm_error_suggests_context_overflow(error: &LlmError) -> bool {
        let message = error.to_string().to_ascii_lowercase();
        [
            "context length",
            "context window",
            "too many tokens",
            "token limit",
            "maximum context",
            "prompt is too long",
            "context overflow",
        ]
        .iter()
        .any(|needle| message.contains(needle))
    }

    fn refresh_messages_from_memory(ctx: &mut AgentRunnerContext<'_>) {
        *ctx.messages = Self::convert_memory_to_messages(ctx.agent.memory().get_messages());
    }

    fn spawn_narrative_task(
        &self,
        reasoning: Option<&str>,
        tool_calls: &[crate::llm::ToolCall],
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) {
        let Some(tx) = progress_tx else { return };

        let narrator = std::sync::Arc::clone(&self.narrator);
        let reasoning = reasoning.map(str::to_string);
        let tool_calls = tool_calls.to_vec();
        let tx = tx.clone();

        tokio::spawn(async move {
            if let Some(narrative) = narrator.generate(reasoning.as_deref(), &tool_calls).await {
                let _ = tx
                    .send(AgentEvent::Narrative {
                        headline: narrative.headline,
                        content: narrative.content,
                    })
                    .await;
            }
        });
    }

    /// Build a cancellation error and perform cleanup.
    pub(super) async fn cancelled_error(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
    ) -> anyhow::Error {
        ctx.agent.memory_mut().todos.clear();
        let mut todos = ctx.todos_arc.lock().await;
        todos.clear();

        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::TodosUpdated {
                    todos: crate::agent::providers::TodoList::new(),
                })
                .await;
            let _ = tx.send(AgentEvent::Cancelled).await;
        }

        anyhow!("Task cancelled by user")
    }

    // Response helpers live in responses.rs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::{
        CompactionService, CompactionSummarizer, CompactionSummarizerConfig,
    };
    use crate::agent::context::{AgentContext, EphemeralSession};
    use crate::agent::provider::ToolProvider;
    use crate::agent::registry::ToolRegistry;
    use crate::agent::runner::{AgentRunResult, AgentRunnerConfig, AgentRunnerContext};
    use crate::config::AgentSettings;
    use crate::llm::{
        ChatResponse, LlmClient, MockLlmProvider, TokenUsage, ToolCall, ToolCallFunction,
        ToolDefinition,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    struct LargeOutputToolProvider;

    struct SmallOutputToolProvider;

    #[async_trait]
    impl ToolProvider for LargeOutputToolProvider {
        fn name(&self) -> &'static str {
            "large-output"
        }

        fn tools(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "fake_large_tool".to_string(),
                description: "Return a large payload".to_string(),
                parameters: json!({"type": "object", "properties": {}}),
            }]
        }

        fn can_handle(&self, tool_name: &str) -> bool {
            tool_name == "fake_large_tool"
        }

        async fn execute(
            &self,
            _tool_name: &str,
            _arguments: &str,
            _progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
            _cancellation_token: Option<&CancellationToken>,
        ) -> anyhow::Result<String> {
            Ok("Z".repeat(5_000))
        }
    }

    #[async_trait]
    impl ToolProvider for SmallOutputToolProvider {
        fn name(&self) -> &'static str {
            "small-output"
        }

        fn tools(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "fake_small_tool".to_string(),
                description: "Return a small payload".to_string(),
                parameters: json!({"type": "object", "properties": {}}),
            }]
        }

        fn can_handle(&self, tool_name: &str) -> bool {
            tool_name == "fake_small_tool"
        }

        async fn execute(
            &self,
            _tool_name: &str,
            _arguments: &str,
            _progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
            _cancellation_token: Option<&CancellationToken>,
        ) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
    }

    #[tokio::test]
    async fn run_applies_pre_run_compaction_before_first_llm_call() {
        let llm_client = build_llm_client(single_final_response_provider());
        let summarizer = CompactionSummarizer::new(
            Arc::clone(&llm_client),
            CompactionSummarizerConfig {
                model_name: String::new(),
                provider_name: String::new(),
                timeout_secs: 1,
            },
        );
        let compaction_service = CompactionService::default().with_summarizer(summarizer);
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(256);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Ship stage 9"));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Older request about compaction."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Older response with findings."));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Recent request 1."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Recent response 1."));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Recent request 2."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Recent response 2."));

        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let mut ctx = AgentRunnerContext {
            task: "Ship stage 9",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-pre-run",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_service: Some(&compaction_service),
            config: AgentRunnerConfig::new("mock-model".to_string(), 2, 1, 30, 256),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");

        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));
        assert!(ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .any(|message| message.summary_payload().is_some()));
        assert!(!ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .any(|message| message.content == "Older request about compaction."));
    }

    #[tokio::test]
    async fn run_applies_pre_iteration_compaction_after_tool_growth() {
        let llm_client = build_llm_client(tool_then_final_provider());
        let compaction_service = CompactionService::default();
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Inspect large tool payloads"));

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(LargeOutputToolProvider));
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let mut ctx = AgentRunnerContext {
            task: "Inspect large tool payloads",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-pre-iteration",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_service: Some(&compaction_service),
            config: AgentRunnerConfig::new("mock-model".to_string(), 3, 1, 30, 256),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");

        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));
        assert!(ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .any(AgentMessage::is_externalized));
    }

    #[tokio::test]
    async fn run_does_not_warn_when_cleanup_follows_single_summary_compaction() {
        let llm_client = build_llm_client(tool_then_final_provider());
        let summarizer = CompactionSummarizer::new(
            Arc::clone(&llm_client),
            CompactionSummarizerConfig {
                model_name: String::new(),
                provider_name: String::new(),
                timeout_secs: 1,
            },
        );
        let compaction_service = CompactionService::default().with_summarizer(summarizer);
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(256);
        session
            .memory_mut()
            .add_message(AgentMessage::topic_agents_md(
                "# Topic AGENTS\nPreserve operator instructions.",
            ));
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Ship stage 12"));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Older request about compaction."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Older response with findings."));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Recent request 1."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Recent response 1."));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Recent request 2."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Recent response 2."));

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(LargeOutputToolProvider));
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(32);
        let mut ctx = AgentRunnerContext {
            task: "Ship stage 12",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-repeated-compaction",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_service: Some(&compaction_service),
            config: AgentRunnerConfig::new("mock-model".to_string(), 3, 1, 30, 256),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");

        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));
        assert!(ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .any(|message| message.summary_payload().is_some()));
        assert!(ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .any(AgentMessage::is_externalized));

        drop(ctx);
        drop(progress_tx);

        let mut repeated_warning = None;
        let mut completion_events = 0usize;
        while let Some(event) = progress_rx.recv().await {
            match event {
                AgentEvent::CompactionCompleted { .. } => {
                    completion_events = completion_events.saturating_add(1);
                }
                AgentEvent::RepeatedCompactionWarning { kind, count } => {
                    repeated_warning = Some((kind, count));
                }
                _ => {}
            }
        }

        assert!(completion_events >= 2);
        assert_eq!(repeated_warning, None);
    }

    #[tokio::test]
    async fn run_emits_repeated_cleanup_warning_on_second_cleanup_pass() {
        let llm_client = build_llm_client(repeated_cleanup_provider());
        let compaction_service = CompactionService::default();
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Inspect repeated cleanup"));

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(LargeOutputToolProvider));
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let mut ctx = AgentRunnerContext {
            task: "Inspect repeated cleanup",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-repeated-cleanup",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_service: Some(&compaction_service),
            config: AgentRunnerConfig::new("mock-model".to_string(), 4, 1, 30, 256),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");

        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));
        drop(ctx);
        drop(progress_tx);

        let events = collect_progress_events(&mut progress_rx).await;
        let repeated_cleanup = events.iter().find_map(|event| match event {
            AgentEvent::RepeatedCompactionWarning { kind, count } => Some((*kind, *count)),
            _ => None,
        });
        let cleanup_completions = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    AgentEvent::CompactionCompleted {
                        externalized_count,
                        summary_updated,
                        ..
                    } if *externalized_count > 0 && !summary_updated
                )
            })
            .count();
        let second_cleanup_completion_index = events
            .iter()
            .enumerate()
            .filter(|(_, event)| {
                matches!(
                    event,
                    AgentEvent::CompactionCompleted {
                        externalized_count,
                        summary_updated,
                        ..
                    } if *externalized_count > 0 && !summary_updated
                )
            })
            .nth(1)
            .map(|(index, _)| index)
            .expect("second cleanup completion event");
        let warning_index = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::RepeatedCompactionWarning {
                        kind: RepeatedCompactionKind::Cleanup,
                        count: 2,
                    }
                )
            })
            .expect("cleanup warning event");

        assert_eq!(repeated_cleanup, Some((RepeatedCompactionKind::Cleanup, 2)));
        assert_eq!(cleanup_completions, 2);
        assert!(warning_index > second_cleanup_completion_index);
    }

    #[tokio::test]
    async fn run_emits_repeated_summary_warning_on_second_summary_pass() {
        let llm_client = build_llm_client(repeated_summary_provider());
        let summarizer = CompactionSummarizer::new(
            Arc::clone(&llm_client),
            CompactionSummarizerConfig {
                model_name: String::new(),
                provider_name: String::new(),
                timeout_secs: 1,
            },
        );
        let compaction_service = CompactionService::default().with_summarizer(summarizer);
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = base_summary_session("Inspect repeated summary compaction");

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(SmallOutputToolProvider));
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let mut ctx = AgentRunnerContext {
            task: "Inspect repeated summary compaction",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-repeated-summary",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_service: Some(&compaction_service),
            config: AgentRunnerConfig::new("mock-model".to_string(), 5, 1, 30, 256),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");

        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));
        drop(ctx);
        drop(progress_tx);

        let events = collect_progress_events(&mut progress_rx).await;
        let summary_completions: Vec<usize> = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| match event {
                AgentEvent::CompactionCompleted {
                    summary_updated: true,
                    ..
                } => Some(index),
                _ => None,
            })
            .collect();
        let warning_index = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::RepeatedCompactionWarning {
                        kind: RepeatedCompactionKind::Compaction,
                        count: 2,
                    }
                )
            })
            .expect("summary warning event");

        assert!(summary_completions.len() >= 2);
        assert!(warning_index > summary_completions[1]);
    }

    #[tokio::test]
    async fn run_overflow_retry_emits_manual_compaction_progress_in_order() {
        let llm_client = build_llm_client(context_overflow_then_final_provider());
        let summarizer = CompactionSummarizer::new(
            Arc::clone(&llm_client),
            CompactionSummarizerConfig {
                model_name: String::new(),
                provider_name: String::new(),
                timeout_secs: 1,
            },
        );
        let compaction_service = CompactionService::default().with_summarizer(summarizer);
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session.memory_mut().add_message(AgentMessage::user_task(
            "Retry after overflow with progress",
        ));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Older request about compaction."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Older response with findings."));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Recent request 1."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Recent response 1."));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Recent request 2."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Recent response 2."));

        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let mut ctx = AgentRunnerContext {
            task: "Retry after overflow with progress",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-overflow-progress",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_service: Some(&compaction_service),
            config: AgentRunnerConfig::new("mock-model".to_string(), 2, 1, 30, 256),
        };

        let result = runner
            .run(&mut ctx)
            .await
            .expect("runner succeeds after retry");

        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));
        drop(ctx);
        drop(progress_tx);

        let events = collect_progress_events(&mut progress_rx).await;
        let manual_started = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::CompactionStarted {
                        trigger: CompactionTrigger::Manual,
                    }
                )
            })
            .expect("manual compaction started event");
        let manual_completed = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::CompactionCompleted {
                        trigger: CompactionTrigger::Manual,
                        summary_updated: true,
                        ..
                    }
                )
            })
            .expect("manual compaction completed event");

        assert!(manual_started < manual_completed);
    }

    #[tokio::test]
    async fn run_retries_after_context_overflow_with_manual_compaction() {
        let llm_client = build_llm_client(context_overflow_then_final_provider());
        let summarizer = CompactionSummarizer::new(
            Arc::clone(&llm_client),
            CompactionSummarizerConfig {
                model_name: String::new(),
                provider_name: String::new(),
                timeout_secs: 1,
            },
        );
        let compaction_service = CompactionService::default().with_summarizer(summarizer);
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Retry after overflow"));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Older request about compaction."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Older response with findings."));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Recent request 1."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Recent response 1."));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Recent request 2."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Recent response 2."));

        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let mut ctx = AgentRunnerContext {
            task: "Retry after overflow",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-overflow-retry",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_service: Some(&compaction_service),
            config: AgentRunnerConfig::new("mock-model".to_string(), 2, 1, 30, 256),
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
            .any(|message| message.summary_payload().is_some()));
    }

    #[tokio::test]
    async fn preprocess_llm_response_keeps_memory_tokens_separate_from_api_usage() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Inspect token metrics"));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Recent response"));

        let estimated_tokens = session.memory().token_count();
        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let mut ctx = AgentRunnerContext {
            task: "Inspect token metrics",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-token-metrics",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_service: None,
            config: AgentRunnerConfig::new("mock-model".to_string(), 1, 1, 30, 256),
        };
        let mut response = ChatResponse {
            content: Some("done".to_string()),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            reasoning_content: None,
            usage: Some(TokenUsage {
                prompt_tokens: 9_000,
                completion_tokens: 512,
                total_tokens: 9_512,
            }),
        };

        runner
            .preprocess_llm_response(&mut response, &mut ctx)
            .await;

        assert_eq!(ctx.agent.memory().token_count(), estimated_tokens);
        assert_eq!(ctx.agent.memory().api_token_count(), Some(9_512));
    }

    fn build_llm_client(provider: MockLlmProvider) -> Arc<LlmClient> {
        let settings = AgentSettings {
            agent_model_id: Some("mock-model".to_string()),
            agent_model_provider: Some("mock".to_string()),
            agent_model_max_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm_client = LlmClient::new(&settings);
        llm_client.register_provider("mock".to_string(), Arc::new(provider));
        Arc::new(llm_client)
    }

    fn single_final_response_provider() -> MockLlmProvider {
        let mut provider = MockLlmProvider::new();
        provider.expect_chat_with_tools().return_once(|_| {
            Ok(ChatResponse {
                content: Some(r#"{"thought":"done","final_answer":"done"}"#.to_string()),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                reasoning_content: None,
                usage: None,
            })
        });
        provider
            .expect_chat_completion()
            .returning(|_, _, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_transcribe_audio()
            .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_analyze_image()
            .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
    }

    fn tool_then_final_provider() -> MockLlmProvider {
        let mut provider = MockLlmProvider::new();
        let mut sequence = mockall::Sequence::new();
        provider
            .expect_chat_with_tools()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_| {
                Ok(ChatResponse {
                    content: Some(String::new()),
                    tool_calls: vec![ToolCall {
                        id: "call-1".to_string(),
                        function: ToolCallFunction {
                            name: "fake_large_tool".to_string(),
                            arguments: "{}".to_string(),
                        },
                        is_recovered: false,
                    }],
                    finish_reason: "tool_calls".to_string(),
                    reasoning_content: None,
                    usage: None,
                })
            });
        provider
            .expect_chat_with_tools()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_| {
                Ok(ChatResponse {
                    content: Some(r#"{"thought":"done","final_answer":"done"}"#.to_string()),
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    reasoning_content: None,
                    usage: None,
                })
            });
        provider
            .expect_chat_completion()
            .returning(|_, _, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_transcribe_audio()
            .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_analyze_image()
            .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
    }

    fn context_overflow_then_final_provider() -> MockLlmProvider {
        let mut provider = MockLlmProvider::new();
        let mut sequence = mockall::Sequence::new();
        provider
            .expect_chat_with_tools()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_| {
                Err(LlmError::ApiError(
                    "maximum context length exceeded".to_string(),
                ))
            });
        provider
            .expect_chat_with_tools()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_| {
                Ok(ChatResponse {
                    content: Some(r#"{"thought":"done","final_answer":"done"}"#.to_string()),
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    reasoning_content: None,
                    usage: None,
                })
            });
        provider
            .expect_chat_completion()
            .returning(|_, _, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_transcribe_audio()
            .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_analyze_image()
            .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
    }

    fn repeated_cleanup_provider() -> MockLlmProvider {
        let mut provider = MockLlmProvider::new();
        let mut sequence = mockall::Sequence::new();
        for call_id in ["call-1", "call-2"] {
            provider
                .expect_chat_with_tools()
                .times(1)
                .in_sequence(&mut sequence)
                .return_once(move |_| {
                    Ok(ChatResponse {
                        content: Some(String::new()),
                        tool_calls: vec![ToolCall {
                            id: call_id.to_string(),
                            function: ToolCallFunction {
                                name: "fake_large_tool".to_string(),
                                arguments: "{}".to_string(),
                            },
                            is_recovered: false,
                        }],
                        finish_reason: "tool_calls".to_string(),
                        reasoning_content: None,
                        usage: None,
                    })
                });
        }
        provider
            .expect_chat_with_tools()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_| {
                Ok(ChatResponse {
                    content: Some(r#"{"thought":"done","final_answer":"done"}"#.to_string()),
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    reasoning_content: None,
                    usage: None,
                })
            });
        provider
            .expect_chat_completion()
            .returning(|_, _, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_transcribe_audio()
            .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_analyze_image()
            .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
    }

    fn repeated_summary_provider() -> MockLlmProvider {
        let mut provider = MockLlmProvider::new();
        let mut sequence = mockall::Sequence::new();
        for call_id in ["call-1", "call-2", "call-3"] {
            provider
                .expect_chat_with_tools()
                .times(1)
                .in_sequence(&mut sequence)
                .return_once(move |_| {
                    Ok(ChatResponse {
                        content: Some(String::new()),
                        tool_calls: vec![ToolCall {
                            id: call_id.to_string(),
                            function: ToolCallFunction {
                                name: "fake_small_tool".to_string(),
                                arguments: "{}".to_string(),
                            },
                            is_recovered: false,
                        }],
                        finish_reason: "tool_calls".to_string(),
                        reasoning_content: None,
                        usage: None,
                    })
                });
        }
        provider
            .expect_chat_with_tools()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_| {
                Ok(ChatResponse {
                    content: Some(r#"{"thought":"done","final_answer":"done"}"#.to_string()),
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    reasoning_content: None,
                    usage: None,
                })
            });
        provider
            .expect_chat_completion()
            .returning(|_, _, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_transcribe_audio()
            .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_analyze_image()
            .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
    }

    fn base_summary_session(task: &str) -> EphemeralSession {
        let mut session = EphemeralSession::new(256);
        session
            .memory_mut()
            .add_message(AgentMessage::topic_agents_md(
                "# Topic AGENTS\nPreserve operator instructions.",
            ));
        session
            .memory_mut()
            .add_message(AgentMessage::user_task(task));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Older request about compaction."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Older response with findings."));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Recent request 1."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Recent response 1."));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Recent request 2."));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Recent response 2."));
        session
    }

    async fn collect_progress_events(
        progress_rx: &mut tokio::sync::mpsc::Receiver<AgentEvent>,
    ) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Some(event) = progress_rx.recv().await {
            events.push(event);
        }
        events
    }
}
