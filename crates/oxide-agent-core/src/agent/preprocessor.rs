//! Input preprocessor for multimodal content
//!
//! Handles voice, image, and video preprocessing using the configured
//! multimodal model before passing to the agent for execution.

use super::providers::SandboxRuntime;
use crate::llm::LlmClient;
use crate::sandbox::{ExecResult, SandboxExec, SandboxFileOps, SandboxScope};
use anyhow::Result;
use std::sync::Arc;
use tracing::info;

/// Upload limit: 1 GB per session
const UPLOAD_LIMIT_BYTES: u64 = 1024 * 1024 * 1024;

/// Preprocessor for converting multimodal inputs to text
pub struct Preprocessor {
    llm_client: Arc<LlmClient>,
    sandbox_fileops: Arc<dyn SandboxFileOps>,
    sandbox_exec: Arc<dyn SandboxExec>,
}

impl Preprocessor {
    /// Create a new preprocessor with the given LLM client and user ID
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use oxide_agent_core::agent::preprocessor::Preprocessor;
    /// use oxide_agent_core::config::AgentSettings;
    /// use oxide_agent_core::llm::LlmClient;
    ///
    /// let settings = AgentSettings::new().unwrap();
    /// let llm_client = Arc::new(LlmClient::new(&settings));
    /// let preprocessor = Preprocessor::new(llm_client, 123456789);
    /// ```
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>, sandbox_scope: impl Into<SandboxScope>) -> Self {
        let runtime = Arc::new(SandboxRuntime::new(sandbox_scope.into()));
        let sandbox_fileops: Arc<dyn SandboxFileOps> = Arc::<SandboxRuntime>::clone(&runtime);
        let sandbox_exec: Arc<dyn SandboxExec> = runtime;
        Self::with_sandbox_backends(llm_client, sandbox_fileops, sandbox_exec)
    }

    /// Create a preprocessor with explicit sandbox backends.
    #[must_use]
    pub fn with_sandbox_backends(
        llm_client: Arc<LlmClient>,
        sandbox_fileops: Arc<dyn SandboxFileOps>,
        sandbox_exec: Arc<dyn SandboxExec>,
    ) -> Self {
        Self {
            llm_client,
            sandbox_fileops,
            sandbox_exec,
        }
    }

    /// Transcribe voice audio to text using the configured multimodal model
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::sync::Arc;
    /// # use oxide_agent_core::agent::preprocessor::Preprocessor;
    /// # use oxide_agent_core::config::AgentSettings;
    /// # use oxide_agent_core::llm::LlmClient;
    /// # #[tokio::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// # let settings = AgentSettings::new().unwrap();
    /// # let llm_client = Arc::new(LlmClient::new(&settings));
    /// let preprocessor = Preprocessor::new(llm_client, 123456789);
    /// let audio_bytes = vec![0; 100];
    /// let text = preprocessor.transcribe_voice(audio_bytes, "audio/ogg").await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the transcription fails.
    pub async fn transcribe_voice(&self, audio_bytes: Vec<u8>, mime_type: &str) -> Result<String> {
        info!(
            "Transcribing voice message: {} bytes, mime: {mime_type}",
            audio_bytes.len()
        );

        let model_name = self
            .llm_client
            .resolve_media_model_name_for_audio_stt()
            .map_err(|e| anyhow::anyhow!("MEDIA_ROUTE_UNAVAILABLE: {e}"))?;

        let transcription = self
            .llm_client
            .transcribe_audio(audio_bytes, mime_type, &model_name)
            .await
            .map_err(|e| anyhow::anyhow!("Transcription failed: {e}"))?;

        info!("Transcription result: {} chars", transcription.len());
        Ok(transcription)
    }

    /// Describe an image for the agent context
    ///
    /// Generates a detailed description that the agent can use
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::sync::Arc;
    /// # use oxide_agent_core::agent::preprocessor::Preprocessor;
    /// # use oxide_agent_core::config::AgentSettings;
    /// # use oxide_agent_core::llm::LlmClient;
    /// # #[tokio::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// # let settings = AgentSettings::new().unwrap();
    /// # let llm_client = Arc::new(LlmClient::new(&settings));
    /// let preprocessor = Preprocessor::new(llm_client, 123456789);
    /// let image_bytes = vec![0; 100];
    /// let description = preprocessor.describe_image(image_bytes, Some("Explain this chart")).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the image analysis fails.
    pub async fn describe_image(
        &self,
        image_bytes: Vec<u8>,
        user_context: Option<&str>,
    ) -> Result<String> {
        info!("Describing image: {} bytes", image_bytes.len());

        let prompt = user_context.map_or_else(
            || {
                "Extract the content of this image for an AI agent. \
                     Include all important details, visible text/OCR, objects and their locations. \
                     Do not answer, translate, summarize, or otherwise perform a user request."
                    .to_string()
            },
            |ctx| {
                format!(
                    "Extract the content of this image for an AI agent that will perform the user's request later. \
                     User request for context only: {ctx}\n\
                     Include all important details, visible text/OCR, objects and their locations. \
                     Do not answer, translate, summarize, or otherwise perform the user request."
                )
            },
        );

        let system_prompt = "You are a visual analyzer for an AI agent. \
                            Your task is to extract image content and visible text so the agent can understand it without accessing the image itself. \
                            Do not perform the user's request; only describe the image content.";

        let model_name = self
            .llm_client
            .resolve_media_model_name_for_image()
            .map_err(|e| anyhow::anyhow!("MEDIA_ROUTE_UNAVAILABLE: {e}"))?;

        let description = self
            .llm_client
            .analyze_image(image_bytes, &prompt, system_prompt, &model_name)
            .await
            .map_err(|e| anyhow::anyhow!("Image analysis failed: {e}"))?;

        info!("Image description: {} chars", description.len());
        Ok(description)
    }

    /// Describe a video for the agent context.
    ///
    /// # Errors
    ///
    /// Returns an error if the video analysis fails.
    pub async fn describe_video(
        &self,
        video_bytes: Vec<u8>,
        mime_type: &str,
        user_context: Option<&str>,
    ) -> Result<String> {
        info!(
            "Describing video: {} bytes, mime: {mime_type}",
            video_bytes.len()
        );

        let prompt = user_context.map_or_else(
            || {
                "Extract the content of this video for an AI agent. Describe the sequence of events, visible text/OCR, spoken or implied context, and important objects or actions frame-to-frame. Do not answer, translate, summarize, or otherwise perform a user request."
                    .to_string()
            },
            |ctx| {
                format!(
                    "Extract the content of this video for an AI agent that will perform the user's request later. User request for context only: {ctx}\n\
                     Describe the sequence of events, visible text/OCR, spoken or implied context, and important objects or actions frame-to-frame. \
                     Do not answer, translate, summarize, or otherwise perform the user request."
                )
            },
        );

        let system_prompt = "You are a video analyzer for an AI agent. Your task is to extract video content and visible text so the agent can understand the timeline without accessing the video itself. Do not perform the user's request; only describe the video content.";

        let model_name = self
            .llm_client
            .resolve_media_model_name_for_video()
            .map_err(|e| anyhow::anyhow!("MEDIA_ROUTE_UNAVAILABLE: {e}"))?;

        let description = self
            .llm_client
            .analyze_video(video_bytes, mime_type, &prompt, system_prompt, &model_name)
            .await
            .map_err(|e| anyhow::anyhow!("Video analysis failed: {e}"))?;

        info!("Video description: {} chars", description.len());
        Ok(description)
    }

    /// Process a document uploaded by the user
    ///
    /// Uploads the file to the sandbox and returns a formatted description
    ///
    /// # Errors
    ///
    /// Returns an error if file upload fails or limit is exceeded.
    #[allow(clippy::cast_precision_loss)] // Reason: Precision loss is acceptable for display purposes in GB/MB/KB
    async fn process_document(
        &self,
        bytes: Vec<u8>,
        file_name: String,
        mime_type: Option<String>,
        caption: Option<String>,
    ) -> Result<String> {
        let upload_path = format!("/workspace/uploads/{}", Self::sanitize_filename(&file_name));

        // Check upload limit
        let current_size = self.current_uploads_size().await.unwrap_or(0);
        let new_size = current_size + bytes.len() as u64;

        if new_size > UPLOAD_LIMIT_BYTES {
            anyhow::bail!(
                "Upload limit exceeded: {:.1} GB / 1 GB. Recreate the container.",
                new_size as f64 / 1024.0 / 1024.0 / 1024.0
            );
        }

        self.sandbox_fileops
            .write_file(&upload_path, &bytes)
            .await?;

        let size_str = Self::format_file_size(bytes.len());
        let hint = Self::get_file_type_hint(&file_name);

        let mut parts = vec![
            "📎 **User uploaded a file:**".to_string(),
            format!("   Path: `{}`", upload_path),
            format!("   Size: {}", size_str),
        ];

        if let Some(mime_type) = &mime_type {
            parts.push(format!("   Type: {mime_type}"));
        }

        parts.push(String::new());
        parts.push(hint);

        parts.push(String::new());
        if let Some(caption) = caption {
            parts.push(format!("**Message:** {caption}"));
        } else {
            parts.push("_User did not leave a comment._".to_string());
        }

        Ok(parts.join("\n"))
    }

    async fn current_uploads_size(&self) -> Result<u64> {
        let result = self
            .sandbox_exec
            .exec("du -sb /workspace/uploads 2>/dev/null || echo '0'", None)
            .await?;
        Self::parse_uploads_size(&result)
    }

    fn parse_uploads_size(result: &ExecResult) -> Result<u64> {
        let size_str = result.stdout.split_whitespace().next().unwrap_or("0");
        size_str
            .parse::<u64>()
            .map_err(|error| anyhow::anyhow!("Failed to parse uploads size: {error}"))
    }

    /// Sanitize a filename by replacing dangerous characters
    #[must_use]
    fn sanitize_filename(name: &str) -> String {
        // Manually extract file name handling both / and \ as separators
        // This is necessary because Path::new uses OS-specific separators,
        // but we might receive paths from different OSs (e.g. Windows path on Linux container)
        let name = name.rsplit(['/', '\\']).next().unwrap_or(name);

        let name = if name.is_empty() { "file" } else { name };

        name.chars()
            .map(|c| match c {
                '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | ' ' => '_',
                _ => c,
            })
            .collect()
    }

    /// Format file size in human-readable format
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // Reason: Precision loss is acceptable for human-readable file sizes
    fn format_file_size(bytes: usize) -> String {
        const KB: usize = 1024;
        const MB: usize = KB * 1024;

        if bytes >= MB {
            format!("{:.1} MB", bytes as f64 / MB as f64)
        } else if bytes >= KB {
            format!("{:.1} KB", bytes as f64 / KB as f64)
        } else {
            format!("{bytes} B")
        }
    }

    /// Get a hint about how to handle the file based on its extension
    #[must_use]
    fn get_file_type_hint(file_name: &str) -> String {
        let ext = std::path::Path::new(file_name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase());

        match ext.as_deref() {
            Some("py" | "rs" | "js" | "ts" | "go" | "java" | "cpp" | "c" | "h") => {
                "💡 Source code. Use `read_file` or execute.".into()
            }
            Some("json" | "yaml" | "yml" | "toml" | "xml") => {
                "💡 Structured data. Read via `read_file`.".into()
            }
            Some("csv") => "💡 CSV. Process via Python pandas.".into(),
            Some("xlsx" | "xls") => "💡 Excel. Use Python openpyxl/pandas.".into(),
            Some("zip" | "tar" | "gz" | "7z" | "rar") => {
                "💡 Archive. Unpack: `unzip`, `tar -xf`, etc.".into()
            }
            Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg") => {
                "💡 Image. Process via Python PIL.".into()
            }
            Some("txt" | "md" | "log" | "ini" | "cfg") => "💡 Text. Read via `read_file`.".into(),
            Some("pdf") => "💡 PDF. Use Python PyPDF2/pdfplumber.".into(),
            Some("sql" | "db" | "sqlite") => "💡 Database. Use Python sqlite3.".into(),
            _ => "💡 Use appropriate tools.".into(),
        }
    }

    /// Preprocess any input type and return text suitable for the agent
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::sync::Arc;
    /// # use oxide_agent_core::agent::preprocessor::{AgentInput, Preprocessor};
    /// # use oxide_agent_core::config::AgentSettings;
    /// # use oxide_agent_core::llm::LlmClient;
    /// # #[tokio::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// # let settings = AgentSettings::new().unwrap();
    /// # let llm_client = Arc::new(LlmClient::new(&settings));
    /// let preprocessor = Preprocessor::new(llm_client, 123456789);
    /// let input = AgentInput::Text("Hello".to_string());
    /// let result = preprocessor.preprocess_input(input).await?;
    /// assert_eq!(result, "Hello");
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if transcription or media analysis fails.
    pub async fn preprocess_input(&self, input: AgentInput) -> Result<String> {
        match input {
            AgentInput::Text(text) => Ok(text),
            AgentInput::Voice { bytes, mime_type } => {
                self.transcribe_voice(bytes, &mime_type).await
            }
            AgentInput::Image { bytes, context } => {
                let description = self.describe_image(bytes, context.as_deref()).await?;
                Ok(format_media_task(
                    context.as_deref(),
                    "Attached image content",
                    &description,
                ))
            }
            AgentInput::Video {
                bytes,
                mime_type,
                context,
            } => {
                let description = self
                    .describe_video(bytes, &mime_type, context.as_deref())
                    .await?;
                Ok(format_media_task(
                    context.as_deref(),
                    "Attached video content",
                    &description,
                ))
            }
            AgentInput::ImageWithText { image_bytes, text } => {
                let description = self.describe_image(image_bytes, Some(&text)).await?;
                Ok(format_media_task(
                    Some(&text),
                    "Attached image content",
                    &description,
                ))
            }
            AgentInput::Document {
                bytes,
                file_name,
                mime_type,
                caption,
            } => {
                self.process_document(bytes, file_name, mime_type, caption)
                    .await
            }
        }
    }
}

