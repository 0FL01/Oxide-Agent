//! Kokoro TTS Tool Provider
//!
//! Provides native typed runtime executors for English text-to-speech synthesis.
//! Sends generated audio as voice messages via the progress channel.

use super::client::KokoroClient;
use super::types::{TextToSpeechArgs, TtsConfig};
use crate::agent::providers::SandboxRuntime;
use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use crate::sandbox::{SandboxExec, SandboxFileOps, SandboxScope};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use shell_escape::escape;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

use crate::agent::progress::AgentEvent;
use crate::agent::progress::FileDeliveryKind;
use crate::agent::providers::file_delivery::{
    deliver_file_via_progress, FileDeliveryRequest, FileDeliveryStatus,
};

const TOOL_TEXT_TO_SPEECH_EN: &str = "text_to_speech_en";
const TOOL_TEXT_TO_SPEECH_EN_FILE: &str = "text_to_speech_en_file";

/// Kokoro TTS provider
pub struct KokoroTtsProvider {
    client: KokoroClient,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    fileops: Option<Arc<dyn SandboxFileOps>>,
    exec: Option<Arc<dyn SandboxExec>>,
}

impl KokoroTtsProvider {
    /// Create a new Kokoro TTS provider
    #[must_use]
    pub fn new(config: TtsConfig) -> Self {
        Self {
            client: KokoroClient::new(config),
            progress_tx: None,
            fileops: None,
            exec: None,
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

    /// Attach sandbox scope for file-writing workflows.
    #[must_use]
    pub fn with_sandbox_scope(self, scope: impl Into<SandboxScope>) -> Self {
        self.with_sandbox_runtime(Arc::new(SandboxRuntime::new(scope.into())))
    }

    /// Attach shared sandbox runtime for file-writing workflows.
    #[must_use]
    pub fn with_sandbox_runtime(self, runtime: Arc<SandboxRuntime>) -> Self {
        let fileops: Arc<dyn SandboxFileOps> = Arc::<SandboxRuntime>::clone(&runtime);
        let exec: Arc<dyn SandboxExec> = runtime;
        self.with_sandbox_backends(fileops, exec)
    }

    /// Attach narrow sandbox capabilities for file-writing workflows.
    #[must_use]
    pub fn with_sandbox_backends(
        mut self,
        fileops: Arc<dyn SandboxFileOps>,
        exec: Arc<dyn SandboxExec>,
    ) -> Self {
        self.fileops = Some(fileops);
        self.exec = Some(exec);
        self
    }

    /// Build native typed runtime executors for Kokoro TTS tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        let execution_lock = Arc::new(Mutex::new(()));
        Self::tool_definitions()
            .into_iter()
            .map(|spec| {
                Arc::new(KokoroTtsToolExecutor {
                    provider: Arc::clone(self),
                    name: ToolName::from(spec.name.clone()),
                    spec,
                    execution_lock: Arc::clone(&execution_lock),
                }) as Arc<dyn ToolExecutor>
            })
            .collect()
    }

    fn tool_definitions() -> Vec<ToolDefinition> {
        vec![
            Self::text_to_speech_tool(),
            Self::text_to_speech_file_tool(),
        ]
    }

    fn text_to_speech_tool() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_TEXT_TO_SPEECH_EN.to_string(),
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
        }
    }

    fn text_to_speech_file_tool() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_TEXT_TO_SPEECH_EN_FILE.to_string(),
            description: concat!(
                "Convert English text to speech and save the audio inside the sandbox for downstream tools such as ffmpeg. ",
                "Use this when you need a file path instead of immediate delivery to the user."
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
                        "description": "Voice to use. Default: af_heart."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["ogg", "mp3", "wav"],
                        "description": "Audio format. Use 'wav' when a downstream editor needs PCM audio."
                    },
                    "speed": {
                        "type": "number",
                        "minimum": 0.5,
                        "maximum": 2.0,
                        "description": "Speech speed multiplier."
                    },
                    "output_path": {
                        "type": "string",
                        "description": "Optional sandbox output path. Relative paths are placed under /workspace/. Defaults to /workspace/generated/..."
                    }
                },
                "required": ["text"]
            }),
        }
    }

    async fn execute_tool(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<String> {
        debug!(tool = tool_name, "Executing TTS tool");

        match tool_name {
            TOOL_TEXT_TO_SPEECH_EN => {
                let args: TextToSpeechArgs = match serde_json::from_str(arguments) {
                    Ok(args) => args,
                    Err(error) => {
                        return Ok(format!("Invalid arguments: {error}"));
                    }
                };

                self.execute_text_to_speech_en(args, progress_tx).await
            }
            TOOL_TEXT_TO_SPEECH_EN_FILE => {
                let args: TextToSpeechArgs = match serde_json::from_str(arguments) {
                    Ok(args) => args,
                    Err(error) => {
                        return Ok(format!("Invalid arguments: {error}"));
                    }
                };

                self.execute_text_to_speech_en_file(args).await
            }
            _ => anyhow::bail!("Unknown TTS tool: {tool_name}"),
        }
    }

    async fn write_audio_file(&self, path: &str, content: &[u8]) -> Result<()> {
        let exec = self
            .exec
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Sandbox exec is not configured for Kokoro TTS"))?;
        let fileops = self
            .fileops
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Sandbox fileops is not configured for Kokoro TTS"))?;

        ensure_parent_dir(exec, path).await?;
        fileops.write_file(path, content).await
    }

    /// Execute English text-to-speech synthesis and send to user
    ///
    /// # Errors
    ///
    /// Returns error if synthesis fails or file cannot be sent
    #[instrument(skip(self, progress_tx), level = "debug")]
    async fn execute_text_to_speech_en(
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
                    kind: FileDeliveryKind::VoiceNote,
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

    #[instrument(skip(self), level = "debug")]
    async fn execute_text_to_speech_en_file(&self, args: TextToSpeechArgs) -> Result<String> {
        debug!("Parsing TTS file arguments");

        let config = TtsConfig::from_env();
        let request = match args.to_request(&config) {
            Ok(req) => req,
            Err(error) => {
                return Ok(format!("Invalid TTS parameters: {error}"));
            }
        };

        let audio_bytes = match self.client.synthesize(&request).await {
            Ok(bytes) => bytes,
            Err(error) => {
                error!(error = %error, "TTS synthesis failed");
                return Ok(format!("Failed to generate speech: {error}"));
            }
        };

        let output_path =
            build_output_path(args.output_path.as_deref(), "speech_en", &request.format);
        self.write_audio_file(&output_path, &audio_bytes)
            .await
            .with_context(|| {
                format!("Failed to write speech file to sandbox path {output_path}")
            })?;

        Ok(serde_json::to_string(&json!({
            "ok": true,
            "path": output_path,
            "bytes": audio_bytes.len(),
            "format": request.format,
            "voice": request.voice,
            "duration_seconds_estimate": estimate_duration(&request.text, request.speed),
        }))?)
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

fn build_output_path(output_path: Option<&str>, prefix: &str, extension: &str) -> String {
    match output_path {
        Some(path) if path.starts_with('/') => path.to_string(),
        Some(path) => format!("/workspace/{path}"),
        None => format!(
            "/workspace/generated/{prefix}_{}.{}",
            Uuid::new_v4().simple(),
            extension
        ),
    }
}

async fn ensure_parent_dir(exec: &dyn SandboxExec, path: &str) -> Result<()> {
    let parent = Path::new(path).parent().map_or_else(
        || "/workspace".to_string(),
        |value| value.to_string_lossy().to_string(),
    );
    let command = format!("mkdir -p {}", escape(parent.as_str().into()));
    let result = exec.exec(&command, None).await?;
    if result.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "Failed to create output directory {parent}: {}",
            result.combined_output()
        )
    }
}

struct KokoroTtsToolExecutor {
    provider: Arc<KokoroTtsProvider>,
    name: ToolName,
    spec: ToolDefinition,
    execution_lock: Arc<Mutex<()>>,
}

#[async_trait]
impl ToolExecutor for KokoroTtsToolExecutor {
    fn name(&self) -> ToolName {
        self.name.clone()
    }

    fn spec(&self) -> ToolDefinition {
        self.spec.clone()
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let _guard = self.execution_lock.lock().await;
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            artifact_dir: invocation.execution_context.artifact_dir.clone(),
            ..ToolRuntimeConfig::default()
        });
        self.provider
            .execute_tool(
                self.name.as_str(),
                &invocation.raw_arguments,
                self.provider.progress_tx.as_ref(),
            )
            .await
            .map(|output| normalizer.success(&invocation, &output, ""))
            .map_err(|error| ToolRuntimeError::Failure(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolOutputStatus, ToolTimeoutConfig, TurnId,
    };
    use crate::llm::InvocationId;
    use chrono::Utc;
    use tokio_util::sync::CancellationToken;

    fn runtime_invocation(tool_name: &str, raw_arguments: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(77),
            turn_id: TurnId::from("turn-kokoro-tts"),
            batch_id: ToolBatchId::from("batch-kokoro-tts"),
            batch_index: 0,
            invocation_id: InvocationId::from(format!("invoke-{tool_name}")),
            tool_call_id: ToolCallId::from(format!("call-{tool_name}")),
            provider_tool_call_id: None,
            tool_name: ToolName::from(tool_name),
            raw_provider_payload: json!({}),
            raw_arguments: raw_arguments.to_string(),
            normalized_arguments: serde_json::Value::Null,
            cancellation_token: CancellationToken::new(),
            timeout: ToolTimeoutConfig::default(),
            execution_context: ToolExecutionContext::new(std::env::temp_dir()),
            provider_metadata: ProviderMetadata {
                provider: "test".to_string(),
                protocol: "chat_like".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "test-model".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: now,
            started_at: Some(now),
        }
    }

    #[test]
    fn typed_runtime_specs_include_kokoro_tools() {
        let provider = Arc::new(KokoroTtsProvider::from_env());
        let tools = provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.spec())
            .collect::<Vec<_>>();

        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, TOOL_TEXT_TO_SPEECH_EN);
        assert_eq!(tools[1].name, TOOL_TEXT_TO_SPEECH_EN_FILE);
    }

    #[test]
    fn typed_runtime_executors_register_only_kokoro_tools() {
        let provider = Arc::new(KokoroTtsProvider::from_env());
        let names = provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.name().as_str().to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![TOOL_TEXT_TO_SPEECH_EN, TOOL_TEXT_TO_SPEECH_EN_FILE]
        );
    }

    #[tokio::test]
    async fn typed_runtime_executor_returns_invalid_parameter_message_before_http() {
        let provider = Arc::new(KokoroTtsProvider::from_env());
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == TOOL_TEXT_TO_SPEECH_EN)
            .expect("typed Kokoro TTS executor registered");

        let output = executor
            .execute(runtime_invocation(
                TOOL_TEXT_TO_SPEECH_EN,
                r#"{"text":"hello","voice":"missing_voice"}"#,
            ))
            .await
            .expect("invalid voice returns model-visible output");

        assert_eq!(output.status, ToolOutputStatus::Success);
        assert!(output
            .stdout
            .text
            .as_deref()
            .expect("stdout text")
            .contains("Invalid TTS parameters"));
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

    #[test]
    fn output_path_defaults_into_workspace_generated() {
        let path = build_output_path(None, "speech_en", "wav");
        assert!(path.starts_with("/workspace/generated/speech_en_"));
        assert!(path.ends_with(".wav"));
    }
}
