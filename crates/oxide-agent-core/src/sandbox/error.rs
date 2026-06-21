//! Typed sandbox error enum.
//!
//! Replaces the previous `anyhow`-based error handling throughout the
//! sandbox subsystem. Each variant represents a distinct failure class
//! that callers can match on programmatically.

use thiserror::Error;

/// Typed error for sandbox operations.
#[derive(Debug, Error)]
pub enum SandboxError {
    /// Sandbox container is not running.
    #[error("sandbox is not running")]
    NotRunning,

    /// Sandbox container was not found after creation or recreation.
    #[error("sandbox container not found: {0}")]
    ContainerNotFound(String),

    /// Command execution timed out.
    #[error("command execution timed out after {0}s")]
    ExecTimeout(u64),

    /// Command execution was cancelled by the user.
    #[error("command execution cancelled")]
    Cancelled,

    /// File was not found in the sandbox workspace.
    #[error("file not found in sandbox: {0}")]
    FileNotFound(String),

    /// Sandbox backend is not compiled in this build.
    #[error("sandbox backend not compiled: {0}")]
    BackendNotCompiled(&'static str),

    /// Sandbox broker returned an error response.
    #[error("sandbox broker error: {0}")]
    Broker(String),

    /// Sandbox protocol error (unexpected response, encoding/decoding failure).
    #[error("sandbox protocol error: {0}")]
    Protocol(String),

    /// Invalid file edit parameters (binary content, encoding, replacement count, etc.).
    #[error("invalid file edit: {0}")]
    InvalidEdit(String),

    /// File read guard mismatch (file changed after last read or guard missing).
    #[error("file read guard mismatch: {0}")]
    ReadGuardMismatch(String),

    /// Docker daemon error (connection, API, container operations).
    #[cfg(feature = "sandbox-backend-docker-direct")]
    #[error("Docker error: {0}")]
    Docker(#[from] bollard::errors::Error),

    /// IO error (socket, file system).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Other sandbox error not covered by a specific variant.
    #[error("{0}")]
    Other(String),
}
