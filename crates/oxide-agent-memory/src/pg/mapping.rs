//! Mapping helpers between domain records and Postgres rows.

use crate::repository::RepositoryError;
use crate::types::{
    ArtifactRef, CleanupStatus, EpisodeOutcome, EpisodeRecord, EpisodeSearchHit, MemoryRecord,
    MemorySearchHit, MemoryType, SessionStateRecord, ThreadRecord,
};
use chrono::{DateTime, Utc};
use sqlx::types::Json;

/// Row shape for `memory_threads`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct ThreadRow {
    pub thread_id: String,
    pub user_id: i64,
    pub context_key: String,
    pub title: String,
    pub short_summary: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
}

impl From<ThreadRow> for ThreadRecord {
    fn from(value: ThreadRow) -> Self {
        Self {
            thread_id: value.thread_id,
            user_id: value.user_id,
            context_key: value.context_key,
            title: value.title,
            short_summary: value.short_summary,
            created_at: value.created_at,
            updated_at: value.updated_at,
            last_activity_at: value.last_activity_at,
        }
    }
}

/// Row shape for `memory_episodes`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct EpisodeRow {
    pub episode_id: String,
    pub thread_id: String,
    pub context_key: String,
    pub goal: String,
    pub summary: String,
    pub outcome: String,
    pub tools_used: Vec<String>,
    pub artifacts: Json<Vec<ArtifactRef>>,
    pub failures: Vec<String>,
    pub importance: f32,
    pub created_at: DateTime<Utc>,
}

impl TryFrom<EpisodeRow> for EpisodeRecord {
    type Error = RepositoryError;

    fn try_from(value: EpisodeRow) -> Result<Self, Self::Error> {
        Ok(Self {
            episode_id: value.episode_id,
            thread_id: value.thread_id,
            context_key: value.context_key,
            goal: value.goal,
            summary: value.summary,
            outcome: decode_episode_outcome(&value.outcome)?,
            tools_used: value.tools_used,
            artifacts: value.artifacts.0,
            failures: value.failures,
            importance: value.importance,
            created_at: value.created_at,
        })
    }
}

/// Row shape for `memory_records`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct MemoryRow {
    pub memory_id: String,
    pub context_key: String,
    pub source_episode_id: Option<String>,
    pub memory_type: String,
    pub title: String,
    pub content: String,
    pub short_description: String,
    pub importance: f32,
    pub confidence: f32,
    pub source: Option<String>,
    pub reason: Option<String>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TryFrom<MemoryRow> for MemoryRecord {
    type Error = RepositoryError;

    fn try_from(value: MemoryRow) -> Result<Self, Self::Error> {
        Ok(Self {
            memory_id: value.memory_id,
            context_key: value.context_key,
            source_episode_id: value.source_episode_id,
            memory_type: decode_memory_type(&value.memory_type)?,
            title: value.title,
            content: value.content,
            short_description: value.short_description,
            importance: value.importance,
            confidence: value.confidence,
            source: value.source,
            reason: value.reason,
            tags: value.tags,
            created_at: value.created_at,
            updated_at: value.updated_at,
        })
    }
}

/// Row shape for lexical episode search results.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct EpisodeSearchRow {
    pub episode_id: String,
    pub thread_id: String,
    pub context_key: String,
    pub goal: String,
    pub summary: String,
    pub outcome: String,
    pub tools_used: Vec<String>,
    pub artifacts: Json<Vec<ArtifactRef>>,
    pub failures: Vec<String>,
    pub importance: f32,
    pub created_at: DateTime<Utc>,
    pub lexical_score: f32,
    pub lexical_snippet: String,
}

impl TryFrom<EpisodeSearchRow> for EpisodeSearchHit {
    type Error = RepositoryError;

    fn try_from(value: EpisodeSearchRow) -> Result<Self, Self::Error> {
        Ok(Self {
            record: EpisodeRecord {
                episode_id: value.episode_id,
                thread_id: value.thread_id,
                context_key: value.context_key,
                goal: value.goal,
                summary: value.summary,
                outcome: decode_episode_outcome(&value.outcome)?,
                tools_used: value.tools_used,
                artifacts: value.artifacts.0,
                failures: value.failures,
                importance: value.importance,
                created_at: value.created_at,
            },
            score: value.lexical_score,
            snippet: value.lexical_snippet,
        })
    }
}

/// Row shape for lexical memory search results.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct MemorySearchRow {
    pub memory_id: String,
    pub context_key: String,
    pub source_episode_id: Option<String>,
    pub memory_type: String,
    pub title: String,
    pub content: String,
    pub short_description: String,
    pub importance: f32,
    pub confidence: f32,
    pub source: Option<String>,
    pub reason: Option<String>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub lexical_score: f32,
    pub lexical_snippet: String,
}

impl TryFrom<MemorySearchRow> for MemorySearchHit {
    type Error = RepositoryError;

