//! Explicit media-analysis tools for files already stored in the sandbox.

use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::{LlmClient, ToolDefinition};
use crate::sandbox::{SandboxExec, SandboxFileOps, SandboxScope};
use crate::storage::StorageProvider;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use reqwest::Url;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use serde_json::json;
use shell_escape::escape;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::path::resolve_file_path;
use super::sandbox::SandboxRuntime;

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
    fileops: Arc<dyn SandboxFileOps>,
    exec: Arc<dyn SandboxExec>,
    /// Durable storage for resolving `artifact://` URIs (browser-live screenshots).
    /// `None` when no browser-live context is available.
    storage: Option<Arc<dyn StorageProvider>>,
    /// User ID for `artifact://` ownership checks in durable storage.
    user_id: Option<i64>,
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
        Self::from_runtime(
            llm_client,
            Arc::new(SandboxRuntime::new(sandbox_scope.into())),
        )
    }

    /// Create a new provider from the shared sandbox runtime.
    #[must_use]
    pub fn from_runtime(llm_client: Arc<LlmClient>, runtime: Arc<SandboxRuntime>) -> Self {
        let fileops: Arc<dyn SandboxFileOps> = Arc::<SandboxRuntime>::clone(&runtime);
        let exec: Arc<dyn SandboxExec> = runtime;
        Self::with_sandbox_backends(llm_client, fileops, exec)
    }

    /// Create a new provider from the shared sandbox runtime with durable
    /// storage for `artifact://` URI resolution (browser-live screenshots).
    #[must_use]
    pub fn from_runtime_with_storage(
        llm_client: Arc<LlmClient>,
        runtime: Arc<SandboxRuntime>,
        storage: Arc<dyn StorageProvider>,
        user_id: i64,
    ) -> Self {
        let fileops: Arc<dyn SandboxFileOps> = Arc::<SandboxRuntime>::clone(&runtime);
        let exec: Arc<dyn SandboxExec> = runtime;
        Self::with_sandbox_backends_and_storage(
            llm_client,
            fileops,
            exec,
            Some(storage),
            Some(user_id),
        )
    }

    /// Create a provider from narrow sandbox capability traits.
    #[must_use]
    pub fn with_sandbox_backends(
        llm_client: Arc<LlmClient>,
        fileops: Arc<dyn SandboxFileOps>,
        exec: Arc<dyn SandboxExec>,
    ) -> Self {
        Self::with_sandbox_backends_and_storage(llm_client, fileops, exec, None, None)
    }

    /// Create a provider from narrow sandbox capability traits with durable
    /// storage for `artifact://` URI resolution (browser-live screenshots).
    #[must_use]
    pub fn with_sandbox_backends_and_storage(
        llm_client: Arc<LlmClient>,
        fileops: Arc<dyn SandboxFileOps>,
        exec: Arc<dyn SandboxExec>,
        storage: Option<Arc<dyn StorageProvider>>,
        user_id: Option<i64>,
    ) -> Self {
        Self {
            llm_client,
            fileops,
            exec,
            storage,
            user_id,
        }
    }

    /// Build native typed runtime executors for all media-file tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        self.tool_runtime_executors_for(&[
            TOOL_TRANSCRIBE_AUDIO_FILE,
            TOOL_DESCRIBE_IMAGE_FILE,
            TOOL_DESCRIBE_VIDEO_FILE,
        ])
    }

    /// Build native typed runtime executors for a module-owned media tool subset.
    #[must_use]
    pub fn tool_runtime_executors_for(
        self: &Arc<Self>,
        tool_names: &[&str],
    ) -> Vec<Arc<dyn ToolExecutor>> {
        let execution_lock = Arc::new(Mutex::new(()));
        Self::tool_definitions()
            .into_iter()
            .filter(|spec| tool_names.contains(&spec.name.as_str()))
            .map(|spec| {
                Arc::new(MediaFileToolExecutor {
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
            Self::transcribe_audio_tool(),
            Self::describe_image_tool(),
            Self::describe_video_tool(),
        ]
    }

    fn transcribe_audio_tool() -> ToolDefinition {
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
        }
    }

    fn describe_image_tool() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_DESCRIBE_IMAGE_FILE.to_string(),
            description: "Analyze an image file stored in the sandbox, fetched from a direct URL, or referenced by an artifact:// URI and return a detailed description. Use this only when you explicitly need image understanding.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the image file in the sandbox (relative or absolute), an http(s) URL to a remote image, or an artifact:// URI produced by the browser-live tool"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Optional task-specific prompt that explains what to focus on"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    fn describe_video_tool() -> ToolDefinition {
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
        }
    }

    async fn read_media_source(
        &self,
        path: &str,
        kind: Option<MediaKind>,
        artifact_dir: Option<&Path>,
    ) -> Result<(String, Vec<u8>, Option<String>)> {
        if path.starts_with("artifact://") {
            // Tier 1: filesystem (locally cached artifacts / backward compat).
            if let Some(artifact_dir) = artifact_dir {
                let relative = path.strip_prefix("artifact://").unwrap_or(path);
                let local_path = artifact_dir.join(relative);
                let canonical_dir = std::fs::canonicalize(artifact_dir)
                    .ok()
                    .unwrap_or_else(|| artifact_dir.to_path_buf());
                let canonical_path = std::fs::canonicalize(&local_path)
                    .ok()
                    .unwrap_or_else(|| local_path.clone());
                if !canonical_path.starts_with(&canonical_dir) {
                    return Err(anyhow!(
                        "artifact URI resolves outside the artifact directory"
                    ));
                }
                if let Ok(bytes) = tokio::fs::read(&canonical_path).await {
                    return Ok((canonical_path.to_string_lossy().to_string(), bytes, None));
                }
                // FS miss — fall through to durable storage.
            }

            // Tier 2: Postgres BYTEA (browser-live screenshots persisted by
            // `BrowserLiveProvider::persist_latest_screenshot`).
            if let Some(storage) = &self.storage
                && let Some(user_id) = self.user_id
            {
                let artifact = storage
                    .load_browser_artifact(user_id, path)
                    .await
                    .map_err(|error| anyhow!("Failed to load browser artifact {path}: {error}"))?;
                if let Some(data) = artifact {
                    return Ok((path.to_string(), data.data, None));
                }
            }

            return Err(anyhow!(
                "artifact URI {path} not found on disk or in durable storage"
            ));
        }

        if is_remote_url(path) {
            let media_kind = kind.ok_or_else(|| anyhow!("Remote media kind is required"))?;
            let resolved_path = self.download_remote_media_file(path, media_kind).await?;
            let bytes = self.fileops.read_file(&resolved_path).await?;
            return Ok((resolved_path.clone(), bytes, Some(resolved_path)));
        }

        let resolved_path = resolve_file_path(self.exec.as_ref(), path).await?;
        let bytes = self.fileops.read_file(&resolved_path).await?;
        Ok((resolved_path, bytes, None))
    }

    async fn read_media_file(
        &self,
        path: &str,
        artifact_dir: Option<&Path>,
    ) -> Result<(String, Vec<u8>)> {
        let (resolved_path, bytes, _) = self.read_media_source(path, None, artifact_dir).await?;
        Ok((resolved_path, bytes))
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

    async fn download_remote_media_file(&self, source: &str, kind: MediaKind) -> Result<String> {
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

        self.exec
            .exec(
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

        let result = self.exec.exec(&download_cmd, None).await?;
        if !result.success() {
            return Err(anyhow!(
                "Failed to download remote media: {}",
                result.combined_output()
            ));
        }

        let size = self.fileops.file_size_bytes(&resolved_path, None).await?;
        if size == 0 {
            return Err(anyhow!("Downloaded remote media is empty"));
        }

        Ok(resolved_path)
    }

    async fn cleanup_downloaded_media(&self, path: &str) {
        if let Err(error) = self
            .exec
            .exec(&format!("rm -f {}", escape(path.into())), None)
            .await
        {
            warn!(path = %path, error = %error, "Failed to clean up remote media download");
        }
    }

    async fn handle_transcribe_audio_file(
        &self,
        arguments: &str,
        artifact_dir: Option<&Path>,
    ) -> Result<String> {
        let args: AudioFileArgs = serde_json::from_str(arguments)?;
        let (resolved_path, audio_bytes) = self.read_media_file(&args.path, artifact_dir).await?;
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

    async fn handle_describe_image_file(
        &self,
        arguments: &str,
        artifact_dir: Option<&Path>,
    ) -> Result<String> {
        let args: ImageFileArgs = serde_json::from_str(arguments)?;
        let (resolved_path, image_bytes, cleanup_path) = self
            .read_media_source(&args.path, Some(MediaKind::Image), artifact_dir)
            .await?;
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
            self.cleanup_downloaded_media(&path).await;
        }

        Ok(serde_json::to_string(&json!({
            "ok": true,
            "path": resolved_path,
            "model": model_name,
            "description": description,
        }))?)
    }

    async fn handle_describe_video_file(
        &self,
        arguments: &str,
        artifact_dir: Option<&Path>,
    ) -> Result<String> {
        let args: VideoFileArgs = serde_json::from_str(arguments)?;
        let (resolved_path, video_bytes, cleanup_path) = self
            .read_media_source(&args.path, Some(MediaKind::Video), artifact_dir)
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
            self.cleanup_downloaded_media(&path).await;
        }

        Ok(serde_json::to_string(&json!({
            "ok": true,
            "path": resolved_path,
            "mime_type": mime_type,
            "model": model_name,
            "description": description,
        }))?)
    }

    async fn execute_tool(
        &self,
        tool_name: &str,
        arguments: &str,
        artifact_dir: Option<&Path>,
    ) -> Result<String> {
        debug!(tool = tool_name, "Executing media_file tool");
        match tool_name {
            TOOL_TRANSCRIBE_AUDIO_FILE => {
                self.handle_transcribe_audio_file(arguments, artifact_dir)
                    .await
            }
            TOOL_DESCRIBE_IMAGE_FILE => {
                self.handle_describe_image_file(arguments, artifact_dir)
                    .await
            }
            TOOL_DESCRIBE_VIDEO_FILE => {
                self.handle_describe_video_file(arguments, artifact_dir)
                    .await
            }
            _ => anyhow::bail!("Unknown media_file tool: {tool_name}"),
        }
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

struct MediaFileToolExecutor {
    provider: Arc<MediaFileProvider>,
    name: ToolName,
    spec: ToolDefinition,
    execution_lock: Arc<Mutex<()>>,
}

#[async_trait]
impl ToolExecutor for MediaFileToolExecutor {
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
                Some(&invocation.execution_context.artifact_dir),
            )
            .await
            .map(|output| normalizer.success(&invocation, &output, ""))
            .map_err(media_file_runtime_error)
    }
}

