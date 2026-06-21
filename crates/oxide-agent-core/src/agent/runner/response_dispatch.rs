//! LLM response dispatch for native, structured, and unstructured responses.

use super::AgentRunner;
use super::tools::ToolTurnAssistantContent;
use super::types::{
    AgentRunResult, AgentRunnerContext, FinalResponseInput, RunState, StructuredOutputFailure,
};
use crate::agent::compaction::CompactionTrigger;
use crate::agent::loop_detection::LoopDetectionOutcome;
use crate::agent::progress::{AgentEvent, AgentEventSource};
use crate::agent::structured_output::parse_structured_output;
use crate::llm::ChatResponse;
use anyhow::{Result, anyhow};
use tracing::{debug, info, warn};

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

            let content_outcome = self.content_loop_outcome(final_answer.as_str()).await;
            if !matches!(content_outcome, LoopDetectionOutcome::NoLoop) {
                return self.handle_loop_outcome(ctx, state, content_outcome).await;
            }

            let input = FinalResponseInput {
                final_answer,
                reasoning: response.reasoning_content,
            };

            return self.handle_final_response(ctx, state, input).await;
        }

        let tool_outcome = self.tool_loop_outcome(&tool_calls).await;
        if !matches!(tool_outcome, LoopDetectionOutcome::NoLoop) {
            return self.handle_loop_outcome(ctx, state, tool_outcome).await;
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
        let tool_calls = std::mem::take(&mut response.tool_calls);

        let tool_outcome = self.tool_loop_outcome(&tool_calls).await;
        if !matches!(tool_outcome, LoopDetectionOutcome::NoLoop) {
            return self.handle_loop_outcome(ctx, state, tool_outcome).await;
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
        if let Some(parsed) = should_parse_unstructured_structured_output_fallback(&raw_output)
            .then(|| parse_structured_output(&raw_output, ctx.tools))
            .and_then(Result::ok)
        {
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

                let content_outcome = self.content_loop_outcome(final_answer.as_str()).await;
                if !matches!(content_outcome, LoopDetectionOutcome::NoLoop) {
                    return self.handle_loop_outcome(ctx, state, content_outcome).await;
                }

                let input = FinalResponseInput {
                    final_answer,
                    reasoning,
                };

                return self.handle_final_response(ctx, state, input).await;
            }

            let tool_outcome = self.tool_loop_outcome(&tool_calls).await;
            if !matches!(tool_outcome, LoopDetectionOutcome::NoLoop) {
                return self.handle_loop_outcome(ctx, state, tool_outcome).await;
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

        let content_outcome = self.content_loop_outcome(final_answer.as_str()).await;
        if !matches!(content_outcome, LoopDetectionOutcome::NoLoop) {
            return self.handle_loop_outcome(ctx, state, content_outcome).await;
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
        {
            let content_len = response.content.as_deref().map(|c| c.len()).unwrap_or(0);
            let finish_reason = &response.finish_reason;
            let is_truncated = finish_reason == "length";
            if is_truncated {
                warn!(
                    task_id = %ctx.task_id,
                    finish_reason,
                    content_len,
                    tool_calls = response.tool_calls.len(),
                    provider = ctx.config.model_provider.as_deref().unwrap_or("unknown"),
                    model = %ctx.config.model_name,
                    "LLM response truncated by provider (finish_reason=length)"
                );
            } else {
                info!(
                    task_id = %ctx.task_id,
                    finish_reason,
                    content_len,
                    tool_calls = response.tool_calls.len(),
                    "LLM response received"
                );
            }
        }

        if let Some(u) = &response.usage {
            ctx.agent.memory_mut().sync_api_usage(u.clone());
            let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
            Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;
        }

        if let Some(ref reasoning) = response.reasoning_content {
            debug!(reasoning_len = reasoning.len(), "Model reasoning received");

            if let Some(tx) = ctx.progress_tx {
                let summary = crate::agent::thoughts::extract_reasoning_summary(reasoning, 100);
                let _ = tx
                    .send(AgentEvent::Reasoning {
                        source: AgentEventSource::Root,
                        summary,
                    })
                    .await;
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

fn should_parse_unstructured_structured_output_fallback(raw: &str) -> bool {
    let trimmed = raw.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with("```")
}

#[cfg(test)]
mod tests {
    #![cfg_attr(
        not(oxide_module_llm_provider_opencode_go),
        allow(dead_code, unused_imports)
    )]

    use super::*;
    use crate::agent::context::{AgentContext, EphemeralSession};
    use crate::agent::memory::AgentMessage;
    use crate::agent::runner::test_support::{
        accidental_structured_final_answer_provider, build_llm_client,
        build_llm_client_for_provider, single_final_response_provider,
    };
    use crate::agent::runner::types::RunState;
    use crate::agent::runner::{AgentRunResult, AgentRunnerConfig, AgentRunnerContext};
    use crate::llm::{ChatResponse, TokenUsage, ToolCall, ToolCallFunction};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[cfg(oxide_module_llm_provider_opencode_go)]
    #[tokio::test]
    async fn run_unstructured_mode_parses_accidental_structured_final_answer() {
        let llm_client = build_llm_client_for_provider(
            accidental_structured_final_answer_provider(),
            "opencode-go",
            "unstructured-model",
        );
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(768);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Какие инструменты тебе доступны?"));

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let mut ctx = AgentRunnerContext {
            task: "Какие инструменты тебе доступны?",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-unstructured-structured-fallback",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            storage: None,
            config: AgentRunnerConfig::new("unstructured-model".to_string(), 1, 1, 30, 256),
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

    #[test]
    fn unstructured_fallback_only_tries_json_like_candidates() {
        assert!(should_parse_unstructured_structured_output_fallback(
            r#"{"thought":"done","tool_call":null,"final_answer":"ok","awaiting_user_input":null}"#
        ));
        assert!(should_parse_unstructured_structured_output_fallback(
            "```json\n{}\n```"
        ));
        assert!(!should_parse_unstructured_structured_output_fallback(
            "# Обзор: Gemma 4 12B\n\nОбычный markdown-ответ модели."
        ));
    }

    #[tokio::test]
    async fn tool_calls_without_typed_runtime_fail_without_history_mutation() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(llm_client);
        let mut session = EphemeralSession::new(2048);
        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "tool runtime missing",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "tool-runtime-missing",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: Some("42".to_string()),
            memory_scope: None,
            memory_behavior: None,
            storage: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 4, 1, 30, 1024)
                .with_model_provider("llm-provider/opencode-go"),
        };
        let mut state = RunState::new();
        let response = ChatResponse {
            content: Some("assistant raw".to_string()),
            tool_calls: vec![ToolCall::new(
                "invoke-runtime-missing",
                ToolCallFunction {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
                },
                false,
            )],
            finish_reason: "tool_calls".to_string(),
            reasoning_content: None,
            usage: None,
        };

        let error = match runner
            .handle_llm_response(response, &mut ctx, &mut state)
            .await
        {
            Ok(_) => panic!("tool calls without typed runtime must fail"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("tool runtime registry is required for active tool calls")
        );
        assert!(
            ctx.agent.memory().get_messages().is_empty(),
            "missing tool runtime must not write partial assistant/tool history"
        );
        assert!(ctx.messages.is_empty());
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
        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let mut ctx = AgentRunnerContext {
            task: "Inspect token metrics",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-token-metrics",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            storage: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 1, 1, 30, 256),
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
                ..TokenUsage::default()
            }),
        };

        runner
            .preprocess_llm_response(&mut response, &mut ctx)
            .await;

        assert_eq!(ctx.agent.memory().token_count(), estimated_tokens);
        assert_eq!(ctx.agent.memory().api_token_count(), Some(9_512));
    }
}
