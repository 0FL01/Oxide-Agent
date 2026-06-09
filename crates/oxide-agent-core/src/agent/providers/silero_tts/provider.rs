//! Silero TTS Tool Provider.
//!
//! Provides native typed runtime executors for Russian text-to-speech synthesis using Silero.
//! Sends generated audio as voice messages via the progress channel.
//! Supports SSML for enhanced speech control.

use super::client::SileroClient;
use super::types::{SileroTtsConfig, TextToSpeechRuArgs};
use crate::agent::progress::AgentEvent;
use crate::agent::progress::FileDeliveryKind;
use crate::agent::providers::SandboxRuntime;
use crate::agent::providers::file_delivery::{
    FileDeliveryRequest, FileDeliveryStatus, deliver_file_via_progress,
};
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

const TOOL_TEXT_TO_SPEECH_RU: &str = "text_to_speech_ru";
const TOOL_TEXT_TO_SPEECH_RU_FILE: &str = "text_to_speech_ru_file";

/// Silero TTS provider.
pub struct SileroTtsProvider {
    client: SileroClient,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    fileops: Option<Arc<dyn SandboxFileOps>>,
    exec: Option<Arc<dyn SandboxExec>>,
}

impl SileroTtsProvider {
    /// Create a new Silero TTS provider.
    #[must_use]
    pub fn new(config: SileroTtsConfig) -> Self {
        Self {
            client: SileroClient::new(config),
            progress_tx: None,
            fileops: None,
            exec: None,
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

    /// Build native typed runtime executors for Silero TTS tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        let execution_lock = Arc::new(Mutex::new(()));
        Self::tool_definitions()
            .into_iter()
            .map(|spec| {
                Arc::new(SileroTtsToolExecutor {
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
            name: TOOL_TEXT_TO_SPEECH_RU.to_string(),
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
        }
    }

    fn text_to_speech_file_tool() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_TEXT_TO_SPEECH_RU_FILE.to_string(),
            description: concat!(
                "Convert Russian text to speech and save the audio inside the sandbox for downstream tools such as ffmpeg. ",
                "Use this when you need a file path instead of immediate delivery to the user."
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
                        "description": "Speaker voice. Default: baya."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["ogg", "wav"],
                        "description": "Audio format. Use 'wav' when a downstream editor needs PCM audio."
                    },
                    "sample_rate": {
                        "type": "integer",
                        "enum": [8000, 24000, 48000],
                        "description": "Sample rate in Hz."
                    },
                    "ssml": {
                        "type": "boolean",
                        "description": "Whether the text is SSML markup."
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
        debug!(tool = tool_name, "Executing Silero TTS tool");

        match tool_name {
            TOOL_TEXT_TO_SPEECH_RU => {
                let args: TextToSpeechRuArgs = match serde_json::from_str(arguments) {
                    Ok(args) => args,
                    Err(error) => {
                        return Ok(format!("Invalid arguments: {error}"));
                    }
                };

                self.execute_text_to_speech_ru(args, progress_tx).await
            }
            TOOL_TEXT_TO_SPEECH_RU_FILE => {
                let args: TextToSpeechRuArgs = match serde_json::from_str(arguments) {
                    Ok(args) => args,
                    Err(error) => {
                        return Ok(format!("Invalid arguments: {error}"));
                    }
                };

                self.execute_text_to_speech_ru_file(args).await
            }
            _ => anyhow::bail!("Unknown Silero TTS tool: {tool_name}"),
        }
    }

    async fn write_audio_file(&self, path: &str, content: &[u8]) -> Result<()> {
        let exec = self
            .exec
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Sandbox exec is not configured for Silero TTS"))?;
        let fileops = self
            .fileops
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Sandbox fileops is not configured for Silero TTS"))?;

        ensure_parent_dir(exec, path).await?;
        fileops.write_file(path, content).await
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

        let text_to_validate = if request.ssml || request.text.contains('<') {
            strip_ssml_tags(&request.text)
        } else {
            request.text.clone()
        };

        // Reject Arabic numerals in spoken text. For SSML, allow digits in tag attributes.
        if text_to_validate.chars().any(|c| c.is_ascii_digit()) {
            return Ok(
                "ERROR: Text contains Arabic numerals (0-9) which Silero cannot pronounce. "
                    .to_string()
                    + "Digits inside SSML attributes are allowed (e.g. <break time=\"500ms\"/>). "
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
                FileDeliveryStatus::TooLarge { limit_bytes } => Ok(format!(
                    "Russian voice message is too large for chat delivery ({:.2} MB limit). Increase OXIDE_CHAT_DELIVERY_MAX_FILE_SIZE_BYTES or use a smaller output.",
                    limit_bytes as f64 / 1024.0 / 1024.0
                )),
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

    #[instrument(skip(self), level = "debug")]
    async fn execute_text_to_speech_ru_file(&self, args: TextToSpeechRuArgs) -> Result<String> {
        debug!("Parsing Silero TTS file arguments");

        let config = SileroTtsConfig::from_env();
        let request = match args.to_request(&config) {
            Ok(req) => req,
            Err(error) => {
                return Ok(format!("Invalid Silero TTS parameters: {error}"));
            }
        };

        let text_to_validate = if request.ssml || request.text.contains('<') {
            strip_ssml_tags(&request.text)
        } else {
            request.text.clone()
        };
        if text_to_validate.chars().any(|c| c.is_ascii_digit()) {
            return Ok(
                "ERROR: Text contains Arabic numerals (0-9) which Silero cannot pronounce. Rewrite the text using Russian words for numbers and try again.".to_string(),
            );
        }

        let audio_bytes = match self.client.synthesize(&request).await {
            Ok(bytes) => bytes,
            Err(error) => {
                error!(error = %error, "Silero TTS synthesis failed");
                return Ok(format!("Failed to generate Russian speech: {error}"));
            }
        };

        let output_path =
            build_output_path(args.output_path.as_deref(), "speech_ru", &request.format);
        self.write_audio_file(&output_path, &audio_bytes)
            .await
            .with_context(|| {
                format!("Failed to write Russian speech file to sandbox path {output_path}")
            })?;

        Ok(serde_json::to_string(&json!({
            "ok": true,
            "path": output_path,
            "bytes": audio_bytes.len(),
            "format": request.format,
            "speaker": request.speaker,
            "sample_rate": request.sample_rate,
            "duration_seconds_estimate": estimate_duration(&request.text),
        }))?)
    }
}

/// Estimate audio duration based on word count.
/// Average Russian speaking rate: ~150 words per minute = 2.5 words per second.
fn estimate_duration(text: &str) -> f32 {
    let words: Vec<&str> = text.split_whitespace().collect();
    let word_count = words.len() as f32;
    word_count / 2.5
}

fn strip_ssml_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
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

struct SileroTtsToolExecutor {
    provider: Arc<SileroTtsProvider>,
    name: ToolName,
    spec: ToolDefinition,
    execution_lock: Arc<Mutex<()>>,
}

#[async_trait]
impl ToolExecutor for SileroTtsToolExecutor {
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
            turn_id: TurnId::from("turn-silero-tts"),
            batch_id: ToolBatchId::from("batch-silero-tts"),
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
    fn typed_runtime_specs_include_silero_tools() {
        let provider = Arc::new(SileroTtsProvider::from_env());
        let tools = provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.spec())
            .collect::<Vec<_>>();

        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, TOOL_TEXT_TO_SPEECH_RU);
        assert_eq!(tools[1].name, TOOL_TEXT_TO_SPEECH_RU_FILE);
    }

    #[test]
    fn typed_runtime_executors_register_only_silero_tools() {
        let provider = Arc::new(SileroTtsProvider::from_env());
        let names = provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.name().as_str().to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![TOOL_TEXT_TO_SPEECH_RU, TOOL_TEXT_TO_SPEECH_RU_FILE]
        );
    }

    #[tokio::test]
    async fn typed_runtime_executor_rejects_digits_before_http() {
        let provider = Arc::new(SileroTtsProvider::from_env());
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == TOOL_TEXT_TO_SPEECH_RU)
            .expect("typed Silero TTS executor registered");

        let output = executor
            .execute(runtime_invocation(
                TOOL_TEXT_TO_SPEECH_RU,
                r#"{"text":"Привет 123"}"#,
            ))
            .await
            .expect("digits return model-visible output");

        assert_eq!(output.status, ToolOutputStatus::Success);
        assert!(
            output
                .stdout
                .text
                .as_deref()
                .expect("stdout text")
                .contains("Arabic numerals")
        );
    }

    #[test]
    fn duration_estimation() {
        // 6 words at normal speed = ~2.4 seconds
        let duration = estimate_duration("Это тестовое предложение из шести слов");
        assert!(duration > 2.0 && duration < 3.0);

        // Empty text
        let duration = estimate_duration("");
        assert_eq!(duration, 0.0);
    }

    #[test]
    fn output_path_defaults_into_workspace_generated() {
        let path = build_output_path(None, "speech_ru", "wav");
        assert!(path.starts_with("/workspace/generated/speech_ru_"));
        assert!(path.ends_with(".wav"));
    }
}
