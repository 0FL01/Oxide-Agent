//! Explicit media-analysis tools for files already stored in the sandbox.

use crate::agent::provider::ToolProvider;
use crate::llm::{LlmClient, ToolDefinition};
use crate::sandbox::{SandboxManager, SandboxScope};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use reqwest::Url;
use serde::Deserialize;
use serde_json::json;
use shell_escape::escape;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::path::resolve_file_path;

const TOOL_TRANSCRIBE_AUDIO_FILE: &str = "transcribe_audio_file";
const TOOL_DESCRIBE_IMAGE_FILE: &str = "describe_image_file";
const TOOL_DESCRIBE_VIDEO_FILE: &str = "describe_video_file";
const REMOTE_MEDIA_DIR: &str = "/workspace/downloads/media";
const REMOTE_MEDIA_HEAD_TIMEOUT_SECS: u64 = 15;
const REMOTE_MEDIA_DOWNLOAD_TIMEOUT_SECS: u64 = 180;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Image,
    Video,
}

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

    async fn read_media_source(
        &self,
        path: &str,
        kind: Option<MediaKind>,
    ) -> Result<(SandboxManager, String, Vec<u8>, Option<String>)> {
        self.ensure_sandbox().await?;
        let mut sandbox = {
            let guard = self.sandbox.lock().await;
            guard
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("Sandbox not initialized"))?
        };

        if is_remote_url(path) {
            let media_kind = kind.ok_or_else(|| anyhow!("Remote media kind is required"))?;
            let resolved_path = self
                .download_remote_media_file(&mut sandbox, path, media_kind)
                .await?;
            let bytes = sandbox.read_file(&resolved_path).await?;
            return Ok((sandbox, resolved_path.clone(), bytes, Some(resolved_path)));
        }

        let resolved_path = resolve_file_path(&mut sandbox, path).await?;
        let bytes = sandbox.read_file(&resolved_path).await?;
        Ok((sandbox, resolved_path, bytes, None))
    }

    async fn read_media_file(&self, path: &str) -> Result<(SandboxManager, String, Vec<u8>)> {
        let (sandbox, resolved_path, bytes, _) = self.read_media_source(path, None).await?;
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

    async fn probe_remote_media_content_type(&self, url: &Url) -> Option<String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .timeout(Duration::from_secs(REMOTE_MEDIA_HEAD_TIMEOUT_SECS))
            .build()
            .ok()?;

        let response = client.head(url.clone()).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }

        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.split(';').next().unwrap_or(value).trim().to_string())
    }

    async fn download_remote_media_file(
        &self,
        sandbox: &mut SandboxManager,
        source: &str,
        kind: MediaKind,
    ) -> Result<String> {
        let url = normalize_remote_media_url(source)?;
        let content_type = self.probe_remote_media_content_type(&url).await;

        if let Some(content_type) = content_type.as_deref() {
            if is_html_content_type(content_type) {
                return Err(anyhow!(
                    "URL resolves to HTML content, not media; use a direct image/video URL or upload the file into the sandbox"
                ));
            }

            if let Some(expected_tool) = mismatched_media_kind(content_type, kind) {
                return Err(anyhow!(
                    "URL resolves to {content_type} content; use `{expected_tool}` instead"
                ));
            }
        }

        sandbox
            .exec_command(
                &format!("mkdir -p {}", escape(REMOTE_MEDIA_DIR.into())),
                None,
            )
            .await?;

        let file_name = remote_media_file_name(&url, kind, content_type.as_deref());
        let resolved_path = format!("{REMOTE_MEDIA_DIR}/{file_name}");
        let download_cmd = format!(
            "curl -fsSL --retry 3 --retry-all-errors --retry-delay 1 --connect-timeout 10 --max-time {timeout} -o {path} {url}",
            timeout = REMOTE_MEDIA_DOWNLOAD_TIMEOUT_SECS,
            path = escape(resolved_path.as_str().into()),
            url = escape(url.as_str().into()),
        );

        let result = sandbox.exec_command(&download_cmd, None).await?;
        if !result.success() {
            return Err(anyhow!(
                "Failed to download remote media: {}",
                result.combined_output()
            ));
        }

        let size = sandbox.file_size_bytes(&resolved_path, None).await?;
        if size == 0 {
            return Err(anyhow!("Downloaded remote media is empty"));
        }

        Ok(resolved_path)
    }

    async fn cleanup_downloaded_media(&self, sandbox: &mut SandboxManager, path: &str) {
        if let Err(error) = sandbox
            .exec_command(&format!("rm -f {}", escape(path.into())), None)
            .await
        {
            warn!(path = %path, error = %error, "Failed to clean up remote media download");
        }
    }

    fn browser_use_session_from_screenshot_path(path: &str) -> Option<&str> {
        let normalized = path.trim();
        let rest = normalized.strip_prefix("/workspace/browser_use/")?;
        let (session_id, file_name) = rest.split_once('/')?;
        if session_id.is_empty()
            || file_name.is_empty()
            || !file_name.starts_with("screenshot-")
            || !file_name.ends_with(".png")
        {
            return None;
        }
        Some(session_id)
    }

    async fn read_browser_use_latest_screenshot(
        &self,
        original_path: &str,
    ) -> Result<Option<(String, Vec<u8>)>> {
        let Some(session_id) = Self::browser_use_session_from_screenshot_path(original_path) else {
            return Ok(None);
        };

        self.ensure_sandbox().await?;
        let mut sandbox = {
            let guard = self.sandbox.lock().await;
            guard
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("Sandbox not initialized"))?
        };

        let stable_path = format!("/workspace/browser_use/{session_id}/latest.png");
        match sandbox.read_file(&stable_path).await {
            Ok(bytes) => {
                warn!(
                    requested_path = %original_path,
                    fallback_path = %stable_path,
                    "Browser Use screenshot path missing, using stable latest screenshot"
                );
                Ok(Some((stable_path, bytes)))
            }
            Err(_) => Ok(None),
        }
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
        let (mut sandbox, resolved_path, image_bytes, cleanup_path) = match self
            .read_media_source(&args.path, Some(MediaKind::Image))
            .await
        {
            Ok(result) => result,
            Err(error) => {
                if let Some((fallback_path, bytes)) =
                    self.read_browser_use_latest_screenshot(&args.path).await?
                {
                    self.ensure_sandbox().await?;
                    let sandbox = {
                        let guard = self.sandbox.lock().await;
                        guard
                            .as_ref()
                            .cloned()
                            .ok_or_else(|| anyhow!("Sandbox not initialized"))?
                    };
                    (sandbox, fallback_path, bytes, None)
                } else {
                    return Err(error);
                }
            }
        };
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

        if let Some(path) = cleanup_path {
            self.cleanup_downloaded_media(&mut sandbox, &path).await;
        }

        Ok(serde_json::to_string(&json!({
            "ok": true,
            "path": resolved_path,
            "model": model_name,
            "description": description,
        }))?)
    }

    async fn handle_describe_video_file(&self, arguments: &str) -> Result<String> {
        let args: VideoFileArgs = serde_json::from_str(arguments)?;
        let (mut sandbox, resolved_path, video_bytes, cleanup_path) = self
            .read_media_source(&args.path, Some(MediaKind::Video))
            .await?;
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

        if let Some(path) = cleanup_path {
            self.cleanup_downloaded_media(&mut sandbox, &path).await;
        }

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

fn is_remote_url(path: &str) -> bool {
    let trimmed = path.trim_start();
    trimmed.starts_with("http://") || trimmed.starts_with("https://")
}

fn normalize_remote_media_url(source: &str) -> Result<Url> {
    let parsed = Url::parse(source.trim())?;
    if let Some(rewritten) = rewrite_github_blob_url(&parsed) {
        return Ok(rewritten);
    }

    Ok(parsed)
}

fn rewrite_github_blob_url(url: &Url) -> Option<Url> {
    if url.host_str()? != "github.com" {
        return None;
    }

    let segments: Vec<_> = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect();
    let blob_index = segments.iter().position(|segment| *segment == "blob")?;
    if blob_index < 2 || blob_index + 2 >= segments.len() {
        return None;
    }

    let user = segments.first()?;
    let repo = segments.get(1)?;
    let branch = segments.get(blob_index + 1)?;
    let path = segments.get(blob_index + 2..)?.join("/");

    let mut raw = Url::parse("https://raw.githubusercontent.com").ok()?;
    raw.set_path(&format!("{user}/{repo}/{branch}/{path}"));
    raw.set_query(url.query());
    Some(raw)
}

fn mismatched_media_kind(content_type: &str, kind: MediaKind) -> Option<&'static str> {
    match kind {
        MediaKind::Image if content_type.starts_with("video/") => Some("describe_video_file"),
        MediaKind::Video if content_type.starts_with("image/") => Some("describe_image_file"),
        _ => None,
    }
}

