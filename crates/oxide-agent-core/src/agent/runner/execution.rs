//! Core execution loop for the agent runner.

use super::types::{AgentRunResult, AgentRunnerContext, RunState};
use super::AgentRunner;
use crate::agent::compaction::CompactionTrigger;
use crate::agent::memory::AgentMessage;
use crate::agent::progress::AgentEvent;
use anyhow::{anyhow, Result};
use std::future::Future;
use tracing::debug;

impl AgentRunner {
    /// Execute the agent loop until completion or error.
    pub async fn run(&mut self, ctx: &mut AgentRunnerContext<'_>) -> Result<AgentRunResult> {
        self.reset_loop_detector(ctx).await;
        self.apply_before_agent_hooks(ctx)?;
        self.run_loop(ctx).await
    }

    async fn run_loop(&mut self, ctx: &mut AgentRunnerContext<'_>) -> Result<AgentRunResult> {
        let mut state = RunState::new();

        for iteration in 0..ctx.config.max_iterations {
            state.iteration = iteration;

            if ctx.agent.cancellation_token().is_cancelled() {
                return Err(self.cancelled_error(ctx).await);
            }

            if ctx.agent.elapsed_secs() >= ctx.config.timeout_secs {
                if let Some(res) = self.apply_timeout_hook(ctx, &mut state)? {
                    return Ok(AgentRunResult::Final(res));
                }
            }

            self.apply_pending_runtime_context(ctx, &mut state).await;

            self.run_pre_llm_maintenance(ctx, &mut state, iteration)
                .await?;

            debug!(task_id = %ctx.task_id, iteration = iteration, "Agent loop iteration");

            let snapshot_trigger = if iteration == 0 {
                CompactionTrigger::PreRun
            } else {
                CompactionTrigger::PreIteration
            };
            let snapshot = Self::build_token_snapshot(ctx, snapshot_trigger);
            Self::log_token_snapshot(ctx, iteration, "before_llm_call", &snapshot);
            if let Some(tx) = ctx.progress_tx {
                let _ = tx.send(AgentEvent::Thinking { snapshot }).await;
                // Emit milestone when Thinking event is sent (not just received).
                let timestamp_ms = chrono::Utc::now().timestamp_millis();
                let _ = tx
                    .send(AgentEvent::Milestone {
                        name: "thinking_sent".to_string(),
                        timestamp_ms,
                    })
                    .await;
            }

            let cancellation_token = ctx.agent.cancellation_token().clone();
            let Some(loop_detected) = Self::await_until_cancelled(
                cancellation_token,
                self.llm_loop_detected(ctx, &state),
            )
            .await
            else {
                return Err(self.cancelled_error(ctx).await);
            };
            if loop_detected {
                return Err(self
                    .loop_detected_error(
                        ctx,
                        &state,
                        crate::agent::loop_detection::LoopType::CognitiveLoop,
                    )
                    .await);
            }

            let cancellation_token = ctx.agent.cancellation_token().clone();
            let Some(response) = Self::await_until_cancelled(
                cancellation_token,
                self.call_llm_with_tools(ctx, &mut state, iteration),
            )
            .await
            else {
                return Err(self.cancelled_error(ctx).await);
            };
            let response = response?;
            if let Some(result) = self.handle_llm_response(response, ctx, &mut state).await? {
                return Ok(result);
            }
        }

        Err(anyhow!(
            "Agent exceeded iteration limit ({}).",
            ctx.config.max_iterations
        ))
    }

    async fn run_pre_llm_maintenance(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        iteration: usize,
    ) -> Result<()> {
        self.apply_before_iteration_hooks(ctx, state)?;

        let cancellation_token = ctx.agent.cancellation_token().clone();
        if state.take_manual_compaction_request() {
            let Some(compaction_result) = Self::await_until_cancelled(
                cancellation_token.clone(),
                self.run_manual_compaction_checkpoint(ctx, state),
            )
            .await
            else {
                return Err(self.cancelled_error(ctx).await);
            };
            compaction_result?;
        } else {
            let Some(compaction_result) = Self::await_until_cancelled(
                cancellation_token,
                self.run_iteration_compaction(ctx, state, iteration),
            )
            .await
            else {
                return Err(self.cancelled_error(ctx).await);
            };
            compaction_result?;
        }

        if iteration == 0 {
            Self::emit_pre_run_compaction_done(ctx.progress_tx).await;
        }

        Ok(())
    }

    async fn emit_pre_run_compaction_done(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) {
        let Some(tx) = progress_tx else { return };

        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        let _ = tx
            .send(AgentEvent::Milestone {
                name: "pre_run_compaction_done".to_string(),
                timestamp_ms,
            })
            .await;
    }

    async fn await_until_cancelled<T, F>(
        cancellation_token: tokio_util::sync::CancellationToken,
        future: F,
    ) -> Option<T>
    where
        F: Future<Output = T>,
    {
        tokio::pin!(future);

        tokio::select! {
            result = &mut future => Some(result),
            _ = cancellation_token.cancelled() => None,
        }
    }

    async fn apply_pending_runtime_context(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) {
        let pending_context = ctx.agent.drain_runtime_context();
        if pending_context.is_empty() {
            return;
        }

        state.continuation_count += 1;
        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::Continuation {
                    reason: "New user context received, adapting the plan.".to_string(),
                    count: state.continuation_count,
                })
                .await;
        }

        for injection in pending_context {
            ctx.messages
                .push(crate::llm::Message::user(&injection.content));
            ctx.agent
                .memory_mut()
                .add_message(AgentMessage::runtime_context(injection.content));
        }
    }

    pub(super) async fn cancelled_error(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
    ) -> anyhow::Error {
        if let Some(tx) = ctx.progress_tx {
            let _ = tx.send(AgentEvent::Cancelled).await;
        }

        anyhow!("Task cancelled by user")
    }

    // Additional response and runtime helpers live in sibling runner modules.
}
