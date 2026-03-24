//! Tool execution helpers for the agent runner.

use super::hooks::ToolHookDecision;
use super::types::{AgentRunResult, AgentRunnerContext, RunState};
use super::AgentRunner;
use crate::agent::compaction::CompactionTrigger;
use crate::agent::memory::AgentMessage;
use crate::agent::progress::AgentEvent;
use crate::agent::recovery::sanitize_xml_tags;
use crate::agent::tool_bridge::extract_updated_topic_agents_md;

use crate::llm::{InvocationId, Message, ToolCall, ToolCallFunction};
use std::fmt::Write as _;
use tracing::{info, warn};
use uuid::Uuid;

fn format_error_chain(error: &anyhow::Error) -> String {
    let mut output = String::new();
    for (idx, cause) in error.chain().enumerate() {
        if idx > 0 {
            output.push_str(" | caused by: ");
        }
        let _ = write!(&mut output, "{cause}");
    }
    output
}

impl AgentRunner {
    /// Build a tool call payload from validated structured output.
    pub(super) fn build_tool_call(
        &self,
        tool_call: crate::agent::structured_output::ValidatedToolCall,
    ) -> ToolCall {
        let invocation_id = InvocationId::new(format!("call_{}", Uuid::new_v4()));
        ToolCall::new(
            invocation_id.to_string(),
            ToolCallFunction {
                name: tool_call.name,
                arguments: tool_call.arguments_json,
            },
            false,
        )
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
        Self::refresh_messages_from_memory(ctx);
    }

    /// Execute all tool calls in parallel where possible.
    ///
    /// Tools that pass pre-execution hooks run concurrently. Results are
    /// processed sequentially to maintain deterministic ordering.
    pub(super) async fn execute_tools(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &RunState,
        tool_calls: Vec<ToolCall>,
    ) -> anyhow::Result<Option<AgentRunResult>> {
        // Phase 1: Sequential pre-processing - load skills and run hooks
        // This must be sequential because:
        // 1. Hooks may block tools or force finish (decisions affect flow)
        // 2. Skill loading may mutate context
        let mut approved_tools: Vec<(usize, ToolCall)> = Vec::with_capacity(tool_calls.len());
        let mut blocked_results: Vec<(usize, String)> = Vec::new();

        for (idx, tool_call) in tool_calls.iter().enumerate() {
            self.load_skill_context_for_tool(ctx, &tool_call.function.name)
                .await?;

            match self.apply_before_tool_hooks(ctx, state, tool_call)? {
                ToolHookDecision::Continue => {
                    approved_tools.push((idx, tool_call.clone()));
                }
                ToolHookDecision::Blocked { reason } => {
                    blocked_results.push((idx, reason));
                }
                ToolHookDecision::Finish { report } => {
                    return Ok(Some(AgentRunResult::Final(report)));
                }
            }
        }

        // Record blocked results for any tools that were blocked
        for (idx, reason) in blocked_results {
            let tool_call = &tool_calls[idx];
            self.record_blocked_tool_result(ctx, tool_call, &reason)
                .await;
            Self::emit_token_snapshot_update(
                ctx.progress_tx,
                Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration),
            )
            .await;
        }
        if approved_tools.is_empty() {
            return Ok(None);
        }

        // Phase 2: Parallel execution of approved tools
        // Execute raw tool calls in parallel through the registry
        let cancellation_token = ctx.agent.cancellation_token().clone();
        let tool_futures: Vec<_> = approved_tools
            .into_iter()
            .map(|(idx, tool_call)| {
                let registry = ctx.registry;
                let progress_tx = ctx.progress_tx.cloned();
                let cancellation_token = cancellation_token.clone();
                async move {
                    // Emit tool call event
                    if let Some(tx) = &progress_tx {
                        let sanitized_name = sanitize_xml_tags(&tool_call.function.name);
                        let sanitized_args = sanitize_xml_tags(&tool_call.function.arguments);
                        let _ = tx
                            .send(AgentEvent::ToolCall {
                                name: sanitized_name,
                                input: sanitized_args,
                                command_preview: None,
                            })
                            .await;
                    }

                    // Execute the tool
                    let result = registry
                        .execute(
                            &tool_call.function.name,
                            &tool_call.function.arguments,
                            progress_tx.as_ref(),
                            Some(&cancellation_token),
                        )
                        .await;

                    (idx, tool_call, result)
                }
            })
            .collect();

