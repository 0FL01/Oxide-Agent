//! Tool bridge module
//!
//! Handles tool execution with timeout, cancellation support, and progress events.

use super::memory::AgentMemory;
use super::memory::AgentMessage;
use super::progress::AgentEvent;
use super::providers::TodoList;
use super::recovery::sanitize_xml_tags;
use super::registry::ToolRegistry;
use crate::config::AGENT_TOOL_TIMEOUT_SECS;
use crate::llm::{Message, ToolCall};
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

/// Context for tool execution
pub struct ToolExecutionContext<'a> {
    /// Tool registry for executing tools
    pub registry: &'a ToolRegistry,
    /// Channel for sending progress events
    pub progress_tx: Option<&'a tokio::sync::mpsc::Sender<AgentEvent>>,
    /// Shared todo list
    pub todos_arc: &'a Arc<Mutex<TodoList>>,
    /// Messages for the LLM conversation
    pub messages: &'a mut Vec<Message>,
    /// Mutable access to agent memory
    pub memory: &'a mut AgentMemory,
    /// Cancellation token for the current task
    pub cancellation_token: tokio_util::sync::CancellationToken,
    /// Current delegation depth propagated from runner context.
    pub delegation_depth: usize,
}

/// Result of executing a tool call.
pub struct ToolExecutionResult {
    /// Name of the tool that was executed.
    pub tool_name: String,
    /// Output produced by the tool.
    pub output: String,
}

/// Execute a list of tool calls
pub async fn execute_tool_calls(
    tool_calls: Vec<ToolCall>,
    ctx: &mut ToolExecutionContext<'_>,
) -> Result<Vec<ToolExecutionResult>> {
    let mut results = Vec::with_capacity(tool_calls.len());
    for tool_call in tool_calls {
        results.push(execute_single_tool_call(tool_call, ctx).await?);
    }
    Ok(results)
}

/// Execute a single tool call with timeout and cancellation support
pub async fn execute_single_tool_call(
    tool_call: ToolCall,
    ctx: &mut ToolExecutionContext<'_>,
) -> Result<ToolExecutionResult> {
    // Check for cancellation before execution
    if ctx.cancellation_token.is_cancelled() {
        return Err(anyhow::anyhow!("Task cancelled by user"));
    }

    let ToolCall { id, function, .. } = tool_call;
    let name = function.name;
    let args = inject_delegation_depth_argument(&name, &function.arguments, ctx.delegation_depth);

    info!(
        tool_name = %name,
        tool_args = %crate::utils::truncate_str(&args, 200),
        "Executing tool call"
    );

    if let Some(tx) = ctx.progress_tx {
        // Extract command preview for execute_command tool
        let command_preview = if name == "execute_command" {
            extract_command_preview(&args)
        } else {
            None
        };

        // Sanitize XML tags from tool name and input to prevent UI corruption
        // This protects against malformed LLM responses that leak XML syntax
        let _ = tx
            .send(AgentEvent::ToolCall {
                name: sanitize_xml_tags(&name),
                input: sanitize_xml_tags(&args),
                command_preview,
            })
            .await;
    }

    // Execute tool with timeout and cancellation support
    let tool_timeout = Duration::from_secs(AGENT_TOOL_TIMEOUT_SECS);
    let result = {
        use tokio::select;
        select! {
            biased;
            _ = ctx.cancellation_token.cancelled() => {
                warn!(tool_name = %name, "Tool execution cancelled by user");
                if let Some(tx) = ctx.progress_tx {
                    let _ = tx.send(AgentEvent::Cancelling { tool_name: name.clone() }).await;
                    // Give UI time to show cancelling status (2 sec cleanup timeout)
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
                return Err(anyhow::anyhow!("Task cancelled by user"));
            },
            res = timeout(tool_timeout, ctx.registry.execute(&name, &args, ctx.progress_tx, Some(&ctx.cancellation_token))) => {
                match res {
                    Ok(Ok(r)) => r,
                    Ok(Err(e)) => format!("Tool execution error: {e}"),
                    Err(_) => {
                        warn!(
                            tool_name = %name,
                            timeout_secs = AGENT_TOOL_TIMEOUT_SECS,
                            "Tool execution timed out"
                        );
                        format!(
                            "Tool '{name}' timed out ({} seconds)",
                            AGENT_TOOL_TIMEOUT_SECS
                        )
                    }
                }
            },
        }
    };

    // Sync todos if write_todos was called
    if name == "write_todos" {
        sync_todos_from_arc(ctx.memory, ctx.todos_arc).await;
        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::TodosUpdated {
                    todos: ctx.memory.todos.clone(),
                })
                .await;
        }
    }

    // Send tool result event
    if let Some(tx) = ctx.progress_tx {
        let _ = tx
            .send(AgentEvent::ToolResult {
                name: name.clone(),
                output: result.clone(),
            })
            .await;
    }

    // Add result to messages
    ctx.messages.push(Message::tool(&id, &name, &result));
    let tool_msg = AgentMessage::tool(&id, &name, &result);
    ctx.memory.add_message(tool_msg);

    Ok(ToolExecutionResult {
        tool_name: name,
        output: result,
    })
}

/// Synchronize todos from the shared Arc to the session memory
pub async fn sync_todos_from_arc(memory: &mut AgentMemory, todos_arc: &Arc<Mutex<TodoList>>) {
    let current_todos = todos_arc.lock().await;
    memory.todos = (*current_todos).clone();
}

/// Extracts a human-readable command preview from execute_command arguments
fn extract_command_preview(args: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(args)
        .ok()
        .and_then(|v| v.get("command").and_then(|c| c.as_str()).map(String::from))
}

fn inject_delegation_depth_argument(tool_name: &str, arguments: &str, depth: usize) -> String {
    if tool_name != "delegate_to_sub_agent" {
        return arguments.to_string();
    }

    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return arguments.to_string();
    };

    let Some(object) = value.as_object_mut() else {
        return arguments.to_string();
    };

    object.insert(
        "_delegation_depth".to_string(),
        serde_json::Value::from(depth),
    );

    serde_json::to_string(&value).unwrap_or_else(|_| arguments.to_string())
}

#[cfg(test)]
mod tests {
    use super::inject_delegation_depth_argument;

    #[test]
    fn delegate_tool_arguments_receive_internal_depth() {
        let args = r#"{"task":"Collect data","tools":["write_todos"]}"#;
        let patched = inject_delegation_depth_argument("delegate_to_sub_agent", args, 2);
        let value = serde_json::from_str::<serde_json::Value>(&patched)
            .unwrap_or_else(|err| panic!("failed to parse patched args: {err}"));

        assert_eq!(
            value.get("_delegation_depth"),
            Some(&serde_json::Value::from(2))
        );
    }

    #[test]
    fn non_delegate_tool_arguments_remain_unchanged() {
        let args = r#"{"command":"ls"}"#;
        let patched = inject_delegation_depth_argument("execute_command", args, 7);

        assert_eq!(patched, args);
    }
}
