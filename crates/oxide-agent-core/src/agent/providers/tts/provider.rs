//! Kokoro TTS Tool Provider
//!
//! Implements `ToolProvider` trait for text-to-speech synthesis.
//! Sends generated audio as voice messages via the progress channel.

use super::client::KokoroClient;
use super::types::{TextToSpeechArgs, TtsConfig};
use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use tracing::{debug, error, info, instrument, warn};

use crate::agent::progress::AgentEvent;
use crate::agent::providers::file_delivery::{
    deliver_file_via_progress, FileDeliveryRequest, FileDeliveryStatus,
};

/// Kokoro TTS provider
#[derive(Debug)]
pub struct KokoroTtsProvider {
    client: KokoroClient,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
}

impl KokoroTtsProvider {
    /// Create a new Kokoro TTS provider
    #[must_use]
    pub fn new(config: TtsConfig) -> Self {
        Self {
            client: KokoroClient::new(config),
            progress_tx: None,
        }
    }

    /// Create provider from environment variables
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(TtsConfig::from_env())
    }

    /// Create provider from explicit configuration
    #[must_use]
    pub fn from_config(config: TtsConfig) -> Self {
        Self::new(config)
    }

    /// Get the base URL of the TTS server
    #[must_use]
    pub fn base_url(&self) -> &str {
        self.client.base_url()
    }

    /// Set the progress channel for sending files
    #[must_use]
    pub fn with_progress_tx(mut self, tx: tokio::sync::mpsc::Sender<AgentEvent>) -> Self {
        self.progress_tx = Some(tx);
        self
    }

    /// Execute text-to-speech synthesis and send to user
    ///
    /// # Errors
    ///
    /// Returns error if synthesis fails or file cannot be sent
    #[instrument(skip(self, progress_tx), level = "debug")]
    async fn execute_text_to_speech(
        &self,
        args: TextToSpeechArgs,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<String> {
        debug!("Parsing TTS arguments");

        let config = TtsConfig::from_env();
        let request = match args.to_request(&config) {
            Ok(req) => req,
            Err(e) => {
                return Ok(format!("Invalid TTS parameters: {e}"));
            }
        };

        info!(
            text_len = request.text.len(),
            voice = %request.voice,
            format = %request.format,
            "Generating speech"
        );

        // Synthesize audio
        let audio_bytes = match self.client.synthesize(&request).await {
            Ok(bytes) => bytes,
            Err(e) => {
                error!(error = %e, "TTS synthesis failed");
                return Ok(format!("Failed to generate speech: {e}"));
            }
        };

        let file_name = format!("speech.{}", request.format);

        // Send file to user via progress channel
        if progress_tx.is_some() {
            debug!(file_name = %file_name, bytes = audio_bytes.len(), "Sending voice message");

            let report = deliver_file_via_progress(
                progress_tx,
                FileDeliveryRequest {
                    file_name: file_name.clone(),
                    content: audio_bytes,
                    source_path: format!("/tmp/{file_name}"),
                },
            )
            .await;

            match report.status {
                FileDeliveryStatus::Delivered => {
                    info!("Voice message delivered successfully");
                    Ok(format!(
                        "Voice message sent successfully. \
                         Format: {}, Voice: {}, Duration: ~{:.1}s",
                        request.format,
                        request.voice,
                        estimate_duration(&request.text, request.speed)
                    ))
                }
                FileDeliveryStatus::DeliveryFailed(error) => {
                    error!(error = %error, "Voice message delivery failed");
                    Ok(format!("Voice message delivery failed: {error}"))
                }
                FileDeliveryStatus::ConfirmationChannelClosed => {
                    warn!("Voice message confirmation channel closed");
                    Ok("Voice message sent but confirmation channel closed.".to_string())
                }
                FileDeliveryStatus::TimedOut => {
                    warn!("Voice message delivery timed out after 120s");
                    Ok(
                        "Voice message delivery timed out. The audio may still be delivered."
                            .to_string(),
                    )
                }
                FileDeliveryStatus::QueueUnavailable(error) => {
                    error!(error = %error, "Failed to queue voice message");
                    Ok(format!("Failed to queue voice message: {error}"))
                }
                FileDeliveryStatus::EmptyContent => {
                    Ok("Voice message generation returned empty audio.".to_string())
                }
            }
        } else {
            warn!("No progress channel available for sending voice message");
            Ok(format!(
                "Speech generated ({} bytes) but cannot send: no progress channel available",
                audio_bytes.len()
            ))
        }
    }
}

/// Estimate audio duration based on word count and speed
fn estimate_duration(text: &str, speed: f32) -> f32 {
    // Average speaking rate: ~150 words per minute = 2.5 words per second
    let words: Vec<&str> = text.split_whitespace().collect();
    let word_count = words.len() as f32;
    let base_duration = word_count / 2.5;
    base_duration / speed
}

#[async_trait]
impl ToolProvider for KokoroTtsProvider {
    fn name(&self) -> &'static str {
        "kokoro_tts"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "text_to_speech".to_string(),
            description: concat!(
                "Convert text to speech and send as a voice message to the user. ",
                "IMPORTANT: Text must be in English only - the TTS server supports English language exclusively. ",
                "If the user's request is in another language, translate it to English first. ",
                "Best for providing spoken responses, explanations, or when the user requests voice output."
            )
            .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Text to convert to speech. MUST be in English only."
                    },
                    "voice": {
                        "type": "string",
                        "enum": ["af_bella", "af_aoede", "af_alloy", "af_heart"],
                        "description": "Voice to use. Default: af_heart (warm female). Options: af_bella (default female), af_aoede (alternative female), af_alloy (neutral), af_heart (warm female)"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["ogg", "mp3", "wav"],
                        "description": "Audio format. RECOMMENDED: 'ogg' (Opus codec, smallest size, native Telegram support). Fallback options: 'mp3' (wider compatibility), 'wav' (lossless, larger size). Default: 'ogg'"
                    },
                    "speed": {
                        "type": "number",
                        "minimum": 0.5,
                        "maximum": 2.0,
                        "description": "Speech speed multiplier. Default: 1.0. Range: 0.5 (slow) to 2.0 (fast)"
                    }
                },
                "required": ["text"]
            }),
        }]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(tool_name, "text_to_speech")
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        debug!(tool = tool_name, "Executing TTS tool");

        match tool_name {
            "text_to_speech" => {
                let args: TextToSpeechArgs = match serde_json::from_str(arguments) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(format!("Invalid arguments: {e}"));
                    }
                };

                self.execute_text_to_speech(args, progress_tx).await
            }
            _ => anyhow::bail!("Unknown TTS tool: {tool_name}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_creation() {
        let provider = KokoroTtsProvider::from_env();
        assert_eq!(provider.name(), "kokoro_tts");
    }

    #[test]
    fn provider_tools() {
        let provider = KokoroTtsProvider::from_env();
        let tools = provider.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "text_to_speech");
    }

    #[test]
    fn can_handle_check() {
        let provider = KokoroTtsProvider::from_env();
        assert!(provider.can_handle("text_to_speech"));
        assert!(!provider.can_handle("other_tool"));
    }

    #[test]
    fn duration_estimation() {
        // ~10 words at 1.0 speed = ~4 seconds
        let duration = estimate_duration("This is a test sentence with ten words total", 1.0);
        assert!(duration > 3.0 && duration < 5.0);

        // Same text at 2.0 speed = ~2 seconds
        let duration = estimate_duration("This is a test sentence with ten words total", 2.0);
        assert!(duration > 1.5 && duration < 2.5);
    }
}
