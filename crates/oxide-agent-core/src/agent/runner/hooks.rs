//! Hook handling for the agent runner.

use super::types::{AgentRunnerContext, RunState};
use crate::agent::hooks::{HookContext, HookEvent, HookResult};
use crate::agent::memory::AgentMessage;
use crate::llm::Message;

use super::AgentRunner;

pub(super) enum ToolHookDecision {
    Continue,
    Blocked { reason: String },
    Finish { report: String },
}

impl AgentRunner {
    /// Apply hooks that run before the agent starts.
    pub(super) fn apply_before_agent_hooks(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
    ) -> anyhow::Result<()> {
        let hook_context = HookContext::new(
            &ctx.agent.memory().todos,
            ctx.agent.memory(),
            0,
            0,
            ctx.config.continuation_limit,
        )
        .with_sub_agent(ctx.config.is_sub_agent)
        .with_available_tools(ctx.tools)
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

        self.apply_hook_result(result, ctx, None).map(|_| ())
    }

    /// Apply hooks before a loop iteration begins.
    pub(super) fn apply_before_iteration_hooks(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) -> anyhow::Result<()> {
        let hook_context = HookContext::new(
            &ctx.agent.memory().todos,
            ctx.agent.memory(),
            state.iteration,
            state.continuation_count,
            ctx.config.continuation_limit,
        )
        .with_sub_agent(ctx.config.is_sub_agent)
        .with_available_tools(ctx.tools)
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

        self.apply_hook_result(result, ctx, Some(state)).map(|_| ())
    }

    /// Apply hooks before executing a tool call.
    pub(super) fn apply_before_tool_hooks(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        tool_call: &crate::llm::ToolCall,
    ) -> anyhow::Result<ToolHookDecision> {
        let hook_context = HookContext::new(
            &ctx.agent.memory().todos,
            ctx.agent.memory(),
            state.iteration,
            state.continuation_count,
            ctx.config.continuation_limit,
        )
        .with_sub_agent(ctx.config.is_sub_agent)
        .with_available_tools(ctx.tools)
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

        match result {
            HookResult::Continue => Ok(ToolHookDecision::Continue),
            HookResult::InjectContext(context) => {
                self.inject_system_context(ctx, context);
                Ok(ToolHookDecision::Continue)
            }
            HookResult::InjectTransientContext(context) => {
                self.inject_transient_context(ctx, context);
                Ok(ToolHookDecision::Continue)
            }
            HookResult::ForceIteration { reason, context } => {
                if let Some(context) = context {
                    self.inject_system_context(ctx, context);
                }
                Ok(ToolHookDecision::Blocked { reason })
            }
            HookResult::RequestCompaction { reason: _, context } => {
                if let Some(context) = context {
                    self.inject_transient_context(ctx, context);
                }
                state.request_manual_compaction();
                Ok(ToolHookDecision::Continue)
            }
            HookResult::Block { reason } => Ok(ToolHookDecision::Blocked { reason }),
            HookResult::Finish(report) => Ok(ToolHookDecision::Finish { report }),
        }
    }

    /// Apply hooks after a tool call completes.
    pub(super) fn apply_after_tool_hooks(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        tool_result: &crate::agent::tool_bridge::ToolExecutionResult,
    ) {
        let crate::agent::tool_bridge::ToolExecutionResult::Completed { tool_name, output } =
            tool_result
        else {
            return;
        };

        let hook_context = HookContext::new(
            &ctx.agent.memory().todos,
            ctx.agent.memory(),
            state.iteration,
            state.continuation_count,
            ctx.config.continuation_limit,
        )
        .with_sub_agent(ctx.config.is_sub_agent)
        .with_available_tools(ctx.tools)
        .with_tokens(
            ctx.agent.memory().token_count(),
            ctx.agent.memory().max_tokens(),
        );

        let result = self.hook_registry.execute(
            &HookEvent::AfterTool {
                tool_name: tool_name.clone(),
                result: output.clone(),
            },
            &hook_context,
        );

        let _ = self.apply_hook_result(result, ctx, Some(state));
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
            ctx.agent.memory(),
            state.iteration,
            state.continuation_count,
            ctx.config.continuation_limit,
        )
        .with_sub_agent(ctx.config.is_sub_agent)
        .with_available_tools(ctx.tools)
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

