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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Freeform tags for filtering.
    pub tags: Vec<String>,
    /// When the memory was first created.
    pub created_at: DateTime<Utc>,
    /// When the memory was last updated.
    pub updated_at: DateTime<Utc>,
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
    /// Maximum number of results to return.
    pub limit: Option<usize>,
}
