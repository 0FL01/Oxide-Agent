//! Async parallel batch scheduler for typed tool calls.

use super::config::{ToolRuntimeConfig, ToolTimeoutConfig};
use super::history::{ToolHistoryError, ToolHistoryWriter};
use super::invocation::{
    EnvironmentMetadata, ModelMetadata, ProviderMetadata, ToolExecutionContext, ToolInvocation,
};
use super::normalizer::OutputNormalizer;
use super::output::{CancellationReason, CleanupStatus, ToolOutput};
use super::provider_opencode_go::{OpenCodeGoParsedToolCall, OpenCodeGoToolCallBatch};
use super::registry::ToolRegistry;
use super::types::ToolBatchId;
use crate::agent::identity::SessionId;
use chrono::Utc;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// Fatal runtime error. These stop the turn before the next provider request.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ToolRuntimeFatal {
    /// History write failed.
    #[error(transparent)]
    History(#[from] ToolHistoryError),
    /// Batch output invariant failed.
    #[error("tool batch invariant failed: {0}")]
    Invariant(String),
}

/// Context copied into all invocations of one assistant tool-call batch.
#[derive(Debug, Clone)]
pub struct ToolTurnContext {
    /// Transport/session id.
    pub session_id: SessionId,
    /// Tool batch id.
    pub batch_id: ToolBatchId,
    /// Turn-level cancellation token.
    pub cancellation_token: CancellationToken,
    /// Timeout and hung-detection config for each invocation.
    pub timeout: ToolTimeoutConfig,
    /// Execution context inherited by every call.
    pub execution_context: ToolExecutionContext,
    /// Provider metadata.
    pub provider_metadata: ProviderMetadata,
    /// Model metadata.
    pub model_metadata: ModelMetadata,
    /// Optional environment metadata.
    pub environment_metadata: Option<EnvironmentMetadata>,
}

impl ToolTurnContext {
    /// Build a context from runtime config defaults.
    #[must_use]
    pub fn new(
        session_id: SessionId,
        batch_id: ToolBatchId,
        config: &ToolRuntimeConfig,
        provider_metadata: ProviderMetadata,
        model_metadata: ModelMetadata,
    ) -> Self {
        Self {
            session_id,
            batch_id,
            cancellation_token: CancellationToken::new(),
            timeout: config.timeout.clone(),
            execution_context: ToolExecutionContext::new(config.artifact_dir.clone()),
            provider_metadata,
            model_metadata,
            environment_metadata: None,
        }
    }
}

/// Async parallel tool-call runtime.
pub struct ToolCallRuntime {
    registry: Arc<ToolRegistry>,
    history: Arc<dyn ToolHistoryWriter>,
    normalizer: OutputNormalizer,
}

impl ToolCallRuntime {
    /// Create a runtime with deterministic registry and history writer.
    #[must_use]
    pub fn new(
        registry: Arc<ToolRegistry>,
        history: Arc<dyn ToolHistoryWriter>,
        config: ToolRuntimeConfig,
    ) -> Self {
        Self {
            registry,
            history,
            normalizer: OutputNormalizer::new(config),
        }
    }

    /// Execute one assistant tool-call batch with barrier semantics.
    ///
    /// # Errors
    ///
    /// Returns a fatal error when history cannot be written or output pairing
    /// invariants fail.
    pub async fn execute_batch(
        &self,
        batch: OpenCodeGoToolCallBatch,
        context: ToolTurnContext,
    ) -> Result<Vec<ToolOutput>, ToolRuntimeFatal> {
        self.history.record_assistant_tool_calls(&batch).await?;

        let mut handles = Vec::with_capacity(batch.calls.len());
        for call in batch.calls.iter().cloned() {
            let invocation = build_invocation(&batch, &context, &call);
            handles.push(ToolTaskHandle {
                invocation: invocation.clone(),
                handle: tokio::spawn(run_one_tool(
                    Arc::clone(&self.registry),
                    self.normalizer.clone(),
                    invocation,
                    call,
                )),
            });
        }

        let mut outputs = Vec::with_capacity(handles.len());
        for task in handles {
            let output = match task.handle.await {
                Ok(output) => output,
                Err(error) => self.normalizer.internal_runtime_error(
                    &task.invocation,
                    format!("tool task join failed: {error}"),
                ),
            };
            outputs.push(output);
        }

        outputs.sort_by_key(|output| output.batch_index);
        verify_outputs_match_calls(&batch, &outputs)?;

        for output in &outputs {
            self.history.record_tool_output(output).await?;
        }

        Ok(outputs)
    }
}

