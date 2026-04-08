use super::*;
use crate::agent::compaction::ArchiveRef;
use crate::storage::MockStorageProvider;
use chrono::TimeZone;
use oxide_agent_memory::{
    CleanupStatus, EmbeddingPendingUpdate, EmbeddingReadyUpdate, EpisodeEmbeddingCandidate,
    EpisodeOutcome, InMemoryMemoryRepository,
};

struct FakeEmbeddingGenerator;

struct FakePostRunMemoryWriter;

#[async_trait::async_trait]
impl PostRunMemoryWriter for FakePostRunMemoryWriter {
    async fn write(
        &self,
        input: &PostRunMemoryWriterInput<'_>,
    ) -> anyhow::Result<ValidatedPostRunMemoryWrite> {
        let context_label = input.scope.context_key.replace('-', " ");
        let now = chrono::Utc::now();
        let memory_specs = if input.task.contains("hygiene") {
            vec![
                (
                    MemoryType::Decision,
                    "Memory hygiene decision".to_string(),
                    "Use cargo check before build".to_string(),
                ),
                (
                    MemoryType::Constraint,
                    "Memory hygiene constraint".to_string(),
                    "Keep duplicate durable memories merged by content hash".to_string(),
                ),
                (
                    MemoryType::Fact,
                    "Memory hygiene fact".to_string(),
                    "Persistent memory consolidation marks superseded duplicates as deleted"
                        .to_string(),
                ),
            ]
        } else {
            vec![
                (
                    MemoryType::Decision,
                    format!("{} decision", context_label),
                    format!(
                        "Use durable memory isolation for {}",
                        input.scope.context_key
                    ),
                ),
                (
                    MemoryType::Constraint,
                    format!("{} constraint", context_label),
                    format!(
                        "{} durable memory records must stay isolated",
                        input.scope.context_key
                    ),
                ),
                (
                    MemoryType::Fact,
                    format!("{} fact", context_label),
                    format!(
                        "{} records are stored under their own context key",
                        input.scope.context_key
                    ),
                ),
            ]
        };

        let memories = memory_specs
            .into_iter()
            .map(|(memory_type, title, content)| {
                let content_hash =
                    oxide_agent_memory::stable_memory_content_hash(memory_type, &content);
                let mut tags = vec!["llm_post_run".to_string()];
                if input.explicit_remember_intent {
                    tags.push("explicit_remember".to_string());
                }
                MemoryRecord {
                    memory_id: format!(
                        "fake:{}:{}:{}",
                        input.task_id,
                        super::memory_type_label(memory_type),
                        &content_hash[..12.min(content_hash.len())]
                    ),
                    context_key: input.scope.context_key.clone(),
                    source_episode_id: Some(input.task_id.to_string()),
                    memory_type,
                    title,
                    short_description: content.clone(),
                    content,
                    importance: 0.9,
                    confidence: 0.95,
                    source: Some("fake_post_run_writer".to_string()),
                    content_hash: Some(content_hash),
                    reason: Some("test llm writer".to_string()),
                    tags,
                    created_at: now,
                    updated_at: now,
                    deleted_at: None,
                }
            })
            .collect();

        Ok(ValidatedPostRunMemoryWrite {
            thread_short_summary: Some(format!("{} summary", input.task)),
            episode: ValidatedPostRunEpisode {
                summary: format!("Completed task: {}", input.task),
                outcome: EpisodeOutcome::Success,
                failures: Vec::new(),
                importance: 0.9,
            },
            memories,
        })
    }
}

fn test_coordinator(store: Arc<dyn PersistentMemoryStore>) -> PersistentMemoryCoordinator {
    PersistentMemoryCoordinator::new(store).with_memory_writer(Arc::new(FakePostRunMemoryWriter))
}

#[async_trait::async_trait]
impl MemoryEmbeddingGenerator for FakeEmbeddingGenerator {
    async fn embed_document(&self, _text: &str, _title: Option<&str>) -> anyhow::Result<Vec<f32>> {
        Ok(vec![1.0, 0.0])
    }

    async fn embed_query(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![1.0, 0.0])
    }
}

