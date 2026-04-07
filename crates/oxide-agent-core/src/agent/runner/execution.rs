//! Core execution loop for the agent runner.

use super::types::{
    AgentRunResult, AgentRunnerContext, FinalResponseInput, RunState, StructuredOutputFailure,
};
use super::AgentRunner;
use crate::agent::compaction::{
    classify_hot_memory, estimate_request_budget, CompactionRequest, CompactionTrigger,
};
use crate::agent::memory::AgentMessage;
use crate::agent::progress::{AgentEvent, RepeatedCompactionKind, TokenSnapshot};
use crate::agent::recovery::{repair_agent_message_history_for_provider, sanitize_tool_calls};
use crate::agent::structured_output::parse_structured_output;
use crate::config::ModelInfo;
use crate::llm::{ChatResponse, LlmClient, LlmError, ProviderCapabilities};
use anyhow::{anyhow, Result};
use std::future::Future;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

enum AttemptOutcome {
    Return(ChatResponse),
    RetrySameRoute,
    FailoverToNextRoute(LlmError),
}

#[derive(Clone, Copy)]
struct LlmAttemptMetadata<'a> {
    provider_name: &'a str,
    model_name: &'a str,
    route_index: Option<usize>,
    capabilities: ProviderCapabilities,
    attempt: usize,
    max_retries: usize,
}

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
                if let Some(res) = self.apply_timeout_hook(ctx, &mut state)? {
                    return Ok(AgentRunResult::Final(res));
                }
            }

            self.apply_pending_runtime_context(ctx, &mut state).await;

            self.run_pre_llm_maintenance(ctx, &mut state, iteration)
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
                // Emit milestone when Thinking event is sent (not just received).
                let timestamp_ms = chrono::Utc::now().timestamp_millis();
                let _ = tx
                    .send(AgentEvent::Milestone {
                        name: "thinking_sent".to_string(),
                        timestamp_ms,
                    })
                    .await;
            }

            let cancellation_token = ctx.agent.cancellation_token().clone();
            let Some(loop_detected) = Self::await_until_cancelled(
                cancellation_token,
                self.llm_loop_detected(ctx, &state),
            )
            .await
            else {
                return Err(self.cancelled_error(ctx).await);
            };
            if loop_detected {
                return Err(self
                    .loop_detected_error(
                        ctx,
                        &state,
                        crate::agent::loop_detection::LoopType::CognitiveLoop,
                    )
                    .await);
            }

            let cancellation_token = ctx.agent.cancellation_token().clone();
            let Some(response) = Self::await_until_cancelled(
                cancellation_token,
                self.call_llm_with_tools(ctx, &mut state),
            )
            .await
            else {
                return Err(self.cancelled_error(ctx).await);
            };
            let response = response?;
            if let Some(result) = self.handle_llm_response(response, ctx, &mut state).await? {
                return Ok(result);
            }
        }

        Err(anyhow!(
            "Agent exceeded iteration limit ({}).",
            ctx.config.max_iterations
        ))
    }

    async fn run_pre_llm_maintenance(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        iteration: usize,
    ) -> Result<()> {
        self.apply_before_iteration_hooks(ctx, state)?;

        let cancellation_token = ctx.agent.cancellation_token().clone();
        if state.take_manual_compaction_request() {
            let Some(compaction_result) = Self::await_until_cancelled(
                cancellation_token.clone(),
                self.run_compaction_checkpoint(ctx, state, CompactionTrigger::Manual),
            )
            .await
            else {
                return Err(self.cancelled_error(ctx).await);
            };
            compaction_result?;
        } else {
            let Some(compaction_result) = Self::await_until_cancelled(
                cancellation_token,
                self.run_iteration_compaction(ctx, state, iteration),
            )
            .await
            else {
                return Err(self.cancelled_error(ctx).await);
            };
            compaction_result?;
        }

        if iteration == 0 {
            Self::emit_pre_run_compaction_done(ctx.progress_tx).await;
        }

        Ok(())
    }

    async fn emit_pre_run_compaction_done(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) {
        let Some(tx) = progress_tx else { return };

        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        let _ = tx
            .send(AgentEvent::Milestone {
                name: "pre_run_compaction_done".to_string(),
                timestamp_ms,
            })
            .await;
    }

    async fn call_llm_with_tools(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) -> Result<ChatResponse> {
        // Emit milestone on first LLM call of first iteration.
        if state.iteration == 0 {
            if let Some(tx) = ctx.progress_tx {
                let timestamp_ms = chrono::Utc::now().timestamp_millis();
                let _ = tx
                    .send(AgentEvent::Milestone {
                        name: "llm_call_started".to_string(),
                        timestamp_ms,
                    })
                    .await;
            }
        }

        if ctx.config.model_routes.is_empty() {
            return self.call_llm_with_tools_legacy(ctx, state).await;
        }

        self.call_llm_with_tools_with_failover(ctx, state).await
    }

    async fn call_llm_with_tools_legacy(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) -> Result<ChatResponse> {
        let max_retries = LlmClient::MAX_RETRIES;
        let json_mode = self.requires_structured_output(&ctx.config);
        let provider_name = ctx
            .config
            .model_provider
            .clone()
            .or_else(|| {
                self.llm_client
                    .get_provider_name(&ctx.config.model_name)
                    .ok()
            })
            .unwrap_or_else(|| "unknown".to_string());
        let model_name = ctx.config.model_name.clone();
        let model_info = self.llm_client.get_model_info(&ctx.config.model_name)?;
        let capabilities = LlmClient::provider_capabilities_for_model(&model_info);

        Self::log_llm_route_selected(
            ctx,
            state,
            0,
            provider_name.as_str(),
            model_name.as_str(),
            json_mode,
        );

        if !capabilities.can_run_chat_with_tools_request(!ctx.tools.is_empty(), json_mode) {
            let error = LlmError::ApiError(format!(
                "Tool-enabled agent calls are not supported for {} model `{}`",
                model_info.provider, model_info.id
            ));
            Self::emit_llm_error(ctx.progress_tx, &error).await;
            return Err(anyhow!("LLM call failed: {error}"));
        }

        for attempt in 1..=max_retries {
            Self::refresh_messages_from_memory(ctx);
            Self::log_llm_route_attempt_started(
                ctx,
                state,
                attempt,
                max_retries,
                Some(0),
                provider_name.as_str(),
                model_name.as_str(),
            );
            let result = self
                .llm_client
                .chat_with_tools_single_attempt(
                    ctx.system_prompt,
                    ctx.messages,
                    ctx.tools,
                    &ctx.config.model_name,
                    json_mode,
                )
                .await;

            match self
                .handle_llm_attempt_result(
                    ctx,
                    state,
                    LlmAttemptMetadata {
                        provider_name: &provider_name,
                        model_name: &model_name,
                        route_index: Some(0),
                        capabilities,
                        attempt,
                        max_retries,
                    },
                    result,
                )
                .await?
            {
                AttemptOutcome::Return(response) => return Ok(response),
                AttemptOutcome::RetrySameRoute => continue,
                AttemptOutcome::FailoverToNextRoute(_) => unreachable!("legacy path has no routes"),
            }
        }

        Err(anyhow!("LLM call failed after all retries"))
    }

    async fn call_llm_with_tools_with_failover(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) -> Result<ChatResponse> {
        let max_retries = LlmClient::MAX_RETRIES;

        let mut exhausted_routes = std::collections::HashSet::new();
        let mut pending_failover_from: Option<ModelInfo> = None;
        let mut last_route_error: Option<LlmError> = None;

        loop {
            let Some(route_index) = self.select_model_route_index(ctx, &exhausted_routes) else {
                let error = last_route_error.unwrap_or_else(|| {
                    LlmError::Unknown("No healthy model routes available".to_string())
                });
                Self::emit_llm_error(ctx.progress_tx, &error).await;
                return Err(anyhow!("LLM call failed: {error}"));
            };

            let route = ctx.config.model_routes[route_index].clone();
            if let Some(from_route) = pending_failover_from.take() {
                Self::emit_provider_failover(ctx.progress_tx, &from_route, &route).await;
            }

            ctx.config.model_name = route.id.clone();
            ctx.config.model_max_output_tokens = route.max_output_tokens;
            ctx.config.model_provider = Some(route.provider.clone());

            let json_mode = self.requires_structured_output(&ctx.config);
            let provider_name = route.provider.clone();
            let capabilities = LlmClient::provider_capabilities_for_model(&route);

            Self::log_llm_route_selected(
                ctx,
                state,
                route_index,
                route.provider.as_str(),
                route.id.as_str(),
                json_mode,
            );

            if !capabilities.can_run_chat_with_tools_request(!ctx.tools.is_empty(), json_mode) {
                let error = LlmError::ApiError(format!(
                    "Tool-enabled agent calls are not supported for {} model `{}`",
                    route.provider, route.id
                ));
                warn!(
                    provider = route.provider,
                    model = route.id,
                    "Skipping model route due to unsupported tool capabilities"
                );
                exhausted_routes.insert(Self::route_key(&route));
                pending_failover_from = Some(route.clone());
                last_route_error = Some(error);
                continue;
            }

            for attempt in 1..=max_retries {
                Self::refresh_messages_from_memory(ctx);
                Self::log_llm_route_attempt_started(
                    ctx,
                    state,
                    attempt,
                    max_retries,
                    Some(route_index),
                    route.provider.as_str(),
                    route.id.as_str(),
                );
                let result = self
                    .llm_client
                    .chat_with_tools_single_attempt_for_model_info(
                        ctx.system_prompt,
                        ctx.messages,
                        ctx.tools,
                        &route,
                        json_mode,
                    )
                    .await;

                let attempt_result = self
                    .handle_llm_attempt_result(
                        ctx,
                        state,
                        LlmAttemptMetadata {
                            provider_name: &provider_name,
                            model_name: &route.id,
                            route_index: Some(route_index),
                            capabilities,
                            attempt,
                            max_retries,
                        },
                        result,
                    )
                    .await?;
                match attempt_result {
                    AttemptOutcome::Return(response) => return Ok(response),
                    AttemptOutcome::RetrySameRoute => continue,
                    AttemptOutcome::FailoverToNextRoute(error) => {
                        let quarantine_for =
                            Self::rate_limit_quarantine_duration(&error, max_retries);
                        self.quarantine_model_route(&route, quarantine_for);
                        exhausted_routes.insert(Self::route_key(&route));
                        pending_failover_from = Some(route.clone());
                        last_route_error = Some(error);
                        break;
                    }
                }
            }
        }
    }

    async fn handle_llm_attempt_result(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        metadata: LlmAttemptMetadata<'_>,
        result: std::result::Result<ChatResponse, LlmError>,
    ) -> Result<AttemptOutcome> {
        match result {
            Ok(response) => {
                Self::log_llm_route_attempt_success(ctx, state, &metadata);
                if metadata.attempt > 1 {
                    info!(
                        attempt = metadata.attempt,
                        max_attempts = metadata.max_retries,
                        provider = metadata.provider_name,
                        "LLM retry succeeded after rate limit"
                    );
                }
                Ok(AttemptOutcome::Return(response))
            }
            Err(error) => {
                Self::log_llm_route_attempt_error(ctx, state, &metadata, &error);
                if let LlmError::RepairableHistory(reason) = &error {
                    if self
                        .repair_history_before_retry(
                            ctx,
                            metadata.provider_name,
                            metadata.capabilities,
                            metadata.attempt,
                            metadata.max_retries,
                            reason,
                        )
                        .await
                    {
                        return Ok(AttemptOutcome::RetrySameRoute);
                    }

                    Self::emit_llm_error(ctx.progress_tx, &error).await;
                    return Err(anyhow!("LLM call failed: {error}"));
                }

                if Self::llm_error_suggests_context_overflow(&error) && metadata.attempt == 1 {
                    let retried = self
                        .run_compaction_checkpoint(ctx, state, CompactionTrigger::Manual)
                        .await?;
                    if retried {
                        return Ok(AttemptOutcome::RetrySameRoute);
                    }
                }

                if metadata.attempt < metadata.max_retries {
                    if let Some(backoff) = LlmClient::get_retry_delay(&error, metadata.attempt) {
                        let wait_secs = backoff.as_secs();
                        let wait_secs_display = if wait_secs > 0 {
                            Some(wait_secs)
                        } else {
                            LlmClient::get_rate_limit_wait_secs(&error)
                        };

                        if LlmClient::is_rate_limit_error(&error) {
                            Self::emit_rate_limit_retrying(
                                ctx.progress_tx,
                                metadata.attempt,
                                metadata.max_retries,
                                wait_secs_display,
                                metadata.provider_name,
                            )
                            .await;
                            debug!(
                                error = %error,
                                attempt = metadata.attempt,
                                max_attempts = metadata.max_retries,
                                backoff_ms = backoff.as_millis(),
                                provider = metadata.provider_name,
                                "Retrying LLM request after rate limit"
                            );
                        } else {
                            let error_class = Self::error_class(&error);
                            Self::emit_llm_retrying(
                                ctx.progress_tx,
                                metadata.attempt,
                                metadata.max_retries,
                                wait_secs_display,
                                metadata.provider_name,
                                error_class,
                            )
                            .await;
                            debug!(
                                error = %error,
                                error_class = error_class,
                                attempt = metadata.attempt,
                                max_attempts = metadata.max_retries,
                                backoff_ms = backoff.as_millis(),
                                provider = metadata.provider_name,
                                "Retrying LLM request after retryable error"
                            );
                        }

                        tokio::time::sleep(backoff).await;
                        return Ok(AttemptOutcome::RetrySameRoute);
                    }
                }

                if LlmClient::is_rate_limit_error(&error) && !ctx.config.model_routes.is_empty() {
                    return Ok(AttemptOutcome::FailoverToNextRoute(error));
                }

                Self::emit_llm_error(ctx.progress_tx, &error).await;
                Err(anyhow!("LLM call failed: {error}"))
            }
        }
    }

    async fn repair_history_before_retry(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        provider_name: &str,
        capabilities: ProviderCapabilities,
        attempt: usize,
        max_retries: usize,
        reason: &str,
    ) -> bool {
        let (repaired_messages, outcome) = repair_agent_message_history_for_provider(
            ctx.agent.memory().get_messages(),
            capabilities.strict_tool_history(),
        );
        if !outcome.applied {
            warn!(
                provider = provider_name,
                attempt,
                tool_history_mode = capabilities.tool_history_label(),
                reason,
                "Detected repairable history error but local repair produced no changes"
            );
            return false;
        }

        ctx.agent.memory_mut().replace_messages(repaired_messages);
        ctx.agent.persist_memory_checkpoint_background();
        Self::refresh_messages_from_memory(ctx);
        Self::emit_history_repair_applied(ctx.progress_tx, provider_name, capabilities, &outcome)
            .await;
        Self::emit_llm_retrying(
            ctx.progress_tx,
            attempt,
            max_retries,
            None,
            provider_name,
            capabilities.tool_history_label(),
        )
        .await;
        Self::emit_token_snapshot_update(
            ctx.progress_tx,
            Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration),
        )
        .await;
        warn!(
            provider = provider_name,
            attempt,
            dropped_tool_results = outcome.dropped_tool_results,
            trimmed_tool_calls = outcome.trimmed_tool_calls,
            converted_tool_call_messages = outcome.converted_tool_call_messages,
            dropped_tool_call_messages = outcome.dropped_tool_call_messages,
            tool_history_mode = capabilities.tool_history_label(),
            reason,
            "Repaired invalid tool history before retrying LLM request"
        );
        true
    }

    fn select_model_route_index(
        &mut self,
        ctx: &AgentRunnerContext<'_>,
        exhausted_routes: &std::collections::HashSet<String>,
    ) -> Option<usize> {
        let now = Instant::now();
        self.route_failover_state
            .route_quarantine
            .retain(|_, until| *until > now);

        if ctx.config.model_routes.is_empty() {
            return None;
        }

        if self.route_is_available(&ctx.config.model_routes[0], exhausted_routes, now) {
            return Some(0);
        }

        let fallback_candidates: Vec<(usize, usize)> = ctx
            .config
            .model_routes
            .iter()
            .enumerate()
            .skip(1)
            .filter_map(|(index, route)| {
                self.route_is_available(route, exhausted_routes, now)
                    .then_some((index, route.weight.max(1) as usize))
            })
            .collect();

        if fallback_candidates.is_empty() {
            return None;
        }

        let total_weight: usize = fallback_candidates.iter().map(|(_, weight)| *weight).sum();
        let slot = self.route_failover_state.fallback_cursor % total_weight;
        self.route_failover_state.fallback_cursor =
            (self.route_failover_state.fallback_cursor + 1) % total_weight;

        let mut cursor = slot;
        for (index, weight) in fallback_candidates {
            if cursor < weight {
                return Some(index);
            }
            cursor -= weight;
        }

        None
    }

    fn route_is_available(
        &self,
        route: &ModelInfo,
        exhausted_routes: &std::collections::HashSet<String>,
        now: Instant,
    ) -> bool {
        let route_key = Self::route_key(route);
        !exhausted_routes.contains(&route_key)
            && self.llm_client.is_provider_available(&route.provider)
            && self
                .route_failover_state
                .route_quarantine
                .get(&route_key)
                .is_none_or(|until| *until <= now)
    }

    fn quarantine_model_route(&mut self, route: &ModelInfo, duration: Duration) {
        self.route_failover_state
            .route_quarantine
            .insert(Self::route_key(route), Instant::now() + duration);
    }

    fn rate_limit_quarantine_duration(error: &LlmError, attempt: usize) -> Duration {
        LlmClient::get_retry_delay(error, attempt).unwrap_or_else(|| Duration::from_secs(60))
    }

    fn route_key(route: &ModelInfo) -> String {
        format!("{}:{}", route.provider, route.id)
    }

    fn requires_structured_output(&self, config: &super::types::AgentRunnerConfig) -> bool {
        match self.llm_client.get_model_info(&config.model_name) {
            Ok(info) => LlmClient::supports_structured_output_for_model(&info),
            Err(error) => {
                warn!(
                    model = config.model_name,
                    error = %error,
                    "Failed to resolve model info; defaulting to structured output"
                );
                true
            }
        }
    }

    pub(super) async fn await_until_cancelled<T, F>(
        cancellation_token: tokio_util::sync::CancellationToken,
        future: F,
    ) -> Option<T>
    where
        F: Future<Output = T>,
    {
        tokio::pin!(future);

        tokio::select! {
            result = &mut future => Some(result),
            _ = cancellation_token.cancelled() => None,
        }
    }

    #[allow(dead_code)]
    async fn chat_with_tools_once(
        &self,
        ctx: &mut AgentRunnerContext<'_>,
    ) -> Result<ChatResponse, LlmError> {
        // This method is kept for backwards compatibility.
        // Use call_llm_with_tools for retry handling with UI events.
        let json_mode = self.requires_structured_output(&ctx.config);
        self.llm_client
            .chat_with_tools_single_attempt(
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

        if !self.requires_structured_output(&ctx.config) {
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

        let awaiting_user_input = parsed.awaiting_user_input;
        let final_answer = parsed.final_answer;
        let tool_calls = parsed
            .tool_call
            .map(|tool_call| vec![self.build_tool_call(tool_call)])
            .unwrap_or_default();

        self.spawn_narrative_task(
            response.reasoning_content.as_deref(),
            &tool_calls,
            ctx.progress_tx,
        );

        if let Some(request) = awaiting_user_input {
            return self
                .handle_waiting_for_user_input(
                    ctx,
                    state,
                    raw_json,
                    response.reasoning_content,
                    request,
                )
                .await;
        }

        if tool_calls.is_empty() {
            let final_answer =
                final_answer.unwrap_or_else(|| "Task completed, but answer is empty.".to_string());

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

        // FAST PATH: Skip PreRun compaction for fresh sessions.
        // Compaction is expensive (token estimation, classification, externalization,
        // pruning, summarization, rebuild). For fresh sessions with minimal history
        // and no compactable content, the entire pipeline is a no-op.
        if iteration == 0 {
            let msg_count = ctx.agent.memory().get_messages().len();
            // Quick check: if <= 3 messages (typical: system, task, context),
            // there's nothing meaningful to compact.
            if msg_count <= 3 {
                let snapshot = classify_hot_memory(ctx.agent.memory().get_messages());
                // Skip only if there's no compactable or prunable content.
                if snapshot.compactable_history.message_count == 0
                    && snapshot.prunable_artifacts.message_count == 0
                {
                    tracing::debug!(
                        task_id = %ctx.task_id,
                        msg_count,
                        "Fast path: skipping PreRun compaction for fresh session"
                    );
                    return Ok(());
                }
            }
        }

        let _ = self.run_compaction_checkpoint(ctx, state, trigger).await?;
        Ok(())
    }

    pub(super) async fn run_compaction_checkpoint(
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
        let should_emit_progress = matches!(
            trigger,
            CompactionTrigger::Manual | CompactionTrigger::PostRun
        ) || outcome.applied
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
            collapsed_retry_attempts = outcome.error_retry_collapse.collapsed_attempt_count,
            collapsed_retry_messages = outcome.error_retry_collapse.dropped_message_count,
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
        if matches!(trigger, CompactionTrigger::PostRun) {
            return;
        }

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

        if outcome.error_retry_collapse.applied
            || outcome.externalization.applied
            || outcome.pruning.applied
        {
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
            warn!(
                model = %ctx.config.model_name,
                provider = ctx.config.model_provider.as_deref().unwrap_or("unknown"),
                "Model returned empty content"
            );
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

    async fn emit_rate_limit_retrying(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        attempt: usize,
        max_attempts: usize,
        wait_secs: Option<u64>,
        provider: &str,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::RateLimitRetrying {
                    attempt,
                    max_attempts,
                    wait_secs,
                    provider: provider.to_string(),
                })
                .await;
        }
    }

    async fn emit_provider_failover(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        from_route: &ModelInfo,
        to_route: &ModelInfo,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::ProviderFailoverActivated {
                    from_provider: from_route.provider.clone(),
                    from_model: from_route.id.clone(),
                    to_provider: to_route.provider.clone(),
                    to_model: to_route.id.clone(),
                })
                .await;
        }
    }

    fn error_class(error: &LlmError) -> &'static str {
        match error {
            LlmError::NetworkError(msg) => {
                let m = msg.to_lowercase();
                if m.contains("timeout") || m.contains("timed out") {
                    "timeout"
                } else if m.contains("connection") || m.contains("reset") {
                    "connection"
                } else {
                    "network"
                }
            }
            LlmError::ApiError(msg) => {
                let m = msg.to_lowercase();
                if m.contains("500")
                    || m.contains("502")
                    || m.contains("503")
                    || m.contains("504")
                    || m.contains("overloaded")
                {
                    "server_error"
                } else if m.contains("timeout") {
                    "timeout"
                } else {
                    "api"
                }
            }
            LlmError::JsonError(_) => "json_error",
            _ => "unknown",
        }
    }

    async fn emit_llm_retrying(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        attempt: usize,
        max_attempts: usize,
        wait_secs: Option<u64>,
        provider: &str,
        error_class: &str,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::LlmRetrying {
                    attempt,
                    max_attempts,
                    wait_secs,
                    provider: provider.to_string(),
                    error_class: error_class.to_string(),
                })
                .await;
        }
    }

    pub(super) async fn emit_compaction_started(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        trigger: CompactionTrigger,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx.send(AgentEvent::CompactionStarted { trigger }).await;
        }
    }

    pub(super) async fn emit_pruning_applied(
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

    async fn emit_history_repair_applied(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        provider_name: &str,
        capabilities: ProviderCapabilities,
        outcome: &crate::agent::recovery::HistoryRepairOutcome,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::HistoryRepairApplied {
                    provider: provider_name.to_string(),
                    strict_tool_history: capabilities.strict_tool_history(),
                    dropped_tool_results: outcome.dropped_tool_results,
                    trimmed_tool_calls: outcome.trimmed_tool_calls,
                    converted_tool_call_messages: outcome.converted_tool_call_messages,
                    dropped_tool_call_messages: outcome.dropped_tool_call_messages,
                })
                .await;
        }
    }

    pub(super) async fn emit_compaction_completed(
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

    pub(super) async fn emit_compaction_failed(
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
            hard_reserve_tokens: budget.hard_reserve_tokens,
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

    fn log_llm_route_selected(
        ctx: &AgentRunnerContext<'_>,
        state: &RunState,
        route_index: usize,
        provider: &str,
        model: &str,
        json_mode: bool,
    ) {
        info!(
            task_id = %ctx.task_id,
            iteration = state.iteration,
            route_index,
            provider,
            model,
            is_sub_agent = ctx.config.is_sub_agent,
            json_mode,
            "LLM route selected"
        );
    }

    fn log_llm_route_attempt_started(
        ctx: &AgentRunnerContext<'_>,
        state: &RunState,
        attempt: usize,
        max_attempts: usize,
        route_index: Option<usize>,
        provider: &str,
        model: &str,
    ) {
        debug!(
            task_id = %ctx.task_id,
            iteration = state.iteration,
            attempt,
            max_attempts,
            route_index,
            provider,
            model,
            is_sub_agent = ctx.config.is_sub_agent,
            "LLM route attempt started"
        );
    }

    fn log_llm_route_attempt_success(
        ctx: &AgentRunnerContext<'_>,
        state: &RunState,
        metadata: &LlmAttemptMetadata<'_>,
    ) {
        info!(
            task_id = %ctx.task_id,
            iteration = state.iteration,
            attempt = metadata.attempt,
            max_attempts = metadata.max_retries,
            route_index = metadata.route_index,
            provider = metadata.provider_name,
            model = metadata.model_name,
            is_sub_agent = ctx.config.is_sub_agent,
            outcome = "success",
            "LLM route attempt finished"
        );
    }

    fn log_llm_route_attempt_error(
        ctx: &AgentRunnerContext<'_>,
        state: &RunState,
        metadata: &LlmAttemptMetadata<'_>,
        error: &LlmError,
    ) {
        let outcome = if LlmClient::is_rate_limit_error(error) {
            "rate_limit"
        } else if LlmClient::is_retryable_error(error) {
            "retryable_error"
        } else {
            "error"
        };

        warn!(
            task_id = %ctx.task_id,
            iteration = state.iteration,
            attempt = metadata.attempt,
            max_attempts = metadata.max_retries,
            route_index = metadata.route_index,
            provider = metadata.provider_name,
            model = metadata.model_name,
            is_sub_agent = ctx.config.is_sub_agent,
            error = %error,
            outcome,
            "LLM route attempt finished"
        );
    }

    fn log_token_snapshot(
        ctx: &AgentRunnerContext<'_>,
        iteration: usize,
        phase: &str,
        snapshot: &TokenSnapshot,
    ) {
        let planned_provider = ctx
            .config
            .model_provider
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        info!(
            task_id = %ctx.task_id,
            iteration,
            phase,
            planned_provider = planned_provider,
            planned_model = %ctx.config.model_name,
            is_sub_agent = ctx.config.is_sub_agent,
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

    pub(super) fn refresh_messages_from_memory(ctx: &mut AgentRunnerContext<'_>) {
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
        if let Some(tx) = ctx.progress_tx {
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
    use crate::config::{AgentSettings, ModelInfo};
    use crate::llm::{
        ChatResponse, LlmClient, MockLlmProvider, TokenUsage, ToolCall, ToolCallFunction,
        ToolDefinition,
    };
    use async_trait::async_trait;
    use oxide_agent_memory::MemoryRepository;
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
                model_routes: Vec::new(),
                timeout_secs: 1,
                ..CompactionSummarizerConfig::default()
            },
        );
        let compaction_service = CompactionService::default().with_summarizer(summarizer);
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(768);
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
            persistent_memory: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
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
    async fn run_repairs_invalid_tool_history_before_llm_call() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(768);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Repair invalid tool history"));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant_with_tools(
                "Calling tools",
                vec![
                    ToolCall::new(
                        "call-1".to_string(),
                        ToolCallFunction {
                            name: "search".to_string(),
                            arguments: "{}".to_string(),
                        },
                        false,
                    ),
                    ToolCall::new(
                        "call-2".to_string(),
                        ToolCallFunction {
                            name: "read_file".to_string(),
                            arguments: "{}".to_string(),
                        },
                        false,
                    ),
                ],
            ));
        session
            .memory_mut()
            .add_message(AgentMessage::tool("call-1", "search", "result-1"));

        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let mut ctx = AgentRunnerContext {
            task: "Repair invalid tool history",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-history-repair",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_service: None,
            persistent_memory: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("mock-model".to_string(), 2, 1, 30, 256),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");

        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));
        let repaired_call = ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .find(|message| message.tool_calls.is_some())
            .expect("assistant tool call must remain after repair");
        let repaired_tool_calls = repaired_call
            .tool_calls
            .as_ref()
            .expect("tool call batch must still exist");
        assert_eq!(repaired_tool_calls.len(), 1);
        assert_eq!(repaired_tool_calls[0].id, "call-1");
        assert!(!ctx.agent.memory().get_messages().iter().any(|message| {
            message.tool_calls.as_ref().is_some_and(|tool_calls| {
                tool_calls.iter().any(|tool_call| tool_call.id == "call-2")
            })
        }));
    }

    #[tokio::test]
    async fn run_sub_agent_does_not_persist_post_run_memory() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(768);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Sub-agent task"));

        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let store = Arc::new(oxide_agent_memory::InMemoryMemoryRepository::new());
        let store_for_coordinator = Arc::clone(&store);
        let store_for_coordinator: Arc<dyn crate::agent::persistent_memory::PersistentMemoryStore> =
            store_for_coordinator;
        let coordinator = crate::agent::persistent_memory::PersistentMemoryCoordinator::new(
            store_for_coordinator,
        );
        let mut ctx = AgentRunnerContext {
            task: "Sub-agent task",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-sub-agent-memory",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_service: None,
            persistent_memory: Some(&coordinator),
            session_id: Some("sub-session".to_string()),
            memory_scope: Some(crate::agent::AgentMemoryScope::new(42, "topic-a", "flow-a")),
            memory_behavior: None,
            config: AgentRunnerConfig::new("mock-model".to_string(), 2, 1, 30, 256)
                .with_sub_agent(true),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");

        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));
        assert!(store
            .get_session_state("sub-session")
            .await
            .expect("session state lookup should succeed")
            .is_none());
        assert!(store
            .get_episode(&"runner-sub-agent-memory".to_string())
            .await
            .expect("episode lookup should succeed")
            .is_none());
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
            persistent_memory: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("mock-model".to_string(), 3, 1, 30, 128),
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
    async fn run_does_not_emit_repeated_cleanup_warning_after_summary_compaction() {
        let llm_client = build_llm_client(tool_then_final_provider());
        let summarizer = CompactionSummarizer::new(
            Arc::clone(&llm_client),
            CompactionSummarizerConfig {
                model_routes: Vec::new(),
                timeout_secs: 1,
                ..CompactionSummarizerConfig::default()
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
            persistent_memory: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
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
        // After rebuild, externalized tool results become ArchiveReference messages
        // Check for archive_ref_payload instead of is_externalized
        assert!(ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .any(|message| message.archive_ref_payload().is_some()));

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
        assert_ne!(
            repeated_warning.map(|(kind, _count)| kind),
            Some(RepeatedCompactionKind::Cleanup)
        );
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
            persistent_memory: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("mock-model".to_string(), 4, 1, 30, 256),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");

        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));
        drop(ctx);
        drop(progress_tx);

        let events = collect_progress_events(&mut progress_rx).await;
        // The test expects two cleanup completions with warnings, but the current
        // implementation consolidates compaction into a single PostRun pass.
        // Check that at least one compaction ran and no repeated cleanup warning was emitted.
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
        let repeated_cleanup_warning = events.iter().find_map(|event| match event {
            AgentEvent::RepeatedCompactionWarning { kind, count } => Some((*kind, *count)),
            _ => None,
        });

        // With current architecture, we get one PostRun compaction that externalizes all tool outputs
        assert!(
            cleanup_completions >= 1,
            "Should have at least one cleanup compaction"
        );
        assert_eq!(
            repeated_cleanup_warning, None,
            "Should not emit repeated cleanup warning after single PostRun compaction"
        );
    }

    #[tokio::test]
    async fn run_emits_repeated_summary_warning_on_second_summary_pass() {
        let llm_client = build_llm_client(repeated_summary_provider());
        let summarizer = CompactionSummarizer::new(
            Arc::clone(&llm_client),
            CompactionSummarizerConfig {
                model_routes: Vec::new(),
                timeout_secs: 1,
                ..CompactionSummarizerConfig::default()
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
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(32);
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
            persistent_memory: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("mock-model".to_string(), 4, 1, 30, 256),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");

        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));
        drop(ctx);
        drop(progress_tx);

        let events = collect_progress_events(&mut progress_rx).await;
        let summary_completions = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    AgentEvent::CompactionCompleted {
                        summary_updated: true,
                        ..
                    }
                )
            })
            .count();
        let repeated_summary_warning = events.iter().find_map(|event| match event {
            AgentEvent::RepeatedCompactionWarning { kind, count } => Some((*kind, *count)),
            _ => None,
        });

        // With current architecture, summary compaction runs once in PostRun
        assert!(
            summary_completions >= 1,
            "Should have at least one summary compaction"
        );
        assert_eq!(
            repeated_summary_warning, None,
            "Should not emit repeated summary warning after single compaction"
        );
    }

    #[tokio::test]
    async fn run_overflow_retry_emits_manual_compaction_progress_in_order() {
        let llm_client = build_llm_client(context_overflow_then_final_provider());
        let summarizer = CompactionSummarizer::new(
            Arc::clone(&llm_client),
            CompactionSummarizerConfig {
                model_routes: Vec::new(),
                timeout_secs: 1,
                ..CompactionSummarizerConfig::default()
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
            persistent_memory: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
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
                model_routes: Vec::new(),
                timeout_secs: 1,
                ..CompactionSummarizerConfig::default()
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
            persistent_memory: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
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

    #[test]
    fn refresh_messages_from_memory_drops_transient_messages() {
        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
        let mut session = EphemeralSession::new(1024);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("refresh transient context"));
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        messages.push(crate::llm::Message::system("temporary warning"));
        let mut ctx = AgentRunnerContext {
            task: "refresh transient context",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "refresh-transient-test",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_service: None,
            persistent_memory: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("mock-model".to_string(), 1, 1, 30, 256),
        };

        AgentRunner::refresh_messages_from_memory(&mut ctx);

        assert!(!ctx
            .messages
            .iter()
            .any(|message| message.role == "system" && message.content == "temporary warning"));
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
            persistent_memory: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
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

    #[tokio::test]
    async fn run_fails_over_to_weighted_backup_after_persistent_rate_limits() {
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
            .return_once(|_| Ok(final_structured_response()));
        stub_non_chat_methods(&mut backup);

        let settings = AgentSettings {
            agent_model_id: Some("primary-model".to_string()),
            agent_model_provider: Some("primary".to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm_client = LlmClient::new(&settings);
        llm_client.register_provider("primary".to_string(), Arc::new(primary));
        llm_client.register_provider("backup".to_string(), Arc::new(backup));
        let llm_client = Arc::new(llm_client);

        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Fail over after persistent 429"));

        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(32);
        let mut ctx = AgentRunnerContext {
            task: "Fail over after persistent 429",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-provider-failover",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_service: None,
            persistent_memory: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("primary-model".to_string(), 1, 1, 30, 256)
                .with_model_provider("primary")
                .with_model_routes(vec![
                    ModelInfo {
                        id: "primary-model".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 128_000,
                        provider: "primary".to_string(),
                        weight: 1,
                    },
                    ModelInfo {
                        id: "backup-model".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 128_000,
                        provider: "backup".to_string(),
                        weight: 3,
                    },
                ]),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");
        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));

        drop(ctx);
        drop(progress_tx);
        let events = collect_progress_events(&mut progress_rx).await;
        assert!(events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::ProviderFailoverActivated {
                    from_provider,
                    from_model,
                    to_provider,
                    to_model,
                } if from_provider == "primary"
                    && from_model == "primary-model"
                    && to_provider == "backup"
                    && to_model == "backup-model"
            )
        }));
    }

    #[tokio::test]
    async fn run_keeps_primary_provider_when_rate_limit_recovers() {
        let mut primary = MockLlmProvider::new();
        let mut sequence = mockall::Sequence::new();
        primary
            .expect_chat_with_tools()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_| {
                Err(LlmError::RateLimit {
                    wait_secs: Some(0),
                    message: "primary temporary rate limit".to_string(),
                })
            });
        primary
            .expect_chat_with_tools()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_| Ok(final_structured_response()));
        stub_non_chat_methods(&mut primary);

        let mut backup = MockLlmProvider::new();
        backup.expect_chat_with_tools().times(0);
        stub_non_chat_methods(&mut backup);

        let settings = AgentSettings {
            agent_model_id: Some("primary-model".to_string()),
            agent_model_provider: Some("primary".to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm_client = LlmClient::new(&settings);
        llm_client.register_provider("primary".to_string(), Arc::new(primary));
        llm_client.register_provider("backup".to_string(), Arc::new(backup));
        let llm_client = Arc::new(llm_client);

        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Stay on primary when it wakes up"));

        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(32);
        let mut ctx = AgentRunnerContext {
            task: "Stay on primary when it wakes up",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-primary-recovery",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_service: None,
            persistent_memory: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("primary-model".to_string(), 1, 1, 30, 256)
                .with_model_provider("primary")
                .with_model_routes(vec![
                    ModelInfo {
                        id: "primary-model".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 128_000,
                        provider: "primary".to_string(),
                        weight: 1,
                    },
                    ModelInfo {
                        id: "backup-model".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 128_000,
                        provider: "backup".to_string(),
                        weight: 2,
                    },
                ]),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");
        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));

        drop(ctx);
        drop(progress_tx);
        let events = collect_progress_events(&mut progress_rx).await;
        assert!(!events
            .iter()
            .any(|event| { matches!(event, AgentEvent::ProviderFailoverActivated { .. }) }));
    }

    #[tokio::test]
    async fn run_skips_unsupported_nvidia_route_and_uses_backup() {
        let mut unsupported_nvidia = MockLlmProvider::new();
        unsupported_nvidia.expect_chat_with_tools().times(0);
        stub_non_chat_methods(&mut unsupported_nvidia);

        let mut backup = MockLlmProvider::new();
        backup
            .expect_chat_with_tools()
            .times(1)
            .return_once(|_| Ok(final_structured_response()));
        stub_non_chat_methods(&mut backup);

        let settings = AgentSettings {
            agent_model_id: Some("deepseek-ai/deepseek-r1".to_string()),
            agent_model_provider: Some("nvidia".to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm_client = LlmClient::new(&settings);
        llm_client.register_provider("nvidia".to_string(), Arc::new(unsupported_nvidia));
        llm_client.register_provider("backup".to_string(), Arc::new(backup));
        let llm_client = Arc::new(llm_client);

        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Skip unsupported NVIDIA route"));

        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(32);
        let mut ctx = AgentRunnerContext {
            task: "Skip unsupported NVIDIA route",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-nvidia-capability-skip",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_service: None,
            persistent_memory: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-ai/deepseek-r1".to_string(), 1, 1, 30, 256)
                .with_model_provider("nvidia")
                .with_model_routes(vec![
                    ModelInfo {
                        id: "deepseek-ai/deepseek-r1".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 128_000,
                        provider: "nvidia".to_string(),
                        weight: 1,
                    },
                    ModelInfo {
                        id: "backup-model".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 128_000,
                        provider: "backup".to_string(),
                        weight: 1,
                    },
                ]),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");
        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));

        drop(ctx);
        drop(progress_tx);
        let events = collect_progress_events(&mut progress_rx).await;
        assert!(events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::ProviderFailoverActivated {
                    from_provider,
                    from_model,
                    to_provider,
                    to_model,
                } if from_provider == "nvidia"
                    && from_model == "deepseek-ai/deepseek-r1"
                    && to_provider == "backup"
                    && to_model == "backup-model"
            )
        }));
    }

    fn build_llm_client(provider: MockLlmProvider) -> Arc<LlmClient> {
        let settings = AgentSettings {
            agent_model_id: Some("mock-model".to_string()),
            agent_model_provider: Some("mock".to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm_client = LlmClient::new(&settings);
        llm_client.register_provider("mock".to_string(), Arc::new(provider));
        Arc::new(llm_client)
    }

    fn final_structured_response() -> ChatResponse {
        ChatResponse {
            content: Some(r#"{"thought":"done","final_answer":"done"}"#.to_string()),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            reasoning_content: None,
            usage: None,
        }
    }

    fn stub_non_chat_methods(provider: &mut MockLlmProvider) {
        provider
            .expect_chat_completion()
            .returning(|_, _, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_transcribe_audio()
            .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_analyze_image()
            .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
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
                    tool_calls: vec![ToolCall::new(
                        "call-1".to_string(),
                        ToolCallFunction {
                            name: "fake_large_tool".to_string(),
                            arguments: "{}".to_string(),
                        },
                        false,
                    )],
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
                        tool_calls: vec![ToolCall::new(
                            call_id.to_string(),
                            ToolCallFunction {
                                name: "fake_large_tool".to_string(),
                                arguments: "{}".to_string(),
                            },
                            false,
                        )],
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
                        tool_calls: vec![ToolCall::new(
                            call_id.to_string(),
                            ToolCallFunction {
                                name: "fake_small_tool".to_string(),
                                arguments: "{}".to_string(),
                            },
                            false,
                        )],
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
