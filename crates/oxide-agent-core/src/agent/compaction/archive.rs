//! Archive extension points for future cold-context persistence.

use super::types::{
    AgentMessageKind, ArchivePersistenceOutcome, CompactionRetention, CompactionScope,
    CompactionSnapshot, CompactionSummary, CompactionTrigger,
};
use crate::agent::memory::AgentMessage;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

/// Archive kind for compacted history chunks displaced from hot memory.
pub const ARCHIVE_KIND_COMPACTED_HISTORY: &str = "compacted_history";

/// Archive kind for large externalized tool outputs.
pub const ARCHIVE_KIND_TOOL_OUTPUT: &str = "tool_output";

/// Reference to an archived context chunk persisted outside hot memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveRef {
    /// Stable archive identifier.
    pub archive_id: String,
    /// Unix timestamp when the archive record was created.
    pub created_at: i64,
    /// Short human-readable title used for future discovery.
    pub title: String,
    /// Storage key or object path holding the archived payload.
    pub storage_key: String,
}

/// Persisted archive metadata for future retrieval-oriented features.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveRecord {
    /// Stable archive identifier.
    pub archive_id: String,
    /// Scoped context key (topic/thread scope).
    pub context_key: String,
    /// Agent flow identifier.
    pub flow_id: String,
    /// Unix timestamp when the record was created.
    pub created_at: i64,
    /// Inclusive lower bound of the chunk time range.
    pub time_range_start: i64,
    /// Inclusive upper bound of the chunk time range.
    pub time_range_end: i64,
    /// Short title for future lookup.
    pub title: String,
    /// Short summary for future lookup.
    pub short_summary: String,
    /// Logical archive record kind.
    #[serde(default)]
    pub kind: String,
    /// Tool names associated with this archived chunk.
    #[serde(default)]
    pub tool_names: Vec<String>,
    /// File paths associated with this archived chunk.
    #[serde(default)]
    pub file_paths: Vec<String>,
    /// Storage key or payload reference for the archived content.
    #[serde(default)]
    pub payload_ref: String,
}

/// Archive write request carrying metadata plus optional persisted content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveChunk {
    /// Persisted archive metadata for the chunk.
    pub record: ArchiveRecord,
    /// MIME-like format description for the archived content.
    #[serde(default)]
    pub content_format: String,
    /// Archived content body.
    #[serde(default)]
    pub content: String,
}

impl ArchiveChunk {
    /// Build a metadata-only archive write request.
    #[must_use]
    pub fn metadata_only(record: ArchiveRecord) -> Self {
        Self {
            record,
            content_format: String::new(),
            content: String::new(),
        }
    }
}

/// Persistence sink for future compaction archive chunks.
pub trait ArchiveSink: Send + Sync {
    /// Persist an archive record and optionally return a stable reference.
    fn persist(&self, chunk: &ArchiveChunk) -> Result<Option<ArchiveRef>>;
}

/// Placeholder sink used until archive persistence is implemented.
#[derive(Debug, Default)]
pub struct NoopArchiveSink;

