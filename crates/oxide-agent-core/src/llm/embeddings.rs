//! OpenAI-compatible embedding provider.

use super::{http, LlmError};
use crate::llm::support::http::create_http_client_builder;
use gemini_rust::{GeminiBuilder, Model, TaskType};
use serde::Deserialize;

use super::support::http::APP_USER_AGENT;

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

/// Embedding task type supported by configured providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingTaskType {
    /// Embedding optimized for indexed documents.
    RetrievalDocument,
    /// Embedding optimized for search queries.
    RetrievalQuery,
}

pub(crate) struct OpenAiCompatibleEmbeddingProvider {
    http_client: reqwest::Client,
    api_key: String,
    api_base: String,
}

impl OpenAiCompatibleEmbeddingProvider {
    #[must_use]
    fn new(api_key: String, api_base: String) -> Self {
        Self {
            http_client: http::create_http_client(),
            api_key,
            api_base,
        }
    }

    async fn generate(&self, text: &str, model: &str) -> Result<Vec<f32>, LlmError> {
        let url = format!("{}/embeddings", self.api_base);

        let body = serde_json::json!({
            "model": model,
            "input": text
        });

        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("User-Agent", APP_USER_AGENT)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!(
                "Embedding API error: {status} - {error_text}"
            )));
        }

        let parsed: EmbeddingResponse = response
            .json()
            .await
            .map_err(|e| LlmError::JsonError(e.to_string()))?;

        parsed
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| LlmError::ApiError("Empty embedding response".to_string()))
    }
}

/// Universal embedding provider wrapper.
pub(crate) enum EmbeddingProvider {
    /// OpenAI-compatible embedding endpoint.
    OpenAiCompatible(OpenAiCompatibleEmbeddingProvider),
    /// Gemini embedding endpoint.
    Gemini {
        /// API key used for Gemini embedding requests.
        api_key: String,
    },
}

impl EmbeddingProvider {
    /// Create a new OpenAI-compatible embedding provider instance.
    #[must_use]
    pub fn new_openai_compatible(api_key: String, api_base: String) -> Self {
        Self::OpenAiCompatible(OpenAiCompatibleEmbeddingProvider::new(api_key, api_base))
    }

    /// Create a new Gemini embedding provider instance.
    #[must_use]
    pub fn new_gemini(api_key: String) -> Self {
        Self::Gemini { api_key }
    }

    /// Generate embedding vector for given text using the specified model.
    pub async fn generate(
        &self,
        text: &str,
        model: &str,
        task_type: Option<EmbeddingTaskType>,
        title: Option<&str>,
    ) -> Result<Vec<f32>, LlmError> {
        match self {
            Self::OpenAiCompatible(provider) => provider.generate(text, model).await,
            Self::Gemini { api_key } => {
                let client = GeminiBuilder::new(api_key.clone())
                    .with_model(Model::Custom(normalize_gemini_model(model)))
                    .with_http_client(create_http_client_builder())
                    .build()
                    .map_err(map_gemini_error)?;
                let mut builder = client.embed_content().with_text(text.to_string());
                if let Some(task_type) = task_type {
                    builder = builder.with_task_type(match task_type {
                        EmbeddingTaskType::RetrievalDocument => TaskType::RetrievalDocument,
                        EmbeddingTaskType::RetrievalQuery => TaskType::RetrievalQuery,
                    });
                }
                if let Some(title) = title.filter(|value| !value.is_empty()) {
                    builder = builder.with_title(title.to_string());
                }
                let response = builder.execute().await.map_err(map_gemini_error)?;
                Ok(response.embedding.values)
            }
        }
    }

    /// Probe the embedding dimension by generating a test embedding.
    pub async fn probe_dimension(&self, model: &str) -> Option<usize> {
        self.generate("test", model, None, None)
            .await
            .ok()
            .map(|v| v.len())
    }
}

/// Get API base URL for known embedding providers.
pub fn get_api_base(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "mistral" => Some("https://api.mistral.ai/v1"),
        "openrouter" => Some("https://openrouter.ai/api/v1"),
        _ => None,
    }
}

fn normalize_gemini_model(model: &str) -> String {
    if model.starts_with("models/") {
        model.to_string()
    } else {
        format!("models/{model}")
    }
}

fn map_gemini_error(error: gemini_rust::ClientError) -> LlmError {
    use gemini_rust::ClientError;

    match error {
        ClientError::BadResponse { code, description } => LlmError::ApiError(format!(
            "Gemini embedding API error [{code}]: {}",
            description.unwrap_or_else(|| "request failed".to_string())
        )),
        ClientError::PerformRequest { source, .. } | ClientError::PerformRequestNew { source } => {
            LlmError::NetworkError(source.to_string())
        }
        ClientError::Io { source } => LlmError::NetworkError(source.to_string()),
        ClientError::Deserialize { source } => LlmError::JsonError(source.to_string()),
        ClientError::DecodeResponse { source } => LlmError::JsonError(source.to_string()),
        ClientError::InvalidApiKey { source } => {
            LlmError::ApiError(format!("Invalid Gemini API key: {source}"))
        }
        ClientError::ConstructUrl { source, suffix } => LlmError::ApiError(format!(
            "Failed to construct Gemini URL for {suffix}: {source}"
        )),
        ClientError::MissingResponseHeader { header } => {
            LlmError::ApiError(format!("Gemini response missing header: {header}"))
        }
        ClientError::BadPart { source } => LlmError::NetworkError(source.to_string()),
        ClientError::UrlParse { source } => {
            LlmError::ApiError(format!("Failed to parse Gemini URL: {source}"))
        }
        ClientError::OperationTimeout { name } => {
            LlmError::NetworkError(format!("Gemini operation timed out: {name}"))
        }
        ClientError::OperationFailed {
            name,
            code,
            message,
        } => LlmError::ApiError(format!(
            "Gemini operation failed ({name}, code {code}): {message}"
        )),
        ClientError::InvalidResourceName { name } => {
            LlmError::ApiError(format!("Invalid Gemini resource name: {name}"))
        }
    }
}
