//! Response handling for the agent runner.

use super::AgentRunner;
use super::types::{
    AgentRunResult, AgentRunnerContext, FinalResponseInput, RunState, StructuredOutputFailure,
};
use crate::agent::compaction::CompactionTrigger;
use crate::agent::memory::AgentMemory;
use crate::agent::progress::{AgentEvent, AgentEventSource};
use crate::agent::providers::TodoList;
use crate::agent::session::PendingUserInput;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

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

        let mut final_response = input.final_answer;
        let mut reasoning = input.reasoning;
        if let Some(draft) = state.pending_final_draft.take() {
            if draft.should_replace_final_response(&final_response) {
                info!(
                    task_id = ctx.task_id,
                    draft_len = draft.content_len(),
                    short_final_len = final_response.len(),
                    source_iteration = draft.source_iteration,
                    source_tool_name = draft.source_tool_name,
                    "using pending final draft instead of short final response"
                );
                final_response = draft.content;
                reasoning = None;
            } else {
                tracing::debug!(
                    task_id = ctx.task_id,
                    draft_len = draft.content_len(),
                    final_response_len = final_response.len(),
                    source_iteration = draft.source_iteration,
                    source_tool_name = draft.source_tool_name,
                    "discarding pending final draft because final response is substantive"
                );
            }
        }

        sync_todos_from_arc(ctx.agent.memory_mut(), ctx.todos_arc).await;
        let hook_result = self.after_agent_hook_result(ctx, state, &final_response);

        match hook_result {
            crate::agent::hooks::HookResult::ForceIteration { reason, context } => {
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
                let retry_message =
                    format!("[SYSTEM: {reason}]\n\n{}", context.unwrap_or_default());
                self.save_undelivered_final_response_draft(
                    ctx,
                    &final_response,
                    "completion hook forced another iteration",
                );
                ctx.messages
                    .push(crate::llm::Message::system(&retry_message));
                ctx.agent.memory_mut().add_message(
                    crate::agent::memory::AgentMessage::system_context(retry_message),
                );
                let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
                Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;
                return Ok(None);
            }
            crate::agent::hooks::HookResult::Block { reason } => {
                return Err(anyhow::anyhow!(reason));
            }
            crate::agent::hooks::HookResult::Finish(report) => {
                self.save_final_response(ctx, &report, None);
                let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
                Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;

                if let Some(tx) = ctx.progress_tx
                    && !ctx.config.is_sub_agent
                {
                    let _ = tx.send(AgentEvent::Finished).await;
                }
                return Ok(Some(AgentRunResult::Final(report)));
            }
            crate::agent::hooks::HookResult::Continue
            | crate::agent::hooks::HookResult::InjectContext(_)
            | crate::agent::hooks::HookResult::InjectTransientContext(_)
            | crate::agent::hooks::HookResult::RequestCompaction { .. } => {}
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

        self.save_final_response(ctx, &final_response, reasoning);
        let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
        Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;

        if let Some(tx) = ctx.progress_tx
            && !ctx.config.is_sub_agent
        {
            let _ = tx.send(AgentEvent::Finished).await;
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
    use crate::agent::hooks::{CompletionCheckHook, Hook, HookContext, HookEvent, HookResult};
    use crate::agent::providers::{TodoItem, TodoList};
    use crate::agent::runner::test_support::{build_llm_client, single_final_response_provider};
    use crate::agent::runner::types::{FinalResponseInput, PendingFinalDraft, RunState};
    use crate::agent::runner::{AgentRunnerConfig, AgentRunnerContext};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct StaticAfterAgentHook {
        result: HookResult,
    }

    impl Hook for StaticAfterAgentHook {
        fn name(&self) -> &'static str {
            "static_after_agent"
        }

        fn handle(&self, event: &HookEvent, _context: &HookContext) -> HookResult {
            match event {
                HookEvent::AfterAgent { .. } => self.result.clone(),
                _ => HookResult::Continue,
            }
        }
    }

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
            research_runtime: None,
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

    #[tokio::test]
    async fn after_agent_finish_overrides_final_response() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(llm_client);
        runner.register_hook(Box::new(StaticAfterAgentHook {
            result: HookResult::Finish("hook supplied report".to_string()),
        }));

        let mut session = EphemeralSession::new(2048);
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
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
            task_id: "after-agent-finish",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: None,
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096),
        };
        let mut state = RunState::new();

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: "original final".to_string(),
                    reasoning: Some("ignored reasoning".to_string()),
                },
            )
            .await
            .expect("finish hook should complete");

        match result {
            Some(AgentRunResult::Final(response)) => assert_eq!(response, "hook supplied report"),
            _ => panic!("expected overridden final response"),
        }
        let memory = ctx.agent.memory().get_messages();
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content == "hook supplied report"
        }));
        assert!(!memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content == "original final"
        }));
    }

    #[tokio::test]
    async fn after_agent_block_returns_error_without_saving_final_response() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(llm_client);
        runner.register_hook(Box::new(StaticAfterAgentHook {
            result: HookResult::Block {
                reason: "blocked final".to_string(),
            },
        }));

        let mut session = EphemeralSession::new(2048);
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
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
            task_id: "after-agent-block",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: None,
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096),
        };
        let mut state = RunState::new();

        let error = match runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: "original final".to_string(),
                    reasoning: None,
                },
            )
            .await
        {
            Ok(_) => panic!("block hook should fail final response"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("blocked final"));
        let memory = ctx.agent.memory().get_messages();
        assert!(
            !memory
                .iter()
                .any(|message| message.resolved_kind() == AgentMessageKind::AssistantResponse)
        );
    }

    #[tokio::test]
    async fn pending_final_draft_replaces_short_final_response() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(llm_client);
        let mut session = EphemeralSession::new(4096);
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
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
            task_id: "pending-final-draft-replace",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: None,
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096),
        };
        let long_draft = format!(
            "## Итоговый отчёт\n\n{}",
            "- Модель: https://huggingface.co/example/model — годна после проверки.\n".repeat(80)
        );
        let mut state = RunState::new();
        state.pending_final_draft =
            PendingFinalDraft::from_write_todos_content(long_draft.clone(), 18);

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: "Ресёрч завершён. Если нужна детализация — спрашивай."
                        .to_string(),
                    reasoning: None,
                },
            )
            .await
            .expect("final response should be handled");

        match result {
            Some(AgentRunResult::Final(response)) => assert_eq!(response, long_draft.trim()),
            _ => panic!("expected final response"),
        }
        assert!(state.pending_final_draft.is_none());
        let memory = ctx.agent.memory().get_messages();
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content == long_draft.trim()
        }));
    }

    #[tokio::test]
    async fn pending_final_draft_does_not_replace_substantive_final_response() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(llm_client);
        let mut session = EphemeralSession::new(4096);
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
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
            task_id: "pending-final-draft-keep-stop",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            research_runtime: None,
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096),
        };
        let draft = format!("## Старый draft\n\n{}", "draft line\n".repeat(120));
        let final_answer = format!("## Новый финальный ответ\n\n{}", "final line\n".repeat(120));
        let mut state = RunState::new();
        state.pending_final_draft = PendingFinalDraft::from_write_todos_content(draft, 18);

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: final_answer.clone(),
                    reasoning: None,
                },
            )
            .await
            .expect("final response should be handled");

        match result {
            Some(AgentRunResult::Final(response)) => assert_eq!(response, final_answer),
            _ => panic!("expected final response"),
        }
        assert!(state.pending_final_draft.is_none());
        let memory = ctx.agent.memory().get_messages();
        assert!(memory.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::AssistantResponse
                && message.content == final_answer
        }));
    }
}