#[tokio::test]
async fn persist_completed_run_writes_episode_and_session_state() {
    let store = Arc::new(InMemoryMemoryRepository::new());
    let store_for_coordinator = Arc::clone(&store);
    let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
    let coordinator = test_coordinator(store_for_coordinator);
    let scope = AgentMemoryScope::new(42, "topic-a", "flow-1");
    let messages = vec![
        AgentMessage::tool("tool-1", "read_file", "content"),
        AgentMessage::archive_reference_with_ref(
            "Archived displaced context chunk",
            Some(ArchiveRef {
                archive_id: "archive-1".to_string(),
                created_at: 1_700_000_000,
                title: "Compacted history".to_string(),
                storage_key: "archive/topic-a/flow-1/history-1.json".to_string(),
            }),
        ),
        AgentMessage::from_compaction_summary(crate::agent::CompactionSummary {
            goal: "Implement Stage 4".to_string(),
            decisions: vec!["Use persistent memory coordinator for PostRun durable writes".to_string()],
            constraints: vec!["Sub-agent runs must never persist durable memory records".to_string()],
            discoveries: vec!["PostRun persistence is handled in crates/oxide-agent-core/src/agent/runner/responses.rs".to_string()],
            risks: vec!["Need follow-up test".to_string()],
            ..crate::agent::CompactionSummary::default()
        }),
    ];

    coordinator
        .persist_post_run(PersistentRunContext {
            session_id: "session-1",
            task_id: "episode-1",
            scope: &scope,
            task: "Implement Stage 4",
            messages: &messages,
            explicit_remember_intent: false,
            hot_token_estimate: 77,
            tool_memory_drafts: Vec::new(),
            phase: PersistentRunPhase::Completed {
                final_answer: "Done",
            },
        })
        .await
        .expect("post-run persistence should succeed");

    let episode = MemoryRepository::get_episode(store.as_ref(), &"episode-1".to_string())
        .await
        .expect("episode lookup should succeed")
        .expect("episode should exist");
    assert_eq!(episode.goal, "Implement Stage 4");
    assert_eq!(episode.summary, "Completed task: Implement Stage 4");
    assert_eq!(episode.tools_used, vec!["read_file".to_string()]);
    assert!(episode.failures.is_empty());
    assert_eq!(episode.artifacts.len(), 1);
    assert_eq!(
        episode.artifacts[0].storage_key,
        "archive/topic-a/flow-1/history-1.json"
    );

    let session_state = store
        .get_session_state("session-1")
        .await
        .expect("session state lookup should succeed")
        .expect("session state should exist");
    assert_eq!(session_state.cleanup_status, CleanupStatus::Finalized);
    assert_eq!(session_state.pending_episode_id, None);

    let memories =
        MemoryRepository::list_memories(store.as_ref(), "topic-a", &MemoryListFilter::default())
            .await
            .expect("memory lookup should succeed");
    assert_eq!(memories.len(), 3);
    assert!(memories
        .iter()
        .any(|memory| memory.memory_type == MemoryType::Decision));
    assert!(memories
        .iter()
        .any(|memory| memory.memory_type == MemoryType::Constraint));
    assert!(memories
        .iter()
        .any(|memory| memory.memory_type == MemoryType::Fact));
}

#[tokio::test]
async fn persist_waiting_for_user_input_only_updates_session_state() {
    let store = Arc::new(InMemoryMemoryRepository::new());
    let store_for_coordinator = Arc::clone(&store);
    let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
    let coordinator = PersistentMemoryCoordinator::new(store_for_coordinator);
    let scope = AgentMemoryScope::new(42, "topic-a", "flow-1");

    coordinator
        .persist_post_run(PersistentRunContext {
            session_id: "session-1",
            task_id: "episode-1",
            scope: &scope,
            task: "Need browser URL",
            messages: &[],
            explicit_remember_intent: false,
            hot_token_estimate: 21,
            tool_memory_drafts: Vec::new(),
            phase: PersistentRunPhase::WaitingForUserInput,
        })
        .await
        .expect("waiting-state persistence should succeed");

    let state = store
        .get_session_state("session-1")
        .await
        .expect("session state lookup should succeed")
        .expect("session state should exist");
    assert_eq!(state.cleanup_status, CleanupStatus::Idle);
    assert_eq!(state.pending_episode_id.as_deref(), Some("episode-1"));
    assert!(
        MemoryRepository::get_episode(store.as_ref(), &"episode-1".to_string())
            .await
            .expect("episode lookup should succeed")
            .is_none()
    );
    assert!(MemoryRepository::list_memories(
        store.as_ref(),
        "topic-a",
        &MemoryListFilter::default()
    )
    .await
    .expect("memory lookup should succeed")
    .is_empty());
}

