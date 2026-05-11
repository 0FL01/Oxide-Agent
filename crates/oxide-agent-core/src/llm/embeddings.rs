//! Embedding provider abstraction with dimension control and normalization.

use super::{http, LlmError};
use crate::config::EmbeddingPromptStyle;
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
    prompt_style: EmbeddingPromptStyle,
    query_prefix: Option<String>,
    document_prefix: Option<String>,
}

impl OpenAiCompatibleEmbeddingProvider {
    #[must_use]
    fn new(
        api_key: String,
        api_base: String,
        prompt_style: EmbeddingPromptStyle,
        query_prefix: Option<String>,
        document_prefix: Option<String>,
    ) -> Self {
        Self {
            http_client: http::create_http_client(),
            api_key,
            api_base,
            prompt_style,
            query_prefix,
            document_prefix,
        }
    }

    async fn generate(
        &self,
        text: &str,
        model: &str,
        task_type: Option<EmbeddingTaskType>,
        dimensions: Option<u32>,
    ) -> Result<Vec<f32>, LlmError> {
        let url = format!("{}/embeddings", self.api_base);
        let input = self.prepare_input(text, task_type);

        let mut body = serde_json::json!({
            "model": model,
            "input": input
        });
        if let Some(dim) = dimensions {
            body["dimensions"] = serde_json::json!(dim);
        }

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

    fn prepare_input(&self, text: &str, task_type: Option<EmbeddingTaskType>) -> String {
        match (&self.prompt_style, task_type) {
            (EmbeddingPromptStyle::None, _) | (_, None) => text.to_string(),
            (EmbeddingPromptStyle::User2, Some(EmbeddingTaskType::RetrievalQuery)) => {
                format!("search_query: {text}")
            }
            (EmbeddingPromptStyle::User2, Some(EmbeddingTaskType::RetrievalDocument)) => {
                format!("search_document: {text}")
            }
            (EmbeddingPromptStyle::E5, Some(EmbeddingTaskType::RetrievalQuery)) => {
                format!("query: {text}")
            }
            (EmbeddingPromptStyle::E5, Some(EmbeddingTaskType::RetrievalDocument)) => {
                format!("passage: {text}")
            }
            (EmbeddingPromptStyle::Custom, Some(EmbeddingTaskType::RetrievalQuery)) => {
                prefix_embedding_input(self.query_prefix.as_deref(), text)
            }
            (EmbeddingPromptStyle::Custom, Some(EmbeddingTaskType::RetrievalDocument)) => {
                prefix_embedding_input(self.document_prefix.as_deref(), text)
            }
        }
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
    pub fn new_openai_compatible(
        api_key: String,
        api_base: String,
        prompt_style: EmbeddingPromptStyle,
        query_prefix: Option<String>,
        document_prefix: Option<String>,
    ) -> Self {
        Self::OpenAiCompatible(OpenAiCompatibleEmbeddingProvider::new(
            api_key,
            api_base,
            prompt_style,
            query_prefix,
            document_prefix,
        ))
    }

    /// Create a new Gemini embedding provider instance.
    #[must_use]
    pub fn new_gemini(api_key: String) -> Self {
        Self::Gemini { api_key }
    }

    /// Generate embedding vector for given text using the specified model.
    ///
    /// When `dimensions` is set, the provider truncates the output vector to
    /// that size. For Gemini, this uses `output_dimensionality`; for
    /// OpenAI-compatible endpoints, the `dimensions` request field.
    /// Vectors shorter than 3072 dimensions are L2-normalized in-place
    /// (Gemini only auto-normalizes the full 3072-dim output).
    pub async fn generate(
        &self,
        text: &str,
        model: &str,
        task_type: Option<EmbeddingTaskType>,
        title: Option<&str>,
        dimensions: Option<u32>,
    ) -> Result<Vec<f32>, LlmError> {
        let mut vec = match self {
            Self::OpenAiCompatible(provider) => {
                provider
                    .generate(text, model, task_type, dimensions)
                    .await?
            }
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
                if let Some(dim) = dimensions {
                    builder = builder.with_output_dimensionality(dim as i32);
                }
                let response = builder.execute().await.map_err(map_gemini_error)?;
                response.embedding.values
            }
        };

        // Gemini only auto-normalizes at the native 3072 dimensions.
        // For truncated outputs (768, 1536, …) we must L2-normalize ourselves.
        if dimensions.is_some_and(|d| d < 3072) {
            l2_normalize(&mut vec);
        }

        Ok(vec)
    }

    /// Probe the embedding dimension by generating a test embedding.
    pub async fn probe_dimension(&self, model: &str) -> Option<usize> {
        self.generate("test", model, None, None, None)
            .await
            .ok()
            .map(|v| v.len())
    }
}

/// L2-normalize a vector in-place.
///
/// After truncation via `output_dimensionality` the resulting sub-vector is
/// **not** unit-length. Cosine similarity (the default distance metric for
/// pgvector `<=>`) assumes normalized inputs for correct ranking, so we
/// normalise explicitly.
fn l2_normalize(v: &mut [f32]) {
    let norm_sq: f32 = v.iter().map(|x| x * x).sum();
    if norm_sq > 0.0 {
        let inv = 1.0 / norm_sq.sqrt();
        for x in v.iter_mut() {
            *x *= inv;
        }
    }
}

fn prefix_embedding_input(prefix: Option<&str>, text: &str) -> String {
    match prefix.filter(|value| !value.is_empty()) {
        Some(prefix) => format!("{prefix}{text}"),
        None => text.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn openai_prompt_style_prefixes_user2_queries_and_documents() {
        let provider = OpenAiCompatibleEmbeddingProvider::new(
            "key".to_string(),
            "http://127.0.0.1:1/v1".to_string(),
            EmbeddingPromptStyle::User2,
            None,
            None,
        );

        assert_eq!(
            provider.prepare_input("find deploy fix", Some(EmbeddingTaskType::RetrievalQuery)),
            "search_query: find deploy fix"
        );
        assert_eq!(
            provider.prepare_input(
                "deploy fix procedure",
                Some(EmbeddingTaskType::RetrievalDocument)
            ),
            "search_document: deploy fix procedure"
        );
        assert_eq!(provider.prepare_input("plain", None), "plain");
    }

    #[test]
    fn openai_prompt_style_supports_custom_prefixes() {
        let provider = OpenAiCompatibleEmbeddingProvider::new(
            "key".to_string(),
            "http://127.0.0.1:1/v1".to_string(),
            EmbeddingPromptStyle::Custom,
            Some("q: ".to_string()),
            Some("d: ".to_string()),
        );

        assert_eq!(
            provider.prepare_input("hello", Some(EmbeddingTaskType::RetrievalQuery)),
            "q: hello"
        );
        assert_eq!(
            provider.prepare_input("hello", Some(EmbeddingTaskType::RetrievalDocument)),
            "d: hello"
        );
    }

    #[tokio::test]
    async fn openai_compatible_request_serializes_prefixed_input() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let addr = listener.local_addr().expect("local addr");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut header_bytes = Vec::new();
            let mut buffer = [0u8; 1024];
            let header_end;
            loop {
                let read = stream.read(&mut buffer).expect("read request");
                header_bytes.extend_from_slice(&buffer[..read]);
                if let Some(pos) = header_bytes
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                {
                    header_end = pos + 4;
                    break;
                }
            }
            let header_text = String::from_utf8_lossy(&header_bytes[..header_end]);
            let content_length = header_text
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().expect("content length"))
                })
                .expect("content-length header");

            let mut body = header_bytes[header_end..].to_vec();
            while body.len() < content_length {
                let read = stream.read(&mut buffer).expect("read request body");
                body.extend_from_slice(&buffer[..read]);
            }

            let response = r#"{"data":[{"embedding":[1.0,0.0]}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            )
            .expect("write response");

            serde_json::from_slice::<serde_json::Value>(&body).expect("valid json body")
        });

        let provider = OpenAiCompatibleEmbeddingProvider::new(
            "test-key".to_string(),
            format!("http://{addr}/v1"),
            EmbeddingPromptStyle::User2,
            None,
            None,
        );

        let embedding = provider
            .generate(
                "deploy fix",
                "user2-base",
                Some(EmbeddingTaskType::RetrievalQuery),
                Some(768),
            )
            .await
            .expect("embedding request succeeds");
        assert_eq!(embedding, vec![1.0, 0.0]);

        let body = server.join().expect("join server");
        assert_eq!(body["model"], "user2-base");
        assert_eq!(body["input"], "search_query: deploy fix");
        assert_eq!(body["dimensions"], 768);
    }
}
