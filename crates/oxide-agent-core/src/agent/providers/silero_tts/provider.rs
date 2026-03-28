//! Silero TTS Tool Provider.
//!
//! Implements `ToolProvider` trait for Russian text-to-speech synthesis using Silero.
//! Sends generated audio as voice messages via the progress channel.
//! Supports SSML for enhanced speech control.

use super::client::SileroClient;
use super::types::{SileroTtsConfig, TextToSpeechRuArgs};
use crate::agent::progress::AgentEvent;
use crate::agent::progress::FileDeliveryKind;
use crate::agent::provider::ToolProvider;
use crate::agent::providers::file_delivery::{
    deliver_file_via_progress, FileDeliveryRequest, FileDeliveryStatus,
};
use crate::llm::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use tracing::{debug, error, info, instrument, warn};

/// Silero TTS provider.
#[derive(Debug)]
pub struct SileroTtsProvider {
    client: SileroClient,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
}

impl SileroTtsProvider {
    /// Create a new Silero TTS provider.
    #[must_use]
    pub fn new(config: SileroTtsConfig) -> Self {
        Self {
            client: SileroClient::new(config),
            progress_tx: None,
        }
    }

    /// Create provider from environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(SileroTtsConfig::from_env())
    }

    /// Create provider from explicit configuration.
    #[must_use]
    pub fn from_config(config: SileroTtsConfig) -> Self {
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
        debug!("Parsing Silero TTS arguments");

        let config = SileroTtsConfig::from_env();
        let request = match args.to_request(&config) {
            Ok(req) => req,
            Err(error) => {
                return Ok(format!("Invalid Silero TTS parameters: {error}"));
            }
        };

        // Check for Arabic numerals which Silero cannot pronounce
        if request.text.chars().any(|c| c.is_ascii_digit()) {
            return Ok(
                "ERROR: Text contains Arabic numerals (0-9) which Silero cannot pronounce. "
                    .to_string()
                    + "Please rewrite the text using Russian words for numbers.\n\n"
                    + "Examples:\n"
                    + "- '42' -> 'сорок два'\n"
                    + "- '2024' -> 'две тысячи двадцать четыре'\n"
                    + "- '15:30' -> 'пятнадцать часов тридцать минут'\n"
                    + "- 'Room 404' -> 'комната четыреста четыре'\n"
                    + "- 'v2.5' -> 'версия два точка пять'\n\n"
                    + "Rewrite your text with all numbers spelled out and try again.",
            );
        }

        info!(
            text_len = request.text.len(),
            speaker = %request.speaker,
            format = %request.format,
            sample_rate = request.sample_rate,
            ssml = request.ssml,
            "Generating Russian speech with Silero"
        );

        let audio_bytes = match self.client.synthesize(&request).await {
            Ok(bytes) => bytes,
            Err(error) => {
                error!(error = %error, "Silero TTS synthesis failed");
                return Ok(format!("Failed to generate Russian speech: {error}"));
            }
        };

        let file_name = format!("speech.{}", request.format);

        if progress_tx.is_some() {
            debug!(
                file_name = %file_name,
                bytes = audio_bytes.len(),
                "Sending Russian voice message"
            );

            let report = deliver_file_via_progress(
                progress_tx,
                FileDeliveryRequest {
                    kind: FileDeliveryKind::VoiceNote,
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
                        "Russian voice message sent successfully. Format: {}, Speaker: {}, Duration: ~{:.1}s",
                        request.format,
                        request.speaker,
                        estimate_duration(&request.text)
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

/// Estimate audio duration based on word count.
/// Average Russian speaking rate: ~150 words per minute = 2.5 words per second.
fn estimate_duration(text: &str) -> f32 {
    let words: Vec<&str> = text.split_whitespace().collect();
    let word_count = words.len() as f32;
    word_count / 2.5
}

#[async_trait]
impl ToolProvider for SileroTtsProvider {
    fn name(&self) -> &'static str {
        "silero_tts"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "text_to_speech_ru".to_string(),
            description: concat!(
                "Convert Russian text to speech with the Silero TTS server and send it to the user as a voice message. ",
                "CRITICAL: Silero cannot pronounce Arabic numerals (0-9). ",
                "You MUST convert ALL numbers to Russian words before calling. ",
                "Examples: 42 -> 'сорок два', 2024 -> 'две тысячи двадцать четыре', 15:30 -> 'пятнадцать часов тридцать минут'. ",
                "Never use digits like '1', '2', '33' - always spell them out. ",
                "Supports SSML markup for enhanced speech control (pauses, pitch, rate). ",
                "Default speaker is 'baya' (best quality). Use SSML for natural-sounding speech with proper pauses."
            )
            .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Russian text to convert to speech. Can be plain text or SSML markup."
                    },
                    "speaker": {
                        "type": "string",
                        "enum": ["aidar", "baya", "kseniya", "xenia"],
                        "description": "Speaker voice. Default: baya (recommended). Other options: aidar (male), kseniya, xenia"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["ogg", "wav"],
                        "description": "Audio format. Default: ogg (best for Telegram voice messages)"
                    },
                    "sample_rate": {
                        "type": "integer",
                        "enum": [8000, 24000, 48000],
                        "description": "Sample rate in Hz. Default: 48000 (best quality). Lower values produce smaller files"
                    },
                    "ssml": {
                        "type": "boolean",
                        "description": "Whether the text is SSML markup. Default: false. Set to true when using SSML tags like <speak>, <break>, <prosody>"
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
        debug!(tool = tool_name, "Executing Silero TTS tool");

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
            _ => anyhow::bail!("Unknown Silero TTS tool: {tool_name}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_creation() {
        let provider = SileroTtsProvider::from_env();
        assert_eq!(provider.name(), "silero_tts");
    }

    #[test]
    fn provider_tools() {
        let provider = SileroTtsProvider::from_env();
        let tools = provider.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "text_to_speech_ru");
    }

    #[test]
    fn can_handle_check() {
        let provider = SileroTtsProvider::from_env();
        assert!(provider.can_handle("text_to_speech_ru"));
        assert!(!provider.can_handle("other_tool"));
    }

    #[test]
    fn duration_estimation() {
        // ~10 words at normal speed = ~4 seconds
        let duration = estimate_duration("Это тестовое предложение из десяти слов");
        assert!(duration > 3.0 && duration < 5.0);

        // Empty text
        let duration = estimate_duration("");
        assert_eq!(duration, 0.0);
    }
}
