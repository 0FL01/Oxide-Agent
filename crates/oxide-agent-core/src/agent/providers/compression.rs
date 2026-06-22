//! Agent-facing `compress` tool provider.
//!
//! The tool accepts structured range/message compression requests from the
//! LLM. The executor is a **pure parser** — it validates argument syntax
//! (ref format, summary part shape) and returns a typed `CompressRequest` as
//! `structured_payload`. The runner intercepts the compress tool output,
//! extracts the request, and applies it through `CompactionEngine` — the
//! sole mutation authority for `CompactionState`.
//!
//! This contract ensures the LLM never needs to know internal indices, block
//! state, or memory layout. It references only renderer-injected visible refs
//! (`mNNNN`, `bN`).

use crate::agent::compaction::{CompressionSelection, SummaryPart};
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

use crate::agent::compaction::refs::{BlockRef, MessageRef};

/// Stable tool name for agent-triggered context compression.
pub const TOOL_COMPRESS: &str = "compress";

/// Tool names exposed for agent-triggered context compression.
#[must_use]
pub fn compress_tool_names() -> Vec<String> {
    vec![TOOL_COMPRESS.to_string()]
}

// ── Parsed request types ──────────────────────────────────────────────

/// One compression entry — a selection plus a structured summary.
///
/// Produced by parsing the LLM's tool arguments. Consumed by the runner
/// which calls `CompactionEngine::apply_compression` for each entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompressEntry {
    /// Which messages to compress (range or individual refs).
    pub selection: CompressionSelection,
    /// Structured summary parts (text and/or block refs for nesting).
    pub summary: Vec<SummaryPart>,
}

/// All compression entries from one `compress` tool call.
///
/// Serialized into `ToolOutput::structured_payload` by the executor,
/// deserialized by the runner to apply through the engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CompressRequest {
    /// One or more compression operations to apply.
    pub entries: Vec<CompressEntry>,
}

/// Result of one compression entry, reported back to the LLM.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompressEntryResult {
    /// Whether this entry was successfully compressed.
    pub compressed: bool,
    /// New block id if successful (e.g. `"b3"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    /// Structured error if compression failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Human-readable error detail if compression failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
}

/// Aggregate result reported to the LLM as the tool output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompressResult {
    /// Whether all entries succeeded.
    pub compressed: bool,
    /// Per-entry results.
    pub entries: Vec<CompressEntryResult>,
}

