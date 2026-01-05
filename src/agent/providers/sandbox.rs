//! Sandbox Provider - executes tools in Docker sandbox
//!
//! Provides `execute_command`, `read_file`, `write_file`, `send_file_to_user` tools.

use crate::agent::progress::AgentEvent;
use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use crate::sandbox::SandboxManager;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use shell_escape::escape;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

/// Provider for Docker sandbox tools
pub struct SandboxProvider {
    sandbox: Arc<Mutex<Option<SandboxManager>>>,
    user_id: i64,
    progress_tx: Option<Sender<AgentEvent>>,
}

impl SandboxProvider {
    /// Create a new sandbox provider (sandbox is lazily initialized)
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

    /// Set the sandbox manager (for when sandbox is created externally)
    pub async fn set_sandbox(&self, sandbox: SandboxManager) {
        let mut guard = self.sandbox.lock().await;
        *guard = Some(sandbox);
    }

    /// Get or create the sandbox
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

    /// Resolve relative path to absolute path in sandbox
    /// Searches for file if not found at expected location
    async fn resolve_file_path(sandbox: &SandboxManager, path: &str) -> Result<String> {
        if path.starts_with('/') {
            return Ok(path.to_string());
        }

        let workspace_path = format!("/workspace/{path}");
        let check = sandbox
            .exec_command(&format!(
                "test -f '{}' && echo 'exists'",
                escape(workspace_path.as_str().into())
            ))
            .await?;

        if check.stdout.contains("exists") {
            info!(original_path = %path, resolved_path = %workspace_path, "Resolved file path");
            return Ok(workspace_path);
        }

        info!(path = %path, "File not found at /workspace/{path}, searching...");
        let find_cmd = format!("find /workspace -name '{}' -type f", escape(path.into()));
        let result = sandbox.exec_command(&find_cmd).await?;

        let found_paths: Vec<&str> = result.stdout.lines().filter(|l| !l.is_empty()).collect();

        match found_paths.len() {
            0 => anyhow::bail!(
                "Файл '{}' не найден в песочнице. Используйте инструмент 'list_files' для просмотра доступных файлов.",
                path
            ),
            1 => {
                let resolved = found_paths[0].to_string();
                info!(original_path = %path, resolved_path = %resolved, "Found file");
                Ok(resolved)
            }
            _ => {
                let paths_list = found_paths.join("\n  - ");
                anyhow::bail!(
                    "Найдено несколько файлов с именем '{}':\n  - {}\n\nПожалуйста, укажите полный путь к нужному файлу.",
                    path, paths_list
                )
            }
        }
    }
}

/// Arguments for `execute_command` tool
#[derive(Debug, Deserialize)]
struct ExecuteCommandArgs {
    command: String,
}

/// Arguments for `write_file` tool
#[derive(Debug, Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

/// Arguments for `read_file` tool
#[derive(Debug, Deserialize)]
struct ReadFileArgs {
    path: String,
}

/// Arguments for `send_file_to_user` tool
#[derive(Debug, Deserialize)]
struct SendFileArgs {
    path: String,
}

