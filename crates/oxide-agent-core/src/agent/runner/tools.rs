//! Tool execution helpers for the agent runner.

use super::AgentRunner;
use super::types::{AgentRunResult, AgentRunnerContext, PendingFinalDraft, RunState};
use crate::agent::compaction::{
    AdmissionBudget, AdmissionDecision, CompactionEngine, CompactionTrigger, ContextAdmission,
    PayloadDescriptor, PayloadKind, count_tokens_cached,
};
use crate::agent::hooks::HookResult;
use crate::agent::identity::SessionId;
use crate::agent::memory::{AgentMessage, AgentMessageAttachment};
use crate::agent::progress::{AgentEvent, AgentEventSource};
use crate::agent::providers::{CompressRequest, CompressResult, TOOL_COMPRESS};
use crate::agent::tool_failure_summary::summarize_tool_failure_content;
use crate::agent::tool_runtime::{
    ModelMetadata, OpenCodeGoToolCallBatch, ProviderMetadata, ToolBatchId, ToolCallRuntime,
    ToolHistoryError, ToolHistoryWriter, ToolOutput, ToolRuntimeConfig, ToolTurnContext, TurnId,
    v1_tool_runtime_enabled_for_model,
};
use crate::config::ModelInfo;

use crate::llm::{
    InvocationId, Message, ToolCall, ToolCallCorrelation, ToolCallFunction, ToolDefinition,
    ToolProtocol, ToolTransport,
};
use async_trait::async_trait;
use serde_json::json;
use std::collections::BTreeMap;
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
                "typed tool runtime v1 requires a chat-like tool route; active route is {}/{}",
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
        let pending_final_draft_content = native_assistant_content.clone();
        let history = Arc::new(BufferedRuntimeHistory::new(
            reasoning_content,
            native_assistant_content,
        ));
        let mut blocked_calls = BTreeMap::new();
        for call in &batch.calls {
            let hook_result = self.before_tool_hook_result(
                ctx,
                state,
                call.tool_name.as_str(),
                &call.raw_arguments,
            );
            match hook_result {
                HookResult::Continue => {}
                HookResult::Block { reason } => {
                    blocked_calls.insert(call.batch_index, reason);
                }
                other => {
                    if let Some(report) = self.apply_hook_result(other, ctx, Some(&mut *state))? {
                        return Ok(Some(AgentRunResult::Final(report)));
                    }
                }
            }
        }
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

        let result = runtime
            .execute_batch_with_blocked_calls(batch, turn_context, blocked_calls)
            .await;
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
                if let Some(content) = pending_final_draft_content
                    && let Some(draft) =
                        PendingFinalDraft::from_write_todos_content(content, state.iteration)
                {
                    tracing::info!(
                        task_id = ctx.task_id,
                        draft_len = draft.content_len(),
                        source_iteration = draft.source_iteration,
                        source_tool_name = draft.source_tool_name,
                        "stored pending final draft from write_todos tool-call content"
                    );
                    state.pending_final_draft = Some(draft);
                }
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
        let name = tool_call.function.name.clone();
        let args = tool_call.function.arguments.clone();
        let command_preview = if tool_call.function.name == "execute_command" {
            Self::extract_command_preview(&tool_call.function.arguments)
        } else {
            None
        };
        let _ = tx
            .send(AgentEvent::ToolCall {
                id: tool_call.invocation_id().into_inner(),
                source: AgentEventSource::Root,
                name,
                input: args,
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
        mut content: String,
    ) {
        let tool_name = output.tool_name.as_str().to_string();
        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::ToolResult {
                    id: output.invocation_id.as_str().to_string(),
                    source: AgentEventSource::Root,
                    name: tool_name.clone(),
                    output: content.clone(),
                    success: output.success,
                })
                .await;
        }

        self.apply_after_tool_hooks(ctx, state, &tool_name, &content);

        // Apply compress tool results through the compaction engine.
        // The tool executor parses LLM args into a CompressRequest (returned
        // as structured_payload); the runner applies each entry through the
        // engine — the sole mutation authority for CompactionState.
        if output.success && tool_name == TOOL_COMPRESS {
            content = Self::apply_compress_through_engine(ctx, output.structured_payload.as_ref());
        }

        let correlation = ToolCallCorrelation::new(output.invocation_id.clone())
            .with_provider_tool_call_id(output.tool_call_id.as_str())
            .with_protocol(ToolProtocol::ChatLike)
            .with_transport(ToolTransport::ClientRoundTrip);
        let failure_summary = summarize_tool_failure_content(&tool_name, &content);

        // Admission gate: evaluate tool output before hot-memory mutation.
        // Skip when failure_summary already pruned the content (it's already bounded).
        let (model_content, externalized_payload) = if let Some(summary) = &failure_summary {
            (summary.content.clone(), None)
        } else {
            let budget = Self::compute_admission_budget(ctx);
            let descriptor = PayloadDescriptor {
                kind: PayloadKind::ToolOutput {
                    tool_name: tool_name.clone(),
                },
                content: content.clone(),
                source: None,
                size_bytes: content.len(),
            };
            match ContextAdmission::evaluate(&descriptor, &budget) {
                AdmissionDecision::Inline => (content.clone(), None),
                AdmissionDecision::Manifest(spec) => (
                    spec.manifest_content.clone(),
                    Some(spec.externalized_payload),
                ),
                AdmissionDecision::ControlledPause(blocker) => {
                    let placeholder = format!(
                        "[Tool output withheld — context budget exceeded]\n{}",
                        blocker.reason()
                    );
                    (placeholder, None)
                }
            }
        };

        ctx.messages.push(Message::tool_with_correlation(
            output.invocation_id.as_str(),
            correlation.clone(),
            &tool_name,
            &model_content,
        ));
        let mut memory_message = if let Some(summary) = failure_summary {
            AgentMessage::pruned_tool_with_correlation(
                output.invocation_id.as_str(),
                correlation,
                &tool_name,
                summary.content,
                summary.pruned_artifact,
                None,
            )
        } else {
            let mut msg = AgentMessage::tool_with_correlation(
                output.invocation_id.as_str(),
                correlation,
                &tool_name,
                &model_content,
            );
            msg.externalized_payload = externalized_payload;
            msg
        };
        if let Some(image) = output.image_attachment {
            let mut attachment = if let Some(data) = image.data {
                AgentMessageAttachment::image_with_data(
                    image.file_name,
                    image.mime_type,
                    image.size_bytes,
                    image.sandbox_path,
                    data,
                )
            } else {
                AgentMessageAttachment::image(
                    image.file_name,
                    image.mime_type,
                    image.size_bytes,
                    image.sandbox_path,
                )
            };
            if let Some(uri) = image.artifact_uri {
                attachment = attachment.with_artifact_uri(uri);
            }
            memory_message.attachments.push(attachment);
        }
        ctx.agent.memory_mut().add_message(memory_message);
    }

    /// Apply a parsed compress request through the compaction engine.
    ///
    /// Extracts the `CompressRequest` from the tool's `structured_payload`,
    /// calls `CompactionEngine::apply_compression` for each entry, and returns
    /// the result JSON to use as the tool output content (visible to the LLM).
    ///
    /// The tool executor is a pure parser — it cannot access `AgentMemory`.
    /// The runner is the authority that applies parsed requests through the
    /// engine, which is the sole mutation authority for `CompactionState`.
    fn apply_compress_through_engine(
        ctx: &mut AgentRunnerContext<'_>,
        payload: Option<&serde_json::Value>,
    ) -> String {
        let Some(payload) = payload else {
            return serde_json::to_string_pretty(&json!({
                "compressed": false,
                "error": "internal_error",
                "error_detail": "Compress tool returned no structured payload"
            }))
            .unwrap_or_else(|_| r#"{"compressed": false}"#.to_string());
        };

        let request: CompressRequest = match serde_json::from_value(payload.clone()) {
            Ok(req) => req,
            Err(e) => {
                return serde_json::to_string_pretty(&json!({
                    "compressed": false,
                    "error": "internal_error",
                    "error_detail": format!("Failed to parse compress request: {e}")
                }))
                .unwrap_or_else(|_| r#"{"compressed": false}"#.to_string());
            }
        };

        // Clone messages to avoid borrow conflict with compaction_state_mut.
        // The engine does not modify raw messages, so this snapshot is valid
        // for all entries in the request.
        let messages = ctx.agent.memory().get_messages().to_vec();

        let mut outcomes = Vec::with_capacity(request.entries.len());
        for entry in &request.entries {
            let result = CompactionEngine::apply_compression(
                ctx.agent.memory_mut().compaction_state_mut(),
                &messages,
                &entry.selection,
                entry.summary.clone(),
            );
            outcomes.push(result.map_err(|e| e.to_string()));
        }

        let result = CompressResult::from_outcomes(outcomes);
        result
            .to_json()
            .unwrap_or_else(|_| r#"{"compressed": false}"#.to_string())
    }

    /// Compute the admission budget from the current runner context.
    ///
    /// This is used by the tool-output admission gate to decide whether a
    /// tool result can enter hot memory inline or must be externalized.
    /// Uses the route's `context_window_tokens` (the real provider constraint)
    /// rather than `memory.max_tokens()` (which may be set artificially small
    /// for compaction threshold testing).
    fn compute_admission_budget(ctx: &AgentRunnerContext<'_>) -> AdmissionBudget {
        let memory = ctx.agent.memory();
        let route_context_window = ctx
            .config
            .model_routes
            .first()
            .map(|route| route.context_window_tokens as usize)
            .unwrap_or_else(|| memory.max_tokens());
        AdmissionBudget {
            rendered_tokens: memory.token_count(),
            route_context_window,
            system_prompt_tokens: count_tokens_cached(ctx.system_prompt),
            tool_schema_tokens: Self::estimate_tool_schema_tokens(ctx.tools),
            hard_reserve: ctx.config.model_max_output_tokens as usize,
        }
    }

    /// Estimate token overhead from tool definitions (name + description + parameters schema).
    fn estimate_tool_schema_tokens(tools: &[ToolDefinition]) -> usize {
        tools.iter().fold(0, |acc, tool| {
            let param_tokens = serde_json::to_string(&tool.parameters)
                .ok()
                .as_deref()
                .map_or(0, count_tokens_cached);
            acc.saturating_add(count_tokens_cached(&tool.name))
                .saturating_add(count_tokens_cached(&tool.description))
                .saturating_add(param_tokens)
        })
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
    use crate::agent::hooks::SearchBudgetHook;
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
    struct CountingSearchRuntimeExecutor {
        executions: Arc<AtomicUsize>,
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

    #[async_trait]
    impl ToolExecutor for CountingSearchRuntimeExecutor {
        fn name(&self) -> ToolName {
            ToolName::from("web_search")
        }

        fn spec(&self) -> ToolDefinition {
            ToolDefinition {
                name: "web_search".to_string(),
                description: "search test executor".to_string(),
                parameters: json!({ "type": "object" }),
            }
        }

        async fn execute(
            &self,
            invocation: ToolInvocation,
        ) -> Result<ToolOutput, ToolRuntimeError> {
            self.executions.fetch_add(1, Ordering::SeqCst);
            Ok(OutputNormalizer::new(ToolRuntimeConfig::default()).success(
                &invocation,
                "search-ok",
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
            storage: None,
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
    async fn typed_runtime_enforces_search_budget_before_tool_execution() {
        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-flash".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let llm_client = Arc::new(LlmClient::new(&settings));
        let mut runner = AgentRunner::new(llm_client);
        runner.register_hook(Box::new(SearchBudgetHook::new(10)));
        let executions = Arc::new(AtomicUsize::new(0));
        let mut runtime_registry = RuntimeToolRegistry::new();
        runtime_registry
            .register(Arc::new(CountingSearchRuntimeExecutor {
                executions: Arc::clone(&executions),
            }))
            .expect("runtime executor registers");
        let tools = runtime_registry.specs();
        let runtime_registry = Arc::new(runtime_registry);

        let mut session = EphemeralSession::new(2048);
        let todos_arc = Arc::new(tokio::sync::Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "search through runtime",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: Some(runtime_registry),
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runtime-search-budget-test",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: Some("42".to_string()),
            memory_scope: None,
            memory_behavior: None,
            storage: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 4, 1, 30, 1024)
                .with_model_provider("opencode-go")
                .with_model_routes(vec![ModelInfo {
                    id: "deepseek-v4-flash".to_string(),
                    provider: "opencode-go".to_string(),
                    max_output_tokens: 1024,
                    context_window_tokens: 8192,
                    weight: 1,
                }])
                .with_search_limit(1),
        };
        let mut state = RunState::new();
        let first = ToolCall::new(
            "invoke-search-1",
            ToolCallFunction {
                name: "web_search".to_string(),
                arguments: r#"{"query":"rust"}"#.to_string(),
            },
            false,
        );
        let second = ToolCall::new(
            "invoke-search-2",
            ToolCallFunction {
                name: "web_search".to_string(),
                arguments: r#"{"query":"tokio"}"#.to_string(),
            },
            false,
        );

        let result = runner
            .execute_tools_with_runtime(
                &mut ctx,
                &mut state,
                ToolTurnAssistantContent::StructuredControlEnvelope,
                None,
                vec![first, second],
            )
            .await
            .expect("runtime execution succeeds");

        assert!(result.is_none());
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        let memory = ctx.agent.memory().get_messages();
        assert_eq!(memory.len(), 3);
        assert_eq!(memory[0].kind, AgentMessageKind::AssistantToolCall);
        assert_eq!(memory[1].kind, AgentMessageKind::ToolResult);
        assert_eq!(memory[1].tool_call_id.as_deref(), Some("invoke-search-1"));
        assert!(memory[1].content.contains("\"status\":\"success\""));
        assert_eq!(memory[2].kind, AgentMessageKind::ToolResult);
        assert_eq!(memory[2].tool_call_id.as_deref(), Some("invoke-search-2"));
        assert!(memory[2].content.contains("\"status\":\"failure\""));
        assert!(memory[2].content.contains("Search budget exceeded (2/1)"));
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
            storage: None,
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
            storage: None,
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
        assert!(state.pending_final_draft.is_none());

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
    async fn long_native_todo_completion_content_is_stored_as_pending_final_draft() {
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
            task_id: "runtime-final-todo-draft-test",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: Some("42".to_string()),
            memory_scope: None,
            memory_behavior: None,
            storage: None,
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

        let final_report = format!(
            "## Итоговый отчёт\n\n{}",
            "- Модель: https://huggingface.co/example/model — годна после проверки.\n".repeat(80)
        );
        let result = runner
            .execute_tools_with_runtime(
                &mut ctx,
                &mut state,
                ToolTurnAssistantContent::NativeModelContent(&final_report),
                None,
                vec![tool_call],
            )
            .await
            .expect("runtime execution succeeds");

        assert!(result.is_none());
        assert!(todos_arc.lock().await.is_complete());
        let draft = state
            .pending_final_draft
            .as_ref()
            .expect("long final report should be kept as a pending draft");
        assert_eq!(draft.content, final_report.trim());
        assert_eq!(draft.source_tool_name, "write_todos");

        let memory = ctx.agent.memory().get_messages();
        assert_eq!(memory.len(), 2);
        assert_eq!(memory[0].kind, AgentMessageKind::AssistantToolCall);
        assert_eq!(memory[0].content, final_report.trim());
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
            storage: None,
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
    async fn typed_runtime_compress_applies_through_engine() {
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

        let mut session = EphemeralSession::new(10000);
        // Add 3 messages that the compress tool will cover.
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Task description"));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Assistant response"));
        session
            .memory_mut()
            .add_message(AgentMessage::user_turn("Follow-up question"));
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
            storage: None,
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
        let compress_args = r#"{
            "ranges": [
                {
                    "start": "m0001",
                    "end": "m0003",
                    "summary": [{"text": "User task, assistant response, and follow-up"}]
                }
            ]
        }"#;
        let tool_call = ToolCall::new(
            "invoke-compress-1",
            ToolCallFunction {
                name: TOOL_COMPRESS.to_string(),
                arguments: compress_args.to_string(),
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
        // Old manual compaction trigger must NOT be set.
        assert!(!state.force_manual_compaction);
        // Engine must have created an active block.
        assert!(ctx.agent.memory().compaction_state().has_active_blocks());
        // Tool result in memory must show compression success.
        let memory = ctx.agent.memory().get_messages();
        let compress_result = memory
            .iter()
            .find(|m| m.tool_name.as_deref() == Some(TOOL_COMPRESS));
        let compress_result = compress_result.expect("compress tool result should be present");
        assert!(
            compress_result.content.contains(r#""compressed": true"#),
            "compress result must show compressed=true, got: {}",
            compress_result.content
        );
        assert!(
            compress_result.content.contains("b1"),
            "compress result must mention block b1, got: {}",
            compress_result.content
        );
    }

    #[tokio::test]
    async fn typed_runtime_compress_rejects_invalid_refs() {
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

        let mut session = EphemeralSession::new(10000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Task"));
        let todos_arc = Arc::new(tokio::sync::Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "compress with invalid refs",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: Some(runtime_registry),
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "compress-invalid-refs-test",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: Some("42".to_string()),
            memory_scope: None,
            memory_behavior: None,
            storage: None,
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
        // m0099 is out of range — engine must reject.
        let compress_args = r#"{
            "ranges": [
                {
                    "start": "m0099",
                    "end": "m0100",
                    "summary": [{"text": "Invalid range"}]
                }
            ]
        }"#;
        let tool_call = ToolCall::new(
            "invoke-compress-1",
            ToolCallFunction {
                name: TOOL_COMPRESS.to_string(),
                arguments: compress_args.to_string(),
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
        assert!(!state.force_manual_compaction);
        // No blocks should be created for invalid refs.
        assert!(!ctx.agent.memory().compaction_state().has_active_blocks());
        // Tool result must show structured error.
        let memory = ctx.agent.memory().get_messages();
        let compress_result = memory
            .iter()
            .find(|m| m.tool_name.as_deref() == Some(TOOL_COMPRESS));
        let compress_result = compress_result.expect("compress tool result should be present");
        assert!(
            compress_result.content.contains(r#""compressed": false"#),
            "compress result must show compressed=false, got: {}",
            compress_result.content
        );
        assert!(
            compress_result.content.contains("invalid_message_ref"),
            "compress result must contain invalid_message_ref error, got: {}",
            compress_result.content
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
            storage: None,
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
            storage: None,
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
                .contains("typed tool runtime v1 requires a chat-like tool route")
        );
        assert!(
            ctx.agent.memory().get_messages().is_empty(),
            "unsupported route must not write partial assistant/tool history"
        );
    }

    #[tokio::test]
    async fn typed_runtime_path_accepts_openai_base_chat_like_route() {
        let settings = AgentSettings {
            agent_model_id: Some("gemma4-12b-it-q8_0-mtp".to_string()),
            agent_model_provider: Some("openai-base:local".to_string()),
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
            task: "openai base typed runtime route",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: Some(runtime_registry),
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runtime-openai-base-route-test",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: Some("42".to_string()),
            memory_scope: None,
            memory_behavior: None,
            storage: None,
            config: AgentRunnerConfig::new("gemma4-12b-it-q8_0-mtp".to_string(), 4, 1, 30, 1024)
                .with_model_provider("openai-base:local")
                .with_model_routes(vec![ModelInfo {
                    id: "gemma4-12b-it-q8_0-mtp".to_string(),
                    provider: "openai-base:local".to_string(),
                    max_output_tokens: 1024,
                    context_window_tokens: 8192,
                    weight: 1,
                }]),
        };
        let mut state = RunState::new();
        let tool_call = ToolCall::new(
            "call-read-openai-base",
            ToolCallFunction {
                name: "read_file".to_string(),
                arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new("invoke-read-openai-base")
                .with_provider_tool_call_id("call-read-openai-base")
                .with_protocol(ToolProtocol::ChatLike)
                .with_transport(ToolTransport::ClientRoundTrip),
        );

        let result = runner
            .execute_tools_with_runtime(
                &mut ctx,
                &mut state,
                ToolTurnAssistantContent::NativeModelContent(""),
                None,
                vec![tool_call],
            )
            .await
            .expect("openai-base chat-like route should execute tools");

        assert!(result.is_none());
        let memory = ctx.agent.memory().get_messages();
        assert_eq!(memory.len(), 2);
        assert_eq!(memory[0].kind, AgentMessageKind::AssistantToolCall);
        assert_eq!(memory[1].kind, AgentMessageKind::ToolResult);
        assert_eq!(
            memory[1].tool_call_id.as_deref(),
            Some("invoke-read-openai-base")
        );
        assert!(memory[1].content.contains("runtime-ok"));
    }

    struct ScreenshotRuntimeExecutor;

    #[async_trait]
    impl ToolExecutor for ScreenshotRuntimeExecutor {
        fn name(&self) -> ToolName {
            ToolName::from("browser_observe")
        }

        fn spec(&self) -> ToolDefinition {
            ToolDefinition {
                name: "browser_observe".to_string(),
                description: "observe browser".to_string(),
                parameters: json!({ "type": "object" }),
            }
        }

        async fn execute(
            &self,
            invocation: ToolInvocation,
        ) -> Result<ToolOutput, ToolRuntimeError> {
            let output = OutputNormalizer::new(ToolRuntimeConfig::default())
                .success(&invocation, "observed", "")
                .with_image_attachment(
                    crate::agent::tool_runtime::ToolOutputImageAttachment::image(
                        "screenshot.png",
                        Some("image/png".to_string()),
                        3,
                        "/workspace/uploads/screenshot.png",
                    ),
                );
            Ok(output)
        }
    }

    #[tokio::test]
    async fn typed_runtime_tool_output_image_attachment_is_recorded_in_memory() {
        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-flash".to_string()),
            agent_model_provider: Some("opencode-go".to_string()),
            ..AgentSettings::default()
        };
        let llm_client = Arc::new(LlmClient::new(&settings));
        let mut runner = AgentRunner::new(llm_client);
        let mut runtime_registry = RuntimeToolRegistry::new();
        runtime_registry
            .register(Arc::new(ScreenshotRuntimeExecutor))
            .expect("runtime executor registers");
        let tools = runtime_registry.specs();
        let runtime_registry = Arc::new(runtime_registry);

        let mut session = EphemeralSession::new(4096);
        let todos_arc = Arc::new(tokio::sync::Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "screenshot attachment",
            system_prompt: "system prompt",
            date_suffix: "",
            tools: &tools,
            tool_runtime_registry: Some(runtime_registry),
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runtime-screenshot-attachment-test",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: Some("42".to_string()),
            memory_scope: None,
            memory_behavior: None,
            storage: None,
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
            "call-screenshot-1",
            ToolCallFunction {
                name: "browser_observe".to_string(),
                arguments: r#"{"session_id":"sess-1"}"#.to_string(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new("invoke-screenshot-1")
                .with_provider_tool_call_id("call-screenshot-1")
                .with_protocol(ToolProtocol::ChatLike)
                .with_transport(ToolTransport::ClientRoundTrip),
        );

        let result = runner
            .execute_tools_with_runtime(
                &mut ctx,
                &mut state,
                ToolTurnAssistantContent::NativeModelContent(""),
                None,
                vec![tool_call],
            )
            .await
            .expect("runtime execution succeeds");

        assert!(result.is_none());
        let memory = ctx.agent.memory().get_messages();
        let result_message = memory
            .iter()
            .find(|m| m.tool_name.as_deref() == Some("browser_observe"))
            .expect("browser_observe result should be in memory");
        assert_eq!(result_message.attachments.len(), 1);
        assert_eq!(
            result_message.attachments[0].sandbox_path,
            "/workspace/uploads/screenshot.png"
        );
        assert_eq!(result_message.attachments[0].file_name, "screenshot.png");
        assert_eq!(
            result_message.attachments[0].mime_type.as_deref(),
            Some("image/png")
        );
        assert_eq!(result_message.attachments[0].size_bytes, 3);
    }
}
