use crate::llm::providers::protocol_profiles::CHAT_LIKE_TOOL_PROFILE;
use crate::llm::{ToolCall, ToolDefinition};
use gemini_rust::{
    FunctionCall as GeminiFunctionCall, FunctionDeclaration,
    FunctionResponse as GeminiFunctionResponse,
};
use serde_json::{json, Map, Value};

use super::GeminiProvider;

impl GeminiProvider {
    fn sanitize_function_schema(value: Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut sanitized = Map::with_capacity(map.len());

                for (key, value) in map {
                    if key == "additionalProperties" {
                        continue;
                    }

                    if key == "enum" {
                        if let Value::Array(items) = value {
                            if items.iter().all(Value::is_string) {
                                sanitized.insert(
                                    key,
                                    Value::Array(
                                        items
                                            .into_iter()
                                            .map(Self::sanitize_function_schema)
                                            .collect(),
                                    ),
                                );
                            }
                        }
                        continue;
                    }

                    sanitized.insert(key, Self::sanitize_function_schema(value));
                }

                Value::Object(sanitized)
            }
            Value::Array(items) => Value::Array(
                items
                    .into_iter()
                    .map(Self::sanitize_function_schema)
                    .collect(),
            ),
            other => other,
        }
    }

    pub(super) fn function_declarations(tools: &[ToolDefinition]) -> Vec<FunctionDeclaration> {
        tools
            .iter()
            .map(|tool| {
                FunctionDeclaration::new(tool.name.clone(), tool.description.clone(), None)
                    .with_parameters_schema(Self::sanitize_function_schema(tool.parameters.clone()))
            })
            .collect()
    }

    fn normalize_tool_arguments(value: &Value) -> String {
        match value {
            Value::Null => "{}".to_string(),
            Value::String(raw) => Self::normalize_tool_arguments_str(raw),
            other => serde_json::to_string(other).unwrap_or_else(|_| "{}".to_string()),
        }
    }

    fn normalize_tool_arguments_str(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return "{}".to_string();
        }

        let Ok(parsed) = serde_json::from_str::<Value>(trimmed) else {
            return trimmed.to_string();
        };

        match parsed {
            Value::String(inner) => match serde_json::from_str::<Value>(&inner) {
                Ok(inner_parsed) => serde_json::to_string(&inner_parsed).unwrap_or(inner),
                Err(_) => inner,
            },
            other => serde_json::to_string(&other).unwrap_or_else(|_| trimmed.to_string()),
        }
    }

    pub(super) fn sdk_function_call(
        name: impl Into<String>,
        arguments: &str,
        provider_id: Option<String>,
    ) -> GeminiFunctionCall {
        let name = name.into();
        let args = Self::tool_arguments_value(arguments);

        match provider_id.as_deref().map(str::trim) {
            Some(provider_id) if !provider_id.is_empty() => {
                GeminiFunctionCall::with_id(name, args, provider_id)
            }
            _ => GeminiFunctionCall::new(name, args),
        }
    }

    fn tool_arguments_value(arguments: &str) -> Value {
        match serde_json::from_str::<Value>(&Self::normalize_tool_arguments_str(arguments)) {
            Ok(Value::Object(map)) => Value::Object(map),
            Ok(other) => json!({ "input": other }),
            Err(_) => json!({ "input": arguments }),
        }
    }

    pub(super) fn sdk_function_response(
        name: impl Into<String>,
        content: &str,
        provider_id: Option<String>,
    ) -> GeminiFunctionResponse {
        let name = name.into();
        let response = Self::tool_result_value(content);

        match provider_id.as_deref().map(str::trim) {
            Some(provider_id) if !provider_id.is_empty() => {
                GeminiFunctionResponse::with_id(name, response, provider_id)
            }
            _ => GeminiFunctionResponse::new(name, response),
        }
    }

    pub(super) fn tool_result_value(content: &str) -> Value {
        match serde_json::from_str::<Value>(content) {
            Ok(Value::Object(map)) => Value::Object(map),
            Ok(other) => json!({ "output": other }),
            Err(_) => json!({ "output": content }),
        }
    }

    pub(super) fn parse_tool_call(function_call: &GeminiFunctionCall) -> ToolCall {
        let arguments = Self::normalize_tool_arguments(&function_call.args);
        match function_call.id.as_deref().map(str::trim) {
            Some(provider_id) if !provider_id.is_empty() => CHAT_LIKE_TOOL_PROFILE
                .inbound_provider_tool_call(
                    provider_id,
                    None,
                    function_call.name.clone(),
                    arguments,
                ),
            _ => CHAT_LIKE_TOOL_PROFILE
                .inbound_uncorrelated_tool_call(function_call.name.clone(), arguments),
        }
    }
}
