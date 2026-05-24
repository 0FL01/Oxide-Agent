//! YT-DLP Provider - video platform tools via yt-dlp in sandbox
//!
//! Provides tools for video metadata extraction, transcript download,
//! video search, and media download from YouTube and other platforms.
//!
//! All operations execute inside the Docker sandbox where yt-dlp is installed.

use crate::agent::progress::{AgentEvent, FileDeliveryKind};
use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use crate::sandbox::{SandboxExec, SandboxFileOps, SandboxScope};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::fmt::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::file_delivery::{deliver_file_via_progress, FileDeliveryRequest, FileDeliveryStatus};
use super::sandbox::SandboxRuntime;

/// Patterns indicating fatal, unrecoverable yt-dlp errors
/// that should stop execution immediately
const FATAL_ERROR_PATTERNS: &[&str] = &[
    "Video unavailable",
    "Private video",
    "This video is not available",
    "Sign in to confirm your age",
    "age-restricted",
    "members-only",
    "This video is private",
    "removed by the uploader",
    "no longer available",
    "blocked it in your country",
    "geo-restricted",
    "who has blocked it on copyright grounds",
    "copyright claim",
    "terminated account",
    "This video has been removed",
    "ERROR: Unsupported URL",
    "is not a valid URL",
    "Unable to extract video data",
    "Premieres in",
    "This live event will begin",
    "Join this channel to get access",
    "HTTP Error 403",
    "HTTP Error 404",
    "Sign in to view this video",
];

/// Patterns indicating transient errors that might be resolved with retry
const RETRYABLE_ERROR_PATTERNS: &[&str] = &[
    "Connection reset",
    "Connection timed out",
    "Unable to download webpage",
    "HTTP Error 429", // Too Many Requests
    "HTTP Error 503", // Service Unavailable
    "Read timed out",
    "network is unreachable",
    "Temporary failure in name resolution",
];

/// Check if error message indicates a fatal, unrecoverable error
fn is_fatal_ytdlp_error(error_msg: &str) -> bool {
    FATAL_ERROR_PATTERNS
        .iter()
        .any(|pattern| error_msg.contains(pattern))
}

/// Check if error message indicates a retryable error
fn is_retryable_ytdlp_error(error_msg: &str) -> bool {
    RETRYABLE_ERROR_PATTERNS
        .iter()
        .any(|pattern| error_msg.contains(pattern))
}

async fn cleanup_old_downloads(exec: Arc<dyn SandboxExec>) {
    let count_result = match exec
        .exec(
            "find /workspace/downloads -type f -mtime +7 2>/dev/null | wc -l",
            None,
        )
        .await
    {
        Ok(result) => result,
        Err(error) => {
            warn!(error = %error, "Failed to count old yt-dlp downloads");
            return;
        }
    };
    let count: u64 = count_result.stdout.trim().parse().unwrap_or(0);
    if count == 0 {
        return;
    }

    match exec
        .exec(
            "find /workspace/downloads -type f -mtime +7 -delete 2>/dev/null",
            None,
        )
        .await
    {
        Ok(_) => debug!(
            files_deleted = count,
            "Cleaned up old yt-dlp download files"
        ),
        Err(error) => warn!(error = %error, "Failed to clean up old yt-dlp downloads"),
    }
}

/// Maximum character limit for transcript output (to avoid LLM context overflow)
const MAX_TRANSCRIPT_LENGTH: usize = 50_000;

/// Maximum character limit for metadata output
const MAX_METADATA_LENGTH: usize = 25_000;

/// Directory inside sandbox for downloaded media
const DOWNLOADS_DIR: &str = "/workspace/downloads";
const TOOL_YTDLP_GET_METADATA: &str = "ytdlp_get_video_metadata";
const TOOL_YTDLP_DOWNLOAD_TRANSCRIPT: &str = "ytdlp_download_transcript";
const TOOL_YTDLP_SEARCH_VIDEOS: &str = "ytdlp_search_videos";
const TOOL_YTDLP_DOWNLOAD_VIDEO: &str = "ytdlp_download_video";
const TOOL_YTDLP_DOWNLOAD_AUDIO: &str = "ytdlp_download_audio";