struct ToolTaskHandle {
    invocation: ToolInvocation,
    handle: tokio::task::JoinHandle<ToolOutput>,
}

async fn run_one_tool(
    registry: Arc<ToolRegistry>,
    normalizer: OutputNormalizer,
    mut invocation: ToolInvocation,
    call: OpenCodeGoParsedToolCall,
) -> ToolOutput {
    if let Some(issue) = call.protocol_issue {
        return normalizer.provider_protocol_error(&invocation, issue.message());
    }

    match parse_normalized_arguments(&invocation.raw_arguments) {
        Ok(arguments) => {
            invocation.normalized_arguments = arguments;
        }
        Err(message) => {
            return normalizer.invalid_arguments(&invocation, message);
        }
    }

    let timeout = invocation.timeout.per_tool_hard_timeout;
    tokio::select! {
        () = invocation.cancellation_token.cancelled() => {
            normalizer.cancelled(
                &invocation,
                CancellationReason::User,
                CleanupStatus::NotStarted,
            )
        }
        result = tokio::time::timeout(timeout, registry.execute_or_normalize(
            invocation.clone(),
            &normalizer,
        )) => {
            match result {
                Ok(output) => output,
                Err(_) => {
                    invocation.cancellation_token.cancel();
                    normalizer.timeout(&invocation, CleanupStatus::NotStarted)
                }
            }
        }
    }
}

fn build_invocation(
    batch: &OpenCodeGoToolCallBatch,
    context: &ToolTurnContext,
    call: &OpenCodeGoParsedToolCall,
) -> ToolInvocation {
    let now = Utc::now();
    ToolInvocation {
        session_id: context.session_id,
        turn_id: batch.turn_id.clone(),
        batch_id: context.batch_id.clone(),
        batch_index: call.batch_index,
        invocation_id: call.invocation_id.clone(),
        tool_call_id: call.tool_call_id.clone(),
        provider_tool_call_id: call
            .original_provider_tool_call_id
            .as_deref()
            .map(Into::into),
        tool_name: call.tool_name.clone(),
        raw_provider_payload: call.raw_provider_payload.clone(),
        raw_arguments: call.raw_arguments.clone(),
        normalized_arguments: Value::Null,
        cancellation_token: context.cancellation_token.child_token(),
        timeout: context.timeout.clone(),
        execution_context: context.execution_context.clone(),
        provider_metadata: context.provider_metadata.clone(),
        model_metadata: context.model_metadata.clone(),
        working_directory: context.execution_context.cwd.clone(),
        environment_metadata: context.environment_metadata.clone(),
        created_at: now,
        started_at: Some(now),
    }
}

fn parse_normalized_arguments(arguments: &str) -> Result<Value, String> {
    let parsed: Value = serde_json::from_str(arguments)
        .map_err(|error| format!("invalid JSON tool arguments: {error}"))?;
    if !parsed.is_object() {
        return Err("tool arguments must be a JSON object".to_string());
    }
    Ok(parsed)
}

