//! Browser artifact record types for Postgres BYTEA storage.
//!
//! Screenshots are stored as JPEG bytes in the `browser_artifacts` table.
//! The `artifact_uri` is the primary key and lookup key — no filesystem path
//! needed. Deletion is by `(user_id, context_key)` — the transport-agnostic
//! session identifier from `AgentMemoryScope`. No FK: the browser provider
//! (core layer) does not have web-task IDs, so explicit cleanup is used
//! instead of a CASCADE that the sending side cannot satisfy.

/// A browser screenshot artifact stored in Postgres.
#[derive(Debug, Clone)]
pub struct BrowserArtifactRecord {
    /// Primary key — `artifact://browser/{task_id}/{session_id}/step-NNNN-{purpose}.jpg`
    pub artifact_uri: String,
    /// Owning user ID.
    pub user_id: i64,
    /// Transport-agnostic session identifier (from `AgentMemoryScope.context_key`).
    /// Used for deletion when a session is deleted.
    pub context_key: String,
    /// Browser session ID (informational, from sidecar).
    pub session_id: String,
    /// Browser task ID (informational, LLM-provided or fallback).
    pub task_id: String,
    /// MIME type, always `image/jpeg` for screenshots.
    pub mime_type: String,
    /// Raw image bytes (JPEG).
    pub data: Vec<u8>,
    /// Size of `data` in bytes.
    pub bytes: i64,
    /// SHA-256 hex digest of `data`.
    pub sha256: Option<String>,
}

/// Loaded artifact data — subset returned by `load_browser_artifact`.
#[derive(Debug, Clone)]
pub struct BrowserArtifactData {
    /// MIME type, always `image/jpeg` for screenshots.
    pub mime_type: String,
    /// Raw image bytes (JPEG).
    pub data: Vec<u8>,
    /// Size of `data` in bytes.
    pub bytes: i64,
}

impl From<BrowserArtifactRecord> for BrowserArtifactData {
    fn from(record: BrowserArtifactRecord) -> Self {
        Self {
            mime_type: record.mime_type,
            bytes: record.bytes,
            data: record.data,
        }
    }
}
