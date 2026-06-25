//! Agent-facing `compress` tool provider.
//!
//! V2 contract: the LLM does **not** choose message refs, ranges, block refs, or
//! nesting. It only records a checkpoint intent and the facts that must be
//! preserved. The runner applies the request through the runtime compaction
//! controller, which owns safe old-middle selection and delegates all invariant
//! enforcement to the compaction engine.

use crate::agent::compaction::{EngineCompactionOutcome, EngineCompactionSkipped};
use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

/// Stable tool name for agent-triggered context compression.
pub const TOOL_COMPRESS: &str = "compress";

const MAX_PRESERVE_CHARS: usize = 6_000;

/// Tool names exposed for agent-triggered context compression.
#[must_use]
pub fn compress_tool_names() -> Vec<String> {
    vec![TOOL_COMPRESS.to_string()]
}

// ── Parsed request/result types ───────────────────────────────────────

/// Why the agent is requesting compression.
///
/// `checkpoint` records preservation guidance and lets the runtime skip when
/// context is still comfortably within budget. `free_context` and
/// `before_long_task` force a compaction attempt because the agent is declaring
/// that it needs room now.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompressReason {
    /// Closed work checkpoint; compact only when the runtime budget says it is useful.
    #[default]
    Checkpoint,
    /// The agent needs context room immediately.
    FreeContext,
    /// The agent is about to start a long next phase and wants room first.
    BeforeLongTask,
}

impl CompressReason {
    /// Stable JSON label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Checkpoint => "checkpoint",
            Self::FreeContext => "free_context",
            Self::BeforeLongTask => "before_long_task",
        }
    }

    /// Whether this reason should bypass the normal hot-context threshold.
    #[must_use]
    pub const fn force_compaction(self) -> bool {
        match self {
            Self::Checkpoint => false,
            Self::FreeContext | Self::BeforeLongTask => true,
        }
    }
}

/// V2 compression request produced by the `compress` parser.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompressRequest {
    /// Why compression was requested.
    pub reason: CompressReason,
    /// Human-authored checkpoint facts that must survive compaction.
    pub preserve: String,
}

/// Result reported back to the LLM as the `compress` tool output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompressResult {
    /// Whether the checkpoint note was accepted into the raw transcript.
    pub checkpoint_recorded: bool,
    /// Whether a compaction block was committed immediately.
    pub compressed: bool,
    /// Request reason, omitted only for internal failures before request parsing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<CompressReason>,
    /// Preservation note echoed into the tool result so skipped checkpoints are
    /// available to later compaction and future external memory ingestion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserve: Option<String>,
    /// New block id when compaction was applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    /// Provider that generated the summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model route that generated the summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// Rendered token count before compaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_before: Option<usize>,
    /// Rendered token count after compaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_after: Option<usize>,
    /// Rendered item count before compaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_items_before: Option<usize>,
    /// Rendered item count after compaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_items_after: Option<usize>,
    /// Human-readable area compressed by v2.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compressed_area: Option<String>,
    /// Human-readable area kept visible by v2.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept: Option<String>,
    /// Structured skip reason when no block was committed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_reason: Option<String>,
    /// Structured error kind when compression failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Human-readable error detail when compression failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
}

impl CompressResult {
    /// Build a result for an applied v2 checkpoint compression.
    #[must_use]
    pub fn applied(request: &CompressRequest, outcome: &EngineCompactionOutcome) -> Self {
        Self {
            checkpoint_recorded: true,
            compressed: true,
            reason: Some(request.reason),
            preserve: Some(request.preserve.clone()),
            block_id: Some(outcome.block_ref.to_string()),
            provider: Some(outcome.provider.clone()),
            route: Some(outcome.route.clone()),
            token_before: Some(outcome.token_before),
            token_after: Some(outcome.token_after),
            history_items_before: Some(outcome.history_items_before),
            history_items_after: Some(outcome.history_items_after),
            compressed_area: Some("old_middle".to_string()),
            kept: Some("pinned_prefix_and_recent_tail".to_string()),
            skipped_reason: None,
            error: None,
            error_detail: None,
        }
    }

