//! Response handling for the agent runner.

use super::types::{
    AgentRunResult, AgentRunnerContext, FinalResponseInput, RunState, StructuredOutputFailure,
};
use super::AgentRunner;
use crate::agent::compaction::CompactionTrigger;
use crate::agent::progress::{AgentEvent, TokenSnapshot};
use crate::agent::session::PendingUserInput;
use crate::agent::tool_bridge::sync_todos_from_arc;
use crate::config::get_post_run_hot_context_target_tokens;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PostRunCleanupTelemetry {
    before_hot_tokens: usize,
    after_hot_tokens: usize,
    reclaimed_hot_tokens: usize,
    target_hot_tokens: usize,
    target_met: bool,
}

impl PostRunCleanupTelemetry {
    fn from_snapshots(before: &TokenSnapshot, after: &TokenSnapshot) -> Self {
        let target_hot_tokens = get_post_run_hot_context_target_tokens();
        let after_hot_tokens = after.hot_memory_tokens;

        Self {
            before_hot_tokens: before.hot_memory_tokens,
            after_hot_tokens,
            reclaimed_hot_tokens: before
                .hot_memory_tokens
                .saturating_sub(after.hot_memory_tokens),
            target_hot_tokens,
            target_met: after_hot_tokens <= target_hot_tokens,
        }
    }
}

impl AgentRunner {
    fn log_post_run_cleanup(
        task_id: &str,
        phase: &'static str,
        before: &TokenSnapshot,
        after: &TokenSnapshot,
    ) {
        let telemetry = PostRunCleanupTelemetry::from_snapshots(before, after);

        info!(
            task_id = %task_id,
            phase,
            hot_memory_tokens_before = telemetry.before_hot_tokens,
            hot_memory_tokens_after = telemetry.after_hot_tokens,
            reclaimed_hot_tokens = telemetry.reclaimed_hot_tokens,
            cleanup_target_tokens = telemetry.target_hot_tokens,
            cleanup_target_met = telemetry.target_met,
            budget_state_before = ?before.budget_state,
            budget_state_after = ?after.budget_state,
            projected_total_tokens_before = before.projected_total_tokens,
            projected_total_tokens_after = after.projected_total_tokens,
            headroom_tokens_before = before.headroom_tokens,
            headroom_tokens_after = after.headroom_tokens,
            "Post-run cleanup telemetry"
        );

        if !telemetry.target_met {
            warn!(
                task_id = %task_id,
                phase,
                hot_memory_tokens_after = telemetry.after_hot_tokens,
                cleanup_target_tokens = telemetry.target_hot_tokens,
                "Post-run cleanup left hot context above the target budget"
            );
        }
    }

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
                        reason: reason.clone(),
                        count: state.continuation_count,
                    })
                    .await;
            }
            let retry_message = format!("[SYSTEM: {reason}]\n\n{}", context.unwrap_or_default());
            ctx.messages
                .push(crate::llm::Message::assistant(&final_response));
            self.save_final_response(ctx, &final_response, input.reasoning);
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
                        reason: "New user context received, continuing the task.".to_string(),
                        count: state.continuation_count,
                    })
                    .await;
            }

            ctx.messages
                .push(crate::llm::Message::assistant(&final_response));
            self.save_final_response(ctx, &final_response, input.reasoning);
            let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
            Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;
            return Ok(None);
        }

        self.save_final_response(ctx, &final_response, input.reasoning);
        let pre_cleanup_snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PostRun);
        let post_cleanup_snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PostRun);
        Self::log_post_run_cleanup(
            ctx.task_id,
            "completed",
            &pre_cleanup_snapshot,
            &post_cleanup_snapshot,
        );
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
        let pre_cleanup_snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PostRun);
        let _ = state;
        let post_cleanup_snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PostRun);
        Self::log_post_run_cleanup(
            ctx.task_id,
            "waiting_for_user_input",
            &pre_cleanup_snapshot,
            &post_cleanup_snapshot,
        );
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

#[cfg(test)]
mod tests {
    use super::{should_salvage_structured_output_failure, PostRunCleanupTelemetry};
    use crate::agent::compaction::BudgetState;
    use crate::agent::progress::TokenSnapshot;
    use crate::config::get_post_run_hot_context_target_tokens;

    fn snapshot(hot_memory_tokens: usize) -> TokenSnapshot {
        TokenSnapshot {
            hot_memory_tokens,
            system_prompt_tokens: 1_024,
            tool_schema_tokens: 512,
            loaded_skill_tokens: 0,
            total_input_tokens: hot_memory_tokens + 1_536,
            reserved_output_tokens: 2_048,
            hard_reserve_tokens: 512,
            projected_total_tokens: hot_memory_tokens + 4_096,
            context_window_tokens: 128_000,
            headroom_tokens: 64_000,
            budget_state: BudgetState::Healthy,
            last_api_usage: None,
        }
    }

    #[test]
    fn post_run_cleanup_telemetry_marks_target_met() {
        let target = get_post_run_hot_context_target_tokens();
        let after = target.saturating_sub(256);
        let before = after + 10_000;
        let telemetry =
            PostRunCleanupTelemetry::from_snapshots(&snapshot(before), &snapshot(after));

        assert_eq!(telemetry.before_hot_tokens, before);
        assert_eq!(telemetry.after_hot_tokens, after);
        assert_eq!(telemetry.reclaimed_hot_tokens, 10_000);
        assert!(telemetry.target_met);
    }

    #[test]
    fn post_run_cleanup_telemetry_marks_target_miss() {
        let target = get_post_run_hot_context_target_tokens();
        let telemetry = PostRunCleanupTelemetry::from_snapshots(
            &snapshot(target + 10_000),
            &snapshot(target + 1),
        );

        assert_eq!(telemetry.target_hot_tokens, target);
        assert!(!telemetry.target_met);
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
}
