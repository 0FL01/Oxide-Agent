//! Core execution loop for the agent runner.

use super::types::{AgentRunResult, AgentRunnerContext, RunState};
use super::AgentRunner;
use crate::agent::compaction::CompactionTrigger;
use crate::agent::memory::AgentMessage;
use crate::agent::progress::AgentEvent;
use anyhow::{anyhow, Result};
use std::future::Future;
use tracing::debug;

impl AgentRunner {
    /// Execute the agent loop until completion or error.
    pub async fn run(&mut self, ctx: &mut AgentRunnerContext<'_>) -> Result<AgentRunResult> {
        self.reset_loop_detector(ctx).await;
        self.apply_before_agent_hooks(ctx)?;
        self.run_loop(ctx).await
    }

    async fn run_loop(&mut self, ctx: &mut AgentRunnerContext<'_>) -> Result<AgentRunResult> {
        let mut state = RunState::new();

        for iteration in 0..ctx.config.max_iterations {
            state.iteration = iteration;

            if ctx.agent.cancellation_token().is_cancelled() {
                return Err(self.cancelled_error(ctx).await);
            }

            if ctx.agent.elapsed_secs() >= ctx.config.timeout_secs {
                if let Some(res) = self.apply_timeout_hook(ctx, &mut state)? {
                    return Ok(AgentRunResult::Final(res));
                }
            }

            self.apply_pending_runtime_context(ctx, &mut state).await;

            self.run_pre_llm_maintenance(ctx, &mut state, iteration)
                .await?;

            debug!(task_id = %ctx.task_id, iteration = iteration, "Agent loop iteration");

            let snapshot_trigger = if iteration == 0 {
                CompactionTrigger::PreRun
            } else {
                CompactionTrigger::PreIteration
            };
            let snapshot = Self::build_token_snapshot(ctx, snapshot_trigger);
            Self::log_token_snapshot(ctx, iteration, "before_llm_call", &snapshot);
            if let Some(tx) = ctx.progress_tx {
                let _ = tx.send(AgentEvent::Thinking { snapshot }).await;
                // Emit milestone when Thinking event is sent (not just received).
                let timestamp_ms = chrono::Utc::now().timestamp_millis();
                let _ = tx
                    .send(AgentEvent::Milestone {
                        name: "thinking_sent".to_string(),
                        timestamp_ms,
                    })
                    .await;
            }

            let cancellation_token = ctx.agent.cancellation_token().clone();
            let Some(loop_detected) = Self::await_until_cancelled(
                cancellation_token,
                self.llm_loop_detected(ctx, &state),
            )
            .await
            else {
                return Err(self.cancelled_error(ctx).await);
            };
            if loop_detected {
                return Err(self
                    .loop_detected_error(
                        ctx,
                        &state,
                        crate::agent::loop_detection::LoopType::CognitiveLoop,
                    )
                    .await);
            }

            let cancellation_token = ctx.agent.cancellation_token().clone();
            let Some(response) = Self::await_until_cancelled(
                cancellation_token,
                self.call_llm_with_tools(ctx, &mut state, iteration),
            )
            .await
            else {
                return Err(self.cancelled_error(ctx).await);
            };
            let response = response?;
            if let Some(result) = self.handle_llm_response(response, ctx, &mut state).await? {
                return Ok(result);
            }
        }

        Err(anyhow!(
            "Agent exceeded iteration limit ({}).",
            ctx.config.max_iterations
        ))
    }

    async fn run_pre_llm_maintenance(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
        iteration: usize,
    ) -> Result<()> {
        self.apply_before_iteration_hooks(ctx, state)?;

        let cancellation_token = ctx.agent.cancellation_token().clone();
        if state.take_manual_compaction_request() {
            let Some(compaction_result) = Self::await_until_cancelled(
                cancellation_token.clone(),
                self.run_manual_compaction_checkpoint(ctx, state),
            )
            .await
            else {
                return Err(self.cancelled_error(ctx).await);
            };
            compaction_result?;
        } else {
            let Some(compaction_result) = Self::await_until_cancelled(
                cancellation_token,
                self.run_iteration_compaction(ctx, state, iteration),
            )
            .await
            else {
                return Err(self.cancelled_error(ctx).await);
            };
            compaction_result?;
        }

        if iteration == 0 {
            Self::emit_pre_run_compaction_done(ctx.progress_tx).await;
        }

        Ok(())
    }

    async fn emit_pre_run_compaction_done(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) {
        let Some(tx) = progress_tx else { return };

        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        let _ = tx
            .send(AgentEvent::Milestone {
                name: "pre_run_compaction_done".to_string(),
                timestamp_ms,
            })
            .await;
    }

    pub(super) async fn await_until_cancelled<T, F>(
        cancellation_token: tokio_util::sync::CancellationToken,
        future: F,
    ) -> Option<T>
    where
        F: Future<Output = T>,
    {
        tokio::pin!(future);

        tokio::select! {
            result = &mut future => Some(result),
            _ = cancellation_token.cancelled() => None,
        }
    }

