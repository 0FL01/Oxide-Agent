//! Strict opencode-go chat-like tool-call parser and output encoder.

use super::output::ToolOutput;
use super::types::{ToolCallId, ToolName, TurnId};
use crate::llm::{
    InvocationId, ToolCall, ToolCallCorrelation, ToolCallFunction, ToolProtocol, ToolTransport,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use thiserror::Error;

const SYNTHETIC_PROTOCOL_TOOL_NAME: &str = "oxide_provider_protocol_error";

/// Strict parser for opencode-go chat-like `tool_calls[]`.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenCodeGoToolCallParser;

/// Encoder for opencode-go chat-like tool output messages.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenCodeGoToolOutputEncoder;

/// Parsed batch preserving original provider order.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCodeGoToolCallBatch {
    /// Current turn id used for deterministic synthetic ids.
    pub turn_id: TurnId,
    /// Parsed calls in original provider order.
    pub calls: Vec<OpenCodeGoParsedToolCall>,
}

impl OpenCodeGoToolCallBatch {
    /// Convert parsed calls to existing LLM `ToolCall` history items.
    #[must_use]
    pub fn to_llm_tool_calls(&self) -> Vec<ToolCall> {
        self.calls
            .iter()
            .map(OpenCodeGoParsedToolCall::to_llm_tool_call)
            .collect()
    }
}

/// One parsed opencode-go tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCodeGoParsedToolCall {
    /// Original assistant batch index.
    pub batch_index: usize,
    /// Internal runtime invocation id.
    pub invocation_id: InvocationId,
    /// Provider-visible id used in assistant/tool history.
    pub tool_call_id: ToolCallId,
    /// Source provider id before synthetic repair, when present.
    pub original_provider_tool_call_id: Option<String>,
    /// Exact tool name or a synthetic protocol-error tool name.
    pub tool_name: ToolName,
    /// Raw JSON argument string passed to invocation argument parsing.
    pub raw_arguments: String,
    /// Raw provider call payload for diagnostics.
    pub raw_provider_payload: Value,
    /// Pairable protocol issue. Calls with this set should not execute tools.
    pub protocol_issue: Option<OpenCodeGoProtocolIssue>,
}

impl OpenCodeGoParsedToolCall {
    /// Convert to the existing LLM history tool-call shape with chat-like correlation.
    #[must_use]
    pub fn to_llm_tool_call(&self) -> ToolCall {
        ToolCall::new(
            self.invocation_id.clone().into_inner(),
            ToolCallFunction {
                name: self.tool_name.as_str().to_string(),
                arguments: self.raw_arguments.clone(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new(self.invocation_id.clone())
                .with_provider_tool_call_id(self.tool_call_id.as_str())
                .with_protocol(ToolProtocol::ChatLike)
                .with_transport(ToolTransport::ClientRoundTrip),
        )
    }

    /// Whether the call is only a pairable provider protocol error.
    #[must_use]
    pub const fn is_protocol_error(&self) -> bool {
        self.protocol_issue.is_some()
    }
}

/// Pairable opencode-go protocol issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenCodeGoProtocolIssue {
    /// Provider omitted or sent an empty tool-call id.
    MissingToolCallId,
    /// Provider reused a tool-call id in one batch.
    DuplicateToolCallId,
    /// `function.arguments` was neither a JSON string nor a JSON object.
    UnsupportedArgumentsType,
}

impl OpenCodeGoProtocolIssue {
    /// Concise model-facing message.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::MissingToolCallId => "provider omitted tool_call_id",
            Self::DuplicateToolCallId => "provider returned duplicate tool_call_id in one batch",
            Self::UnsupportedArgumentsType => {
                "provider returned unsupported function.arguments type"
            }
        }
    }
}

/// Fatal parser errors where a pairable assistant tool-call message is unsafe.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OpenCodeGoToolParseError {
    /// `tool_calls` was not an array.
    #[error("opencode-go tool_calls must be an array")]
    NonArrayToolCalls,
    /// One item cannot be represented as a valid chat-like function tool call.
    #[error("unpairable opencode-go tool call at index {index}: {reason}")]
    UnpairableToolCall {
        /// Original batch index.
        index: usize,
        /// Concise reason.
        reason: String,
    },
}

impl OpenCodeGoToolCallParser {
    /// Parse and repair a provider batch into pairable runtime calls.
    ///
    /// # Errors
    ///
    /// Returns an error when the payload cannot be represented as valid paired
    /// chat-like assistant/tool history.
    pub fn parse_batch(
        self,
        turn_id: TurnId,
        tool_calls: &Value,
    ) -> Result<OpenCodeGoToolCallBatch, OpenCodeGoToolParseError> {
        let array = tool_calls
            .as_array()
            .ok_or(OpenCodeGoToolParseError::NonArrayToolCalls)?;
        let mut seen_ids = BTreeSet::new();
        let mut calls = Vec::with_capacity(array.len());

        for (batch_index, raw_call) in array.iter().enumerate() {
            calls.push(parse_one_call(
                &turn_id,
                batch_index,
                raw_call,
                &mut seen_ids,
            )?);
        }

        Ok(OpenCodeGoToolCallBatch { turn_id, calls })
    }
}

