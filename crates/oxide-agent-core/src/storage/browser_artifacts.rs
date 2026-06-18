//! Browser artifact record types for Postgres BYTEA storage.
//!
//! Screenshots are stored as JPEG bytes in the `browser_artifacts` table.
//! The `artifact_uri` is the primary key and lookup key — no filesystem path
//! needed. Deletion cascades from `web_tasks` via FK `ON DELETE CASCADE`.

/// A browser screenshot artifact stored in Postgres.
#[derive(Debug, Clone)]
pub struct BrowserArtifactRecord {
    /// Primary key — `artifact://browser/{task_id}/{session_id}/step-NNNN-{purpose}.jpg`
    pub artifact_uri: String,
    /// Owning user ID (FK to `web_tasks`).
    pub user_id: i64,
    /// Owning session ID (FK to `web_tasks`).
    pub session_id: String,
    /// Owning task ID (FK to `web_tasks`).
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
