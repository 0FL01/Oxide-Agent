//! Silero TTS HTTP client.

use super::types::{SileroTtsConfig, SileroTtsRequest};
use anyhow::{Context, Result};
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// HTTP client for Silero TTS API.
#[derive(Debug, Clone)]
pub struct SileroClient {
    client: reqwest::Client,
    config: SileroTtsConfig,
}

impl SileroClient {
    /// Create a new Silero client with the given configuration.
    #[must_use]
    pub fn new(config: SileroTtsConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("Failed to build HTTP client");

        Self { client, config }
    }

    /// Create client from environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(SileroTtsConfig::from_env())
    }

    /// Get the API base URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }

    /// Synthesize speech from Russian text.
    pub async fn synthesize(&self, request: &SileroTtsRequest) -> Result<Vec<u8>> {
        let endpoint = format!("{}/v1/audio/speech", self.config.base_url);

        debug!(
            endpoint = %endpoint,
            text_len = request.text.len(),
            speaker = %request.speaker,
            format = %request.format,
            sample_rate = request.sample_rate,
            ssml = request.ssml,
            "Sending Silero TTS synthesis request"
        );

        let response = self
            .client
            .post(&endpoint)
            .json(request)
            .send()
            .await
            .with_context(|| {
                format!(
                    "Failed to connect to Silero TTS server at {}. Is the service running?",
                    self.config.base_url
                )
            })?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            error!(
                status = %status,
                error = %error_text,
                "Silero TTS API returned error"
            );

            anyhow::bail!(
                "Silero TTS synthesis failed: HTTP {} - {}",
                status,
                error_text
            );
        }

        let audio_bytes = response
            .bytes()
            .await
            .context("Failed to read Silero TTS response body")?;

        info!(
            bytes_received = audio_bytes.len(),
            text_len = request.text.len(),
            "Silero TTS synthesis successful"
        );

        Ok(audio_bytes.to_vec())
    }

    /// Quick health check for the Silero server.
    pub async fn health_check(&self) -> bool {
        let endpoint = format!("{}/healthz", self.config.base_url);

        match self.client.get(&endpoint).send().await {
            Ok(response) if response.status().is_success() => {
                debug!(endpoint = %endpoint, "Silero TTS health check passed");
                true
            }
            Ok(response) => {
                warn!(status = %response.status(), endpoint = %endpoint, "Silero TTS health check failed");
                false
            }
            Err(error) => {
                warn!(error = %error, endpoint = %endpoint, "Silero TTS health check failed");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_creation() {
        let config = SileroTtsConfig::default();
        let client = SileroClient::new(config);
        assert_eq!(client.base_url(), "http://127.0.0.1:8001");
    }

    #[test]
    fn client_from_env() {
        let client = SileroClient::from_env();
        assert_eq!(client.base_url(), "http://127.0.0.1:8001");
    }
}