impl OpenCodeGoToolOutputEncoder {
    /// Encode one typed tool output as an opencode-go chat-like tool message.
    ///
    /// # Errors
    ///
    /// Returns a serialization error if the typed output payload cannot encode.
    pub fn encode(self, output: &ToolOutput) -> Result<Value, serde_json::Error> {
        let content = output.encode_model_content()?;
        Ok(json!({
            "role": "tool",
            "tool_call_id": output.tool_call_id.as_str(),
            "content": content,
        }))
    }
}

fn parse_one_call(
    turn_id: &TurnId,
    batch_index: usize,
    raw_call: &Value,
    seen_ids: &mut BTreeSet<String>,
) -> Result<OpenCodeGoParsedToolCall, OpenCodeGoToolParseError> {
    let function = raw_call
        .get("function")
        .ok_or_else(|| unpairable(batch_index, "missing function object"))?;
    let raw_tool_name = function
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| unpairable(batch_index, "missing function.name"))?;
    let (tool_call_id, original_id, id_issue) = resolve_tool_call_id(
        turn_id,
        batch_index,
        raw_call.get("id").and_then(Value::as_str),
        seen_ids,
    );
    let (raw_arguments, arguments_issue) = normalize_arguments(function.get("arguments"));
    let protocol_issue = id_issue.or(arguments_issue);
    let tool_name = if protocol_issue.is_some() {
        ToolName::from(SYNTHETIC_PROTOCOL_TOOL_NAME)
    } else {
        ToolName::from(raw_tool_name)
    };

    Ok(OpenCodeGoParsedToolCall {
        batch_index,
        invocation_id: InvocationId::from(format!(
            "oxide_invocation_{}_{}",
            turn_id.as_str(),
            batch_index
        )),
        tool_call_id,
        original_provider_tool_call_id: original_id,
        tool_name,
        raw_arguments,
        raw_provider_payload: raw_call.clone(),
        protocol_issue,
    })
}

fn resolve_tool_call_id(
    turn_id: &TurnId,
    batch_index: usize,
    raw_id: Option<&str>,
    seen_ids: &mut BTreeSet<String>,
) -> (ToolCallId, Option<String>, Option<OpenCodeGoProtocolIssue>) {
    let Some(trimmed) = raw_id.map(str::trim).filter(|id| !id.is_empty()) else {
        return (
            ToolCallId::from(format!(
                "oxide_missing_tool_call_id_{}_{}",
                turn_id.as_str(),
                batch_index
            )),
            None,
            Some(OpenCodeGoProtocolIssue::MissingToolCallId),
        );
    };

    if !seen_ids.insert(trimmed.to_string()) {
        return (
            ToolCallId::from(format!(
                "oxide_duplicate_tool_call_id_{}_{}",
                turn_id.as_str(),
                batch_index
            )),
            Some(trimmed.to_string()),
            Some(OpenCodeGoProtocolIssue::DuplicateToolCallId),
        );
    }

    (ToolCallId::from(trimmed), Some(trimmed.to_string()), None)
}

fn normalize_arguments(arguments: Option<&Value>) -> (String, Option<OpenCodeGoProtocolIssue>) {
    match arguments {
        Some(Value::String(arguments)) => (arguments.clone(), None),
        Some(object @ Value::Object(_)) => {
            (serde_json::to_string(object).unwrap_or_default(), None)
        }
        None => ("{}".to_string(), None),
        Some(other) => (
            serde_json::to_string(other).unwrap_or_default(),
            Some(OpenCodeGoProtocolIssue::UnsupportedArgumentsType),
        ),
    }
}

