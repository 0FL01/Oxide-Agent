use crate::agent::provider::ToolProvider;
use crate::agent::task::{PendingChoiceInput, PendingInput, PendingInputKind, PendingTextInput};
use crate::llm::ToolDefinition;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RequestInputKindArg {
    Text,
    Choice,
}

#[derive(Debug, Deserialize)]
struct RequestUserInputArgs {
    prompt: String,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    kind: Option<RequestInputKindArg>,
    #[serde(default)]
    text: Option<RequestTextArgs>,
    #[serde(default)]
    choice: Option<RequestChoiceArgs>,
}

#[derive(Debug, Deserialize)]
struct RequestTextArgs {
    #[serde(default)]
    min_length: Option<u16>,
    #[serde(default)]
    max_length: Option<u16>,
    #[serde(default)]
    multiline: bool,
}

#[derive(Debug, Deserialize)]
struct RequestChoiceArgs {
    options: Vec<String>,
    #[serde(default)]
    allow_multiple: bool,
    #[serde(default = "one")]
    min_choices: u8,
    #[serde(default = "one")]
    max_choices: u8,
}

const fn one() -> u8 {
    1
}

fn to_pending_text(args: Option<RequestTextArgs>) -> PendingTextInput {
    match args {
        Some(value) => PendingTextInput {
            min_length: value.min_length,
            max_length: value.max_length,
            multiline: value.multiline,
        },
        None => PendingTextInput {
            min_length: None,
            max_length: None,
            multiline: false,
        },
    }
}

fn to_pending_choice(args: RequestChoiceArgs) -> PendingChoiceInput {
    PendingChoiceInput {
        options: args.options,
        allow_multiple: args.allow_multiple,
        min_choices: args.min_choices,
        max_choices: args.max_choices,
    }
}

/// Tool provider that allows production agent flows to pause for external user input.
pub struct RequestUserInputProvider;

impl RequestUserInputProvider {
    /// Construct a new input-request provider.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for RequestUserInputProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for RequestUserInputProvider {
    fn name(&self) -> &'static str {
        "request_user_input"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "request_user_input".to_string(),
            description: "Pause execution and request explicit user input before continuing the task. Use this only when external confirmation or missing information is required.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Question shown to the user"
                    },
                    "request_id": {
                        "type": "string",
                        "description": "Optional stable id for this input request"
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["text", "choice"],
                        "description": "Input type. If omitted, defaults to text unless choice options are provided"
                    },
                    "text": {
                        "type": "object",
                        "properties": {
                            "min_length": { "type": "integer", "minimum": 0, "maximum": 4096 },
                            "max_length": { "type": "integer", "minimum": 1, "maximum": 4096 },
                            "multiline": { "type": "boolean" }
                        }
                    },
                    "choice": {
                        "type": "object",
                        "properties": {
                            "options": {
                                "type": "array",
                                "items": { "type": "string" },
                                "minItems": 2,
                                "maxItems": 10
                            },
                            "allow_multiple": { "type": "boolean" },
                            "min_choices": { "type": "integer", "minimum": 1, "maximum": 10 },
                            "max_choices": { "type": "integer", "minimum": 1, "maximum": 10 }
                        },
                        "required": ["options"]
                    }
                },
                "required": ["prompt"]
            }),
        }]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        tool_name == "request_user_input"
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        if tool_name != "request_user_input" {
            return Err(anyhow!("Unknown input request tool: {tool_name}"));
        }

        let args: RequestUserInputArgs = serde_json::from_str(arguments)?;
        let request_id = args
            .request_id
            .unwrap_or_else(|| format!("hitl-{}", Uuid::new_v4()));

        let kind = match args.kind {
            Some(RequestInputKindArg::Text) => PendingInputKind::Text(to_pending_text(args.text)),
            Some(RequestInputKindArg::Choice) => {
                let choice = args.choice.ok_or_else(|| {
                    anyhow!("request_user_input.kind=choice requires a `choice` object")
                })?;
                PendingInputKind::Choice(to_pending_choice(choice))
            }
            None => match args.choice {
                Some(choice) => PendingInputKind::Choice(to_pending_choice(choice)),
                None => PendingInputKind::Text(to_pending_text(args.text)),
            },
        };

        let pending_input = PendingInput {
            request_id,
            prompt: args.prompt,
            kind,
        };
        pending_input.validate()?;

        serde_json::to_string(&pending_input).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_user_input_provider_builds_text_pending_input() {
        let provider = RequestUserInputProvider::new();
        let result = provider
            .execute(
                "request_user_input",
                r#"{"prompt":"Need approval","kind":"text","text":{"min_length":1,"max_length":64}}"#,
                None,
                None,
            )
            .await;

        let serialized = match result {
            Ok(value) => value,
            Err(error) => panic!("expected success, got error: {error}"),
        };
        let pending: PendingInput = match serde_json::from_str(&serialized) {
            Ok(value) => value,
            Err(error) => panic!("expected valid pending input json: {error}"),
        };

        match pending.kind {
            PendingInputKind::Text(text) => {
                assert_eq!(pending.prompt, "Need approval");
                assert_eq!(text.min_length, Some(1));
                assert_eq!(text.max_length, Some(64));
                assert!(!text.multiline);
            }
            PendingInputKind::Choice(_) => panic!("expected text input kind"),
        }
    }

    #[tokio::test]
    async fn request_user_input_provider_requires_choice_payload_for_choice_kind() {
        let provider = RequestUserInputProvider::new();
        let result = provider
            .execute(
                "request_user_input",
                r#"{"prompt":"Pick one","kind":"choice"}"#,
                None,
                None,
            )
            .await;

        let Err(error) = result else {
            panic!("expected choice payload validation error");
        };
        assert!(error.to_string().contains("requires a `choice` object"));
    }

    #[tokio::test]
    async fn request_user_input_provider_rejects_telegram_unsupported_choice_option_counts() {
        let provider = RequestUserInputProvider::new();

        let too_few = provider
            .execute(
                "request_user_input",
                r#"{"prompt":"Pick one","kind":"choice","choice":{"options":["only"],"allow_multiple":false,"min_choices":1,"max_choices":1}}"#,
                None,
                None,
            )
            .await;
        let Err(too_few_error) = too_few else {
            panic!("expected option lower-bound validation error");
        };
        assert!(too_few_error
            .to_string()
            .contains("must contain at least 2 options"));

        let too_many = provider
            .execute(
                "request_user_input",
                r#"{"prompt":"Pick several","kind":"choice","choice":{"options":["1","2","3","4","5","6","7","8","9","10","11"],"allow_multiple":true,"min_choices":1,"max_choices":3}}"#,
                None,
                None,
            )
            .await;
        let Err(too_many_error) = too_many else {
            panic!("expected option upper-bound validation error");
        };
        assert!(too_many_error
            .to_string()
            .contains("must contain at most 10 options"));
    }
}
