use crate::llm::support::http::create_http_client_builder;
use crate::llm::LlmError;
use gemini_rust::{ClientError as GeminiClientError, Gemini, GeminiBuilder, Model};
use reqwest::StatusCode;

use super::GeminiProvider;

impl GeminiProvider {
    pub(super) fn sdk_client(&self, model_id: &str) -> Result<Gemini, LlmError> {
        GeminiBuilder::new(self.api_key.clone())
            .with_model(Self::sdk_model(model_id))
            .with_http_client(create_http_client_builder())
            .build()
            .map_err(Self::map_sdk_error)
    }

    pub(super) fn sdk_model(model_id: &str) -> Model {
        let normalized = if model_id.starts_with("models/") {
            model_id.to_string()
        } else {
            format!("models/{model_id}")
        };

        Model::Custom(normalized)
    }

    pub(super) fn map_sdk_error(error: GeminiClientError) -> LlmError {
        match error {
            GeminiClientError::BadResponse { code, description } => {
                let message = description.unwrap_or_else(|| "Gemini request failed".to_string());
                if code == StatusCode::TOO_MANY_REQUESTS.as_u16() {
                    LlmError::RateLimit {
                        wait_secs: None,
                        message,
                    }
                } else {
                    LlmError::ApiError(format!("Gemini API error [{code}]: {message}"))
                }
            }
            GeminiClientError::PerformRequest { source, .. }
            | GeminiClientError::PerformRequestNew { source } => {
                LlmError::NetworkError(source.to_string())
            }
            GeminiClientError::Io { source } => LlmError::NetworkError(source.to_string()),
            GeminiClientError::Deserialize { source } => LlmError::JsonError(source.to_string()),
            GeminiClientError::DecodeResponse { source } => LlmError::JsonError(source.to_string()),
            GeminiClientError::InvalidApiKey { source } => {
                LlmError::ApiError(format!("Invalid Gemini API key: {source}"))
            }
            GeminiClientError::ConstructUrl { source, suffix } => LlmError::ApiError(format!(
                "Failed to construct Gemini URL for {suffix}: {source}"
            )),
            GeminiClientError::MissingResponseHeader { header } => {
                LlmError::ApiError(format!("Gemini response missing header: {header}"))
            }
            GeminiClientError::BadPart { source } => LlmError::NetworkError(source.to_string()),
            GeminiClientError::UrlParse { source } => {
                LlmError::ApiError(format!("Failed to parse Gemini URL: {source}"))
            }
            GeminiClientError::OperationTimeout { name } => {
                LlmError::NetworkError(format!("Gemini operation timed out: {name}"))
            }
            GeminiClientError::OperationFailed {
                name,
                code,
                message,
            } => LlmError::ApiError(format!(
                "Gemini operation failed ({name}, code {code}): {message}"
            )),
            GeminiClientError::InvalidResourceName { name } => {
                LlmError::ApiError(format!("Invalid Gemini resource name: {name}"))
            }
        }
    }
}
