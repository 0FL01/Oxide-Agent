//! Explicit media-analysis tools for files already stored in the sandbox.

use crate::agent::provider::ToolProvider;
use crate::llm::{LlmClient, ToolDefinition};
use crate::sandbox::{SandboxManager, SandboxScope};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

use super::path::resolve_file_path;

const TOOL_TRANSCRIBE_AUDIO_FILE: &str = "transcribe_audio_file";
const TOOL_DESCRIBE_IMAGE_FILE: &str = "describe_image_file";
const TOOL_DESCRIBE_VIDEO_FILE: &str = "describe_video_file";

/// Provider for explicit media analysis on sandbox files.
pub struct MediaFileProvider {
    llm_client: Arc<LlmClient>,
    sandbox: Arc<Mutex<Option<SandboxManager>>>,
    sandbox_scope: SandboxScope,
}

#[derive(Debug, Deserialize)]
struct AudioFileArgs {
    path: String,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ImageFileArgs {
    path: String,
    #[serde(default)]
    prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VideoFileArgs {
    path: String,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    mime_type: Option<String>,
}

impl MediaFileProvider {
    /// Create a new provider with lazy sandbox initialization.
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>, sandbox_scope: impl Into<SandboxScope>) -> Self {
        Self {
            llm_client,
            sandbox: Arc::new(Mutex::new(None)),
            sandbox_scope: sandbox_scope.into(),
        }
    }

    async fn ensure_sandbox(&self) -> Result<()> {
        if self
            .sandbox
            .lock()
            .await
            .as_ref()
            .is_some_and(SandboxManager::is_running)
        {
            return Ok(());
        }

        let mut sandbox = SandboxManager::new(self.sandbox_scope.clone()).await?;
        sandbox.create_sandbox().await?;
        *self.sandbox.lock().await = Some(sandbox);
        Ok(())
    }

    async fn read_media_file(&self, path: &str) -> Result<(SandboxManager, String, Vec<u8>)> {
        self.ensure_sandbox().await?;
        let mut sandbox = {
            let guard = self.sandbox.lock().await;
            guard
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("Sandbox not initialized"))?
        };

        let resolved_path = resolve_file_path(&mut sandbox, path).await?;
        let bytes = sandbox.read_file(&resolved_path).await?;
        Ok((sandbox, resolved_path, bytes))
    }

    fn resolve_audio_model_name(&self) -> Result<String> {
        self.llm_client
            .resolve_media_model_name_for_audio_stt()
            .map_err(|error| anyhow!("Audio transcription route unavailable: {error}"))
    }

    fn resolve_image_model_name(&self) -> Result<String> {
        self.llm_client
            .resolve_media_model_name_for_image()
            .map_err(|error| anyhow!("Image understanding route unavailable: {error}"))
    }

    fn resolve_video_model_name(&self) -> Result<String> {
        self.llm_client
            .resolve_media_model_name_for_video()
            .map_err(|error| anyhow!("Video understanding route unavailable: {error}"))
    }

    async fn handle_transcribe_audio_file(&self, arguments: &str) -> Result<String> {
        let args: AudioFileArgs = serde_json::from_str(arguments)?;
        let (_sandbox, resolved_path, audio_bytes) = self.read_media_file(&args.path).await?;
        let mime_type = args
            .mime_type
            .unwrap_or_else(|| infer_audio_mime_type(&resolved_path).to_string());
        let prompt = args.prompt.unwrap_or_else(|| {
            "Transcribe this audio accurately for an AI agent. Preserve the spoken content faithfully and include timestamps, speaker turns, or structure only when they are clearly available or explicitly relevant.".to_string()
        });
        let model_name = self.resolve_audio_model_name()?;

        info!(path = %resolved_path, mime_type = %mime_type, model = %model_name, "Transcribing sandbox audio file");
        let transcription = self
            .llm_client
            .transcribe_audio_with_prompt(audio_bytes, &mime_type, &prompt, &model_name)
            .await
            .map_err(|error| anyhow!("Audio transcription failed: {error}"))?;

        Ok(serde_json::to_string(&json!({
            "ok": true,
            "path": resolved_path,
            "mime_type": mime_type,
            "model": model_name,
            "transcription": transcription,
        }))?)
    }

