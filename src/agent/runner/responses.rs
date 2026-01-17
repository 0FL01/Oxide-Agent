//! Response handling for the agent runner.

use super::types::{AgentRunnerContext, FinalResponseInput, RunState, StructuredOutputFailure};
use super::AgentRunner;
use crate::agent::progress::AgentEvent;
use crate::agent::tool_bridge::sync_todos_from_arc;
use tracing::warn;

impl AgentRunner {
    /// Handle malformed structured output responses.
    pub(super) async fn handle_structured_output_error(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        failure: StructuredOutputFailure,
    ) -> anyhow::Result<Option<String>> {
        warn!(
            error = %failure.error,
            raw_preview = %crate::utils::truncate_str(&failure.raw_json, 200),
            "Structured output validation failed"
        );

        state.structured_output_failures += 1;

        // Fail-fast: if we have too many consecutive failures, treat raw response as final answer
        if state.structured_output_failures >= 3 {
            warn!(
                failures = state.structured_output_failures,
                "Too many structured output failures, accepting raw response as final answer"
            );

            if let Some(tx) = ctx.progress_tx {
                let _ = tx
                    .send(AgentEvent::Continuation {
                        reason: "Too many JSON errors, falling back to raw response".to_string(),
                        count: state.continuation_count,
                    })
                    .await;
            }

            let input = FinalResponseInput {
                final_answer: failure.raw_json.clone(),
                raw_json: failure.raw_json,
                reasoning: None,
            };

            return self.handle_final_response(ctx, state, input).await;
        }

        state.continuation_count += 1;
        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::Continuation {
                    reason: "Invalid JSON response, retrying...".to_string(),
                    count: state.continuation_count,
                })
                .await;
        }

        let response_preview = crate::utils::truncate_str(&failure.raw_json, 400);
        let system_message = format!(
            "[SYSTEM: Your previous response does not follow the strict JSON schema.\nError: {}\nResponse: {}\nReturn ONLY valid JSON according to the schema without markdown, XML, or text outside JSON.]",
            failure.error.message(),
            response_preview
        );
        ctx.messages
            .push(crate::llm::Message::system(&system_message));

        Ok(None)
    }

    fn save_final_response(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        raw_json: &str,
        reasoning: Option<String>,
    ) {
        if let Some(reasoning_content) = reasoning {
            ctx.agent.memory_mut().add_message(
                crate::agent::memory::AgentMessage::assistant_with_reasoning(
                    raw_json,
                    reasoning_content,
                ),
            );
        } else {
            ctx.agent
                .memory_mut()
                .add_message(crate::agent::memory::AgentMessage::assistant(raw_json));
        }
    }

    /// Handle a final response payload and run after-agent hooks.
    pub(super) async fn handle_final_response(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        input: FinalResponseInput,
    ) -> anyhow::Result<Option<String>> {
        if ctx.agent.cancellation_token().is_cancelled() {
            return Err(self.cancelled_error(ctx).await);
        }

        let final_response = input.final_answer;

        sync_todos_from_arc(ctx.agent.memory_mut(), ctx.todos_arc).await;
        let hook_result = self.after_agent_hook_result(ctx, state, &final_response, true);

        if let crate::agent::hooks::HookResult::ForceIteration { reason, context } = hook_result {
            state.continuation_count += 1;
            if let Some(tx) = ctx.progress_tx {
                let _ = tx
                    .send(AgentEvent::Continuation {
                        reason: reason.clone(),
                        count: state.continuation_count,
                    })
                    .await;
            }
            ctx.messages
                .push(crate::llm::Message::assistant(&input.raw_json));
            ctx.messages.push(crate::llm::Message::system(&format!(
                "[SYSTEM: {reason}]\n\n{}",
                context.unwrap_or_default()
            )));
            return Ok(None);
        }

        self.save_final_response(ctx, &input.raw_json, input.reasoning);

        if let Some(tx) = ctx.progress_tx {
            if !ctx.config.is_sub_agent {
                let _ = tx.send(AgentEvent::Finished).await;
            }
        }
        Ok(Some(final_response))
    }
}
