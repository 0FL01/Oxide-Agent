//! Pruning of old tool artifacts after protecting the recent raw window.

use super::budget::estimate_message_tokens;
use super::types::{
    ClassifiedMemoryEntry, CompactionPolicy, CompactionRetention, CompactionSnapshot, PruneOutcome,
};
use crate::agent::memory::{AgentMessage, PrunedArtifact};

/// Prune old tool artifacts while preserving the recent working set.
#[must_use]
pub fn prune_hot_memory(
    policy: &CompactionPolicy,
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
) -> (Vec<AgentMessage>, PruneOutcome) {
    let mut rewritten = messages.to_vec();
    let mut outcome = PruneOutcome::default();

    for entry in &snapshot.entries {
        let Some(pruned) = prune_entry(policy, entry, messages) else {
            continue;
        };

        rewritten[entry.index] = pruned.message;
        outcome.applied = true;
        outcome.pruned_count = outcome.pruned_count.saturating_add(1);
        outcome.reclaimed_tokens = outcome
            .reclaimed_tokens
            .saturating_add(pruned.reclaimed_tokens);
        outcome.reclaimed_chars = outcome
            .reclaimed_chars
            .saturating_add(pruned.reclaimed_chars);
        outcome.pruned_indices.push(entry.index);
    }

    (rewritten, outcome)
}

struct PrunedMessage {
    message: AgentMessage,
    reclaimed_tokens: usize,
    reclaimed_chars: usize,
}

fn prune_entry(
    policy: &CompactionPolicy,
    entry: &ClassifiedMemoryEntry,
    messages: &[AgentMessage],
) -> Option<PrunedMessage> {
    if entry.retention != CompactionRetention::PrunableArtifact
        || entry.preserve_in_raw_window
        || entry.is_pruned
    {
        return None;
    }
    if entry.estimated_tokens < policy.prune_min_tokens
        && entry.content_chars < policy.prune_min_chars
    {
        return None;
    }

    let original = messages.get(entry.index)?;
    let tool_name = original.tool_name.as_deref()?;
    let tool_call_id = original.tool_call_id.as_deref()?;
    let preview = original
        .pruned_artifact
        .as_ref()
        .map(|artifact| artifact.preview.clone())
        .or_else(|| {
            original
                .externalized_payload
                .as_ref()
                .map(|artifact| artifact.preview.clone())
        })
        .unwrap_or_else(|| build_preview(&original.content, policy.prune_preview_chars));
    let archive_ref = original
        .externalized_payload
        .as_ref()
        .map(|artifact| artifact.archive_ref.clone());
    let placeholder = build_placeholder(
        tool_name,
        entry.content_chars,
        entry.estimated_tokens,
        archive_ref.as_ref(),
        &preview,
    );
    let replacement = AgentMessage::pruned_tool(
        tool_call_id,
        tool_name,
        placeholder,
        PrunedArtifact {
            estimated_tokens: entry.estimated_tokens,
            original_chars: entry.content_chars,
            preview,
            archive_ref,
        },
        original.externalized_payload.clone(),
    );
    let reclaimed_tokens = entry
        .estimated_tokens
        .saturating_sub(estimate_message_tokens(&replacement));
    let reclaimed_chars = entry
        .content_chars
        .saturating_sub(replacement.content.chars().count());

    Some(PrunedMessage {
        message: replacement,
        reclaimed_tokens,
        reclaimed_chars,
    })
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
    archive_ref: Option<&crate::agent::compaction::ArchiveRef>,
    preview: &str,
) -> String {
    let artifact_line = archive_ref.map_or(String::new(), |reference| {
        format!(
            "\nartifact_id: {}\nstorage_key: {}",
            reference.archive_id, reference.storage_key
        )
    });
    format!(
        "[pruned tool result]\ntool: {tool_name}\nsize_chars: {original_chars}\nestimated_tokens: {estimated_tokens}{artifact_line}\npreview:\n{preview}"
    )
}

#[cfg(test)]
mod tests {
    use super::prune_hot_memory;
    use crate::agent::compaction::{classify_hot_memory, CompactionPolicy};
    use crate::agent::memory::AgentMessage;

    #[test]
    fn prune_hot_memory_rewrites_old_tool_artifacts() {
        let policy = CompactionPolicy {
            prune_min_tokens: 1,
            prune_min_chars: 16,
            ..CompactionPolicy::default()
        };
        let messages = vec![
            AgentMessage::tool("call-1", "search", &"A".repeat(80)),
            AgentMessage::tool("call-2", "search", "recent-1"),
            AgentMessage::tool("call-3", "search", "recent-2"),
            AgentMessage::tool("call-4", "search", "recent-3"),
            AgentMessage::tool("call-5", "search", "recent-4"),
        ];

        let snapshot = classify_hot_memory(&messages);
        let (rewritten, outcome) = prune_hot_memory(&policy, &snapshot, &messages);

        assert!(outcome.applied);
        assert_eq!(outcome.pruned_indices, vec![0]);
        assert!(rewritten[0].is_pruned());
        assert!(rewritten[0].content.contains("[pruned tool result]"));
        assert!(!rewritten[1].is_pruned());
        assert!(!rewritten[4].is_pruned());
    }

    #[test]
    fn prune_hot_memory_protects_recent_tool_window() {
        let policy = CompactionPolicy {
            prune_min_tokens: 1,
            prune_min_chars: 8,
            ..CompactionPolicy::default()
        };
        let messages = vec![
            AgentMessage::tool("call-1", "read_file", "old-but-only-one"),
            AgentMessage::tool("call-2", "read_file", "recent"),
            AgentMessage::tool("call-3", "read_file", "recent"),
            AgentMessage::tool("call-4", "read_file", "recent"),
        ];

        let snapshot = classify_hot_memory(&messages);
        let (rewritten, outcome) = prune_hot_memory(&policy, &snapshot, &messages);

        assert!(!outcome.applied);
        assert_eq!(rewritten.len(), messages.len());
        assert_eq!(rewritten[0].content, messages[0].content);
        assert!(!rewritten[0].is_pruned());
    }
}