    async fn handle_describe_image_file(&self, arguments: &str) -> Result<String> {
        let args: ImageFileArgs = serde_json::from_str(arguments)?;
        let (_sandbox, resolved_path, image_bytes) = self.read_media_file(&args.path).await?;
        let prompt = args.prompt.unwrap_or_else(|| {
            "Describe this image in detail for an AI agent. Include all important details, text, objects and their locations.".to_string()
        });
        let system_prompt = "You are a visual analyzer for an AI agent. Your task is to create a detailed text description of the image that allows the agent to understand its content without accessing the image itself.";
        let model_name = self.resolve_image_model_name()?;

        info!(path = %resolved_path, model = %model_name, "Describing sandbox image file");
        let description = self
            .llm_client
            .analyze_image(image_bytes, &prompt, system_prompt, &model_name)
            .await
            .map_err(|error| anyhow!("Image analysis failed: {error}"))?;

        Ok(serde_json::to_string(&json!({
            "ok": true,
            "path": resolved_path,
            "model": model_name,
            "description": description,
        }))?)
    }

    async fn handle_describe_video_file(&self, arguments: &str) -> Result<String> {
        let args: VideoFileArgs = serde_json::from_str(arguments)?;
        let (_sandbox, resolved_path, video_bytes) = self.read_media_file(&args.path).await?;
        let mime_type = args
            .mime_type
            .unwrap_or_else(|| infer_video_mime_type(&resolved_path).to_string());
        let prompt = args.prompt.unwrap_or_else(|| {
            "Describe this video in detail for an AI agent. Summarize the sequence of events, any visible text, spoken or implied context, and the important objects or actions frame-to-frame.".to_string()
        });
        let system_prompt = "You are a video analyzer for an AI agent. Your task is to create a detailed text description of the clip so the agent can understand the timeline, important visual details, and any visible text without accessing the video itself.";
        let model_name = self.resolve_video_model_name()?;

        info!(path = %resolved_path, mime_type = %mime_type, model = %model_name, "Describing sandbox video file");
        let description = self
            .llm_client
            .analyze_video(video_bytes, &mime_type, &prompt, system_prompt, &model_name)
            .await
            .map_err(|error| anyhow!("Video analysis failed: {error}"))?;

        Ok(serde_json::to_string(&json!({
            "ok": true,
            "path": resolved_path,
            "mime_type": mime_type,
            "model": model_name,
            "description": description,
        }))?)
    }
}

fn infer_audio_mime_type(path: &str) -> &'static str {
    match extension(path).as_deref() {
        Some(ext) if ext.eq_ignore_ascii_case("wav") => "audio/wav",
        Some(ext) if ext.eq_ignore_ascii_case("mp3") => "audio/mpeg",
        Some(ext) if ext.eq_ignore_ascii_case("ogg") || ext.eq_ignore_ascii_case("opus") => {
            "audio/ogg"
        }
        Some(ext) if ext.eq_ignore_ascii_case("m4a") => "audio/mp4",
        Some(ext) if ext.eq_ignore_ascii_case("flac") => "audio/flac",
        Some(ext) if ext.eq_ignore_ascii_case("webm") => "audio/webm",
        _ => "audio/wav",
    }
}

fn infer_video_mime_type(path: &str) -> &'static str {
    match extension(path).as_deref() {
        Some(ext) if ext.eq_ignore_ascii_case("mov") => "video/mov",
        Some(ext) if ext.eq_ignore_ascii_case("mpeg") || ext.eq_ignore_ascii_case("mpg") => {
            "video/mpeg"
        }
        Some(ext) if ext.eq_ignore_ascii_case("webm") => "video/webm",
        _ => "video/mp4",
    }
}

fn extension(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(ToString::to_string)
}