fn unpairable(index: usize, reason: &str) -> OpenCodeGoToolParseError {
    OpenCodeGoToolParseError::UnpairableToolCall {
        index,
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tool_runtime::output::{
        CleanupStatus, OutputTruncationMetadata, ToolOutput, ToolOutputIdentity, ToolOutputStatus,
    };
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn parser_preserves_valid_tool_calls_and_wire_ids() {
        let batch = parse(json!([
            {
                "id": "call_one",
                "type": "function",
                "function": {
                    "name": "read_file",
                    "arguments": "{\"path\":\"Cargo.toml\"}"
                }
            }
        ]));

        let call = &batch.calls[0];
        assert_eq!(call.tool_call_id.as_str(), "call_one");
        assert_eq!(call.tool_name.as_str(), "read_file");
        assert_eq!(call.raw_arguments, "{\"path\":\"Cargo.toml\"}");
        assert_eq!(call.protocol_issue, None);

        let llm_call = call.to_llm_tool_call();
        assert_ne!(llm_call.invocation_id().as_str(), "call_one");
        assert_eq!(llm_call.wire_tool_call_id(), "call_one");
    }

    #[test]
    fn parser_accepts_object_arguments_as_canonical_json() {
        let batch = parse(json!([
            {
                "id": "call_object",
                "type": "function",
                "function": {
                    "name": "read_file",
                    "arguments": { "path": "Cargo.toml" }
                }
            }
        ]));

        assert_eq!(batch.calls[0].raw_arguments, r#"{"path":"Cargo.toml"}"#);
    }

    #[test]
    fn parser_repairs_missing_and_duplicate_ids_before_history_write() {
        let batch = parse(json!([
            {
                "type": "function",
                "function": { "name": "read_file", "arguments": "{}" }
            },
            {
                "id": "call_dup",
                "type": "function",
                "function": { "name": "read_file", "arguments": "{}" }
            },
            {
                "id": "call_dup",
                "type": "function",
                "function": { "name": "read_file", "arguments": "{}" }
            }
        ]));

        assert_eq!(
            batch.calls[0].tool_call_id.as_str(),
            "oxide_missing_tool_call_id_turn_42_0"
        );
        assert_eq!(
            batch.calls[0].protocol_issue,
            Some(OpenCodeGoProtocolIssue::MissingToolCallId)
        );
        assert_eq!(batch.calls[1].tool_call_id.as_str(), "call_dup");
        assert_eq!(
            batch.calls[2].tool_call_id.as_str(),
            "oxide_duplicate_tool_call_id_turn_42_2"
        );
        assert_eq!(
            batch.calls[2].protocol_issue,
            Some(OpenCodeGoProtocolIssue::DuplicateToolCallId)
        );
        assert!(batch.calls[0].is_protocol_error());
        assert_eq!(
            batch.calls[0].to_llm_tool_call().wire_tool_call_id(),
            "oxide_missing_tool_call_id_turn_42_0"
        );
    }

    #[test]
    fn unsupported_argument_shape_becomes_pairable_protocol_error() {
        let batch = parse(json!([
            {
                "id": "call_bad_args",
                "type": "function",
                "function": {
                    "name": "read_file",
                    "arguments": ["not", "an", "object"]
                }
            }
        ]));

        assert_eq!(
            batch.calls[0].protocol_issue,
            Some(OpenCodeGoProtocolIssue::UnsupportedArgumentsType)
        );
        assert_eq!(
            batch.calls[0].tool_name.as_str(),
            SYNTHETIC_PROTOCOL_TOOL_NAME
        );
    }

    #[test]
    fn missing_function_or_name_is_unpairable() {
        let parser = OpenCodeGoToolCallParser;
        let missing_function =
            parser.parse_batch(TurnId::from("turn_42"), &json!([{ "id": "call_missing" }]));
        assert!(matches!(
            missing_function,
            Err(OpenCodeGoToolParseError::UnpairableToolCall { index: 0, .. })
        ));

        let missing_name = parser.parse_batch(
            TurnId::from("turn_42"),
            &json!([{ "id": "call_missing", "function": {} }]),
        );
        assert!(matches!(
            missing_name,
            Err(OpenCodeGoToolParseError::UnpairableToolCall { index: 0, .. })
        ));
    }

    #[test]
    fn output_encoder_uses_exact_tool_call_id_and_json_content() {
        let output = protocol_output("call_encoded");
        let encoded = OpenCodeGoToolOutputEncoder
            .encode(&output)
            .expect("output encodes");
        let content: serde_json::Value =
            serde_json::from_str(encoded["content"].as_str().expect("content string"))
                .expect("content JSON");

        assert_eq!(encoded["role"], "tool");
        assert_eq!(encoded["tool_call_id"], "call_encoded");
        assert_eq!(content["tool_call_id"], "call_encoded");
        assert_eq!(content["status"], "provider_protocol_error");
    }

    fn parse(value: Value) -> OpenCodeGoToolCallBatch {
        OpenCodeGoToolCallParser
            .parse_batch(TurnId::from("turn_42"), &value)
            .expect("batch parses")
    }

    fn protocol_output(tool_call_id: &str) -> ToolOutput {
        let now = Utc::now();
        ToolOutput::terminal(
            ToolOutputIdentity {
                tool_call_id: ToolCallId::from(tool_call_id),
                provider_tool_call_id: None,
                invocation_id: InvocationId::from(format!("invoke_{tool_call_id}")),
                tool_name: ToolName::from("oxide_provider_protocol_error"),
                batch_index: 0,
            },
            ToolOutputStatus::ProviderProtocolError,
            now,
            now,
            OutputTruncationMetadata::new(65_536, 65_536, 131_072),
        )
        .with_error_message("provider omitted tool_call_id")
        .with_cleanup_status(CleanupStatus::NotStarted)
    }
}
