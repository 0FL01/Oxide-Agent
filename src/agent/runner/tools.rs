//! Tool execution helpers for the agent runner.

use super::hooks::ToolHookDecision;
use super::types::{AgentRunnerContext, RunState};
use super::AgentRunner;
use crate::agent::memory::AgentMessage;
use crate::agent::progress::AgentEvent;
use crate::agent::recovery::sanitize_xml_tags;
use crate::agent::tool_bridge::{execute_single_tool_call, ToolExecutionContext};
use crate::llm::{Message, ToolCall, ToolCallFunction};
use tracing::{info, warn};
use uuid::Uuid;

impl AgentRunner {
    /// Build a tool call payload from validated structured output.
    pub(super) fn build_tool_call(
        &self,
        tool_call: crate::agent::structured_output::ValidatedToolCall,
    ) -> ToolCall {
        ToolCall {
            id: format!("call_{}", Uuid::new_v4()),
            function: ToolCallFunction {
                name: tool_call.name,
                arguments: tool_call.arguments_json,
            },
            is_recovered: false,
        }
    }

    /// Record a tool call in both the LLM message log and memory.
    pub(super) fn record_assistant_tool_call(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        raw_json: &str,
        tool_calls: &[ToolCall],
    ) {
        let tool_calls_vec = tool_calls.to_vec();
        ctx.messages.push(Message::assistant_with_tools(
            raw_json,
            tool_calls_vec.clone(),
        ));
        ctx.agent
            .memory_mut()
            .add_message(AgentMessage::assistant_with_tools(
                raw_json.to_string(),
                tool_calls_vec,
            ));
    }

    /// Execute all tool calls in sequence.
    pub(super) async fn execute_tools(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &RunState,
        tool_calls: Vec<ToolCall>,
    ) -> anyhow::Result<()> {
        for tool_call in &tool_calls {
            self.load_skill_context_for_tool(ctx, &tool_call.function.name)
                .await?;
            match self.apply_before_tool_hooks(ctx, state, tool_call)? {
                ToolHookDecision::Continue => {}
                ToolHookDecision::Blocked { reason } => {
                    self.record_blocked_tool_result(ctx, tool_call, &reason)
                        .await;
                    continue;
                }
            }
            let cancellation_token = ctx.agent.cancellation_token().clone();
            let memory = ctx.agent.memory_mut();
            let mut tool_ctx = ToolExecutionContext {
                registry: ctx.registry,
                progress_tx: ctx.progress_tx,
                todos_arc: ctx.todos_arc,
                messages: ctx.messages,
                memory,
                cancellation_token,
            };
            let tool_result = execute_single_tool_call(tool_call.clone(), &mut tool_ctx).await?;
            self.apply_after_tool_hooks(ctx, state, &tool_result);
        }
        Ok(())
    }

    async fn record_blocked_tool_result(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        tool_call: &ToolCall,
        reason: &str,
    ) {
        let tool_name = &tool_call.function.name;
        let tool_args = &tool_call.function.arguments;
        let output = format!("⛔ Tool call blocked by policy.\n{reason}");

        if let Some(tx) = ctx.progress_tx {
            let sanitized_name = sanitize_xml_tags(tool_name);
            let sanitized_args = sanitize_xml_tags(tool_args);
            let command_preview = if tool_name == "execute_command" {
                Self::extract_command_preview(tool_args)
            } else {
                None
            };

            let _ = tx
                .send(AgentEvent::ToolCall {
                    name: sanitized_name.clone(),
                    input: sanitized_args,
                    command_preview,
                })
                .await;
            let _ = tx
                .send(AgentEvent::ToolResult {
                    name: sanitized_name,
                    output: output.clone(),
                })
                .await;
        }

        ctx.messages
            .push(Message::tool(&tool_call.id, tool_name, &output));
        ctx.agent
            .memory_mut()
            .add_message(AgentMessage::tool(&tool_call.id, tool_name, &output));
    }

    fn extract_command_preview(arguments: &str) -> Option<String> {
        serde_json::from_str::<serde_json::Value>(arguments)
            .ok()
            .and_then(|value| {
                value
                    .get("command")
                    .and_then(|command| command.as_str())
                    .map(str::to_string)
            })
    }

    async fn load_skill_context_for_tool(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        tool_name: &str,
    ) -> anyhow::Result<()> {
        let Some(registry) = ctx.skill_registry.as_mut() else {
            return Ok(());
        };

        let skill = match registry.load_skill_for_tool(tool_name).await {
            Ok(skill) => skill,
            Err(err) => {
                warn!(tool_name = %tool_name, error = %err, "Failed to load skill for tool");
                return Ok(());
            }
        };

        let Some(skill) = skill else {
            return Ok(());
        };

        if ctx.agent.is_skill_loaded(&skill.metadata.name) {
            return Ok(());
        }

        let context_message = format!(
            "[Loaded skill: {}]\n{}",
            skill.metadata.name, skill.content
        );

        ctx.agent
            .memory_mut()
            .add_message(AgentMessage::system(context_message.clone()));
        ctx.messages.push(Message::system(&context_message));

        if ctx
            .agent
            .register_loaded_skill(&skill.metadata.name, skill.token_count)
        {
            info!(
                skill = %skill.metadata.name,
                memory_tokens = ctx.agent.memory().token_count(),
                "Dynamic skill loaded"
            );
        }

        Ok(())
    }
}
