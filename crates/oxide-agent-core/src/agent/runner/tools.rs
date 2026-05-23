//! Tool execution helpers for the agent runner.

use super::types::{AgentRunResult, AgentRunnerContext, RunState};
use super::AgentRunner;
use crate::agent::compaction::CompactionTrigger;
use crate::agent::identity::SessionId;
use crate::agent::memory::AgentMessage;
use crate::agent::progress::AgentEvent;
use crate::agent::recovery::sanitize_xml_tags;
use crate::agent::tool_runtime::{
    v1_tool_runtime_enabled_for_model, ModelMetadata, OpenCodeGoToolCallBatch, ProviderMetadata,
    ToolBatchId, ToolCallRuntime, ToolHistoryError, ToolHistoryWriter, ToolOutput,
    ToolRuntimeConfig, ToolTurnContext, TurnId,
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

struct BufferedRuntimeHistory {
    assistant_content: String,
    assistant_reasoning: StdMutex<Option<String>>,
    events: StdMutex<Vec<BufferedRuntimeHistoryEvent>>,
}

impl BufferedRuntimeHistory {
    fn new(assistant_content: String, assistant_reasoning: Option<String>) -> Self {
        Self {
            assistant_content,
            assistant_reasoning: StdMutex::new(assistant_reasoning),
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
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(BufferedRuntimeHistoryEvent::Assistant {
                content: self.assistant_content.clone(),
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
        raw_json: &str,
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
                "typed tool runtime v1 only supports opencode-go/deepseek-v4-flash; active route is {}/{}",
                route.provider,
                route.id
            ));
        }

        for tool_call in &tool_calls {
            self.emit_runtime_tool_call(ctx, tool_call).await;
        }

        let turn_id = TurnId::from(format!("{}_{}", ctx.task_id, state.iteration));
        let batch_id = ToolBatchId::from(format!("{}_tool_batch_{}", ctx.task_id, state.iteration));
        let batch = OpenCodeGoToolCallBatch::from_llm_tool_calls(turn_id, tool_calls);
        let runtime_config = ToolRuntimeConfig::default();
        let history = Arc::new(BufferedRuntimeHistory::new(
            raw_json.to_string(),
            reasoning_content,
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
                    name: sanitize_xml_tags(&tool_name),
                    output: content.clone(),
                    success: output.success,
                })
                .await;
        }

        self.apply_after_tool_hooks(ctx, state, &tool_name, &content);

        let correlation = ToolCallCorrelation::new(output.invocation_id.clone())
            .with_provider_tool_call_id(output.tool_call_id.as_str())
            .with_protocol(ToolProtocol::ChatLike)
            .with_transport(ToolTransport::ClientRoundTrip);
        ctx.messages.push(Message::tool_with_correlation(
            output.invocation_id.as_str(),
            correlation.clone(),
            &tool_name,
            &content,
        ));
        ctx.agent
            .memory_mut()
            .add_message(AgentMessage::tool_with_correlation(
                output.invocation_id.as_str(),
                correlation,
                &tool_name,
                &content,
            ));
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
    struct ParallelRuntimeExecutor {
        active: Arc<AtomicUsize>,
        max_seen: Arc<AtomicUsize>,
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

    #[tokio::test]
    async fn typed_runtime_path_records_paired_assistant_and_tool_history() {
        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-flash".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let llm_client = Arc::new(LlmClient::new(&settings));
        let mut runner = AgentRunner::new(llm_client);
        let legacy_registry = crate::agent::registry::ToolRegistry::new();
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
            tools: &tools,
            registry: &legacy_registry,
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
                "assistant raw",
                Some("reasoning".to_string()),
                vec![tool_call],
            )
            .await
            .expect("runtime execution succeeds");

        assert!(result.is_none());
        let memory = ctx.agent.memory().get_messages();
        assert_eq!(memory.len(), 2);
        assert_eq!(memory[0].kind, AgentMessageKind::AssistantToolCall);
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
    async fn typed_runtime_path_runs_batch_in_parallel_and_preserves_output_order() {
        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-flash".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let llm_client = Arc::new(LlmClient::new(&settings));
        let mut runner = AgentRunner::new(llm_client);
        let legacy_registry = crate::agent::registry::ToolRegistry::new();
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
            tools: &tools,
            registry: &legacy_registry,
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
            .execute_tools_with_runtime(&mut ctx, &mut state, "assistant raw", None, tool_calls)
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
        let legacy_registry = crate::agent::registry::ToolRegistry::new();
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
            tools: &tools,
            registry: &legacy_registry,
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
                "assistant raw",
                None,
                vec![tool_call],
            )
            .await
        {
            Ok(_) => panic!("unsupported active route must fail before tool execution"),
            Err(error) => error,
        };

        assert!(error
            .to_string()
            .contains("typed tool runtime v1 only supports opencode-go/deepseek-v4-flash"));
        assert!(
            ctx.agent.memory().get_messages().is_empty(),
            "unsupported route must not write partial assistant/tool history"
        );
    }
}
