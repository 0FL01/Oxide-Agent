//! Tool execution helpers for the agent runner.

use super::AgentRunner;
use super::types::{AgentRunResult, AgentRunnerContext, RunState};
use crate::agent::compaction::{CompactionPolicy, CompactionTrigger};
use crate::agent::identity::SessionId;
use crate::agent::memory::AgentMessage;
use crate::agent::progress::{AgentEvent, AgentEventSource};
use crate::agent::providers::TOOL_COMPRESS;
use crate::agent::recovery::sanitize_xml_tags;
use crate::agent::tool_failure_summary::summarize_tool_failure_content;
use crate::agent::tool_runtime::{
    ModelMetadata, OpenCodeGoToolCallBatch, ProviderMetadata, ToolBatchId, ToolCallRuntime,
    ToolHistoryError, ToolHistoryWriter, ToolOutput, ToolRuntimeConfig, ToolTurnContext, TurnId,
    v1_tool_runtime_enabled_for_model,
};
use crate::config::ModelInfo;

use crate::llm::{
    InvocationId, Message, ToolCall, ToolCallCorrelation, ToolCallFunction, ToolProtocol,
    ToolTransport,
};
use async_trait::async_trait;
use std::sync::{Arc, Mutex as StdMutex};
use uuid::Uuid;

fn current_execution_model_route(ctx: &AgentRunnerContext<'_>) -> Option<ModelInfo> {
    if let Some(route) = ctx.config.model_routes.iter().find(|route| {
        route.id == ctx.config.model_name
            && ctx
                .config
                .model_provider
                .as_deref()
                .is_none_or(|provider| route.provider == provider)
    }) {
        return Some(route.clone());
    }

    let provider = ctx.config.model_provider.clone()?;
    Some(ModelInfo {
        id: ctx.config.model_name.clone(),
        provider,
        max_output_tokens: ctx.config.model_max_output_tokens,
        context_window_tokens: 0,
        weight: 1,
    })
}

fn runtime_session_id(ctx: &AgentRunnerContext<'_>) -> SessionId {
    ctx.session_id
        .as_deref()
        .and_then(|session_id| session_id.parse::<i64>().ok())
        .map(SessionId::from)
        .unwrap_or_else(|| 0_i64.into())
}

enum BufferedRuntimeHistoryEvent {
    Assistant {
        content: String,
        reasoning: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    ToolOutput {
        output: Box<ToolOutput>,
        content: String,
    },
}

pub(super) enum ToolTurnAssistantContent<'a> {
    NativeModelContent(&'a str),
    StructuredControlEnvelope,
}

impl ToolTurnAssistantContent<'_> {
    fn assistant_content(&self) -> Option<String> {
        match self {
            Self::NativeModelContent(content) => {
                let trimmed = content.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }
            Self::StructuredControlEnvelope => None,
        }
    }
}

struct BufferedRuntimeHistory {
    assistant_reasoning: StdMutex<Option<String>>,
    assistant_content: StdMutex<Option<String>>,
    events: StdMutex<Vec<BufferedRuntimeHistoryEvent>>,
}

impl BufferedRuntimeHistory {
    fn new(assistant_reasoning: Option<String>, assistant_content: Option<String>) -> Self {
        Self {
            assistant_reasoning: StdMutex::new(assistant_reasoning),
            assistant_content: StdMutex::new(assistant_content),
            events: StdMutex::new(Vec::new()),
        }
    }

    fn drain_events(&self) -> Vec<BufferedRuntimeHistoryEvent> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain(..)
            .collect()
    }
}

#[async_trait]
impl ToolHistoryWriter for BufferedRuntimeHistory {
    async fn record_assistant_tool_calls(
        &self,
        batch: &OpenCodeGoToolCallBatch,
    ) -> Result<(), ToolHistoryError> {
        let reasoning = self
            .assistant_reasoning
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let content = self
            .assistant_content
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .unwrap_or_default();
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(BufferedRuntimeHistoryEvent::Assistant {
                content,
                reasoning,
                tool_calls: batch.to_llm_tool_calls(),
            });
        Ok(())
    }

    async fn record_tool_output(&self, output: &ToolOutput) -> Result<(), ToolHistoryError> {
        let content = output
            .encode_model_content()
            .map_err(|error| ToolHistoryError::OutputWriteFailed(error.to_string()))?;
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(BufferedRuntimeHistoryEvent::ToolOutput {
                output: Box::new(output.clone()),
                content,
            });
        Ok(())
    }
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