/// Provider for yt-dlp video tools (executed in sandbox)
pub struct YtdlpProvider {
    exec: Arc<dyn SandboxExec>,
    fileops: Arc<dyn SandboxFileOps>,
    progress_tx: Option<Sender<AgentEvent>>,
    cleanup_started: Arc<AtomicBool>,
}

impl YtdlpProvider {
    /// Create a new YtdlpProvider (sandbox is lazily initialized)
    #[must_use]
    pub fn new(sandbox_scope: impl Into<SandboxScope>) -> Self {
        Self::from_runtime(Arc::new(SandboxRuntime::new(sandbox_scope.into())))
    }

    /// Create a provider from the shared sandbox runtime.
    #[must_use]
    pub fn from_runtime(runtime: Arc<SandboxRuntime>) -> Self {
        let exec: Arc<dyn SandboxExec> = Arc::<SandboxRuntime>::clone(&runtime);
        let fileops: Arc<dyn SandboxFileOps> = runtime;
        Self::with_sandbox_backends(exec, fileops)
    }

    /// Create a provider from narrow sandbox capability traits.
    #[must_use]
    pub fn with_sandbox_backends(
        exec: Arc<dyn SandboxExec>,
        fileops: Arc<dyn SandboxFileOps>,
    ) -> Self {
        Self {
            exec,
            fileops,
            progress_tx: None,
            cleanup_started: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set the progress channel for sending events (like file transfers)
    #[must_use]
    pub fn with_progress_tx(mut self, tx: Sender<AgentEvent>) -> Self {
        self.progress_tx = Some(tx);
        self
    }

    /// Build native typed runtime executors for yt-dlp tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        let execution_lock = Arc::new(Mutex::new(()));
        Self::tool_definitions()
            .into_iter()
            .map(|spec| {
                Arc::new(YtdlpToolExecutor {
                    provider: Arc::clone(self),
                    name: ToolName::from(spec.name.clone()),
                    spec,
                    execution_lock: Arc::clone(&execution_lock),
                }) as Arc<dyn ToolExecutor>
            })
            .collect()
    }

    /// Ensure the downloads directory exists.
    async fn ensure_downloads_dir(&self) -> Result<()> {
        self.exec
            .exec(&format!("mkdir -p {DOWNLOADS_DIR}"), None)
            .await?;

        if !self.cleanup_started.swap(true, Ordering::AcqRel) {
            let exec = Arc::clone(&self.exec);
            tokio::spawn(async move {
                cleanup_old_downloads(exec).await;
            });
        }

        Ok(())
    }

    /// Send file to user with automatic cleanup after successful delivery
    async fn send_file_with_cleanup(&self, file_path: &str, file_name: &str) -> Result<String> {
        // Download file from sandbox
        let content = match self.fileops.read_file(file_path).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(format!(
                    "❌ Failed to read file from sandbox: {e}\n\n\
                     File path: {file_path}"
                ));
            }
        };

        let size_mb = content.len() as f64 / 1024.0 / 1024.0;
        let report = deliver_file_via_progress(
            self.progress_tx.as_ref(),
            FileDeliveryRequest {
                kind: FileDeliveryKind::Auto,
                file_name: file_name.to_string(),
                content,
                source_path: file_path.to_string(),
            },
        )
        .await;

