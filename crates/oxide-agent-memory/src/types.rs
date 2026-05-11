//! Domain types for the persistent memory subsystem.
//!
//! These types represent the typed memory model described in the persistent-memory plan.
//! They are storage-agnostic: concrete persistence backends (in-memory, Postgres, etc.)
//! map these types to their own representations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Thread
// ---------------------------------------------------------------------------

/// Unique identifier for a thread.
pub type ThreadId = String;

/// A conversation / topic thread card.
///
/// One thread corresponds to one sustained dialogue within a topic context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadRecord {
    /// Stable thread identifier.
    pub thread_id: ThreadId,
    /// User who owns this thread.
    pub user_id: i64,
    /// Transport context key (topic / thread scope).
    pub context_key: String,
    /// Short human-readable title.
    pub title: String,
    /// Brief summary of the thread's purpose.
    pub short_summary: String,
    /// When the thread was first created.
    pub created_at: DateTime<Utc>,
    /// When the thread was last updated.
    pub updated_at: DateTime<Utc>,
    /// When the last meaningful activity happened.
    pub last_activity_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Episode
// ---------------------------------------------------------------------------

/// Unique identifier for an episode.
pub type EpisodeId = String;

/// Outcome of a completed episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeOutcome {
    /// Task completed successfully.
    Success,
    /// Task completed with partial results or warnings.
    Partial,
    /// Task failed.
    Failure,
    /// Task was cancelled by user or system.
    Cancelled,
}

/// A compact record of a completed task / work session within a thread.
///
/// One episode = one notable task, subtask, or working session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpisodeRecord {
    /// Stable episode identifier.
    pub episode_id: EpisodeId,
    /// Parent thread.
    pub thread_id: ThreadId,
    /// Transport context key.
    pub context_key: String,
    /// What the user wanted to achieve.
    pub goal: String,
    /// Compact summary of what happened.
    pub summary: String,
    /// How the episode ended.
    pub outcome: EpisodeOutcome,
    /// Tools used during this episode.
    pub tools_used: Vec<String>,
    /// References to artifacts created during this episode.
    pub artifacts: Vec<ArtifactRef>,
    /// Notable failures encountered.
    pub failures: Vec<String>,
    /// Estimated importance (0.0 – 1.0).
    pub importance: f32,
    /// When the episode was created.
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

/// Unique identifier for a memory record.
pub type MemoryId = String;

/// Kind of semantic / procedural memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// A verifiable fact about the project / topic.
    Fact,
    /// A user preference or habitual pattern.
    Preference,
    /// A reusable working procedure or playbook.
    Procedure,
    /// An architectural or design decision.
    Decision,
    /// A constraint or limitation to respect.
    Constraint,
}

/// A normalized, reusable memory record.
///
/// Extracted from episodes or explicitly written by the agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryRecord {
    /// Stable memory identifier.
    pub memory_id: MemoryId,
    /// Transport context key.
    pub context_key: String,
    /// Episode this memory was extracted from (if any).
    pub source_episode_id: Option<EpisodeId>,
    /// Classification of this memory.
    pub memory_type: MemoryType,
    /// Short human-readable title.
    pub title: String,
    /// Full memory content.
    pub content: String,
    /// Brief description for retrieval previews.
    pub short_description: String,
    /// Estimated importance (0.0 – 1.0).
    pub importance: f32,
    /// Confidence in the accuracy / relevance of this memory (0.0 – 1.0).
    pub confidence: f32,
    /// Explicit origin for auditability (e.g. tool, extractor, user request).
    #[serde(default)]
    pub source: Option<String>,
    /// Stable fingerprint of normalized memory content for cross-episode deduplication.
    #[serde(default)]
    pub content_hash: Option<String>,
    /// Why this memory was stored.
    #[serde(default)]
    pub reason: Option<String>,
    /// Freeform tags for filtering.
    pub tags: Vec<String>,
    /// When the memory was first created.
    pub created_at: DateTime<Utc>,
    /// When the memory was last updated.
    pub updated_at: DateTime<Utc>,
    /// When the memory was superseded or expired and should stop participating in retrieval.
    #[serde(default)]
    pub deleted_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Unique identifier for a session state record.
pub type SessionStateId = String;