#[async_trait]
impl ToolProvider for SandboxProvider {
    fn name(&self) -> &'static str {
        "sandbox"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "execute_command".to_string(),
                description: "Execute a bash command in the isolated sandbox environment. Available commands include: python3, pip, curl, wget, date, cat, ls, grep, and other standard Unix tools.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The bash command to execute"
                        }
                    },
                    "required": ["command"]
                }),
            },
            ToolDefinition {
                name: "write_file".to_string(),
                description: "Write content to a file in the sandbox. Creates parent directories if needed.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file (relative to /workspace or absolute)"
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to write to the file"
                        }
                    },
                    "required": ["path", "content"]
                }),
            },
            ToolDefinition {
                name: "read_file".to_string(),
                description: "Read content from a file in the sandbox.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file to read"
                        }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: "send_file_to_user".to_string(),
                description: "Send a file from the sandbox to the user via Telegram. Use this when you need to deliver generated files, images, documents, or any output to the user. Supports both absolute paths (/workspace/file.txt) and relative paths (file.txt) - will automatically search in /workspace if not found.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file in the sandbox to send to the user (relative or absolute)"
                        }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: "list_files".to_string(),
                description: "List files in the sandbox workspace. Returns a tree-like structure of files and directories. Useful for finding file paths before using send_file_to_user.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Optional path to list (defaults to /workspace)"
                        }
                    }
                }),
            },
        ]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            "execute_command" | "read_file" | "write_file" | "send_file_to_user" | "list_files"
        )
    }

    async fn execute(&self, tool_name: &str, arguments: &str) -> Result<String> {
        debug!(tool = tool_name, "Executing sandbox tool");

        // Ensure sandbox is running
        self.ensure_sandbox().await?;

        let sandbox = {
            let guard = self.sandbox.lock().await;
            guard
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Sandbox not initialized"))?
        };

        match tool_name {
            "execute_command" => {
                let args: ExecuteCommandArgs = serde_json::from_str(arguments)?;
                match sandbox.exec_command(&args.command).await {
                    Ok(result) => {
                        if result.success() {
                            if result.stdout.is_empty() {
                                Ok("(команда выполнена успешно, вывод пуст)".to_string())
                            } else {
                                Ok(result.stdout)
                            }
                        } else {
                            Ok(format!(
                                "Ошибка (код {}): {}",
                                result.exit_code,
                                result.combined_output()
                            ))
                        }
                    }
                    Err(e) => Ok(format!("Ошибка выполнения команды: {e}")),
                }
            }
            "write_file" => {
                let args: WriteFileArgs = serde_json::from_str(arguments)?;
                match sandbox
                    .write_file(&args.path, args.content.as_bytes())
                    .await
                {
                    Ok(()) => Ok(format!("Файл {} успешно записан", args.path)),
                    Err(e) => Ok(format!("Ошибка записи файла: {e}")),
                }
            }
            "read_file" => {
                let args: ReadFileArgs = serde_json::from_str(arguments)?;
                match sandbox.read_file(&args.path).await {
                    Ok(content) => Ok(String::from_utf8_lossy(&content).to_string()),
                    Err(e) => Ok(format!("Ошибка чтения файла: {e}")),
                }
            }
            "send_file_to_user" => {
                let args: SendFileArgs = serde_json::from_str(arguments)?;
                info!(path = %args.path, "send_file_to_user called");

                let resolved_path = match Self::resolve_file_path(&sandbox, &args.path).await {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(path = %args.path, error = %e, "Failed to resolve file path");
                        return Ok(format!("❌ {e}"));
                    }
                };

                let file_name = std::path::Path::new(&resolved_path)
                    .file_name()
                    .map_or_else(|| "file".to_string(), |n| n.to_string_lossy().to_string());

                match sandbox.download_file(&resolved_path).await {
                    Ok(content) => {
                        if let Some(ref tx) = self.progress_tx {
                            match tx
                                .send(AgentEvent::FileToSend {
                                    file_name: file_name.clone(),
                                    content,
                                })
                                .await
                            {
                                Ok(()) => {
                                    info!(file_name = %file_name, resolved_path = %resolved_path, "File sent successfully");
                                    Ok(format!("✅ Файл '{file_name}' отправлен пользователю"))
                                }
                                Err(e) => {
                                    warn!(file_name = %file_name, error = %e, "Failed to send FileToSend event");
                                    Ok(format!(
                                        "⚠️ Файл '{file_name}' прочитан из песочницы, но не удалось отправить: {e}"
                                    ))
                                }
                            }
                        } else {
                            warn!(file_name = %file_name, "Progress channel not available");
                            Ok(format!(
                                "⚠️ Файл '{file_name}' прочитан, но канал отправки недоступен"
                            ))
                        }
                    }
                    Err(e) => {
                        error!(path = %args.path, resolved_path = %resolved_path, error = %e, "Failed to download file");
                        Ok(format!("❌ Ошибка загрузки файла: {e}"))
                    }
                }
            }
            "list_files" => {
                #[derive(Debug, Deserialize)]
                struct ListFilesArgs {
                    #[serde(default = "default_workspace_path")]
                    path: String,
                }

                fn default_workspace_path() -> String {
                    "/workspace".to_string()
                }

                let args: ListFilesArgs = serde_json::from_str(arguments)?;
                let cmd = format!(
                    "tree -L 3 -h --du {} 2>/dev/null || find {} -type f -o -type d | head -100",
                    escape(args.path.as_str().into()),
                    escape(args.path.as_str().into())
                );

                match sandbox.exec_command(&cmd).await {
                    Ok(result) => {
                        if result.success() {
                            if result.stdout.is_empty() {
                                Ok(format!(
                                    "Директория '{}' пуста или не существует",
                                    args.path
                                ))
                            } else {
                                Ok(format!(
                                    "📁 Содержимое '{}':\n\n```\n{}\n```",
                                    args.path, result.stdout
                                ))
                            }
                        } else {
                            Ok(format!(
                                "❌ Ошибка при чтении директории: {}",
                                result.stderr
                            ))
                        }
                    }
                    Err(e) => Ok(format!("❌ Ошибка выполнения команды: {e}")),
                }
            }
            _ => anyhow::bail!("Unknown sandbox tool: {tool_name}"),
        }
    }
}
