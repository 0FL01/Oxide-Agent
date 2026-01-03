//! Agent tools module
//!
//! Defines available tools for the agent and handles execution.

use crate::sandbox::SandboxManager;
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, warn};

use crate::llm::ToolDefinition;

/// Get all available agent tools
pub fn get_agent_tools() -> Vec<ToolDefinition> {
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
    ]
}

/// Arguments for execute_command tool
#[derive(Debug, Deserialize)]
pub struct ExecuteCommandArgs {
    pub command: String,
}

/// Arguments for write_file tool
#[derive(Debug, Deserialize)]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
}

/// Arguments for read_file tool
#[derive(Debug, Deserialize)]
pub struct ReadFileArgs {
    pub path: String,
}

/// Execute a tool call and return the result
pub async fn execute_tool(sandbox: &SandboxManager, tool_name: &str, arguments: &str) -> String {
    debug!(tool = tool_name, "Executing agent tool");

    match tool_name {
        "execute_command" => match serde_json::from_str::<ExecuteCommandArgs>(arguments) {
            Ok(args) => match sandbox.exec_command(&args.command).await {
                Ok(result) => {
                    if result.success() {
                        if result.stdout.is_empty() {
                            "(команда выполнена успешно, вывод пуст)".to_string()
                        } else {
                            result.stdout
                        }
                    } else {
                        format!(
                            "Ошибка (код {}): {}",
                            result.exit_code,
                            result.combined_output()
                        )
                    }
                }
                Err(e) => format!("Ошибка выполнения команды: {}", e),
            },
            Err(e) => format!("Ошибка парсинга аргументов: {}", e),
        },
        "write_file" => match serde_json::from_str::<WriteFileArgs>(arguments) {
            Ok(args) => match sandbox
                .write_file(&args.path, args.content.as_bytes())
                .await
            {
                Ok(_) => format!("Файл {} успешно записан", args.path),
                Err(e) => format!("Ошибка записи файла: {}", e),
            },
            Err(e) => format!("Ошибка парсинга аргументов: {}", e),
        },
        "read_file" => match serde_json::from_str::<ReadFileArgs>(arguments) {
            Ok(args) => match sandbox.read_file(&args.path).await {
                Ok(content) => String::from_utf8_lossy(&content).to_string(),
                Err(e) => format!("Ошибка чтения файла: {}", e),
            },
            Err(e) => format!("Ошибка парсинга аргументов: {}", e),
        },
        _ => {
            warn!(tool = tool_name, "Unknown tool called");
            format!("Неизвестный инструмент: {}", tool_name)
        }
    }
}
