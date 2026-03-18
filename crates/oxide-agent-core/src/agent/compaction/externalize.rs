//! Large payload externalization for Agent Mode hot memory.

use super::archive::{
    ArchiveChunk, ArchiveRecord, ArchiveRef, ArchiveSink, ARCHIVE_KIND_TOOL_OUTPUT,
};
use super::types::{
    CompactionPolicy, CompactionRetention, CompactionScope, CompactionSnapshot,
    ExternalizationOutcome,
};
use crate::agent::memory::{AgentMessage, ExternalizedPayload};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

/// Persisted payload record for externalized tool outputs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalizedPayloadRecord {
    /// Stable archive identifier linked to the hot-memory placeholder.
    pub archive_id: String,
    /// Storage key or object path for the raw payload.
    pub storage_key: String,
    /// Unix timestamp when the payload was externalized.
    pub created_at: i64,
    /// Scope key for the current session/topic.
    pub context_key: String,
    /// Flow identifier inside the current scope.
    pub flow_id: String,
    /// Tool name that produced the payload.
    pub tool_name: String,
    /// Human-readable archive title.
    pub title: String,
    /// Short summary for logs and future retrieval indexing.
    pub short_summary: String,
    /// Approximate token count of the original payload.
    pub estimated_tokens: usize,
    /// Character count of the original payload.
    pub original_chars: usize,
    /// Inline preview retained in hot memory.
    pub preview: String,
    /// Full raw payload that was removed from hot memory content.
    pub content: String,
}

/// Sink responsible for persisting large payloads out of band.
pub trait PayloadSink: Send + Sync {
    /// Persist the payload record.
    ///
    /// Returns `true` when the raw payload is safely stored outside hot memory.
    /// Returning `false` asks the caller to retain an inline hidden fallback.
    fn persist(&self, record: &ExternalizedPayloadRecord) -> Result<bool>;
}

/// Placeholder sink used when no external payload store is configured yet.
#[derive(Debug, Default)]
pub struct NoopPayloadSink;

impl PayloadSink for NoopPayloadSink {
    fn persist(&self, _record: &ExternalizedPayloadRecord) -> Result<bool> {
        Ok(false)
    }
}

/// Replace oversized tool outputs with lightweight preview placeholders.
#[must_use]
pub fn externalize_hot_memory(
    policy: &CompactionPolicy,
    scope: &CompactionScope,
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
    payload_sink: &dyn PayloadSink,
    archive_sink: &dyn ArchiveSink,
) -> (Vec<AgentMessage>, ExternalizationOutcome) {
    let mut rewritten = messages.to_vec();
    let mut outcome = ExternalizationOutcome::default();

    for entry in &snapshot.entries {
        let Some(externalized) =
            externalize_entry(policy, scope, entry, messages, payload_sink, archive_sink)
        else {
            continue;
        };

        rewritten[entry.index] = externalized.message;

        outcome.applied = true;
        outcome.externalized_count = outcome.externalized_count.saturating_add(1);
        outcome.reclaimed_tokens = outcome
            .reclaimed_tokens
            .saturating_add(externalized.reclaimed_tokens);
        outcome.reclaimed_chars = outcome
            .reclaimed_chars
            .saturating_add(externalized.reclaimed_chars);
        outcome.archive_refs.push(externalized.archive_ref);
    }

    if outcome.applied {
        warn!(
            externalized_count = outcome.externalized_count,
            reclaimed_tokens = outcome.reclaimed_tokens,
            reclaimed_chars = outcome.reclaimed_chars,
            archive_ref_count = outcome.archive_refs.len(),
            "Compaction externalized oversized tool payloads"
        );
    }

    (rewritten, outcome)
}

struct ExternalizedMessage {
    message: AgentMessage,
    archive_ref: ArchiveRef,
    reclaimed_tokens: usize,
    reclaimed_chars: usize,
}

