use crate::agent::memory::AgentMessage;
use crate::agent::session::AgentMemoryScope;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use oxide_agent_memory::{
    ArtifactRef, EpisodeFinalizationInput, EpisodeFinalizer, EpisodeMemorySignals,
    MemoryRepository, RepositoryError, ReusableMemoryExtractor, SessionStateRecord, ThreadRecord,
};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::warn;

/// Object-safe persistent-memory write surface used by the runner.
#[async_trait]
pub trait PersistentMemoryStore: Send + Sync {
    async fn upsert_thread(&self, record: ThreadRecord) -> Result<ThreadRecord, RepositoryError>;
    async fn create_episode(
        &self,
        record: oxide_agent_memory::EpisodeRecord,
    ) -> Result<oxide_agent_memory::EpisodeRecord, RepositoryError>;
    async fn create_memory(
        &self,
        record: oxide_agent_memory::MemoryRecord,
    ) -> Result<oxide_agent_memory::MemoryRecord, RepositoryError>;
    async fn upsert_session_state(
        &self,
        record: SessionStateRecord,
    ) -> Result<SessionStateRecord, RepositoryError>;
}

#[async_trait]
impl<T> PersistentMemoryStore for T
where
    T: MemoryRepository + Send + Sync,
{
    async fn upsert_thread(&self, record: ThreadRecord) -> Result<ThreadRecord, RepositoryError> {
        MemoryRepository::upsert_thread(self, record).await
    }

    async fn create_episode(
        &self,
        record: oxide_agent_memory::EpisodeRecord,
    ) -> Result<oxide_agent_memory::EpisodeRecord, RepositoryError> {
        MemoryRepository::create_episode(self, record).await
    }

    async fn create_memory(
        &self,
        record: oxide_agent_memory::MemoryRecord,
    ) -> Result<oxide_agent_memory::MemoryRecord, RepositoryError> {
        MemoryRepository::create_memory(self, record).await
    }

    async fn upsert_session_state(
        &self,
        record: SessionStateRecord,
    ) -> Result<SessionStateRecord, RepositoryError> {
        MemoryRepository::upsert_session_state(self, record).await
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PersistentRunPhase<'a> {
    Completed { final_answer: &'a str },
    WaitingForUserInput,
}

pub struct PersistentRunContext<'a> {
    pub session_id: &'a str,
    pub task_id: &'a str,
    pub scope: &'a AgentMemoryScope,
    pub task: &'a str,
    pub messages: &'a [AgentMessage],
    pub hot_token_estimate: usize,
    pub phase: PersistentRunPhase<'a>,
}

#[derive(Clone)]
pub struct PersistentMemoryCoordinator {
    store: Arc<dyn PersistentMemoryStore>,
    finalizer: EpisodeFinalizer,
    extractor: ReusableMemoryExtractor,
}

impl PersistentMemoryCoordinator {
    #[must_use]
    pub fn new(store: Arc<dyn PersistentMemoryStore>) -> Self {
        Self {
            store,
            finalizer: EpisodeFinalizer,
            extractor: ReusableMemoryExtractor::new(),
        }
    }

    pub async fn persist_post_run(&self, ctx: PersistentRunContext<'_>) -> Result<()> {
        let summary_signal = latest_summary_signal(ctx.messages);
        let artifacts = collect_artifacts(ctx.messages);
        let tools_used = collect_tools_used(ctx.messages);
        let final_answer = match ctx.phase {
            PersistentRunPhase::Completed { final_answer } => Some(final_answer.to_string()),
            PersistentRunPhase::WaitingForUserInput => None,
        };
        let plan = self.finalizer.build_plan(EpisodeFinalizationInput {
            user_id: ctx.scope.user_id,
            context_key: ctx.scope.context_key.clone(),
            flow_id: ctx.scope.flow_id.clone(),
            session_id: ctx.session_id.to_string(),
            episode_id: ctx.task_id.to_string(),
            goal: ctx.task.to_string(),
            final_answer,
            compaction_summary: summary_signal
                .as_ref()
                .map(|signal| signal.summary_text.clone()),
            tools_used,
            artifacts,
            failures: summary_signal
                .as_ref()
                .map_or_else(Vec::new, |signal| signal.failures.clone()),
            hot_token_estimate: ctx.hot_token_estimate,
            finalized_at: Utc::now(),
        });

        self.store.upsert_thread(plan.thread).await?;
        let episode = if let Some(episode) = plan.episode {
            Some(self.store.create_episode(episode).await?)
        } else {
            None
        };
        if let Some(episode) = episode.as_ref() {
            self.persist_reusable_memories(episode, summary_signal.as_ref())
                .await;
        }
        self.store.upsert_session_state(plan.session_state).await?;
        Ok(())
    }

    async fn persist_reusable_memories(
        &self,
        episode: &oxide_agent_memory::EpisodeRecord,
        summary_signal: Option<&PersistentSummarySignal>,
    ) {
        let Some(summary_signal) = summary_signal else {
            return;
        };

        let signals = EpisodeMemorySignals {
            decisions: summary_signal.decisions.clone(),
            constraints: summary_signal.constraints.clone(),
            discoveries: summary_signal.discoveries.clone(),
        };
        for memory in self.extractor.extract(episode, &signals) {
            if let Err(error) = self.store.create_memory(memory).await {
                warn!(error = %error, episode_id = %episode.episode_id, "Reusable memory extraction write failed");
            }
        }
    }
}

#[derive(Debug, Clone)]
struct PersistentSummarySignal {
    summary_text: String,
    decisions: Vec<String>,
    constraints: Vec<String>,
    discoveries: Vec<String>,
    failures: Vec<String>,
}

fn latest_summary_signal(messages: &[AgentMessage]) -> Option<PersistentSummarySignal> {
    let mut latest_summary = None;

    for message in messages.iter().rev() {
        let Some(summary) = message.summary_payload() else {
            continue;
        };

        latest_summary = Some(PersistentSummarySignal {
            summary_text: message.content.trim().to_string(),
            decisions: summary
                .decisions
                .iter()
                .map(|item| item.trim())
                .filter(|item| !item.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
            constraints: summary
                .constraints
                .iter()
                .map(|item| item.trim())
                .filter(|item| !item.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
            discoveries: summary
                .discoveries
                .iter()
                .map(|item| item.trim())
                .filter(|item| !item.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
            failures: summary
                .risks
                .iter()
                .map(|risk| risk.trim())
                .filter(|risk| !risk.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
        });
        break;
    }

    latest_summary
}

fn collect_artifacts(messages: &[AgentMessage]) -> Vec<ArtifactRef> {
    let mut seen = HashSet::new();
    let mut artifacts = Vec::new();

    for message in messages {
        if let Some(archive_ref) = message.archive_ref_payload() {
            push_artifact(
                &mut artifacts,
                &mut seen,
                archive_ref.storage_key.clone(),
                archive_ref.title.clone(),
                Some("application/json".to_string()),
                archive_ref.created_at,
            );
        }
        if let Some(payload) = &message.externalized_payload {
            push_artifact(
                &mut artifacts,
                &mut seen,
                payload.archive_ref.storage_key.clone(),
                payload.archive_ref.title.clone(),
                Some("text/plain".to_string()),
                payload.archive_ref.created_at,
            );
        }
        if let Some(artifact) = &message.pruned_artifact {
            if let Some(archive_ref) = &artifact.archive_ref {
                push_artifact(
                    &mut artifacts,
                    &mut seen,
                    archive_ref.storage_key.clone(),
                    archive_ref.title.clone(),
                    Some("text/plain".to_string()),
                    archive_ref.created_at,
                );
            }
        }
    }

    artifacts
}

fn push_artifact(
    artifacts: &mut Vec<ArtifactRef>,
    seen: &mut HashSet<String>,
    storage_key: String,
    description: String,
    content_type: Option<String>,
    created_at: i64,
) {
    if !seen.insert(storage_key.clone()) {
        return;
    }

    let Some(created_at) = chrono::DateTime::<Utc>::from_timestamp(created_at, 0) else {
        return;
    };

    artifacts.push(ArtifactRef {
        storage_key,
        description,
        content_type,
        source: Some("post_run_extract".to_string()),
        reason: None,
        tags: vec!["archive".to_string()],
        created_at,
    });
}

fn collect_tools_used(messages: &[AgentMessage]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut tools = Vec::new();

    for message in messages {
        let Some(tool_name) = message.tool_name.as_deref() else {
            continue;
        };
        let tool_name = tool_name.trim();
        if tool_name.is_empty() || !seen.insert(tool_name.to_string()) {
            continue;
        }
        tools.push(tool_name.to_string());
    }

    tools
}

#[cfg(test)]
mod tests {
    use super::{
        PersistentMemoryCoordinator, PersistentMemoryStore, PersistentRunContext,
        PersistentRunPhase,
    };
    use crate::agent::compaction::ArchiveRef;
    use crate::agent::memory::AgentMessage;
    use crate::agent::session::AgentMemoryScope;
    use oxide_agent_memory::{
        CleanupStatus, InMemoryMemoryRepository, MemoryListFilter, MemoryRepository, MemoryType,
    };
    use std::sync::Arc;

    #[tokio::test]
    async fn persist_completed_run_writes_episode_and_session_state() {
        let store = Arc::new(InMemoryMemoryRepository::new());
        let store_for_coordinator = Arc::clone(&store);
        let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
        let coordinator = PersistentMemoryCoordinator::new(store_for_coordinator);
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
                hot_token_estimate: 77,
                phase: PersistentRunPhase::Completed {
                    final_answer: "Done",
                },
            })
            .await
            .expect("post-run persistence should succeed");

        let episode = store
            .get_episode(&"episode-1".to_string())
            .await
            .expect("episode lookup should succeed")
            .expect("episode should exist");
        assert_eq!(episode.goal, "Implement Stage 4");
        assert_eq!(episode.tools_used, vec!["read_file".to_string()]);
        assert_eq!(episode.failures, vec!["Need follow-up test".to_string()]);
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

        let memories = store
            .list_memories("topic-a", &MemoryListFilter::default())
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
                hot_token_estimate: 21,
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
        assert!(store
            .get_episode(&"episode-1".to_string())
            .await
            .expect("episode lookup should succeed")
            .is_none());
        assert!(store
            .list_memories("topic-a", &MemoryListFilter::default())
            .await
            .expect("memory lookup should succeed")
            .is_empty());
    }

    #[tokio::test]
    async fn persist_post_run_keeps_topic_scopes_isolated() {
        let store = Arc::new(InMemoryMemoryRepository::new());
        let store_for_coordinator = Arc::clone(&store);
        let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
        let coordinator = PersistentMemoryCoordinator::new(store_for_coordinator);

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
                hot_token_estimate: 128,
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
                hot_token_estimate: 256,
                phase: PersistentRunPhase::Completed {
                    final_answer: "done",
                },
            })
            .await
            .expect("topic-b persistence should succeed");

        let topic_a_memories = store
            .list_memories("topic-a", &MemoryListFilter::default())
            .await
            .expect("topic-a memory lookup should succeed");
        let topic_b_memories = store
            .list_memories("topic-b", &MemoryListFilter::default())
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
        assert!(store
            .get_episode(&"episode-a".to_string())
            .await
            .expect("episode-a lookup should succeed")
            .is_some());
        assert!(store
            .get_episode(&"episode-b".to_string())
            .await
            .expect("episode-b lookup should succeed")
            .is_some());
    }
}