#[tokio::test]
async fn persist_completed_run_propagates_explicit_remember_intent_to_writer() {
    let store = Arc::new(InMemoryMemoryRepository::new());
    let store_for_coordinator = Arc::clone(&store);
    let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
    let coordinator = test_coordinator(store_for_coordinator);
    let scope = AgentMemoryScope::new(42, "topic-a", "flow-1");
    let messages = vec![AgentMessage::user_turn(
        "Please remember this deployment workaround for later.",
    )];

    coordinator
        .persist_post_run(PersistentRunContext {
            session_id: "session-remember",
            task_id: "episode-remember",
            scope: &scope,
            task: "Remember this deployment workaround",
            messages: &messages,
            explicit_remember_intent: true,
            hot_token_estimate: 33,
            tool_memory_drafts: Vec::new(),
            phase: PersistentRunPhase::Completed {
                final_answer: "Saved in memory after completion.",
            },
        })
        .await
        .expect("remember-intent persistence should succeed");

    let memories =
        MemoryRepository::list_memories(store.as_ref(), "topic-a", &MemoryListFilter::default())
            .await
            .expect("memory lookup should succeed");
    assert!(!memories.is_empty());
    assert!(memories
        .iter()
        .all(|memory| memory.tags.iter().any(|tag| tag == "explicit_remember")));
}

#[tokio::test]
async fn persist_post_run_keeps_topic_scopes_isolated() {
    let store = Arc::new(InMemoryMemoryRepository::new());
    let store_for_coordinator = Arc::clone(&store);
    let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
    let coordinator = test_coordinator(store_for_coordinator);

    let topic_a_scope = AgentMemoryScope::new(42, "topic-a", "flow-a");
    let topic_b_scope = AgentMemoryScope::new(42, "topic-b", "flow-b");

    let topic_a_messages = vec![
        AgentMessage::tool("tool-a", "read_file", "content"),
        AgentMessage::from_compaction_summary(crate::agent::CompactionSummary {
            goal: "Topic A".to_string(),
            decisions: vec![
                "Use persistent memory repository for topic-a durable writes".to_string(),
            ],
            constraints: vec!["Topic-a durable memory records must stay isolated".to_string()],
            discoveries: vec!["topic-a records are stored in context_key".to_string()],
            risks: vec!["Need follow-up test".to_string()],
            ..crate::agent::CompactionSummary::default()
        }),
    ];
    let topic_b_messages = vec![
        AgentMessage::tool("tool-b", "read_file", "content"),
        AgentMessage::from_compaction_summary(crate::agent::CompactionSummary {
            goal: "Topic B".to_string(),
            decisions: vec![
                "Use persistent memory repository for topic-b durable writes".to_string(),
            ],
            constraints: vec!["Topic-b durable memory records must stay isolated".to_string()],
            discoveries: vec!["topic-b records are stored in context_key".to_string()],
            risks: vec!["Need follow-up test".to_string()],
            ..crate::agent::CompactionSummary::default()
        }),
    ];

    coordinator
        .persist_post_run(PersistentRunContext {
            session_id: "session-a",
            task_id: "episode-a",
            scope: &topic_a_scope,
            task: "topic a task",
            messages: &topic_a_messages,
            explicit_remember_intent: false,
            hot_token_estimate: 128,
            tool_memory_drafts: Vec::new(),
            phase: PersistentRunPhase::Completed {
                final_answer: "done",
            },
        })
        .await
        .expect("topic-a persistence should succeed");

    coordinator
        .persist_post_run(PersistentRunContext {
            session_id: "session-b",
            task_id: "episode-b",
            scope: &topic_b_scope,
            task: "topic b task",
            messages: &topic_b_messages,
            explicit_remember_intent: false,
            hot_token_estimate: 256,
            tool_memory_drafts: Vec::new(),
            phase: PersistentRunPhase::Completed {
                final_answer: "done",
            },
        })
        .await
        .expect("topic-b persistence should succeed");

    let topic_a_memories =
        MemoryRepository::list_memories(store.as_ref(), "topic-a", &MemoryListFilter::default())
            .await
            .expect("topic-a memory lookup should succeed");
    let topic_b_memories =
        MemoryRepository::list_memories(store.as_ref(), "topic-b", &MemoryListFilter::default())
            .await
            .expect("topic-b memory lookup should succeed");

    assert_eq!(topic_a_memories.len(), 3);
    assert_eq!(topic_b_memories.len(), 3);
    assert!(topic_a_memories
        .iter()
        .all(|memory| memory.context_key == "topic-a"));
    assert!(topic_b_memories
        .iter()
        .all(|memory| memory.context_key == "topic-b"));
    assert!(
        MemoryRepository::get_episode(store.as_ref(), &"episode-a".to_string())
            .await
            .expect("episode-a lookup should succeed")
            .is_some()
    );
    assert!(
        MemoryRepository::get_episode(store.as_ref(), &"episode-b".to_string())
            .await
            .expect("episode-b lookup should succeed")
            .is_some()
    );
}

