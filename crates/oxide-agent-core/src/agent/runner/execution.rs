//! Core execution loop for the agent runner.

use super::types::{
    AgentRunResult, AgentRunnerContext, FinalResponseInput, RunState, StructuredOutputFailure,
};
use super::AgentRunner;
use crate::agent::compaction::{
    count_tokens_cached, estimate_request_budget, BudgetState, CompactRequestContext,
    CompactRunOutcome, CompactionBackend, CompactionPhase, CompactionPolicy, CompactionReason,
    CompactionRequest, CompactionTrigger,
};
use crate::agent::memory::AgentMessage;
use crate::agent::progress::{AgentEvent, RepeatedCompactionKind, TokenSnapshot};
use crate::agent::recovery::{repair_agent_message_history_for_provider, sanitize_tool_calls};
use crate::agent::structured_output::parse_structured_output;
use crate::config::ModelInfo;
use crate::llm::{ChatResponse, LlmClient, LlmError, ProviderCapabilities};
use anyhow::{anyhow, Result};
use std::borrow::Cow;
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
    route: &'a ModelInfo,
    route_index: Option<usize>,
    capabilities: ProviderCapabilities,
    attempt: usize,
    max_retries: usize,
}

#[derive(Clone, Copy)]
struct RuntimeCompactionRequest<'a> {
    route: &'a ModelInfo,
    reason: CompactionReason,
    phase: CompactionPhase,
    force: bool,
    target_token_budget: usize,
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
                self.call_llm_with_tools(ctx, &mut state, iteration),
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
                self.run_manual_compaction_checkpoint(ctx, state),
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
        iteration: usize,
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
            return self.call_llm_with_tools_legacy(ctx, state, iteration).await;
        }

        self.call_llm_with_tools_with_failover(ctx, state, iteration)
            .await
    }

    async fn call_llm_with_tools_legacy(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        iteration: usize,
    ) -> Result<ChatResponse> {
        let max_retries = LlmClient::MAX_RETRIES;
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
        let json_mode = Self::structured_output_required_for_model(&model_info);
        let capabilities = LlmClient::provider_capabilities_for_model(&model_info);

        if Self::json_mode_forbids_route(json_mode, &model_info) {
            let error = LlmError::ApiError(format!(
                "Structured-output agent calls are disabled for {} model `{}`; configure a non-ChatGPT route for json_mode",
                model_info.provider, model_info.id
            ));
            Self::emit_llm_error(ctx.progress_tx, &error).await;
            return Err(anyhow!("LLM call failed: {error}"));
        }

        Self::log_llm_route_selected(
            ctx,
            state,
            0,
            provider_name.as_str(),
            model_name.as_str(),
            json_mode,
        );

        self.maybe_run_runtime_pre_sampling_compaction(ctx, state, iteration, &model_info)
            .await?;

        if !capabilities.can_run_chat_with_tools_request(!ctx.tools.is_empty(), json_mode) {
            let error = LlmError::ApiError(format!(
                "Tool-enabled agent calls are not supported for {} model `{}`",
                model_info.provider, model_info.id
            ));
            Self::emit_llm_error(ctx.progress_tx, &error).await;
            return Err(anyhow!("LLM call failed: {error}"));
        }

        let mut attempt = 1usize;
        loop {
            let attempt_max_retries = max_retries.max(attempt);
            Self::refresh_messages_from_memory(ctx);
            Self::log_llm_route_attempt_started(
                ctx,
                state,
                attempt,
                attempt_max_retries,
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
                    ctx.config.temperature,
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
                        route: &model_info,
                        route_index: Some(0),
                        capabilities,
                        attempt,
                        max_retries: attempt_max_retries,
                    },
                    result,
                )
                .await?
            {
                AttemptOutcome::Return(response) => return Ok(response),
                AttemptOutcome::RetrySameRoute => {
                    attempt = attempt.saturating_add(1);
                    continue;
                }
                AttemptOutcome::FailoverToNextRoute(_) => unreachable!("legacy path has no routes"),
            }
        }
    }

    async fn call_llm_with_tools_with_failover(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        iteration: usize,
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
            let failover_from = pending_failover_from.take();
            if let Some(from_route) = failover_from.as_ref() {
                Self::emit_provider_failover(ctx.progress_tx, from_route, &route).await;
            }
            let previous_route_for_downshift = failover_from
                .clone()
                .or_else(|| (route_index != 0).then(|| ctx.config.model_routes[0].clone()));

            ctx.config.model_name = route.id.clone();
            ctx.config.model_max_output_tokens = route.max_output_tokens;
            ctx.config.model_provider = Some(route.provider.clone());

            let json_mode = Self::structured_output_required_for_model(&route);
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

            if let Some(previous_route) = previous_route_for_downshift {
                self.maybe_run_runtime_model_downshift_compaction(
                    ctx,
                    state,
                    &previous_route,
                    &route,
                )
                .await?;
            }

            self.maybe_run_runtime_pre_sampling_compaction(ctx, state, iteration, &route)
                .await?;

            let mut attempt = 1usize;
            loop {
                let attempt_max_retries = max_retries.max(attempt);
                Self::refresh_messages_from_memory(ctx);
                Self::log_llm_route_attempt_started(
                    ctx,
                    state,
                    attempt,
                    attempt_max_retries,
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
                        ctx.config.temperature,
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
                            route: &route,
                            route_index: Some(route_index),
                            capabilities,
                            attempt,
                            max_retries: attempt_max_retries,
                        },
                        result,
                    )
                    .await?;
                match attempt_result {
                    AttemptOutcome::Return(response) => return Ok(response),
                    AttemptOutcome::RetrySameRoute => {
                        attempt = attempt.saturating_add(1);
                        continue;
                    }
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
                        "LLM retry succeeded after retryable error"
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
                    let retried = if ctx.config.codex_style_compaction_enabled {
                        self.run_runtime_context_limit_compaction(ctx, state, metadata.route)
                            .await?
                    } else {
                        false
                    };
                    if retried {
                        return Ok(AttemptOutcome::RetrySameRoute);
                    }
                }

                let unbounded_retry =
                    Self::opencode_go_unbounded_retry_allowed(metadata.provider_name, &error);
                let retry_budget_remaining =
                    metadata.attempt < metadata.max_retries || unbounded_retry;
                if retry_budget_remaining {
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
                                unbounded_retry,
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
                                unbounded_retry,
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
            false,
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
        info!(
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
        let json_mode = self.structured_output_required_for_config(&ctx.config);
        self.route_failover_state
            .route_quarantine
            .retain(|_, until| *until > now);

        if ctx.config.model_routes.is_empty() {
            return None;
        }

        if self.route_is_available(
            &ctx.config.model_routes[0],
            exhausted_routes,
            now,
            json_mode,
        ) {
            return Some(0);
        }

        let fallback_candidates: Vec<(usize, usize)> = ctx
            .config
            .model_routes
            .iter()
            .enumerate()
            .skip(1)
            .filter_map(|(index, route)| {
                self.route_is_available(route, exhausted_routes, now, json_mode)
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
        json_mode: bool,
    ) -> bool {
        let route_key = Self::route_key(route);
        !Self::json_mode_forbids_route(json_mode, route)
            && !exhausted_routes.contains(&route_key)
            && self.llm_client.is_provider_available(&route.provider)
            && self
                .route_failover_state
                .route_quarantine
                .get(&route_key)
                .is_none_or(|until| *until <= now)
    }

    fn json_mode_forbids_route(json_mode: bool, route: &ModelInfo) -> bool {
        json_mode && route.provider.eq_ignore_ascii_case("chatgpt")
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

    fn active_model_info_for_config<'a>(
        &self,
        config: &'a super::types::AgentRunnerConfig,
    ) -> Result<Cow<'a, ModelInfo>, LlmError> {
        if let Some(provider) = config.model_provider.as_ref() {
            return Ok(Cow::Owned(ModelInfo {
                id: config.model_name.clone(),
                max_output_tokens: config.model_max_output_tokens,
                context_window_tokens: 0,
                provider: provider.clone(),
                weight: 1,
            }));
        }

        if let Some(route) = config.model_routes.first() {
            return Ok(Cow::Borrowed(route));
        }

        self.llm_client
            .get_model_info(&config.model_name)
            .map(Cow::Owned)
    }

    fn structured_output_required_for_config(
        &self,
        config: &super::types::AgentRunnerConfig,
    ) -> bool {
        match self.active_model_info_for_config(config) {
            Ok(info) => Self::structured_output_required_for_model(info.as_ref()),
            Err(error) => {
                warn!(
                    model = config.model_name,
                    provider = config.model_provider.as_deref().unwrap_or("unknown"),
                    error = %error,
                    "Failed to resolve model info; defaulting to structured output"
                );
                true
            }
        }
    }

    fn structured_output_required_for_model(model_info: &ModelInfo) -> bool {
        LlmClient::supports_structured_output_for_model(model_info)
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
        let json_mode = self.structured_output_required_for_config(&ctx.config);
        self.llm_client
            .chat_with_tools_single_attempt(
                ctx.system_prompt,
                ctx.messages,
                ctx.tools,
                &ctx.config.model_name,
                ctx.config.temperature,
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

        if !self.structured_output_required_for_config(&ctx.config) {
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

        self.record_assistant_tool_call(ctx, &raw_json, &tool_calls, response.reasoning_content);
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
        _ctx: &mut AgentRunnerContext<'_>,
        _state: &mut RunState,
        _iteration: usize,
    ) -> Result<()> {
        Ok(())
    }

    async fn run_manual_compaction_checkpoint(
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

    async fn maybe_run_runtime_pre_sampling_compaction(
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

    async fn run_runtime_context_limit_compaction(
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

    async fn maybe_run_runtime_model_downshift_compaction(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        previous_route: &ModelInfo,
        next_route: &ModelInfo,
    ) -> Result<bool> {
        if !ctx.config.codex_style_compaction_enabled {
            return Ok(false);
        }
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
            .saturating_add(ctx.agent.skill_token_count())
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
            .saturating_sub(ctx.agent.skill_token_count())
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

    async fn handle_tool_calls_response(
        &mut self,
        response: &mut ChatResponse,
        raw_json: &str,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) -> Result<Option<AgentRunResult>> {
        let tool_calls = sanitize_tool_calls(std::mem::take(&mut response.tool_calls));

        if self.tool_loop_detected(&tool_calls).await {
            return Err(self
                .loop_detected_error(
                    ctx,
                    state,
                    crate::agent::loop_detection::LoopType::ToolCallLoop,
                )
                .await);
        }

        if ctx.tool_runtime_registry.is_some() {
            return self
                .execute_tools_with_runtime(
                    ctx,
                    state,
                    raw_json,
                    response.reasoning_content.take(),
                    tool_calls,
                )
                .await;
        }

        self.record_assistant_tool_call(
            ctx,
            raw_json,
            &tool_calls,
            response.reasoning_content.take(),
        );
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
        if let Ok(parsed) = parse_structured_output(&raw_output, ctx.tools) {
            warn!(
                model = %ctx.config.model_name,
                provider = ctx.config.model_provider.as_deref().unwrap_or("unknown"),
                "Model returned structured-output JSON while structured output was disabled; applying fallback parser"
            );

            state.structured_output_failures = 0;
            let awaiting_user_input = parsed.awaiting_user_input;
            let final_answer = parsed.final_answer;
            let tool_calls = parsed
                .tool_call
                .map(|tool_call| vec![self.build_tool_call(tool_call)])
                .unwrap_or_default();

            if let Some(request) = awaiting_user_input {
                return self
                    .handle_waiting_for_user_input(ctx, state, raw_output, reasoning, request)
                    .await;
            }

            if tool_calls.is_empty() {
                let final_answer = final_answer
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
                    reasoning,
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

            return self
                .handle_tool_calls_response(
                    &mut ChatResponse {
                        content: Some(raw_output.clone()),
                        tool_calls,
                        finish_reason: "stop".to_string(),
                        reasoning_content: reasoning,
                        usage: None,
                    },
                    &raw_output,
                    ctx,
                    state,
                )
                .await;
        }

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
        unbounded: bool,
        wait_secs: Option<u64>,
        provider: &str,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::RateLimitRetrying {
                    attempt,
                    max_attempts,
                    unbounded,
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
            LlmError::EmptyResponse(_) => "empty_response",
            LlmError::JsonError(_) => "json_error",
            _ => "unknown",
        }
    }

    async fn emit_llm_retrying(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        attempt: usize,
        max_attempts: usize,
        unbounded: bool,
        wait_secs: Option<u64>,
        provider: &str,
        error_class: &str,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::LlmRetrying {
                    attempt,
                    max_attempts,
                    unbounded,
                    wait_secs,
                    provider: provider.to_string(),
                    error_class: error_class.to_string(),
                })
                .await;
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
                    backend: CompactionBackend::LocalLlmSummary,
                    provider: None,
                    route: None,
                    token_before,
                    history_items_before,
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
        let policy = CompactionPolicy::default();
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

    fn opencode_go_unbounded_retry_allowed(provider_name: &str, error: &LlmError) -> bool {
        Self::is_opencode_go_provider_name(provider_name) && LlmClient::is_retryable_error(error)
    }

    fn is_opencode_go_provider_name(provider_name: &str) -> bool {
        matches!(
            provider_name.trim().to_ascii_lowercase().as_str(),
            "opencode-go" | "opencode_go"
        )
    }

    pub(super) fn refresh_messages_from_memory(ctx: &mut AgentRunnerContext<'_>) {
        *ctx.messages = Self::convert_memory_to_messages(ctx.agent.memory().get_messages());
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
        CompactSummaryBackend, CompactSummaryError, CompactSummaryRequest, CompactSummaryResult,
        CompactionController, OXIDE_COMPACTED_SUMMARY_PREFIX,
    };
    use crate::agent::context::{AgentContext, EphemeralSession};
    use crate::agent::registry::ToolRegistry;
    use crate::agent::runner::{AgentRunResult, AgentRunnerConfig, AgentRunnerContext};
    use crate::config::{AgentSettings, ModelInfo};
    use crate::llm::{
        ChatResponse, LlmClient, LlmError, MockLlmProvider, TokenUsage, ToolCall, ToolCallFunction,
    };
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct StaticRuntimeSummaryBackend;

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

    #[test]
    fn json_mode_forbids_chatgpt_routes_only() {
        let chatgpt_route = ModelInfo {
            id: "gpt-5.4-mini".to_string(),
            provider: "chatgpt".to_string(),
            max_output_tokens: 32_000,
            context_window_tokens: 200_000,
            weight: 1,
        };
        let zai_route = ModelInfo {
            id: "glm-4.7".to_string(),
            provider: "zai".to_string(),
            max_output_tokens: 32_000,
            context_window_tokens: 200_000,
            weight: 1,
        };

        assert!(AgentRunner::json_mode_forbids_route(true, &chatgpt_route));
        assert!(!AgentRunner::json_mode_forbids_route(false, &chatgpt_route));
        assert!(!AgentRunner::json_mode_forbids_route(true, &zai_route));
    }

    #[test]
    fn structured_output_requirement_uses_active_provider_without_registry_lookup() {
        let llm_client = build_llm_client(single_final_response_provider());
        let runner = AgentRunner::new(llm_client);
        let config = AgentRunnerConfig::new("glm-4.7".to_string(), 8, 4, 60, 4096)
            .with_model_provider("zai")
            .with_model_routes(vec![ModelInfo {
                id: "gpt-5.4-mini".to_string(),
                provider: "chatgpt".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            }]);

        assert!(runner.structured_output_required_for_config(&config));
    }

    #[test]
    fn unbounded_retry_is_limited_to_opencode_go_retryable_errors() {
        let rate_limit = LlmError::RateLimit {
            wait_secs: None,
            message: "too many requests".to_string(),
        };
        let invalid_request = LlmError::ApiError("invalid API key".to_string());

        assert!(AgentRunner::opencode_go_unbounded_retry_allowed(
            "opencode-go",
            &rate_limit
        ));
        assert!(AgentRunner::opencode_go_unbounded_retry_allowed(
            "opencode_go",
            &LlmError::NetworkError("connection reset".to_string())
        ));
        assert!(!AgentRunner::opencode_go_unbounded_retry_allowed(
            "openrouter",
            &rate_limit
        ));
        assert!(!AgentRunner::opencode_go_unbounded_retry_allowed(
            "opencode-go",
            &invalid_request
        ));
    }

    #[test]
    fn structured_output_requirement_disables_chatgpt_primary_route() {
        let llm_client = build_llm_client(single_final_response_provider());
        let runner = AgentRunner::new(llm_client);
        let config = AgentRunnerConfig::new("gpt-5.4-mini".to_string(), 8, 4, 60, 4096)
            .with_model_provider("chatgpt")
            .with_model_routes(vec![ModelInfo {
                id: "gpt-5.4-mini".to_string(),
                provider: "chatgpt".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            }]);

        assert!(!runner.structured_output_required_for_config(&config));
    }

    #[test]
    fn select_model_route_index_keeps_chatgpt_route_when_structured_output_is_disabled() {
        let llm_client = build_llm_client_for_provider(
            single_final_response_provider(),
            "chatgpt",
            "gpt-5.4-mini",
        );
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(768);
        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let ctx = AgentRunnerContext {
            task: "Route selection regression",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-chatgpt-route-selection",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("gpt-5.4-mini".to_string(), 8, 4, 60, 4096)
                .with_model_provider("chatgpt")
                .with_model_routes(vec![ModelInfo {
                    id: "gpt-5.4-mini".to_string(),
                    provider: "chatgpt".to_string(),
                    max_output_tokens: 32_000,
                    context_window_tokens: 200_000,
                    weight: 1,
                }]),
        };

        assert_eq!(
            runner.select_model_route_index(&ctx, &std::collections::HashSet::new()),
            Some(0)
        );
    }

    #[test]
    fn structured_output_requirement_uses_primary_route_before_selection() {
        let llm_client = build_llm_client(single_final_response_provider());
        let runner = AgentRunner::new(llm_client);
        let config = AgentRunnerConfig::new("missing-model-name".to_string(), 8, 4, 60, 4096)
            .with_model_routes(vec![ModelInfo {
                id: "glm-4.7".to_string(),
                provider: "zai".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            }]);

        assert!(runner.structured_output_required_for_config(&config));
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
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-history-repair",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_controller: None,
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
    async fn run_unstructured_mode_parses_accidental_structured_final_answer() {
        let llm_client = build_llm_client_for_provider(
            accidental_structured_final_answer_provider(),
            "openrouter",
            "chat-openrouter",
        );
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(768);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Какие инструменты тебе доступны?"));

        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let mut ctx = AgentRunnerContext {
            task: "Какие инструменты тебе доступны?",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-unstructured-structured-fallback",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("chat-openrouter".to_string(), 1, 1, 30, 256),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");

        assert!(
            matches!(result, AgentRunResult::Final(answer) if answer == "Tools: `read_file`, `write_file`")
        );
        let last_message = ctx
            .agent
            .memory()
            .get_messages()
            .last()
            .expect("assistant response should be saved");
        assert_eq!(last_message.content, "Tools: `read_file`, `write_file`");
        assert!(!last_message.content.contains("\"final_answer\""));
    }

    #[tokio::test]
    async fn run_retries_after_context_overflow_with_runtime_context_limit_compaction() {
        let llm_client = build_llm_client(context_overflow_then_summary_then_final_provider());
        let compaction_controller = CompactionController::local_llm(Arc::clone(&llm_client), 1);
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Retry after overflow"));
        session
            .memory_mut()
            .add_message(AgentMessage::summary("[COMPACTION_SUMMARY]\nOld"));
        session
            .memory_mut()
            .add_message(AgentMessage::user("Recent request."));

        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let mut ctx = AgentRunnerContext {
            task: "Retry after overflow",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-overflow-runtime-compaction",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_controller: Some(&compaction_controller),
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("mock-model".to_string(), 2, 1, 30, 256)
                .with_codex_style_compaction(true),
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
        let llm_client = build_llm_client(pre_sampling_summary_then_final_provider());
        let compaction_controller = CompactionController::local_llm(Arc::clone(&llm_client), 1);
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(1_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Pre-sampling compact"));
        session
            .memory_mut()
            .add_message(AgentMessage::user("older ".repeat(4_000)));

        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let mut ctx = AgentRunnerContext {
            task: "Pre-sampling compact",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-pre-sampling-runtime-compaction",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_controller: Some(&compaction_controller),
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("mock-model".to_string(), 1, 1, 30, 256)
                .with_codex_style_compaction(true),
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
        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
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
            registry: &registry,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-full-budget-threshold",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("mock-model".to_string(), 1, 1, 30, 16_000),
        };
        let route = ModelInfo {
            id: "mock-model".to_string(),
            provider: "mock".to_string(),
            max_output_tokens: 16_000,
            context_window_tokens: 20_000,
            weight: 1,
        };

        assert!(AgentRunner::runtime_compaction_threshold_reached(
            &ctx, &route
        ));
        drop(ctx);
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
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "refresh-transient-test",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_controller: None,
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
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-token-metrics",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_controller: None,
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
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-provider-failover",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_controller: None,
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
            agent_model_id: Some("primary-model".to_string()),
            agent_model_provider: Some("primary".to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm_client = LlmClient::new(&settings);
        llm_client.register_provider("primary".to_string(), Arc::new(primary));
        llm_client.register_provider("backup".to_string(), Arc::new(backup));
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

        let registry = ToolRegistry::new();
        let tools = registry.all_tools();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let mut ctx = AgentRunnerContext {
            task: "Fail over to a smaller model route",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-model-downshift",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_controller: Some(&compaction_controller),
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("primary-model".to_string(), 1, 1, 30, 256)
                .with_model_provider("primary")
                .with_codex_style_compaction(true)
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
                        context_window_tokens: 15_000,
                        provider: "backup".to_string(),
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
                } if provider == "backup" && route == "backup-model"
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
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-primary-recovery",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_controller: None,
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
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-nvidia-capability-skip",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_controller: None,
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
        build_llm_client_for_provider(provider, "mock", "mock-model")
    }

    fn build_llm_client_for_provider(
        provider: MockLlmProvider,
        provider_name: &str,
        model_name: &str,
    ) -> Arc<LlmClient> {
        let settings = AgentSettings {
            agent_model_id: Some(model_name.to_string()),
            agent_model_provider: Some(provider_name.to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm_client = LlmClient::new(&settings);
        llm_client.register_provider(provider_name.to_string(), Arc::new(provider));
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

    fn accidental_structured_final_answer_provider() -> MockLlmProvider {
        let mut provider = MockLlmProvider::new();
        provider.expect_chat_with_tools().return_once(|_| {
            Ok(ChatResponse {
                content: Some(
                    r#"{"thought":"Tool list ready","tool_call":null,"final_answer":"Tools: `read_file`, `write_file`","awaiting_user_input":null}"#
                        .to_string(),
                ),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                reasoning_content: None,
                usage: None,
            })
        });
        stub_non_chat_methods(&mut provider);
        provider
    }

    fn context_overflow_then_summary_then_final_provider() -> MockLlmProvider {
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
            .times(1)
            .returning(|_, _, user_message, model_id, _| {
                assert_eq!(model_id, "mock-model");
                assert!(user_message.contains("## Source History"));
                Ok("Runtime context-limit handoff summary.".to_string())
            });
        provider
            .expect_transcribe_audio()
            .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_analyze_image()
            .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
    }

    fn pre_sampling_summary_then_final_provider() -> MockLlmProvider {
        let mut provider = MockLlmProvider::new();
        provider.expect_chat_with_tools().times(1).return_once(|_| {
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
            .times(1)
            .returning(|_, _, user_message, model_id, _| {
                assert_eq!(model_id, "mock-model");
                assert!(user_message.contains("## Source History"));
                Ok("Pre-sampling handoff summary.".to_string())
            });
        provider
            .expect_transcribe_audio()
            .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_analyze_image()
            .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
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
