//! Input preprocessor for multimodal content
//!
//! Handles voice and image preprocessing using Gemini Flash
//! before passing to the agent for execution.

use crate::llm::LlmClient;
use crate::sandbox::SandboxManager;
use anyhow::Result;
use std::sync::Arc;
use tracing::info;

/// Upload limit: 1 GB per session
const UPLOAD_LIMIT_BYTES: u64 = 1024 * 1024 * 1024;

/// Preprocessor for converting multimodal inputs to text
pub struct Preprocessor {
    llm_client: Arc<LlmClient>,
    user_id: i64,
}

impl Preprocessor {
    /// Create a new preprocessor with the given LLM client and user ID
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use oxide_agent::llm::LlmClient;
    /// use oxide_agent::agent::preprocessor::Preprocessor;
    /// use oxide_agent::config::Settings;
    ///
    /// let settings = Settings::new().unwrap();
    /// let llm_client = Arc::new(LlmClient::new(&settings));
    /// let preprocessor = Preprocessor::new(llm_client, 123456789);
    /// ```
    #[must_use]
    pub const fn new(llm_client: Arc<LlmClient>, user_id: i64) -> Self {
        Self {
            llm_client,
            user_id,
        }
    }

    /// Transcribe voice audio to text using Gemini Flash
    ///
    /// Uses the existing transcription infrastructure with `OpenRouter` Gemini
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::sync::Arc;
    /// # use oxide_agent::llm::LlmClient;
    /// # use oxide_agent::agent::preprocessor::Preprocessor;
    /// # use oxide_agent::config::Settings;
    /// # #[tokio::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// # let settings = Settings::new().unwrap();
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
        if !self.llm_client.is_multimodal_available() {
            return Err(anyhow::anyhow!("MULTIMODAL_DISABLED"));
        }

        info!(
            "Transcribing voice message: {} bytes, mime: {mime_type}",
            audio_bytes.len()
        );

        // Use Gemini Flash for transcription (via OpenRouter)
        let transcription = self
            .llm_client
            .transcribe_audio(audio_bytes, mime_type, "OR Gemini 3 Flash")
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
    /// # use oxide_agent::llm::LlmClient;
    /// # use oxide_agent::agent::preprocessor::Preprocessor;
    /// # use oxide_agent::config::Settings;
    /// # #[tokio::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// # let settings = Settings::new().unwrap();
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
        if !self.llm_client.is_multimodal_available() {
            return Err(anyhow::anyhow!("MULTIMODAL_DISABLED"));
        }

        info!("Describing image: {} bytes", image_bytes.len());

        let prompt = user_context.map_or_else(
            || {
                "Describe this image in detail for an AI agent. \
                     Include all important details, text, objects and their locations."
                    .to_string()
            },
            |ctx| {
                format!(
                    "Describe this image in detail for an AI agent that will perform a task. \
                 User context: {ctx}"
                )
            },
        );

        let system_prompt = "You are a visual analyzer for an AI agent. \
                            Your task is to create a detailed text description of the image that allows the agent to understand its content without accessing the image itself.";

        let description = self
            .llm_client
            .analyze_image(
                image_bytes,
                &prompt,
                system_prompt,
                "OR Gemini 3 Flash", // Use Gemini for multimodal
            )
            .await
            .map_err(|e| anyhow::anyhow!("Image analysis failed: {e}"))?;

        info!("Image description: {} chars", description.len());
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

        // Lazy-create sandbox
        let mut manager = SandboxManager::new(self.user_id).await?;
        if !manager.is_running() {
            manager.create_sandbox().await?;
        }

        // Check upload limit
        let current_size = manager.get_uploads_size().await.unwrap_or(0);
        let new_size = current_size + bytes.len() as u64;

        if new_size > UPLOAD_LIMIT_BYTES {
            anyhow::bail!(
                "Upload limit exceeded: {:.1} GB / 1 GB. Recreate the container.",
                new_size as f64 / 1024.0 / 1024.0 / 1024.0
            );
        }

        manager.upload_file(&upload_path, &bytes).await?;

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
    /// # use oxide_agent::llm::LlmClient;
    /// # use oxide_agent::agent::preprocessor::{Preprocessor, AgentInput};
    /// # use oxide_agent::config::Settings;
    /// # #[tokio::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// # let settings = Settings::new().unwrap();
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
    /// Returns an error if transcription or image analysis fails.
    pub async fn preprocess_input(&self, input: AgentInput) -> Result<String> {
        match input {
            AgentInput::Text(text) => Ok(text),
            AgentInput::Voice { bytes, mime_type } => {
                self.transcribe_voice(bytes, &mime_type).await
            }
            AgentInput::Image { bytes, context } => {
                self.describe_image(bytes, context.as_deref()).await
            }
            AgentInput::ImageWithText { image_bytes, text } => {
                let description = self.describe_image(image_bytes, Some(&text)).await?;
                Ok(format!(
                    "User sent an image with text: \"{text}\"\n\nImage description:\n{description}"
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
    fn test_get_file_type_hint() {
        assert!(Preprocessor::get_file_type_hint("script.py").contains("Source code"));
        assert!(Preprocessor::get_file_type_hint("data.csv").contains("pandas"));
        assert!(Preprocessor::get_file_type_hint("archive.zip").contains("Archive"));
        assert!(Preprocessor::get_file_type_hint("image.png").contains("PIL"));
        assert!(Preprocessor::get_file_type_hint("unknown.xyz").contains("tools"));
    }
}