#[tokio::test]
async fn embedding_indexer_backfills_existing_episode_records() {
    let mut storage = MockStorageProvider::new();
    let candidate = EpisodeEmbeddingCandidate {
        record: EpisodeRecord {
            episode_id: "episode-1".to_string(),
            thread_id: "thread-1".to_string(),
            context_key: "topic-a".to_string(),
            goal: "Index embeddings".to_string(),
            summary: "Backfill older persistent memory records".to_string(),
            outcome: oxide_agent_memory::EpisodeOutcome::Success,
            tools_used: vec!["memory_search".to_string()],
            artifacts: Vec::new(),
            failures: Vec::new(),
            importance: 0.8,
            created_at: chrono::Utc::now(),
        },
        embedding: None,
    };

    storage
        .expect_list_memory_episode_embedding_backfill_candidates()
        .times(1)
        .return_once(move |_| Ok(vec![candidate]));
    storage
        .expect_list_memory_record_embedding_backfill_candidates()
        .times(1)
        .return_once(|_| Ok(Vec::new()));
    storage
        .expect_upsert_memory_embedding_pending()
        .times(1)
        .returning(|update: EmbeddingPendingUpdate| {
            Ok(oxide_agent_memory::EmbeddingRecord {
                owner_id: update.base.owner_id,
                owner_type: update.base.owner_type,
                model_id: update.base.model_id,
                content_hash: update.base.content_hash,
                embedding: None,
                dimensions: None,
                status: oxide_agent_memory::EmbeddingStatus::Pending,
                last_error: None,
                retry_count: 0,
                created_at: update.requested_at,
                updated_at: update.requested_at,
                indexed_at: None,
            })
        });
    storage
        .expect_upsert_memory_embedding_ready()
        .times(1)
        .returning(|update: EmbeddingReadyUpdate| {
            Ok(oxide_agent_memory::EmbeddingRecord {
                owner_id: update.base.owner_id,
                owner_type: update.base.owner_type,
                model_id: update.base.model_id,
                content_hash: update.base.content_hash,
                dimensions: Some(update.embedding.len()),
                embedding: Some(update.embedding),
                status: oxide_agent_memory::EmbeddingStatus::Ready,
                last_error: None,
                retry_count: 0,
                created_at: update.indexed_at,
                updated_at: update.indexed_at,
                indexed_at: Some(update.indexed_at),
            })
        });

    let indexer = PersistentMemoryEmbeddingIndexer::new(
        Arc::new(storage),
        Arc::new(FakeEmbeddingGenerator),
        "gemini-embedding-001",
    );

    indexer.backfill().await.expect("backfill should succeed");
}

fn retrieval_scope() -> AgentMemoryScope {
    AgentMemoryScope::new(42, "topic-a", "flow-a")
}

fn retrieval_episode() -> EpisodeRecord {
    EpisodeRecord {
        episode_id: "episode-1".to_string(),
        thread_id: "thread-1".to_string(),
        context_key: "topic-a".to_string(),
        goal: "Fix deploy regression".to_string(),
        summary: "Earlier deploy broke staging until config was corrected.".to_string(),
        outcome: EpisodeOutcome::Success,
        tools_used: vec!["memory_search".to_string()],
        artifacts: Vec::new(),
        failures: Vec::new(),
        importance: 0.82,
        created_at: chrono::Utc::now(),
    }
}

