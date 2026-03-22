//! Jira MCP provider configuration.

use serde::{Deserialize, Serialize};

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
    /// - `JIRA_MCP_BINARY_PATH`: Path to the jira-mcp binary
    /// - `JIRA_URL`: Jira Server base URL
    /// - `JIRA_EMAIL`: Jira user email
    /// - `JIRA_API_TOKEN`: Jira API token
    ///
    /// Returns `None` if any required variable is missing.
    pub fn from_env() -> Option<Self> {
        let binary = std::env::var("JIRA_MCP_BINARY_PATH").ok()?;
        let url = std::env::var("JIRA_URL").ok()?;
        let email = std::env::var("JIRA_EMAIL").ok()?;
        let token = std::env::var("JIRA_API_TOKEN").ok()?;

        // Validate that values are non-empty
        if binary.is_empty() || url.is_empty() || email.is_empty() || token.is_empty() {
            return None;
        }

        Some(Self {
            binary_path: binary,
            jira_url: url,
            jira_email: email,
            jira_token: token,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_env_missing_vars() {
        // Ensure environment variables are not set
        for var in [
            "JIRA_MCP_BINARY_PATH",
            "JIRA_URL",
            "JIRA_EMAIL",
            "JIRA_API_TOKEN",
        ] {
            unsafe { std::env::remove_var(var) };
        }

        assert!(JiraMcpConfig::from_env().is_none());
    }

    #[test]
    fn test_from_env_all_vars_set() {
        unsafe {
            std::env::set_var("JIRA_MCP_BINARY_PATH", "/usr/local/bin/jira-mcp");
            std::env::set_var("JIRA_URL", "https://jira.company.com");
            std::env::set_var("JIRA_EMAIL", "agent@company.com");
            std::env::set_var("JIRA_API_TOKEN", "secret-token");
        }

        let config = JiraMcpConfig::from_env().expect("should parse config");
        assert_eq!(config.binary_path, "/usr/local/bin/jira-mcp");
        assert_eq!(config.jira_url, "https://jira.company.com");
        assert_eq!(config.jira_email, "agent@company.com");
        assert_eq!(config.jira_token, "secret-token");

        // Cleanup
        unsafe {
            std::env::remove_var("JIRA_MCP_BINARY_PATH");
            std::env::remove_var("JIRA_URL");
            std::env::remove_var("JIRA_EMAIL");
            std::env::remove_var("JIRA_API_TOKEN");
        }
    }
}
