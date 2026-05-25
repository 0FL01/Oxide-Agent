//! References to externalized tool payloads stored outside hot memory.

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
