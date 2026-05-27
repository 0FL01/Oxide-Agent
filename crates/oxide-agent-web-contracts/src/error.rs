use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ErrorEnvelope {
    pub error: ApiError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    Forbidden,
    NotFound,
    ValidationError,
    InvalidCredentials,
    RegistrationDisabled,
    BootstrapUnavailable,
    CsrfRequired,
    CsrfInvalid,
    SessionBusy,
    TaskWaitingForUserInput,
    TaskActive,
    TaskNotRunning,
    Conflict,
    RateLimited,
    BackendUnavailable,
    Internal,
}

impl ErrorEnvelope {
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            error: ApiError {
                code,
                message: message.into(),
                retryable,
                details: None,
            },
        }
    }

    #[must_use]
    pub fn with_details(mut self, details: Value) -> Self {
        self.error.details = Some(details);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{ErrorCode, ErrorEnvelope};

    #[test]
    fn error_envelope_uses_prd_shape() {
        let envelope = ErrorEnvelope::new(
            ErrorCode::TaskWaitingForUserInput,
            "The current task is waiting for user input.",
            false,
        )
        .with_details(serde_json::json!({ "task_id": "task-1" }));

        let value = serde_json::to_value(envelope).expect("error envelope serializes");
        assert_eq!(value["error"]["code"], "task_waiting_for_user_input");
        assert_eq!(value["error"]["retryable"], false);
        assert_eq!(value["error"]["details"]["task_id"], "task-1");
    }
}
