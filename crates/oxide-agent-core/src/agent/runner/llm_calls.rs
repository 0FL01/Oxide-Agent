//! LLM calling, retry, failover, and history repair helpers.

use super::AgentRunner;
use super::types::{AgentRunnerContext, RunState};
use crate::agent::compaction::{
    CompactionPolicy, CompactionRequest, CompactionTrigger, estimate_request_budget,
};
use crate::agent::memory::{AgentMessage, AgentMessageAttachment, AgentMessageAttachmentKind};
use crate::agent::progress::AgentEvent;
use crate::agent::providers::SandboxRuntime;
use crate::agent::recovery::repair_agent_message_history_for_provider;
use crate::config::{AGENT_RESPONSE_SOFT_MAX_OUTPUT_TOKENS, ModelInfo};
use crate::llm::{
    ChatResponse, LlmClient, LlmError, Message, MessageContentPart, ProviderCapabilities,
};
use crate::sandbox::SandboxFileOps;
use anyhow::{Result, anyhow};
use std::time::Duration;
use tracing::{debug, info, warn};

const AGENT_LATENCY_TARGET: &str = "oxide_agent_core::agent_latency";

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

const MAX_NATIVE_IMAGE_PART_BYTES: u64 = 20 * 1024 * 1024;

#[async_trait::async_trait]
trait NativeImageFileReader: Send + Sync {
    async fn read_native_image_file(&self, path: &str) -> Result<Vec<u8>>;
}

#[async_trait::async_trait]
impl<T> NativeImageFileReader for T
where
    T: SandboxFileOps + ?Sized,
{
    async fn read_native_image_file(&self, path: &str) -> Result<Vec<u8>> {
        self.read_file(path).await
    }
}

impl AgentRunner {
    async fn refresh_messages_for_route(ctx: &mut AgentRunnerContext<'_>, route: &ModelInfo) {
        Self::refresh_messages_from_memory(ctx);
        Self::attach_native_image_parts_for_route(ctx, route).await;
    }

    async fn attach_native_image_parts_for_route(
        ctx: &mut AgentRunnerContext<'_>,
        route: &ModelInfo,
    ) {
        if !Self::route_supports_native_image_parts(route) {
            return;
        }

        let memory_messages = ctx.agent.memory().get_messages().to_vec();
        if !Self::has_image_attachment_refs(&memory_messages) {
            return;
        }

        let Some(sandbox_scope) = ctx.agent.sandbox_scope().cloned() else {
            warn!(
                provider = route.provider.as_str(),
                model = route.id.as_str(),
                "Skipping native image attachment resolution because the agent context has no sandbox scope"
            );
            return;
        };

        let sandbox_fileops = SandboxRuntime::new(sandbox_scope);
        Self::attach_native_image_parts_from_refs(
            ctx.messages,
            &memory_messages,
            &sandbox_fileops,
            route,
        )
        .await;
    }

    fn route_supports_native_image_parts(route: &ModelInfo) -> bool {
        crate::llm::provider_media_capabilities_for_model(route).supports_image_understanding
    }

    fn has_image_attachment_refs(memory_messages: &[AgentMessage]) -> bool {
        memory_messages.iter().any(|message| {
            message
                .native_image_attachments()
                .iter()
                .any(|attachment| attachment.kind == AgentMessageAttachmentKind::Image)
        })
    }

