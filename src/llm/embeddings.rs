//! OpenAI-compatible embedding provider.

use super::{http_utils, LlmError};
use serde::Deserialize;

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

/// Universal OpenAI-compatible embedding provider.
pub struct EmbeddingProvider {
    http_client: reqwest::Client,
    api_key: String,
    api_base: String,
}

impl EmbeddingProvider {
    /// Create a new embedding provider instance.
    #[must_use]
    pub fn new(api_key: String, api_base: String) -> Self {
        Self {
            http_client: http_utils::create_http_client(),
            api_key,
            api_base,
        }
    }

    /// Generate embedding vector for given text using the specified model.
    pub async fn generate(&self, text: &str, model: &str) -> Result<Vec<f32>, LlmError> {
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

/// Get API base URL for known embedding providers.
pub fn get_api_base(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "mistral" => Some("https://api.mistral.ai/v1"),
        "openrouter" => Some("https://openrouter.ai/api/v1"),
        _ => None,
    }
}
