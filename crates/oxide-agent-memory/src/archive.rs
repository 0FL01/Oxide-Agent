//! Archive blob storage trait for large-object persistence.
//!
//! Decoupled from the typed memory repository so that blob storage
//! can be backed by R2, local filesystem, or any object store.

use crate::types::ArtifactRef;
use anyhow::Result;

/// Abstraction over raw blob/object storage for archived content.
///
/// Implementations may persist to R2, local filesystem, or any
/// object store. The typed memory layer stores only references
/// (`ArtifactRef`) and delegates actual payload I/O here.
#[allow(async_fn_in_trait)]
pub trait ArchiveBlobStore: Send + Sync {
    /// Persist a blob and return a stable reference.
    ///
    /// `key` is a caller-provided storage path (e.g.
    /// `"archive/{context_key}/{flow_id}/history-{uuid}.json"`).
    /// `content_type` is a MIME hint (e.g. `"application/json"`).
    fn put(
        &self,
        key: &str,
        data: &[u8],
        content_type: Option<&str>,
    ) -> impl std::future::Future<Output = Result<ArtifactRef>> + Send;

    /// Retrieve a previously stored blob.
    ///
    /// Returns `None` if the key does not exist.
    fn get(&self, key: &str) -> impl std::future::Future<Output = Result<Option<Vec<u8>>>> + Send;

    /// Delete a stored blob.
    ///
    /// Silently succeeds if the key does not exist.
    fn delete(&self, key: &str) -> impl std::future::Future<Output = Result<()>> + Send;

    /// Check whether a blob exists.
    fn exists(&self, key: &str) -> impl std::future::Future<Output = Result<bool>> + Send;
}