    async fn attach_native_image_parts_from_refs(
        messages: &mut [Message],
        memory_messages: &[AgentMessage],
        image_reader: &dyn NativeImageFileReader,
        route: &ModelInfo,
    ) {
        for (message, memory_message) in messages.iter_mut().zip(memory_messages) {
            if message.role != "user" && message.role != "tool" {
                continue;
            }

            let mut content_parts = Vec::new();
            for attachment in memory_message.native_image_attachments() {
                if attachment.kind != AgentMessageAttachmentKind::Image {
                    continue;
                }

                let Some(mime_type) = Self::native_image_mime_type(attachment) else {
                    warn!(
                        provider = route.provider.as_str(),
                        model = route.id.as_str(),
                        file_name = attachment.file_name.as_str(),
                        mime_type = attachment.mime_type.as_deref().unwrap_or(""),
                        "Skipping native image attachment with non-image MIME type"
                    );
                    continue;
                };

                if attachment.size_bytes > MAX_NATIVE_IMAGE_PART_BYTES {
                    warn!(
                        provider = route.provider.as_str(),
                        model = route.id.as_str(),
                        file_name = attachment.file_name.as_str(),
                        size_bytes = attachment.size_bytes,
                        max_bytes = MAX_NATIVE_IMAGE_PART_BYTES,
                        "Skipping oversized native image attachment"
                    );
                    continue;
                }

                // Use inline data when available (e.g. browser screenshots from
                // Postgres). Fall back to reading from sandbox_path (filesystem).
                let image_bytes_result: Result<Vec<u8>, String> =
                    if let Some(data) = &attachment.data {
                        Ok(data.clone())
                    } else {
                        image_reader
                            .read_native_image_file(&attachment.sandbox_path)
                            .await
                            .map_err(|e| e.to_string())
                    };

                match image_bytes_result {
                    Ok(bytes) if bytes.is_empty() => {
                        warn!(
                            provider = route.provider.as_str(),
                            model = route.id.as_str(),
                            path = attachment.sandbox_path.as_str(),
                            "Skipping empty native image attachment"
                        );
                    }
                    Ok(bytes) => {
                        let byte_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                        if byte_len > MAX_NATIVE_IMAGE_PART_BYTES {
                            warn!(
                                provider = route.provider.as_str(),
                                model = route.id.as_str(),
                                path = attachment.sandbox_path.as_str(),
                                size_bytes = byte_len,
                                max_bytes = MAX_NATIVE_IMAGE_PART_BYTES,
                                "Skipping oversized native image attachment after read"
                            );
                            continue;
                        }
                        content_parts.push(MessageContentPart::image(mime_type, bytes));
                    }
                    Err(error) => {
                        warn!(
                            provider = route.provider.as_str(),
                            model = route.id.as_str(),
                            path = attachment.sandbox_path.as_str(),
                            error = %error,
                            "Skipping native image attachment because image bytes could not be resolved"
                        );
                    }
                }
            }

            if !content_parts.is_empty() {
                message.content_parts.extend(content_parts);
            }
        }
    }

    fn native_image_mime_type(attachment: &AgentMessageAttachment) -> Option<String> {
        let Some(mime_type) = attachment.mime_type.as_deref().map(str::trim) else {
            return Some("image/jpeg".to_string());
        };
        if mime_type.is_empty() {
            return Some("image/jpeg".to_string());
        }
        mime_type
            .starts_with("image/")
            .then(|| mime_type.to_string())
    }

