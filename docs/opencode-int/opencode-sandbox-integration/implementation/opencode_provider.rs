// oxide-agent-core/src/agent/providers/opencode.rs
//
// OpencodeToolProvider - HTTP клиент для взаимодействия с Opencode Server
//
// Этот провайдер позволяет LLM агенту управлять разработкой кода через
// OpenCode архитектора (@explore, @developer, @review, git operations)
//

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Default timeout for Opencode requests (5 minutes)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Opencode Tool Provider
///
/// HTTP клиент для взаимодействия с Opencode Server (opencode serve).
/// Позволяет создавать sessions, отправлять prompts и получать результаты.
#[derive(Debug, Clone)]
pub struct OpencodeToolProvider {
    base_url: String,
    client: reqwest::Client,
    timeout: Duration,
}

/// Request body for creating a new session
#[derive(Debug, Serialize)]
struct CreateSessionBody {
    title: String,
    agent: String,
}

/// Response from creating a session
#[derive(Debug, Deserialize)]
struct SessionResponse {
    id: String,
    title: String,
}

/// Request body for sending a prompt to a session
#[derive(Debug, Serialize)]
struct PromptBody {
    agent: String,
    parts: Vec<PromptPart>,
}

/// Part in a prompt request
#[derive(Debug, Serialize)]
struct PromptPart {
    #[serde(rename = "type")]
    part_type: String,
    text: String,
}

/// Response from sending a prompt
#[derive(Debug, Deserialize)]
struct PromptResponse {
    info: serde_json::Value,
    parts: Vec<serde_json::Value>,
}

/// Error types for Opencode operations
#[derive(Debug)]
pub enum OpencodeError {
    /// HTTP request failed
    RequestFailed(String),
    /// Server returned non-success status
    ServerError(u16, String),
    /// Failed to parse JSON response
    ParseFailed(String),
    /// No text content found in response
    NoContent,
    /// Health check failed
    Unhealthy,
}

impl std::fmt::Display for OpencodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpencodeError::RequestFailed(msg) => write!(f, "Request failed: {}", msg),
            OpencodeError::ServerError(code, msg) => write!(f, "Server error {}: {}", code, msg),
            OpencodeError::ParseFailed(msg) => write!(f, "Parse failed: {}", msg),
            OpencodeError::NoContent => write!(f, "No content found in response"),
            OpencodeError::Unhealthy => write!(f, "Opencode server is unhealthy"),
        }
    }
}

impl std::error::Error for OpencodeError {}

