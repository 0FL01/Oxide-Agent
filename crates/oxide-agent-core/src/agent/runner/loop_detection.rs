//! Loop detection helpers for the agent runner.

use super::types::{AgentRunnerContext, RunState};
use super::AgentRunner;
use crate::agent::loop_detection::LoopType;
use crate::agent::progress::AgentEvent;
use tracing::{debug, warn};

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
            "Loop detected in agent execution"
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

    /// Check for LLM-based loop detection signals.
    pub(super) async fn llm_loop_detected(
        &self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &RunState,
    ) -> bool {
        let mut detector = self.loop_detector.lock().await;
        match detector
            .check_llm_periodic(ctx.agent.memory(), state.iteration)
            .await
        {
            Ok(detected) => detected,
            Err(err) => {
                warn!(error = %err, "LLM loop check failed, continuing without LLM signal");
                false
            }
        }
    }

    /// Check for repeated content loops.
    pub(super) async fn content_loop_detected(&self, content: &str) -> bool {
        let mut detector = self.loop_detector.lock().await;
        match detector.check_content(content) {
            Ok(detected) => detected,
            Err(err) => {
                warn!(error = %err, "Content loop check failed, continuing");
                false
            }
        }
    }

    /// Check for repeated tool call loops.
    pub(super) async fn tool_loop_detected(&self, tool_calls: &[crate::llm::ToolCall]) -> bool {
        let mut detector = self.loop_detector.lock().await;
        for tool_call in tool_calls {
            if tool_call.is_recovered {
                debug!(
                    tool_name = %tool_call.function.name,
                    "Skipping recovered tool call in loop detection"
                );
                detector.reset_content_tracking();
                continue;
            }
            match detector.check_tool_call(&tool_call.function.name, &tool_call.function.arguments)
            {
                Ok(true) => return true,
                Ok(false) => {}
                Err(err) => {
                    warn!(error = %err, "Tool loop check failed, continuing");
                }
            }
        }
        false
    }
}