/// Persistent state of an active (or recently active) session.
///
/// Used for session resumption, deferred finalization, and background cleanup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionStateRecord {
    /// Session identifier.
    pub session_id: SessionStateId,
    /// Transport context key.
    pub context_key: String,
    /// Estimated hot token count at last checkpoint.
    pub hot_token_estimate: usize,
    /// When the last compaction ran.
    pub last_compacted_at: Option<DateTime<Utc>>,
    /// When the last episode finalization ran.
    pub last_finalized_at: Option<DateTime<Utc>>,
    /// Current cleanup status.
    pub cleanup_status: CleanupStatus,
    /// Episode pending finalization (if any).
    pub pending_episode_id: Option<EpisodeId>,
    /// When this record was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Status of session cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupStatus {
    /// Session is active, no cleanup pending.
    Active,
    /// Session is idle, deferred cleanup may run.
    Idle,
    /// Cleanup is in progress.
    Cleaning,
    /// Session has been fully finalized.
    Finalized,
}

// ---------------------------------------------------------------------------
// Artifact ref
// ---------------------------------------------------------------------------

/// A lightweight reference to an artifact (file, sandbox output, etc.).
///
/// Stored inside episode records to link work products to their originating episode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactRef {
    /// Storage key or path where the artifact lives.
    pub storage_key: String,
    /// Human-readable description.
    pub description: String,
    /// MIME type or format hint (e.g. `"application/json"`, `"text/plain"`).
    #[serde(default)]
    pub content_type: Option<String>,
    /// Explicit origin for auditability (e.g. tool, extractor, user request).
    #[serde(default)]
    pub source: Option<String>,
    /// Why this artifact was linked.
    #[serde(default)]
    pub reason: Option<String>,
    /// Freeform tags for filtering and audit.
    #[serde(default)]
    pub tags: Vec<String>,
    /// When the artifact was created.
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Common query types
// ---------------------------------------------------------------------------

/// Filter parameters for listing episodes within a thread.
#[derive(Debug, Clone, Default)]
pub struct EpisodeListFilter {
    /// Only return episodes with at least this importance.
    pub min_importance: Option<f32>,
    /// Only return episodes matching this outcome.
    pub outcome: Option<EpisodeOutcome>,
    /// Maximum number of results to return.
    pub limit: Option<usize>,
}

/// Filter parameters for listing memories.
#[derive(Debug, Clone, Default)]
pub struct MemoryListFilter {
    /// Filter by memory type.
    pub memory_type: Option<MemoryType>,
    /// Only return memories with at least this importance.
    pub min_importance: Option<f32>,
    /// Filter by tag presence.
    pub tags: Vec<String>,
    /// Include soft-deleted records in the result set.
    pub include_deleted: bool,
    /// Maximum number of results to return.
    pub limit: Option<usize>,
}

/// Filter parameters for listing tracked session states.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionStateListFilter {
    /// Restrict results to one transport context.
    pub context_key: Option<String>,
    /// Restrict results to the provided cleanup statuses.
    pub statuses: Vec<CleanupStatus>,
    /// Only return records updated before or at this timestamp.
    pub updated_before: Option<DateTime<Utc>>,
    /// Maximum number of results to return.
    pub limit: Option<usize>,
}

/// Optional inclusive time range for search and retrieval filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimeRange {
    /// Lower bound for timestamps.
    pub since: Option<DateTime<Utc>>,
    /// Upper bound for timestamps.
    pub until: Option<DateTime<Utc>>,
}

/// Filter parameters for lexical episode search.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EpisodeSearchFilter {
    /// Restrict results to one transport context.
    pub context_key: Option<String>,
    /// Restrict results to threads owned by one user.
    pub user_id: Option<i64>,
    /// Restrict results to a specific outcome.
    pub outcome: Option<EpisodeOutcome>,
    /// Only return episodes with at least this importance.
    pub min_importance: Option<f32>,
    /// Restrict results to a time range over `created_at`.
    pub time_range: TimeRange,
    /// Maximum number of results to return.
    pub limit: Option<usize>,
}

/// Ranked lexical hit for an episode search.
#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeSearchHit {
    /// Matching episode record.
    pub record: EpisodeRecord,
    /// Backend-provided lexical relevance score.
    pub score: f32,
    /// Short preview showing the matched content.
    pub snippet: String,
}

