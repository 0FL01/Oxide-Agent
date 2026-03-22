// oxide-agent-core/src/agent/registry.rs
//
// ToolRegistry Integration with OpencodeToolProvider
//
// Этот файл показывает как интегрировать OpencodeToolProvider в существующую
// систему ToolRegistry для маршрутизации вызовов инструментов
//

use crate::agent::providers::opencode::OpencodeToolProvider;
use crate::agent::providers::sandbox::SandboxProvider;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

/// Progress events from agent execution
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Tool call started
    ToolCall {
        name: String,
        input: String,
        command_preview: Option<String>,
    },
    /// Tool call completed
    ToolResult {
        name: String,
        output: String,
    },
    /// File to send to user
    FileToSend {
        filename: String,
        content: Vec<u8>,
    },
}

/// Trait for tool providers
#[async_trait::async_trait]
pub trait ToolProvider: Send + Sync {
    /// Check if this provider can handle the tool
    fn can_handle(&self, tool_name: &str) -> bool;

    /// Execute a tool call
    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: &Sender<AgentEvent>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String, String>;
}

/// ToolRegistry - маршрутизирует вызовы инструментов к соответствующим провайдерам
pub struct ToolRegistry {
    /// List of tool providers (sandbox, etc.)
    providers: Vec<Box<dyn ToolProvider>>,

    /// Opencode provider for code development
    opencode_provider: OpencodeToolProvider,
}

impl ToolRegistry {
    /// Create a new ToolRegistry
    ///
    /// # Arguments
    ///
    /// * `opencode_url` - URL of Opencode Server (optional, defaults to http://127.0.0.1:4096)
    ///
    /// # Example
    ///
    /// ```
    /// let registry = ToolRegistry::new(Some("http://127.0.0.1:4096".to_string()));
    /// ```
    pub fn new(opencode_url: Option<String>) -> Self {
        let opencode_url = opencode_url.unwrap_or_else(|| {
            std::env::var("OPENCODE_BASE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:4096".to_string())
        });

        Self {
            providers: vec![
                Box::new(SandboxProvider::new()),  // Твоя существующая песочница
            ],
            opencode_provider: OpencodeToolProvider::new(opencode_url),
        }
    }

    /// Execute a tool call
    ///
    /// Routes the tool call to the appropriate provider based on tool name.
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool to execute (e.g., "opencode", "execute_command")
    /// * `arguments` - JSON string with tool arguments
    /// * `progress_tx` - Channel for sending progress events
    /// * `cancellation_token` - Optional token for cancelling execution
    ///
    /// # Returns
    ///
    /// Tool execution result as string
    ///
    /// # Errors
    ///
    /// Returns `Err` if:
    /// - Tool not found
    /// - Tool execution failed
    /// - Progress events failed to send
    ///
    /// # Example
    ///
    /// ```
    /// let (tx, _rx) = tokio::sync::mpsc::channel(100);
    /// let result = registry.execute("opencode", r#"{"task": "list files"}"#, &tx, None).await?;
    /// ```
    pub async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: &Sender<AgentEvent>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String, String> {
        // Handle opencode tool directly (not routed to providers)
        if tool_name == "opencode" {
            return self.handle_opencode(arguments, progress_tx).await;
        }

        // Route to other providers (sandbox, etc.)
        for provider in &self.providers {
            if provider.can_handle(tool_name) {
                return provider
                    .execute(tool_name, arguments, progress_tx, cancellation_token)
                    .await;
            }
        }

        Err(format!("Tool not found: {}", tool_name))
    }

    /// Handle opencode tool call
    ///
    /// Creates session, sends prompt, and returns result
    async fn handle_opencode(
        &self,
        arguments: &str,
        progress_tx: &Sender<AgentEvent>,
    ) -> Result<String, String> {
        // Send progress event: tool call started
        progress_tx
            .send(AgentEvent::ToolCall {
                name: "opencode".to_string(),
                input: arguments.to_string(),
                command_preview: Some("Opencode task execution".to_string()),
            })
            .await
            .map_err(|e| format!("Failed to send progress event: {}", e))?;

        // Parse arguments
        let args: serde_json::Value = serde_json::from_str(arguments)
            .map_err(|e| format!("Failed to parse opencode args: {}", e))?;

        let task = args
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or("No 'task' field in opencode args")?;

        // Execute task via Opencode
        let result = self.opencode_provider.execute_task(task).await?;

        // Send progress event: tool call completed
        progress_tx
            .send(AgentEvent::ToolResult {
                name: "opencode".to_string(),
                output: result.clone(),
            })
            .await
            .map_err(|e| format!("Failed to send result event: {}", e))?;

        Ok(result)
    }

