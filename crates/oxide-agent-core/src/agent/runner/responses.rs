//! Response handling for the agent runner.

use super::types::{
    AgentRunResult, AgentRunnerContext, FinalResponseInput, RunState, StructuredOutputFailure,
};
use super::AgentRunner;
use crate::agent::compaction::CompactionTrigger;
use crate::agent::memory::AgentMemory;
use crate::agent::progress::{AgentEvent, AgentEventSource};
use crate::agent::providers::TodoList;
use crate::agent::session::PendingUserInput;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::warn;

impl AgentRunner {
    /// Handle malformed structured output responses.
    pub(super) async fn handle_structured_output_error(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        failure: StructuredOutputFailure,
    ) -> anyhow::Result<Option<AgentRunResult>> {
        warn!(
            error = %failure.error,
            raw_preview = %crate::utils::truncate_str(&failure.raw_json, 200),
            "Structured output validation failed"
        );

        if should_salvage_structured_output_failure(&failure.raw_json) {
            warn!(
                raw_preview = %crate::utils::truncate_str(&failure.raw_json, 200),
                "Structured output failed but response looks like a final prose answer; salvaging without retry"
            );
            state.structured_output_failures = 0;
            let input = FinalResponseInput {
                final_answer: failure.raw_json,
                reasoning: None,
            };
            return self.handle_final_response(ctx, state, input).await;
        }

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
                        source: AgentEventSource::Root,
                        reason: "Too many JSON errors, falling back to raw response".to_string(),
                        count: state.continuation_count,
                    })
                    .await;
            }

            let input = FinalResponseInput {
                final_answer: failure.raw_json.clone(),
                reasoning: None,
            };

            return self.handle_final_response(ctx, state, input).await;
        }

        state.continuation_count += 1;
        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::Continuation {
                    source: AgentEventSource::Root,
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
        ctx.agent
            .memory_mut()
            .add_message(crate::agent::memory::AgentMessage::system_context(
                system_message,
            ));

        Ok(None)
    }

    fn save_final_response(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        rendered_response: &str,
        reasoning: Option<String>,
    ) {
        if let Some(reasoning_content) = reasoning {
            ctx.agent.memory_mut().add_message(
                crate::agent::memory::AgentMessage::assistant_with_reasoning(
                    rendered_response,
                    reasoning_content,
                ),
            );
        } else {
            ctx.agent
                .memory_mut()
                .add_message(crate::agent::memory::AgentMessage::assistant(
                    rendered_response,
                ));
        }
    }

    fn save_undelivered_final_response_draft(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        final_response: &str,
        reason: &str,
    ) {
        let trimmed = final_response.trim();
        if trimmed.is_empty() {
            return;
        }

        let notice = format!(
            "[SYSTEM: The previous assistant final response was not delivered to the user. \
Reason: {reason}. Use it only as internal context; if any of it is needed for the user, \
include it explicitly in a later final_answer.]\n\nUndelivered draft:\n{trimmed}"
        );
        ctx.messages.push(crate::llm::Message::system(&notice));
        ctx.agent
            .memory_mut()
            .add_message(crate::agent::memory::AgentMessage::undelivered_assistant_draft(notice));
    }

    /// Handle a final response payload and run after-agent hooks.
    pub(super) async fn handle_final_response(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        input: FinalResponseInput,
    ) -> anyhow::Result<Option<AgentRunResult>> {
        if ctx.agent.cancellation_token().is_cancelled() {
            return Err(self.cancelled_error(ctx).await);
        }

        let final_response = input.final_answer;

        sync_todos_from_arc(ctx.agent.memory_mut(), ctx.todos_arc).await;
        let hook_result = self.after_agent_hook_result(ctx, state, &final_response);

        if let crate::agent::hooks::HookResult::ForceIteration { reason, context } = hook_result {
            state.continuation_count += 1;
            if let Some(tx) = ctx.progress_tx {
                let _ = tx
                    .send(AgentEvent::Continuation {
                        source: AgentEventSource::Root,
                        reason: reason.clone(),
                        count: state.continuation_count,
                    })
                    .await;
            }
            let retry_message = format!("[SYSTEM: {reason}]\n\n{}", context.unwrap_or_default());
            self.save_undelivered_final_response_draft(
                ctx,
                &final_response,
                "completion hook forced another iteration",
            );
            ctx.messages
                .push(crate::llm::Message::system(&retry_message));
            ctx.agent
                .memory_mut()
                .add_message(crate::agent::memory::AgentMessage::system_context(
                    retry_message,
                ));
            let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
            Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;
            return Ok(None);
        }

        if ctx.agent.has_pending_runtime_context() {
            state.continuation_count += 1;
            if let Some(tx) = ctx.progress_tx {
                let _ = tx
                    .send(AgentEvent::Continuation {
                        source: AgentEventSource::Root,
                        reason: "New user context received, continuing the task.".to_string(),
                        count: state.continuation_count,
                    })
                    .await;
            }

            self.save_undelivered_final_response_draft(
                ctx,
                &final_response,
                "new user context arrived before delivery",
            );
            let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
            Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;
            return Ok(None);
        }

        self.save_final_response(ctx, &final_response, input.reasoning);
        let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
        Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;

        if let Some(tx) = ctx.progress_tx {
            if !ctx.config.is_sub_agent {
                let _ = tx.send(AgentEvent::Finished).await;
            }
        }
        Ok(Some(AgentRunResult::Final(final_response)))
    }

    /// Handle a blocked response that requires more user input.
    pub(super) async fn handle_waiting_for_user_input(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        _raw_json: String,
        reasoning: Option<String>,
        request: PendingUserInput,
    ) -> anyhow::Result<Option<AgentRunResult>> {
        if ctx.agent.cancellation_token().is_cancelled() {
            return Err(self.cancelled_error(ctx).await);
        }

        sync_todos_from_arc(ctx.agent.memory_mut(), ctx.todos_arc).await;
        self.save_final_response(ctx, &request.prompt, reasoning);
        let _ = state;
        let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
        Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;

        Ok(Some(AgentRunResult::WaitingForUserInput(request)))
    }
}