fn format_media_task(user_request: Option<&str>, content_label: &str, description: &str) -> String {
    match user_request
        .map(str::trim)
        .filter(|request| !request.is_empty())
    {
        Some(request) => {
            format!("User request:\n{request}\n\n{content_label}:\n{description}")
        }
        None => description.to_string(),
    }
}

/// Types of input the agent can receive
pub enum AgentInput {
    /// Plain text message
    Text(String),
    /// Voice message to be transcribed
    Voice {
        /// Raw audio bytes
        bytes: Vec<u8>,
        /// MIME type of the audio
        mime_type: String,
    },
    /// Image to be described
    Image {
        /// Raw image bytes
        bytes: Vec<u8>,
        /// Optional context from the user (caption)
        context: Option<String>,
    },
    /// Video clip to be described
    Video {
        /// Raw video bytes
        bytes: Vec<u8>,
        /// MIME type of the video
        mime_type: String,
        /// Optional context from the user (caption)
        context: Option<String>,
    },
    /// Image with accompanying text
    ImageWithText {
        /// Raw image bytes
        image_bytes: Vec<u8>,
        /// Accompanying text
        text: String,
    },
    /// Document uploaded by user
    Document {
        /// Raw file bytes
        bytes: Vec<u8>,
        /// Original filename
        file_name: String,
        /// MIME type of the file
        mime_type: Option<String>,
        /// Optional caption from the user
        caption: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentSettings;
    #[cfg(feature = "llm-openrouter")]
    use crate::config::ModuleRuntimeConfig;
    #[cfg(feature = "llm-openrouter")]
    use crate::llm::MockLlmProvider;
    use crate::sandbox::{SandboxBackend, SandboxBackendId, SandboxCapability, SandboxFileListing};
    use std::sync::Arc;
    use std::sync::Mutex;

    const TEST_FILEOPS_BACKEND_ID: SandboxBackendId =
        SandboxBackendId::new("test/preprocessor-fileops");
    const TEST_EXEC_BACKEND_ID: SandboxBackendId = SandboxBackendId::new("test/preprocessor-exec");
    const TEST_FILEOPS_CAPABILITIES: &[SandboxCapability] = &[SandboxCapability::FileOps];
    const TEST_EXEC_CAPABILITIES: &[SandboxCapability] = &[SandboxCapability::Exec];

    #[derive(Default)]
    struct RecordingSandboxFileOps {
        writes: Mutex<Vec<(String, Vec<u8>)>>,
    }

    impl RecordingSandboxFileOps {
        fn writes(&self) -> Vec<(String, Vec<u8>)> {
            self.writes.lock().expect("writes mutex poisoned").clone()
        }
    }

    impl SandboxBackend for RecordingSandboxFileOps {
        fn id(&self) -> SandboxBackendId {
            TEST_FILEOPS_BACKEND_ID
        }

        fn capabilities(&self) -> &'static [SandboxCapability] {
            TEST_FILEOPS_CAPABILITIES
        }
    }