impl OpencodeToolProvider {
    /// Create a new OpencodeToolProvider
    ///
    /// # Arguments
    ///
    /// * `base_url` - Base URL of Opencode Server (e.g., "http://127.0.0.1:4096")
    ///
    /// # Example
    ///
    /// ```
    /// let provider = OpencodeToolProvider::new("http://127.0.0.1:4096".to_string());
    /// ```
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: reqwest::Client::builder()
                .timeout(DEFAULT_TIMEOUT)
                .build()
                .expect("Failed to create HTTP client"),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Set custom timeout for requests
    ///
    /// # Arguments
    ///
    /// * `timeout` - Custom timeout duration
    ///
    /// # Example
    ///
    /// ```
    /// let provider = OpencodeToolProvider::new("http://127.0.0.1:4096".to_string())
    ///     .with_timeout(Duration::from_secs(600));
    /// ```
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self.client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("Failed to create HTTP client with timeout");
        self
    }

    /// Execute a task through Opencode architect agent
    ///
    /// This method:
    /// 1. Creates a new session
    /// 2. Sends the task as a prompt to the architect agent
    /// 3. Waits for completion
    /// 4. Returns the text result
    ///
    /// # Arguments
    ///
    /// * `task` - Task description (e.g., "add request logging for all API endpoints")
    ///
    /// # Returns
    ///
    /// Text result from the architect agent
    ///
    /// # Errors
    ///
    /// Returns `Err` if:
    /// - Failed to create session
    /// - Failed to send prompt
    /// - Failed to parse response
    ///
    /// # Example
    ///
    /// ```
    /// let result = provider.execute_task("add logging").await?;
    /// println!("{}", result);
    /// ```
    pub async fn execute_task(&self, task: &str) -> Result<String, OpencodeError> {
        // 1. Create session
        let session = self.create_session(task).await?;

        // 2. Send prompt
        let response = self.send_prompt(&session.id, task).await?;

        // 3. Extract text from response
        let text = self.extract_text_from_response(&response)?;

        Ok(text)
    }

    /// Create a new session in Opencode
    ///
    /// # Arguments
    ///
    /// * `task` - Task description (used for session title)
    ///
    /// # Returns
    ///
    /// Created session with ID
    async fn create_session(&self, task: &str) -> Result<SessionResponse, OpencodeError> {
        let url = format!("{}/session", self.base_url);

        let body = CreateSessionBody {
            title: format!("Sandbox: {}", task.chars().take(50).collect::<String>()),
            agent: "architect".to_string(),
        };

        let res = self.client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| OpencodeError::RequestFailed(e.to_string()))?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(OpencodeError::ServerError(
                status.as_u16(),
                body,
            ));
        }

        res.json::<SessionResponse>()
            .await
            .map_err(|e| OpencodeError::ParseFailed(e.to_string()))
    }

    /// Send a prompt to a session
    ///
    /// # Arguments
    ///
    /// * `session_id` - ID of the session
    /// * `task` - Task description to send as prompt
    ///
    /// # Returns
    ///
    /// Response from the architect agent
    async fn send_prompt(&self, session_id: &str, task: &str) -> Result<PromptResponse, OpencodeError> {
        let url = format!("{}/session/{}/message", self.base_url, session_id);

        let body = PromptBody {
            agent: "architect".to_string(),
            parts: vec![PromptPart {
                part_type: "text".to_string(),
                text: task.to_string(),
            }],
        };

        let res = self.client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| OpencodeError::RequestFailed(e.to_string()))?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(OpencodeError::ServerError(
                status.as_u16(),
                body,
            ));
        }

        res.json::<PromptResponse>()
            .await
            .map_err(|e| OpencodeError::ParseFailed(e.to_string()))
    }

    /// Extract text content from response parts
    ///
    /// # Arguments
    ///
    /// * `response` - Response from Opencode
    ///
    /// # Returns
    ///
    /// Concatenated text from all text parts
    fn extract_text_from_response(&self, response: &PromptResponse) -> Result<String, OpencodeError> {
        let mut text_parts = Vec::new();

        for part in &response.parts {
            if let Some(part_type) = part.get("type").and_then(|v| v.as_str()) {
                if part_type == "text" {
                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                        text_parts.push(text.to_string());
                    }
                }
            }
        }

        if text_parts.is_empty() {
            // If no text parts, try to get from info
            if let Some(summary) = response.info.get("summary").and_then(|v| v.as_str()) {
                return Ok(summary.to_string());
            }
            return Err(OpencodeError::NoContent);
        }

        Ok(text_parts.join("\n"))
    }

    /// Check if Opencode server is healthy
    ///
    /// # Returns
    ///
    /// `Ok(())` if server is healthy, `Err` otherwise
    pub async fn health_check(&self) -> Result<(), OpencodeError> {
        let url = format!("{}/vcs", self.base_url);

        let res = self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| OpencodeError::RequestFailed(e.to_string()))?;

        if res.status().is_success() {
            Ok(())
        } else {
            Err(OpencodeError::Unhealthy)
        }
    }
}

// Convert OpencodeError to String for compatibility with existing code
impl From<OpencodeError> for String {
    fn from(err: OpencodeError) -> Self {
        err.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "Requires running Opencode server"]
    async fn test_opencode_provider_health_check() {
        let provider = OpencodeToolProvider::new("http://127.0.0.1:4096".to_string());

        let result = provider.health_check().await;
        assert!(result.is_ok(), "Opencode server should be healthy");
    }

    #[tokio::test]
    #[ignore = "Requires running Opencode server"]
    async fn test_opencode_execute_task() {
        let provider = OpencodeToolProvider::new("http://127.0.0.1:4096".to_string());

        let result = provider.execute_task("list files in current directory").await;
        assert!(result.is_ok(), "Task should execute successfully");

        let output = result.unwrap();
        assert!(!output.is_empty(), "Output should not be empty");
    }

    #[tokio::test]
    #[ignore = "Requires running Opencode server"]
    async fn test_opencode_with_custom_timeout() {
        let provider = OpencodeToolProvider::new("http://127.0.0.1:4096".to_string())
            .with_timeout(Duration::from_secs(600));

        let result = provider.health_check().await;
        assert!(result.is_ok(), "Health check should succeed with custom timeout");
    }
}