        match report.status {
            FileDeliveryStatus::Delivered => {
                info!(file_path = %file_path, "File delivered successfully, cleaning up");
                if let Err(e) = self.exec.exec(&format!("rm -f '{file_path}'"), None).await {
                    warn!(error = %e, file_path = %file_path, "Failed to cleanup file after delivery");
                }
                Ok(format!(
                    "✅ File '{file_name}' ({size_mb:.2} MB) sent to user successfully"
                ))
            }
            FileDeliveryStatus::DeliveryFailed(error) => {
                warn!(error = %error, file_path = %file_path, "File delivery failed after retries");
                Ok(format!(
                    "⚠️ Failed to send file to user: {error}\n\
                     File remains in sandbox at: {file_path}\n\
                     You can retry using `send_file_to_user` tool."
                ))
            }
            FileDeliveryStatus::ConfirmationChannelClosed => Ok(format!(
                "⚠️ File delivery status unknown (channel closed)\n\
                 File remains in sandbox at: {file_path}"
            )),
            FileDeliveryStatus::TimedOut => Ok(format!(
                "⚠️ File delivery timed out (2 minutes)\n\
                 File remains in sandbox at: {file_path}"
            )),
            FileDeliveryStatus::QueueUnavailable(error) => Ok(format!(
                "⚠️ File downloaded ({size_mb:.2} MB) but failed to queue for sending: {error}\n\
                 Path: {file_path}\n\
                 Use `send_file_to_user` tool to send it manually."
            )),
            FileDeliveryStatus::EmptyContent => Ok(format!(
                "❌ File '{file_name}' is empty and cannot be sent\n\
                 Path: {file_path}"
            )),
        }
    }

    /// Execute yt-dlp command and return output
    async fn exec_ytdlp(
        &self,
        args: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        let cmd = format!("yt-dlp {args}");
        debug!(cmd = %cmd, "Executing yt-dlp command");

        let result = self.exec.exec(&cmd, cancellation_token).await?;

        if result.success() {
            Ok(result.stdout)
        } else {
            let error_msg = if result.stderr.is_empty() {
                result.stdout
            } else {
                result.stderr
            };

            // Check if this is a fatal, unrecoverable error
            if is_fatal_ytdlp_error(&error_msg) {
                warn!(error = %error_msg, "Fatal yt-dlp error detected");
                anyhow::bail!("yt-dlp fatal error: {error_msg}")
            }

            // Check if this is a retryable error (network issues, etc.)
            if is_retryable_ytdlp_error(&error_msg) {
                warn!(error = %error_msg, "Retryable yt-dlp error detected");
                return Ok(format!(
                    "⚠️ Temporary yt-dlp error (possible retry): {error_msg}"
                ));
            }

            // Non-fatal, non-retryable errors (e.g., format not available)
            // return as Ok with warning so agent can adjust
            Ok(format!("yt-dlp warning: {error_msg}"))
        }
    }

    /// Handle ytdlp_get_video_metadata tool
    async fn handle_get_metadata(
        &self,
        arguments: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        let args: GetMetadataArgs = serde_json::from_str(arguments)?;

        // Build field selection if specified
        let fields_arg = if let Some(ref fields) = args.fields {
            let field_list = fields.join(",");
            format!("-O '%({field_list})j'")
        } else {
            // Default: dump full JSON metadata
            "-j".to_string()
        };

        let ytdlp_args = format!(
            "--no-download --no-warnings --ignore-errors {} '{}'",
            fields_arg,
            args.url.replace('\'', "'\\''")
        );

        let output = match self.exec_ytdlp(&ytdlp_args, cancellation_token).await {
            Ok(out) => out,
            Err(e) => {
                return Ok(format!(
                    "❌ **Failed to retrieve video metadata**\n\n\
                     Reason: {e}\n\n\
                     This may mean the video is unavailable, private, \
                     blocked in your region, or requires authentication."
                ));
            }
        };

        // Truncate if too long
        let truncated = if output.len() > MAX_METADATA_LENGTH {
            format!(
                "{}...\n\n(truncated, {} chars total)",
                &output[..MAX_METADATA_LENGTH],
                output.len()
            )
        } else {
            output
        };

        Ok(format!("## Video Metadata\n\n```json\n{truncated}\n```"))
    }

    /// Handle ytdlp_download_transcript tool
    async fn handle_download_transcript(
        &self,
        arguments: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        let args: TranscriptArgs = serde_json::from_str(arguments)?;

        let lang = args.language.as_deref().unwrap_or("en");
        let url_escaped = args.url.replace('\'', "'\\''");

        // Download subtitles in VTT format, then convert to plain text
        let ytdlp_args = format!(
            "--skip-download --write-auto-sub --sub-lang '{lang}' \
             --sub-format vtt --convert-subs srt \
             -o '{DOWNLOADS_DIR}/transcript.%(ext)s' \
             --no-warnings '{url_escaped}'"
        );

        if let Err(e) = self.exec_ytdlp(&ytdlp_args, cancellation_token).await {
            return Ok(format!(
                "❌ **Failed to download transcript**\n\n\
                 Reason: {e}\n\n\
                 The video may be unavailable or have no subtitles."
            ));
        }

        // Try to find the subtitle file
        let find_result = self
            .exec
            .exec(
                &format!("find {DOWNLOADS_DIR} -name '*.srt' -o -name '*.vtt' | head -1"),
                None,
            )
            .await?;

        let subtitle_path = find_result.stdout.trim();
        if subtitle_path.is_empty() {
            return Ok("No subtitles/transcript available for this video. The video might not have captions or auto-generated subtitles.".to_string());
        }

        // Read and clean the transcript (remove timestamps and formatting)
        let clean_cmd = format!(
            "cat '{}' | sed '/^[0-9]/d' | sed '/-->/d' | sed '/^$/d' | tr '\\n' ' '",
            subtitle_path
        );
        let result = self.exec.exec(&clean_cmd, None).await?;

        // Clean up
        self.exec
            .exec(&format!("rm -f {DOWNLOADS_DIR}/transcript.*"), None)
            .await?;

        let transcript = result.stdout.trim().to_string();
        if transcript.is_empty() {
            return Ok("Transcript is empty or could not be extracted.".to_string());
        }

        // Truncate if needed
        let truncated = if transcript.len() > MAX_TRANSCRIPT_LENGTH {
            format!(
                "{}...\n\n(truncated, {} chars total)",
                &transcript[..MAX_TRANSCRIPT_LENGTH],
                transcript.len()
            )
        } else {
            transcript
        };

        Ok(format!("## Transcript\n\n{truncated}"))
    }

    /// Handle ytdlp_search_videos tool
    async fn handle_search_videos(
        &self,
        arguments: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        let args: SearchVideosArgs = serde_json::from_str(arguments)?;

        let max_results = args.max_results.unwrap_or(5).min(20);
        let query_escaped = args.query.replace('\'', "'\\''");

        // Use ytsearch to search YouTube
        let ytdlp_args =
            format!("-j --flat-playlist --no-warnings 'ytsearch{max_results}:{query_escaped}'");

        let output = match self.exec_ytdlp(&ytdlp_args, cancellation_token).await {
            Ok(out) => out,
            Err(e) => {
                return Ok(format!(
                    "❌ **Failed to execute video search**\n\n\
                     Reason: {e}\n\n\
                     Possible temporary issues with YouTube access."
                ));
            }
        };

        if output.starts_with("yt-dlp error:") || output.starts_with("yt-dlp warning:") {
            return Ok(output);
        }

        // Parse NDJSON output and format results
        let mut results = String::new();
        let _ = writeln!(results, "## Search Results for: {}\n", args.query);

        for (i, line) in output.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }

            if let Ok(video) = serde_json::from_str::<serde_json::Value>(line) {
                let title = video["title"].as_str().unwrap_or("Unknown");
                let channel = video["channel"].as_str().unwrap_or("Unknown");
                let duration = video["duration_string"]
                    .as_str()
                    .or_else(|| video["duration"].as_i64().map(|_| "N/A"))
                    .unwrap_or("N/A");
                let url = video["url"]
                    .as_str()
                    .or_else(|| video["webpage_url"].as_str())
                    .unwrap_or("");

                let _ = writeln!(results, "### {}. {}", i + 1, title);
                let _ = writeln!(results, "- **Channel**: {channel}");
                let _ = writeln!(results, "- **Duration**: {duration}");
                if !url.is_empty() {
                    let _ = writeln!(results, "- **URL**: {url}");
                }
                let _ = writeln!(results);
            }
        }

        if results.lines().count() <= 2 {
            return Ok("No videos found for this query.".to_string());
        }

        Ok(results)
    }

    /// Handle ytdlp_download_video tool
    async fn handle_download_video(
        &self,
        arguments: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        let args: DownloadVideoArgs = serde_json::from_str(arguments)?;

        let resolution = args.resolution.as_deref().unwrap_or("720");
        let url_escaped = args.url.replace('\'', "'\\''");

        // Format selection based on resolution
        let format_arg = match resolution {
            "480" | "480p" => "bestvideo[height<=480]+bestaudio/best[height<=480]",
            "720" | "720p" => "bestvideo[height<=720]+bestaudio/best[height<=720]",
            "1080" | "1080p" => "bestvideo[height<=1080]+bestaudio/best[height<=1080]",
            "best" => "bestvideo+bestaudio/best",
            _ => "bestvideo[height<=720]+bestaudio/best[height<=720]",
        };

        let mut ytdlp_args = format!(
            "-f '{format_arg}' --merge-output-format mp4 \
             -o '{DOWNLOADS_DIR}/%(title).50s.%(ext)s' \
             --no-warnings --progress '{url_escaped}'"
        );

        // Add time range if specified
        if let Some(ref start) = args.start_time {
            ytdlp_args.push_str(&format!(" --download-sections '*{start}-'"));
        }
        if let Some(ref end) = args.end_time {
            let start = args.start_time.as_deref().unwrap_or("0");
            ytdlp_args = ytdlp_args.replace(&format!("'*{start}-'"), &format!("'*{start}-{end}'"));
        }

        let output = match self.exec_ytdlp(&ytdlp_args, cancellation_token).await {
            Ok(out) => out,
            Err(e) => {
                return Ok(format!(
                    "❌ **Failed to download video**\n\n\
                     Reason: {e}\n\n\
                     The video may be unavailable, private, or blocked."
                ));
            }
        };

        if output.contains("yt-dlp error:") || output.contains("ERROR") {
            return Ok(format!("Download failed: {output}"));
        }

        // Find the downloaded file
        let find_result = self
            .exec
            .exec(
                &format!("ls -1t {DOWNLOADS_DIR}/*.mp4 2>/dev/null | head -1"),
                None,
            )
            .await?;

        let video_path = find_result.stdout.trim();
        if video_path.is_empty() {
            return Ok(
                "Video download completed but file not found. Try checking the sandbox files."
                    .to_string(),
            );
        }

        // Get file size
        let size_bytes = self
            .fileops
            .file_size_bytes(video_path, None)
            .await
            .unwrap_or(0);
        let size_mb = size_bytes as f64 / 1024.0 / 1024.0;

        let filename = std::path::Path::new(video_path)
            .file_name()
            .map_or("video.mp4".to_string(), |n| n.to_string_lossy().to_string());

        if args.send_to_user {
            // Auto-send to user with confirmation for cleanup
            return self.send_file_with_cleanup(video_path, &filename).await;
        }

        Ok(format!(
            "Video downloaded successfully!\n\n\
             - **File**: {filename}\n\
             - **Path**: {video_path}\n\
             - **Size**: {size_mb:.2} MB\n\n\
             Use `send_file_to_user` tool with path `{video_path}` to send it to the user."
        ))
    }

    /// Handle ytdlp_download_audio tool
    async fn handle_download_audio(
        &self,
        arguments: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        let args: DownloadAudioArgs = serde_json::from_str(arguments)?;

        let url_escaped = args.url.replace('\'', "'\\''");

        // Extract best audio and convert to mp3
        let ytdlp_args = format!(
            "-x --audio-format mp3 --audio-quality 0 \
             -o '{DOWNLOADS_DIR}/%(title).50s.%(ext)s' \
             --no-warnings --progress '{url_escaped}'"
        );

        let output = match self.exec_ytdlp(&ytdlp_args, cancellation_token).await {
            Ok(out) => out,
            Err(e) => {
                return Ok(format!(
                    "❌ **Failed to extract audio**\n\n\
                     Reason: {e}\n\n\
                     Video may be unavailable, private, or blocked."
                ));
            }
        };

        if output.contains("yt-dlp error:") || output.contains("ERROR") {
            return Ok(format!("Audio extraction failed: {output}"));
        }

        // Find the downloaded file
        let find_result = self
            .exec
            .exec(
                &format!("ls -1t {DOWNLOADS_DIR}/*.mp3 2>/dev/null | head -1"),
                None,
            )
            .await?;

        let audio_path = find_result.stdout.trim();
        if audio_path.is_empty() {
            return Ok(
                "Audio extraction completed but file not found. Try checking the sandbox files."
                    .to_string(),
            );
        }

        // Get file size
        let size_bytes = self
            .fileops
            .file_size_bytes(audio_path, None)
            .await
            .unwrap_or(0);
        let size_mb = size_bytes as f64 / 1024.0 / 1024.0;

        let filename = std::path::Path::new(audio_path)
            .file_name()
            .map_or("audio.mp3".to_string(), |n| n.to_string_lossy().to_string());

        if args.send_to_user {
            // Auto-send to user with confirmation for cleanup
            return self.send_file_with_cleanup(audio_path, &filename).await;
        }

        Ok(format!(
            "Audio extracted successfully!\n\n\
             - **File**: {filename}\n\
             - **Path**: {audio_path}\n\
             - **Size**: {size_mb:.2} MB\n\n\
             Use `send_file_to_user` tool with path `{audio_path}` to send it to the user."
        ))
    }
}