impl CompressResult {
    /// Build a result from engine outcomes.
    #[must_use]
    pub fn from_outcomes(outcomes: Vec<Result<BlockRef, String>>) -> Self {
        let entries: Vec<CompressEntryResult> = outcomes
            .into_iter()
            .map(|outcome| match outcome {
                Ok(block_ref) => CompressEntryResult {
                    compressed: true,
                    block_id: Some(block_ref.to_string()),
                    error: None,
                    error_detail: None,
                },
                Err(message) => CompressEntryResult {
                    compressed: false,
                    block_id: None,
                    error: Some(extract_error_kind(&message)),
                    error_detail: Some(message),
                },
            })
            .collect();
        let all_compressed = entries.iter().all(|e| e.compressed);
        Self {
            compressed: all_compressed,
            entries,
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

/// Extract a short error kind from a full error message.
fn extract_error_kind(message: &str) -> String {
    if message.contains("stale or out of range") {
        "invalid_message_ref".to_string()
    } else if message.contains("must be <=") {
        "invalid_range".to_string()
    } else if message.contains("selection is empty") {
        "empty_selection".to_string()
    } else if message.contains("overlaps with active block") {
        "overlaps_active_block".to_string()
    } else if message.contains("splits tool-call/result pair") {
        "splits_tool_batch".to_string()
    } else if message.contains("not a consumed block") {
        "invalid_block_ref".to_string()
    } else if message.contains("more than once") {
        "duplicate_block_ref".to_string()
    } else {
        "compression_error".to_string()
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
    "Compress conversation history into a structured summary. Each message in the conversation \
     is labeled with a stable ref like <m0001>. Use these refs to select which messages to \
     compress.\n\n\
     Provide one or more compression entries:\n\
     - Ranges: compress a contiguous span (start..end) into one summary.\n\
     - Messages: compress specific non-contiguous messages into one summary.\n\n\
     Each summary is a list of parts: text segments and/or block refs (bN) that reference \
     prior compressed blocks whose summaries should be nested inside the new summary. Only \
     reference block refs for blocks that fall within your selected range — these are \
     consumed by the new block.\n\n\
     The tool rejects ranges that split a tool-call/result pair (always include the full \
     pair), ranges that overlap with existing active blocks (consume them instead by \
     including their range), and block refs that don't match consumed blocks.\n\n\
     After compression, the covered messages are replaced by the summary in future model \
     context. The raw transcript is preserved internally. Compression is irreversible for \
     the current session — do not compress preemptively."
}

/// JSON schema for the compress tool arguments.
fn compress_tool_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "ranges": {
                "type": "array",
                "description": "Contiguous message ranges to compress. Each range becomes one compression block.",
                "items": {
                    "type": "object",
                    "properties": {
                        "start": {
                            "type": "string",
                            "description": "First message ref in the range (e.g. \"m0003\")."
                        },
                        "end": {
                            "type": "string",
                            "description": "Last message ref in the range (e.g. \"m0008\")."
                        },
                        "summary": {
                            "type": "array",
                            "description": "Structured summary parts for this range.",
                            "items": summary_part_schema(),
                            "minItems": 1
                        }
                    },
                    "required": ["start", "end", "summary"],
                    "additionalProperties": false
                }
            },
            "messages": {
                "type": "array",
                "description": "Individual or non-contiguous message sets to compress. Each entry becomes one compression block.",
                "items": {
                    "type": "object",
                    "properties": {
                        "refs": {
                            "type": "array",
                            "description": "Message refs to compress together into one block.",
                            "items": {
                                "type": "string",
                                "description": "Message ref (e.g. \"m0005\")."
                            },
                            "minItems": 1
                        },
                        "summary": {
                            "type": "array",
                            "description": "Structured summary parts for these messages.",
                            "items": summary_part_schema(),
                            "minItems": 1
                        }
                    },
                    "required": ["refs", "summary"],
                    "additionalProperties": false
                }
            }
        },
        "additionalProperties": false
    })
}

/// Schema for one summary part (text or block_ref, exactly one).
fn summary_part_schema() -> Value {
    json!({
        "type": "object",
        "description": "One part of a structured summary. Set exactly one of `text` or `block_ref`.",
        "properties": {
            "text": {
                "type": "string",
                "description": "Plain text summary content."
            },
            "block_ref": {
                "type": "string",
                "description": "Block ref to nest inline (e.g. \"b1\"). Must reference a consumed block within the selected range."
            }
        },
        "additionalProperties": false
    })
}

// ── Argument parser ───────────────────────────────────────────────────