fn externalize_entry(
    policy: &CompactionPolicy,
    scope: &CompactionScope,
    entry: &crate::agent::compaction::ClassifiedMemoryEntry,
    messages: &[AgentMessage],
    payload_sink: &dyn PayloadSink,
    archive_sink: &dyn ArchiveSink,
) -> Option<ExternalizedMessage> {
    if entry.retention != CompactionRetention::PrunableArtifact || entry.is_externalized {
        return None;
    }
    if entry.estimated_tokens < policy.externalize_threshold_tokens
        && entry.content_chars < policy.externalize_threshold_chars
    {
        return None;
    }

    let original = messages.get(entry.index)?;
    let tool_name = original.tool_name.as_deref()?;
    let tool_call_id = original.tool_call_id.as_deref()?;
    let artifact = build_artifact(policy, scope, entry, original, payload_sink, archive_sink)?;
    let placeholder = build_placeholder(
        tool_name,
        entry.content_chars,
        entry.estimated_tokens,
        &artifact.archive_ref,
        &artifact.preview,
    );
    let reclaimed_tokens = entry.estimated_tokens;
    let reclaimed_chars = entry.content_chars;
    let persisted_externally = artifact.inline_fallback.is_none();
    warn!(
        tool_name,
        message_index = entry.index,
        estimated_tokens = entry.estimated_tokens,
        original_chars = entry.content_chars,
        persisted_externally,
        archive_id = %artifact.archive_ref.archive_id,
        storage_key = %artifact.archive_ref.storage_key,
        "Compaction externalized tool payload"
    );

    Some(ExternalizedMessage {
        message: AgentMessage::externalized_tool(
            tool_call_id,
            tool_name,
            placeholder,
            ExternalizedPayload {
                archive_ref: artifact.archive_ref.clone(),
                estimated_tokens: entry.estimated_tokens,
                original_chars: entry.content_chars,
                preview: artifact.preview,
                inline_fallback: artifact.inline_fallback,
            },
        ),
        archive_ref: artifact.archive_ref,
        reclaimed_tokens,
        reclaimed_chars,
    })
}

struct PersistedArtifact {
    archive_ref: ArchiveRef,
    preview: String,
    inline_fallback: Option<String>,
}

fn build_artifact(
    policy: &CompactionPolicy,
    scope: &CompactionScope,
    entry: &crate::agent::compaction::ClassifiedMemoryEntry,
    original: &AgentMessage,
    payload_sink: &dyn PayloadSink,
    archive_sink: &dyn ArchiveSink,
) -> Option<PersistedArtifact> {
    let tool_name = original.tool_name.as_deref()?;
    let created_at = current_unix_timestamp();
    let archive_id = uuid::Uuid::new_v4().to_string();
    let title = format!("{tool_name} artifact");
    let storage_key = format!(
        "compaction/{}/{}/{}-{}.txt",
        sanitize_key_component(&scope.context_key),
        sanitize_key_component(&scope.flow_id),
        sanitize_key_component(tool_name),
        archive_id
    );
    let preview = build_preview(&original.content, policy.externalize_preview_chars);
    let short_summary = format!(
        "Externalized {} chars (~{} tokens) from tool {}",
        entry.content_chars, entry.estimated_tokens, tool_name
    );
    let payload_record = ExternalizedPayloadRecord {
        archive_id: archive_id.clone(),
        storage_key: storage_key.clone(),
        created_at,
        context_key: scope.context_key.clone(),
        flow_id: scope.flow_id.clone(),
        tool_name: tool_name.to_string(),
        title: title.clone(),
        short_summary: short_summary.clone(),
        estimated_tokens: entry.estimated_tokens,
        original_chars: entry.content_chars,
        preview: preview.clone(),
        content: original.content.clone(),
    };
    let persisted_externally =
        persist_payload(payload_sink, &payload_record, entry.index, tool_name)?;
    let archive_ref = persist_archive_metadata(
        archive_sink,
        ArchiveChunk::metadata_only(ArchiveRecord {
            archive_id,
            context_key: scope.context_key.clone(),
            flow_id: scope.flow_id.clone(),
            created_at,
            time_range_start: created_at,
            time_range_end: created_at,
            title,
            short_summary,
            kind: ARCHIVE_KIND_TOOL_OUTPUT.to_string(),
            tool_names: vec![tool_name.to_string()],
            file_paths: Vec::new(),
            payload_ref: storage_key.clone(),
        }),
        storage_key,
        entry.index,
        tool_name,
    );

    Some(PersistedArtifact {
        archive_ref,
        preview,
        inline_fallback: (!persisted_externally).then(|| original.content.clone()),
    })
}

fn persist_payload(
    payload_sink: &dyn PayloadSink,
    payload_record: &ExternalizedPayloadRecord,
    message_index: usize,
    tool_name: &str,
) -> Option<bool> {
    match payload_sink.persist(payload_record) {
        Ok(persisted) => Some(persisted),
        Err(error) => {
            warn!(
                tool_name,
                message_index,
                error = %error,
                "Skipping payload externalization after sink failure"
            );
            None
        }
    }
}

fn persist_archive_metadata(
    archive_sink: &dyn ArchiveSink,
    archive_chunk: ArchiveChunk,
    storage_key: String,
    message_index: usize,
    tool_name: &str,
) -> ArchiveRef {
    match archive_sink.persist(&archive_chunk) {
        Ok(Some(reference)) => reference,
        Ok(None) => ArchiveRef {
            archive_id: archive_chunk.record.archive_id,
            created_at: archive_chunk.record.created_at,
            title: archive_chunk.record.title,
            storage_key,
        },
        Err(error) => {
            warn!(
                tool_name,
                message_index,
                error = %error,
                "Archive metadata persistence failed, using local artifact ref"
            );
            ArchiveRef {
                archive_id: archive_chunk.record.archive_id,
                created_at: archive_chunk.record.created_at,
                title: archive_chunk.record.title,
                storage_key,
            }
        }
    }
}

