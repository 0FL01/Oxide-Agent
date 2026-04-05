//! In-memory implementations for the typed memory subsystem.

use crate::archive::ArchiveBlobStore;
use crate::repository::{MemoryRepository, RepositoryError};
use crate::types::{
    ArtifactRef, EpisodeId, EpisodeListFilter, EpisodeRecord, MemoryId, MemoryListFilter,
    MemoryRecord, SessionStateId, SessionStateRecord, ThreadId, ThreadRecord,
};
use chrono::Utc;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

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
    use crate::types::{ArtifactRef, CleanupStatus, EpisodeOutcome, MemoryType};
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