/// Parse the compress tool arguments into a typed `CompressRequest`.
///
/// This is a pure function — no memory access, no side effects.
/// Validates ref format and summary part shape but does NOT validate
/// refs against actual messages (that is the engine's job).
fn parse_compress_arguments(arguments: &str) -> Result<CompressRequest> {
    let value: Value = if arguments.trim().is_empty() {
        return Err(anyhow!(
            "compress requires at least one range or message entry"
        ));
    } else {
        serde_json::from_str(arguments)?
    };

    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("compress arguments must be a JSON object"))?;

    let ranges = obj.get("ranges").and_then(|v| v.as_array());
    let messages = obj.get("messages").and_then(|v| v.as_array());

    if ranges.is_none() && messages.is_none() {
        return Err(anyhow!(
            "compress requires at least one range or message entry"
        ));
    }

    let mut entries = Vec::new();

    if let Some(ranges) = ranges {
        for (i, range) in ranges.iter().enumerate() {
            let start_str = range
                .get("start")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("ranges[{i}].start is required and must be a string"))?;
            let end_str = range
                .get("end")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("ranges[{i}].end is required and must be a string"))?;

            let start = start_str
                .parse::<MessageRef>()
                .map_err(|e| anyhow!("ranges[{i}].start: {e}"))?;
            let end = end_str
                .parse::<MessageRef>()
                .map_err(|e| anyhow!("ranges[{i}].end: {e}"))?;

            let summary =
                parse_summary_parts(range.get("summary"), &format!("ranges[{i}].summary"))?;

            entries.push(CompressEntry {
                selection: CompressionSelection::Range { start, end },
                summary,
            });
        }
    }

    if let Some(messages) = messages {
        for (i, msg) in messages.iter().enumerate() {
            let refs_arr = msg
                .get("refs")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow!("messages[{i}].refs is required and must be an array"))?;

            if refs_arr.is_empty() {
                return Err(anyhow!("messages[{i}].refs must not be empty"));
            }

            let mut refs = Vec::with_capacity(refs_arr.len());
            for (j, ref_val) in refs_arr.iter().enumerate() {
                let ref_str = ref_val
                    .as_str()
                    .ok_or_else(|| anyhow!("messages[{i}].refs[{j}] must be a string"))?;
                let ref_parsed = ref_str
                    .parse::<MessageRef>()
                    .map_err(|e| anyhow!("messages[{i}].refs[{j}]: {e}"))?;
                refs.push(ref_parsed);
            }

            let summary =
                parse_summary_parts(msg.get("summary"), &format!("messages[{i}].summary"))?;

            entries.push(CompressEntry {
                selection: CompressionSelection::Messages { refs },
                summary,
            });
        }
    }

    if entries.is_empty() {
        return Err(anyhow!("compress requires at least one entry"));
    }

    Ok(CompressRequest { entries })
}