    /// Check health of Opencode server
    ///
    /// # Returns
    ///
    /// `Ok(())` if healthy, `Err` with error message otherwise
    pub async fn health_check(&self) -> Result<(), String> {
        self.opencode_provider.health_check().await
    }

    /// Add a custom tool provider
    ///
    /// Useful for testing or extending with additional providers
    ///
    /// # Arguments
    ///
    /// * `provider` - Tool provider to add
    pub fn add_provider(&mut self, provider: Box<dyn ToolProvider>) {
        self.providers.push(provider);
    }

    /// Get list of available tool names
    ///
    /// # Returns
    ///
    /// Vector of tool names (including "opencode")
    pub fn available_tools(&self) -> Vec<String> {
        let mut tools = vec!["opencode".to_string()];

        for provider in &self.providers {
            // This assumes providers expose their tool names somehow
            // You might need to add a method to ToolProvider trait
            // tools.extend(provider.get_tool_names());
        }

        tools
    }
}

/// Example implementation of SandboxProvider
///
/// This is a placeholder for your existing SandboxProvider implementation
pub struct SandboxProvider {
    // Your existing fields
}

impl SandboxProvider {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait::async_trait]
impl ToolProvider for SandboxProvider {
    fn can_handle(&self, tool_name: &str) -> bool {
        // Return true for sandbox tools: execute_command, write_file, etc.
        matches!(
            tool_name,
            "execute_command" | "write_file" | "read_file" | "list_files"
        )
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: &Sender<AgentEvent>,
        _cancellation_token: Option<&CancellationToken>,
    ) -> Result<String, String> {
        // Send progress event
        progress_tx
            .send(AgentEvent::ToolCall {
                name: tool_name.to_string(),
                input: arguments.to_string(),
                command_preview: Some(format!("{}: {}", tool_name, arguments)),
            })
            .await
            .map_err(|e| format!("Failed to send progress event: {}", e))?;

        // Execute tool (your existing implementation)
        let result = match tool_name {
            "execute_command" => {
                // Your execute_command implementation
                format!("Executed: {}", arguments)
            }
            "write_file" => {
                // Your write_file implementation
                "File written".to_string()
            }
            "read_file" => {
                // Your read_file implementation
                "File content".to_string()
            }
            "list_files" => {
                // Your list_files implementation
                "file1.txt\nfile2.txt".to_string()
            }
            _ => return Err(format!("Unknown sandbox tool: {}", tool_name)),
        };

        // Send progress event
        progress_tx
            .send(AgentEvent::ToolResult {
                name: tool_name.to_string(),
                output: result.clone(),
            })
            .await
            .map_err(|e| format!("Failed to send result event: {}", e))?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "Requires running Opencode server"]
    async fn test_registry_opencode_tool() {
        let registry = ToolRegistry::new(Some("http://127.0.0.1:4096".to_string()));
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);

        let args = r#"{"task": "list files"}"#;
        let result = registry.execute("opencode", args, &tx, None).await;
        assert!(result.is_ok(), "Opencode tool should execute");

        // Check progress events
        let event = rx.recv().await.unwrap();
        matches!(event, AgentEvent::ToolCall { name, .. } if name == "opencode");

        let event = rx.recv().await.unwrap();
        matches!(event, AgentEvent::ToolResult { name, .. } if name == "opencode");
    }

    #[tokio::test]
    #[ignore = "Requires running Opencode server"]
    async fn test_registry_health_check() {
        let registry = ToolRegistry::new(Some("http://127.0.0.1:4096".to_string()));

        let result = registry.health_check().await;
        assert!(result.is_ok(), "Opencode server should be healthy");
    }

    #[tokio::test]
    async fn test_registry_unknown_tool() {
        let registry = ToolRegistry::new(None);
        let (tx, _rx) = tokio::sync::mpsc::channel(100);

        let result = registry.execute("unknown_tool", "{}", &tx, None).await;
        assert!(result.is_err(), "Unknown tool should return error");

        assert!(result.unwrap_err().contains("Tool not found"));
    }

    #[tokio::test]
    async fn test_registry_sandbox_tool() {
        let registry = ToolRegistry::new(None);
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);

        let args = r#"echo 'Hello, World!'"#;
        let result = registry.execute("execute_command", args, &tx, None).await;
        assert!(result.is_ok(), "Sandbox tool should execute");

        // Check progress events
        let event = rx.recv().await.unwrap();
        matches!(event, AgentEvent::ToolCall { name, .. } if name == "execute_command");

        let event = rx.recv().await.unwrap();
        matches!(event, AgentEvent::ToolResult { name, .. } if name == "execute_command");
    }
}