    /// Build a result for a recorded checkpoint that did not need immediate compaction.
    #[must_use]
    pub fn skipped(request: &CompressRequest, skipped: &EngineCompactionSkipped) -> Self {
        Self {
            checkpoint_recorded: true,
            compressed: false,
            reason: Some(request.reason),
            preserve: Some(request.preserve.clone()),
            block_id: None,
            provider: None,
            route: None,
            token_before: None,
            token_after: None,
            history_items_before: None,
            history_items_after: None,
            compressed_area: None,
            kept: Some("pinned_prefix_and_recent_tail".to_string()),
            skipped_reason: Some(skipped.skipped_reason.clone()),
            error: None,
            error_detail: None,
        }
    }

    /// Build a result for a parsed request that could not be applied.
    #[must_use]
    pub fn failed(
        request: &CompressRequest,
        error: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            checkpoint_recorded: true,
            compressed: false,
            reason: Some(request.reason),
            preserve: Some(request.preserve.clone()),
            block_id: None,
            provider: None,
            route: None,
            token_before: None,
            token_after: None,
            history_items_before: None,
            history_items_after: None,
            compressed_area: None,
            kept: None,
            skipped_reason: None,
            error: Some(error.into()),
            error_detail: Some(detail.into()),
        }
    }

    /// Build a result for an internal failure before a typed request exists.
    #[must_use]
    pub fn internal_error(detail: impl Into<String>) -> Self {
        Self {
            checkpoint_recorded: false,
            compressed: false,
            reason: None,
            preserve: None,
            block_id: None,
            provider: None,
            route: None,
            token_before: None,
            token_after: None,
            history_items_before: None,
            history_items_after: None,
            compressed_area: None,
            kept: None,
            skipped_reason: None,
            error: Some("internal_error".to_string()),
            error_detail: Some(detail.into()),
        }
    }

    /// Serialize to JSON string for tool output.
    ///
    /// # Errors
    /// Returns a serialization error if the result cannot be encoded.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

// ── Provider ──────────────────────────────────────────────────────────

/// Minimal provider that only advertises the `compress` tool.
pub struct CompressionProvider;

impl CompressionProvider {
    /// Create a new compression tool provider.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn tools_definitions() -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: TOOL_COMPRESS.to_string(),
            description: compress_tool_description().to_string(),
            parameters: compress_tool_schema(),
        }]
    }

    /// Build native typed runtime executors for structured context compression.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        Self::tools_definitions()
            .into_iter()
            .map(|spec| {
                Arc::new(CompressionToolExecutor {
                    name: ToolName::from(spec.name.clone()),
                    spec,
                }) as Arc<dyn ToolExecutor>
            })
            .collect()
    }
}

impl Default for CompressionProvider {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tool description and schema ───────────────────────────────────────

/// Human-readable description for the compress tool.
fn compress_tool_description() -> &'static str {
    "Record a checkpoint and optionally compact old conversation history. Provide only a human \n\
     preservation note; do not choose message ids, ranges, block refs, or tool-call boundaries. \n\
     Oxide selects the safe old-middle range internally, keeps pinned task/instruction context and \n\
     the recent working tail visible, includes full tool-call/result batches, consumes existing \n\
     compression blocks safely, and preserves the raw transcript.\n\n\
     Use reason=checkpoint after a closed phase; it records the note and compacts only when the \n\
     context is hot. Use reason=free_context or before_long_task when the agent needs room now."
}

/// JSON schema for the compress tool arguments.
fn compress_tool_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "reason": {
                "type": "string",
                "enum": ["checkpoint", "free_context", "before_long_task"],
                "description": "Why compression is requested. Defaults to checkpoint."
            },
            "preserve": {
                "type": "string",
                "description": "Facts, decisions, files, commands, blockers, and next steps that must survive in the handoff summary.",
                "minLength": 1,
                "maxLength": MAX_PRESERVE_CHARS
            }
        },
        "required": ["preserve"],
        "additionalProperties": false
    })
}

// ── Argument parser ───────────────────────────────────────────────────

