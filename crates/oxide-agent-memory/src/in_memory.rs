//! In-memory implementations for the typed memory subsystem.

use crate::archive::ArchiveBlobStore;
use crate::repository::{MemoryRepository, RepositoryError};
use crate::types::{
    ArtifactRef, EpisodeId, EpisodeListFilter, EpisodeRecord, EpisodeSearchFilter,
    EpisodeSearchHit, MemoryId, MemoryListFilter, MemoryRecord, MemorySearchFilter,
    MemorySearchHit, SessionStateId, SessionStateRecord, ThreadId, ThreadRecord,
};
use chrono::Utc;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

const EPISODE_SNIPPET_LEN: usize = 160;
const MEMORY_SNIPPET_LEN: usize = 160;

#[derive(Debug, Default)]
struct RepositoryState {
    threads: HashMap<ThreadId, ThreadRecord>,
    episodes: HashMap<EpisodeId, EpisodeRecord>,
    memories: HashMap<MemoryId, MemoryRecord>,
    session_states: HashMap<SessionStateId, SessionStateRecord>,
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryMemoryRepository {
    state: Arc<RwLock<RepositoryState>>,
}

impl InMemoryMemoryRepository {
    /// Create a new empty in-memory repository.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn map_storage_error() -> RepositoryError {
        RepositoryError::Storage("in-memory repository lock poisoned".to_string())
    }
}

fn lexical_terms(query: &str) -> Vec<String> {
    query
        .split(|character: char| {
            !(character.is_alphanumeric() || character == '_' || character == '-')
        })
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn field_matches(field: &str, term: &str) -> bool {
    field.to_ascii_lowercase().contains(term)
}

fn lexical_score(fields: &[(&str, f32)], terms: &[String]) -> f32 {
    terms
        .iter()
        .map(|term| {
            fields
                .iter()
                .filter_map(|(field, weight)| field_matches(field, term).then_some(*weight))
                .sum::<f32>()
        })
        .sum()
}

fn snippet_for(fields: &[&str], terms: &[String], max_chars: usize) -> String {
    let source = fields
        .iter()
        .copied()
        .find(|field| !field.is_empty() && terms.iter().any(|term| field_matches(field, term)))
        .or_else(|| fields.iter().copied().find(|field| !field.is_empty()))
        .unwrap_or_default();

    truncate_snippet(source, max_chars)
}

fn truncate_snippet(value: &str, max_chars: usize) -> String {
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        truncated.push('…');
    }
    truncated
}

impl MemoryRepository for InMemoryMemoryRepository {
    fn upsert_thread(
        &self,
        record: ThreadRecord,
    ) -> impl Future<Output = Result<ThreadRecord, RepositoryError>> + Send {
        let state = Arc::clone(&self.state);
        async move {
            let mut guard = state.write().map_err(|_| Self::map_storage_error())?;
            let stored = if let Some(existing) = guard.threads.get(&record.thread_id) {
                ThreadRecord {
                    created_at: existing.created_at,
                    ..record
                }
            } else {
                record
            };

            guard
                .threads
                .insert(stored.thread_id.clone(), stored.clone());
            Ok(stored)
        }
    }

    fn get_thread(
        &self,
        thread_id: &ThreadId,
    ) -> impl Future<Output = Result<Option<ThreadRecord>, RepositoryError>> + Send {
        let state = Arc::clone(&self.state);
        let thread_id = thread_id.clone();
        async move {
            let guard = state.read().map_err(|_| Self::map_storage_error())?;
            Ok(guard.threads.get(&thread_id).cloned())
        }
    }

    fn create_episode(
        &self,
        record: EpisodeRecord,
    ) -> impl Future<Output = Result<EpisodeRecord, RepositoryError>> + Send {
        let state = Arc::clone(&self.state);
        async move {
            let mut guard = state.write().map_err(|_| Self::map_storage_error())?;
            if guard.episodes.contains_key(&record.episode_id) {
                return Err(RepositoryError::Conflict(format!(
                    "episode {} already exists",
                    record.episode_id
                )));
            }

            guard
                .episodes
                .insert(record.episode_id.clone(), record.clone());
            Ok(record)
        }
    }