    #[async_trait::async_trait]
    impl SandboxFileOps for RecordingSandboxFileOps {
        async fn write_file(&self, path: &str, bytes: &[u8]) -> Result<()> {
            self.writes
                .lock()
                .expect("writes mutex poisoned")
                .push((path.to_string(), bytes.to_vec()));
            Ok(())
        }

        async fn read_file(&self, _path: &str) -> Result<Vec<u8>> {
            Ok(Vec::new())
        }

        async fn file_size_bytes(
            &self,
            _path: &str,
            _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
        ) -> Result<u64> {
            Ok(0)
        }

        async fn list_files(&self, path: &str) -> Result<SandboxFileListing> {
            Ok(SandboxFileListing {
                path: path.to_string(),
                listing: String::new(),
                stderr: String::new(),
                exit_code: 0,
            })
        }

        async fn apply_file_edit(
            &self,
            _path: &str,
            _edit: crate::sandbox::SandboxFileEdit,
        ) -> Result<crate::sandbox::SandboxApplyFileEditResult> {
            anyhow::bail!("test sandbox file edit is not implemented")
        }
    }

    struct RecordingSandboxExec {
        result: ExecResult,
        commands: Mutex<Vec<String>>,
    }

    impl RecordingSandboxExec {
        fn new(stdout: &str) -> Self {
            Self {
                result: ExecResult {
                    stdout: stdout.to_string(),
                    stderr: String::new(),
                    exit_code: 0,
                },
                commands: Mutex::new(Vec::new()),
            }
        }

