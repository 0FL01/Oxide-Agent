//! Structured output parsing and validation.
//!
//! Parses the strict JSON response format used by the agent loop and validates
//! that it conforms to the expected schema.

use crate::llm::ToolDefinition;
use serde::Deserialize;
use std::fmt;

/// Error returned when structured output parsing or validation fails.
#[derive(Debug, Clone)]
pub struct StructuredOutputError {
    message: String,
}

impl StructuredOutputError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    #[must_use]
    /// Returns the validation error message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for StructuredOutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for StructuredOutputError {}

#[derive(Debug, Deserialize)]
struct StructuredOutput {
    thought: String,
    tool_call: Option<StructuredToolCall>,
    final_answer: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StructuredToolCall {
    name: String,
    arguments: serde_json::Value,
}

/// Parsed and validated structured output from the agent model.
#[derive(Debug, Clone)]
pub struct ValidatedStructuredOutput {
    /// Thought string describing the current reasoning step.
    pub thought: String,
    /// Tool call payload if a tool should be executed.
    pub tool_call: Option<ValidatedToolCall>,
    /// Final answer payload if the task is complete.
    pub final_answer: Option<String>,
}

/// Validated tool call payload extracted from the structured response.
#[derive(Debug, Clone)]
pub struct ValidatedToolCall {
    /// Tool name to execute.
    pub name: String,
    /// Serialized JSON arguments for the tool.
    pub arguments_json: String,
}

/// Parse and validate a structured JSON response against the agent schema.
pub fn parse_structured_output(
    raw: &str,
    tools: &[ToolDefinition],
) -> Result<ValidatedStructuredOutput, StructuredOutputError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(StructuredOutputError::new("Empty response content"));
    }

    let parsed: StructuredOutput = serde_json::from_str(trimmed)
        .map_err(|e| StructuredOutputError::new(format!("JSON parse error: {e}")))?;

    validate_structured_output(parsed, tools)
}

fn validate_structured_output(
    output: StructuredOutput,
    tools: &[ToolDefinition],
) -> Result<ValidatedStructuredOutput, StructuredOutputError> {
    if output.thought.trim().is_empty() {
        return Err(StructuredOutputError::new(
            "Field 'thought' must be a non-empty string",
        ));
    }

    if let Some(ref final_answer) = output.final_answer {
        if final_answer.trim().is_empty() {
            return Err(StructuredOutputError::new(
                "Field 'final_answer' must be a non-empty string when provided",
            ));
        }
    }

    let has_tool_call = output.tool_call.is_some();
    let has_final_answer = output.final_answer.is_some();

    if has_tool_call == has_final_answer {
        return Err(StructuredOutputError::new(
            "Exactly one of 'tool_call' or 'final_answer' must be set",
        ));
    }

    let mut validated_tool_call = None;
    if let Some(tool_call) = output.tool_call {
        let name = tool_call.name.trim().to_string();
        if name.is_empty() {
            return Err(StructuredOutputError::new(
                "Field 'tool_call.name' must be a non-empty string",
            ));
        }

        let known_tool = tools.iter().any(|tool| tool.name == name);
        if !known_tool {
            let available = tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let available = if available.is_empty() {
                "no tools available".to_string()
            } else {
                available
            };
            return Err(StructuredOutputError::new(format!(
                "Unknown tool '{name}'. Available tools: {available}"
            )));
        }

        if !tool_call.arguments.is_object() {
            return Err(StructuredOutputError::new(
                "Field 'tool_call.arguments' must be a JSON object",
            ));
        }

        let arguments_json = serde_json::to_string(&tool_call.arguments).map_err(|e| {
            StructuredOutputError::new(format!("Failed to serialize 'tool_call.arguments': {e}"))
        })?;

        validated_tool_call = Some(ValidatedToolCall {
            name,
            arguments_json,
        });
    }

    Ok(ValidatedStructuredOutput {
        thought: output.thought,
        tool_call: validated_tool_call,
        final_answer: output.final_answer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools_fixture() -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({"type": "object"}),
        }]
    }

    #[test]
    fn parses_valid_final_answer() {
        let raw = r#"{"thought":"done","tool_call":null,"final_answer":"ok"}"#;
        let result = parse_structured_output(raw, &tools_fixture());
        if let Ok(parsed) = result {
            assert_eq!(parsed.final_answer.as_deref(), Some("ok"));
            assert!(parsed.tool_call.is_none());
        } else {
            panic!("Expected valid structured output");
        }
    }

    #[test]
    fn parses_valid_tool_call() {
        let raw = r#"{"thought":"need file","tool_call":{"name":"read_file","arguments":{"path":"test.txt"}},"final_answer":null}"#;
        let result = parse_structured_output(raw, &tools_fixture());
        if let Ok(parsed) = result {
            if let Some(tool_call) = parsed.tool_call {
                assert_eq!(tool_call.name, "read_file");
                assert!(tool_call.arguments_json.contains("test.txt"));
            } else {
                panic!("tool_call should be present");
            }
        } else {
            panic!("Expected valid structured output");
        }
    }

    #[test]
    fn rejects_missing_both() {
        let raw = r#"{"thought":"none","tool_call":null,"final_answer":null}"#;
        let result = parse_structured_output(raw, &tools_fixture());
        assert!(result.is_err());
    }

    #[test]
    fn rejects_both_set() {
        let raw = r#"{"thought":"bad","tool_call":{"name":"read_file","arguments":{}},"final_answer":"nope"}"#;
        let result = parse_structured_output(raw, &tools_fixture());
        assert!(result.is_err());
    }

    #[test]
    fn rejects_unknown_tool() {
        let raw = r#"{"thought":"bad","tool_call":{"name":"missing","arguments":{}},"final_answer":null}"#;
        let result = parse_structured_output(raw, &tools_fixture());
        assert!(result.is_err());
    }

    #[test]
    fn rejects_non_object_arguments() {
        let raw = r#"{"thought":"bad","tool_call":{"name":"read_file","arguments":"no"},"final_answer":null}"#;
        let result = parse_structured_output(raw, &tools_fixture());
        assert!(result.is_err());
    }

    #[test]
    fn rejects_empty_thought() {
        let raw = r#"{"thought":" ","tool_call":null,"final_answer":"ok"}"#;
        let result = parse_structured_output(raw, &tools_fixture());
        assert!(result.is_err());
    }
}
