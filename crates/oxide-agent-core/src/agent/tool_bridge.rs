//! Tool bridge module
//!
//! Handles tool execution with timeout, cancellation support, and progress events.

use super::context::AgentContext;
use super::memory::AgentMemory;
use super::memory::AgentMessage;
use super::progress::AgentEvent;
use super::providers::TodoList;
use super::recovery::sanitize_xml_tags;
use super::registry::ToolRegistry;
use super::PendingSshReplay;
use crate::config::AGENT_TOOL_TIMEOUT_SECS;
use crate::llm::{Message, ToolCall};
use anyhow::Result;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

struct ParsedToolCall {
    id: String,
    name: String,
    args: String,
}

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
    /// Mutable access to agent state
    pub agent: &'a mut dyn AgentContext,
    /// Cancellation token for the current task
    pub cancellation_token: tokio_util::sync::CancellationToken,
}

/// Result of executing a tool call.
pub enum ToolExecutionResult {
    /// Tool execution completed normally.
    Completed {
        /// Name of the tool that was executed.
        tool_name: String,
        /// Output produced by the tool.
        output: String,
    },
    /// Tool execution is paused pending operator approval.
    WaitingForApproval {
        /// Name of the tool awaiting approval.
        tool_name: String,
    },
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
    ensure_tool_execution_not_cancelled(&ctx.cancellation_token)?;
    let parsed = parse_tool_call(tool_call);

    log_tool_call(&parsed);
    emit_tool_call_event(&parsed, ctx.progress_tx).await;

    let result = execute_tool_with_timeout(&parsed, ctx).await?;
    normalize_tool_result(parsed, result, ctx).await
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

fn ensure_tool_execution_not_cancelled(
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    if cancellation_token.is_cancelled() {
        return Err(anyhow::anyhow!("Task cancelled by user"));
    }
    Ok(())
}

fn parse_tool_call(tool_call: ToolCall) -> ParsedToolCall {
    let ToolCall { id, function, .. } = tool_call;
    ParsedToolCall {
        id,
        name: function.name,
        args: function.arguments,
    }
}

fn log_tool_call(tool_call: &ParsedToolCall) {
    info!(
        tool_name = %tool_call.name,
        tool_args = %crate::utils::truncate_str(&tool_call.args, 200),
        "Executing tool call"
    );
}

async fn emit_tool_call_event(
    tool_call: &ParsedToolCall,
    progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
) {
    let Some(tx) = progress_tx else {
        return;
    };

    let command_preview = if tool_call.name == "execute_command" {
        extract_command_preview(&tool_call.args)
    } else {
        None
    };

    let _ = tx
        .send(AgentEvent::ToolCall {
            name: sanitize_xml_tags(&tool_call.name),
            input: sanitize_xml_tags(&tool_call.args),
            command_preview,
        })
        .await;
}

async fn execute_tool_with_timeout(
    tool_call: &ParsedToolCall,
    ctx: &mut ToolExecutionContext<'_>,
) -> Result<String> {
    let tool_timeout = Duration::from_secs(AGENT_TOOL_TIMEOUT_SECS);

    use tokio::select;
    select! {
        biased;
        _ = ctx.cancellation_token.cancelled() => {
            handle_tool_cancellation(&tool_call.name, ctx.progress_tx).await;
            Err(anyhow::anyhow!("Task cancelled by user"))
        },
        res = timeout(
            tool_timeout,
            ctx.registry.execute(
                &tool_call.name,
                &tool_call.args,
                ctx.progress_tx,
                Some(&ctx.cancellation_token),
            ),
        ) => Ok(map_tool_execution_result(&tool_call.name, res)),
    }
}

async fn handle_tool_cancellation(
    tool_name: &str,
    progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
) {
    warn!(tool_name = %tool_name, "Tool execution cancelled by user");
    if let Some(tx) = progress_tx {
        let _ = tx
            .send(AgentEvent::Cancelling {
                tool_name: tool_name.to_string(),
            })
            .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}

fn map_tool_execution_result(
    tool_name: &str,
    result: Result<Result<String, anyhow::Error>, tokio::time::error::Elapsed>,
) -> String {
    match result {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => format!("Tool execution error: {error}"),
        Err(_) => {
            warn!(
                tool_name = %tool_name,
                timeout_secs = AGENT_TOOL_TIMEOUT_SECS,
                "Tool execution timed out"
            );
            format!(
                "Tool '{tool_name}' timed out ({} seconds)",
                AGENT_TOOL_TIMEOUT_SECS
            )
        }
    }
}

async fn normalize_tool_result(
    tool_call: ParsedToolCall,
    result: String,
    ctx: &mut ToolExecutionContext<'_>,
) -> Result<ToolExecutionResult> {
    sync_todos_if_needed(&tool_call.name, ctx).await;

    if let Some(approval) = parse_pending_ssh_approval(&result) {
        store_pending_ssh_approval(&tool_call, &approval, ctx).await;
        return Ok(ToolExecutionResult::WaitingForApproval {
            tool_name: tool_call.name,
        });
    }

    emit_tool_result_event(&tool_call.name, &result, ctx.progress_tx).await;
    append_tool_result_to_memory(&tool_call, &result, ctx);
    Ok(ToolExecutionResult::Completed {
        tool_name: tool_call.name,
        output: result,
    })
}

async fn sync_todos_if_needed(tool_name: &str, ctx: &mut ToolExecutionContext<'_>) {
    if tool_name != "write_todos" {
        return;
    }

    sync_todos_from_arc(ctx.agent.memory_mut(), ctx.todos_arc).await;
    if let Err(error) = ctx.agent.persist_memory_checkpoint().await {
        warn!(error = %error, "Failed to persist todo checkpoint");
    }
    if let Some(tx) = ctx.progress_tx {
        let _ = tx
            .send(AgentEvent::TodosUpdated {
                todos: ctx.agent.memory().todos.clone(),
            })
            .await;
    }
}

async fn store_pending_ssh_approval(
    tool_call: &ParsedToolCall,
    approval: &PendingSshApprovalPayload,
    ctx: &mut ToolExecutionContext<'_>,
) {
    ctx.agent.store_pending_ssh_replay(PendingSshReplay {
        request_id: approval.request_id.clone(),
        tool_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        arguments: tool_call.args.clone(),
    });

    if let Some(tx) = ctx.progress_tx {
        let _ = tx
            .send(AgentEvent::WaitingForApproval {
                tool_name: sanitize_xml_tags(&tool_call.name),
                target_name: sanitize_xml_tags(&approval.target_name),
                summary: sanitize_xml_tags(&approval.summary),
            })
            .await;
    }
}

async fn emit_tool_result_event(
    tool_name: &str,
    result: &str,
    progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
) {
    if let Some(tx) = progress_tx {
        let _ = tx
            .send(AgentEvent::ToolResult {
                name: tool_name.to_string(),
                output: result.to_string(),
            })
            .await;
    }
}

fn append_tool_result_to_memory(
    tool_call: &ParsedToolCall,
    result: &str,
    ctx: &mut ToolExecutionContext<'_>,
) {
    ctx.messages
        .push(Message::tool(&tool_call.id, &tool_call.name, result));
    let tool_msg = AgentMessage::tool(&tool_call.id, &tool_call.name, result);
    ctx.agent.memory_mut().add_message(tool_msg);
}

#[derive(Debug, Deserialize)]
struct PendingSshApprovalPayload {
    #[serde(default)]
    approval_required: bool,
    request_id: String,
    target_name: String,
    summary: String,
}

fn parse_pending_ssh_approval(output: &str) -> Option<PendingSshApprovalPayload> {
    let payload: PendingSshApprovalPayload = serde_json::from_str(output).ok()?;
    payload.approval_required.then_some(payload)
}

#[cfg(test)]
mod tests {
    use super::{execute_single_tool_call, parse_pending_ssh_approval, ToolExecutionContext};
    use crate::agent::provider::ToolProvider;
    use crate::agent::providers::TodoList;
    use crate::agent::registry::ToolRegistry;
    use crate::agent::session::AgentSession;
    use crate::llm::{ToolCall, ToolCallFunction, ToolDefinition};
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct PendingApprovalProvider;

    #[async_trait]
    impl ToolProvider for PendingApprovalProvider {
        fn name(&self) -> &'static str {
            "pending_approval_provider"
        }

        fn tools(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "ssh_sudo_exec".to_string(),
                description: "test".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            }]
        }