fn is_html_content_type(content_type: &str) -> bool {
    content_type.starts_with("text/html") || content_type.starts_with("application/xhtml+xml")
}

fn remote_media_file_name(url: &Url, kind: MediaKind, content_type: Option<&str>) -> String {
    let last_segment = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>()
        })
        .and_then(|segments| segments.last().copied())
        .unwrap_or("media");

    let mut file_name = sanitize_file_name(last_segment);
    if Path::new(&file_name).extension().is_none() {
        let ext = content_type
            .and_then(remote_media_extension_from_content_type)
            .unwrap_or(match kind {
                MediaKind::Image => "png",
                MediaKind::Video => "mp4",
            });
        file_name.push('.');
        file_name.push_str(ext);
    }

    if file_name == "." || file_name.is_empty() {
        file_name = format!(
            "media.{}",
            match kind {
                MediaKind::Image => "png",
                MediaKind::Video => "mp4",
            }
        );
    }

    let (stem, ext) = match Path::new(&file_name)
        .extension()
        .and_then(|ext| ext.to_str())
    {
        Some(ext) => (
            Path::new(&file_name)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("media"),
            ext,
        ),
        None => (file_name.as_str(), "bin"),
    };

    format!("{stem}-{}.{}", remote_media_nonce(), ext)
}