/// Parse the compress tool arguments into a typed v2 checkpoint request.
///
/// This is a pure function — no memory access, no side effects. It deliberately
/// rejects the v1 `ranges`/`messages` contract so LLMs cannot keep using manual
/// boundary selection.
fn parse_compress_arguments(arguments: &str) -> Result<CompressRequest> {
    if arguments.trim().is_empty() {
        return Err(anyhow!("compress v2 requires a `preserve` string"));
    }

    let value: Value = serde_json::from_str(arguments)?;
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("compress arguments must be a JSON object"))?;

    for key in obj.keys() {
        if key != "reason" && key != "preserve" {
            return Err(anyhow!(
                "unknown compress v2 field `{key}`; use only `reason` and `preserve`"
            ));
        }
    }

    let preserve = obj
        .get("preserve")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("compress v2 requires `preserve` as a string"))?
        .trim()
        .to_string();
    if preserve.is_empty() {
        return Err(anyhow!("compress v2 `preserve` must not be empty"));
    }
    let preserve_chars = preserve.chars().count();
    if preserve_chars > MAX_PRESERVE_CHARS {
        return Err(anyhow!(
            "compress v2 `preserve` is too long ({preserve_chars} chars, max {MAX_PRESERVE_CHARS})"
        ));
    }

    let reason = match obj.get("reason") {
        Some(value) => parse_reason(
            value
                .as_str()
                .ok_or_else(|| anyhow!("compress v2 `reason` must be a string"))?,
        )?,
        None => CompressReason::default(),
    };

    Ok(CompressRequest { reason, preserve })
}

fn parse_reason(value: &str) -> Result<CompressReason> {
    match value {
        "checkpoint" => Ok(CompressReason::Checkpoint),
        "free_context" => Ok(CompressReason::FreeContext),
        "before_long_task" => Ok(CompressReason::BeforeLongTask),
        other => Err(anyhow!(
            "unsupported compress v2 reason `{other}`; expected checkpoint, free_context, or before_long_task"
        )),
    }
}

// ── Tool executor ─────────────────────────────────────────────────────

struct CompressionToolExecutor {
    name: ToolName,
    spec: ToolDefinition,
}

#[async_trait]
impl ToolExecutor for CompressionToolExecutor {
    fn name(&self) -> ToolName {
        self.name.clone()
    }

    fn spec(&self) -> ToolDefinition {
        self.spec.clone()
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            artifact_dir: invocation.execution_context.artifact_dir.clone(),
            ..ToolRuntimeConfig::default()
        });

        let request = parse_compress_arguments(&invocation.raw_arguments)
            .map_err(compression_runtime_error)?;
        let payload = serde_json::to_value(&request)
            .map_err(|error| ToolRuntimeError::Internal(error.to_string()))?;

        let stdout = format!(
            "Compression checkpoint parsed: reason={}, preserve_chars={}",
            request.reason.as_str(),
            request.preserve.chars().count()
        );
        let mut output = normalizer.success(&invocation, &stdout, "");
        output.structured_payload = Some(payload);
        Ok(output)
    }
}