    pub(super) async fn call_llm_with_tools(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        iteration: usize,
    ) -> Result<ChatResponse> {
        // Emit milestone on first LLM call of first iteration.
        if state.iteration == 0 {
            debug!(
                target: AGENT_LATENCY_TARGET,
                task_id = %ctx.task_id,
                iteration,
                model = %ctx.config.model_name,
                provider = ?ctx.config.model_provider,
                route_count = ctx.config.model_routes.len(),
                "Agent first LLM call starting"
            );
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
            let error = LlmError::api_error(format!(
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
            let error = LlmError::api_error(format!(
                "Tool-enabled agent calls are not supported for {} model `{}`",
                model_info.provider, model_info.id
            ));
            Self::emit_llm_error(ctx.progress_tx, &error).await;
            return Err(anyhow!("LLM call failed: {error}"));
        }

        let mut attempt = 1usize;
        loop {
            let attempt_max_retries = max_retries.max(attempt);
            Self::refresh_messages_for_route(ctx, &model_info).await;
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
                    ctx.date_suffix,
                    ctx.messages,
                    ctx.tools,
                    &ctx.config.model_name,
                    ctx.config.temperature,
                    json_mode,
                    ctx.config.reasoning_effort.as_deref(),
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
                let error = LlmError::api_error(format!(
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
                Self::refresh_messages_for_route(ctx, &route).await;
                Self::log_llm_route_attempt_started(
                    ctx,
                    state,
                    attempt,
                    attempt_max_retries,
                    Some(route_index),
                    route.provider.as_str(),
                    route.id.as_str(),
                );
                let request_route = Self::route_with_soft_output_cap(ctx, &route);
                let result = self
                    .llm_client
                    .chat_with_tools_single_attempt_for_model_info(
                        ctx.system_prompt,
                        ctx.date_suffix,
                        ctx.messages,
                        ctx.tools,
                        &request_route,
                        ctx.config.temperature,
                        json_mode,
                        ctx.config.reasoning_effort.as_deref(),
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
                if retry_budget_remaining
                    && let Some(backoff) = LlmClient::get_retry_delay(&error, metadata.attempt)
                {
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
            LlmError::RequestBuilder(_) => "request_builder",
            LlmError::NetworkError(_) => "network",
            LlmError::ApiError {
                status: Some(status),
                ..
            } if crate::llm::is_transient_server_status(*status) => "server_error",
            LlmError::ApiError { .. } => "api",
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

    fn route_with_soft_output_cap(ctx: &AgentRunnerContext<'_>, route: &ModelInfo) -> ModelInfo {
        let mut capped_route = route.clone();
        capped_route.max_output_tokens = Self::effective_request_max_output_tokens(ctx, route);
        capped_route
    }

    fn effective_request_max_output_tokens(ctx: &AgentRunnerContext<'_>, route: &ModelInfo) -> u32 {
        let policy = CompactionPolicy::default();
        let request = CompactionRequest::new(
            CompactionTrigger::PreIteration,
            ctx.task,
            ctx.system_prompt,
            ctx.tools,
            &route.id,
            route.max_output_tokens,
            ctx.config.is_sub_agent,
        );
        let budget = estimate_request_budget(&policy, &request, ctx.agent);
        let context_window_tokens = if route.context_window_tokens == 0 {
            budget.context_window_tokens
        } else {
            route.context_window_tokens as usize
        };
        let remaining_output_tokens = context_window_tokens
            .saturating_sub(budget.total_input_tokens)
            .saturating_sub(policy.hard_reserve_tokens);
        let remaining_output_tokens = u32::try_from(remaining_output_tokens)
            .unwrap_or(u32::MAX)
            .max(1);

        route
            .max_output_tokens
            .clamp(1, AGENT_RESPONSE_SOFT_MAX_OUTPUT_TOKENS)
            .min(remaining_output_tokens)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::context::{AgentContext, EphemeralSession};
    use crate::agent::memory::AgentMessage;
    use crate::agent::progress::AgentEvent;
    use crate::agent::runner::test_support::{
        build_llm_client, collect_progress_events, final_structured_response,
        single_final_response_provider, stub_non_chat_methods,
    };
    use crate::agent::runner::{AgentRunResult, AgentRunnerConfig, AgentRunnerContext};
    use crate::config::{AGENT_RESPONSE_SOFT_MAX_OUTPUT_TOKENS, AgentSettings, ModelInfo};
    use crate::llm::{LlmError, MockLlmProvider, ToolCall, ToolCallFunction};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::sync::Mutex;

    #[derive(Default)]
    struct FakeImageFileOps {
        files: StdMutex<HashMap<String, Vec<u8>>>,
        reads: StdMutex<Vec<String>>,
    }

    impl FakeImageFileOps {
        fn with_file(path: &str, bytes: &[u8]) -> Self {
            let mut files = HashMap::new();
            files.insert(path.to_string(), bytes.to_vec());
            Self {
                files: StdMutex::new(files),
                reads: StdMutex::new(Vec::new()),
            }
        }

        fn read_count(&self) -> usize {
            self.reads.lock().expect("reads lock").len()
        }
    }

    #[async_trait::async_trait]
    impl NativeImageFileReader for FakeImageFileOps {
        async fn read_native_image_file(&self, path: &str) -> Result<Vec<u8>> {
            self.reads
                .lock()
                .expect("reads lock")
                .push(path.to_string());
            self.files
                .lock()
                .expect("files lock")
                .get(path)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing file: {path}"))
        }
    }

    fn test_route(provider: &str, id: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            provider: provider.to_string(),
            max_output_tokens: 256,
            context_window_tokens: 128_000,
            weight: 1,
        }
    }

    fn image_message(path: &str) -> AgentMessage {
        AgentMessage::user_task("What is shown in the image?").with_user_attachments(vec![
            crate::agent::memory::AgentMessageAttachment::image(
                "screenshot.jpg",
                Some("image/jpeg".to_string()),
                3,
                path,
            ),
        ])
    }

    #[tokio::test]
    async fn native_image_parts_resolve_from_user_attachment_refs() {
        let memory_messages = vec![image_message("/workspace/uploads/shot.jpg")];
        let mut messages = AgentRunner::convert_memory_to_messages(&memory_messages);
        let fileops = FakeImageFileOps::with_file("/workspace/uploads/shot.jpg", b"jpg");
        let route = test_route("opencode-go", "mimo-v2.5");

        AgentRunner::attach_native_image_parts_from_refs(
            &mut messages,
            &memory_messages,
            &fileops,
            &route,
        )
        .await;

        assert_eq!(fileops.read_count(), 1);
        assert_eq!(messages[0].text_projection(), "What is shown in the image?");
        assert_eq!(messages[0].content_parts.len(), 1);
        assert!(matches!(
            &messages[0].content_parts[0],
            MessageContentPart::Image { mime_type, bytes }
                if mime_type == "image/jpeg" && bytes == b"jpg"
        ));
    }

    #[tokio::test]
    async fn text_only_route_degrades_without_reading_image_refs() {
        let memory_messages = vec![image_message("/workspace/uploads/shot.jpg")];
        let mut messages = AgentRunner::convert_memory_to_messages(&memory_messages);
        let fileops = FakeImageFileOps::with_file("/workspace/uploads/shot.jpg", b"jpg");
        let route = test_route("unknown-text-provider", "text-only");

        assert!(!AgentRunner::route_supports_native_image_parts(&route));
        if AgentRunner::route_supports_native_image_parts(&route) {
            AgentRunner::attach_native_image_parts_from_refs(
                &mut messages,
                &memory_messages,
                &fileops,
                &route,
            )
            .await;
        }

        assert_eq!(fileops.read_count(), 0);
        assert!(messages[0].content_parts.is_empty());
        assert_eq!(messages[0].text_projection(), "What is shown in the image?");
    }

    #[cfg(feature = "llm-openai-base")]
    #[test]
    fn openai_base_route_supports_native_image_parts_by_capability() {
        let route = test_route("openai-base:local", "local-vision-model");

        assert!(AgentRunner::route_supports_native_image_parts(&route));
    }

    #[tokio::test]
    async fn missing_image_ref_degrades_to_text_only() {
        let memory_messages = vec![image_message("/workspace/uploads/missing.jpg")];
        let mut messages = AgentRunner::convert_memory_to_messages(&memory_messages);
        let fileops = FakeImageFileOps::default();
        let route = test_route("opencode-go", "mimo-v2.5");

        AgentRunner::attach_native_image_parts_from_refs(
            &mut messages,
            &memory_messages,
            &fileops,
            &route,
        )
        .await;

        assert_eq!(fileops.read_count(), 1);
        assert!(messages[0].content_parts.is_empty());
        assert_eq!(messages[0].text_projection(), "What is shown in the image?");
    }

    #[tokio::test]
    async fn native_image_parts_resolve_for_tool_messages() {
        let mut tool_message = AgentMessage::tool("call-1", "describe_image_file", "result");
        tool_message.attachments = image_message("/workspace/uploads/shot.jpg").attachments;
        let memory_messages = vec![tool_message];
        let mut messages = AgentRunner::convert_memory_to_messages(&memory_messages);
        let fileops = FakeImageFileOps::with_file("/workspace/uploads/shot.jpg", b"jpg");
        let route = test_route("opencode-go", "mimo-v2.5");

        AgentRunner::attach_native_image_parts_from_refs(
            &mut messages,
            &memory_messages,
            &fileops,
            &route,
        )
        .await;

        assert_eq!(fileops.read_count(), 1);
        assert_eq!(messages[0].content_parts.len(), 1);
        assert!(matches!(
            &messages[0].content_parts[0],
            MessageContentPart::Image { mime_type, bytes }
                if mime_type == "image/jpeg" && bytes == b"jpg"
        ));
        assert_eq!(messages[0].role, "tool");
    }

    #[test]
    fn unbounded_retry_is_limited_to_opencode_go_retryable_errors() {
        let rate_limit = LlmError::RateLimit {
            wait_secs: None,
            message: "too many requests".to_string(),
        };
        let invalid_request = LlmError::api_error("invalid API key");

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
    fn route_with_soft_output_cap_limits_provider_request_without_reserving_context() {
        let tools = Vec::new();
        let mut session = EphemeralSession::new(200_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Keep input context available"));

        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let ctx = AgentRunnerContext {
            task: "Keep input context available",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-soft-output-cap",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("wide-model".to_string(), 1, 1, 30, 200_000),
        };
        let route = ModelInfo {
            id: "wide-model".to_string(),
            provider: "mock".to_string(),
            max_output_tokens: 200_000,
            context_window_tokens: 200_000,
            weight: 1,
        };

        let capped = AgentRunner::route_with_soft_output_cap(&ctx, &route);
        assert_eq!(
            capped.max_output_tokens,
            AGENT_RESPONSE_SOFT_MAX_OUTPUT_TOKENS
        );

        let tight_route = ModelInfo {
            context_window_tokens: 10_000,
            ..route
        };
        let tight = AgentRunner::route_with_soft_output_cap(&ctx, &tight_route);
        assert!(tight.max_output_tokens < AGENT_RESPONSE_SOFT_MAX_OUTPUT_TOKENS);
        assert!(tight.max_output_tokens <= 1_808);
        drop(ctx);
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

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let mut ctx = AgentRunnerContext {
            task: "Repair invalid tool history",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-history-repair",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 2, 1, 30, 256),
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
            agent_model_id: Some("deepseek-v4-pro".to_string()),
            agent_model_provider: Some("llm-provider/opencode-go".to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm_client = LlmClient::new(&settings);
        llm_client.register_provider("llm-provider/opencode-go".to_string(), Arc::new(primary));
        llm_client.register_provider("opencode-go".to_string(), Arc::new(backup));
        let llm_client = Arc::new(llm_client);

        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Fail over after persistent 429"));

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(32);
        let mut ctx = AgentRunnerContext {
            task: "Fail over after persistent 429",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-provider-failover",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
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
                        context_window_tokens: 128_000,
                        provider: "opencode-go".to_string(),
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
                } if from_provider == "llm-provider/opencode-go"
                    && from_model == "deepseek-v4-pro"
                    && to_provider == "opencode-go"
                    && to_model == "deepseek-v4-flash"
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
            agent_model_id: Some("deepseek-v4-pro".to_string()),
            agent_model_provider: Some("llm-provider/opencode-go".to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm_client = LlmClient::new(&settings);
        llm_client.register_provider("llm-provider/opencode-go".to_string(), Arc::new(primary));
        llm_client.register_provider("opencode-go".to_string(), Arc::new(backup));
        let llm_client = Arc::new(llm_client);

        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Stay on primary when it wakes up"));

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(32);
        let mut ctx = AgentRunnerContext {
            task: "Stay on primary when it wakes up",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-primary-recovery",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
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
                        context_window_tokens: 128_000,
                        provider: "opencode-go".to_string(),
                        weight: 2,
                    },
                ]),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");
        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));

        drop(ctx);
        drop(progress_tx);
        let events = collect_progress_events(&mut progress_rx).await;
        assert!(
            !events
                .iter()
                .any(|event| { matches!(event, AgentEvent::ProviderFailoverActivated { .. }) })
        );
    }
}