    /// Apply timeout hooks when time limit is reached.
    pub(super) fn apply_timeout_hook(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) -> anyhow::Result<Option<String>> {
        let hook_context = HookContext::new(
            &ctx.agent.memory().todos,
            ctx.agent.memory(),
            state.iteration,
            state.continuation_count,
            ctx.config.continuation_limit,
        )
        .with_sub_agent(ctx.config.is_sub_agent)
        .with_available_tools(ctx.tools)
        .with_tokens(
            ctx.agent.memory().token_count(),
            ctx.agent.memory().max_tokens(),
        );

        let result = self
            .hook_registry
            .execute(&HookEvent::Timeout, &hook_context);

        self.apply_hook_result(result, ctx, Some(state))
    }

    fn apply_hook_result(
        &mut self,
        result: HookResult,
        ctx: &mut AgentRunnerContext<'_>,
        state: Option<&mut RunState>,
    ) -> anyhow::Result<Option<String>> {
        match result {
            HookResult::Continue => Ok(None),
            HookResult::InjectContext(context) => {
                self.inject_system_context(ctx, context);
                Ok(None)
            }
            HookResult::InjectTransientContext(context) => {
                self.inject_transient_context(ctx, context);
                Ok(None)
            }
            HookResult::RequestCompaction { context, .. } => {
                if let Some(context) = context {
                    self.inject_transient_context(ctx, context);
                }
                if let Some(state) = state {
                    state.request_manual_compaction();
                }
                Ok(None)
            }
            HookResult::Block { reason } => Err(anyhow::anyhow!(reason)),
            HookResult::ForceIteration { .. } => Ok(None),
            HookResult::Finish(report) => Ok(Some(report)),
        }
    }

    fn inject_system_context(&mut self, ctx: &mut AgentRunnerContext<'_>, context: String) {
        ctx.messages.push(Message::system(&context));
        ctx.agent
            .memory_mut()
            .add_message(AgentMessage::system_context(context));
    }

    fn inject_transient_context(&mut self, ctx: &mut AgentRunnerContext<'_>, context: String) {
        ctx.messages.push(Message::system(&context));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::context::{AgentContext, EphemeralSession};
    use crate::agent::runner::{AgentRunnerConfig, AgentRunnerContext};
    use crate::config::AgentSettings;
    use crate::llm::LlmClient;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn transient_context_is_not_persisted_to_memory() {
        let mut llm_client = LlmClient::new(&AgentSettings::default());
        llm_client.register_provider(
            "mock".to_string(),
            Arc::new(crate::testing::mock_llm_simple("ok")),
        );
        let mut runner = AgentRunner::new(Arc::new(llm_client));
        let registry = crate::agent::registry::ToolRegistry::new();
        let tools = registry.all_tools();
        let mut session = EphemeralSession::new(1024);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("test transient context"));
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let mut ctx = AgentRunnerContext {
            task: "test transient context",
            system_prompt: "system prompt",
            tools: &tools,
            registry: &registry,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "transient-hook-test",
            messages: &mut messages,
            agent: &mut session,
            skill_registry: None,
            compaction_service: None,
            config: AgentRunnerConfig::default(),
        };

        runner
            .apply_hook_result(
                HookResult::InjectTransientContext("temporary warning".to_string()),
                &mut ctx,
                None,
            )
            .expect("transient hook result should apply");

        assert!(ctx
            .messages
            .iter()
            .any(|message| message.role == "system" && message.content == "temporary warning"));
        assert!(!ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .any(|message| message.content == "temporary warning"));
    }
}