/// Map parsing errors to typed runtime errors.
fn compression_runtime_error(error: anyhow::Error) -> ToolRuntimeError {
    let message = error.to_string();
    if error.downcast_ref::<serde_json::Error>().is_some()
        || message.contains("requires")
        || message.contains("must")
        || message.contains("unknown compress v2 field")
        || message.contains("unsupported compress v2 reason")
        || message.contains("too long")
    {
        ToolRuntimeError::InvalidArguments(message)
    } else {
        ToolRuntimeError::Failure(message)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::{
        CompactionPhase, CompactionReason, EngineCompactionOutcome, EngineCompactionSkipped,
    };
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolInvocation, ToolOutputStatus, ToolRuntimeError, ToolTimeoutConfig, TurnId,
    };
    use crate::llm::InvocationId;
    use chrono::Utc;
    use tokio_util::sync::CancellationToken;

    fn runtime_invocation(tool_name: &str, raw_arguments: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(42_i64),
            turn_id: TurnId::from("turn-1"),
            batch_id: ToolBatchId::from("batch-1"),
            batch_index: 0,
            invocation_id: InvocationId::new("invocation-1"),
            tool_call_id: ToolCallId::from("tool-call-1"),
            provider_tool_call_id: None,
            tool_name: ToolName::from(tool_name),
            raw_provider_payload: json!({}),
            raw_arguments: raw_arguments.to_string(),
            normalized_arguments: serde_json::from_str(raw_arguments).unwrap_or(Value::Null),
            cancellation_token: CancellationToken::new(),
            timeout: ToolTimeoutConfig::default(),
            execution_context: ToolExecutionContext::new(std::env::temp_dir()),
            provider_metadata: ProviderMetadata {
                provider: "mock".to_string(),
                protocol: "chat".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "mock-model".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: now,
            started_at: Some(now),
        }
    }

    #[test]
    fn compress_tool_spec_is_v2_checkpoint_contract() {
        let provider = Arc::new(CompressionProvider::new());
        let executors = provider.tool_runtime_executors();
        let spec = executors[0].spec();

        assert_eq!(spec.name, TOOL_COMPRESS);
        assert!(spec.description.contains("do not choose message ids"));
        assert!(spec.parameters["properties"].get("preserve").is_some());
        assert!(spec.parameters["properties"].get("reason").is_some());
        assert!(spec.parameters["properties"].get("ranges").is_none());
        assert!(spec.parameters["properties"].get("messages").is_none());
        assert_eq!(spec.parameters["additionalProperties"], false);
    }

    #[test]
    fn tool_name_list_contains_compress() {
        assert_eq!(compress_tool_names(), vec![TOOL_COMPRESS.to_string()]);
    }

    #[test]
    fn typed_runtime_executors_register_compress_tool() {
        let provider = Arc::new(CompressionProvider::new());
        let executors = provider.tool_runtime_executors();

        assert_eq!(executors.len(), 1);
        assert_eq!(executors[0].name().as_str(), TOOL_COMPRESS);
    }

    #[test]
    fn parse_checkpoint_defaults_reason() {
        let request = parse_compress_arguments(
            r#"{"preserve":"Closed RECON: receiver owns compression range selection."}"#,
        )
        .expect("valid v2 checkpoint parses");

        assert_eq!(request.reason, CompressReason::Checkpoint);
        assert_eq!(
            request.preserve,
            "Closed RECON: receiver owns compression range selection."
        );
    }

    #[test]
    fn parse_free_context_reason() {
        let request = parse_compress_arguments(
            r#"{"reason":"free_context","preserve":"Need room before implementation."}"#,
        )
        .expect("valid free_context parses");

        assert_eq!(request.reason, CompressReason::FreeContext);
        assert!(request.reason.force_compaction());
    }

    #[test]
    fn parse_before_long_task_reason() {
        let request = parse_compress_arguments(
            r#"{"reason":"before_long_task","preserve":"Starting a long validation phase."}"#,
        )
        .expect("valid before_long_task parses");

        assert_eq!(request.reason, CompressReason::BeforeLongTask);
        assert!(request.reason.force_compaction());
    }

    #[test]
    fn parse_rejects_empty_arguments() {
        let result = parse_compress_arguments("");
        assert!(result.is_err());
    }

    #[test]
    fn parse_rejects_missing_preserve() {
        let result = parse_compress_arguments(r#"{"reason":"checkpoint"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn parse_rejects_empty_preserve() {
        let result = parse_compress_arguments(r#"{"preserve":"   "}"#);
        assert!(result.is_err());
    }

    #[test]
    fn parse_rejects_unknown_fields_including_v1_ranges() {
        let result = parse_compress_arguments(
            r#"{"ranges":[{"start":"m0001","end":"m0003","summary":[{"text":"old"}]}]}"#,
        );

        assert!(result.is_err());
        assert!(
            result
                .expect_err("v1 ranges rejected")
                .to_string()
                .contains("unknown compress v2 field `ranges`")
        );
    }

    #[test]
    fn parse_rejects_invalid_reason() {
        let result =
            parse_compress_arguments(r#"{"reason":"manual_range","preserve":"Keep facts."}"#);

        assert!(result.is_err());
    }

    #[test]
    fn parse_rejects_too_large_preserve_note() {
        let too_large = "x".repeat(MAX_PRESERVE_CHARS + 1);
        let args = json!({ "preserve": too_large }).to_string();
        let result = parse_compress_arguments(&args);

        assert!(result.is_err());
    }

    #[test]
    fn compress_request_serde_round_trip() {
        let request = CompressRequest {
            reason: CompressReason::BeforeLongTask,
            preserve: "Next step: run full validation.".to_string(),
        };

        let json = serde_json::to_string(&request).expect("serialize");
        let restored: CompressRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, request);
    }

    fn request() -> CompressRequest {
        CompressRequest {
            reason: CompressReason::FreeContext,
            preserve: "Preserve the architecture decision.".to_string(),
        }
    }

    fn applied_outcome() -> EngineCompactionOutcome {
        EngineCompactionOutcome {
            block_ref: crate::agent::compaction::BlockRef::new(3),
            summary_text: "summary".to_string(),
            provider: "mock".to_string(),
            route: "mock-route".to_string(),
            reason: CompactionReason::Manual,
            phase: CompactionPhase::MidTurn,
            token_before: 10_000,
            token_after: 4_000,
            history_items_before: 80,
            history_items_after: 20,
        }
    }

    #[test]
    fn compress_result_applied_contains_checkpoint_ledger_fields() {
        let result = CompressResult::applied(&request(), &applied_outcome());
        let json = result.to_json().expect("json");

        assert!(json.contains(r#""checkpoint_recorded": true"#));
        assert!(json.contains(r#""compressed": true"#));
        assert!(json.contains(r#""block_id": "b3""#));
        assert!(json.contains("Preserve the architecture decision."));
        assert!(json.contains("pinned_prefix_and_recent_tail"));
    }

    #[test]
    fn compress_result_skipped_keeps_preserve_note() {
        let skipped = EngineCompactionSkipped {
            reason: CompactionReason::Manual,
            phase: CompactionPhase::MidTurn,
            skipped_reason: "Context is within budget threshold".to_string(),
        };
        let result = CompressResult::skipped(&request(), &skipped);

        assert!(!result.compressed);
        assert_eq!(
            result.preserve.as_deref(),
            Some("Preserve the architecture decision.")
        );
        assert_eq!(
            result.skipped_reason.as_deref(),
            Some("Context is within budget threshold")
        );
    }

    #[tokio::test]
    async fn typed_runtime_executor_parses_valid_checkpoint_request() {
        let provider = Arc::new(CompressionProvider::new());
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("compress typed executor registered");

        let args = r#"{
            "reason": "free_context",
            "preserve": "Summary of completed phase and next action."
        }"#;

        let output = executor
            .execute(runtime_invocation(TOOL_COMPRESS, args))
            .await
            .expect("compress parse succeeds");

        assert_eq!(output.status, ToolOutputStatus::Success);
        let payload = output
            .structured_payload
            .as_ref()
            .expect("structured_payload must be set");
        let request: CompressRequest = serde_json::from_value(payload.clone())
            .expect("payload deserializes to CompressRequest");
        assert_eq!(request.reason, CompressReason::FreeContext);
        assert_eq!(
            request.preserve,
            "Summary of completed phase and next action."
        );
    }

    #[tokio::test]
    async fn typed_runtime_executor_rejects_v1_arguments() {
        let provider = Arc::new(CompressionProvider::new());
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("compress typed executor registered");

        let error = executor
            .execute(runtime_invocation(
                TOOL_COMPRESS,
                r#"{"ranges":[{"start":"m0001","end":"m0002","summary":[{"text":"old"}]}]}"#,
            ))
            .await
            .expect_err("v1 args must be rejected");

        assert!(matches!(error, ToolRuntimeError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn typed_runtime_executor_rejects_empty_arguments() {
        let provider = Arc::new(CompressionProvider::new());
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("compress typed executor registered");

        let error = executor
            .execute(runtime_invocation(TOOL_COMPRESS, ""))
            .await
            .expect_err("empty args must be rejected");

        assert!(matches!(error, ToolRuntimeError::InvalidArguments(_)));
    }
}