fn retrieval_memory() -> MemoryRecord {
    MemoryRecord {
        memory_id: "memory-1".to_string(),
        context_key: "topic-a".to_string(),
        source_episode_id: Some("episode-9".to_string()),
        memory_type: MemoryType::Procedure,
        title: "Deploy fix procedure".to_string(),
        content: "Rebuild config, then rerun the deploy with the staging profile.".to_string(),
        short_description: "staging recovery steps".to_string(),
        importance: 0.93,
        confidence: 0.94,
        source: Some("test".to_string()),
        content_hash: Some(oxide_agent_memory::stable_memory_content_hash(
            MemoryType::Procedure,
            "Rebuild config, then rerun the deploy with the staging profile.",
        )),
        reason: Some("fixture".to_string()),
        tags: vec!["deploy".to_string(), "staging".to_string()],
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
    }
}

fn ts(seconds: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::Utc
        .timestamp_opt(seconds, 0)
        .single()
        .expect("valid timestamp")
}

fn repeated_summary_messages() -> Vec<AgentMessage> {
    vec![AgentMessage::from_compaction_summary(crate::agent::CompactionSummary {
        goal: "Keep memory hygiene".to_string(),
        decisions: vec!["Use persistent memory coordinator for durable writes".to_string()],
        constraints: vec!["Sub-agent runs must never persist durable memory records".to_string()],
        discoveries: vec!["PostRun persistence is handled in crates/oxide-agent-core/src/agent/runner/responses.rs".to_string()],
        ..crate::agent::CompactionSummary::default()
    })]
}

#[tokio::test]
async fn durable_memory_retriever_skips_smalltalk_queries() {
    let storage = MockStorageProvider::new();
    let retriever = DurableMemoryRetriever::new(Arc::new(storage));

    let retrieval = retriever
        .retrieve(
            "thanks",
            &retrieval_scope(),
            DurableMemoryRetrievalOptions::default(),
        )
        .await
        .expect("smalltalk retrieval should not fail");

    assert!(retrieval.is_none());
}

#[tokio::test]
async fn durable_memory_retriever_prefers_memory_recall_for_procedure_queries() {
    let memory_for_lexical = retrieval_memory();
    let memory_for_vector = retrieval_memory();
    let mut storage = MockStorageProvider::new();
    storage
        .expect_search_memory_records_lexical()
        .times(1)
        .return_once(move |_, _| {
            Ok(vec![MemorySearchHit {
                record: memory_for_lexical,
                score: 0.3,
                snippet: "memory lexical".to_string(),
            }])
        });
    storage
        .expect_search_memory_records_vector()
        .times(1)
        .return_once(move |_, _| {
            Ok(vec![MemorySearchHit {
                record: memory_for_vector,
                score: 0.96,
                snippet: "memory semantic".to_string(),
            }])
        });

    let retriever = DurableMemoryRetriever::new(Arc::new(storage))
        .with_query_embedding_generator(Arc::new(FakeEmbeddingGenerator));
    let retrieval = retriever
        .retrieve(
            "deploy procedure for staging fix",
            &retrieval_scope(),
            DurableMemoryRetrievalOptions::default(),
        )
        .await
        .expect("procedure retrieval should succeed")
        .expect("retrieval should produce candidates");

    assert_eq!(retrieval.items.len(), 1);
    assert!(matches!(retrieval.items[0], HybridCandidate::Memory { .. }));

    let rendered = retrieval.render_for_prompt();
    assert!(rendered.contains("Scoped durable memory context"));
    assert!(rendered.contains("memory memory-1"));
    assert!(!rendered.contains("episode episode-1"));
}