        fn commands(&self) -> Vec<String> {
            self.commands
                .lock()
                .expect("commands mutex poisoned")
                .clone()
        }
    }

    impl SandboxBackend for RecordingSandboxExec {
        fn id(&self) -> SandboxBackendId {
            TEST_EXEC_BACKEND_ID
        }

        fn capabilities(&self) -> &'static [SandboxCapability] {
            TEST_EXEC_CAPABILITIES
        }
    }

    #[async_trait::async_trait]
    impl SandboxExec for RecordingSandboxExec {
        async fn exec(
            &self,
            command: &str,
            _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
        ) -> Result<ExecResult> {
            self.commands
                .lock()
                .expect("commands mutex poisoned")
                .push(command.to_string());
            Ok(self.result.clone())
        }
    }

    #[test]
    fn test_sanitize_filename_basic() {
        assert_eq!(Preprocessor::sanitize_filename("file.txt"), "file.txt");
        assert_eq!(
            Preprocessor::sanitize_filename("my file.txt"),
            "my_file.txt"
        );
    }

    #[test]
    fn test_sanitize_filename_path_traversal() {
        assert_eq!(
            Preprocessor::sanitize_filename("../../../etc/passwd"),
            "passwd"
        );
        assert_eq!(Preprocessor::sanitize_filename("/etc/passwd"), "passwd");
        assert_eq!(
            Preprocessor::sanitize_filename("..\\..\\windows\\system32"),
            "system32"
        );
    }

    #[test]
    fn test_sanitize_filename_special_chars() {
        assert_eq!(
            Preprocessor::sanitize_filename("file:name?.txt"),
            "file_name_.txt"
        );
        assert_eq!(
            Preprocessor::sanitize_filename("test<>|file"),
            "test___file"
        );
    }

    #[test]
    fn test_sanitize_empty_filename() {
        // Empty name should become "file"
        assert_eq!(Preprocessor::sanitize_filename(""), "file");
    }

    #[test]
    fn test_sanitize_only_extension() {
        assert_eq!(Preprocessor::sanitize_filename(".gitignore"), ".gitignore");
    }

    #[test]
    fn test_format_file_size() {
        assert_eq!(Preprocessor::format_file_size(500), "500 B");
        assert_eq!(Preprocessor::format_file_size(1_024), "1.0 KB");
        assert_eq!(Preprocessor::format_file_size(1_536), "1.5 KB");
        assert_eq!(Preprocessor::format_file_size(1_048_576), "1.0 MB");
        assert_eq!(Preprocessor::format_file_size(1_572_864), "1.5 MB");
    }

    #[test]
    fn test_format_file_size_zero() {
        assert_eq!(Preprocessor::format_file_size(0), "0 B");
    }

    #[test]
    fn test_format_file_size_large() {
        // 1 GB
        assert_eq!(Preprocessor::format_file_size(1_073_741_824), "1024.0 MB");
    }

    #[test]
    fn test_parse_uploads_size() {
        let result = ExecResult {
            stdout: "1536\t/workspace/uploads\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        };

        assert_eq!(
            Preprocessor::parse_uploads_size(&result).expect("uploads size should parse"),
            1536
        );
    }

    #[test]
    fn test_get_file_type_hint() {
        assert!(Preprocessor::get_file_type_hint("script.py").contains("Source code"));
        assert!(Preprocessor::get_file_type_hint("data.csv").contains("pandas"));
        assert!(Preprocessor::get_file_type_hint("archive.zip").contains("Archive"));
        assert!(Preprocessor::get_file_type_hint("image.png").contains("PIL"));
        assert!(Preprocessor::get_file_type_hint("unknown.xyz").contains("tools"));
    }

    #[tokio::test]
    async fn process_document_uses_narrow_sandbox_backends() {
        let settings = AgentSettings::default();
        let llm = Arc::new(crate::llm::LlmClient::new(&settings));
        let fileops = Arc::new(RecordingSandboxFileOps::default());
        let exec = Arc::new(RecordingSandboxExec::new("12\t/workspace/uploads\n"));
        let sandbox_fileops: Arc<dyn SandboxFileOps> =
            Arc::<RecordingSandboxFileOps>::clone(&fileops);
        let sandbox_exec: Arc<dyn SandboxExec> = Arc::<RecordingSandboxExec>::clone(&exec);
        let preprocessor = Preprocessor::with_sandbox_backends(llm, sandbox_fileops, sandbox_exec);

        let processed = preprocessor
            .process_document(
                b"abc".to_vec(),
                "../my file.txt".to_string(),
                Some("text/plain".to_string()),
                Some("caption".to_string()),
            )
            .await
            .expect("document processing should succeed");

        assert_eq!(
            fileops.writes(),
            vec![(
                "/workspace/uploads/my_file.txt".to_string(),
                b"abc".to_vec()
            )]
        );
        assert_eq!(
            exec.commands(),
            vec!["du -sb /workspace/uploads 2>/dev/null || echo '0'".to_string()]
        );
        assert!(processed.contains("Path: `/workspace/uploads/my_file.txt`"));
        assert!(processed.contains("Size: 3 B"));
        assert!(processed.contains("Type: text/plain"));
        assert!(processed.contains("**Message:** caption"));
    }

    #[cfg(feature = "llm-openrouter")]
    #[tokio::test]
    async fn preprocess_image_preserves_user_request_separately_from_description() {
        let mut settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("openrouter".to_string()),
            media_model_id: Some("google/gemini-3-flash-preview".to_string()),
            media_model_provider: Some("openrouter".to_string()),
            ..AgentSettings::default()
        };
        settings.modules.insert(
            "llm-provider/openrouter".to_string(),
            ModuleRuntimeConfig::default().with_string_value("api_key", "test-key"),
        );
        let mut provider = MockLlmProvider::new();
        provider
            .expect_analyze_image()
            .withf(|bytes, prompt, system_prompt, model_id| {
                bytes == &b"image".to_vec()
                    && prompt.contains("Перевод на ру яз")
                    && prompt.contains("Do not answer, translate")
                    && system_prompt.contains("Do not perform the user's request")
                    && model_id == "google/gemini-3-flash-preview"
            })
            .return_once(|_, _, _, _| Ok("Visible text: OpenAI Developers".to_string()));

        let mut llm = crate::llm::LlmClient::new(&settings);
        llm.register_provider("openrouter".to_string(), Arc::new(provider));

        let preprocessor = Preprocessor::new(Arc::new(llm), 42_i64);
        let result = preprocessor
            .preprocess_input(AgentInput::Image {
                bytes: b"image".to_vec(),
                context: Some("Перевод на ру яз".to_string()),
            })
            .await
            .expect("image preprocess succeeds");

        assert!(result.contains("User request:\nПеревод на ру яз"));
        assert!(result.contains("Attached image content:\nVisible text: OpenAI Developers"));
    }

    #[cfg(feature = "llm-openrouter")]
    #[tokio::test]
    async fn preprocess_image_without_context_keeps_plain_description() {
        let mut settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("openrouter".to_string()),
            media_model_id: Some("google/gemini-3-flash-preview".to_string()),
            media_model_provider: Some("openrouter".to_string()),
            ..AgentSettings::default()
        };
        settings.modules.insert(
            "llm-provider/openrouter".to_string(),
            ModuleRuntimeConfig::default().with_string_value("api_key", "test-key"),
        );
        let mut provider = MockLlmProvider::new();
        provider
            .expect_analyze_image()
            .withf(|bytes, prompt, system_prompt, model_id| {
                bytes == &b"image".to_vec()
                    && prompt.contains("Extract the content of this image")
                    && prompt.contains("Do not answer, translate")
                    && system_prompt.contains("visual analyzer")
                    && model_id == "google/gemini-3-flash-preview"
            })
            .return_once(|_, _, _, _| Ok("plain image description".to_string()));

        let mut llm = crate::llm::LlmClient::new(&settings);
        llm.register_provider("openrouter".to_string(), Arc::new(provider));

        let preprocessor = Preprocessor::new(Arc::new(llm), 42_i64);
        let result = preprocessor
            .preprocess_input(AgentInput::Image {
                bytes: b"image".to_vec(),
                context: None,
            })
            .await
            .expect("image preprocess succeeds");

        assert_eq!(result, "plain image description");
    }

    #[cfg(feature = "llm-openrouter")]
    #[tokio::test]
    async fn preprocess_video_uses_media_model() {
        let mut settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("openrouter".to_string()),
            media_model_id: Some("google/gemini-3-flash-preview".to_string()),
            media_model_provider: Some("openrouter".to_string()),
            ..AgentSettings::default()
        };
        settings.modules.insert(
            "llm-provider/openrouter".to_string(),
            ModuleRuntimeConfig::default().with_string_value("api_key", "test-key"),
        );
        let mut provider = MockLlmProvider::new();
        provider
            .expect_analyze_video()
            .withf(|bytes, mime_type, prompt, system_prompt, model_id| {
                bytes == &b"video".to_vec()
                    && mime_type == "video/mp4"
                    && prompt.contains("release demo")
                    && prompt.contains("Do not answer, translate")
                    && system_prompt.contains("video analyzer")
                    && system_prompt.contains("Do not perform the user's request")
                    && model_id == "google/gemini-3-flash-preview"
            })
            .return_once(|_, _, _, _, _| Ok("timeline".to_string()));

        let mut llm = crate::llm::LlmClient::new(&settings);
        llm.register_provider("openrouter".to_string(), Arc::new(provider));

        let preprocessor = Preprocessor::new(Arc::new(llm), 42_i64);
        let result = preprocessor
            .preprocess_input(AgentInput::Video {
                bytes: b"video".to_vec(),
                mime_type: "video/mp4".to_string(),
                context: Some("release demo".to_string()),
            })
            .await
            .expect("video preprocess succeeds");

        assert!(result.contains("User request:\nrelease demo"));
        assert!(result.contains("Attached video content:\ntimeline"));
    }
}