/// Parse summary parts from a JSON array.
fn parse_summary_parts(value: Option<&Value>, path: &str) -> Result<Vec<SummaryPart>> {
    let arr = value
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("{path} is required and must be a non-empty array"))?;

    if arr.is_empty() {
        return Err(anyhow!("{path} must not be empty"));
    }

    let mut parts = Vec::with_capacity(arr.len());
    for (i, part) in arr.iter().enumerate() {
        let obj = part
            .as_object()
            .ok_or_else(|| anyhow!("{path}[{i}] must be an object"))?;

        let has_text = obj.contains_key("text");
        let has_block_ref = obj.contains_key("block_ref");

        match (has_text, has_block_ref) {
            (true, false) => {
                let text = obj
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("{path}[{i}].text must be a string"))?;
                if text.trim().is_empty() {
                    return Err(anyhow!("{path}[{i}].text must not be empty"));
                }
                parts.push(SummaryPart::Text(text.to_string()));
            }
            (false, true) => {
                let ref_str = obj
                    .get("block_ref")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("{path}[{i}].block_ref must be a string"))?;
                let block_ref = ref_str
                    .parse::<BlockRef>()
                    .map_err(|e| anyhow!("{path}[{i}].block_ref: {e}"))?;
                parts.push(SummaryPart::BlockRef(block_ref));
            }
            (true, true) => {
                return Err(anyhow!(
                    "{path}[{i}] must have exactly one of `text` or `block_ref`, not both"
                ));
            }
            (false, false) => {
                return Err(anyhow!(
                    "{path}[{i}] must have exactly one of `text` or `block_ref`"
                ));
            }
        }
    }

    Ok(parts)
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

        // Parse arguments into typed CompressRequest.
        let request = parse_compress_arguments(&invocation.raw_arguments)
            .map_err(compression_runtime_error)?;

        // Serialize request into structured_payload for the runner to consume.
        let payload = serde_json::to_value(&request)
            .map_err(|e| ToolRuntimeError::Internal(e.to_string()))?;

        let entry_count = request.entries.len();
        let stdout = format!(
            "Compression request parsed: {entry_count} entr{} ready to apply.",
            if entry_count == 1 { "y" } else { "ies" }
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
        || message.contains("is required")
        || message.contains("must be")
        || message.contains("must not be")
        || message.contains("requires at least")
        || message.contains("must have exactly one")
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
            session_id: SessionId::from(77),
            turn_id: TurnId::from("turn-compression"),
            batch_id: ToolBatchId::from("batch-compression"),
            batch_index: 0,
            invocation_id: InvocationId::from(format!("invoke-{tool_name}")),
            tool_call_id: ToolCallId::from(format!("call-{tool_name}")),
            provider_tool_call_id: None,
            tool_name: ToolName::from(tool_name),
            raw_provider_payload: json!({}),
            raw_arguments: raw_arguments.to_string(),
            normalized_arguments: serde_json::Value::Null,
            cancellation_token: CancellationToken::new(),
            timeout: ToolTimeoutConfig::default(),
            execution_context: ToolExecutionContext::new(std::env::temp_dir()),
            provider_metadata: ProviderMetadata {
                provider: "test".to_string(),
                protocol: "chat_like".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "test-model".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: now,
            started_at: Some(now),
        }
    }

    // ── Tool definition tests ─────────────────────────────────────────

    #[test]
    fn exposes_compress_tool_definition() {
        let provider = Arc::new(CompressionProvider::new());
        let executors = provider.tool_runtime_executors();

        assert_eq!(executors.len(), 1);
        let spec = executors[0].spec();
        assert_eq!(spec.name, TOOL_COMPRESS);
        assert!(spec.description.contains("compress"));
        assert!(spec.description.contains("m0001"));
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
    fn compress_schema_has_ranges_and_messages() {
        let provider = Arc::new(CompressionProvider::new());
        let executors = provider.tool_runtime_executors();
        let spec = executors[0].spec();
        let params = &spec.parameters;
        assert!(params["properties"].get("ranges").is_some());
        assert!(params["properties"].get("messages").is_some());
        assert!(params["additionalProperties"].is_boolean());
    }

    // ── Parser tests ──────────────────────────────────────────────────

    #[test]
    fn parse_valid_range_compression() {
        let args = r#"{
            "ranges": [
                {
                    "start": "m0003",
                    "end": "m0008",
                    "summary": [
                        {"text": "User asked to refactor auth module"},
                        {"text": "We explored three approaches"}
                    ]
                }
            ]
        }"#;
        let request = parse_compress_arguments(args).expect("valid range parse");
        assert_eq!(request.entries.len(), 1);
        match &request.entries[0].selection {
            CompressionSelection::Range { start, end } => {
                assert_eq!(start.to_string(), "m0003");
                assert_eq!(end.to_string(), "m0008");
            }
            CompressionSelection::Messages { .. } => panic!("expected Range selection"),
        }
        assert_eq!(request.entries[0].summary.len(), 2);
    }

    #[test]
    fn parse_valid_messages_compression() {
        let args = r#"{
            "messages": [
                {
                    "refs": ["m0005", "m0010", "m0015"],
                    "summary": [{"text": "Scattered tool outputs summarized"}]
                }
            ]
        }"#;
        let request = parse_compress_arguments(args).expect("valid messages parse");
        assert_eq!(request.entries.len(), 1);
        match &request.entries[0].selection {
            CompressionSelection::Messages { refs } => {
                assert_eq!(refs.len(), 3);
                assert_eq!(refs[0].to_string(), "m0005");
            }
            CompressionSelection::Range { .. } => panic!("expected Messages selection"),
        }
    }

    #[test]
    fn parse_summary_with_block_ref() {
        let args = r#"{
            "ranges": [
                {
                    "start": "m0001",
                    "end": "m0010",
                    "summary": [
                        {"text": "Earlier work "},
                        {"block_ref": "b1"},
                        {"text": " continued in this range"}
                    ]
                }
            ]
        }"#;
        let request = parse_compress_arguments(args).expect("valid block ref parse");
        assert_eq!(request.entries[0].summary.len(), 3);
        assert_eq!(
            request.entries[0].summary[1],
            SummaryPart::BlockRef(BlockRef::new(1))
        );
    }

    #[test]
    fn parse_multiple_entries() {
        let args = r#"{
            "ranges": [
                {
                    "start": "m0001",
                    "end": "m0005",
                    "summary": [{"text": "First range"}]
                },
                {
                    "start": "m0010",
                    "end": "m0015",
                    "summary": [{"text": "Second range"}]
                }
            ],
            "messages": [
                {
                    "refs": ["m0020"],
                    "summary": [{"text": "One message"}]
                }
            ]
        }"#;
        let request = parse_compress_arguments(args).expect("valid multi-entry parse");
        assert_eq!(request.entries.len(), 3);
    }

    #[test]
    fn parse_empty_arguments_rejected() {
        let result = parse_compress_arguments("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least one"));
    }

    #[test]
    fn parse_no_ranges_or_messages_rejected() {
        let result = parse_compress_arguments("{}");
        assert!(result.is_err());
    }

    #[test]
    fn parse_invalid_ref_format_rejected() {
        let args = r#"{
            "ranges": [
                {
                    "start": "invalid",
                    "end": "m0008",
                    "summary": [{"text": "test"}]
                }
            ]
        }"#;
        let result = parse_compress_arguments(args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("start"));
    }

    #[test]
    fn parse_summary_part_with_both_text_and_block_ref_rejected() {
        let args = r#"{
            "ranges": [
                {
                    "start": "m0001",
                    "end": "m0005",
                    "summary": [
                        {"text": "hello", "block_ref": "b1"}
                    ]
                }
            ]
        }"#;
        let result = parse_compress_arguments(args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("exactly one"));
    }

    #[test]
    fn parse_summary_part_with_neither_text_nor_block_ref_rejected() {
        let args = r#"{
            "ranges": [
                {
                    "start": "m0001",
                    "end": "m0005",
                    "summary": [{}]
                }
            ]
        }"#;
        let result = parse_compress_arguments(args);
        assert!(result.is_err());
    }

    #[test]
    fn parse_empty_summary_rejected() {
        let args = r#"{
            "ranges": [
                {
                    "start": "m0001",
                    "end": "m0005",
                    "summary": []
                }
            ]
        }"#;
        let result = parse_compress_arguments(args);
        assert!(result.is_err());
    }

    #[test]
    fn parse_empty_text_rejected() {
        let args = r#"{
            "ranges": [
                {
                    "start": "m0001",
                    "end": "m0005",
                    "summary": [{"text": "  "}]
                }
            ]
        }"#;
        let result = parse_compress_arguments(args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn parse_empty_messages_refs_rejected() {
        let args = r#"{
            "messages": [
                {
                    "refs": [],
                    "summary": [{"text": "test"}]
                }
            ]
        }"#;
        let result = parse_compress_arguments(args);
        assert!(result.is_err());
    }

    #[test]
    fn parse_zero_block_ref_rejected() {
        let args = r#"{
            "ranges": [
                {
                    "start": "m0001",
                    "end": "m0005",
                    "summary": [{"block_ref": "b0"}]
                }
            ]
        }"#;
        let result = parse_compress_arguments(args);
        assert!(result.is_err());
    }

    #[test]
    fn compress_request_serde_round_trip() {
        let request = CompressRequest {
            entries: vec![CompressEntry {
                selection: CompressionSelection::Range {
                    start: MessageRef::from_index(2),
                    end: MessageRef::from_index(7),
                },
                summary: vec![
                    SummaryPart::Text("test summary".to_string()),
                    SummaryPart::BlockRef(BlockRef::new(1)),
                ],
            }],
        };
        let json = serde_json::to_string(&request).expect("serialize");
        let restored: CompressRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(request, restored);
    }

    // ── CompressResult tests ──────────────────────────────────────────

    #[test]
    fn compress_result_all_success() {
        let outcomes = vec![Ok(BlockRef::new(1)), Ok(BlockRef::new(2))];
        let result = CompressResult::from_outcomes(outcomes);
        assert!(result.compressed);
        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.entries[0].block_id.as_deref(), Some("b1"));
        assert_eq!(result.entries[1].block_id.as_deref(), Some("b2"));
    }

    #[test]
    fn compress_result_partial_failure() {
        let outcomes = vec![
            Ok(BlockRef::new(3)),
            Err("selection splits tool-call/result pair at index 5".to_string()),
        ];
        let result = CompressResult::from_outcomes(outcomes);
        assert!(!result.compressed);
        assert!(result.entries[0].compressed);
        assert!(!result.entries[1].compressed);
        assert_eq!(
            result.entries[1].error.as_deref(),
            Some("splits_tool_batch")
        );
    }

    #[test]
    fn compress_result_all_failure() {
        let outcomes = vec![Err("message ref m0099 is stale or out of range".to_string())];
        let result = CompressResult::from_outcomes(outcomes);
        assert!(!result.compressed);
        assert_eq!(
            result.entries[0].error.as_deref(),
            Some("invalid_message_ref")
        );
    }

    #[test]
    fn compress_result_to_json() {
        let result = CompressResult::from_outcomes(vec![Ok(BlockRef::new(1))]);
        let json = result.to_json().expect("serialize");
        assert!(json.contains("\"compressed\": true"));
        assert!(json.contains("\"b1\""));
    }

    #[test]
    fn extract_error_kind_maps_known_errors() {
        assert_eq!(
            extract_error_kind("message ref m0099 is stale or out of range"),
            "invalid_message_ref"
        );
        assert_eq!(
            extract_error_kind("range start m0005 must be <= end m0003"),
            "invalid_range"
        );
        assert_eq!(extract_error_kind("selection is empty"), "empty_selection");
        assert_eq!(
            extract_error_kind("selection overlaps with active block b2"),
            "overlaps_active_block"
        );
        assert_eq!(
            extract_error_kind("selection splits tool-call/result pair at index 5"),
            "splits_tool_batch"
        );
        assert_eq!(
            extract_error_kind("summary references block b9 which is not a consumed block"),
            "invalid_block_ref"
        );
        assert_eq!(
            extract_error_kind("summary references block b1 more than once"),
            "duplicate_block_ref"
        );
    }

    // ── Executor integration tests ────────────────────────────────────

    #[tokio::test]
    async fn typed_runtime_executor_parses_valid_range_request() {
        let provider = Arc::new(CompressionProvider::new());
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("compress typed executor registered");

        let args = r#"{
            "ranges": [
                {
                    "start": "m0003",
                    "end": "m0008",
                    "summary": [{"text": "Summary of earlier conversation"}]
                }
            ]
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
        assert_eq!(request.entries.len(), 1);
    }

    #[tokio::test]
    async fn typed_runtime_executor_rejects_invalid_arguments() {
        let provider = Arc::new(CompressionProvider::new());
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("compress typed executor registered");

        let error = executor
            .execute(runtime_invocation(TOOL_COMPRESS, r#"{"force":true}"#))
            .await
            .expect_err("invalid args must be rejected");

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

    #[tokio::test]
    async fn typed_runtime_executor_sets_structured_payload_with_request() {
        let provider = Arc::new(CompressionProvider::new());
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("compress typed executor registered");

        let args = r#"{
            "messages": [
                {
                    "refs": ["m0005", "m0010"],
                    "summary": [{"text": "Tool outputs summarized"}, {"block_ref": "b1"}]
                }
            ]
        }"#;

        let output = executor
            .execute(runtime_invocation(TOOL_COMPRESS, args))
            .await
            .expect("compress parse succeeds");

        let payload = output
            .structured_payload
            .as_ref()
            .expect("structured_payload must be set");
        let request: CompressRequest = serde_json::from_value(payload.clone())
            .expect("payload deserializes to CompressRequest");

        match &request.entries[0].selection {
            CompressionSelection::Messages { refs } => {
                assert_eq!(refs.len(), 2);
            }
            CompressionSelection::Range { .. } => panic!("expected Messages selection"),
        }
        assert_eq!(request.entries[0].summary.len(), 2);
        assert_eq!(
            request.entries[0].summary[1],
            SummaryPart::BlockRef(BlockRef::new(1))
        );
    }
}