#[async_trait]
impl ToolProvider for MediaFileProvider {
    fn name(&self) -> &'static str {
        "media_file"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TRANSCRIBE_AUDIO_FILE.to_string(),
                description: "Transcribe an audio file that already exists in the sandbox. Use this when you need explicit audio understanding for a preserved upload instead of automatic preprocessing.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the audio file in the sandbox (relative or absolute)"
                        },
                        "mime_type": {
                            "type": "string",
                            "description": "Optional MIME type override, for example audio/ogg or audio/wav"
                        },
                        "prompt": {
                            "type": "string",
                            "description": "Optional task-specific prompt that explains what transcription format or details you need, for example timestamps, speakers, or translation-ready text"
                        }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: TOOL_DESCRIBE_IMAGE_FILE.to_string(),
                description: "Analyze an image file stored in the sandbox and return a detailed description. Use this only when you explicitly need image understanding.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the image file in the sandbox (relative or absolute)"
                        },
                        "prompt": {
                            "type": "string",
                            "description": "Optional task-specific prompt that explains what to focus on"
                        }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: TOOL_DESCRIBE_VIDEO_FILE.to_string(),
                description: "Analyze a video file stored in the sandbox and return a detailed description of the clip. Use this only when you explicitly need video understanding.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the video file in the sandbox (relative or absolute)"
                        },
                        "prompt": {
                            "type": "string",
                            "description": "Optional task-specific prompt that explains what to focus on"
                        },
                        "mime_type": {
                            "type": "string",
                            "description": "Optional MIME type override, for example video/mp4 or video/webm"
                        }
                    },
                    "required": ["path"]
                }),
            },
        ]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            TOOL_TRANSCRIBE_AUDIO_FILE | TOOL_DESCRIBE_IMAGE_FILE | TOOL_DESCRIBE_VIDEO_FILE
        )
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        debug!(tool = tool_name, "Executing media_file tool");
        match tool_name {
            TOOL_TRANSCRIBE_AUDIO_FILE => self.handle_transcribe_audio_file(arguments).await,
            TOOL_DESCRIBE_IMAGE_FILE => self.handle_describe_image_file(arguments).await,
            TOOL_DESCRIBE_VIDEO_FILE => self.handle_describe_video_file(arguments).await,
            _ => anyhow::bail!("Unknown media_file tool: {tool_name}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentSettings;
    use crate::llm::LlmClient;

    #[test]
    fn infers_audio_mime_type_from_extension() {
        assert_eq!(infer_audio_mime_type("/workspace/audio.ogg"), "audio/ogg");
        assert_eq!(infer_audio_mime_type("clip.wav"), "audio/wav");
        assert_eq!(infer_audio_mime_type("voice.unknown"), "audio/wav");
    }

    #[test]
    fn infers_video_mime_type_from_extension() {
        assert_eq!(infer_video_mime_type("movie.webm"), "video/webm");
        assert_eq!(infer_video_mime_type("clip.mov"), "video/mov");
        assert_eq!(infer_video_mime_type("clip.bin"), "video/mp4");
    }

    #[test]
    fn transcribe_audio_tool_accepts_custom_prompt() {
        let provider =
            MediaFileProvider::new(Arc::new(LlmClient::new(&AgentSettings::default())), 42_i64);
        let tool = provider
            .tools()
            .into_iter()
            .find(|tool| tool.name == TOOL_TRANSCRIBE_AUDIO_FILE)
            .expect("transcribe_audio_file tool must exist");

        assert_eq!(
            tool.parameters["properties"]["prompt"]["type"],
            serde_json::json!("string")
        );
    }

    mod media_resolver_tests {
        use super::*;

        #[test]
        fn resolve_audio_model_name_supports_mistral_stt_route() {
            let settings = AgentSettings {
                chat_model_id: Some("chat-mistral".to_string()),
                chat_model_provider: Some("mistral".to_string()),
                mistral_api_key: Some("test-mistral-key".to_string()),
                ..AgentSettings::default()
            };
            let provider = MediaFileProvider::new(Arc::new(LlmClient::new(&settings)), 42_i64);

            assert_eq!(
                provider.resolve_audio_model_name().expect("audio model"),
                "chat-mistral"
            );
        }

        #[test]
        fn resolve_video_model_name_falls_back_to_chat_route() {
            let settings = AgentSettings {
                chat_model_id: Some("chat-openrouter".to_string()),
                chat_model_provider: Some("openrouter".to_string()),
                media_model_id: Some("media-mistral".to_string()),
                media_model_provider: Some("mistral".to_string()),
                openrouter_api_key: Some("test-openrouter-key".to_string()),
                mistral_api_key: Some("test-mistral-key".to_string()),
                ..AgentSettings::default()
            };
            let provider = MediaFileProvider::new(Arc::new(LlmClient::new(&settings)), 42_i64);

            assert_eq!(
                provider.resolve_video_model_name().expect("video model"),
                "chat-openrouter"
            );
        }

        #[test]
        fn resolve_image_model_name_reports_unavailable_route() {
            let settings = AgentSettings {
                chat_model_id: Some("chat-mistral".to_string()),
                chat_model_provider: Some("mistral".to_string()),
                mistral_api_key: Some("test-mistral-key".to_string()),
                ..AgentSettings::default()
            };
            let provider = MediaFileProvider::new(Arc::new(LlmClient::new(&settings)), 42_i64);

            let error = provider
                .resolve_image_model_name()
                .expect_err("image model unavailable")
                .to_string();
            assert!(error.contains("Image understanding route unavailable"));
        }
    }
}