    fn get_episode(
        &self,
        episode_id: &EpisodeId,
    ) -> impl Future<Output = Result<Option<EpisodeRecord>, RepositoryError>> + Send {
        let state = Arc::clone(&self.state);
        let episode_id = episode_id.clone();
        async move {
            let guard = state.read().map_err(|_| Self::map_storage_error())?;
            Ok(guard.episodes.get(&episode_id).cloned())
        }
    }

    fn list_episodes_for_thread(
        &self,
        thread_id: &ThreadId,
        filter: &EpisodeListFilter,
    ) -> impl Future<Output = Result<Vec<EpisodeRecord>, RepositoryError>> + Send {
        let state = Arc::clone(&self.state);
        let thread_id = thread_id.clone();
        let filter = filter.clone();
        async move {
            let guard = state.read().map_err(|_| Self::map_storage_error())?;
            let mut episodes: Vec<_> = guard
                .episodes
                .values()
                .filter(|episode| episode.thread_id == thread_id)
                .filter(|episode| match filter.min_importance {
                    Some(min_importance) => episode.importance >= min_importance,
                    None => true,
                })
                .filter(|episode| match filter.outcome {
                    Some(outcome) => episode.outcome == outcome,
                    None => true,
                })
                .cloned()
                .collect();

            episodes.sort_by(|left, right| {
                right
                    .created_at
                    .cmp(&left.created_at)
                    .then_with(|| left.episode_id.cmp(&right.episode_id))
            });

            if let Some(limit) = filter.limit {
                episodes.truncate(limit);
            }

            Ok(episodes)
        }
    }

    fn create_memory(
        &self,
        record: MemoryRecord,
    ) -> impl Future<Output = Result<MemoryRecord, RepositoryError>> + Send {
        let state = Arc::clone(&self.state);
        async move {
            let mut guard = state.write().map_err(|_| Self::map_storage_error())?;
            if guard.memories.contains_key(&record.memory_id) {
                return Err(RepositoryError::Conflict(format!(
                    "memory {} already exists",
                    record.memory_id
                )));
            }

            guard
                .memories
                .insert(record.memory_id.clone(), record.clone());
            Ok(record)
        }
    }

    fn get_memory(
        &self,
        memory_id: &str,
    ) -> impl Future<Output = Result<Option<MemoryRecord>, RepositoryError>> + Send {
        let state = Arc::clone(&self.state);
        let memory_id = memory_id.to_string();
        async move {
            let guard = state.read().map_err(|_| Self::map_storage_error())?;
            Ok(guard.memories.get(&memory_id).cloned())
        }
    }

    fn list_memories(
        &self,
        context_key: &str,
        filter: &MemoryListFilter,
    ) -> impl Future<Output = Result<Vec<MemoryRecord>, RepositoryError>> + Send {
        let state = Arc::clone(&self.state);
        let context_key = context_key.to_string();
        let filter = filter.clone();
        async move {
            let guard = state.read().map_err(|_| Self::map_storage_error())?;
            let mut memories: Vec<_> = guard
                .memories
                .values()
                .filter(|memory| memory.context_key == context_key)
                .filter(|memory| match filter.memory_type {
                    Some(memory_type) => memory.memory_type == memory_type,
                    None => true,
                })
                .filter(|memory| match filter.min_importance {
                    Some(min_importance) => memory.importance >= min_importance,
                    None => true,
                })
                .filter(|memory| filter.tags.iter().all(|tag| memory.tags.contains(tag)))
                .cloned()
                .collect();

            memories.sort_by(|left, right| {
                right
                    .updated_at
                    .cmp(&left.updated_at)
                    .then_with(|| left.memory_id.cmp(&right.memory_id))
            });

            if let Some(limit) = filter.limit {
                memories.truncate(limit);
            }

            Ok(memories)
        }
    }

