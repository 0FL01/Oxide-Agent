//! Artifact references emitted by the tool runtime.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Runtime artifact kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// Captured stdout stream.
    Stdout,
    /// Captured stderr stream.
    Stderr,
    /// Structured payload that was too large for inline model context.
    StructuredPayload,
    /// Tool-produced file artifact.
    File,
    /// Runtime diagnostic log.
    Log,
}

/// Internal artifact reference. Public download URLs are opt-in and separate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    /// Stable internal URI resolvable by Oxide Agent.
    pub uri: String,
    /// Local path for the stored artifact.
    pub local_path: PathBuf,
    /// Optional explicit user-download URI created by delivery/upload tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_download_uri: Option<String>,
    /// Artifact kind.
    pub kind: ArtifactKind,
    /// Artifact size in bytes.
    pub bytes: u64,
    /// Optional SHA-256 checksum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Optional retention expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// Whether the artifact represents a partial result from timeout/cancel/hung.
    #[serde(default)]
    pub partial: bool,
}

impl ArtifactRef {
    /// Build an internal artifact reference without making it user-downloadable.
    #[must_use]
    pub fn internal(
        uri: impl Into<String>,
        local_path: impl Into<PathBuf>,
        kind: ArtifactKind,
        bytes: u64,
    ) -> Self {
        Self {
            uri: uri.into(),
            local_path: local_path.into(),
            user_download_uri: None,
            kind,
            bytes,
            sha256: None,
            expires_at: None,
            partial: false,
        }
    }

    /// Mark the artifact as partial.
    #[must_use]
    pub fn partial(mut self) -> Self {
        self.partial = true;
        self
    }
}
