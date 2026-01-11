//! Shared helpers for providers that work with sandbox file paths.

use crate::sandbox::SandboxManager;
use anyhow::Result;
use shell_escape::escape;
use tracing::{info, warn};

/// Resolve a relative path to an absolute path in the sandbox.
///
/// Rules:
/// - Absolute paths are returned as-is.
/// - Relative paths are treated as `/workspace/<path>`.
/// - If not found, performs a `find` under `/workspace`.
///
/// # Errors
///
/// Returns an error if the file is not found or if multiple matches exist.
pub(super) async fn resolve_file_path(sandbox: &SandboxManager, path: &str) -> Result<String> {
    if path.starts_with('/') {
        return Ok(path.to_string());
    }

    let workspace_path = format!("/workspace/{path}");
    let check_cmd = format!(
        "test -f {} && echo 'exists'",
        escape(workspace_path.as_str().into())
    );
    let check = sandbox.exec_command(&check_cmd, None).await?;

    if check.stdout.contains("exists") {
        info!(original_path = %path, resolved_path = %workspace_path, "Resolved file path");
        return Ok(workspace_path);
    }

    info!(path = %path, "File not found at /workspace/{path}, searching...");
    let find_cmd = format!("find /workspace -name {} -type f", escape(path.into()));
    let result = sandbox.exec_command(&find_cmd, None).await?;

    let found_paths: Vec<&str> = result.stdout.lines().filter(|l| !l.is_empty()).collect();

    match found_paths.len() {
        0 => anyhow::bail!(
            "File '{}' not found in sandbox. Use 'list_files' tool to see available files.",
            path
        ),
        1 => {
            let resolved = found_paths[0].to_string();
            info!(original_path = %path, resolved_path = %resolved, "Found file");
            Ok(resolved)
        }
        _ => {
            warn!(path = %path, matches = found_paths.len(), "Multiple files found");
            let paths_list = found_paths.join("\n  - ");
            anyhow::bail!(
                "Multiple files found with name '{}':\n  - {}\n\nPlease specify the full path to the desired file.",
                path, paths_list
            )
        }
    }
}