        let tool_results: Vec<(usize, ToolCall, anyhow::Result<String>)> =
            futures_util::future::join_all(tool_futures).await;

        // Phase 3: Sequential post-processing - handle results in original order
        // This ensures messages are added to context in deterministic order
        // and after hooks run in sequence
        let mut sorted_results = tool_results;
        sorted_results.sort_by_key(|(idx, _, _)| *idx);

        for (_idx, tool_call, result) in sorted_results {
            if self
                .record_tool_execution_result(ctx, state, tool_call, result)
                .await?
            {
                return Ok(Some(AgentRunResult::WaitingForApproval));
            }
        }

        Ok(None)
    }

    async fn record_tool_execution_result(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &RunState,
        tool_call: ToolCall,
        result: anyhow::Result<String>,
    ) -> anyhow::Result<bool> {
        let output = match result {
            Ok(output) => output,
            Err(e) => {
                warn!(
                    tool = %tool_call.function.name,
                    error = %e,
                    error_chain = %format_error_chain(&e),
                    "Tool execution failed"
                );
                format!("Tool execution error: {e}")
            }
        };

        if output.contains("APPROVAL_PENDING") || output.contains("Waiting for approval") {
            return Ok(true);
        }

        if let Some(tx) = ctx.progress_tx {
            let sanitized_name = sanitize_xml_tags(&tool_call.function.name);
            let _ = tx
                .send(AgentEvent::ToolResult {
                    name: sanitized_name,
                    output: output.clone(),
                })
                .await;
        }

        let tool_result = crate::agent::tool_bridge::ToolExecutionResult::Completed {
            tool_name: tool_call.function.name.clone(),
            output: output.clone(),
        };

        self.apply_after_tool_hooks(ctx, state, &tool_result);
        let tool_call_correlation = tool_call.correlation();
        let invocation_id = tool_call_correlation.invocation_id.clone();
        ctx.messages.push(Message::tool_with_correlation(
            invocation_id.as_str(),
            tool_call_correlation.clone(),
            &tool_call.function.name,
            &output,
        ));
        ctx.agent
            .memory_mut()
            .add_message(AgentMessage::tool_with_correlation(
                invocation_id.as_str(),
                tool_call_correlation,
                &tool_call.function.name,
                &output,
            ));
        Self::refresh_messages_from_memory(ctx);

        if let Some(agents_md) = extract_updated_topic_agents_md(&tool_call.function.name, &output)
        {
            ctx.agent.memory_mut().upsert_topic_agents_md(&agents_md);
            Self::refresh_messages_from_memory(ctx);
            ctx.agent.persist_memory_checkpoint_background();
        }

        let snapshot = Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration);
        Self::emit_token_snapshot_update(ctx.progress_tx, snapshot).await;

        if tool_call.function.name == "write_todos" {
            Self::sync_todos_after_tool(ctx).await;
        }

        Ok(false)
    }

    /// Sync todos from shared Arc to memory and emit TodosUpdated event.
    /// Called after write_todos tool execution to update UI.
    /// Note: Memory persistence is handled separately by tool_bridge's
    /// original sync path; skipping persist here avoids spawn overhead.
    async fn sync_todos_after_tool(ctx: &mut AgentRunnerContext<'_>) {
        // Sync todos from Arc to memory
        let current_todos = ctx.todos_arc.lock().await;
        ctx.agent.memory_mut().todos = (*current_todos).clone();
        drop(current_todos);

        // Emit TodosUpdated event for UI
        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::TodosUpdated {
                    todos: ctx.agent.memory().todos.clone(),
                })
                .await;
        }
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

        let invocation_id = tool_call.invocation_id();
        ctx.messages
            .push(Message::tool(invocation_id.as_str(), tool_name, &output));
        ctx.agent.memory_mut().add_message(AgentMessage::tool(
            invocation_id.as_str(),
            tool_name,
            &output,
        ));
        Self::refresh_messages_from_memory(ctx);
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

        let context_message = format!("[Loaded skill: {}]\n{}", skill.metadata.name, skill.content);

        ctx.agent
            .memory_mut()
            .add_message(AgentMessage::skill_context(context_message.clone()));
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
