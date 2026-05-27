//! LLM calling, retry, failover, and history repair helpers.

use super::types::{AgentRunnerContext, RunState};
use super::AgentRunner;
use crate::agent::compaction::CompactionTrigger;
use crate::agent::progress::AgentEvent;
use crate::agent::recovery::repair_agent_message_history_for_provider;
use crate::config::ModelInfo;
use crate::llm::{ChatResponse, LlmClient, LlmError, ProviderCapabilities};
use anyhow::{anyhow, Result};
use std::time::Duration;
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

impl AgentRunner {
    pub(super) async fn call_llm_with_tools(
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
            return self
                .call_llm_with_tools_single_route(ctx, state, iteration)
                .await;
        }

        self.call_llm_with_tools_with_failover(ctx, state, iteration)
            .await
    }

    async fn call_llm_with_tools_single_route(
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

        if !capabilities.can_run_agent_tools()
            || !capabilities.can_run_chat_with_tools_request(!ctx.tools.is_empty(), json_mode)
        {
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
                AttemptOutcome::FailoverToNextRoute(_) => {
                    unreachable!("single-route path has no failover route")
                }
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

            if !capabilities.can_run_agent_tools()
                || !capabilities.can_run_chat_with_tools_request(!ctx.tools.is_empty(), json_mode)
            {
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
                    let retried = self
                        .run_runtime_context_limit_compaction(ctx, state, metadata.route)
                        .await?;
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
                        debug!(
                            error = %error,
                            error_class = Self::error_class(&error),
                            attempt = metadata.attempt,
                            max_attempts = metadata.max_retries,
                            provider = metadata.provider_name,
                            unbounded_retry,
                            retry_budget_remaining,
                            backoff_ms = backoff.as_millis(),
                            "LLM retry decision computed"
                        );
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

    fn rate_limit_quarantine_duration(error: &LlmError, attempt: usize) -> Duration {
        LlmClient::get_retry_delay(error, attempt).unwrap_or_else(|| Duration::from_secs(60))
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

    pub(super) fn opencode_go_unbounded_retry_allowed(
        provider_name: &str,
        error: &LlmError,
    ) -> bool {
        Self::is_opencode_go_provider_name(provider_name) && LlmClient::is_retryable_error(error)
    }

    fn is_opencode_go_provider_name(provider_name: &str) -> bool {
        matches!(
            provider_name.trim().to_ascii_lowercase().as_str(),
            "opencode-go" | "opencode_go"
        )
    }
}
