//! Kokoro TTS HTTP client
//!
//! Handles communication with the local Kokoro TTS API server.

use super::types::{TtsConfig, TtsRequest};
use anyhow::{Context, Result};
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// HTTP client for Kokoro TTS API
#[derive(Debug, Clone)]
pub struct KokoroClient {
    client: reqwest::Client,
    config: TtsConfig,
}

impl KokoroClient {
    /// Create a new Kokoro client with the given configuration
    #[must_use]
    pub fn new(config: TtsConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("Failed to build HTTP client");

        Self { client, config }
    }

    /// Create client from environment variables
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(TtsConfig::from_env())
    }

    /// Get the API base URL
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }

    /// Synthesize speech from text
    ///
    /// # Arguments
    ///
    /// * `request` - TTS request with text, voice, format, and speed
    ///
    /// # Returns
    ///
    /// Raw audio bytes in the requested format
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Connection to TTS server fails
    /// - Server returns non-200 status
    /// - Response cannot be read
    pub async fn synthesize(&self, request: &TtsRequest) -> Result<Vec<u8>> {
        let endpoint = format!("{}/v1/audio/speech", self.config.base_url);

        debug!(
            endpoint = %endpoint,
            text_len = request.text.len(),
            voice = %request.voice,
            format = %request.format,
            speed = request.speed,
            "Sending TTS synthesis request"
        );

        // Check for non-ASCII text and warn
        if let Some(warning) = request.validate_english() {
            warn!(%warning, "Non-ASCII text detected in TTS request");
        }

        let response = self
            .client
            .post(&endpoint)
            .json(request)
            .send()
            .await
            .with_context(|| {
                format!(
                    "Failed to connect to Kokoro TTS server at {}. \
                     Is the service running?",
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
                "Kokoro TTS API returned error"
            );

            anyhow::bail!(
                "TTS synthesis failed: HTTP {} - {}",
                status,
                error_text
            );
        }

        let audio_bytes = response
            .bytes()
            .await
            .context("Failed to read TTS response body")?;

        info!(
            bytes_received = audio_bytes.len(),
            text_len = request.text.len(),
            "TTS synthesis successful"
        );

        Ok(audio_bytes.to_vec())
    }

    /// Quick health check for the TTS server
    ///
    /// # Returns
    ///
    /// `true` if server responds with success
    pub async fn health_check(&self) -> bool {
        // Kokoro doesn't have a dedicated health endpoint,
        // so we try a minimal synthesis request
        let test_request = TtsRequest::new("test");

        match self.synthesize(&test_request).await {
            Ok(_) => {
                debug!("Kokoro TTS health check passed");
                true
            }
            Err(e) => {
                warn!(error = %e, "Kokoro TTS health check failed");
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
        let config = TtsConfig::default();
        let client = KokoroClient::new(config);
        assert_eq!(client.base_url(), "http://127.0.0.1:8000");
    }

    #[test]
    fn client_from_env() {
        // Should use defaults when env vars not set
        let client = KokoroClient::from_env();
        assert_eq!(client.base_url(), "http://127.0.0.1:8000");
    }
}