    fn search_episodes_lexical(
        &self,
        query: &str,
        filter: &EpisodeSearchFilter,
    ) -> impl Future<Output = Result<Vec<EpisodeSearchHit>, RepositoryError>> + Send {
        let state = Arc::clone(&self.state);
        let query = query.to_string();
        let filter = filter.clone();
        async move {
            let terms = lexical_terms(&query);
            if terms.is_empty() {
                return Ok(Vec::new());
            }

            let guard = state.read().map_err(|_| Self::map_storage_error())?;
            let mut hits: Vec<_> = guard
                .episodes
                .values()
                .filter(|episode| match &filter.context_key {
                    Some(context_key) => &episode.context_key == context_key,
                    None => true,
                })
                .filter(|episode| match filter.user_id {
                    Some(user_id) => guard
                        .threads
                        .get(&episode.thread_id)
                        .is_some_and(|thread| thread.user_id == user_id),
                    None => true,
                })
                .filter(|episode| match filter.outcome {
                    Some(outcome) => episode.outcome == outcome,
                    None => true,
                })
                .filter(|episode| match filter.min_importance {
                    Some(min_importance) => episode.importance >= min_importance,
                    None => true,
                })
                .filter(|episode| match filter.time_range.since {
                    Some(since) => episode.created_at >= since,
                    None => true,
                })
                .filter(|episode| match filter.time_range.until {
                    Some(until) => episode.created_at <= until,
                    None => true,
                })
                .filter_map(|episode| {
                    let tools = episode.tools_used.join(" ");
                    let failures = episode.failures.join(" ");
                    let score = lexical_score(
                        &[
                            (&episode.goal, 3.0),
                            (&episode.summary, 2.0),
                            (&tools, 1.5),
                            (&failures, 1.5),
                        ],
                        &terms,
                    );
                    (score > 0.0).then(|| EpisodeSearchHit {
                        record: episode.clone(),
                        score,
                        snippet: snippet_for(
                            &[&episode.goal, &episode.summary],
                            &terms,
                            EPISODE_SNIPPET_LEN,
                        ),
                    })
                })
                .collect();

            hits.sort_by(|left, right| {
                right
                    .score
                    .total_cmp(&left.score)
                    .then_with(|| right.record.importance.total_cmp(&left.record.importance))
                    .then_with(|| right.record.created_at.cmp(&left.record.created_at))
                    .then_with(|| left.record.episode_id.cmp(&right.record.episode_id))
            });

            if let Some(limit) = filter.limit {
                hits.truncate(limit);
            }

            Ok(hits)
        }
    }

