//! Jira MCP provider configuration.

use serde::{Deserialize, Serialize};

/// Default binary paths to check (in order of priority).
const DEFAULT_BINARY_PATHS: &[&str] = &["/usr/local/bin/jira-mcp", "/usr/bin/jira-mcp"];

/// Configuration for the Jira MCP provider.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JiraMcpConfig {
    /// Path to the jira-mcp binary.
    pub binary_path: String,
    /// Jira Server base URL (e.g., https://jira.company.com).
    pub jira_url: String,
    /// Jira user email/username.
    pub jira_email: String,
    /// Jira API token or password.
    pub jira_token: String,
}

impl JiraMcpConfig {
    /// Attempts to create configuration from environment variables.
    ///
    /// Required environment variables:
    /// - `JIRA_URL`: Jira Server base URL
    /// - `JIRA_EMAIL`: Jira user email
    /// - `JIRA_API_TOKEN`: Jira API token
    ///
    /// Optional environment variables:
    /// - `JIRA_MCP_BINARY_PATH`: Path to the jira-mcp binary (auto-detected if not set)
    ///
    /// Auto-detection checks the following paths in order:
    /// 1. `/usr/local/bin/jira-mcp`
    /// 2. `/usr/bin/jira-mcp`
    /// 3. Falls back to `jira-mcp` (searches in PATH)
    ///
    /// Returns `None` if any required variable is missing or binary not found.
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("JIRA_URL").ok()?;
        let email = std::env::var("JIRA_EMAIL").ok()?;
        let token = std::env::var("JIRA_API_TOKEN").ok()?;

        // Validate that values are non-empty
        if url.is_empty() || email.is_empty() || token.is_empty() {
            return None;
        }

        // Resolve binary path with auto-detection
        let binary_path = Self::resolve_binary_path()?;

        Some(Self {
            binary_path,
            jira_url: url,
            jira_email: email,
            jira_token: token,
        })
    }

    /// Resolves the jira-mcp binary path.
    ///
    /// Priority:
    /// 1. `JIRA_MCP_BINARY_PATH` environment variable
    /// 2. Default paths in `DEFAULT_BINARY_PATHS`
    /// 3. Falls back to `jira-mcp` (expects it to be in PATH)
    fn resolve_binary_path() -> Option<String> {
        // Check explicit environment variable first
        if let Ok(path) = std::env::var("JIRA_MCP_BINARY_PATH") {
            if !path.is_empty() {
                return Some(path);
            }
        }

        // Check default paths
        for path in DEFAULT_BINARY_PATHS {
            if std::path::Path::new(path).exists() {
                return Some(path.to_string());
            }
        }

        // Fallback: assume binary is in PATH
        Some("jira-mcp".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_binary_paths_defined() {
        assert!(!DEFAULT_BINARY_PATHS.is_empty());
        assert_eq!(DEFAULT_BINARY_PATHS[0], "/usr/local/bin/jira-mcp");
    }

    #[test]
    fn test_config_struct_creation() {
        let config = JiraMcpConfig {
            binary_path: "/usr/bin/jira-mcp".to_string(),
            jira_url: "https://test.atlassian.net".to_string(),
            jira_email: "test@example.com".to_string(),
            jira_token: "secret123".to_string(),
        };

        assert_eq!(config.binary_path, "/usr/bin/jira-mcp");
        assert_eq!(config.jira_url, "https://test.atlassian.net");
        assert_eq!(config.jira_email, "test@example.com");
        assert_eq!(config.jira_token, "secret123");
    }
}
