//! YT-DLP Provider - video platform tools via yt-dlp in sandbox
//!
//! Provides tools for video metadata extraction, transcript download,
//! video search, and media download from YouTube and other platforms.
//!
//! All operations execute inside the Docker sandbox where yt-dlp is installed.

use crate::agent::progress::AgentEvent;
use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use crate::sandbox::SandboxManager;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::fmt::Write;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

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

/// Maximum character limit for transcript output (to avoid LLM context overflow)
const MAX_TRANSCRIPT_LENGTH: usize = 50_000;

/// Maximum character limit for metadata output
const MAX_METADATA_LENGTH: usize = 25_000;

/// Directory inside sandbox for downloaded media
const DOWNLOADS_DIR: &str = "/workspace/downloads";

/// Provider for yt-dlp video tools (executed in sandbox)
pub struct YtdlpProvider {
    sandbox: Arc<Mutex<Option<SandboxManager>>>,
    user_id: i64,
    progress_tx: Option<Sender<AgentEvent>>,
}

impl YtdlpProvider {
    /// Create a new YtdlpProvider (sandbox is lazily initialized)
    #[must_use]
    pub fn new(user_id: i64) -> Self {
        Self {
            sandbox: Arc::new(Mutex::new(None)),
            user_id,
            progress_tx: None,
        }
    }

    /// Set the progress channel for sending events (like file transfers)
    #[must_use]
    pub fn with_progress_tx(mut self, tx: Sender<AgentEvent>) -> Self {
        self.progress_tx = Some(tx);
        self
    }

    /// Ensure sandbox is running
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

        debug!(user_id = self.user_id, "Creating sandbox for YtdlpProvider");
        let mut sandbox = SandboxManager::new(self.user_id).await?;
        sandbox.create_sandbox().await?;

        // Create downloads directory
        sandbox
            .exec_command(&format!("mkdir -p {DOWNLOADS_DIR}"), None)
            .await?;

        // Cleanup old downloads (files older than 7 days) on sandbox init
        // This runs at most once per sandbox lifecycle
        tokio::spawn({
            let sb = sandbox.clone();
            async move {
                if let Ok(count) = sb.cleanup_old_downloads().await {
                    if count > 0 {
                        debug!(
                            files_deleted = count,
                            "Cleaned up old download files on init"
                        );
                    }
                }
            }
        });