fn remote_media_extension_from_content_type(content_type: &str) -> Option<&'static str> {
    match content_type {
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/bmp" => Some("bmp"),
        "image/svg+xml" => Some("svg"),
        "video/mp4" => Some("mp4"),
        "video/webm" => Some("webm"),
        "video/quicktime" => Some("mov"),
        "video/x-matroska" => Some("mkv"),
        "video/x-msvideo" => Some("avi"),
        _ => None,
    }
}

fn sanitize_file_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();

    sanitized.trim_matches('_').to_string()
}

fn remote_media_nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
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
                description: "Analyze an image file stored in the sandbox or fetched from a direct URL and return a detailed description. Use this only when you explicitly need image understanding.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the image file in the sandbox (relative or absolute) or an http(s) URL to a remote image"
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
                description: "Analyze a video file stored in the sandbox or fetched from a direct URL and return a detailed description of the clip. Use this only when you explicitly need video understanding.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the video file in the sandbox (relative or absolute) or an http(s) URL to a remote video"
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
    fn browser_use_session_parses_valid_screenshot_path() {
        let session_id = MediaFileProvider::browser_use_session_from_screenshot_path(
            "/workspace/browser_use/browser-use-123/screenshot-20260402T163159020465Z.png",
        );
        assert_eq!(session_id, Some("browser-use-123"));
    }

    #[test]
    fn browser_use_session_rejects_non_screenshot_paths() {
        assert!(MediaFileProvider::browser_use_session_from_screenshot_path(
            "/workspace/browser_use/browser-use-123/latest.png"
        )
        .is_none());
        assert!(MediaFileProvider::browser_use_session_from_screenshot_path(
            "/workspace/other/screenshot-20260402T163159020465Z.png"
        )
        .is_none());
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

    #[test]
    fn remote_media_helpers_recognize_and_rewrite_urls() {
        assert!(is_remote_url("https://example.com/a.gif"));
        assert!(!is_remote_url("/workspace/a.gif"));

        let raw = normalize_remote_media_url(
            "https://github.com/Tarquinen/oc-tps/blob/main/assets/demo.gif",
        )
        .expect("github blob url should parse");
        assert_eq!(
            raw.as_str(),
            "https://raw.githubusercontent.com/Tarquinen/oc-tps/main/assets/demo.gif"
        );

        let file_name = remote_media_file_name(
            &Url::parse("https://raw.githubusercontent.com/Tarquinen/oc-tps/main/assets/demo.gif")
                .expect("url"),
            MediaKind::Image,
            Some("image/gif"),
        );
        assert!(file_name.starts_with("demo-"));
        assert!(file_name.ends_with(".gif"));
    }

    #[test]
    fn media_tool_descriptions_mention_urls() {
        let provider =
            MediaFileProvider::new(Arc::new(LlmClient::new(&AgentSettings::default())), 42_i64);
        let tools = provider.tools();

        let image_tool = tools
            .iter()
            .find(|tool| tool.name == TOOL_DESCRIBE_IMAGE_FILE)
            .expect("image tool");
        let video_tool = tools
            .iter()
            .find(|tool| tool.name == TOOL_DESCRIBE_VIDEO_FILE)
            .expect("video tool");

        assert!(image_tool.parameters["properties"]["path"]["description"]
            .as_str()
            .is_some_and(|text| text.contains("URL")));
        assert!(video_tool.parameters["properties"]["path"]["description"]
            .as_str()
            .is_some_and(|text| text.contains("URL")));
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