// ============================================================================
// Argument structs
// ============================================================================

#[derive(Debug, Deserialize)]
struct GetMetadataArgs {
    url: String,
    #[serde(default)]
    fields: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct TranscriptArgs {
    url: String,
    #[serde(default)]
    language: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchVideosArgs {
    query: String,
    #[serde(default)]
    max_results: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct DownloadVideoArgs {
    url: String,
    #[serde(default)]
    resolution: Option<String>,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    end_time: Option<String>,
    /// Automatically send to user after download (default: true)
    #[serde(default = "default_true")]
    send_to_user: bool,
}

#[derive(Debug, Deserialize)]
struct DownloadAudioArgs {
    url: String,
    /// Automatically send to user after download (default: true)
    #[serde(default = "default_true")]
    send_to_user: bool,
}

fn default_true() -> bool {
    true
}

// ============================================================================
// Tool Definitions - Split into multiple functions to satisfy clippy
// ============================================================================

impl YtdlpProvider {
    fn tool_definitions() -> Vec<ToolDefinition> {
        vec![
            Self::get_metadata_tool(),
            Self::get_transcript_tool(),
            Self::get_search_tool(),
            Self::get_download_video_tool(),
            Self::get_download_audio_tool(),
        ]
    }

    fn get_metadata_tool() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_YTDLP_GET_METADATA.to_string(),
            description: "Get comprehensive metadata for a video from YouTube or other supported platforms. Returns JSON with title, channel, duration, views, upload date, description, tags, and more. No video download required.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Video URL (YouTube, Vimeo, Facebook, etc.)"
                    },
                    "fields": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional: specific fields to extract (e.g., ['title', 'channel', 'duration', 'view_count'])"
                    }
                },
                "required": ["url"]
            }),
        }
    }

    fn get_transcript_tool() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_YTDLP_DOWNLOAD_TRANSCRIPT.to_string(),
            description: "Download and extract clean text transcript from a video. Supports auto-generated and manual subtitles. Returns plain text without timestamps.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Video URL"
                    },
                    "language": {
                        "type": "string",
                        "description": "Subtitle language code (default: 'en'). Examples: 'en', 'ru', 'es', 'zh-Hans'"
                    }
                },
                "required": ["url"]
            }),
        }
    }

    fn get_search_tool() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_YTDLP_SEARCH_VIDEOS.to_string(),
            description: "Search for videos on YouTube. Returns list of videos with titles, channels, durations, and URLs.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results (1-20, default: 5)"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    fn get_download_video_tool() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_YTDLP_DOWNLOAD_VIDEO.to_string(),
            description: "Download a video from YouTube or other platforms. By default, automatically sends the file to the user and cleans up after successful delivery. Set send_to_user=false to keep the file in sandbox for further processing.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Video URL"
                    },
                    "resolution": {
                        "type": "string",
                        "description": "Video resolution: '480', '720', '1080', or 'best' (default: '720')"
                    },
                    "start_time": {
                        "type": "string",
                        "description": "Optional start time for trimming (format: 'MM:SS' or 'HH:MM:SS')"
                    },
                    "end_time": {
                        "type": "string",
                        "description": "Optional end time for trimming (format: 'MM:SS' or 'HH:MM:SS')"
                    },
                    "send_to_user": {
                        "type": "boolean",
                        "description": "Automatically send file to user after download (default: true). Set to false if you need to process the file first.",
                        "default": true
                    }
                },
                "required": ["url"]
            }),
        }
    }

    fn get_download_audio_tool() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_YTDLP_DOWNLOAD_AUDIO.to_string(),
            description: "Extract and download audio from a video as MP3. By default, automatically sends the file to the user and cleans up after successful delivery. Set send_to_user=false to keep the file in sandbox for further processing.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Video URL"
                    },
                    "send_to_user": {
                        "type": "boolean",
                        "description": "Automatically send file to user after download (default: true). Set to false if you need to process the file first.",
                        "default": true
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute_tool(
        &self,
        tool_name: &str,
        arguments: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        debug!(tool = tool_name, "Executing ytdlp tool");

        // Ensure the sandbox-backed downloads directory exists.
        if let Err(e) = self.ensure_downloads_dir().await {
            warn!(error = %e, "Failed to initialize sandbox for ytdlp");
            return Ok(format!("Failed to initialize sandbox: {e}"));
        }

        match tool_name {
            TOOL_YTDLP_GET_METADATA => {
                self.handle_get_metadata(arguments, cancellation_token)
                    .await
            }
            TOOL_YTDLP_DOWNLOAD_TRANSCRIPT => {
                self.handle_download_transcript(arguments, cancellation_token)
                    .await
            }
            TOOL_YTDLP_SEARCH_VIDEOS => {
                self.handle_search_videos(arguments, cancellation_token)
                    .await
            }
            TOOL_YTDLP_DOWNLOAD_VIDEO => {
                self.handle_download_video(arguments, cancellation_token)
                    .await
            }
            TOOL_YTDLP_DOWNLOAD_AUDIO => {
                self.handle_download_audio(arguments, cancellation_token)
                    .await
            }
            _ => anyhow::bail!("Unknown ytdlp tool: {tool_name}"),
        }
    }
}

