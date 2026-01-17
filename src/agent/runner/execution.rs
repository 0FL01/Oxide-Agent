//! Core execution loop for the agent runner.

use super::types::{AgentRunnerContext, FinalResponseInput, RunState, StructuredOutputFailure};
use super::AgentRunner;
use crate::agent::progress::AgentEvent;
use crate::agent::recovery::sanitize_tool_calls;
use crate::agent::structured_output::parse_structured_output;
use crate::llm::ChatResponse;
use anyhow::{anyhow, Result};
use tracing::{debug, warn};

impl AgentRunner {
    /// Execute the agent loop until completion or error.
    pub async fn run(&mut self, ctx: &mut AgentRunnerContext<'_>) -> Result<String> {
        self.reset_loop_detector(ctx).await;
        self.apply_before_agent_hooks(ctx)?;
        self.run_loop(ctx).await
    }

    async fn run_loop(&mut self, ctx: &mut AgentRunnerContext<'_>) -> Result<String> {
        let mut state = RunState::new();

        for iteration in 0..ctx.config.max_iterations {
            state.iteration = iteration;

            if ctx.agent.cancellation_token().is_cancelled() {
                return Err(self.cancelled_error(ctx).await);
            }

            self.apply_before_iteration_hooks(ctx, &state)?;

            debug!(task_id = %ctx.task_id, iteration = iteration, "Agent loop iteration");

            if let Some(tx) = ctx.progress_tx {
                let current_tokens = ctx.agent.memory().token_count();
                let display_tokens = ctx
                    .agent
                    .memory()
                    .api_token_count()
                    .unwrap_or(current_tokens);

                let _ = tx
                    .send(AgentEvent::Thinking {
                        tokens: display_tokens,
                    })
                    .await;
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

            let response = self.call_llm_with_tools(ctx).await?;
            if let Some(result) = self.handle_llm_response(response, ctx, &mut state).await? {
                return Ok(result);
            }
        }

        Err(anyhow!(
            "Agent exceeded iteration limit ({}).",
            ctx.config.max_iterations
        ))
    }

    async fn call_llm_with_tools(&self, ctx: &mut AgentRunnerContext<'_>) -> Result<ChatResponse> {
        let response = self
            .llm_client
            .chat_with_tools(
                ctx.system_prompt,
                ctx.messages,
                ctx.tools,
                &ctx.config.model_name,
                true, // json_mode
            )
            .await;

        if let Err(ref e) = response {
            if let Some(tx) = ctx.progress_tx {
                let _ = tx
                    .send(AgentEvent::Error(format!("LLM call failed: {e}")))
                    .await;
            }
        }

        response.map_err(|e| anyhow!("LLM call failed: {e}"))
    }

    async fn handle_llm_response(
        &mut self,
        mut response: ChatResponse,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) -> Result<Option<String>> {
        self.preprocess_llm_response(&mut response, ctx).await;

        let raw_json = response
            .content
            .clone()
            .unwrap_or_default()
            .trim()
            .to_string();

        if !response.tool_calls.is_empty() {
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

            self.record_assistant_tool_call(ctx, &raw_json, &tool_calls);
            self.execute_tools(ctx, state, tool_calls).await?;
            return Ok(None);
        }

        let parsed = match parse_structured_output(&raw_json, ctx.tools) {
            Ok(parsed) => parsed,
            Err(error) => {
                let failure = StructuredOutputFailure { error, raw_json };
                self.handle_structured_output_error(ctx, state, failure)
                    .await;
                return Ok(None);
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
        self.execute_tools(ctx, state, tool_calls).await?;
        Ok(None)
    }

    async fn preprocess_llm_response(
        &mut self,
        response: &mut ChatResponse,
        ctx: &mut AgentRunnerContext<'_>,
    ) {
        if let Some(u) = &response.usage {
            ctx.agent
                .memory_mut()
                .sync_token_count(u.total_tokens as usize);
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