    fn search_memories_lexical(
        &self,
        query: &str,
        filter: &MemorySearchFilter,
    ) -> impl Future<Output = Result<Vec<MemorySearchHit>, RepositoryError>> + Send {
        let state = Arc::clone(&self.state);
        let query = query.to_string();
        let filter = filter.clone();
        async move {
            let terms = lexical_terms(&query);
            if terms.is_empty() {
                return Ok(Vec::new());
            }

            let guard = state.read().map_err(|_| Self::map_storage_error())?;
            let mut hits: Vec<_> = guard
                .memories
                .values()
                .filter(|memory| match &filter.context_key {
                    Some(context_key) => &memory.context_key == context_key,
                    None => true,
                })
                .filter(|memory| match filter.user_id {
                    Some(user_id) => memory
                        .source_episode_id
                        .as_ref()
                        .and_then(|episode_id| guard.episodes.get(episode_id))
                        .and_then(|episode| guard.threads.get(&episode.thread_id))
                        .is_some_and(|thread| thread.user_id == user_id),
                    None => true,
                })
                .filter(|memory| match filter.memory_type {
                    Some(memory_type) => memory.memory_type == memory_type,
                    None => true,
                })
                .filter(|memory| match filter.min_importance {
                    Some(min_importance) => memory.importance >= min_importance,
                    None => true,
                })
                .filter(|memory| filter.tags.iter().all(|tag| memory.tags.contains(tag)))
                .filter(|memory| match filter.time_range.since {
                    Some(since) => memory.updated_at >= since,
                    None => true,
                })
                .filter(|memory| match filter.time_range.until {
                    Some(until) => memory.updated_at <= until,
                    None => true,
                })
                .filter_map(|memory| {
                    let tags = memory.tags.join(" ");
                    let score = lexical_score(
                        &[
                            (&memory.title, 3.0),
                            (&memory.short_description, 2.0),
                            (&memory.content, 2.0),
                            (&tags, 1.0),
                        ],
                        &terms,
                    );
                    (score > 0.0).then(|| MemorySearchHit {
                        record: memory.clone(),
                        score,
                        snippet: snippet_for(
                            &[&memory.title, &memory.short_description, &memory.content],
                            &terms,
                            MEMORY_SNIPPET_LEN,
                        ),
                    })
                })
                .collect();

            hits.sort_by(|left, right| {
                right
                    .score
                    .total_cmp(&left.score)
                    .then_with(|| right.record.importance.total_cmp(&left.record.importance))
                    .then_with(|| right.record.confidence.total_cmp(&left.record.confidence))
                    .then_with(|| right.record.updated_at.cmp(&left.record.updated_at))
                    .then_with(|| left.record.memory_id.cmp(&right.record.memory_id))
            });

            if let Some(limit) = filter.limit {
                hits.truncate(limit);
            }

            Ok(hits)
        }
    }

    fn upsert_session_state(
        &self,
        record: SessionStateRecord,
    ) -> impl Future<Output = Result<SessionStateRecord, RepositoryError>> + Send {
        let state = Arc::clone(&self.state);
        async move {
            let mut guard = state.write().map_err(|_| Self::map_storage_error())?;
            guard
                .session_states
                .insert(record.session_id.clone(), record.clone());
            Ok(record)
        }
    }

    fn get_session_state(
        &self,
        session_id: &str,
    ) -> impl Future<Output = Result<Option<SessionStateRecord>, RepositoryError>> + Send {
        let state = Arc::clone(&self.state);
        let session_id = session_id.to_string();
        async move {
            let guard = state.read().map_err(|_| Self::map_storage_error())?;
            Ok(guard.session_states.get(&session_id).cloned())
        }
    }
}

#[derive(Debug, Clone, Default)]
struct BlobEntry {
    data: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryArchiveBlobStore {
    blobs: Arc<RwLock<HashMap<String, BlobEntry>>>,
}

impl InMemoryArchiveBlobStore {
    /// Create a new empty archive blob store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn map_storage_error() -> anyhow::Error {
        anyhow::anyhow!("in-memory blob store lock poisoned")
    }
}

impl ArchiveBlobStore for InMemoryArchiveBlobStore {
    fn put(
        &self,
        key: &str,
        data: &[u8],
        content_type: Option<&str>,
    ) -> impl Future<Output = anyhow::Result<ArtifactRef>> + Send {
        let blobs = Arc::clone(&self.blobs);
        let key = key.to_string();
        let content_type = content_type.map(|value| value.to_string());
        let data = data.to_vec();
        async move {
            let artifact = ArtifactRef {
                storage_key: key.clone(),
                description: format!("Archived blob at {key}"),
                content_type: content_type.clone(),
                source: Some("archive_blob_store".to_string()),
                reason: None,
                tags: vec!["archive".to_string()],
                created_at: Utc::now(),
            };

            let entry = BlobEntry { data };

            let mut guard = blobs.write().map_err(|_| Self::map_storage_error())?;
            guard.insert(key, entry);
            Ok(artifact)
        }
    }

    fn get(&self, key: &str) -> impl Future<Output = anyhow::Result<Option<Vec<u8>>>> + Send {
        let blobs = Arc::clone(&self.blobs);
        let key = key.to_string();
        async move {
            let guard = blobs.read().map_err(|_| Self::map_storage_error())?;
            Ok(guard.get(&key).map(|entry| entry.data.clone()))
        }
    }