#[tokio::test]
async fn durable_memory_search_supports_explicit_hybrid_requests() {
    let episode = retrieval_episode();
    let memory_for_lexical = retrieval_memory();
    let memory_for_vector = retrieval_memory();
    let mut storage = MockStorageProvider::new();
    storage
        .expect_search_memory_episodes_lexical()
        .times(1)
        .returning({
            let episode = episode.clone();
            move |_, _| {
                Ok(vec![EpisodeSearchHit {
                    record: episode.clone(),
                    score: 0.4,
                    snippet: "episode lexical".to_string(),
                }])
            }
        });
    storage
        .expect_search_memory_episodes_vector()
        .times(1)
        .returning(|_, _| Ok(Vec::new()));
    storage
        .expect_search_memory_records_lexical()
        .times(1)
        .returning({
            let memory_for_lexical = memory_for_lexical.clone();
            move |_, _| {
                Ok(vec![MemorySearchHit {
                    record: memory_for_lexical.clone(),
                    score: 0.3,
                    snippet: "memory lexical".to_string(),
                }])
            }
        });
    storage
        .expect_search_memory_records_vector()
        .times(1)
        .returning({
            let memory_for_vector = memory_for_vector.clone();
            move |_, _| {
                Ok(vec![MemorySearchHit {
                    record: memory_for_vector.clone(),
                    score: 0.96,
                    snippet: "memory semantic".to_string(),
                }])
            }
        });

    let retriever = DurableMemoryRetriever::new(Arc::new(storage))
        .with_query_embedding_generator(Arc::new(FakeEmbeddingGenerator));
    let search_items = retriever
        .search(
            &retrieval_scope(),
            DurableMemorySearchRequest {
                query: "how was the deploy fixed before?".to_string(),
                search_episodes: true,
                search_memories: true,
                memory_type: Some(MemoryType::Procedure),
                time_range: Default::default(),
                min_importance: Some(0.45),
                limit: 5,
                candidate_limit: Some(8),
                allow_full_thread_read: true,
            },
        )
        .await
        .expect("tool search should succeed");

    assert_eq!(search_items.len(), 2);
    assert!(matches!(
        search_items[0],
        DurableMemorySearchItem::Memory { .. }
    ));
    assert!(matches!(
        search_items[1],
        DurableMemorySearchItem::Episode { .. }
    ));
}

#[tokio::test]
async fn durable_memory_retriever_skips_external_fresh_fact_queries() {
    let storage = MockStorageProvider::new();
    let retriever = DurableMemoryRetriever::new(Arc::new(storage));

    let retrieval = retriever
        .retrieve(
            "Когда релиз The Boys S5 будет?",
            &retrieval_scope(),
            DurableMemoryRetrievalOptions::default(),
        )
        .await
        .expect("fresh external fact retrieval should not fail");

    assert!(retrieval.is_none());
}

#[tokio::test]
async fn durable_memory_search_filters_vector_only_memory_for_general_queries() {
    let memory_for_vector = retrieval_memory();
    let mut storage = MockStorageProvider::new();
    storage
        .expect_search_memory_records_lexical()
        .times(1)
        .return_once(|_, _| Ok(Vec::new()));
    storage
        .expect_search_memory_records_vector()
        .times(1)
        .return_once(move |_, _| {
            Ok(vec![MemorySearchHit {
                record: memory_for_vector,
                score: 0.96,
                snippet: "memory semantic".to_string(),
            }])
        });

    let retriever = DurableMemoryRetriever::new(Arc::new(storage))
        .with_query_embedding_generator(Arc::new(FakeEmbeddingGenerator));
    let outcome = retriever
        .search_with_diagnostics(
            &retrieval_scope(),
            DurableMemorySearchRequest {
                query: "Summarize this".to_string(),
                search_episodes: false,
                search_memories: true,
                memory_type: None,
                time_range: Default::default(),
                min_importance: Some(0.0),
                limit: 5,
                candidate_limit: Some(8),
                allow_full_thread_read: false,
            },
        )
        .await
        .expect("general memory search should succeed");

    assert!(outcome.items.is_empty());
    assert_eq!(outcome.diagnostics.filtered_vector_only_memory, 1);
    assert_eq!(
        outcome.diagnostics.empty_reason,
        Some("all_candidates_deduplicated_or_covered")
    );
}

#[tokio::test]
async fn durable_memory_search_reports_empty_reason_when_search_returns_no_candidates() {
    let mut storage = MockStorageProvider::new();
    storage
        .expect_search_memory_episodes_lexical()
        .times(1)
        .return_once(|_, _| Ok(Vec::new()));
    storage
        .expect_search_memory_records_lexical()
        .times(1)
        .return_once(|_, _| Ok(Vec::new()));

    let retriever = DurableMemoryRetriever::new(Arc::new(storage));
    let outcome = retriever
        .search_with_diagnostics(
            &retrieval_scope(),
            DurableMemorySearchRequest {
                query: "how was the deploy fixed before?".to_string(),
                search_episodes: true,
                search_memories: true,
                memory_type: None,
                time_range: Default::default(),
                min_importance: None,
                limit: 5,
                candidate_limit: Some(8),
                allow_full_thread_read: true,
            },
        )
        .await
        .expect("diagnostic search should succeed");

    assert!(outcome.items.is_empty());
    assert_eq!(outcome.diagnostics.empty_reason, Some("no_search_hits"));
    assert_eq!(outcome.diagnostics.episode_lexical_hits, 0);
    assert_eq!(outcome.diagnostics.injected_item_count, 0);
    assert_eq!(outcome.diagnostics.filtered_low_score, 0);
}

