//! Piper TTS Tool Provider.

use super::client::PiperClient;
use super::types::{PiperTtsConfig, TextToSpeechRuArgs};
use crate::agent::progress::AgentEvent;
use crate::agent::provider::ToolProvider;
use crate::agent::providers::file_delivery::{
    deliver_file_via_progress, FileDeliveryRequest, FileDeliveryStatus,
};
use crate::llm::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use tracing::{debug, error, info, instrument, warn};

/// Piper TTS provider.
#[derive(Debug)]
pub struct PiperTtsProvider {
    client: PiperClient,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
}

impl PiperTtsProvider {
    /// Create a new Piper TTS provider.
    #[must_use]
    pub fn new(config: PiperTtsConfig) -> Self {
        Self {
            client: PiperClient::new(config),
            progress_tx: None,
        }
    }

    /// Create provider from environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(PiperTtsConfig::from_env())
    }

    /// Create provider from explicit configuration.
    #[must_use]
    pub fn from_config(config: PiperTtsConfig) -> Self {
        Self::new(config)
    }

    /// Get the base URL of the TTS server.
    #[must_use]
    pub fn base_url(&self) -> &str {
        self.client.base_url()
    }

    /// Set the progress channel for sending files.
    #[must_use]
    pub fn with_progress_tx(mut self, tx: tokio::sync::mpsc::Sender<AgentEvent>) -> Self {
        self.progress_tx = Some(tx);
        self
    }

    /// Execute Russian text-to-speech synthesis and send to user.
    #[instrument(skip(self, progress_tx), level = "debug")]
    async fn execute_text_to_speech_ru(
        &self,
        args: TextToSpeechRuArgs,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<String> {
        debug!("Parsing Piper TTS arguments");

        let config = PiperTtsConfig::from_env();
        let request = match args.to_request(&config) {
            Ok(req) => req,
            Err(error) => {
                return Ok(format!("Invalid Piper TTS parameters: {error}"));
            }
        };

        info!(
            text_len = request.text.len(),
            voice = %request.voice,
            format = %request.format,
            speed = request.speed,
            noise_scale = request.noise_scale,
            noise_w_scale = request.noise_w_scale,
            sentence_silence = request.sentence_silence,
            "Generating Russian speech"
        );

        let audio_bytes = match self.client.synthesize(&request).await {
            Ok(bytes) => bytes,
            Err(error) => {
                error!(error = %error, "Piper TTS synthesis failed");
                return Ok(format!("Failed to generate Russian speech: {error}"));
            }
        };

        let file_name = format!("speech.{}", request.format);

        if progress_tx.is_some() {
            debug!(file_name = %file_name, bytes = audio_bytes.len(), "Sending Russian voice message");

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
                    info!("Russian voice message delivered successfully");
                    Ok(format!(
                        "Russian voice message sent successfully. Format: {}, Voice: {}, Duration: ~{:.1}s",
                        request.format,
                        request.voice,
                        estimate_duration(&request.text, request.speed)
                    ))
                }
                FileDeliveryStatus::DeliveryFailed(error) => {
                    error!(error = %error, "Russian voice message delivery failed");
                    Ok(format!("Russian voice message delivery failed: {error}"))
                }
                FileDeliveryStatus::ConfirmationChannelClosed => {
                    warn!("Russian voice message confirmation channel closed");
                    Ok("Russian voice message sent but confirmation channel closed.".to_string())
                }
                FileDeliveryStatus::TimedOut => {
                    warn!("Russian voice message delivery timed out after 120s");
                    Ok("Russian voice message delivery timed out. The audio may still be delivered.".to_string())
                }
                FileDeliveryStatus::QueueUnavailable(error) => {
                    error!(error = %error, "Failed to queue Russian voice message");
                    Ok(format!("Failed to queue Russian voice message: {error}"))
                }
                FileDeliveryStatus::EmptyContent => {
                    Ok("Russian voice message generation returned empty audio.".to_string())
                }
            }
        } else {
            warn!("No progress channel available for sending Russian voice message");
            Ok(format!(
                "Russian speech generated ({} bytes) but cannot send: no progress channel available",
                audio_bytes.len()
            ))
        }
    }
}

