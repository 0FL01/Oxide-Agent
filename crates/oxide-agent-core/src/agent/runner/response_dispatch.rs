//! LLM response dispatch for native, structured, and unstructured responses.

use super::tools::ToolTurnAssistantContent;
use super::types::{
    AgentRunResult, AgentRunnerContext, FinalResponseInput, RunState, StructuredOutputFailure,
};
use super::AgentRunner;
use crate::agent::compaction::CompactionTrigger;
use crate::agent::progress::AgentEvent;
use crate::agent::recovery::sanitize_tool_calls;
use crate::agent::structured_output::parse_structured_output;
use crate::llm::ChatResponse;
use anyhow::{anyhow, Result};
use tracing::{debug, warn};

impl AgentRunner {
    pub(super) async fn handle_llm_response(
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

        if ctx.tool_runtime_registry.is_none() {
            return Err(Self::missing_tool_runtime_registry_error(ctx));
        }
        self.execute_tools_with_runtime(
            ctx,
            state,
            ToolTurnAssistantContent::StructuredControlEnvelope,
            response.reasoning_content,
            tool_calls,
        )
        .await
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
                    ToolTurnAssistantContent::NativeModelContent(raw_json),
                    response.reasoning_content.take(),
                    tool_calls,
                )
                .await;
        }

        Err(Self::missing_tool_runtime_registry_error(ctx))
    }

    fn missing_tool_runtime_registry_error(ctx: &AgentRunnerContext<'_>) -> anyhow::Error {
        anyhow!(
            "tool runtime registry is required for active tool calls; current provider={}, model={}",
            ctx.config.model_provider.as_deref().unwrap_or("unknown"),
            ctx.config.model_name,
        )
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
                .execute_tools_with_runtime(
                    ctx,
                    state,
                    ToolTurnAssistantContent::StructuredControlEnvelope,
                    reasoning,
                    tool_calls,
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

    pub(super) async fn preprocess_llm_response(
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
}