#[tokio::test]
async fn persist_post_run_consolidates_duplicate_memories() {
    let store = Arc::new(InMemoryMemoryRepository::new());
    let store_for_coordinator = Arc::clone(&store);
    let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
    let coordinator = test_coordinator(store_for_coordinator);
    let scope = AgentMemoryScope::new(42, "topic-a", "flow-1");
    let messages = repeated_summary_messages();

    coordinator
        .persist_post_run(PersistentRunContext {
            session_id: "session-1",
            task_id: "episode-1",
            scope: &scope,
            task: "keep project memory hygiene",
            messages: &messages,
            explicit_remember_intent: false,
            hot_token_estimate: 32,
            tool_memory_drafts: Vec::new(),
            phase: PersistentRunPhase::Completed {
                final_answer: "done",
            },
        })
        .await
        .expect("first persistence should succeed");
    coordinator
        .persist_post_run(PersistentRunContext {
            session_id: "session-2",
            task_id: "episode-2",
            scope: &scope,
            task: "keep project memory hygiene again",
            messages: &messages,
            explicit_remember_intent: false,
            hot_token_estimate: 40,
            tool_memory_drafts: Vec::new(),
            phase: PersistentRunPhase::Completed {
                final_answer: "done",
            },
        })
        .await
        .expect("second persistence should succeed");

    let active =
        MemoryRepository::list_memories(store.as_ref(), "topic-a", &MemoryListFilter::default())
            .await
            .expect("active memories should list");
    let all = MemoryRepository::list_memories(
        store.as_ref(),
        "topic-a",
        &MemoryListFilter {
            include_deleted: true,
            ..MemoryListFilter::default()
        },
    )
    .await
    .expect("full memory listing should succeed");

    assert_eq!(active.len(), 3);
    assert_eq!(
        active
            .iter()
            .filter_map(|memory| memory.content_hash.as_ref())
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    assert!((3..=6).contains(&all.len()));
    assert!(
        all.iter()
            .filter(|memory| memory.deleted_at.is_some())
            .count()
            <= 3
    );
}

#[tokio::test]
async fn persist_post_run_suppresses_llm_memories_for_external_fresh_facts() {
    let store = Arc::new(InMemoryMemoryRepository::new());
    let store_for_coordinator = Arc::clone(&store);
    let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
    let coordinator = test_coordinator(store_for_coordinator);
    let scope = AgentMemoryScope::new(42, "topic-a", "flow-1");
    let messages = vec![AgentMessage::user_turn("Когда релиз The Boys S5 будет?")];

    coordinator
        .persist_post_run(PersistentRunContext {
            session_id: "session-fresh-fact",
            task_id: "episode-fresh-fact",
            scope: &scope,
            task: "Когда релиз The Boys S5 будет?",
            messages: &messages,
            explicit_remember_intent: false,
            hot_token_estimate: 24,
            tool_memory_drafts: Vec::new(),
            phase: PersistentRunPhase::Completed {
                final_answer: "Сезон еще не вышел.",
            },
        })
        .await
        .expect("fresh fact persistence should succeed");

    let memories =
        MemoryRepository::list_memories(store.as_ref(), "topic-a", &MemoryListFilter::default())
            .await
            .expect("memory lookup should succeed");
    assert!(memories.is_empty());
    assert!(
        MemoryRepository::get_episode(store.as_ref(), &"episode-fresh-fact".to_string())
            .await
            .expect("episode lookup should succeed")
            .is_some()
    );
}

#[tokio::test]
async fn persist_post_run_persists_tool_drafts_when_llm_memories_are_suppressed() {
    let store = Arc::new(InMemoryMemoryRepository::new());
    let store_for_coordinator = Arc::clone(&store);
    let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
    let coordinator = test_coordinator(store_for_coordinator);
    let scope = AgentMemoryScope::new(42, "topic-a", "flow-1");
    let messages = vec![AgentMessage::user_turn("Когда релиз The Boys S5 будет?")];
    let tool_memory_drafts = vec![ToolDerivedMemoryDraft {
        memory_type: MemoryType::Procedure,
        title: "Release lookup workflow".to_string(),
        content: "Use web search to verify release date and current streaming availability before answering.".to_string(),
        short_description: "Check release date and streaming status".to_string(),
        importance: 0.81,
        confidence: 0.88,
        source: "test_tool_draft".to_string(),
        reason: "Observed explicit fact-check workflow".to_string(),
        tags: vec!["procedure".to_string(), "web_search".to_string()],
        captured_at: chrono::Utc::now(),
    }];

    coordinator
        .persist_post_run(PersistentRunContext {
            session_id: "session-tool-draft",
            task_id: "episode-tool-draft",
            scope: &scope,
            task: "Когда релиз The Boys S5 будет?",
            messages: &messages,
            explicit_remember_intent: false,
            hot_token_estimate: 24,
            tool_memory_drafts,
            phase: PersistentRunPhase::Completed {
                final_answer: "Сезон еще не вышел.",
            },
        })
        .await
        .expect("tool-draft persistence should succeed");

    let memories =
        MemoryRepository::list_memories(store.as_ref(), "topic-a", &MemoryListFilter::default())
            .await
            .expect("memory lookup should succeed");
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].memory_type, MemoryType::Procedure);
    assert!(memories[0].tags.iter().any(|tag| tag == "tool_draft"));
    assert_eq!(memories[0].source.as_deref(), Some("test_tool_draft"));
}

