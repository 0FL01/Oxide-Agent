use crate::agent::memory::AgentMessage;
use crate::agent::session::AgentMemoryScope;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use oxide_agent_memory::{
    ArtifactRef, EpisodeFinalizationInput, EpisodeFinalizer, MemoryRepository, RepositoryError,
    SessionStateRecord, ThreadRecord,
};
use std::collections::HashSet;
use std::sync::Arc;

/// Object-safe persistent-memory write surface used by the runner.
#[async_trait]
pub trait PersistentMemoryStore: Send + Sync {
    async fn upsert_thread(&self, record: ThreadRecord) -> Result<ThreadRecord, RepositoryError>;
    async fn create_episode(
        &self,
        record: oxide_agent_memory::EpisodeRecord,
    ) -> Result<oxide_agent_memory::EpisodeRecord, RepositoryError>;
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
}

impl PersistentMemoryCoordinator {
    #[must_use]
    pub fn new(store: Arc<dyn PersistentMemoryStore>) -> Self {
        Self {
            store,
            finalizer: EpisodeFinalizer,
        }
    }

    pub async fn persist_post_run(&self, ctx: PersistentRunContext<'_>) -> Result<()> {
        let (summary_text, failures) = latest_summary_and_failures(ctx.messages);
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
            compaction_summary: summary_text,
            tools_used,
            artifacts,
            failures,
            hot_token_estimate: ctx.hot_token_estimate,
            finalized_at: Utc::now(),
        });

        self.store.upsert_thread(plan.thread).await?;
        if let Some(episode) = plan.episode {
            self.store.create_episode(episode).await?;
        }
        self.store.upsert_session_state(plan.session_state).await?;
        Ok(())
    }
}

fn latest_summary_and_failures(messages: &[AgentMessage]) -> (Option<String>, Vec<String>) {
    let mut latest_summary = None;
    let mut failures = Vec::new();

    for message in messages.iter().rev() {
        let Some(summary) = message.summary_payload() else {
            continue;
        };

        latest_summary = Some(message.content.trim().to_string());
        failures = summary
            .risks
            .iter()
            .map(|risk| risk.trim())
            .filter(|risk| !risk.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        break;
    }

    (latest_summary, failures)
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
    use crate::agent::memory::AgentMessage;
    use crate::agent::session::AgentMemoryScope;
    use oxide_agent_memory::{CleanupStatus, InMemoryMemoryRepository, MemoryRepository};
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
            AgentMessage::from_compaction_summary(crate::agent::CompactionSummary {
                goal: "Implement Stage 4".to_string(),
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

        let session_state = store
            .get_session_state("session-1")
            .await
            .expect("session state lookup should succeed")
            .expect("session state should exist");
        assert_eq!(session_state.cleanup_status, CleanupStatus::Finalized);
        assert_eq!(session_state.pending_episode_id, None);
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
    }
}