    pub(super) async fn execute_tools_with_runtime(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        assistant_content: ToolTurnAssistantContent<'_>,
        reasoning_content: Option<String>,
        tool_calls: Vec<ToolCall>,
    ) -> anyhow::Result<Option<AgentRunResult>> {
        let registry = ctx
            .tool_runtime_registry
            .as_ref()
            .map(Arc::clone)
            .ok_or_else(|| anyhow::anyhow!("typed tool runtime registry is not configured"))?;
        let route = current_execution_model_route(ctx)
            .ok_or_else(|| anyhow::anyhow!("typed tool runtime requires an active model route"))?;
        if !v1_tool_runtime_enabled_for_model(&route) {
            return Err(anyhow::anyhow!(
                "typed tool runtime v1 requires an opencode-go or opencode-zen route; active route is {}/{}",
                route.provider,
                route.id
            ));
        }

        for tool_call in &tool_calls {
            self.emit_runtime_tool_call(ctx, tool_call).await;
        }

        let turn_id = TurnId::from(format!("{}_{}", ctx.task_id, state.iteration));
        let batch_id = ToolBatchId::from(format!("{}_tool_batch_{}", ctx.task_id, state.iteration));
        let finish_after_todo_update = tool_calls
            .iter()
            .all(|tool_call| tool_call.function.name == "write_todos");
        let batch = OpenCodeGoToolCallBatch::from_llm_tool_calls(turn_id, tool_calls);
        let runtime_config = ToolRuntimeConfig::default();
        let native_assistant_content = assistant_content.assistant_content();
        let native_assistant_content_len = native_assistant_content
            .as_ref()
            .map_or(0, |content| content.len());
        let history = Arc::new(BufferedRuntimeHistory::new(
            reasoning_content,
            native_assistant_content,
        ));
        let history_writer: Arc<dyn ToolHistoryWriter> =
            Arc::<BufferedRuntimeHistory>::clone(&history);
        let runtime = ToolCallRuntime::new(registry, history_writer, runtime_config.clone());
        let mut turn_context = ToolTurnContext::new(
            runtime_session_id(ctx),
            batch_id,
            &runtime_config,
            ProviderMetadata {
                provider: route.provider.clone(),
                protocol: "chat_like".to_string(),
            },
            ModelMetadata {
                model: route.id.clone(),
            },
        );
        turn_context.cancellation_token = ctx.agent.cancellation_token().clone();

        let result = runtime.execute_batch(batch, turn_context).await;
        self.apply_buffered_runtime_history(ctx, state, history.drain_events())
            .await;
        result.map_err(|error| anyhow::anyhow!("typed tool runtime failed: {error}"))?;

        if finish_after_todo_update {
            let todos_arc = Arc::clone(ctx.todos_arc);
            let todos_complete = todos_arc.lock().await.is_complete();
            if todos_complete {
                tracing::debug!(
                    task_id = ctx.task_id,
                    assistant_content_len = native_assistant_content_len,
                    "write_todos completed all todos; continuing for explicit final response"
                );
            }
        }

        Self::emit_token_snapshot_update(
            ctx.progress_tx,
            Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration),
        )
        .await;
        Ok(None)
    }

    async fn emit_runtime_tool_call(&self, ctx: &mut AgentRunnerContext<'_>, tool_call: &ToolCall) {
        let Some(tx) = ctx.progress_tx else { return };
        let sanitized_name = sanitize_xml_tags(&tool_call.function.name);
        let sanitized_args = sanitize_xml_tags(&tool_call.function.arguments);
        let command_preview = if tool_call.function.name == "execute_command" {
            Self::extract_command_preview(&tool_call.function.arguments)
        } else {
            None
        };
        let _ = tx
            .send(AgentEvent::ToolCall {
                id: tool_call.invocation_id().into_inner(),
                source: AgentEventSource::Root,
                name: sanitized_name,
                input: sanitized_args,
                command_preview,
            })
            .await;
    }

    async fn apply_buffered_runtime_history(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        events: Vec<BufferedRuntimeHistoryEvent>,
    ) {
        for event in events {
            match event {
                BufferedRuntimeHistoryEvent::Assistant {
                    content,
                    reasoning,
                    tool_calls,
                } => {
                    ctx.messages
                        .push(Message::assistant_with_tools_and_reasoning(
                            &content,
                            reasoning.clone(),
                            tool_calls.clone(),
                        ));
                    ctx.agent.memory_mut().add_message(
                        AgentMessage::assistant_with_tools_and_reasoning(
                            content, reasoning, tool_calls,
                        ),
                    );
                }
                BufferedRuntimeHistoryEvent::ToolOutput { output, content } => {
                    self.apply_runtime_tool_output(ctx, state, *output, content)
                        .await;
                }
            }
        }
        Self::refresh_messages_from_memory(ctx);
    }

    async fn apply_runtime_tool_output(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        output: ToolOutput,
        content: String,
    ) {
        let tool_name = output.tool_name.as_str().to_string();
        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::ToolResult {
                    id: output.invocation_id.as_str().to_string(),
                    source: AgentEventSource::Root,
                    name: sanitize_xml_tags(&tool_name),
                    output: content.clone(),
                    success: output.success,
                })
                .await;
        }

        self.apply_after_tool_hooks(ctx, state, &tool_name, &content);
        if output.success && tool_name == TOOL_COMPRESS {
            let memory = ctx.agent.memory();
            let threshold = memory.max_tokens() / 100
                * CompactionPolicy::default().compact_threshold_percent as usize;
            if memory.token_count() >= threshold {
                state.request_manual_compaction();
            }
        }

        let correlation = ToolCallCorrelation::new(output.invocation_id.clone())
            .with_provider_tool_call_id(output.tool_call_id.as_str())
            .with_protocol(ToolProtocol::ChatLike)
            .with_transport(ToolTransport::ClientRoundTrip);
        let failure_summary = summarize_tool_failure_content(&tool_name, &content);
        let memory_content = failure_summary
            .as_ref()
            .map(|summary| summary.content.as_str())
            .unwrap_or(content.as_str());
        ctx.messages.push(Message::tool_with_correlation(
            output.invocation_id.as_str(),
            correlation.clone(),
            &tool_name,
            memory_content,
        ));
        let memory_message = if let Some(summary) = failure_summary {
            AgentMessage::pruned_tool_with_correlation(
                output.invocation_id.as_str(),
                correlation,
                &tool_name,
                summary.content,
                summary.pruned_artifact,
                None,
            )
        } else {
            AgentMessage::tool_with_correlation(
                output.invocation_id.as_str(),
                correlation,
                &tool_name,
                &content,
            )
        };
        ctx.agent.memory_mut().add_message(memory_message);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::AgentMessageKind;
    use crate::agent::context::{AgentContext, EphemeralSession};
    use crate::agent::providers::{CompressionProvider, TodoItem, TodoList, TodoStatus};
    use crate::agent::runner::AgentRunnerConfig;
    use crate::agent::tool_runtime::{
        OutputNormalizer, ToolExecutor, ToolInvocation, ToolName,
        ToolRegistry as RuntimeToolRegistry, ToolRuntimeError,
    };
    use crate::config::AgentSettings;
    use crate::llm::{LlmClient, ToolDefinition};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct StaticRuntimeExecutor;
    struct DeadEndFailureRuntimeExecutor;
    struct ParallelRuntimeExecutor {
        active: Arc<AtomicUsize>,
        max_seen: Arc<AtomicUsize>,
    }
    struct CompleteTodosRuntimeExecutor {
        todos: Arc<tokio::sync::Mutex<TodoList>>,
    }

    #[async_trait]
    impl ToolExecutor for StaticRuntimeExecutor {
        fn name(&self) -> ToolName {
            ToolName::from("read_file")
        }

        fn spec(&self) -> ToolDefinition {
            ToolDefinition {
                name: "read_file".to_string(),
                description: "read test file".to_string(),
                parameters: json!({ "type": "object" }),
            }
        }

        async fn execute(
            &self,
            invocation: ToolInvocation,
        ) -> Result<ToolOutput, ToolRuntimeError> {
            Ok(OutputNormalizer::new(ToolRuntimeConfig::default()).success(
                &invocation,
                "runtime-ok",
                "",
            ))
        }
    }

    #[async_trait]
    impl ToolExecutor for DeadEndFailureRuntimeExecutor {
        fn name(&self) -> ToolName {
            ToolName::from("web_markdown")
        }

        fn spec(&self) -> ToolDefinition {
            ToolDefinition {
                name: "web_markdown".to_string(),
                description: "fetch markdown".to_string(),
                parameters: json!({ "type": "object" }),
            }
        }

        async fn execute(
            &self,
            invocation: ToolInvocation,
        ) -> Result<ToolOutput, ToolRuntimeError> {
            let mut output = OutputNormalizer::new(ToolRuntimeConfig::default()).failure(
                &invocation,
                "web_markdown blocked by anti-bot protection at platform.kimi.ai; this lightweight fetcher cannot solve JS/CAPTCHA/PoW challenges. Do not retry this host in this task; use another source.",
            );
            output.structured_payload = Some(json!({
                "provider": "web_markdown",
                "kind": "fetch",
                "error_kind": "anti_bot",
                "host": "platform.kimi.ai",
                "url": "https://platform.kimi.ai/pricing/limits",
                "retryable": false,
                "provider_unavailable": true
            }));
            Ok(output)
        }
    }

    #[async_trait]
    impl ToolExecutor for ParallelRuntimeExecutor {
        fn name(&self) -> ToolName {
            ToolName::from("read_file")
        }

        fn spec(&self) -> ToolDefinition {
            ToolDefinition {
                name: "read_file".to_string(),
                description: "parallel read test file".to_string(),
                parameters: json!({ "type": "object" }),
            }
        }

        async fn execute(
            &self,
            invocation: ToolInvocation,
        ) -> Result<ToolOutput, ToolRuntimeError> {
            let active_now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_seen.fetch_max(active_now, Ordering::SeqCst);

            let value = invocation
                .normalized_arguments
                .get("value")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let delay_ms = 60_u64.saturating_sub(value.saturating_mul(10));
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);

            Ok(OutputNormalizer::new(ToolRuntimeConfig::default()).success(
                &invocation,
                &format!("runtime-{value}"),
                "",
            ))
        }
    }

    #[async_trait]
    impl ToolExecutor for CompleteTodosRuntimeExecutor {
        fn name(&self) -> ToolName {
            ToolName::from("write_todos")
        }

        fn spec(&self) -> ToolDefinition {
            ToolDefinition {
                name: "write_todos".to_string(),
                description: "complete todos".to_string(),
                parameters: json!({ "type": "object" }),
            }
        }

        async fn execute(
            &self,
            invocation: ToolInvocation,
        ) -> Result<ToolOutput, ToolRuntimeError> {
            let mut todos = self.todos.lock().await;
            if todos.items.is_empty() {
                todos.items.push(TodoItem {
                    description: "finalize".to_string(),
                    status: TodoStatus::Completed,
                });
            } else {
                for item in &mut todos.items {
                    item.status = TodoStatus::Completed;
                }
            }
            drop(todos);

            Ok(OutputNormalizer::new(ToolRuntimeConfig::default()).success(
                &invocation,
                "all todos complete",
                "",
            ))
        }
    }

    #[tokio::test]
    async fn typed_runtime_path_records_paired_assistant_and_tool_history() {
        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-flash".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let llm_client = Arc::new(LlmClient::new(&settings));
        let mut runner = AgentRunner::new(llm_client);
        let mut runtime_registry = RuntimeToolRegistry::new();
        runtime_registry
            .register(Arc::new(StaticRuntimeExecutor))
            .expect("runtime executor registers");
        let tools = runtime_registry.specs();
        let runtime_registry = Arc::new(runtime_registry);

        let mut session = EphemeralSession::new(2048);
        let todos_arc = Arc::new(tokio::sync::Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "read through runtime",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: Some(runtime_registry),
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runtime-test",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: Some("42".to_string()),
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 4, 1, 30, 1024)
                .with_model_provider("opencode-go")
                .with_model_routes(vec![ModelInfo {
                    id: "deepseek-v4-flash".to_string(),
                    provider: "opencode-go".to_string(),
                    max_output_tokens: 1024,
                    context_window_tokens: 8192,
                    weight: 1,
                }]),
        };
        let mut state = RunState::new();
        let tool_call = ToolCall::new(
            "invoke-read-1",
            ToolCallFunction {
                name: "read_file".to_string(),
                arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new("invoke-read-1")
                .with_provider_tool_call_id("call-read-1")
                .with_protocol(ToolProtocol::ChatLike)
                .with_transport(ToolTransport::ClientRoundTrip),
        );

        let result = runner
            .execute_tools_with_runtime(
                &mut ctx,
                &mut state,
                ToolTurnAssistantContent::StructuredControlEnvelope,
                Some("reasoning".to_string()),
                vec![tool_call],
            )
            .await
            .expect("runtime execution succeeds");

        assert!(result.is_none());
        let memory = ctx.agent.memory().get_messages();
        assert_eq!(memory.len(), 2);
        assert_eq!(memory[0].kind, AgentMessageKind::AssistantToolCall);
        assert!(
            memory[0].content.is_empty(),
            "structured tool-call envelopes must not be replayed as assistant prose"
        );
        assert_eq!(memory[1].kind, AgentMessageKind::ToolResult);
        assert_eq!(memory[1].tool_call_id.as_deref(), Some("invoke-read-1"));
        assert_eq!(memory[1].tool_name.as_deref(), Some("read_file"));
        assert!(memory[1].content.contains("\"status\":\"success\""));
        assert_eq!(
            memory[1]
                .tool_call_correlation
                .as_ref()
                .map(ToolCallCorrelation::wire_tool_call_id),
            Some("call-read-1")
        );
    }

    #[tokio::test]
    async fn native_tool_call_content_is_recorded_on_assistant_tool_call() {
        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-flash".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let llm_client = Arc::new(LlmClient::new(&settings));
        let mut runner = AgentRunner::new(llm_client);
        let mut runtime_registry = RuntimeToolRegistry::new();
        runtime_registry
            .register(Arc::new(StaticRuntimeExecutor))
            .expect("runtime executor registers");
        let tools = runtime_registry.specs();
        let runtime_registry = Arc::new(runtime_registry);

        let mut session = EphemeralSession::new(2048);
        let todos_arc = Arc::new(tokio::sync::Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "read through runtime",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: Some(runtime_registry),
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runtime-test",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: Some("42".to_string()),
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 4, 1, 30, 1024)
                .with_model_provider("opencode-go")
                .with_model_routes(vec![ModelInfo {
                    id: "deepseek-v4-flash".to_string(),
                    provider: "opencode-go".to_string(),
                    max_output_tokens: 1024,
                    context_window_tokens: 8192,
                    weight: 1,
                }]),
        };
        let mut state = RunState::new();
        let tool_call = ToolCall::new(
            "invoke-read-1",
            ToolCallFunction {
                name: "read_file".to_string(),
                arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
            },
            false,
        );

        let draft = "Full report that was generated before the tool finished.";
        let result = runner
            .execute_tools_with_runtime(
                &mut ctx,
                &mut state,
                ToolTurnAssistantContent::NativeModelContent(draft),
                None,
                vec![tool_call],
            )
            .await
            .expect("runtime execution succeeds");

        assert!(result.is_none());
        let memory = ctx.agent.memory().get_messages();
        assert_eq!(memory.len(), 2);
        assert_eq!(memory[0].kind, AgentMessageKind::AssistantToolCall);
        assert_eq!(memory[0].content, draft);
        assert_eq!(memory[1].kind, AgentMessageKind::ToolResult);
    }

    #[tokio::test]
    async fn native_todo_completion_continues_for_explicit_final_response() {
        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-flash".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let llm_client = Arc::new(LlmClient::new(&settings));
        let mut runner = AgentRunner::new(llm_client);

        let mut initial_todos = TodoList::new();
        initial_todos.items.push(TodoItem {
            description: "write final report".to_string(),
            status: TodoStatus::InProgress,
        });
        let todos_arc = Arc::new(tokio::sync::Mutex::new(initial_todos.clone()));
        let mut runtime_registry = RuntimeToolRegistry::new();
        runtime_registry
            .register(Arc::new(CompleteTodosRuntimeExecutor {
                todos: Arc::clone(&todos_arc),
            }))
            .expect("runtime executor registers");
        let tools = runtime_registry.specs();
        let runtime_registry = Arc::new(runtime_registry);

        let mut session = EphemeralSession::new(4096);
        session.memory_mut().todos = initial_todos;
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "produce final report",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: Some(runtime_registry),
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runtime-final-todo-test",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: Some("42".to_string()),
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 4, 1, 30, 1024)
                .with_model_provider("opencode-go")
                .with_model_routes(vec![ModelInfo {
                    id: "deepseek-v4-flash".to_string(),
                    provider: "opencode-go".to_string(),
                    max_output_tokens: 1024,
                    context_window_tokens: 8192,
                    weight: 1,
                }]),
        };
        let mut state = RunState::new();
        let tool_call = ToolCall::new(
            "invoke-todos-1",
            ToolCallFunction {
                name: "write_todos".to_string(),
                arguments:
                    r#"{"todos":[{"description":"write final report","status":"completed"}]}"#
                        .to_string(),
            },
            false,
        );

        let final_report = "Full report that should be delivered to the user.";
        let result = runner
            .execute_tools_with_runtime(
                &mut ctx,
                &mut state,
                ToolTurnAssistantContent::NativeModelContent(final_report),
                None,
                vec![tool_call],
            )
            .await
            .expect("runtime execution succeeds");

        assert!(result.is_none());
        assert!(todos_arc.lock().await.is_complete());

        let memory = ctx.agent.memory().get_messages();
        assert_eq!(memory.len(), 2);
        assert_eq!(memory[0].kind, AgentMessageKind::AssistantToolCall);
        assert_eq!(memory[0].content, final_report);
        assert_eq!(memory[1].kind, AgentMessageKind::ToolResult);
        assert!(
            !memory
                .iter()
                .any(|message| message.kind == AgentMessageKind::AssistantResponse)
        );
    }

    #[tokio::test]
    async fn typed_runtime_path_prunes_dead_end_tool_failure_in_memory() {
        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-flash".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let llm_client = Arc::new(LlmClient::new(&settings));
        let mut runner = AgentRunner::new(llm_client);
        let mut runtime_registry = RuntimeToolRegistry::new();
        runtime_registry
            .register(Arc::new(DeadEndFailureRuntimeExecutor))
            .expect("runtime executor registers");
        let tools = runtime_registry.specs();
        let runtime_registry = Arc::new(runtime_registry);

        let mut session = EphemeralSession::new(2048);
        let todos_arc = Arc::new(tokio::sync::Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "fetch blocked page through runtime",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: Some(runtime_registry),
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runtime-dead-end-test",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: Some("42".to_string()),
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 4, 1, 30, 1024)
                .with_model_provider("opencode-go")
                .with_model_routes(vec![ModelInfo {
                    id: "deepseek-v4-flash".to_string(),
                    provider: "opencode-go".to_string(),
                    max_output_tokens: 1024,
                    context_window_tokens: 8192,
                    weight: 1,
                }]),
        };
        let mut state = RunState::new();
        let tool_call = ToolCall::new(
            "invoke-web-1",
            ToolCallFunction {
                name: "web_markdown".to_string(),
                arguments: r#"{"url":"https://platform.kimi.ai/pricing/limits"}"#.to_string(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new("invoke-web-1")
                .with_provider_tool_call_id("call-web-1")
                .with_protocol(ToolProtocol::ChatLike)
                .with_transport(ToolTransport::ClientRoundTrip),
        );

        let result = runner
            .execute_tools_with_runtime(
                &mut ctx,
                &mut state,
                ToolTurnAssistantContent::StructuredControlEnvelope,
                None,
                vec![tool_call],
            )
            .await
            .expect("runtime execution succeeds");

        assert!(result.is_none());
        let memory = ctx.agent.memory().get_messages();
        assert_eq!(memory.len(), 2);
        let tool_result = &memory[1];
        assert_eq!(tool_result.kind, AgentMessageKind::ToolResult);
        assert_eq!(tool_result.tool_call_id.as_deref(), Some("invoke-web-1"));
        assert_eq!(
            tool_result
                .tool_call_correlation
                .as_ref()
                .map(ToolCallCorrelation::wire_tool_call_id),
            Some("call-web-1")
        );
        assert!(tool_result.is_pruned());
        assert!(tool_result.content.contains("\"dead_end\":true"));
        assert!(tool_result.content.contains("\"dead_end_scope\":\"host\""));
        assert!(!tool_result.content.contains("bytes_captured"));
        assert_eq!(ctx.messages[1].content, tool_result.content);
    }

    #[tokio::test]
    async fn typed_runtime_compress_output_requests_manual_compaction() {
        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-flash".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let llm_client = Arc::new(LlmClient::new(&settings));
        let mut runner = AgentRunner::new(llm_client);
        let mut runtime_registry = RuntimeToolRegistry::new();
        let compress_executor = Arc::new(CompressionProvider::new())
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("compress executor registered");
        runtime_registry
            .register(compress_executor)
            .expect("runtime executor registers");
        let tools = runtime_registry.specs();
        let runtime_registry = Arc::new(runtime_registry);

        let mut session = EphemeralSession::new(30);
        // Fill memory above the 85% compaction threshold (30 * 85% = 25 tokens).
        session.memory_mut().add_message(AgentMessage::user_task(
            "padding task to push token count above the eighty-five percent compaction threshold of the thirty token max window for the compress budget guard test",
        ));
        let todos_arc = Arc::new(tokio::sync::Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "compress through runtime",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: Some(runtime_registry),
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runtime-compress-test",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: Some("42".to_string()),
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 4, 1, 30, 1024)
                .with_model_provider("opencode-go")
                .with_model_routes(vec![ModelInfo {
                    id: "deepseek-v4-flash".to_string(),
                    provider: "opencode-go".to_string(),
                    max_output_tokens: 1024,
                    context_window_tokens: 8192,
                    weight: 1,
                }]),
        };
        let mut state = RunState::new();
        let tool_call = ToolCall::new(
            "invoke-compress-1",
            ToolCallFunction {
                name: TOOL_COMPRESS.to_string(),
                arguments: "{}".to_string(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new("invoke-compress-1")
                .with_provider_tool_call_id("call-compress-1")
                .with_protocol(ToolProtocol::ChatLike)
                .with_transport(ToolTransport::ClientRoundTrip),
        );

        let result = runner
            .execute_tools_with_runtime(
                &mut ctx,
                &mut state,
                ToolTurnAssistantContent::StructuredControlEnvelope,
                None,
                vec![tool_call],
            )
            .await
            .expect("runtime execution succeeds");

        assert!(result.is_none());
        assert!(state.force_manual_compaction);
        let memory = ctx.agent.memory().get_messages();
        // 1 padding user_task + 1 compress tool result = 2 (user_task was added in setup).
        assert!(memory.len() >= 2);
        let compress_result = memory
            .iter()
            .find(|m| m.tool_name.as_deref() == Some(TOOL_COMPRESS));
        let compress_result = compress_result.expect("compress tool result should be present");
        assert!(compress_result.content.contains("scheduled"));
    }

    #[tokio::test]
    async fn typed_runtime_compress_skips_when_below_budget_threshold() {
        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-flash".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let llm_client = Arc::new(LlmClient::new(&settings));
        let mut runner = AgentRunner::new(llm_client);
        let mut runtime_registry = RuntimeToolRegistry::new();
        let compress_executor = Arc::new(CompressionProvider::new())
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("compress executor registered");
        runtime_registry
            .register(compress_executor)
            .expect("runtime executor registers");
        let tools = runtime_registry.specs();
        let runtime_registry = Arc::new(runtime_registry);

        // Large context window, no messages — well below the 85% threshold.
        let mut session = EphemeralSession::new(10000);
        let todos_arc = Arc::new(tokio::sync::Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "compress below threshold",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: Some(runtime_registry),
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "compress-guard-test",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: Some("42".to_string()),
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 4, 1, 30, 1024)
                .with_model_provider("opencode-go")
                .with_model_routes(vec![ModelInfo {
                    id: "deepseek-v4-flash".to_string(),
                    provider: "opencode-go".to_string(),
                    max_output_tokens: 1024,
                    context_window_tokens: 8192,
                    weight: 1,
                }]),
        };
        let mut state = RunState::new();
        let tool_call = ToolCall::new(
            "invoke-compress-1",
            ToolCallFunction {
                name: TOOL_COMPRESS.to_string(),
                arguments: "{}".to_string(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new("invoke-compress-1")
                .with_provider_tool_call_id("call-compress-1")
                .with_protocol(ToolProtocol::ChatLike)
                .with_transport(ToolTransport::ClientRoundTrip),
        );

        let result = runner
            .execute_tools_with_runtime(
                &mut ctx,
                &mut state,
                ToolTurnAssistantContent::StructuredControlEnvelope,
                None,
                vec![tool_call],
            )
            .await
            .expect("runtime execution succeeds");

        assert!(result.is_none());
        // Budget guard should have prevented compaction.
        assert!(!state.force_manual_compaction);
    }

    #[tokio::test]
    async fn typed_runtime_path_runs_batch_in_parallel_and_preserves_output_order() {
        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-flash".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let llm_client = Arc::new(LlmClient::new(&settings));
        let mut runner = AgentRunner::new(llm_client);
        let active = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let mut runtime_registry = RuntimeToolRegistry::new();
        runtime_registry
            .register(Arc::new(ParallelRuntimeExecutor {
                active: Arc::clone(&active),
                max_seen: Arc::clone(&max_seen),
            }))
            .expect("runtime executor registers");
        let tools = runtime_registry.specs();
        let runtime_registry = Arc::new(runtime_registry);

        let mut session = EphemeralSession::new(4096);
        let todos_arc = Arc::new(tokio::sync::Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "read several files through runtime",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: Some(runtime_registry),
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runtime-parallel-test",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: Some("42".to_string()),
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 4, 1, 30, 1024)
                .with_model_provider("opencode-go")
                .with_model_routes(vec![ModelInfo {
                    id: "deepseek-v4-flash".to_string(),
                    provider: "opencode-go".to_string(),
                    max_output_tokens: 1024,
                    context_window_tokens: 8192,
                    weight: 1,
                }]),
        };
        let mut state = RunState::new();
        let tool_calls = (0..6)
            .map(|value| {
                ToolCall::new(
                    format!("invoke-read-{value}"),
                    ToolCallFunction {
                        name: "read_file".to_string(),
                        arguments: format!(r#"{{"value":{value}}}"#),
                    },
                    false,
                )
                .with_correlation(
                    ToolCallCorrelation::new(format!("invoke-read-{value}"))
                        .with_provider_tool_call_id(format!("call-read-{value}"))
                        .with_protocol(ToolProtocol::ChatLike)
                        .with_transport(ToolTransport::ClientRoundTrip),
                )
            })
            .collect::<Vec<_>>();

        let result = runner
            .execute_tools_with_runtime(
                &mut ctx,
                &mut state,
                ToolTurnAssistantContent::StructuredControlEnvelope,
                None,
                tool_calls,
            )
            .await
            .expect("runtime execution succeeds");

        assert!(result.is_none());
        assert!(
            max_seen.load(Ordering::SeqCst) > 1,
            "runtime must execute more than one tool concurrently"
        );

        let memory = ctx.agent.memory().get_messages();
        assert_eq!(memory.len(), 7);
        assert_eq!(memory[0].kind, AgentMessageKind::AssistantToolCall);
        for value in 0..6 {
            let message = &memory[value + 1];
            let expected_invocation_id = format!("invoke-read-{value}");
            assert_eq!(message.kind, AgentMessageKind::ToolResult);
            assert_eq!(
                message.tool_call_id.as_deref(),
                Some(expected_invocation_id.as_str())
            );
            assert!(
                message.content.contains(&format!("runtime-{value}")),
                "missing ordered output for value {value}"
            );
        }
    }

    #[tokio::test]
    async fn typed_runtime_path_rejects_unsupported_active_route_without_history_write() {
        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-flash".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let llm_client = Arc::new(LlmClient::new(&settings));
        let mut runner = AgentRunner::new(llm_client);
        let mut runtime_registry = RuntimeToolRegistry::new();
        runtime_registry
            .register(Arc::new(StaticRuntimeExecutor))
            .expect("runtime executor registers");
        let tools = runtime_registry.specs();
        let runtime_registry = Arc::new(runtime_registry);

        let mut session = EphemeralSession::new(2048);
        let todos_arc = Arc::new(tokio::sync::Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "unsupported typed runtime route",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: Some(runtime_registry),
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runtime-unsupported-route-test",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: Some("42".to_string()),
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 4, 1, 30, 1024)
                .with_model_provider("openrouter")
                .with_model_routes(vec![ModelInfo {
                    id: "deepseek-v4-flash".to_string(),
                    provider: "openrouter".to_string(),
                    max_output_tokens: 1024,
                    context_window_tokens: 8192,
                    weight: 1,
                }]),
        };
        let mut state = RunState::new();
        let tool_call = ToolCall::new(
            "invoke-read-unsupported",
            ToolCallFunction {
                name: "read_file".to_string(),
                arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
            },
            false,
        );

        let error = match runner
            .execute_tools_with_runtime(
                &mut ctx,
                &mut state,
                ToolTurnAssistantContent::StructuredControlEnvelope,
                None,
                vec![tool_call],
            )
            .await
        {
            Ok(_) => panic!("unsupported active route must fail before tool execution"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("typed tool runtime v1 requires an opencode-go or opencode-zen route")
        );
        assert!(
            ctx.agent.memory().get_messages().is_empty(),
            "unsupported route must not write partial assistant/tool history"
        );
    }
}
