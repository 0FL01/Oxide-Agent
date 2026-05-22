//! Tool execution helpers for the agent runner.

use super::hooks::ToolHookDecision;
use super::types::{AgentRunResult, AgentRunnerContext, RunState};
use super::AgentRunner;
use crate::agent::compaction::{
    CompactRequestContext, CompactionBackend, CompactionPhase, CompactionReason, CompactionTrigger,
};
use crate::agent::identity::SessionId;
use crate::agent::memory::AgentMessage;
use crate::agent::progress::AgentEvent;
use crate::agent::providers::TOOL_COMPRESS;
use crate::agent::recovery::sanitize_xml_tags;
use crate::agent::tool_bridge::extract_updated_topic_agents_md;
use crate::agent::tool_model_route::scope_tool_model_route;
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
use std::fmt::Write as _;
use std::sync::{Arc, Mutex as StdMutex};
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

    /// Record a tool call in both the LLM message log and memory.
    pub(super) fn record_assistant_tool_call(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        raw_json: &str,
        tool_calls: &[ToolCall],
        reasoning_content: Option<String>,
    ) {
        let tool_calls_vec = tool_calls.to_vec();
        ctx.messages
            .push(Message::assistant_with_tools_and_reasoning(
                raw_json,
                reasoning_content.clone(),
                tool_calls_vec.clone(),
            ));
        ctx.agent
            .memory_mut()
            .add_message(AgentMessage::assistant_with_tools_and_reasoning(
                raw_json.to_string(),
                reasoning_content,
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
        state: &mut RunState,
        tool_calls: Vec<ToolCall>,
    ) -> anyhow::Result<Option<AgentRunResult>> {
        // Phase 1: Sequential pre-processing - load skills and run hooks
        // This must be sequential because:
        // 1. Hooks may block tools or force finish (decisions affect flow)
        // 2. Skill loading may mutate context
        let mut approved_tools: Vec<(usize, ToolCall)> = Vec::with_capacity(tool_calls.len());
        let mut compress_tools: Vec<(usize, ToolCall)> = Vec::new();
        let mut blocked_results: Vec<(usize, String)> = Vec::new();

        for (idx, tool_call) in tool_calls.iter().enumerate() {
            self.load_skill_context_for_tool(ctx, &tool_call.function.name)
                .await?;

            match self.apply_before_tool_hooks(ctx, state, tool_call)? {
                ToolHookDecision::Continue => {
                    if tool_call.function.name == TOOL_COMPRESS {
                        compress_tools.push((idx, tool_call.clone()));
                    } else {
                        approved_tools.push((idx, tool_call.clone()));
                    }
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
        if approved_tools.is_empty() && compress_tools.is_empty() {
            return Ok(None);
        }

        for (_idx, tool_call) in compress_tools {
            let result = self.execute_compress_tool(ctx, &tool_call).await;
            if self
                .record_tool_execution_result(ctx, state, tool_call, result)
                .await?
            {
                return Ok(Some(AgentRunResult::WaitingForApproval));
            }

            Self::emit_token_snapshot_update(
                ctx.progress_tx,
                Self::build_token_snapshot(ctx, CompactionTrigger::PreIteration),
            )
            .await;
        }

        self.execute_approved_tools(ctx, state, approved_tools)
            .await
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

    async fn execute_approved_tools(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        approved_tools: Vec<(usize, ToolCall)>,
    ) -> anyhow::Result<Option<AgentRunResult>> {
        if approved_tools.is_empty() {
            return Ok(None);
        }

        // Phase 2: Parallel execution of approved tools
        // Execute raw tool calls in parallel through the registry
        let cancellation_token = ctx.agent.cancellation_token().clone();
        let active_route = current_execution_model_route(ctx);
        let tool_futures: Vec<_> = approved_tools
            .into_iter()
            .map(|(idx, tool_call)| {
                let registry = ctx.registry;
                let progress_tx = ctx.progress_tx.cloned();
                let cancellation_token = cancellation_token.clone();
                let active_route = active_route.clone();
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
                    let execution = registry.execute(
                        &tool_call.function.name,
                        &tool_call.function.arguments,
                        progress_tx.as_ref(),
                        Some(&cancellation_token),
                    );
                    let result = if let Some(route) = active_route {
                        scope_tool_model_route(route, execution).await
                    } else {
                        execution.await
                    };

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

    async fn execute_compress_tool(
        &self,
        ctx: &mut AgentRunnerContext<'_>,
        tool_call: &ToolCall,
    ) -> anyhow::Result<String> {
        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::ToolCall {
                    name: sanitize_xml_tags(&tool_call.function.name),
                    input: sanitize_xml_tags(&tool_call.function.arguments),
                    command_preview: None,
                })
                .await;
        }

        Self::execute_runtime_compress_tool(ctx).await
    }

    async fn execute_runtime_compress_tool(
        ctx: &mut AgentRunnerContext<'_>,
    ) -> anyhow::Result<String> {
        let Some(controller) = ctx.compaction_controller else {
            return Err(anyhow::anyhow!(
                "compress tool is unavailable in this runner context"
            ));
        };

        Self::emit_runtime_tool_compaction_started(
            ctx.progress_tx,
            ctx.agent.memory().token_count(),
            ctx.agent.memory().get_messages().len(),
        )
        .await;
        let context = CompactRequestContext {
            task: ctx.task.to_string(),
            route: current_execution_model_route(ctx).ok_or_else(|| {
                anyhow::anyhow!("compress requires an active runtime model route")
            })?,
            reason: CompactionReason::Manual,
            phase: CompactionPhase::Manual,
            target_token_budget: ctx.agent.memory().max_tokens(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        let cancellation_token = ctx.agent.cancellation_token().clone();
        let outcome = match Self::await_until_cancelled(
            cancellation_token,
            controller.manual_compact(ctx.agent.memory_mut(), context),
        )
        .await
        {
            Some(Ok(outcome)) => outcome,
            Some(Err(error)) => {
                Self::emit_runtime_tool_compaction_failed(ctx.progress_tx, error.to_string()).await;
                return Err(error.into());
            }
            None => {
                if let Some(tx) = ctx.progress_tx {
                    let _ = tx.send(AgentEvent::Cancelled).await;
                }
                Self::emit_runtime_tool_compaction_failed(
                    ctx.progress_tx,
                    "Task cancelled by user".to_string(),
                )
                .await;
                return Err(anyhow::anyhow!("Task cancelled by user"));
            }
        };

        ctx.agent.persist_memory_checkpoint_background();
        Self::refresh_messages_from_memory(ctx);
        Self::emit_runtime_tool_compaction_completed(ctx.progress_tx, &outcome).await;

        Ok(serde_json::json!({
            "ok": true,
            "applied": true,
            "summary_updated": true,
            "reason": "manual",
            "phase": "manual",
            "backend": outcome.metadata.backend.as_str(),
            "provider": outcome.metadata.provider,
            "route": outcome.metadata.route,
            "generation": outcome.metadata.generation,
            "tokens_before": outcome.replacement.token_before,
            "tokens_after": outcome.replacement.token_after,
            "reclaimed_tokens": outcome.replacement.token_before.saturating_sub(outcome.replacement.token_after),
            "history_items_before": outcome.replacement.history_items_before,
            "history_items_after": outcome.replacement.history_items_after,
        })
        .to_string())
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

        let tool_result = crate::agent::tool_bridge::ToolExecutionResult::Completed {
            tool_name: tool_name.clone(),
            output: content.clone(),
        };
        self.apply_after_tool_hooks(ctx, state, &tool_result);

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

    async fn emit_runtime_tool_compaction_started(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        token_before: usize,
        history_items_before: usize,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::RuntimeCompactionStarted {
                    reason: CompactionReason::Manual,
                    phase: CompactionPhase::Manual,
                    backend: CompactionBackend::LocalLlmSummary,
                    provider: None,
                    route: None,
                    token_before,
                    history_items_before,
                })
                .await;
        }
    }

    async fn emit_runtime_tool_compaction_completed(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        outcome: &crate::agent::compaction::CompactRunOutcome,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::RuntimeCompactionCompleted {
                    reason: outcome.metadata.reason,
                    phase: outcome.metadata.phase,
                    backend: outcome.metadata.backend,
                    provider: outcome.metadata.provider.clone(),
                    route: outcome.metadata.route.clone(),
                    token_before: outcome.replacement.token_before,
                    token_after: outcome.replacement.token_after,
                    history_items_before: outcome.replacement.history_items_before,
                    history_items_after: outcome.replacement.history_items_after,
                    generation: outcome.metadata.generation,
                    repair_applied: outcome.metadata.repair_applied,
                })
                .await;
        }
    }

    async fn emit_runtime_tool_compaction_failed(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        error: String,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::RuntimeCompactionFailed {
                    reason: CompactionReason::Manual,
                    phase: CompactionPhase::Manual,
                    backend: CompactionBackend::LocalLlmSummary,
                    provider: None,
                    route: None,
                    error,
                })
                .await;
        }
    }

    async fn record_tool_execution_result(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        tool_call: ToolCall,
        result: anyhow::Result<String>,
    ) -> anyhow::Result<bool> {
        let (output, success) = match result {
            Ok(output) => (output, true),
            Err(e) => {
                warn!(
                    tool = %tool_call.function.name,
                    error = %e,
                    error_chain = %format_error_chain(&e),
                    "Tool execution failed"
                );
                (format!("Tool execution error: {e}"), false)
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
                    success,
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
                    success: false,
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
            skill_registry: None,
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
            skill_registry: None,
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
}