fn should_salvage_structured_output_failure(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return false;
    }

    if trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.starts_with("```")
        || trimmed.starts_with("<")
        || trimmed.starts_with("[SYSTEM:")
    {
        return false;
    }

    let has_sentence_content = trimmed.chars().filter(|ch| !ch.is_whitespace()).count() >= 24
        && trimmed.chars().any(char::is_alphabetic);
    if !has_sentence_content {
        return false;
    }

    let unfinished_tail = ['{', '[', ':', ',', '-', '"']
        .iter()
        .any(|tail| trimmed.ends_with(*tail));
    !unfinished_tail
}

async fn sync_todos_from_arc(memory: &mut AgentMemory, todos_arc: &Arc<Mutex<TodoList>>) {
    let current_todos = todos_arc.lock().await;
    memory.todos = (*current_todos).clone();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::AgentMessageKind;
    use crate::agent::context::{AgentContext, EphemeralSession};
    use crate::agent::hooks::CompletionCheckHook;
    use crate::agent::providers::{TodoItem, TodoList};
    use crate::agent::runner::test_support::{build_llm_client, single_final_response_provider};
    use crate::agent::runner::types::{FinalResponseInput, RunState};
    use crate::agent::runner::{AgentRunnerConfig, AgentRunnerContext};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn salvage_detector_accepts_plain_final_prose() {
        assert!(should_salvage_structured_output_failure(
            "**TL;DR**\n\nВ Молдове есть официальный режим для digital nomad с минимальным доходом около 52 200 MDL в месяц."
        ));
    }

    #[test]
    fn salvage_detector_rejects_json_like_or_truncated_content() {
        assert!(!should_salvage_structured_output_failure(
            r#"{"final_answer":"hello"}"#
        ));
        assert!(!should_salvage_structured_output_failure(
            "```json\n{}\n```"
        ));
        assert!(!should_salvage_structured_output_failure("tool_call:"));
        assert!(!should_salvage_structured_output_failure("short answer"));
    }

    #[tokio::test]
    async fn forced_final_response_is_saved_as_undelivered_draft() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(llm_client);
        runner.register_hook(Box::new(CompletionCheckHook::new()));

        let mut session = EphemeralSession::new(2048);
        let mut todos = TodoList::new();
        todos.items.push(TodoItem::new("finish work"));
        session.memory_mut().todos = todos.clone();
        let todos_arc = Arc::new(Mutex::new(todos));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "produce report",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "forced-final-draft",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096),
        };
        let mut state = RunState::new();
        let draft = "Full report generated before todos were complete.";

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: draft.to_string(),
                    reasoning: None,
                },
            )
            .await
            .expect("forced final response should continue");

        assert!(result.is_none());
        let memory = ctx.agent.memory().get_messages();
        assert!(
            !memory.iter().any(|message| {
                message.resolved_kind() == AgentMessageKind::AssistantResponse
                    && message.content.contains(draft)
            }),
            "forced final response must not be stored as delivered assistant prose"
        );
        let draft_message = memory
            .iter()
            .find(|message| message.resolved_kind() == AgentMessageKind::UndeliveredAssistantDraft)
            .expect("undelivered draft should be recorded");
        assert!(draft_message.content.contains("not delivered to the user"));
        assert!(draft_message.content.contains(draft));
    }
}
