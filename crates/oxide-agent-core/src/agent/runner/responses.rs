//! Response handling for the agent runner.

use super::types::{
    AgentRunResult, AgentRunnerContext, FinalResponseInput, RunState, StructuredOutputFailure,
};
use super::AgentRunner;
use crate::agent::compaction::CompactionTrigger;
use crate::agent::memory::AgentMessage;
use crate::agent::persistent_memory::{PersistentRunContext, PersistentRunPhase};
use crate::agent::progress::{AgentEvent, TokenSnapshot};
use crate::agent::session::PendingUserInput;
use crate::agent::tool_bridge::sync_todos_from_arc;
use tracing::{info, warn};

const POST_RUN_HOT_CONTEXT_TARGET_TOKENS: usize = 16 * 1024;

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
        let target_hot_tokens = POST_RUN_HOT_CONTEXT_TARGET_TOKENS;
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

    async fn persist_post_run_memory(
        &self,
        ctx: &mut AgentRunnerContext<'_>,
        phase: PersistentRunPhase<'_>,
        pre_compaction_messages: Option<&[AgentMessage]>,
    ) {
        if ctx.config.is_sub_agent {
            return;
        }

        let (Some(persistent_memory), Some(session_id), Some(scope)) = (
            ctx.persistent_memory,
            ctx.session_id.as_deref(),
            ctx.memory_scope.as_ref(),
        ) else {
            return;
        };
        let tool_memory_drafts = ctx
            .memory_behavior
            .as_ref()
            .map(|runtime| runtime.snapshot())
            .unwrap_or_default();

        // Use pre-compaction messages when available (PostRun path): compaction may have
        // truncated the live messages to a single summary, losing all the original
        // user turns, tool results and artifacts that the episode finalizer needs.
        let messages = pre_compaction_messages.unwrap_or_else(|| ctx.agent.memory().get_messages());

        if let Err(error) = persistent_memory
            .persist_post_run(PersistentRunContext {
                session_id,
                task_id: ctx.task_id,
                scope,
                task: ctx.task,
                messages,
                hot_token_estimate: ctx.agent.memory().token_count(),
                tool_memory_drafts,
                phase,
            })
            .await
        {
            warn!(error = %error, task_id = %ctx.task_id, "Persistent memory post-run write failed");
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
                .push(crate::llm::Message::assistant(&input.raw_json));
            self.save_final_response(ctx, &input.raw_json, input.reasoning);
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
                .push(crate::llm::Message::assistant(&input.raw_json));
            self.save_final_response(ctx, &input.raw_json, input.reasoning);
            let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
            Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;
            return Ok(None);
        }

        self.save_final_response(ctx, &input.raw_json, input.reasoning);
        // Snapshot messages before PostRun compaction — the truncation will wipe the
        // live message list but the episode finalizer needs the original history to
        // extract artifacts, tools used and summary signal.
        let pre_compaction_messages: Vec<AgentMessage> = ctx.agent.memory().get_messages().to_vec();
        let pre_cleanup_snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PostRun);
        let _ = self
            .run_compaction_checkpoint(ctx, state, CompactionTrigger::PostRun)
            .await?;
        let post_cleanup_snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PostRun);
        Self::log_post_run_cleanup(
            ctx.task_id,
            "completed",
            &pre_cleanup_snapshot,
            &post_cleanup_snapshot,
        );
        let mut durable_messages = pre_compaction_messages;
        durable_messages.extend(
            ctx.agent
                .memory()
                .get_messages()
                .iter()
                .filter(|message| {
                    message.summary_payload().is_some() || message.archive_ref_payload().is_some()
                })
                .cloned(),
        );
        self.persist_post_run_memory(
            ctx,
            PersistentRunPhase::Completed {
                final_answer: &final_response,
            },
            Some(&durable_messages),
        )
        .await;
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
        raw_json: String,
        reasoning: Option<String>,
        request: PendingUserInput,
    ) -> anyhow::Result<Option<AgentRunResult>> {
        if ctx.agent.cancellation_token().is_cancelled() {
            return Err(self.cancelled_error(ctx).await);
        }

        sync_todos_from_arc(ctx.agent.memory_mut(), ctx.todos_arc).await;
        self.save_final_response(ctx, &raw_json, reasoning);
        let pre_compaction_messages: Vec<AgentMessage> = ctx.agent.memory().get_messages().to_vec();
        let pre_cleanup_snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PostRun);
        let _ = self
            .run_compaction_checkpoint(ctx, state, CompactionTrigger::PostRun)
            .await?;
        let post_cleanup_snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PostRun);
        Self::log_post_run_cleanup(
            ctx.task_id,
            "waiting_for_user_input",
            &pre_cleanup_snapshot,
            &post_cleanup_snapshot,
        );
        let mut durable_messages = pre_compaction_messages;
        durable_messages.extend(
            ctx.agent
                .memory()
                .get_messages()
                .iter()
                .filter(|message| {
                    message.summary_payload().is_some() || message.archive_ref_payload().is_some()
                })
                .cloned(),
        );
        self.persist_post_run_memory(
            ctx,
            PersistentRunPhase::WaitingForUserInput,
            Some(&durable_messages),
        )
        .await;
        let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
        Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;

        Ok(Some(AgentRunResult::WaitingForUserInput(request)))
    }
}

#[cfg(test)]
mod tests {
    use super::{PostRunCleanupTelemetry, POST_RUN_HOT_CONTEXT_TARGET_TOKENS};
    use crate::agent::compaction::BudgetState;
    use crate::agent::progress::TokenSnapshot;

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
        let telemetry = PostRunCleanupTelemetry::from_snapshots(
            &snapshot(48_000),
            &snapshot(POST_RUN_HOT_CONTEXT_TARGET_TOKENS - 256),
        );

        assert_eq!(telemetry.before_hot_tokens, 48_000);
        assert_eq!(
            telemetry.after_hot_tokens,
            POST_RUN_HOT_CONTEXT_TARGET_TOKENS - 256
        );
        assert_eq!(telemetry.reclaimed_hot_tokens, 31_872);
        assert!(telemetry.target_met);
    }

    #[test]
    fn post_run_cleanup_telemetry_marks_target_miss() {
        let telemetry = PostRunCleanupTelemetry::from_snapshots(
            &snapshot(48_000),
            &snapshot(POST_RUN_HOT_CONTEXT_TARGET_TOKENS + 1),
        );

        assert_eq!(
            telemetry.target_hot_tokens,
            POST_RUN_HOT_CONTEXT_TARGET_TOKENS
        );
        assert!(!telemetry.target_met);
    }
}