    fn try_from(value: MemorySearchRow) -> Result<Self, Self::Error> {
        Ok(Self {
            record: MemoryRecord {
                memory_id: value.memory_id,
                context_key: value.context_key,
                source_episode_id: value.source_episode_id,
                memory_type: decode_memory_type(&value.memory_type)?,
                title: value.title,
                content: value.content,
                short_description: value.short_description,
                importance: value.importance,
                confidence: value.confidence,
                source: value.source,
                reason: value.reason,
                tags: value.tags,
                created_at: value.created_at,
                updated_at: value.updated_at,
            },
            score: value.lexical_score,
            snippet: value.lexical_snippet,
        })
    }
}

/// Row shape for `memory_session_state`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct SessionStateRow {
    pub session_id: String,
    pub context_key: String,
    pub hot_token_estimate: i64,
    pub last_compacted_at: Option<DateTime<Utc>>,
    pub last_finalized_at: Option<DateTime<Utc>>,
    pub cleanup_status: String,
    pub pending_episode_id: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl TryFrom<SessionStateRow> for SessionStateRecord {
    type Error = RepositoryError;

    fn try_from(value: SessionStateRow) -> Result<Self, Self::Error> {
        let hot_token_estimate = usize::try_from(value.hot_token_estimate).map_err(|_| {
            RepositoryError::Storage(format!(
                "session {} has negative hot_token_estimate {}",
                value.session_id, value.hot_token_estimate
            ))
        })?;

        Ok(Self {
            session_id: value.session_id,
            context_key: value.context_key,
            hot_token_estimate,
            last_compacted_at: value.last_compacted_at,
            last_finalized_at: value.last_finalized_at,
            cleanup_status: decode_cleanup_status(&value.cleanup_status)?,
            pending_episode_id: value.pending_episode_id,
            updated_at: value.updated_at,
        })
    }
}

/// Encodes `EpisodeOutcome` into the database representation.
#[must_use]
pub(crate) fn encode_episode_outcome(value: EpisodeOutcome) -> &'static str {
    match value {
        EpisodeOutcome::Success => "success",
        EpisodeOutcome::Partial => "partial",
        EpisodeOutcome::Failure => "failure",
        EpisodeOutcome::Cancelled => "cancelled",
    }
}

/// Encodes `MemoryType` into the database representation.
#[must_use]
pub(crate) fn encode_memory_type(value: MemoryType) -> &'static str {
    match value {
        MemoryType::Fact => "fact",
        MemoryType::Preference => "preference",
        MemoryType::Procedure => "procedure",
        MemoryType::Decision => "decision",
        MemoryType::Constraint => "constraint",
    }
}

/// Encodes `CleanupStatus` into the database representation.
#[must_use]
pub(crate) fn encode_cleanup_status(value: CleanupStatus) -> &'static str {
    match value {
        CleanupStatus::Active => "active",
        CleanupStatus::Idle => "idle",
        CleanupStatus::Cleaning => "cleaning",
        CleanupStatus::Finalized => "finalized",
    }
}

fn decode_episode_outcome(value: &str) -> Result<EpisodeOutcome, RepositoryError> {
    match value {
        "success" => Ok(EpisodeOutcome::Success),
        "partial" => Ok(EpisodeOutcome::Partial),
        "failure" => Ok(EpisodeOutcome::Failure),
        "cancelled" => Ok(EpisodeOutcome::Cancelled),
        _ => Err(RepositoryError::Storage(format!(
            "unknown episode outcome {value}"
        ))),
    }
}

fn decode_memory_type(value: &str) -> Result<MemoryType, RepositoryError> {
    match value {
        "fact" => Ok(MemoryType::Fact),
        "preference" => Ok(MemoryType::Preference),
        "procedure" => Ok(MemoryType::Procedure),
        "decision" => Ok(MemoryType::Decision),
        "constraint" => Ok(MemoryType::Constraint),
        _ => Err(RepositoryError::Storage(format!(
            "unknown memory type {value}"
        ))),
    }
}

