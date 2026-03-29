//! HTTP client for Mistral AI API

use crate::llm::support::http_utils;
use async_openai::{config::OpenAIConfig, Client};
use reqwest::Client as HttpClient;

/// Creates a new OpenAI-compatible client for Mistral
pub fn create_openai_client(api_key: &str) -> Client<OpenAIConfig> {
    let config = OpenAIConfig::new()
        .with_api_key(api_key.to_string())
        .with_api_base("https://api.mistral.ai/v1");
    Client::with_config(config)
}

/// Creates a new HTTP client for Mistral API
pub fn create_http_client() -> HttpClient {
    http_utils::create_http_client()
}