    fn delete(&self, key: &str) -> impl Future<Output = anyhow::Result<()>> + Send {
        let blobs = Arc::clone(&self.blobs);
        let key = key.to_string();
        async move {
            let mut guard = blobs.write().map_err(|_| Self::map_storage_error())?;
            guard.remove(&key);
            Ok(())
        }
    }

    fn exists(&self, key: &str) -> impl Future<Output = anyhow::Result<bool>> + Send {
        let blobs = Arc::clone(&self.blobs);
        let key = key.to_string();
        async move {
            let guard = blobs.read().map_err(|_| Self::map_storage_error())?;
            Ok(guard.contains_key(&key))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        ArtifactRef, CleanupStatus, EpisodeOutcome, EpisodeSearchFilter, MemorySearchFilter,
        MemoryType, TimeRange,
    };
    use chrono::TimeZone;

    fn utc(ts: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(ts, 0).single().expect("valid timestamp")
    }

    fn thread_record(thread_id: &str) -> ThreadRecord {
        ThreadRecord {
            thread_id: thread_id.to_string(),
            user_id: 1,
            context_key: "topic-a".to_string(),
            title: format!("Thread {thread_id}"),
            short_summary: "summary".to_string(),
            created_at: utc(1_000),
            updated_at: utc(1_100),
            last_activity_at: utc(1_100),
        }
    }

    fn episode_record(
        episode_id: &str,
        thread_id: &str,
        ts: i64,
        outcome: EpisodeOutcome,
    ) -> EpisodeRecord {
        EpisodeRecord {
            episode_id: episode_id.to_string(),
            thread_id: thread_id.to_string(),
            context_key: "topic-a".to_string(),
            goal: "goal".to_string(),
            summary: format!("episode {episode_id}"),
            outcome,
            tools_used: vec!["compress".to_string()],
            artifacts: vec![ArtifactRef {
                storage_key: format!("r2://artifact/{episode_id}"),
                description: "artifact".to_string(),
                content_type: Some("text/plain".to_string()),
                source: Some("test".to_string()),
                reason: None,
                tags: vec!["fixture".to_string()],
                created_at: utc(ts),
            }],
            failures: vec![],
            importance: 0.8,
            created_at: utc(ts),
        }
    }

    fn memory_record(
        memory_id: &str,
        kind: MemoryType,
        tags: Vec<&str>,
        updated_at: i64,
    ) -> MemoryRecord {
        MemoryRecord {
            memory_id: memory_id.to_string(),
            context_key: "topic-a".to_string(),
            source_episode_id: Some("ep-1".to_string()),
            memory_type: kind,
            title: format!("Memory {memory_id}"),
            content: format!("content {memory_id}"),
            short_description: "desc".to_string(),
            importance: 0.9,
            confidence: 0.7,
            source: Some("test".to_string()),
            reason: None,
            tags: tags.into_iter().map(ToOwned::to_owned).collect(),
            created_at: utc(updated_at - 10),
            updated_at: utc(updated_at),
        }
    }

    #[tokio::test]
    async fn upsert_thread_preserves_created_at() {
        let repo = InMemoryMemoryRepository::new();
        let mut first = thread_record("th-1");
        first.created_at = utc(10);
        first.updated_at = utc(20);
        first.last_activity_at = utc(20);

        let stored_first = repo
            .upsert_thread(first.clone())
            .await
            .expect("thread upsert should succeed");
        assert_eq!(stored_first, first);

        let mut second = first;
        second.title = "Updated".to_string();
        second.updated_at = utc(30);
        second.last_activity_at = utc(40);

        let stored_second = repo
            .upsert_thread(second)
            .await
            .expect("thread update should succeed");
        assert_eq!(stored_second.created_at, utc(10));
        assert_eq!(stored_second.title, "Updated");
        assert_eq!(
            repo.get_thread(&"th-1".to_string())
                .await
                .expect("thread lookup should succeed")
                .expect("thread should exist")
                .created_at,
            utc(10)
        );
    }

    #[tokio::test]
    async fn episode_roundtrip_and_listing_filters() {
        let repo = InMemoryMemoryRepository::new();
        repo.create_episode(episode_record("ep-1", "th-1", 10, EpisodeOutcome::Success))
            .await
            .expect("episode 1 should store");
        repo.create_episode(episode_record("ep-2", "th-1", 20, EpisodeOutcome::Failure))
            .await
            .expect("episode 2 should store");
        repo.create_episode(episode_record("ep-3", "th-2", 30, EpisodeOutcome::Success))
            .await
            .expect("episode 3 should store");

        let filter = EpisodeListFilter {
            min_importance: Some(0.5),
            outcome: Some(EpisodeOutcome::Success),
            limit: Some(10),
        };

        let episodes = repo
            .list_episodes_for_thread(&"th-1".to_string(), &filter)
            .await
            .expect("episodes should list");
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].episode_id, "ep-1");