/// Filter parameters for lexical reusable-memory search.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemorySearchFilter {
    /// Restrict results to one transport context.
    pub context_key: Option<String>,
    /// Restrict results to memories linked to threads owned by one user.
    pub user_id: Option<i64>,
    /// Restrict results to a specific memory kind.
    pub memory_type: Option<MemoryType>,
    /// Only return memories with at least this importance.
    pub min_importance: Option<f32>,
    /// Require every tag in this list to be present.
    pub tags: Vec<String>,
    /// Restrict results to a time range over `updated_at`.
    pub time_range: TimeRange,
    /// Maximum number of results to return.
    pub limit: Option<usize>,
}

/// Ranked lexical hit for a reusable-memory search.
#[derive(Debug, Clone, PartialEq)]
pub struct MemorySearchHit {
    /// Matching reusable memory record.
    pub record: MemoryRecord,
    /// Backend-provided lexical relevance score.
    pub score: f32,
    /// Short preview showing the matched content.
    pub snippet: String,
}

/// Embedded owner kind stored in `memory_embeddings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingOwnerType {
    /// Embedding belongs to an episodic record.
    Episode,
    /// Embedding belongs to a reusable memory record.
    Memory,
}

/// Current indexing status for one embedding row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingStatus {
    /// Row is queued or waiting for embedding generation.
    Pending,
    /// Embedding vector is available and searchable.
    Ready,
    /// Latest indexing attempt failed.
    Failed,
}

/// Persisted embedding state for one episode or memory record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbeddingRecord {
    /// Record identifier from the owning table.
    pub owner_id: String,
    /// Type of owning record.
    pub owner_type: EmbeddingOwnerType,
    /// Embedding model used to index the content.
    pub model_id: String,
    /// Stable content hash used for audit and reindex tracking.
    pub content_hash: String,
    /// Stored vector when indexing succeeded.
    pub embedding: Option<Vec<f32>>,
    /// Cached dimension of the stored vector.
    pub dimensions: Option<usize>,
    /// Current indexing status.
    pub status: EmbeddingStatus,
    /// Last indexing error, if any.
    pub last_error: Option<String>,
    /// Number of failed indexing attempts seen for this row.
    pub retry_count: u32,
    /// When the embedding row was first created.
    pub created_at: DateTime<Utc>,
    /// When the embedding row was last updated.
    pub updated_at: DateTime<Utc>,
    /// When the latest successful indexing completed.
    pub indexed_at: Option<DateTime<Utc>>,
}

/// Common parameters for pending / ready / failed embedding updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingUpdateBase {
    /// Record identifier from the owning table.
    pub owner_id: String,
    /// Type of owning record.
    pub owner_type: EmbeddingOwnerType,
    /// Embedding model used to index the content.
    pub model_id: String,
    /// Stable content hash used for audit and reindex tracking.
    pub content_hash: String,
}

/// Request to mark one embedding as pending generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingPendingUpdate {
    /// Shared owner/model metadata.
    pub base: EmbeddingUpdateBase,
    /// Timestamp of the pending marker.
    pub requested_at: DateTime<Utc>,
}

/// Request to store a successful embedding vector.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingReadyUpdate {
    /// Shared owner/model metadata.
    pub base: EmbeddingUpdateBase,
    /// Generated dense vector.
    pub embedding: Vec<f32>,
    /// Timestamp of the successful indexing operation.
    pub indexed_at: DateTime<Utc>,
}

/// Request to record an embedding indexing failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingFailureUpdate {
    /// Shared owner/model metadata.
    pub base: EmbeddingUpdateBase,
    /// Failure text preserved for audit/debugging.
    pub error: String,
    /// Timestamp of the failed indexing operation.
    pub failed_at: DateTime<Utc>,
}

/// Parameters for bounded embedding backfill discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingBackfillRequest {
    /// Target embedding model that should exist on all returned records.
    pub model_id: String,
    /// Maximum number of candidates to return.
    pub limit: Option<usize>,
}

/// Episode plus its current embedding state for indexing/backfill.
#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeEmbeddingCandidate {
    /// Episode record requiring indexing or reindexing.
    pub record: EpisodeRecord,
    /// Existing embedding state, if any.
    pub embedding: Option<EmbeddingRecord>,
}

/// Reusable memory plus its current embedding state for indexing/backfill.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryEmbeddingCandidate {
    /// Memory record requiring indexing or reindexing.
    pub record: MemoryRecord,
    /// Existing embedding state, if any.
    pub embedding: Option<EmbeddingRecord>,
}