#[tokio::test]
async fn watchdog_pass_consolidates_stale_context() {
    let store = Arc::new(InMemoryMemoryRepository::new());
    MemoryRepository::create_memory(
        store.as_ref(),
        MemoryRecord {
            memory_id: "memory-a".to_string(),
            context_key: "topic-a".to_string(),
            source_episode_id: Some("episode-a".to_string()),
            memory_type: MemoryType::Fact,
            title: "Fact a".to_string(),
            content: "Use cargo check before build".to_string(),
            short_description: "cargo check before build".to_string(),
            importance: 0.7,
            confidence: 0.8,
            source: Some("test".to_string()),
            content_hash: Some(oxide_agent_memory::stable_memory_content_hash(
                MemoryType::Fact,
                "Use cargo check before build",
            )),
            reason: None,
            tags: vec!["fact".to_string()],
            created_at: ts(10),
            updated_at: ts(10),
            deleted_at: None,
        },
    )
    .await
    .expect("first memory should store");
    MemoryRepository::create_memory(
        store.as_ref(),
        MemoryRecord {
            memory_id: "memory-b".to_string(),
            context_key: "topic-a".to_string(),
            source_episode_id: Some("episode-b".to_string()),
            memory_type: MemoryType::Fact,
            title: "Fact b".to_string(),
            content: "Use cargo check before build".to_string(),
            short_description: "cargo check before build".to_string(),
            importance: 0.6,
            confidence: 0.7,
            source: Some("test".to_string()),
            content_hash: Some(oxide_agent_memory::stable_memory_content_hash(
                MemoryType::Fact,
                "Use cargo check before build",
            )),
            reason: None,
            tags: vec!["fact".to_string()],
            created_at: ts(20),
            updated_at: ts(20),
            deleted_at: None,
        },
    )
    .await
    .expect("second memory should store");
    MemoryRepository::upsert_session_state(
        store.as_ref(),
        SessionStateRecord {
            session_id: "session-a".to_string(),
            context_key: "topic-a".to_string(),
            hot_token_estimate: 64,
            last_compacted_at: None,
            last_finalized_at: None,
            cleanup_status: CleanupStatus::Idle,
            pending_episode_id: None,
            updated_at: ts(0),
        },
    )
    .await
    .expect("session state should store");

    let store_for_coordinator = Arc::clone(&store);
    let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
    let coordinator = PersistentMemoryCoordinator::new(store_for_coordinator);
    coordinator.run_watchdog_pass(ts(100_000)).await;

    let active =
        MemoryRepository::list_memories(store.as_ref(), "topic-a", &MemoryListFilter::default())
            .await
            .expect("active memories should list");
    let deleted = MemoryRepository::list_memories(
        store.as_ref(),
        "topic-a",
        &MemoryListFilter {
            include_deleted: true,
            ..MemoryListFilter::default()
        },
    )
    .await
    .expect("deleted memories should list");

    assert_eq!(active.len(), 1);
    assert_eq!(
        deleted
            .iter()
            .filter(|memory| memory.deleted_at.is_some())
            .count(),
        1
    );
}