fn verify_outputs_match_calls(
    batch: &OpenCodeGoToolCallBatch,
    outputs: &[ToolOutput],
) -> Result<(), ToolRuntimeFatal> {
    if outputs.len() != batch.calls.len() {
        return Err(ToolRuntimeFatal::Invariant(format!(
            "expected {} outputs, got {}",
            batch.calls.len(),
            outputs.len()
        )));
    }

    let mut seen = BTreeSet::new();
    for (call, output) in batch.calls.iter().zip(outputs) {
        if call.batch_index != output.batch_index {
            return Err(ToolRuntimeFatal::Invariant(format!(
                "batch index mismatch for {}",
                call.tool_call_id
            )));
        }
        if call.tool_call_id != output.tool_call_id {
            return Err(ToolRuntimeFatal::Invariant(format!(
                "tool_call_id mismatch: expected {}, got {}",
                call.tool_call_id, output.tool_call_id
            )));
        }
        if !seen.insert(output.tool_call_id.clone()) {
            return Err(ToolRuntimeFatal::Invariant(format!(
                "duplicate output for {}",
                output.tool_call_id
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tool_runtime::executor::ToolExecutor;
    use crate::agent::tool_runtime::normalizer::ToolRuntimeError;
    use crate::agent::tool_runtime::output::ToolOutputStatus;
    use crate::agent::tool_runtime::provider_opencode_go::OpenCodeGoToolCallParser;
    use crate::agent::tool_runtime::registry::ToolRegistry;
    use crate::agent::tool_runtime::types::{ToolName, TurnId};
    use crate::llm::ToolDefinition;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct RecordingHistory {
        events: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ToolHistoryWriter for RecordingHistory {
        async fn record_assistant_tool_calls(
            &self,
            batch: &OpenCodeGoToolCallBatch,
        ) -> Result<(), ToolHistoryError> {
            self.events
                .lock()
                .expect("events lock")
                .push(format!("assistant:{}", batch.calls.len()));
            Ok(())
        }

        async fn record_tool_output(&self, output: &ToolOutput) -> Result<(), ToolHistoryError> {
            self.events
                .lock()
                .expect("events lock")
                .push(format!("tool:{}", output.tool_call_id));
            Ok(())
        }
    }

    struct DelayedExecutor {
        name: ToolName,
        delay: Duration,
        panic: bool,
    }

    #[async_trait]
    impl ToolExecutor for DelayedExecutor {
        fn name(&self) -> ToolName {
            self.name.clone()
        }

        fn spec(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.as_str().to_string(),
                description: "delayed test tool".to_string(),
                parameters: json!({ "type": "object" }),
            }
        }

        async fn execute(
            &self,
            invocation: ToolInvocation,
        ) -> Result<ToolOutput, ToolRuntimeError> {
            assert!(invocation.normalized_arguments.is_object());
            tokio::time::sleep(self.delay).await;
            assert!(!self.panic, "executor panic requested");
            Ok(OutputNormalizer::new(ToolRuntimeConfig::default()).success(
                &invocation,
                self.name.as_str(),
                "",
            ))
        }
    }

    #[tokio::test]
    async fn batch_runs_tools_in_parallel_and_writes_outputs_in_order() {
        let runtime = runtime_with_executor("fast", Duration::from_millis(80), false);
        let batch = parse_batch(json!([
            call("call_1", "fast"),
            call("call_2", "fast"),
            call("call_3", "fast")
        ]));
        let started = Instant::now();

        let outputs = runtime
            .execute_batch(batch, turn_context(Duration::from_secs(5)))
            .await
            .expect("batch succeeds");

        assert!(started.elapsed() < Duration::from_millis(190));
        assert_eq!(
            outputs
                .iter()
                .map(|output| output.tool_call_id.as_str())
                .collect::<Vec<_>>(),
            vec!["call_1", "call_2", "call_3"]
        );
    }

    #[tokio::test]
    async fn timeout_returns_paired_output_without_waiting_for_executor() {
        let runtime = runtime_with_executor("slow", Duration::from_secs(5), false);
        let batch = parse_batch(json!([call("call_timeout", "slow")]));

        let output = runtime
            .execute_batch(batch, turn_context(Duration::from_millis(30)))
            .await
            .expect("batch completes")
            .remove(0);

        assert_eq!(output.status, ToolOutputStatus::Timeout);
        assert_eq!(output.tool_call_id.as_str(), "call_timeout");
    }

    #[tokio::test]
    async fn cancellation_returns_cancelled_output() {
        let runtime = runtime_with_executor("slow", Duration::from_secs(5), false);
        let context = turn_context(Duration::from_secs(5));
        let cancel = context.cancellation_token.clone();
        let batch = parse_batch(json!([call("call_cancelled", "slow")]));
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel.cancel();
        });

        let output = runtime
            .execute_batch(batch, context)
            .await
            .expect("batch completes")
            .remove(0);

        assert_eq!(output.status, ToolOutputStatus::Cancelled);
        assert_eq!(output.tool_call_id.as_str(), "call_cancelled");
    }

    #[tokio::test]
    async fn invalid_arguments_are_normalized_before_executor_dispatch() {
        let runtime = runtime_with_executor("fast", Duration::from_millis(1), false);
        let batch = parse_batch(json!([
            {
                "id": "call_bad_json",
                "type": "function",
                "function": { "name": "fast", "arguments": "not json" }
            }
        ]));

        let output = runtime
            .execute_batch(batch, turn_context(Duration::from_secs(5)))
            .await
            .expect("batch completes")
            .remove(0);

        assert_eq!(output.status, ToolOutputStatus::InvalidArguments);
        assert_eq!(output.tool_call_id.as_str(), "call_bad_json");
    }

    #[tokio::test]
    async fn provider_protocol_issue_is_output_without_dispatch() {
        let runtime = runtime_with_executor("fast", Duration::from_millis(1), false);
        let batch = parse_batch(json!([
            {
                "type": "function",
                "function": { "name": "fast", "arguments": "{}" }
            }
        ]));

        let output = runtime
            .execute_batch(batch, turn_context(Duration::from_secs(5)))
            .await
            .expect("batch completes")
            .remove(0);

        assert_eq!(output.status, ToolOutputStatus::ProviderProtocolError);
        assert_eq!(
            output.tool_call_id.as_str(),
            "oxide_missing_tool_call_id_turn_runtime_0"
        );
    }

    #[tokio::test]
    async fn executor_panic_is_join_error_output() {
        let runtime = runtime_with_executor("panic_tool", Duration::from_millis(1), true);
        let batch = parse_batch(json!([call("call_panic", "panic_tool")]));

        let output = runtime
            .execute_batch(batch, turn_context(Duration::from_secs(5)))
            .await
            .expect("batch completes")
            .remove(0);

        assert_eq!(output.status, ToolOutputStatus::InternalRuntimeError);
        assert_eq!(output.tool_call_id.as_str(), "call_panic");
    }

    #[tokio::test]
    async fn history_records_assistant_before_ordered_tool_outputs() {
        let history = Arc::new(RecordingHistory::default());
        let runtime = runtime_with_history(
            Arc::clone(&history),
            "fast",
            Duration::from_millis(1),
            false,
        );
        let batch = parse_batch(json!([call("call_a", "fast"), call("call_b", "fast")]));

        runtime
            .execute_batch(batch, turn_context(Duration::from_secs(5)))
            .await
            .expect("batch succeeds");

        let events = history.events.lock().expect("events lock").clone();
        assert_eq!(events, vec!["assistant:2", "tool:call_a", "tool:call_b"]);
    }

    fn runtime_with_executor(name: &str, delay: Duration, panic: bool) -> ToolCallRuntime {
        runtime_with_history(Arc::new(RecordingHistory::default()), name, delay, panic)
    }

    fn runtime_with_history(
        history: Arc<RecordingHistory>,
        name: &str,
        delay: Duration,
        panic: bool,
    ) -> ToolCallRuntime {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(DelayedExecutor {
                name: ToolName::from(name),
                delay,
                panic,
            }))
            .expect("executor registers");
        ToolCallRuntime::new(Arc::new(registry), history, ToolRuntimeConfig::default())
    }

    fn turn_context(timeout: Duration) -> ToolTurnContext {
        let config = ToolRuntimeConfig::default();
        let mut context = ToolTurnContext::new(
            SessionId::from(42),
            ToolBatchId::from("batch_runtime"),
            &config,
            ProviderMetadata {
                provider: "opencode-go".to_string(),
                protocol: "chat_like".to_string(),
            },
            ModelMetadata {
                model: "deepseek-v4-flash".to_string(),
            },
        );
        context.timeout.per_tool_hard_timeout = timeout;
        context
    }

    fn parse_batch(value: Value) -> OpenCodeGoToolCallBatch {
        OpenCodeGoToolCallParser
            .parse_batch(TurnId::from("turn_runtime"), &value)
            .expect("batch parses")
    }

    fn call(id: &str, name: &str) -> Value {
        json!({
            "id": id,
            "type": "function",
            "function": {
                "name": name,
                "arguments": "{}"
            }
        })
    }
}
