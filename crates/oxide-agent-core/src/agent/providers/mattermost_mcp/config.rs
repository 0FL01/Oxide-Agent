//! Mattermost MCP provider configuration.

use serde::{Deserialize, Serialize};

const DEFAULT_BINARY_PATHS: &[&str] = &[
    "/usr/local/bin/mattermost-mcp",
    "/usr/bin/mattermost-mcp",
    "/usr/local/bin/mcp-server-mattermost",
    "/usr/bin/mcp-server-mattermost",
];

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_MAX_RETRIES: u32 = 3;
const DEFAULT_VERIFY_SSL: bool = true;

/// Configuration for the Mattermost MCP provider.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MattermostMcpConfig {
    /// Path to the mattermost MCP binary.
    pub binary_path: String,
    /// Mattermost base URL (for example, https://mattermost.company.com).
    pub mattermost_url: String,
    /// Mattermost bot or user token.
    pub mattermost_token: String,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Maximum upstream retry attempts.
    pub max_retries: u32,
    /// Whether to verify SSL certificates.
    pub verify_ssl: bool,
}

impl MattermostMcpConfig {
    /// Attempts to create configuration from environment variables.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let mattermost_url = std::env::var("MATTERMOST_URL").ok()?;
        let mattermost_token = std::env::var("MATTERMOST_TOKEN").ok()?;

        if mattermost_url.trim().is_empty() || mattermost_token.trim().is_empty() {
            return None;
        }

        let binary_path = Self::resolve_binary_path()?;

        Some(Self {
            binary_path,
            mattermost_url,
            mattermost_token,
            timeout_secs: Self::env_u64("MATTERMOST_TIMEOUT", DEFAULT_TIMEOUT_SECS),
            max_retries: Self::env_u32("MATTERMOST_MAX_RETRIES", DEFAULT_MAX_RETRIES),
            verify_ssl: Self::env_bool("MATTERMOST_VERIFY_SSL", DEFAULT_VERIFY_SSL),
        })
    }

    fn resolve_binary_path() -> Option<String> {
        if let Ok(path) = std::env::var("MATTERMOST_MCP_BINARY_PATH") {
            if !path.trim().is_empty() {
                return Some(path);
            }
        }

        for path in DEFAULT_BINARY_PATHS {
            if std::path::Path::new(path).exists() {
                return Some((*path).to_string());
            }
        }

        Some("mattermost-mcp".to_string())
    }

    fn env_u64(name: &str, default: u64) -> u64 {
        std::env::var(name)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(default)
    }

    fn env_u32(name: &str, default: u32) -> u32 {
        std::env::var(name)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .unwrap_or(default)
    }

    fn env_bool(name: &str, default: bool) -> bool {
        std::env::var(name)
            .ok()
            .and_then(|value| parse_bool(&value))
            .unwrap_or(default)
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_binary_paths_defined() {
        assert!(!DEFAULT_BINARY_PATHS.is_empty());
        assert_eq!(DEFAULT_BINARY_PATHS[0], "/usr/local/bin/mattermost-mcp");
    }

    #[test]
    fn test_config_struct_creation() {
        let config = MattermostMcpConfig {
            binary_path: "/usr/bin/mattermost-mcp".to_string(),
            mattermost_url: "https://mattermost.example.com".to_string(),
            mattermost_token: "secret123".to_string(),
            timeout_secs: 45,
            max_retries: 5,
            verify_ssl: false,
        };

        assert_eq!(config.binary_path, "/usr/bin/mattermost-mcp");
        assert_eq!(config.mattermost_url, "https://mattermost.example.com");
        assert_eq!(config.mattermost_token, "secret123");
        assert_eq!(config.timeout_secs, 45);
        assert_eq!(config.max_retries, 5);
        assert!(!config.verify_ssl);
    }

    #[test]
    fn test_parse_bool_variants() {
        assert_eq!(parse_bool("true"), Some(true));
        assert_eq!(parse_bool("YES"), Some(true));
        assert_eq!(parse_bool("0"), Some(false));
        assert_eq!(parse_bool("off"), Some(false));
        assert_eq!(parse_bool("maybe"), None);
    }
}