        *self.sandbox.lock().await = Some(sandbox);
        Ok(())
    }

    /// Get sandbox reference
    async fn get_sandbox(&self) -> Result<SandboxManager> {
        let guard = self.sandbox.lock().await;
        guard
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Sandbox not initialized"))
    }

    /// Send file to user with automatic cleanup after successful delivery
    async fn send_file_with_cleanup(
        &self,
        sandbox: &SandboxManager,
        file_path: &str,
        file_name: &str,
    ) -> Result<String> {
        // Download file from sandbox
        let content = match sandbox.download_file(file_path).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(format!(
                    "❌ Failed to read file from sandbox: {e}\n\n\
                     File path: {file_path}"
                ));
            }
        };

        let size_mb = content.len() as f64 / 1024.0 / 1024.0;

        if let Some(ref tx) = self.progress_tx {
            // Create oneshot channel for delivery confirmation
            let (confirm_tx, confirm_rx) = tokio::sync::oneshot::channel();

            // Send file with confirmation request
            if let Err(e) = tx
                .send(AgentEvent::FileToSendWithConfirmation {
                    file_name: file_name.to_string(),
                    content,
                    sandbox_path: file_path.to_string(),
                    confirmation_tx: confirm_tx,
                })
                .await
            {
                warn!(error = %e, "Failed to send FileToSendWithConfirmation event");
                return Ok(format!(
                    "⚠️ File downloaded ({size_mb:.2} MB) but failed to queue for sending: {e}\n\
                     Path: {file_path}"
                ));
            }

            // Wait for confirmation with timeout (2 minutes)
            match tokio::time::timeout(std::time::Duration::from_secs(120), confirm_rx).await {
                Ok(Ok(Ok(()))) => {
                    // Success! Delete file from sandbox
                    info!(file_path = %file_path, "File delivered successfully, cleaning up");
                    if let Err(e) = sandbox
                        .exec_command(&format!("rm -f '{file_path}'"), None)
                        .await
                    {
                        warn!(error = %e, file_path = %file_path, "Failed to cleanup file after delivery");
                    }
                    Ok(format!(
                        "✅ File '{file_name}' ({size_mb:.2} MB) sent to user successfully"
                    ))
                }
                Ok(Ok(Err(e))) => {
                    // Delivery failed after retries
                    warn!(error = %e, file_path = %file_path, "File delivery failed after retries");
                    Ok(format!(
                        "⚠️ Failed to send file to user: {e}\n\
                         File remains in sandbox at: {file_path}\n\
                         You can retry using `send_file_to_user` tool."
                    ))
                }
                Ok(Err(_)) => {
                    // Channel closed unexpectedly
                    warn!(file_path = %file_path, "Confirmation channel closed unexpectedly");
                    Ok(format!(
                        "⚠️ File delivery status unknown (channel closed)\n\
                         File remains in sandbox at: {file_path}"
                    ))
                }
                Err(_) => {
                    // Timeout
                    warn!(file_path = %file_path, "File delivery confirmation timeout");
                    Ok(format!(
                        "⚠️ File delivery timed out (2 minutes)\n\
                         File remains in sandbox at: {file_path}"
                    ))
                }
            }
        } else {
            warn!("Progress channel not available for file delivery");
            Ok(format!(
                "⚠️ File downloaded ({size_mb:.2} MB) but progress channel not available\n\
                 Path: {file_path}\n\
                 Use `send_file_to_user` tool to send it manually."
            ))
        }
    }

    /// Execute yt-dlp command and return output
    async fn exec_ytdlp(
        &self,
        args: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        let sandbox = self.get_sandbox().await?;
        let cmd = format!("yt-dlp {args}");
        debug!(cmd = %cmd, "Executing yt-dlp command");

        let result = sandbox.exec_command(&cmd, cancellation_token).await?;

        if result.success() {
            Ok(result.stdout)
        } else {
            let error_msg = if result.stderr.is_empty() {
                result.stdout.clone()
            } else {
                result.stderr.clone()
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

        // Read and clean transcript
        let sandbox = self.get_sandbox().await?;

        // Try to find the subtitle file
        let find_result = sandbox
            .exec_command(
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
        let result = sandbox.exec_command(&clean_cmd, None).await?;

        // Clean up
        sandbox
            .exec_command(&format!("rm -f {DOWNLOADS_DIR}/transcript.*"), None)
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

        if output.starts_with("yt-dlp error:") || output.starts_with("yt-dlp warning:")
        {
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
        let sandbox = self.get_sandbox().await?;
        let find_result = sandbox
            .exec_command(
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
        let size_result = sandbox
            .exec_command(&format!("stat -c %s '{video_path}'"), None)
            .await?;
        let size_bytes: u64 = size_result.stdout.trim().parse().unwrap_or(0);
        let size_mb = size_bytes as f64 / 1024.0 / 1024.0;

        let filename = std::path::Path::new(video_path)
            .file_name()
            .map_or("video.mp4".to_string(), |n| n.to_string_lossy().to_string());

        if args.send_to_user {
            // Auto-send to user with confirmation for cleanup
            return self
                .send_file_with_cleanup(&sandbox, video_path, &filename)
                .await;
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
        let sandbox = self.get_sandbox().await?;
        let find_result = sandbox
            .exec_command(
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
        let size_result = sandbox
            .exec_command(&format!("stat -c %s '{audio_path}'"), None)
            .await?;
        let size_bytes: u64 = size_result.stdout.trim().parse().unwrap_or(0);
        let size_mb = size_bytes as f64 / 1024.0 / 1024.0;

        let filename = std::path::Path::new(audio_path)
            .file_name()
            .map_or("audio.mp3".to_string(), |n| n.to_string_lossy().to_string());

        if args.send_to_user {
            // Auto-send to user with confirmation for cleanup
            return self
                .send_file_with_cleanup(&sandbox, audio_path, &filename)
                .await;
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
    fn get_metadata_tool() -> ToolDefinition {
        ToolDefinition {
            name: "ytdlp_get_video_metadata".to_string(),
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
            name: "ytdlp_download_transcript".to_string(),
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
            name: "ytdlp_search_videos".to_string(),
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
            name: "ytdlp_download_video".to_string(),
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
            name: "ytdlp_download_audio".to_string(),
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
}

// ============================================================================
// ToolProvider implementation
// ============================================================================

#[async_trait]
impl ToolProvider for YtdlpProvider {
    fn name(&self) -> &'static str {
        "ytdlp"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            Self::get_metadata_tool(),
            Self::get_transcript_tool(),
            Self::get_search_tool(),
            Self::get_download_video_tool(),
            Self::get_download_audio_tool(),
        ]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            "ytdlp_get_video_metadata"
                | "ytdlp_download_transcript"
                | "ytdlp_search_videos"
                | "ytdlp_download_video"
                | "ytdlp_download_audio"
        )
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        debug!(tool = tool_name, "Executing ytdlp tool");

        // Ensure sandbox is running
        if let Err(e) = self.ensure_sandbox().await {
            warn!(error = %e, "Failed to initialize sandbox for ytdlp");
            return Ok(format!("Failed to initialize sandbox: {e}"));
        }

        match tool_name {
            "ytdlp_get_video_metadata" => {
                self.handle_get_metadata(arguments, cancellation_token)
                    .await
            }
            "ytdlp_download_transcript" => {
                self.handle_download_transcript(arguments, cancellation_token)
                    .await
            }
            "ytdlp_search_videos" => {
                self.handle_search_videos(arguments, cancellation_token)
                    .await
            }
            "ytdlp_download_video" => {
                self.handle_download_video(arguments, cancellation_token)
                    .await
            }
            "ytdlp_download_audio" => {
                self.handle_download_audio(arguments, cancellation_token)
                    .await
            }
            _ => anyhow::bail!("Unknown ytdlp tool: {tool_name}"),
        }
    }
}