impl ArchiveSink for NoopArchiveSink {
    fn persist(&self, _chunk: &ArchiveChunk) -> Result<Option<ArchiveRef>> {
        Ok(None)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CompactedHistoryArchivePayload {
    trigger: CompactionTrigger,
    summary: CompactionSummary,
    messages: Vec<AgentMessage>,
}

/// Persist displaced compacted history as a future retrieval extension point.
#[must_use]
pub fn persist_compacted_history_chunk(
    scope: &CompactionScope,
    trigger: CompactionTrigger,
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
    summary: &CompactionSummary,
    archive_sink: &dyn ArchiveSink,
) -> ArchivePersistenceOutcome {
    let archived_messages = collect_archived_messages(snapshot, messages);
    if archived_messages.is_empty() {
        return ArchivePersistenceOutcome::default();
    }

    let created_at = current_unix_timestamp();
    let archive_id = uuid::Uuid::new_v4().to_string();
    let title = build_archive_title(summary);
    let storage_key = format!(
        "archive/{}/{}/history-{}.json",
        sanitize_key_component(&scope.context_key),
        sanitize_key_component(&scope.flow_id),
        archive_id
    );
    let record = ArchiveRecord {
        archive_id: archive_id.clone(),
        context_key: scope.context_key.clone(),
        flow_id: scope.flow_id.clone(),
        created_at,
        time_range_start: created_at,
        time_range_end: created_at,
        title: title.clone(),
        short_summary: build_archive_short_summary(summary, archived_messages.len()),
        kind: ARCHIVE_KIND_COMPACTED_HISTORY.to_string(),
        tool_names: collect_tool_names(&archived_messages),
        file_paths: collect_file_paths(summary),
        payload_ref: storage_key.clone(),
    };
    let payload = CompactedHistoryArchivePayload {
        trigger,
        summary: summary.clone(),
        messages: archived_messages,
    };
    let content = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
    let chunk = ArchiveChunk {
        record: record.clone(),
        content_format: "application/json".to_string(),
        content,
    };
    let archive_ref = persist_archive_chunk(archive_sink, &chunk, &storage_key);

    ArchivePersistenceOutcome {
        attempted: true,
        archived_chunk_count: 1,
        archived_message_count: payload.messages.len(),
        archive_refs: vec![archive_ref],
    }
}

fn collect_archived_messages(
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
) -> Vec<AgentMessage> {
    let has_displaced_history = snapshot.entries.iter().any(|entry| {
        !entry.preserve_in_raw_window
            && matches!(
                entry.retention,
                CompactionRetention::CompactableHistory | CompactionRetention::PrunableArtifact
            )
    });
    if !has_displaced_history {
        return Vec::new();
    }

    snapshot
        .entries
        .iter()
        .filter(|entry| {
            let displaced_history = !entry.preserve_in_raw_window
                && matches!(
                    entry.retention,
                    CompactionRetention::CompactableHistory | CompactionRetention::PrunableArtifact
                );
            let displaced_summary = entry.kind == AgentMessageKind::Summary;
            displaced_history || displaced_summary
        })
        .filter_map(|entry| messages.get(entry.index).cloned())
        .collect()
}

fn persist_archive_chunk(
    archive_sink: &dyn ArchiveSink,
    chunk: &ArchiveChunk,
    storage_key: &str,
) -> ArchiveRef {
    match archive_sink.persist(chunk) {
        Ok(Some(reference)) => reference,
        Ok(None) => ArchiveRef {
            archive_id: chunk.record.archive_id.clone(),
            created_at: chunk.record.created_at,
            title: chunk.record.title.clone(),
            storage_key: storage_key.to_string(),
        },
        Err(error) => {
            warn!(
                error = %error,
                archive_id = chunk.record.archive_id,
                "Archive chunk persistence failed, using local archive ref"
            );
            ArchiveRef {
                archive_id: chunk.record.archive_id.clone(),
                created_at: chunk.record.created_at,
                title: chunk.record.title.clone(),
                storage_key: storage_key.to_string(),
            }
        }
    }
}

fn build_archive_title(summary: &CompactionSummary) -> String {
    let goal = summary.goal.trim();
    if goal.is_empty() {
        return "Compacted history".to_string();
    }

    format!("Compacted history: {}", truncate_chars(goal, 80))
}

fn build_archive_short_summary(
    summary: &CompactionSummary,
    archived_message_count: usize,
) -> String {
    summary
        .remaining_work
        .iter()
        .chain(summary.decisions.iter())
        .chain(summary.discoveries.iter())
        .find_map(|item| {
            let trimmed = item.trim();
            (!trimmed.is_empty()).then(|| truncate_chars(trimmed, 160))
        })
        .unwrap_or_else(|| {
            format!("Archived {archived_message_count} displaced hot-memory messages")
        })
}

fn collect_tool_names(messages: &[AgentMessage]) -> Vec<String> {
    let mut tool_names = Vec::new();
    for tool_name in messages
        .iter()
        .filter_map(|message| message.tool_name.as_deref())
    {
        if tool_names.iter().any(|existing| existing == tool_name) {
            continue;
        }
        tool_names.push(tool_name.to_string());
    }
    tool_names
}

fn collect_file_paths(summary: &CompactionSummary) -> Vec<String> {
    let mut file_paths = Vec::new();
    for item in &summary.relevant_files_entities {
        let trimmed = item.trim();
        if trimmed.is_empty()
            || !looks_like_file_path(trimmed)
            || file_paths.iter().any(|existing| existing == trimmed)
        {
            continue;
        }
        file_paths.push(trimmed.to_string());
    }
    file_paths
}

fn looks_like_file_path(value: &str) -> bool {
    value.contains('/')
        || value.ends_with(".rs")
        || value.ends_with(".md")
        || value.ends_with(".toml")
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let mut truncated: String = value.chars().take(limit).collect();
    if value.chars().count() > limit {
        truncated.push_str("...");
    }
    truncated
}

fn sanitize_key_component(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            sanitized.push(ch);
        } else {
            sanitized.push('_');
        }
    }
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn current_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::{
        persist_compacted_history_chunk, ArchiveChunk, ArchiveRef, ArchiveSink,
        ARCHIVE_KIND_COMPACTED_HISTORY,
    };
    use crate::agent::compaction::{
        classify_hot_memory, CompactionScope, CompactionSummary, CompactionTrigger,
    };
    use crate::agent::memory::AgentMessage;
    use anyhow::Result;
    use std::sync::Mutex;

