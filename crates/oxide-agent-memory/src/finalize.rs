use crate::types::{
    ArtifactRef, CleanupStatus, EpisodeOutcome, EpisodeRecord, SessionStateRecord, ThreadRecord,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

const TITLE_MAX_CHARS: usize = 96;
const SHORT_SUMMARY_MAX_CHARS: usize = 220;

/// Generic inputs required to finalize a durable episode/thread/session-state write.
#[derive(Debug, Clone)]
pub struct EpisodeFinalizationInput {
    pub user_id: i64,
    pub context_key: String,
    pub flow_id: String,
    pub session_id: String,
    pub episode_id: String,
    pub goal: String,
    pub final_answer: Option<String>,
    pub compaction_summary: Option<String>,
    pub tools_used: Vec<String>,
    pub artifacts: Vec<ArtifactRef>,
    pub failures: Vec<String>,
    pub hot_token_estimate: usize,
    pub finalized_at: DateTime<Utc>,
}

/// Finalized record set ready to persist.
#[derive(Debug, Clone)]
pub struct EpisodeFinalizationPlan {
    pub thread: ThreadRecord,
    pub episode: Option<EpisodeRecord>,
    pub session_state: SessionStateRecord,
}

/// Deterministic record builder for Stage-4 durable write path.
#[derive(Debug, Clone, Default)]
pub struct EpisodeFinalizer;

impl EpisodeFinalizer {
    #[must_use]
    pub fn build_plan(&self, input: EpisodeFinalizationInput) -> EpisodeFinalizationPlan {
        let thread_id = thread_id_for_scope(input.user_id, &input.context_key, &input.flow_id);
        let thread = ThreadRecord {
            thread_id: thread_id.clone(),
            user_id: input.user_id,
            context_key: input.context_key.clone(),
            title: truncate_chars(&input.goal, TITLE_MAX_CHARS),
            short_summary: truncate_chars(
                &select_short_summary(
                    input.compaction_summary.as_deref(),
                    input.final_answer.as_deref(),
                    &input.goal,
                ),
                SHORT_SUMMARY_MAX_CHARS,
            ),
            created_at: input.finalized_at,
            updated_at: input.finalized_at,
            last_activity_at: input.finalized_at,
        };
        let has_artifacts = !input.artifacts.is_empty();
        let used_tools = !input.tools_used.is_empty();
        let has_failures = !input.failures.is_empty();

        let (episode, cleanup_status, pending_episode_id, last_finalized_at) =
            if let Some(final_answer) = input.final_answer.as_deref() {
                let summary =
                    compose_episode_summary(final_answer, input.compaction_summary.as_deref());
                let episode_tools = input.tools_used.clone();
                let episode_artifacts = input.artifacts.clone();
                let episode_failures = input.failures.clone();
                let outcome = if has_failures {
                    EpisodeOutcome::Partial
                } else {
                    EpisodeOutcome::Success
                };
                let episode = EpisodeRecord {
                    episode_id: input.episode_id.clone(),
                    thread_id,
                    context_key: input.context_key.clone(),
                    goal: input.goal.clone(),
                    summary,
                    outcome,
                    tools_used: episode_tools,
                    artifacts: episode_artifacts,
                    failures: episode_failures,
                    importance: estimate_importance(
                        input.compaction_summary.is_some(),
                        !final_answer.trim().is_empty(),
                        has_artifacts,
                        used_tools,
                        has_failures,
                    ),
                    created_at: input.finalized_at,
                };
                (
                    Some(episode),
                    CleanupStatus::Finalized,
                    None,
                    Some(input.finalized_at),
                )
            } else {
                (None, CleanupStatus::Idle, Some(input.episode_id), None)
            };

        let session_state = SessionStateRecord {
            session_id: input.session_id,
            context_key: input.context_key,
            hot_token_estimate: input.hot_token_estimate,
            last_compacted_at: Some(input.finalized_at),
            last_finalized_at,
            cleanup_status,
            pending_episode_id,
            updated_at: input.finalized_at,
        };

        EpisodeFinalizationPlan {
            thread,
            episode,
            session_state,
        }
    }
}

fn thread_id_for_scope(user_id: i64, context_key: &str, flow_id: &str) -> String {
    let scoped = format!("thread:{user_id}:{context_key}:{flow_id}");
    format!(
        "thread-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_URL, scoped.as_bytes())
    )
}

fn compose_episode_summary(final_answer: &str, compaction_summary: Option<&str>) -> String {
    let answer = final_answer.trim();
    match compaction_summary
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(summary) if !answer.is_empty() => {
            format!("Final answer:\n{answer}\n\nCompaction summary:\n{summary}")
        }
        Some(summary) => summary.to_string(),
        None => answer.to_string(),
    }
}