fn decode_cleanup_status(value: &str) -> Result<CleanupStatus, RepositoryError> {
    match value {
        "active" => Ok(CleanupStatus::Active),
        "idle" => Ok(CleanupStatus::Idle),
        "cleaning" => Ok(CleanupStatus::Cleaning),
        "finalized" => Ok(CleanupStatus::Finalized),
        _ => Err(RepositoryError::Storage(format!(
            "unknown cleanup status {value}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        decode_cleanup_status, decode_episode_outcome, decode_memory_type, encode_cleanup_status,
        encode_episode_outcome, encode_memory_type, EpisodeRow, EpisodeSearchRow, MemoryRow,
        MemorySearchRow, SessionStateRow,
    };
    use crate::types::{ArtifactRef, CleanupStatus, EpisodeOutcome, MemoryType};
    use chrono::{TimeZone, Utc};
    use sqlx::types::Json;

    fn ts(seconds: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0)
            .single()
            .expect("valid timestamp")
    }

    #[test]
    fn enum_codecs_roundtrip() {
        assert_eq!(
            decode_episode_outcome(encode_episode_outcome(EpisodeOutcome::Partial))
                .expect("episode outcome should roundtrip"),
            EpisodeOutcome::Partial
        );
        assert_eq!(
            decode_memory_type(encode_memory_type(MemoryType::Decision))
                .expect("memory type should roundtrip"),
            MemoryType::Decision
        );
        assert_eq!(
            decode_cleanup_status(encode_cleanup_status(CleanupStatus::Idle))
                .expect("cleanup status should roundtrip"),
            CleanupStatus::Idle
        );
    }

    #[test]
    fn episode_row_maps_back_to_domain_record() {
        let row = EpisodeRow {
            episode_id: "episode-1".to_string(),
            thread_id: "thread-1".to_string(),
            context_key: "topic-a".to_string(),
            goal: "goal".to_string(),
            summary: "summary".to_string(),
            outcome: "success".to_string(),
            tools_used: vec!["compress".to_string()],
            artifacts: Json(vec![ArtifactRef {
                storage_key: "archive/topic-a/episode-1.json".to_string(),
                description: "episode archive".to_string(),
                content_type: Some("application/json".to_string()),
                source: Some("test".to_string()),
                reason: None,
                tags: vec!["archive".to_string()],
                created_at: ts(1),
            }]),
            failures: vec!["warning".to_string()],
            importance: 0.8,
            created_at: ts(2),
        };

        let record = crate::types::EpisodeRecord::try_from(row).expect("row should map");
        assert_eq!(record.outcome, EpisodeOutcome::Success);
        assert_eq!(record.artifacts.len(), 1);
    }

    #[test]
    fn session_state_row_rejects_negative_tokens() {
        let row = SessionStateRow {
            session_id: "session-1".to_string(),
            context_key: "topic-a".to_string(),
            hot_token_estimate: -1,
            last_compacted_at: None,
            last_finalized_at: None,
            cleanup_status: "active".to_string(),
            pending_episode_id: None,
            updated_at: ts(3),
        };

        let error = crate::types::SessionStateRecord::try_from(row)
            .expect_err("negative tokens should fail");
        assert!(error.to_string().contains("negative hot_token_estimate"));
    }

    #[test]
    fn memory_row_maps_back_to_domain_record() {
        let row = MemoryRow {
            memory_id: "memory-1".to_string(),
            context_key: "topic-a".to_string(),
            source_episode_id: Some("episode-1".to_string()),
            memory_type: "constraint".to_string(),
            title: "Constraint".to_string(),
            content: "Sub-agent runs must not persist durable memory".to_string(),
            short_description: "constraint".to_string(),
            importance: 0.9,
            confidence: 0.95,
            source: Some("tool".to_string()),
            reason: Some("user requested durable save".to_string()),
            tags: vec!["episode".to_string()],
            created_at: ts(4),
            updated_at: ts(5),
        };

        let record = crate::types::MemoryRecord::try_from(row).expect("row should map");
        assert_eq!(record.memory_type, MemoryType::Constraint);
    }

    #[test]
    fn lexical_episode_row_maps_back_to_search_hit() {
        let row = EpisodeSearchRow {
            episode_id: "episode-2".to_string(),
            thread_id: "thread-1".to_string(),
            context_key: "topic-a".to_string(),
            goal: "Fix lexical search".to_string(),
            summary: "R2_REGION should remain searchable".to_string(),
            outcome: "partial".to_string(),
            tools_used: vec!["grep".to_string()],
            artifacts: Json(Vec::new()),
            failures: vec!["fts ranking".to_string()],
            importance: 0.7,
            created_at: ts(6),
            lexical_score: 0.42,
            lexical_snippet: "R2_REGION should remain searchable".to_string(),
        };

        let hit = crate::types::EpisodeSearchHit::try_from(row).expect("row should map");
        assert_eq!(hit.record.outcome, EpisodeOutcome::Partial);
        assert_eq!(hit.score, 0.42);
    }

    #[test]
    fn lexical_memory_row_maps_back_to_search_hit() {
        let row = MemorySearchRow {
            memory_id: "memory-2".to_string(),
            context_key: "topic-a".to_string(),
            source_episode_id: Some("episode-2".to_string()),
            memory_type: "fact".to_string(),
            title: "R2_REGION exact lookup".to_string(),
            content: "Use lexical search for env vars".to_string(),
            short_description: "env var retrieval".to_string(),
            importance: 0.8,
            confidence: 0.9,
            source: Some("tool".to_string()),
            reason: Some("promote stable retrieval hint".to_string()),
            tags: vec!["search".to_string()],
            created_at: ts(7),
            updated_at: ts(8),
            lexical_score: 0.5,
            lexical_snippet: "R2_REGION exact lookup".to_string(),
        };

        let hit = crate::types::MemorySearchHit::try_from(row).expect("row should map");
        assert_eq!(hit.record.memory_type, MemoryType::Fact);
        assert_eq!(hit.snippet, "R2_REGION exact lookup");
    }
}