        assert_eq!(
            repo.get_episode(&"ep-2".to_string())
                .await
                .expect("episode lookup should succeed")
                .expect("episode should exist")
                .outcome,
            EpisodeOutcome::Failure
        );
    }

    #[tokio::test]
    async fn memory_roundtrip_and_tag_filters() {
        let repo = InMemoryMemoryRepository::new();
        repo.create_memory(memory_record(
            "mem-1",
            MemoryType::Fact,
            vec!["topic", "rust"],
            100,
        ))
        .await
        .expect("memory 1 should store");
        repo.create_memory(memory_record(
            "mem-2",
            MemoryType::Procedure,
            vec!["topic"],
            200,
        ))
        .await
        .expect("memory 2 should store");

        let filter = MemoryListFilter {
            memory_type: Some(MemoryType::Fact),
            min_importance: None,
            tags: vec!["topic".to_string(), "rust".to_string()],
            limit: Some(10),
        };

        let memories = repo
            .list_memories("topic-a", &filter)
            .await
            .expect("memories should list");
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].memory_id, "mem-1");
        assert_eq!(
            repo.get_memory("mem-1")
                .await
                .expect("memory lookup should succeed")
                .expect("memory should exist")
                .title,
            "Memory mem-1"
        );
    }

    #[tokio::test]
    async fn session_state_roundtrip() {
        let repo = InMemoryMemoryRepository::new();
        let state = SessionStateRecord {
            session_id: "sess-1".to_string(),
            context_key: "topic-a".to_string(),
            hot_token_estimate: 123,
            last_compacted_at: Some(utc(1000)),
            last_finalized_at: None,
            cleanup_status: CleanupStatus::Idle,
            pending_episode_id: Some("ep-1".to_string()),
            updated_at: utc(1100),
        };

        let stored = repo
            .upsert_session_state(state.clone())
            .await
            .expect("session state should store");
        assert_eq!(stored, state);

        let loaded = repo
            .get_session_state("sess-1")
            .await
            .expect("session state lookup should succeed")
            .expect("session state should exist");
        assert_eq!(loaded.context_key, "topic-a");
        assert_eq!(loaded.hot_token_estimate, 123);
    }

    #[tokio::test]
    async fn lexical_episode_search_respects_scope_and_ordering() {
        let repo = InMemoryMemoryRepository::new();
        repo.upsert_thread(thread_record("th-1"))
            .await
            .expect("thread 1 should store");

        let mut thread_two = thread_record("th-2");
        thread_two.user_id = 2;
        thread_two.context_key = "topic-b".to_string();
        repo.upsert_thread(thread_two)
            .await
            .expect("thread 2 should store");

        let mut matching = episode_record("ep-1", "th-1", 100, EpisodeOutcome::Success);
        matching.goal = "Fix R2_REGION lexical search bug".to_string();
        matching.summary = "Confirmed lexical search should keep R2_REGION exact".to_string();
        repo.create_episode(matching)
            .await
            .expect("matching episode should store");

        let mut weaker = episode_record("ep-2", "th-1", 200, EpisodeOutcome::Success);
        weaker.summary = "Investigated lexical search fallback".to_string();
        repo.create_episode(weaker)
            .await
            .expect("weaker episode should store");

        let mut wrong_scope = episode_record("ep-3", "th-2", 300, EpisodeOutcome::Success);
        wrong_scope.context_key = "topic-b".to_string();
        wrong_scope.goal = "Fix lexical search in another topic".to_string();
        repo.create_episode(wrong_scope)
            .await
            .expect("other scope episode should store");

        let hits = repo
            .search_episodes_lexical(
                "R2_REGION lexical",
                &EpisodeSearchFilter {
                    context_key: Some("topic-a".to_string()),
                    user_id: Some(1),
                    outcome: Some(EpisodeOutcome::Success),
                    min_importance: Some(0.5),
                    time_range: TimeRange::default(),
                    limit: Some(10),
                },
            )
            .await
            .expect("episode search should succeed");

        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].record.episode_id, "ep-1");
        assert!(hits[0].score > hits[1].score);
        assert!(hits[0].snippet.contains("R2_REGION"));
    }

    #[tokio::test]
    async fn lexical_memory_search_respects_filters_and_limit() {
        let repo = InMemoryMemoryRepository::new();
        repo.upsert_thread(thread_record("th-1"))
            .await
            .expect("thread should store");
        repo.create_episode(episode_record("ep-1", "th-1", 100, EpisodeOutcome::Success))
            .await
            .expect("episode should store");

        let mut fact = memory_record("mem-1", MemoryType::Fact, vec!["topic", "search"], 120);
        fact.title = "R2_REGION exact lookup".to_string();
        fact.short_description = "Keep exact env var matching in lexical search".to_string();
        repo.create_memory(fact)
            .await
            .expect("fact memory should store");

        let mut procedure = memory_record("mem-2", MemoryType::Procedure, vec!["topic"], 140);
        procedure.title = "Lexical search rollout".to_string();
        procedure.content = "Run cargo check before enabling lexical memory tools".to_string();
        repo.create_memory(procedure)
            .await
            .expect("procedure memory should store");

        let mut filtered_out = memory_record("mem-3", MemoryType::Fact, vec!["topic"], 160);
        filtered_out.context_key = "topic-b".to_string();
        filtered_out.title = "Lexical search in another topic".to_string();
        repo.create_memory(filtered_out)
            .await
            .expect("other topic memory should store");

        let hits = repo
            .search_memories_lexical(
                "R2_REGION lexical search",
                &MemorySearchFilter {
                    context_key: Some("topic-a".to_string()),
                    user_id: Some(1),
                    memory_type: Some(MemoryType::Fact),
                    min_importance: Some(0.5),
                    tags: vec!["topic".to_string()],
                    time_range: TimeRange::default(),
                    limit: Some(1),
                },
            )
            .await
            .expect("memory search should succeed");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.memory_id, "mem-1");
        assert!(hits[0].snippet.contains("R2_REGION"));
    }

    #[tokio::test]
    async fn archive_blob_store_roundtrip() {
        let store = InMemoryArchiveBlobStore::new();
        let artifact = store
            .put(
                "archive/topic-a/history-1.json",
                br#"{"hello":"world"}"#,
                Some("application/json"),
            )
            .await
            .expect("artifact should store");

        assert_eq!(artifact.storage_key, "archive/topic-a/history-1.json");
        assert!(store
            .exists(&artifact.storage_key)
            .await
            .expect("exists check should succeed"));
        assert_eq!(
            store
                .get(&artifact.storage_key)
                .await
                .expect("artifact fetch should succeed")
                .expect("artifact should exist"),
            br#"{"hello":"world"}"#
        );

        store
            .delete(&artifact.storage_key)
            .await
            .expect("delete should succeed");
        assert!(!store
            .exists(&artifact.storage_key)
            .await
            .expect("exists after delete should succeed"));
    }
}