    async fn apply_pending_runtime_context(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
        state: &mut RunState,
    ) {
        let pending_context = ctx.agent.drain_runtime_context();
        if pending_context.is_empty() {
            return;
        }

        state.continuation_count += 1;
        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::Continuation {
                    reason: "New user context received, adapting the plan.".to_string(),
                    count: state.continuation_count,
                })
                .await;
        }

        for injection in pending_context {
            ctx.messages
                .push(crate::llm::Message::user(&injection.content));
            ctx.agent
                .memory_mut()
                .add_message(AgentMessage::runtime_context(injection.content));
        }
    }

    pub(super) async fn cancelled_error(
        &mut self,
        ctx: &mut AgentRunnerContext<'_>,
    ) -> anyhow::Error {
        if let Some(tx) = ctx.progress_tx {
            let _ = tx.send(AgentEvent::Cancelled).await;
        }

        anyhow!("Task cancelled by user")
    }

    // Additional response and runtime helpers live in sibling runner modules.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::{
        AgentMessageKind, CompactSummaryBackend, CompactSummaryError, CompactSummaryRequest,
        CompactSummaryResult, CompactedSummaryMetadata, CompactionBackend, CompactionController,
        CompactionPhase, CompactionReason, OXIDE_COMPACTED_SUMMARY_PREFIX,
    };
    use crate::agent::context::{AgentContext, EphemeralSession};
    use crate::agent::hooks::CompletionCheckHook;
    use crate::agent::providers::{TodoItem, TodoList};
    use crate::agent::runner::types::FinalResponseInput;
    use crate::agent::runner::{AgentRunResult, AgentRunnerConfig, AgentRunnerContext};
    use crate::agent::tool_runtime::ToolRegistry as RuntimeToolRegistry;
    use crate::config::{AgentSettings, ModelInfo};
    use crate::llm::{
        ChatResponse, LlmClient, LlmError, MockLlmProvider, TokenUsage, ToolCall, ToolCallFunction,
    };
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct StaticRuntimeSummaryBackend;

    fn existing_compacted_summary() -> AgentMessage {
        AgentMessage::compacted_summary(
            "Old current-format state.",
            &CompactedSummaryMetadata {
                generation: 1,
                reason: CompactionReason::Manual,
                phase: CompactionPhase::Manual,
                token_before: 100,
                token_after: 10,
                history_items_before: 3,
                history_items_after: 1,
                provider: "opencode-go".to_string(),
                route: "deepseek-v4-flash".to_string(),
                backend: CompactionBackend::LocalLlmSummary,
                created_at: "2026-05-21T20:10:00+03:00".to_string(),
                previous_summary_detected: false,
                repair_applied: false,
                wiki_memory_lookup_available: false,
            },
        )
    }

    #[async_trait]
    impl CompactSummaryBackend for StaticRuntimeSummaryBackend {
        async fn summarize(
            &self,
            request: CompactSummaryRequest<'_>,
        ) -> std::result::Result<CompactSummaryResult, CompactSummaryError> {
            Ok(CompactSummaryResult {
                summary_text: "Condensed history for smaller fallback route.".to_string(),
                provider: request.route.provider.clone(),
                route: request.route.id.clone(),
            })
        }
    }

    #[test]
    fn json_mode_forbids_chatgpt_routes_only() {
        let chatgpt_routes = [
            ModelInfo {
                id: "gpt-5.4-mini".to_string(),
                provider: "chatgpt".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            },
            ModelInfo {
                id: "gpt-5.4-mini".to_string(),
                provider: "openai-chatgpt".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            },
            ModelInfo {
                id: "gpt-5.4-mini".to_string(),
                provider: "llm-provider/openai-chatgpt".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            },
        ];
        let zai_route = ModelInfo {
            id: "glm-4.7".to_string(),
            provider: "zai".to_string(),
            max_output_tokens: 32_000,
            context_window_tokens: 200_000,
            weight: 1,
        };

        for route in chatgpt_routes {
            assert!(AgentRunner::json_mode_forbids_route(true, &route));
            assert!(!AgentRunner::json_mode_forbids_route(false, &route));
        }
        assert!(!AgentRunner::json_mode_forbids_route(true, &zai_route));
    }

    #[test]
    fn structured_output_requirement_uses_active_provider_without_registry_lookup() {
        let llm_client = build_llm_client(single_final_response_provider());
        let runner = AgentRunner::new(llm_client);
        let config = AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 8, 4, 60, 4096)
            .with_model_provider("llm-provider/opencode-go")
            .with_model_routes(vec![ModelInfo {
                id: "gpt-5.4-mini".to_string(),
                provider: "chatgpt".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            }]);

        assert!(runner.structured_output_required_for_config(&config));
    }

    #[tokio::test]
    async fn forced_final_response_is_saved_as_undelivered_draft() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(llm_client);
        runner.register_hook(Box::new(CompletionCheckHook::new()));

        let mut session = EphemeralSession::new(2048);
        let mut todos = TodoList::new();
        todos.items.push(TodoItem::new("finish work"));
        session.memory_mut().todos = todos.clone();
        let todos_arc = Arc::new(Mutex::new(todos));
        let tools = Vec::new();
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "produce report",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "forced-final-draft",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("test-model".to_string(), 8, 4, 60, 4096),
        };
        let mut state = RunState::new();
        let draft = "Full report generated before todos were complete.";

        let result = runner
            .handle_final_response(
                &mut ctx,
                &mut state,
                FinalResponseInput {
                    final_answer: draft.to_string(),
                    reasoning: None,
                },
            )
            .await
            .expect("forced final response should continue");

        assert!(result.is_none());
        let memory = ctx.agent.memory().get_messages();
        assert!(
            !memory.iter().any(|message| {
                message.resolved_kind() == AgentMessageKind::AssistantResponse
                    && message.content.contains(draft)
            }),
            "forced final response must not be stored as delivered assistant prose"
        );
        let draft_message = memory
            .iter()
            .find(|message| message.resolved_kind() == AgentMessageKind::UndeliveredAssistantDraft)
            .expect("undelivered draft should be recorded");
        assert!(draft_message.content.contains("not delivered to the user"));
        assert!(draft_message.content.contains(draft));
    }

    #[test]
    fn unbounded_retry_is_limited_to_opencode_go_retryable_errors() {
        let rate_limit = LlmError::RateLimit {
            wait_secs: None,
            message: "too many requests".to_string(),
        };
        let invalid_request = LlmError::ApiError("invalid API key".to_string());

        assert!(AgentRunner::opencode_go_unbounded_retry_allowed(
            "opencode-go",
            &rate_limit
        ));
        assert!(AgentRunner::opencode_go_unbounded_retry_allowed(
            "opencode_go",
            &LlmError::NetworkError("connection reset".to_string())
        ));
        assert!(!AgentRunner::opencode_go_unbounded_retry_allowed(
            "openrouter",
            &rate_limit
        ));
        assert!(!AgentRunner::opencode_go_unbounded_retry_allowed(
            "opencode-go",
            &invalid_request
        ));
    }

    #[test]
    fn structured_output_requirement_disables_chatgpt_primary_route() {
        let llm_client = build_llm_client(single_final_response_provider());
        let runner = AgentRunner::new(llm_client);
        let config = AgentRunnerConfig::new("gpt-5.4-mini".to_string(), 8, 4, 60, 4096)
            .with_model_provider("chatgpt")
            .with_model_routes(vec![ModelInfo {
                id: "gpt-5.4-mini".to_string(),
                provider: "chatgpt".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            }]);

        assert!(!runner.structured_output_required_for_config(&config));
    }

    #[cfg(feature = "llm-chatgpt")]
    #[test]
    fn select_model_route_index_keeps_chatgpt_route_when_structured_output_is_disabled() {
        let llm_client = build_llm_client_for_provider(
            single_final_response_provider(),
            "chatgpt",
            "gpt-5.4-mini",
        );
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(768);
        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let ctx = AgentRunnerContext {
            task: "Route selection regression",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-chatgpt-route-selection",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("gpt-5.4-mini".to_string(), 8, 4, 60, 4096)
                .with_model_provider("chatgpt")
                .with_model_routes(vec![ModelInfo {
                    id: "gpt-5.4-mini".to_string(),
                    provider: "chatgpt".to_string(),
                    max_output_tokens: 32_000,
                    context_window_tokens: 200_000,
                    weight: 1,
                }]),
        };

        assert_eq!(
            runner.select_model_route_index(&ctx, &std::collections::HashSet::new()),
            Some(0)
        );
    }

    #[test]
    fn select_model_route_index_does_not_fail_over_typed_runtime_to_non_v1_route() {
        let mut opencode = MockLlmProvider::new();
        stub_non_chat_methods(&mut opencode);
        let mut openrouter = MockLlmProvider::new();
        stub_non_chat_methods(&mut openrouter);

        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-flash".to_string()),
            agent_model_provider: Some("llm-provider/opencode-go".to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm = LlmClient::new(&settings);
        llm.register_provider("opencode-go".to_string(), Arc::new(opencode));
        llm.register_provider("openrouter".to_string(), Arc::new(openrouter));
        let llm_client = Arc::new(llm);

        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(768);
        let tools: Vec<crate::llm::ToolDefinition> = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let ctx = AgentRunnerContext {
            task: "Typed runtime route selection",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: Some(Arc::new(RuntimeToolRegistry::new())),
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-typed-route-selection",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 8, 4, 60, 4096)
                .with_model_provider("llm-provider/opencode-go")
                .with_model_routes(vec![
                    ModelInfo {
                        id: "deepseek-v4-flash".to_string(),
                        provider: "opencode-go".to_string(),
                        max_output_tokens: 4096,
                        context_window_tokens: 200_000,
                        weight: 1,
                    },
                    ModelInfo {
                        id: "deepseek-v4-flash".to_string(),
                        provider: "openrouter".to_string(),
                        max_output_tokens: 4096,
                        context_window_tokens: 200_000,
                        weight: 10,
                    },
                ]),
        };

        assert_eq!(
            runner.select_model_route_index(&ctx, &std::collections::HashSet::new()),
            Some(0)
        );

        let mut exhausted = std::collections::HashSet::new();
        exhausted.insert(AgentRunner::route_key(&ctx.config.model_routes[0]));
        assert_eq!(runner.select_model_route_index(&ctx, &exhausted), None);
    }

    #[test]
    fn structured_output_requirement_uses_primary_route_before_selection() {
        let llm_client = build_llm_client(single_final_response_provider());
        let runner = AgentRunner::new(llm_client);
        let config = AgentRunnerConfig::new("missing-model-name".to_string(), 8, 4, 60, 4096)
            .with_model_routes(vec![ModelInfo {
                id: "deepseek-v4-flash".to_string(),
                provider: "opencode-go".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            }]);

        assert!(runner.structured_output_required_for_config(&config));
    }

    #[tokio::test]
    async fn run_repairs_invalid_tool_history_before_llm_call() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(768);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Repair invalid tool history"));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant_with_tools(
                "Calling tools",
                vec![
                    ToolCall::new(
                        "call-1".to_string(),
                        ToolCallFunction {
                            name: "search".to_string(),
                            arguments: "{}".to_string(),
                        },
                        false,
                    ),
                    ToolCall::new(
                        "call-2".to_string(),
                        ToolCallFunction {
                            name: "read_file".to_string(),
                            arguments: "{}".to_string(),
                        },
                        false,
                    ),
                ],
            ));
        session
            .memory_mut()
            .add_message(AgentMessage::tool("call-1", "search", "result-1"));

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let mut ctx = AgentRunnerContext {
            task: "Repair invalid tool history",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-history-repair",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 2, 1, 30, 256),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");

        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));
        let repaired_call = ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .find(|message| message.tool_calls.is_some())
            .expect("assistant tool call must remain after repair");
        let repaired_tool_calls = repaired_call
            .tool_calls
            .as_ref()
            .expect("tool call batch must still exist");
        assert_eq!(repaired_tool_calls.len(), 1);
        assert_eq!(repaired_tool_calls[0].id, "call-1");
        assert!(!ctx.agent.memory().get_messages().iter().any(|message| {
            message.tool_calls.as_ref().is_some_and(|tool_calls| {
                tool_calls.iter().any(|tool_call| tool_call.id == "call-2")
            })
        }));
    }

    #[tokio::test]
    async fn run_unstructured_mode_parses_accidental_structured_final_answer() {
        let llm_client = build_llm_client_for_provider(
            accidental_structured_final_answer_provider(),
            "opencode-go",
            "unstructured-model",
        );
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(768);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Какие инструменты тебе доступны?"));

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let mut ctx = AgentRunnerContext {
            task: "Какие инструменты тебе доступны?",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-unstructured-structured-fallback",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("unstructured-model".to_string(), 1, 1, 30, 256),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");

        assert!(
            matches!(result, AgentRunResult::Final(answer) if answer == "Tools: `read_file`, `write_file`")
        );
        let last_message = ctx
            .agent
            .memory()
            .get_messages()
            .last()
            .expect("assistant response should be saved");
        assert_eq!(last_message.content, "Tools: `read_file`, `write_file`");
        assert!(!last_message.content.contains("\"final_answer\""));
    }

    #[tokio::test]
    async fn run_retries_after_context_overflow_with_runtime_context_limit_compaction() {
        let llm_client = build_llm_client(context_overflow_then_summary_then_final_provider());
        let compaction_controller = CompactionController::local_llm(Arc::clone(&llm_client), 1);
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Retry after overflow"));
        session
            .memory_mut()
            .add_message(existing_compacted_summary());
        session
            .memory_mut()
            .add_message(AgentMessage::user("Recent request."));

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let mut ctx = AgentRunnerContext {
            task: "Retry after overflow",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-overflow-runtime-compaction",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: Some(&compaction_controller),
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 2, 1, 30, 256),
        };

        let result = runner
            .run(&mut ctx)
            .await
            .expect("runner succeeds after retry");

        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));
        assert!(ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .any(|message| message
                .content
                .starts_with(crate::agent::compaction::OXIDE_COMPACTED_SUMMARY_PREFIX)));
        assert!(ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .all(|message| !message.content.contains("[COMPACTION_SUMMARY]")));
        drop(ctx);
        drop(progress_tx);

        let events = collect_progress_events(&mut progress_rx).await;
        let started = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::RuntimeCompactionStarted {
                        reason: CompactionReason::ContextLimit,
                        phase: CompactionPhase::MidTurn,
                        ..
                    }
                )
            })
            .expect("runtime context-limit compaction started");
        let completed = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::RuntimeCompactionCompleted {
                        reason: CompactionReason::ContextLimit,
                        phase: CompactionPhase::MidTurn,
                        ..
                    }
                )
            })
            .expect("runtime context-limit compaction completed");
        assert!(started < completed);
    }

    #[tokio::test]
    async fn run_pre_sampling_uses_runtime_compaction_when_threshold_reached() {
        let llm_client = build_llm_client(pre_sampling_summary_then_final_provider());
        let compaction_controller = CompactionController::local_llm(Arc::clone(&llm_client), 1);
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(1_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Pre-sampling compact"));
        session
            .memory_mut()
            .add_message(AgentMessage::user("older ".repeat(4_000)));

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let mut ctx = AgentRunnerContext {
            task: "Pre-sampling compact",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-pre-sampling-runtime-compaction",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: Some(&compaction_controller),
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 1, 1, 30, 256),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");

        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));
        assert!(ctx
            .agent
            .memory()
            .get_messages()
            .iter()
            .any(|message| message
                .content
                .starts_with(crate::agent::compaction::OXIDE_COMPACTED_SUMMARY_PREFIX)));
        drop(ctx);
        drop(progress_tx);

        let events = collect_progress_events(&mut progress_rx).await;
        assert!(events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::RuntimeCompactionCompleted {
                    reason: CompactionReason::PreTurn,
                    phase: CompactionPhase::PreSampling,
                    ..
                }
            )
        }));
    }

    #[test]
    fn runtime_compaction_threshold_uses_full_request_budget() {
        let tools = Vec::new();
        let mut session = EphemeralSession::new(20_000);
        session.memory_mut().add_message(AgentMessage::user_task(
            "Compact because output reserve consumes the route window",
        ));

        assert!(
            session.memory().token_count().saturating_mul(100)
                < session.memory().max_tokens().saturating_mul(85),
            "test fixture must stay below the old hot-memory-only threshold"
        );

        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let ctx = AgentRunnerContext {
            task: "Compact because output reserve consumes the route window",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-full-budget-threshold",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 1, 1, 30, 16_000),
        };
        let route = ModelInfo {
            id: "deepseek-v4-flash".to_string(),
            provider: "opencode-go".to_string(),
            max_output_tokens: 16_000,
            context_window_tokens: 20_000,
            weight: 1,
        };

        assert!(AgentRunner::runtime_compaction_threshold_reached(
            &ctx, &route
        ));
        drop(ctx);
    }

    #[test]
    fn refresh_messages_from_memory_drops_transient_messages() {
        let tools = Vec::new();
        let mut session = EphemeralSession::new(1024);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("refresh transient context"));
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        messages.push(crate::llm::Message::system("temporary warning"));
        let mut ctx = AgentRunnerContext {
            task: "refresh transient context",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "refresh-transient-test",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 1, 1, 30, 256),
        };

        AgentRunner::refresh_messages_from_memory(&mut ctx);

        assert!(!ctx
            .messages
            .iter()
            .any(|message| message.role == "system" && message.content == "temporary warning"));
    }

    #[tokio::test]
    async fn tool_calls_without_typed_runtime_fail_without_history_mutation() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(llm_client);
        let mut session = EphemeralSession::new(2048);
        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = Vec::new();
        let mut ctx = AgentRunnerContext {
            task: "tool runtime missing",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "tool-runtime-missing",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: Some("42".to_string()),
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 4, 1, 30, 1024)
                .with_model_provider("llm-provider/opencode-go"),
        };
        let mut state = RunState::new();
        let response = ChatResponse {
            content: Some("assistant raw".to_string()),
            tool_calls: vec![ToolCall::new(
                "invoke-runtime-missing",
                ToolCallFunction {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
                },
                false,
            )],
            finish_reason: "tool_calls".to_string(),
            reasoning_content: None,
            usage: None,
        };

        let error = match runner
            .handle_llm_response(response, &mut ctx, &mut state)
            .await
        {
            Ok(_) => panic!("tool calls without typed runtime must fail"),
            Err(error) => error,
        };

        assert!(error
            .to_string()
            .contains("tool runtime registry is required for active tool calls"));
        assert!(
            ctx.agent.memory().get_messages().is_empty(),
            "missing tool runtime must not write partial assistant/tool history"
        );
        assert!(ctx.messages.is_empty());
    }

    #[tokio::test]
    async fn preprocess_llm_response_keeps_memory_tokens_separate_from_api_usage() {
        let llm_client = build_llm_client(single_final_response_provider());
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Inspect token metrics"));
        session
            .memory_mut()
            .add_message(AgentMessage::assistant("Recent response"));

        let estimated_tokens = session.memory().token_count();
        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let mut ctx = AgentRunnerContext {
            task: "Inspect token metrics",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: "runner-token-metrics",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-flash".to_string(), 1, 1, 30, 256),
        };
        let mut response = ChatResponse {
            content: Some("done".to_string()),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            reasoning_content: None,
            usage: Some(TokenUsage {
                prompt_tokens: 9_000,
                completion_tokens: 512,
                total_tokens: 9_512,
            }),
        };

        runner
            .preprocess_llm_response(&mut response, &mut ctx)
            .await;

        assert_eq!(ctx.agent.memory().token_count(), estimated_tokens);
        assert_eq!(ctx.agent.memory().api_token_count(), Some(9_512));
    }

    #[tokio::test]
    async fn run_fails_over_to_weighted_backup_after_persistent_rate_limits() {
        let mut primary = MockLlmProvider::new();
        primary
            .expect_chat_with_tools()
            .times(LlmClient::MAX_RETRIES)
            .returning(|_| {
                Err(LlmError::RateLimit {
                    wait_secs: Some(0),
                    message: "primary rate limit".to_string(),
                })
            });
        stub_non_chat_methods(&mut primary);

        let mut backup = MockLlmProvider::new();
        backup
            .expect_chat_with_tools()
            .times(1)
            .return_once(|_| Ok(final_structured_response()));
        stub_non_chat_methods(&mut backup);

        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-pro".to_string()),
            agent_model_provider: Some("llm-provider/opencode-go".to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm_client = LlmClient::new(&settings);
        llm_client.register_provider("llm-provider/opencode-go".to_string(), Arc::new(primary));
        llm_client.register_provider("opencode-go".to_string(), Arc::new(backup));
        let llm_client = Arc::new(llm_client);

        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Fail over after persistent 429"));

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(32);
        let mut ctx = AgentRunnerContext {
            task: "Fail over after persistent 429",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-provider-failover",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-pro".to_string(), 1, 1, 30, 256)
                .with_model_provider("llm-provider/opencode-go")
                .with_model_routes(vec![
                    ModelInfo {
                        id: "deepseek-v4-pro".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 128_000,
                        provider: "llm-provider/opencode-go".to_string(),
                        weight: 1,
                    },
                    ModelInfo {
                        id: "deepseek-v4-flash".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 128_000,
                        provider: "opencode-go".to_string(),
                        weight: 3,
                    },
                ]),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");
        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));

        drop(ctx);
        drop(progress_tx);
        let events = collect_progress_events(&mut progress_rx).await;
        assert!(events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::ProviderFailoverActivated {
                    from_provider,
                    from_model,
                    to_provider,
                    to_model,
                } if from_provider == "llm-provider/opencode-go"
                    && from_model == "deepseek-v4-pro"
                    && to_provider == "opencode-go"
                    && to_model == "deepseek-v4-flash"
            )
        }));
    }

    #[tokio::test]
    async fn run_compacts_before_downshifting_to_smaller_model_route() {
        let mut primary = MockLlmProvider::new();
        primary
            .expect_chat_with_tools()
            .times(LlmClient::MAX_RETRIES)
            .returning(|_| {
                Err(LlmError::RateLimit {
                    wait_secs: Some(0),
                    message: "primary rate limit".to_string(),
                })
            });
        stub_non_chat_methods(&mut primary);

        let mut backup = MockLlmProvider::new();
        backup
            .expect_chat_with_tools()
            .times(1)
            .withf(|request| {
                request
                    .system_prompt
                    .contains(OXIDE_COMPACTED_SUMMARY_PREFIX)
                    && !request.messages.iter().any(|message| {
                        message
                            .content
                            .trim_start()
                            .starts_with(OXIDE_COMPACTED_SUMMARY_PREFIX)
                    })
            })
            .return_once(|_| Ok(final_structured_response()));
        stub_non_chat_methods(&mut backup);

        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-pro".to_string()),
            agent_model_provider: Some("llm-provider/opencode-go".to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm_client = LlmClient::new(&settings);
        llm_client.register_provider("llm-provider/opencode-go".to_string(), Arc::new(primary));
        llm_client.register_provider("opencode-go".to_string(), Arc::new(backup));
        let llm_client = Arc::new(llm_client);

        let compaction_controller =
            CompactionController::new(Arc::new(StaticRuntimeSummaryBackend));
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(50_000);
        session.memory_mut().add_message(AgentMessage::user_task(
            "Fail over to a smaller model route",
        ));
        for index in 0..24 {
            session
                .memory_mut()
                .add_message(AgentMessage::user_turn(format!(
                    "background-{index}: {}",
                    "route downshift history ".repeat(160)
                )));
        }

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let mut ctx = AgentRunnerContext {
            task: "Fail over to a smaller model route",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-model-downshift",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: Some(&compaction_controller),
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-pro".to_string(), 1, 1, 30, 256)
                .with_model_provider("llm-provider/opencode-go")
                .with_model_routes(vec![
                    ModelInfo {
                        id: "deepseek-v4-pro".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 128_000,
                        provider: "llm-provider/opencode-go".to_string(),
                        weight: 1,
                    },
                    ModelInfo {
                        id: "deepseek-v4-flash".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 15_000,
                        provider: "opencode-go".to_string(),
                        weight: 1,
                    },
                ]),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");
        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));

        assert!(ctx.agent.memory().get_messages().iter().any(|message| {
            message
                .content
                .trim_start()
                .starts_with(OXIDE_COMPACTED_SUMMARY_PREFIX)
        }));

        drop(ctx);
        drop(progress_tx);
        let events = collect_progress_events(&mut progress_rx).await;
        assert!(events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::RuntimeCompactionCompleted {
                    reason: CompactionReason::ModelDownshift,
                    phase: CompactionPhase::ModelSwitch,
                    provider,
                    route,
                    ..
                } if provider == "opencode-go" && route == "deepseek-v4-flash"
            )
        }));
    }

    #[tokio::test]
    async fn run_keeps_primary_provider_when_rate_limit_recovers() {
        let mut primary = MockLlmProvider::new();
        let mut sequence = mockall::Sequence::new();
        primary
            .expect_chat_with_tools()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_| {
                Err(LlmError::RateLimit {
                    wait_secs: Some(0),
                    message: "primary temporary rate limit".to_string(),
                })
            });
        primary
            .expect_chat_with_tools()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_| Ok(final_structured_response()));
        stub_non_chat_methods(&mut primary);

        let mut backup = MockLlmProvider::new();
        backup.expect_chat_with_tools().times(0);
        stub_non_chat_methods(&mut backup);

        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-pro".to_string()),
            agent_model_provider: Some("llm-provider/opencode-go".to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm_client = LlmClient::new(&settings);
        llm_client.register_provider("llm-provider/opencode-go".to_string(), Arc::new(primary));
        llm_client.register_provider("opencode-go".to_string(), Arc::new(backup));
        let llm_client = Arc::new(llm_client);

        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Stay on primary when it wakes up"));

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(32);
        let mut ctx = AgentRunnerContext {
            task: "Stay on primary when it wakes up",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-primary-recovery",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-v4-pro".to_string(), 1, 1, 30, 256)
                .with_model_provider("llm-provider/opencode-go")
                .with_model_routes(vec![
                    ModelInfo {
                        id: "deepseek-v4-pro".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 128_000,
                        provider: "llm-provider/opencode-go".to_string(),
                        weight: 1,
                    },
                    ModelInfo {
                        id: "deepseek-v4-flash".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 128_000,
                        provider: "opencode-go".to_string(),
                        weight: 2,
                    },
                ]),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");
        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));

        drop(ctx);
        drop(progress_tx);
        let events = collect_progress_events(&mut progress_rx).await;
        assert!(!events
            .iter()
            .any(|event| { matches!(event, AgentEvent::ProviderFailoverActivated { .. }) }));
    }

    #[tokio::test]
    async fn run_skips_unsupported_nvidia_route_and_uses_backup() {
        let mut unsupported_nvidia = MockLlmProvider::new();
        unsupported_nvidia.expect_chat_with_tools().times(0);
        stub_non_chat_methods(&mut unsupported_nvidia);

        let mut backup = MockLlmProvider::new();
        backup
            .expect_chat_with_tools()
            .times(1)
            .return_once(|_| Ok(final_structured_response()));
        stub_non_chat_methods(&mut backup);

        let settings = AgentSettings {
            agent_model_id: Some("deepseek-ai/deepseek-r1".to_string()),
            agent_model_provider: Some("nvidia".to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm_client = LlmClient::new(&settings);
        llm_client.register_provider("nvidia".to_string(), Arc::new(unsupported_nvidia));
        llm_client.register_provider("opencode-go".to_string(), Arc::new(backup));
        let llm_client = Arc::new(llm_client);

        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
        let mut session = EphemeralSession::new(20_000);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Skip unsupported NVIDIA route"));

        let tools = Vec::new();
        let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
        let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(32);
        let mut ctx = AgentRunnerContext {
            task: "Skip unsupported NVIDIA route",
            system_prompt: "system prompt",
            tools: &tools,
            tool_runtime_registry: None,
            progress_tx: Some(&progress_tx),
            todos_arc: &todos_arc,
            task_id: "runner-nvidia-capability-skip",
            messages: &mut messages,
            agent: &mut session,
            compaction_controller: None,
            session_id: None,
            memory_scope: None,
            memory_behavior: None,
            config: AgentRunnerConfig::new("deepseek-ai/deepseek-r1".to_string(), 1, 1, 30, 256)
                .with_model_provider("nvidia")
                .with_model_routes(vec![
                    ModelInfo {
                        id: "deepseek-ai/deepseek-r1".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 128_000,
                        provider: "nvidia".to_string(),
                        weight: 1,
                    },
                    ModelInfo {
                        id: "deepseek-v4-flash".to_string(),
                        max_output_tokens: 256,
                        context_window_tokens: 128_000,
                        provider: "opencode-go".to_string(),
                        weight: 1,
                    },
                ]),
        };

        let result = runner.run(&mut ctx).await.expect("runner succeeds");
        assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));

        drop(ctx);
        drop(progress_tx);
        let events = collect_progress_events(&mut progress_rx).await;
        assert!(!events
            .iter()
            .any(|event| { matches!(event, AgentEvent::ProviderFailoverActivated { .. }) }));
    }

    fn build_llm_client(provider: MockLlmProvider) -> Arc<LlmClient> {
        build_llm_client_for_provider(provider, "opencode-go", "deepseek-v4-flash")
    }

    fn build_llm_client_for_provider(
        provider: MockLlmProvider,
        provider_name: &str,
        model_name: &str,
    ) -> Arc<LlmClient> {
        let settings = AgentSettings {
            agent_model_id: Some(model_name.to_string()),
            agent_model_provider: Some(provider_name.to_string()),
            agent_model_max_output_tokens: Some(256),
            ..AgentSettings::default()
        };
        let mut llm_client = LlmClient::new(&settings);
        llm_client.register_provider(provider_name.to_string(), Arc::new(provider));
        Arc::new(llm_client)
    }

    fn final_structured_response() -> ChatResponse {
        ChatResponse {
            content: Some(r#"{"thought":"done","final_answer":"done"}"#.to_string()),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            reasoning_content: None,
            usage: None,
        }
    }

    fn stub_non_chat_methods(provider: &mut MockLlmProvider) {
        provider
            .expect_complete_internal_text()
            .returning(|_, _, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_transcribe_audio()
            .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_analyze_image()
            .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
    }

    fn single_final_response_provider() -> MockLlmProvider {
        let mut provider = MockLlmProvider::new();
        provider.expect_chat_with_tools().return_once(|_| {
            Ok(ChatResponse {
                content: Some(r#"{"thought":"done","final_answer":"done"}"#.to_string()),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                reasoning_content: None,
                usage: None,
            })
        });
        provider
            .expect_complete_internal_text()
            .returning(|_, _, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_transcribe_audio()
            .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_analyze_image()
            .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
    }

    fn accidental_structured_final_answer_provider() -> MockLlmProvider {
        let mut provider = MockLlmProvider::new();
        provider.expect_chat_with_tools().return_once(|_| {
            Ok(ChatResponse {
                content: Some(
                    r#"{"thought":"Tool list ready","tool_call":null,"final_answer":"Tools: `read_file`, `write_file`","awaiting_user_input":null}"#
                        .to_string(),
                ),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                reasoning_content: None,
                usage: None,
            })
        });
        stub_non_chat_methods(&mut provider);
        provider
    }

    fn context_overflow_then_summary_then_final_provider() -> MockLlmProvider {
        let mut provider = MockLlmProvider::new();
        let mut sequence = mockall::Sequence::new();
        provider
            .expect_chat_with_tools()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_| {
                Err(LlmError::ApiError(
                    "maximum context length exceeded".to_string(),
                ))
            });
        provider
            .expect_chat_with_tools()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_| {
                Ok(ChatResponse {
                    content: Some(r#"{"thought":"done","final_answer":"done"}"#.to_string()),
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    reasoning_content: None,
                    usage: None,
                })
            });
        provider.expect_complete_internal_text().times(1).returning(
            |_, _, user_message, model_id, _| {
                assert_eq!(model_id, "deepseek-v4-flash");
                assert!(user_message.contains("## Source History"));
                Ok("Runtime context-limit handoff summary.".to_string())
            },
        );
        provider
            .expect_transcribe_audio()
            .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_analyze_image()
            .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
    }

    fn pre_sampling_summary_then_final_provider() -> MockLlmProvider {
        let mut provider = MockLlmProvider::new();
        provider.expect_chat_with_tools().times(1).return_once(|_| {
            Ok(ChatResponse {
                content: Some(r#"{"thought":"done","final_answer":"done"}"#.to_string()),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                reasoning_content: None,
                usage: None,
            })
        });
        provider.expect_complete_internal_text().times(1).returning(
            |_, _, user_message, model_id, _| {
                assert_eq!(model_id, "deepseek-v4-flash");
                assert!(user_message.contains("## Source History"));
                Ok("Pre-sampling handoff summary.".to_string())
            },
        );
        provider
            .expect_transcribe_audio()
            .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
            .expect_analyze_image()
            .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
        provider
    }

    async fn collect_progress_events(
        progress_rx: &mut tokio::sync::mpsc::Receiver<AgentEvent>,
    ) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Some(event) = progress_rx.recv().await {
            events.push(event);
        }
        events
    }
}
