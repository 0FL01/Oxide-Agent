//! Archive extension points for future cold-context persistence.

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Reference to an archived context chunk persisted outside hot memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveRef {
    /// Stable archive identifier.
    pub archive_id: String,
    /// Unix timestamp when the archive record was created.
    pub created_at: i64,
    /// Short human-readable title used for future discovery.
    pub title: String,
    /// Storage key or object path holding the archived payload.
    pub storage_key: String,
}

/// Persisted archive metadata for future retrieval-oriented features.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveRecord {
    /// Stable archive identifier.
    pub archive_id: String,
    /// Scoped context key (topic/thread scope).
    pub context_key: String,
    /// Agent flow identifier.
    pub flow_id: String,
    /// Unix timestamp when the record was created.
    pub created_at: i64,
    /// Inclusive lower bound of the chunk time range.
    pub time_range_start: i64,
    /// Inclusive upper bound of the chunk time range.
    pub time_range_end: i64,
    /// Short title for future lookup.
    pub title: String,
    /// Short summary for future lookup.
    pub short_summary: String,
    /// Logical archive record kind.
    #[serde(default)]
    pub kind: String,
    /// Tool names associated with this archived chunk.
    #[serde(default)]
    pub tool_names: Vec<String>,
    /// File paths associated with this archived chunk.
    #[serde(default)]
    pub file_paths: Vec<String>,
    /// Storage key or payload reference for the archived content.
    #[serde(default)]
    pub payload_ref: String,
}

/// Persistence sink for future compaction archive chunks.
pub trait ArchiveSink: Send + Sync {
    /// Persist an archive record and optionally return a stable reference.
    fn persist(&self, record: &ArchiveRecord) -> Result<Option<ArchiveRef>>;
}

/// Placeholder sink used until archive persistence is implemented.
#[derive(Debug, Default)]
pub struct NoopArchiveSink;

impl ArchiveSink for NoopArchiveSink {
    fn persist(&self, _record: &ArchiveRecord) -> Result<Option<ArchiveRef>> {
        Ok(None)
    }
}