fn build_preview(content: &str, preview_chars: usize) -> String {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return "(empty output)".to_string();
    }

    let mut preview: String = trimmed.chars().take(preview_chars).collect();
    if trimmed.chars().count() > preview_chars {
        preview.push_str("...");
    }
    preview
}

fn build_placeholder(
    tool_name: &str,
    original_chars: usize,
    estimated_tokens: usize,
    archive_ref: &ArchiveRef,
    preview: &str,
) -> String {
    format!(
        "[externalized tool result]\ntool: {tool_name}\nsize_chars: {original_chars}\nestimated_tokens: {estimated_tokens}\nartifact_id: {}\nstorage_key: {}\npreview:\n{}",
        archive_ref.archive_id, archive_ref.storage_key, preview
    )
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
    use super::{externalize_hot_memory, ExternalizedPayloadRecord, PayloadSink};
    use crate::agent::compaction::{
        classify_hot_memory, ArchiveChunk, ArchiveRef, ArchiveSink, CompactionPolicy,
        CompactionScope, NoopArchiveSink, NoopPayloadSink,
    };
    use crate::agent::memory::AgentMessage;
    use anyhow::Result;
    use std::sync::Mutex;

    #[test]
    fn externalize_hot_memory_rewrites_large_tool_outputs() {
        let policy = CompactionPolicy::default();
        let messages = vec![
            AgentMessage::user_task("Inspect artifact"),
            AgentMessage::tool("call-1", "read_file", &"A".repeat(5_000)),
            AgentMessage::assistant("Done"),
        ];
        let snapshot = classify_hot_memory(&messages);
        let payload_sink = RecordingPayloadSink::default();
        let archive_sink = RecordingArchiveSink::default();

        let (rewritten, outcome) = externalize_hot_memory(
            &policy,
            &CompactionScope {
                context_key: "topic-1".to_string(),
                flow_id: "flow-a".to_string(),
            },
            &snapshot,
            &messages,
            &payload_sink,
            &archive_sink,
        );

        assert!(outcome.applied);
        assert_eq!(outcome.externalized_count, 1);
        assert_eq!(payload_sink.records().len(), 1);
        assert_eq!(archive_sink.records().len(), 1);
        assert!(rewritten[1].is_externalized());
        assert!(rewritten[1].content.contains("[externalized tool result]"));
        assert!(rewritten[1].content.contains("storage_key:"));
        assert_eq!(
            rewritten[1]
                .externalized_payload
                .as_ref()
                .and_then(|payload| payload.inline_fallback.as_ref()),
            None
        );
    }

    #[test]
    fn noop_payload_sink_keeps_hidden_inline_fallback() {
        let policy = CompactionPolicy {
            externalize_threshold_chars: 16,
            externalize_threshold_tokens: 1,
            ..CompactionPolicy::default()
        };

        let messages = vec![AgentMessage::tool(
            "call-1",
            "execute_command",
            "01234567890123456789",
        )];
        let snapshot = classify_hot_memory(&messages);

        let (rewritten, outcome) = externalize_hot_memory(
            &policy,
            &CompactionScope::default(),
            &snapshot,
            &messages,
            &NoopPayloadSink,
            &NoopArchiveSink,
        );

        assert!(outcome.applied);
        assert_eq!(outcome.externalized_count, 1);
        assert_eq!(
            rewritten[0]
                .externalized_payload
                .as_ref()
                .and_then(|payload| payload.inline_fallback.as_deref()),
            Some("01234567890123456789")
        );
    }

    #[derive(Debug, Default)]
    struct RecordingPayloadSink {
        records: Mutex<Vec<ExternalizedPayloadRecord>>,
    }

    impl RecordingPayloadSink {
        fn records(&self) -> Vec<ExternalizedPayloadRecord> {
            self.records.lock().expect("payload records lock").clone()
        }
    }

    impl PayloadSink for RecordingPayloadSink {
        fn persist(&self, record: &ExternalizedPayloadRecord) -> Result<bool> {
            self.records
                .lock()
                .expect("payload records lock")
                .push(record.clone());
            Ok(true)
        }
    }

    #[derive(Debug, Default)]
    struct RecordingArchiveSink {
        records: Mutex<Vec<ArchiveChunk>>,
    }

    impl RecordingArchiveSink {
        fn records(&self) -> Vec<ArchiveChunk> {
            self.records.lock().expect("archive records lock").clone()
        }
    }

    impl ArchiveSink for RecordingArchiveSink {
        fn persist(&self, chunk: &ArchiveChunk) -> Result<Option<ArchiveRef>> {
            self.records
                .lock()
                .expect("archive records lock")
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