/// Estimate audio duration based on word count and speed.
fn estimate_duration(text: &str, speed: f32) -> f32 {
    let words: Vec<&str> = text.split_whitespace().collect();
    let word_count = words.len() as f32;
    let base_duration = word_count / 2.5;
    base_duration / speed
}

#[async_trait]
impl ToolProvider for PiperTtsProvider {
    fn name(&self) -> &'static str {
        "piper_tts"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "text_to_speech_ru".to_string(),
            description: concat!(
                "Convert Russian text to speech with the local Piper TTS server and send it to the user as a voice message. ",
                "Use this for Russian voice replies. Defaults are tuned for natural delivery with the `ruslan` voice."
            )
            .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Russian text to convert to speech."
                    },
                    "voice": {
                        "type": "string",
                        "enum": ["denis", "dmitri", "irina", "ruslan"],
                        "description": "Voice alias. Default: ruslan."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["ogg", "mp3", "wav"],
                        "description": "Audio format. Default: ogg."
                    },
                    "length_scale": {
                        "type": "number",
                        "exclusiveMinimum": 0,
                        "description": "Optional duration scale. Smaller values speak faster."
                    },
                    "speed": {
                        "type": "number",
                        "exclusiveMinimum": 0,
                        "description": "Speech speed. Default natural preset: 0.9."
                    },
                    "noise_scale": {
                        "type": "number",
                        "exclusiveMinimum": 0,
                        "description": "Speech variability. Default natural preset: 0.62."
                    },
                    "noise_w_scale": {
                        "type": "number",
                        "exclusiveMinimum": 0,
                        "description": "Word-level variability. Default natural preset: 0.78."
                    },
                    "volume": {
                        "type": "number",
                        "exclusiveMinimum": 0,
                        "description": "Output volume. Default: 1.0."
                    },
                    "normalize_audio": {
                        "type": "boolean",
                        "description": "Whether to normalize audio. Default: true."
                    },
                    "sentence_silence": {
                        "type": "number",
                        "minimum": 0,
                        "maximum": 2,
                        "description": "Pause between sentences in seconds. Default natural preset: 0.10."
                    }
                },
                "required": ["text"]
            }),
        }]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(tool_name, "text_to_speech_ru")
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        debug!(tool = tool_name, "Executing Piper TTS tool");

        match tool_name {
            "text_to_speech_ru" => {
                let args: TextToSpeechRuArgs = match serde_json::from_str(arguments) {
                    Ok(args) => args,
                    Err(error) => {
                        return Ok(format!("Invalid arguments: {error}"));
                    }
                };

                self.execute_text_to_speech_ru(args, progress_tx).await
            }
            _ => anyhow::bail!("Unknown Piper TTS tool: {tool_name}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_creation() {
        let provider = PiperTtsProvider::from_env();
        assert_eq!(provider.name(), "piper_tts");
    }

    #[test]
    fn provider_tools() {
        let provider = PiperTtsProvider::from_env();
        let tools = provider.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "text_to_speech_ru");
    }

    #[test]
    fn can_handle_check() {
        let provider = PiperTtsProvider::from_env();
        assert!(provider.can_handle("text_to_speech_ru"));
        assert!(!provider.can_handle("other_tool"));
    }

    #[test]
    fn duration_estimation() {
        let duration = estimate_duration("Это тестовое предложение из пяти слов", 1.0);
        assert!(duration > 2.0 && duration < 4.0);

        let duration = estimate_duration("Это тестовое предложение из пяти слов", 2.0);
        assert!(duration > 1.0 && duration < 2.0);
    }
}