struct YtdlpToolExecutor {
    provider: Arc<YtdlpProvider>,
    name: ToolName,
    spec: ToolDefinition,
    execution_lock: Arc<Mutex<()>>,
}

#[async_trait]
impl ToolExecutor for YtdlpToolExecutor {
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
                Some(&invocation.cancellation_token),
            )
            .await
            .map(|output| normalizer.success(&invocation, &output, ""))
            .map_err(ytdlp_runtime_error)
    }
}

fn ytdlp_runtime_error(error: anyhow::Error) -> ToolRuntimeError {
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
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolOutputStatus, ToolTimeoutConfig, TurnId,
    };
    use crate::llm::InvocationId;
    use crate::sandbox::{
        ExecResult, SandboxBackend, SandboxBackendId, SandboxCapability, SandboxFileListing,
    };
    use chrono::Utc;
    use std::sync::Mutex as StdMutex;
    use tokio_util::sync::CancellationToken;

    #[derive(Default)]
    struct FakeSandbox {
        commands: StdMutex<Vec<String>>,
    }

    impl FakeSandbox {
        fn commands(&self) -> Vec<String> {
            self.commands
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    impl SandboxBackend for FakeSandbox {
        fn id(&self) -> SandboxBackendId {
            SandboxBackendId::new("sandbox/fake-ytdlp")
        }

        fn capabilities(&self) -> &'static [SandboxCapability] {
            &[SandboxCapability::Exec, SandboxCapability::FileOps]
        }
    }

    #[async_trait]
    impl SandboxExec for FakeSandbox {
        async fn exec(
            &self,
            command: &str,
            _cancellation_token: Option<&CancellationToken>,
        ) -> Result<ExecResult> {
            self.commands
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(command.to_string());

            let stdout = if command.contains("ytsearch") {
                r#"{"title":"RustConf talk","channel":"Rust Project","duration_string":"10:00","webpage_url":"https://youtube.test/watch?v=abc"}"#
                    .to_string()
            } else if command.contains("find /workspace/downloads -type f -mtime +7") {
                "0\n".to_string()
            } else {
                String::new()
            };

            Ok(ExecResult {
                stdout,
                stderr: String::new(),
                exit_code: 0,
            })
        }
    }

    #[async_trait]
    impl SandboxFileOps for FakeSandbox {
        async fn write_file(&self, _path: &str, _bytes: &[u8]) -> Result<()> {
            Ok(())
        }

        async fn read_file(&self, _path: &str) -> Result<Vec<u8>> {
            Ok(Vec::new())
        }

        async fn file_size_bytes(
            &self,
            _path: &str,
            _cancellation_token: Option<&CancellationToken>,
        ) -> Result<u64> {
            Ok(0)
        }

        async fn list_files(&self, _path: &str) -> Result<SandboxFileListing> {
            Ok(SandboxFileListing {
                path: DOWNLOADS_DIR.to_string(),
                listing: String::new(),
                stderr: String::new(),
                exit_code: 0,
            })
        }
    }

    fn runtime_invocation(tool_name: &str, raw_arguments: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(77),
            turn_id: TurnId::from("turn-ytdlp"),
            batch_id: ToolBatchId::from("batch-ytdlp"),
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
    fn typed_runtime_executors_register_ytdlp_tools() {
        let sandbox = Arc::new(FakeSandbox::default());
        let exec: Arc<dyn SandboxExec> = Arc::<FakeSandbox>::clone(&sandbox);
        let fileops: Arc<dyn SandboxFileOps> = sandbox;
        let provider = Arc::new(YtdlpProvider::with_sandbox_backends(exec, fileops));
        let names = provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.name().as_str().to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                TOOL_YTDLP_GET_METADATA,
                TOOL_YTDLP_DOWNLOAD_TRANSCRIPT,
                TOOL_YTDLP_SEARCH_VIDEOS,
                TOOL_YTDLP_DOWNLOAD_VIDEO,
                TOOL_YTDLP_DOWNLOAD_AUDIO,
            ]
        );
    }

    #[tokio::test]
    async fn typed_runtime_executor_searches_videos_with_fake_sandbox() {
        let sandbox = Arc::new(FakeSandbox::default());
        let exec: Arc<dyn SandboxExec> = Arc::<FakeSandbox>::clone(&sandbox);
        let fileops: Arc<dyn SandboxFileOps> = Arc::<FakeSandbox>::clone(&sandbox);
        let provider = Arc::new(YtdlpProvider::with_sandbox_backends(exec, fileops));
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == TOOL_YTDLP_SEARCH_VIDEOS)
            .expect("typed yt-dlp search executor registered");

        let output = executor
            .execute(runtime_invocation(
                TOOL_YTDLP_SEARCH_VIDEOS,
                r#"{"query":"rust talk","max_results":1}"#,
            ))
            .await
            .expect("typed yt-dlp search succeeds");

        assert_eq!(output.status, ToolOutputStatus::Success);
        let stdout = output.stdout.text.as_deref().expect("stdout text");
        assert!(stdout.contains("## Search Results for: rust talk"));
        assert!(stdout.contains("RustConf talk"));
        assert!(stdout.contains("https://youtube.test/watch?v=abc"));
        assert!(sandbox
            .commands()
            .iter()
            .any(|command| command.contains("ytsearch1:rust talk")));
    }
}
