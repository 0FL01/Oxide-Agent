use chrono::Utc;
use oxide_agent_memory::{
    stable_memory_content_hash, ArchiveBlobStore, ArtifactRef, CleanupStatus, EpisodeListFilter,
    EpisodeOutcome, EpisodeRecord, InMemoryArchiveBlobStore, InMemoryMemoryRepository,
    MemoryListFilter, MemoryRecord, MemoryRepository, MemoryType, SessionStateRecord, ThreadRecord,
};

fn record_times() -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
    let created_at = Utc::now();
    let updated_at = created_at;
    (created_at, updated_at)
}

#[tokio::test]
async fn in_memory_memory_crate_supports_core_contracts() {
    let repo = InMemoryMemoryRepository::new();
    let archive = InMemoryArchiveBlobStore::new();
    let (created_at, updated_at) = record_times();

    let thread = repo
        .upsert_thread(ThreadRecord {
            thread_id: "thread-1".to_string(),
            user_id: 7,
            context_key: "topic-a".to_string(),
            title: "Topic A".to_string(),
            short_summary: "summary".to_string(),
            created_at,
            updated_at,
            last_activity_at: updated_at,
        })
        .await
        .expect("thread stored");

    let episode = repo
        .create_episode(EpisodeRecord {
            episode_id: "episode-1".to_string(),
            thread_id: thread.thread_id.clone(),
            context_key: thread.context_key.clone(),
            goal: "demo goal".to_string(),
            summary: "demo summary".to_string(),
            outcome: EpisodeOutcome::Success,
            tools_used: vec!["compress".to_string()],
            artifacts: vec![ArtifactRef {
                storage_key: "archive/topic-a/episode-1.json".to_string(),
                description: "episode archive".to_string(),
                content_type: Some("application/json".to_string()),
                source: Some("test".to_string()),
                reason: None,
                tags: vec!["archive".to_string()],
                created_at,
            }],
            failures: vec![],
            importance: 0.9,
            created_at,
        })
        .await
        .expect("episode stored");

    repo.create_memory(MemoryRecord {
        memory_id: "memory-1".to_string(),
        context_key: thread.context_key.clone(),
        source_episode_id: Some(episode.episode_id.clone()),
        memory_type: MemoryType::Decision,
        title: "decision".to_string(),
        content: "use hybrid memory".to_string(),
        short_description: "decision summary".to_string(),
        importance: 0.8,
        confidence: 0.9,
        source: Some("test".to_string()),
        content_hash: Some(stable_memory_content_hash(
            MemoryType::Decision,
            "use hybrid memory",
        )),
        reason: Some("contract fixture".to_string()),
        tags: vec!["memory".to_string(), "decision".to_string()],
        created_at,
        updated_at,
        deleted_at: None,
    })
    .await
    .expect("memory stored");

    let _state = repo
        .upsert_session_state(SessionStateRecord {
            session_id: "session-1".to_string(),
            context_key: thread.context_key.clone(),
            hot_token_estimate: 128,
            last_compacted_at: None,
            last_finalized_at: Some(updated_at),
            cleanup_status: CleanupStatus::Active,
            pending_episode_id: Some(episode.episode_id.clone()),
            updated_at,
        })
        .await
        .expect("session state stored");

    let episodes = repo
        .list_episodes_for_thread(&thread.thread_id, &EpisodeListFilter::default())
        .await
        .expect("episodes listed");
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].episode_id, episode.episode_id);

    let memories = repo
        .list_memories(&thread.context_key, &MemoryListFilter::default())
        .await
        .expect("memories listed");
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].memory_id, "memory-1");

    let artifact = archive
        .put(
            "archive/topic-a/episode-1.json",
            br#"{"status":"ok"}"#,
            Some("application/json"),
        )
        .await
        .expect("artifact stored");
    assert!(archive
        .exists(&artifact.storage_key)
        .await
        .expect("exists check"));
    assert_eq!(
        archive
            .get(&artifact.storage_key)
            .await
            .expect("artifact fetch")
            .expect("artifact should exist"),
        br#"{"status":"ok"}"#
    );
}