fn media_file_runtime_error(error: anyhow::Error) -> ToolRuntimeError {
    if error.downcast_ref::<serde_json::Error>().is_some() {
        ToolRuntimeError::InvalidArguments(error.to_string())
    } else {
        ToolRuntimeError::Failure(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::{
        ModelMetadata, OutputNormalizer, ProviderMetadata, ToolBatchId, ToolCallId,
        ToolExecutionContext, ToolOutputStatus, ToolRuntimeConfig, ToolTimeoutConfig, TurnId,
    };
    use crate::config::{AgentSettings, ModuleRuntimeConfig};
    use crate::llm::{InvocationId, LlmClient};
    use chrono::Utc;
    use tokio_util::sync::CancellationToken;

    fn runtime_invocation(tool_name: &str, raw_arguments: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(77),
            turn_id: TurnId::from("turn-media-file"),
            batch_id: ToolBatchId::from("batch-media-file"),
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
        let provider = Arc::new(MediaFileProvider::new(
            Arc::new(LlmClient::new(&AgentSettings::default())),
            42_i64,
        ));
        let tool = provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == TOOL_TRANSCRIBE_AUDIO_FILE)
            .expect("transcribe_audio_file executor must exist")
            .spec();

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
        let provider = Arc::new(MediaFileProvider::new(
            Arc::new(LlmClient::new(&AgentSettings::default())),
            42_i64,
        ));
        let tools = provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.spec())
            .collect::<Vec<_>>();

        let image_tool = tools
            .iter()
            .find(|tool| tool.name == TOOL_DESCRIBE_IMAGE_FILE)
            .expect("image tool");
        let video_tool = tools
            .iter()
            .find(|tool| tool.name == TOOL_DESCRIBE_VIDEO_FILE)
            .expect("video tool");

        assert!(
            image_tool.parameters["properties"]["path"]["description"]
                .as_str()
                .is_some_and(|text| text.contains("URL"))
        );
        assert!(
            video_tool.parameters["properties"]["path"]["description"]
                .as_str()
                .is_some_and(|text| text.contains("URL"))
        );
    }

    #[test]
    fn typed_runtime_executors_register_media_tools() {
        let provider = Arc::new(MediaFileProvider::new(
            Arc::new(LlmClient::new(&AgentSettings::default())),
            42_i64,
        ));

        let names = provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.name().into_inner())
            .collect::<std::collections::BTreeSet<_>>();

        assert!(names.contains(TOOL_TRANSCRIBE_AUDIO_FILE));
        assert!(names.contains(TOOL_DESCRIBE_IMAGE_FILE));
        assert!(names.contains(TOOL_DESCRIBE_VIDEO_FILE));
    }

    #[test]
    fn typed_runtime_executors_for_filters_media_tools() {
        let provider = Arc::new(MediaFileProvider::new(
            Arc::new(LlmClient::new(&AgentSettings::default())),
            42_i64,
        ));

        let names = provider
            .tool_runtime_executors_for(&[TOOL_DESCRIBE_IMAGE_FILE])
            .into_iter()
            .map(|executor| executor.name().into_inner())
            .collect::<Vec<_>>();

        assert_eq!(names, vec![TOOL_DESCRIBE_IMAGE_FILE.to_string()]);
    }

    #[tokio::test]
    async fn typed_runtime_executor_rejects_missing_path_before_sandbox() {
        let provider = Arc::new(MediaFileProvider::new(
            Arc::new(LlmClient::new(&AgentSettings::default())),
            42_i64,
        ));
        let executor = provider
            .tool_runtime_executors_for(&[TOOL_DESCRIBE_IMAGE_FILE])
            .into_iter()
            .next()
            .expect("image executor");

        let error = executor
            .execute(runtime_invocation(TOOL_DESCRIBE_IMAGE_FILE, "{}"))
            .await
            .expect_err("missing path must be invalid arguments");

        let output = OutputNormalizer::new(ToolRuntimeConfig::default())
            .executor_error(&runtime_invocation(TOOL_DESCRIBE_IMAGE_FILE, "{}"), error);
        assert_eq!(output.status, ToolOutputStatus::InvalidArguments);
    }

    #[tokio::test]
    async fn artifact_uri_resolves_to_local_artifact_file() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time after Unix epoch")
            .as_nanos();
        let artifact_dir = std::env::temp_dir().join(format!("oxide-media-artifact-test-{nonce}"));
        let inner = artifact_dir.join("browser/task-1/session-1");
        tokio::fs::create_dir_all(&inner)
            .await
            .expect("create artifact test dirs");
        let artifact_path = inner.join("step-0001-milestone.png");
        let fake_png = b"\x89PNG\r\n\x1a\nfake-image-bytes";
        tokio::fs::write(&artifact_path, fake_png)
            .await
            .expect("write artifact test file");

        let provider =
            MediaFileProvider::new(Arc::new(LlmClient::new(&AgentSettings::default())), 42_i64);
        let (resolved_path, bytes, cleanup_path) = provider
            .read_media_source(
                "artifact://browser/task-1/session-1/step-0001-milestone.png",
                Some(MediaKind::Image),
                Some(&artifact_dir),
            )
            .await
            .expect("resolve artifact URI");

        assert_eq!(resolved_path, artifact_path.to_string_lossy());
        assert_eq!(bytes, fake_png);
        assert!(cleanup_path.is_none());

        let _ = tokio::fs::remove_dir_all(&artifact_dir).await;
    }

    #[tokio::test]
    async fn artifact_uri_falls_back_to_durable_storage_when_fs_misses() {
        use crate::storage::{BrowserArtifactData, MockStorageProvider};
        use mockall::predicate::eq;

        let uri = "artifact://browser/task-1/session-1/step-0001-milestone.jpg";
        let fake_jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];

        let mut mock_storage = MockStorageProvider::new();
        let expected_bytes = fake_jpeg.clone();
        mock_storage
            .expect_load_browser_artifact()
            .with(eq(42i64), eq(uri.to_string()))
            .returning(move |_, _| {
                Ok(Some(BrowserArtifactData {
                    mime_type: "image/jpeg".to_string(),
                    data: expected_bytes.clone(),
                    bytes: expected_bytes.len() as i64,
                }))
            });

        let provider = MediaFileProvider::from_runtime_with_storage(
            Arc::new(LlmClient::new(&AgentSettings::default())),
            Arc::new(SandboxRuntime::new(SandboxScope::from(42_i64))),
            Arc::new(mock_storage),
            42,
        );

        let (resolved_path, bytes, cleanup_path) = provider
            .read_media_source(uri, Some(MediaKind::Image), None)
            .await
            .expect("resolve artifact URI from durable storage");

        assert_eq!(resolved_path, uri);
        assert_eq!(bytes, fake_jpeg);
        assert!(cleanup_path.is_none());
    }

    mod media_resolver_tests {
        use super::*;

        fn with_provider_key(
            mut settings: AgentSettings,
            module_id: &str,
            api_key: &str,
        ) -> AgentSettings {
            settings.modules.insert(
                module_id.to_string(),
                ModuleRuntimeConfig::default().with_string_value("api_key", api_key),
            );
            settings
        }

        #[test]
        fn resolve_video_model_name_requires_video_capable_media_route() {
            let settings = with_provider_key(
                AgentSettings {
                    agent_model_id: Some("agent-openrouter".to_string()),
                    agent_model_provider: Some("openrouter".to_string()),
                    media_model_id: Some("media-missing".to_string()),
                    media_model_provider: Some("missing-provider".to_string()),
                    ..AgentSettings::default()
                },
                "llm-provider/openrouter",
                "test-openrouter-key",
            );
            let provider = MediaFileProvider::new(Arc::new(LlmClient::new(&settings)), 42_i64);

            let error = provider
                .resolve_video_model_name()
                .expect_err("video model unavailable")
                .to_string();
            assert!(error.contains("Video understanding route unavailable"));
        }

        #[test]
        fn resolve_image_model_name_reports_unavailable_route() {
            let settings = AgentSettings {
                agent_model_id: Some("agent-missing".to_string()),
                agent_model_provider: Some("missing-provider".to_string()),
                media_model_id: Some("media-missing".to_string()),
                media_model_provider: Some("missing-provider".to_string()),
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
