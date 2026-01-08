//! Tool bridge module
//!
//! Handles tool execution with timeout, cancellation support, and progress events.

use super::memory::AgentMessage;
use super::progress::AgentEvent;
use super::providers::TodoList;
use super::recovery::sanitize_xml_tags;
use super::registry::ToolRegistry;
use super::session::AgentSession;
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
}

/// Execute a list of tool calls
pub async fn execute_tool_calls(
    tool_calls: Vec<ToolCall>,
    session: &mut AgentSession,
    ctx: &mut ToolExecutionContext<'_>,
) -> Result<()> {
    let tool_names: Vec<String> = tool_calls
        .iter()
        .map(|tc| tc.function.name.clone())
        .collect();
    ctx.messages.push(Message::assistant_with_tools(
        &format!("[Вызов инструментов: {}]", tool_names.join(", ")),
        tool_calls.clone(),
    ));

    let assistant_msg = AgentMessage::assistant_with_tools(
        format!("[Вызов инструментов: {}]", tool_names.join(", ")),
        tool_calls.clone(),
    );
    session.memory.add_message(assistant_msg);

    for tool_call in tool_calls {
        execute_single_tool_call(tool_call, session, ctx).await?;
    }
    Ok(())
}

/// Execute a single tool call with timeout and cancellation support
pub async fn execute_single_tool_call(
    tool_call: ToolCall,
    session: &mut AgentSession,
    ctx: &mut ToolExecutionContext<'_>,
) -> Result<()> {
    // Check for cancellation before execution
    if session.cancellation_token.is_cancelled() {
        return Err(anyhow::anyhow!("Задача отменена пользователем"));
    }

    let (name, args) = super::recovery::sanitize_tool_call(
        &tool_call.function.name,
        &tool_call.function.arguments,
    );

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
            _ = session.cancellation_token.cancelled() => {
                warn!(tool_name = %name, "Tool execution cancelled by user");
                if let Some(tx) = ctx.progress_tx {
                    let _ = tx.send(AgentEvent::Cancelling { tool_name: name.clone() }).await;
                    // Give UI time to show cancelling status (2 sec cleanup timeout)
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
                return Err(anyhow::anyhow!("Задача отменена пользователем"));
            },
            res = timeout(tool_timeout, ctx.registry.execute(&name, &args, Some(&session.cancellation_token))) => {
                match res {
                    Ok(Ok(r)) => r,
                    Ok(Err(e)) => format!("Ошибка выполнения инструмента: {e}"),
                    Err(_) => {
                        warn!(
                            tool_name = %name,
                            timeout_secs = AGENT_TOOL_TIMEOUT_SECS,
                            "Tool execution timed out"
                        );
                        format!(
                            "Инструмент '{name}' превысил лимит времени ({} секунд)",
                            AGENT_TOOL_TIMEOUT_SECS
                        )
                    }
                }
            },
        }
    };

    // Sync todos if write_todos was called
    if name == "write_todos" {
        sync_todos_from_arc(session, ctx.todos_arc).await;
        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::TodosUpdated {
                    todos: session.memory.todos.clone(),
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
    ctx.messages
        .push(Message::tool(&tool_call.id, &name, &result));
    let tool_msg = AgentMessage::tool(&tool_call.id, &name, &result);
    session.memory.add_message(tool_msg);

    Ok(())
}

/// Synchronize todos from the shared Arc to the session memory
pub async fn sync_todos_from_arc(session: &mut AgentSession, todos_arc: &Arc<Mutex<TodoList>>) {
    let current_todos = todos_arc.lock().await;
    session.memory.todos = (*current_todos).clone();
}

/// Extracts a human-readable command preview from execute_command arguments
fn extract_command_preview(args: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(args)
        .ok()
        .and_then(|v| v.get("command").and_then(|c| c.as_str()).map(String::from))
}