        fn can_handle(&self, tool_name: &str) -> bool {
            tool_name == "ssh_sudo_exec"
        }

        async fn execute(
            &self,
            _tool_name: &str,
            _arguments: &str,
            _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
            _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
        ) -> anyhow::Result<String> {
            Ok(r#"{"ok":false,"approval_required":true,"request_id":"req-1","tool_name":"ssh_sudo_exec","topic_id":"topic-a","target_name":"n-de1","summary":"sudo exec on n-de1: journalctl","expires_at":123}"#.to_string())
        }
    }

    #[test]
    fn parses_pending_ssh_approval_payload() {
        let payload = parse_pending_ssh_approval(
            r#"{"ok":false,"approval_required":true,"request_id":"req-1","tool_name":"ssh_sudo_exec","topic_id":"topic-a","target_name":"n-de1","summary":"sudo exec on n-de1: journalctl","expires_at":123}"#,
        )
        .expect("payload must parse");

        assert_eq!(payload.target_name, "n-de1");
        assert_eq!(payload.summary, "sudo exec on n-de1: journalctl");
    }

    #[test]
    fn ignores_non_approval_payloads() {
        assert!(parse_pending_ssh_approval(r#"{"ok":true}"#).is_none());
    }

    #[tokio::test]
    async fn waiting_for_approval_does_not_append_tool_result_to_memory() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(PendingApprovalProvider));

        let mut session = AgentSession::new(1_i64.into());
        let todos_arc = Arc::new(Mutex::new(TodoList::new()));
        let mut messages = Vec::new();
        let cancellation_token = tokio_util::sync::CancellationToken::new();

        let tool_call = ToolCall {
            id: "call-1".to_string(),
            function: ToolCallFunction {
                name: "ssh_sudo_exec".to_string(),
                arguments: r#"{"command":"journalctl -p err -n 10 --no-pager"}"#.to_string(),
            },
            is_recovered: false,
        };

        let mut ctx = ToolExecutionContext {
            registry: &registry,
            progress_tx: None,
            todos_arc: &todos_arc,
            messages: &mut messages,
            agent: &mut session,
            cancellation_token,
        };

        let result = execute_single_tool_call(tool_call, &mut ctx)
            .await
            .expect("tool call must succeed");

        assert!(matches!(
            result,
            super::ToolExecutionResult::WaitingForApproval { .. }
        ));
        assert!(messages.is_empty(), "tool response must not be appended");
        assert!(
            session.memory.get_messages().is_empty(),
            "agent memory must not record a fake tool result"
        );
        assert_eq!(
            session
                .pending_ssh_replay("req-1")
                .expect("pending replay must be stored")
                .tool_call_id,
            "call-1"
        );
    }
}
