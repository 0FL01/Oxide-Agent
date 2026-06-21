//! Loop detection helpers for the agent runner.

use super::AgentRunner;
use super::types::{AgentRunnerContext, RunState};
use crate::agent::loop_detection::{LoopDetectionOutcome, LoopType};
use crate::agent::memory::AgentMessage;
use crate::agent::progress::AgentEvent;
use crate::llm::Message;
use tracing::warn;

impl AgentRunner {
    /// Reset loop detector state for the current task.
    pub(super) async fn reset_loop_detector(&mut self, ctx: &mut AgentRunnerContext<'_>) {
        let mut detector = self.loop_detector.lock().await;
        detector.reset(ctx.task_id.to_string());
        if self.loop_detection_disabled_next_run {
            detector.disable_for_session();
            self.loop_detection_disabled_next_run = false;
        }
    }

    /// Build and emit a loop-detected error.
    ///
    /// Cancels the agent's cancellation token to ensure the run loop terminates
    /// before the UI notification reaches the user (fixes race condition with reset).
    pub(super) async fn loop_detected_error(
        &self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &RunState,
        loop_type: LoopType,
    ) -> anyhow::Error {
        let event = {
            let detector = self.loop_detector.lock().await;
            detector.create_event(loop_type, state.iteration)
        };

        warn!(
            session_id = %event.session_id,
            loop_type = ?event.loop_type,
            iteration = event.iteration,
            "Loop detected in agent execution (halt)"
        );

        // Cancel the token BEFORE sending the UI event.
        // This ensures the run loop terminates before user can press "Reset".
        ctx.agent.cancellation_token().cancel();

        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::LoopDetected {
                    loop_type,
                    iteration: state.iteration,
                })
                .await;
        }

        anyhow::anyhow!("Loop detected: {:?}", loop_type)
    }

    /// Handle a loop detection outcome.
    ///
    /// - `NoLoop`: no action, returns `Ok(None)`.
    /// - `RePrompt`: injects "you are looping" context into memory and messages,
    ///   returns `Ok(None)` so the caller continues iterating (skips tool execution
    ///   or final answer acceptance).
    /// - `Halt`: cancels the run and returns `Err`.
    pub(super) async fn handle_loop_outcome(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &RunState,
        outcome: LoopDetectionOutcome,
    ) -> anyhow::Result<Option<super::types::AgentRunResult>> {
        match outcome {
            LoopDetectionOutcome::NoLoop => Ok(None),
            LoopDetectionOutcome::RePrompt { context, loop_type } => {
                warn!(
                    session_id = %ctx.task_id,
                    loop_type = ?loop_type,
                    iteration = state.iteration,
                    "Loop detected, injecting re-prompt remediation"
                );

                // Inject the re-prompt as a persistent system context so the LLM
                // sees it on the next call. Using system_context (not transient)
                // because refresh_messages_from_memory rebuilds ctx.messages from
                // memory before each LLM call, so transient messages are dropped.
                ctx.messages.push(Message::system(&context));
                ctx.agent
                    .memory_mut()
                    .add_message(AgentMessage::system_context(context));

                Ok(None)
            }
            LoopDetectionOutcome::Halt { loop_type } => {
                Err(self.loop_detected_error(ctx, state, loop_type).await)
            }
        }
    }

    /// Check for LLM-based loop detection signals.
    pub(super) async fn llm_loop_outcome(
        &self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &RunState,
    ) -> LoopDetectionOutcome {
        let mut detector = self.loop_detector.lock().await;
        match detector
            .check_llm_periodic(ctx.agent.memory(), state.iteration)
            .await
        {
            Ok(outcome) => outcome,
            Err(err) => {
                warn!(error = %err, "LLM loop check failed, continuing without LLM signal");
                LoopDetectionOutcome::NoLoop
            }
        }
    }

    /// Check for repeated content loops.
    pub(super) async fn content_loop_outcome(&self, content: &str) -> LoopDetectionOutcome {
        let mut detector = self.loop_detector.lock().await;
        match detector.check_content(content) {
            Ok(outcome) => outcome,
            Err(err) => {
                warn!(error = %err, "Content loop check failed, continuing");
                LoopDetectionOutcome::NoLoop
            }
        }
    }

    /// Check for repeated tool call loops.
    ///
    /// All tool calls feed into the detector, including recovered ones.
    /// The `is_recovered` bypass was removed — recovered calls that repeat
    /// the same action are just as much a loop as non-recovered ones.
    pub(super) async fn tool_loop_outcome(
        &self,
        tool_calls: &[crate::llm::ToolCall],
    ) -> LoopDetectionOutcome {
        let mut detector = self.loop_detector.lock().await;
        for tool_call in tool_calls {
            match detector.check_tool_call(&tool_call.function.name, &tool_call.function.arguments)
            {
                Ok(LoopDetectionOutcome::NoLoop) => {}
                Ok(outcome) => return outcome,
                Err(err) => {
                    warn!(error = %err, "Tool loop check failed, continuing");
                }
            }
        }
        LoopDetectionOutcome::NoLoop
    }
}