    #[test]
    fn persist_compacted_history_chunk_archives_displaced_messages() {
        let messages = vec![
            AgentMessage::topic_agents_md("# Topic AGENTS\nStay safe."),
            AgentMessage::user_task("Ship stage 10"),
            AgentMessage::user("Older request"),
            AgentMessage::assistant("Older response"),
            AgentMessage::user("Recent request 1"),
            AgentMessage::assistant("Recent response 1"),
            AgentMessage::user("Recent request 2"),
            AgentMessage::assistant("Recent response 2"),
        ];
        let summary = CompactionSummary {
            goal: "Ship stage 10".to_string(),
            relevant_files_entities: vec![
                "crates/oxide-agent-core/src/agent/compaction/archive.rs".to_string(),
            ],
            remaining_work: vec!["Verify sink extension points.".to_string()],
            ..CompactionSummary::default()
        };
        let snapshot = classify_hot_memory(&messages);
        let archive_sink = RecordingArchiveSink::default();

        let outcome = persist_compacted_history_chunk(
            &CompactionScope {
                context_key: "topic-1".to_string(),
                flow_id: "flow-a".to_string(),
            },
            CompactionTrigger::Manual,
            &snapshot,
            &messages,
            &summary,
            &archive_sink,
        );

        assert!(outcome.attempted);
        assert_eq!(outcome.archived_chunk_count, 1);
        assert_eq!(outcome.archived_message_count, 2);
        let chunks = archive_sink.chunks();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].record.kind, ARCHIVE_KIND_COMPACTED_HISTORY);
        assert_eq!(chunks[0].record.tool_names, Vec::<String>::new());
        assert_eq!(
            chunks[0].record.file_paths,
            vec!["crates/oxide-agent-core/src/agent/compaction/archive.rs".to_string()]
        );
        assert!(chunks[0].content.contains("Older request"));
        assert!(chunks[0].content.contains("Older response"));
    }

    #[test]
    fn persist_compacted_history_chunk_is_noop_without_displaced_history() {
        let messages = vec![
            AgentMessage::user("Recent request 1"),
            AgentMessage::assistant("Recent response 1"),
            AgentMessage::user("Recent request 2"),
            AgentMessage::assistant("Recent response 2"),
        ];
        let summary = CompactionSummary {
            goal: "No archive needed".to_string(),
            ..CompactionSummary::default()
        };
        let snapshot = classify_hot_memory(&messages);

        let outcome = persist_compacted_history_chunk(
            &CompactionScope::default(),
            CompactionTrigger::Manual,
            &snapshot,
            &messages,
            &summary,
            &super::NoopArchiveSink,
        );

        assert!(!outcome.attempted);
        assert!(outcome.archive_refs.is_empty());
    }

    #[test]
    fn persist_compacted_history_chunk_returns_serializable_archive_ref_with_noop_sink() {
        let messages = vec![
            AgentMessage::topic_agents_md("# Topic AGENTS\nStay safe."),
            AgentMessage::user_task("Ship stage 12"),
            AgentMessage::user("Older request"),
            AgentMessage::assistant("Older response"),
            AgentMessage::user("Recent request 1"),
            AgentMessage::assistant("Recent response 1"),
            AgentMessage::user("Recent request 2"),
            AgentMessage::assistant("Recent response 2"),
        ];
        let summary = CompactionSummary {
            goal: "Ship stage 12".to_string(),
            remaining_work: vec!["Validate archive ref serialization.".to_string()],
            ..CompactionSummary::default()
        };
        let snapshot = classify_hot_memory(&messages);

        let outcome = persist_compacted_history_chunk(
            &CompactionScope {
                context_key: "topic-2".to_string(),
                flow_id: "flow-b".to_string(),
            },
            CompactionTrigger::PreRun,
            &snapshot,
            &messages,
            &summary,
            &super::NoopArchiveSink,
        );

        assert!(outcome.attempted);
        let archive_ref = outcome
            .archive_refs
            .first()
            .cloned()
            .expect("archive ref fallback");
        assert!(archive_ref
            .storage_key
            .contains("archive/topic-2/flow-b/history-"));

        let message = AgentMessage::archive_reference_with_ref(
            "[archived context chunk]",
            Some(archive_ref.clone()),
        );
        let serialized = serde_json::to_string(&message).expect("serialize archive reference");
        let roundtrip: AgentMessage =
            serde_json::from_str(&serialized).expect("deserialize archive reference");

        assert_eq!(roundtrip.archive_ref_payload(), Some(&archive_ref));
    }

    #[derive(Debug, Default)]
    struct RecordingArchiveSink {
        chunks: Mutex<Vec<ArchiveChunk>>,
    }

    impl RecordingArchiveSink {
        fn chunks(&self) -> Vec<ArchiveChunk> {
            self.chunks.lock().expect("archive chunks lock").clone()
        }
    }

    impl ArchiveSink for RecordingArchiveSink {
        fn persist(&self, chunk: &ArchiveChunk) -> Result<Option<ArchiveRef>> {
            self.chunks
                .lock()
                .expect("archive chunks lock")
                .push(chunk.clone());
            Ok(Some(ArchiveRef {
                archive_id: chunk.record.archive_id.clone(),
                created_at: chunk.record.created_at,
                title: chunk.record.title.clone(),
                storage_key: chunk.record.payload_ref.clone(),
            }))
        }
    }
}