fn select_short_summary(
    compaction_summary: Option<&str>,
    final_answer: Option<&str>,
    goal: &str,
) -> String {
    compaction_summary
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            final_answer
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or(goal)
        .to_string()
}

fn estimate_importance(
    has_summary: bool,
    has_final_answer: bool,
    has_artifacts: bool,
    used_tools: bool,
    has_failures: bool,
) -> f32 {
    let mut importance: f32 = 0.45;
    if has_summary {
        importance += 0.15;
    }
    if has_final_answer {
        importance += 0.1;
    }
    if has_artifacts {
        importance += 0.15;
    }
    if used_tools {
        importance += 0.1;
    }
    if has_failures {
        importance += 0.1;
    }
    importance.min(1.0_f32)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.trim().chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::{EpisodeFinalizationInput, EpisodeFinalizer};
    use crate::types::{ArtifactRef, CleanupStatus, EpisodeOutcome};
    use chrono::{TimeZone, Utc};

    fn ts() -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0)
            .single()
            .expect("valid timestamp")
    }

    #[test]
    fn build_plan_creates_episode_for_completed_run() {
        let finalizer = EpisodeFinalizer;
        let artifact = ArtifactRef {
            storage_key: "archive/ref-1".to_string(),
            description: "Compaction archive".to_string(),
            content_type: Some("application/json".to_string()),
            created_at: ts(),
        };
        let plan = finalizer.build_plan(EpisodeFinalizationInput {
            user_id: 42,
            context_key: "topic-a".to_string(),
            flow_id: "flow-1".to_string(),
            session_id: "session-1".to_string(),
            episode_id: "episode-1".to_string(),
            goal: "Implement Stage 4".to_string(),
            final_answer: Some("Done".to_string()),
            compaction_summary: Some("Important summary".to_string()),
            tools_used: vec!["read_file".to_string()],
            artifacts: vec![artifact.clone()],
            failures: vec!["warning".to_string()],
            hot_token_estimate: 123,
            finalized_at: ts(),
        });

        let episode = plan.episode.expect("episode should exist");
        assert_eq!(episode.thread_id, plan.thread.thread_id);
        assert_eq!(episode.outcome, EpisodeOutcome::Partial);
        assert_eq!(episode.artifacts, vec![artifact]);
        assert!(episode.summary.contains("Final answer"));
        assert_eq!(plan.session_state.cleanup_status, CleanupStatus::Finalized);
        assert_eq!(plan.session_state.pending_episode_id, None);
        assert_eq!(plan.session_state.last_finalized_at, Some(ts()));
    }

    #[test]
    fn build_plan_leaves_episode_pending_while_waiting_for_user_input() {
        let finalizer = EpisodeFinalizer;
        let plan = finalizer.build_plan(EpisodeFinalizationInput {
            user_id: 42,
            context_key: "topic-a".to_string(),
            flow_id: "flow-1".to_string(),
            session_id: "session-1".to_string(),
            episode_id: "episode-1".to_string(),
            goal: "Need more input".to_string(),
            final_answer: None,
            compaction_summary: Some("Waiting summary".to_string()),
            tools_used: vec![],
            artifacts: vec![],
            failures: vec![],
            hot_token_estimate: 123,
            finalized_at: ts(),
        });

        assert!(plan.episode.is_none());
        assert_eq!(plan.session_state.cleanup_status, CleanupStatus::Idle);
        assert_eq!(
            plan.session_state.pending_episode_id.as_deref(),
            Some("episode-1")
        );
        assert_eq!(plan.session_state.last_finalized_at, None);
    }
}
