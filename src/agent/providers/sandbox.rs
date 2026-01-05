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
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tracing::{debug, warn};

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
                description: "Send a file from the sandbox to the user via Telegram. Use this when you need to deliver generated files, images, documents, or any output to the user.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file in the sandbox to send to the user"
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
            "execute_command" | "read_file" | "write_file" | "send_file_to_user"
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

                // Extract file name from path
                let file_name = std::path::Path::new(&args.path)
                    .file_name()
                    .map_or_else(|| "file".to_string(), |n| n.to_string_lossy().to_string());

                match sandbox.download_file(&args.path).await {
                    Ok(content) => {
                        // Send file via progress channel
                        if let Some(ref tx) = self.progress_tx {
                            match tx
                                .send(AgentEvent::FileToSend {
                                    file_name: file_name.clone(),
                                    content,
                                })
                                .await
                            {
                                Ok(()) => {
                                    debug!(file_name = %file_name, "FileToSend event sent successfully");
                                    Ok(format!("✅ Файл '{file_name}' отправлен пользователю"))
                                }
                                Err(e) => {
                                    warn!(file_name = %file_name, error = %e, "Failed to send FileToSend event");
                                    Ok(format!(
                                        "⚠️ Файл '{file_name}' прочитан, но не удалось отправить событие: {e}"
                                    ))
                                }
                            }
                        } else {
                            Ok(format!(
                                "⚠️ Файл '{file_name}' прочитан, но канал отправки недоступен"
                            ))
                        }
                    }
                    Err(e) => Ok(format!("❌ Ошибка отправки файла: {e}")),
                }
            }
            _ => anyhow::bail!("Unknown sandbox tool: {tool_name}"),
        }
    }
}
