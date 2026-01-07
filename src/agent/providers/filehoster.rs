//! FileHoster provider - uploads large sandbox files to external hosting.
//!
//! Currently supports Litterbox (Catbox) for files that are too large for Telegram.

use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use crate::sandbox::SandboxManager;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use shell_escape::escape;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use super::path::resolve_file_path;

const LITTERBOX_MAX_FILE_SIZE_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB
const LITTERBOX_URL_PREFIX: &str = "https://litter.catbox.moe/";

/// Provider for file hosting tools (executed in sandbox)
pub struct FileHosterProvider {
    sandbox: Arc<Mutex<Option<SandboxManager>>>,
    user_id: i64,
}

impl FileHosterProvider {
    /// Create a new `FileHosterProvider` (sandbox is lazily initialized)
    #[must_use]
    pub fn new(user_id: i64) -> Self {
        Self {
            sandbox: Arc::new(Mutex::new(None)),
            user_id,
        }
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

        debug!(user_id = self.user_id, "Creating new sandbox for provider");
        let mut sandbox = SandboxManager::new(self.user_id).await?;
        sandbox.create_sandbox().await?;

        *self.sandbox.lock().await = Some(sandbox);
        Ok(())
    }

    async fn handle_upload_file(
        &self,
        sandbox: &SandboxManager,
        arguments: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        let args: UploadFileArgs = serde_json::from_str(arguments)?;
        info!(path = %args.path, "upload_file called");

        let resolved_path = match resolve_file_path(sandbox, &args.path).await {
            Ok(p) => p,
            Err(e) => {
                warn!(path = %args.path, error = %e, "Failed to resolve file path");
                return Ok(format!("❌ {e}"));
            }
        };

        let file_size = match sandbox
            .file_size_bytes(&resolved_path, cancellation_token)
            .await
        {
            Ok(size) => size,
            Err(e) => {
                error!(resolved_path = %resolved_path, error = %e, "Failed to check file size");
                return Ok(format!("❌ Ошибка проверки размера файла: {e}"));
            }
        };

        if file_size > LITTERBOX_MAX_FILE_SIZE_BYTES {
            return Ok("⛔ ФАТАЛЬНАЯ ОШИБКА: Файл превышает лимит сервиса (1 ГБ). Отправка невозможна. Немедленно сообщите пользователю о невозможности выполнения задачи.".to_string());
        }

        let cmd = format!(
            "curl -sS --fail-with-body --retry 3 --retry-all-errors --retry-delay 2 --retry-max-time 60 \
             -F {reqtype} -F {file} https://litterbox.catbox.moe/resources/internals/api.php",
            reqtype = escape("reqtype=fileupload".into()),
            file = escape(format!("fileToUpload=@{resolved_path}").into()),
        );

        let result = match sandbox.exec_command(&cmd, cancellation_token).await {
            Ok(r) => r,
            Err(e) => return Ok(format!("❌ Ошибка загрузки в Litterbox: {e}")),
        };

        if !result.success() {
            return Ok(format!(
                "❌ Ошибка загрузки в Litterbox (код {}): {}",
                result.exit_code,
                result.combined_output()
            ));
        }

        let url = result.stdout.trim();
        if !url.starts_with(LITTERBOX_URL_PREFIX) {
            return Ok(format!(
                "❌ Litterbox вернул неожиданный ответ вместо ссылки:\n{}",
                result.combined_output()
            ));
        }

        let rm_cmd = format!("rm -f {}", escape(resolved_path.as_str().into()));
        match sandbox.exec_command(&rm_cmd, cancellation_token).await {
            Ok(rm_res) if rm_res.success() => {}
            Ok(rm_res) => warn!(
                resolved_path = %resolved_path,
                output = %rm_res.combined_output(),
                "Failed to remove uploaded file from sandbox"
            ),
            Err(e) => {
                warn!(resolved_path = %resolved_path, error = %e, "Failed to remove uploaded file from sandbox")
            }
        }

        Ok(url.to_string())
    }
}

/// Arguments for `upload_file` tool
#[derive(Debug, Deserialize)]
struct UploadFileArgs {
    path: String,
}

#[async_trait]
impl ToolProvider for FileHosterProvider {
    fn name(&self) -> &'static str {
        "filehoster"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "upload_file".to_string(),
            description: "Upload a file from the sandbox to external hosting (Litterbox). Use this for files too large for Telegram (>50 MB). Returns a public link on success.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file in the sandbox (relative or absolute)"
                    }
                },
                "required": ["path"]
            }),
        }]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(tool_name, "upload_file")
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        debug!(tool = tool_name, "Executing filehoster tool");

        self.ensure_sandbox().await?;
        let sandbox = {
            let guard = self.sandbox.lock().await;
            guard
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Sandbox not initialized"))?
        };

        match tool_name {
            "upload_file" => {
                self.handle_upload_file(&sandbox, arguments, cancellation_token)
                    .await
            }
            _ => anyhow::bail!("Unknown filehoster tool: {tool_name}"),
        }
    }
}
