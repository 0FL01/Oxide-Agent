//! Hook handling for the agent runner.

use super::types::{AgentRunnerContext, RunState};
use crate::agent::hooks::{HookContext, HookEvent, HookResult};
use crate::agent::memory::AgentMessage;
use crate::llm::Message;

use super::AgentRunner;

impl AgentRunner {
    /// Apply hooks that run before the agent starts.
    pub(super) fn apply_before_agent_hooks(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
    ) -> anyhow::Result<()> {
        let hook_context = HookContext::new(
            &ctx.agent.memory().todos,
            0,
            0,
            ctx.config.continuation_limit,
        )
        .with_tokens(
            ctx.agent.memory().token_count(),
            ctx.agent.memory().max_tokens(),
        );

        let result = self.hook_registry.execute(
            &HookEvent::BeforeAgent {
                prompt: ctx.task.to_string(),
            },
            &hook_context,
        );

        self.apply_hook_result(result, ctx)
    }

    /// Apply hooks before a loop iteration begins.
    pub(super) fn apply_before_iteration_hooks(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &RunState,
    ) -> anyhow::Result<()> {
        let hook_context = HookContext::new(
            &ctx.agent.memory().todos,
            state.iteration,
            state.continuation_count,
            ctx.config.continuation_limit,
        )
        .with_tokens(
            ctx.agent.memory().token_count(),
            ctx.agent.memory().max_tokens(),
        );

        let result = self.hook_registry.execute(
            &HookEvent::BeforeIteration {
                iteration: state.iteration,
            },
            &hook_context,
        );

        self.apply_hook_result(result, ctx)
    }

    /// Apply hooks before executing a tool call.
    pub(super) fn apply_before_tool_hooks(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &RunState,
        tool_call: &crate::llm::ToolCall,
    ) -> anyhow::Result<()> {
        let hook_context = HookContext::new(
            &ctx.agent.memory().todos,
            state.iteration,
            state.continuation_count,
            ctx.config.continuation_limit,
        )
        .with_tokens(
            ctx.agent.memory().token_count(),
            ctx.agent.memory().max_tokens(),
        );

        let result = self.hook_registry.execute(
            &HookEvent::BeforeTool {
                tool_name: tool_call.function.name.clone(),
                arguments: tool_call.function.arguments.clone(),
            },
            &hook_context,
        );

        self.apply_hook_result(result, ctx)
    }

    /// Apply hooks after a tool call completes.
    pub(super) fn apply_after_tool_hooks(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &RunState,
        tool_result: &crate::agent::tool_bridge::ToolExecutionResult,
    ) {
        let hook_context = HookContext::new(
            &ctx.agent.memory().todos,
            state.iteration,
            state.continuation_count,
            ctx.config.continuation_limit,
        )
        .with_tokens(
            ctx.agent.memory().token_count(),
            ctx.agent.memory().max_tokens(),
        );

        let result = self.hook_registry.execute(
            &HookEvent::AfterTool {
                tool_name: tool_result.tool_name.clone(),
                result: tool_result.output.clone(),
            },
            &hook_context,
        );

        let _ = self.apply_hook_result(result, ctx);
    }

    /// Evaluate hooks after the agent produces a final response.
    pub(super) fn after_agent_hook_result(
        &self,
        ctx: &AgentRunnerContext<'_>,
        state: &RunState,
        final_response: &str,
    ) -> HookResult {
        let hook_context = HookContext::new(
            &ctx.agent.memory().todos,
            state.iteration,
            state.continuation_count,
            ctx.config.continuation_limit,
        )
        .with_tokens(
            ctx.agent.memory().token_count(),
            ctx.agent.memory().max_tokens(),
        );

        self.hook_registry.execute(
            &HookEvent::AfterAgent {
                response: final_response.to_string(),
            },
            &hook_context,
        )
    }

    fn apply_hook_result(
        &mut self,
        result: HookResult,
        ctx: &mut AgentRunnerContext<'_>,
    ) -> anyhow::Result<()> {
        match result {
            HookResult::Continue => Ok(()),
            HookResult::InjectContext(context) => {
                self.inject_system_context(ctx, context);
                Ok(())
            }
            HookResult::Block { reason } => Err(anyhow::anyhow!(reason)),
            HookResult::ForceIteration { .. } => Ok(()),
        }
    }

    fn inject_system_context(&mut self, ctx: &mut AgentRunnerContext<'_>, context: String) {
        ctx.messages.push(Message::system(&context));
        ctx.agent
            .memory_mut()
            .add_message(AgentMessage::system(context));
    }
}
