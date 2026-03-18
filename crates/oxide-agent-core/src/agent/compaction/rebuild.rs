//! Hot-context rebuild after pruning and summary compaction.

use super::archive::ArchiveRef;
use super::types::{
    AgentMessageKind, CompactionRetention, CompactionSnapshot, CompactionSummary, RebuildOutcome,
};
use crate::agent::memory::AgentMessage;

/// Rebuild hot memory into pinned, live, structured-summary, and recent-raw slices.
#[must_use]
pub fn rebuild_hot_context(
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
    new_summary: Option<CompactionSummary>,
    archive_ref: Option<ArchiveRef>,
) -> (Vec<AgentMessage>, RebuildOutcome) {
    let existing_structured_summaries: Vec<CompactionSummary> = snapshot
        .entries
        .iter()
        .filter(|entry| entry.kind == AgentMessageKind::Summary)
        .filter_map(|entry| messages.get(entry.index))
        .filter_map(|message| message.summary_payload().cloned())
        .collect();
    let summary = merge_summaries(existing_structured_summaries, new_summary);
    let has_old_history = snapshot.entries.iter().any(|entry| {
        !entry.preserve_in_raw_window
            && matches!(
                entry.retention,
                CompactionRetention::CompactableHistory | CompactionRetention::PrunableArtifact
            )
    });
    let has_existing_structured_summary = snapshot.entries.iter().any(|entry| {
        entry.kind == AgentMessageKind::Summary
            && messages
                .get(entry.index)
                .and_then(|message| message.summary_payload())
                .is_some()
    });

    if !has_old_history || summary.is_none() {
        return (messages.to_vec(), RebuildOutcome::default());
    }

    let mut rebuilt = Vec::new();
    let mut preserved = vec![false; messages.len()];
    let mut outcome = RebuildOutcome {
        inserted_summary: true,
        preserved_recent_indices: snapshot
            .entries
            .iter()
            .filter(|entry| entry.preserve_in_raw_window)
            .map(|entry| entry.index)
            .collect(),
        ..RebuildOutcome::default()
    };

    append_matching_messages(
        &mut rebuilt,
        &mut preserved,
        snapshot,
        messages,
        |entry, message| {
            entry.retention == CompactionRetention::Pinned
                && !(entry.kind == AgentMessageKind::Summary && message.summary_payload().is_some())
        },
    );
    append_matching_messages(
        &mut rebuilt,
        &mut preserved,
        snapshot,
        messages,
        |entry, _message| entry.retention == CompactionRetention::ProtectedLive,
    );
    rebuilt.push(AgentMessage::from_compaction_summary(
        summary.expect("summary presence already checked"),
    ));
    if let Some(archive_ref) = archive_ref {
        outcome.inserted_archive_reference = true;
        rebuilt.push(AgentMessage::archive_reference_with_ref(
            format_archive_reference(&archive_ref),
            Some(archive_ref),
        ));
    }
    append_matching_messages(
        &mut rebuilt,
        &mut preserved,
        snapshot,
        messages,
        |entry, _message| entry.preserve_in_raw_window,
    );

    outcome.dropped_indices = preserved
        .iter()
        .enumerate()
        .filter_map(|(index, is_preserved)| (!is_preserved).then_some(index))
        .collect();
    outcome.dropped_message_count = outcome.dropped_indices.len();
    outcome.applied = outcome.dropped_message_count > 0 || has_existing_structured_summary;

    (rebuilt, outcome)
}

fn format_archive_reference(archive_ref: &ArchiveRef) -> String {
    format!(
        "[archived context chunk]\narchive_id: {}\ntitle: {}\nstorage_key: {}",
        archive_ref.archive_id, archive_ref.title, archive_ref.storage_key
    )
}

fn append_matching_messages(
    rebuilt: &mut Vec<AgentMessage>,
    preserved: &mut [bool],
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
    predicate: impl Fn(&super::types::ClassifiedMemoryEntry, &AgentMessage) -> bool,
) {
    for entry in &snapshot.entries {
        if preserved.get(entry.index).copied().unwrap_or(false) {
            continue;
        }
        let Some(message) = messages.get(entry.index) else {
            continue;
        };
        if !predicate(entry, message) {
            continue;
        }
        rebuilt.push(message.clone());
        preserved[entry.index] = true;
    }
}

fn merge_summaries(
    existing_summaries: Vec<CompactionSummary>,
    new_summary: Option<CompactionSummary>,
) -> Option<CompactionSummary> {
    let mut summaries: Vec<CompactionSummary> = existing_summaries
        .into_iter()
        .filter(summary_has_signal)
        .collect();
    if let Some(summary) = new_summary.filter(summary_has_signal) {
        summaries.push(summary);
    }
    if summaries.is_empty() {
        return None;
    }

    let mut merged = CompactionSummary {
        goal: summaries
            .iter()
            .rev()
            .find_map(|summary| {
                let goal = summary.goal.trim();
                (!goal.is_empty()).then(|| goal.to_string())
            })
            .unwrap_or_default(),
        ..CompactionSummary::default()
    };
    for summary in summaries {
        push_unique(&mut merged.constraints, summary.constraints);
        push_unique(&mut merged.decisions, summary.decisions);
        push_unique(&mut merged.discoveries, summary.discoveries);
        push_unique(
            &mut merged.relevant_files_entities,
            summary.relevant_files_entities,
        );
        push_unique(&mut merged.remaining_work, summary.remaining_work);
        push_unique(&mut merged.risks, summary.risks);
    }

    summary_has_signal(&merged).then_some(merged)
}

fn summary_has_signal(summary: &CompactionSummary) -> bool {
    !summary.goal.trim().is_empty()
        || !summary.constraints.is_empty()
        || !summary.decisions.is_empty()
        || !summary.discoveries.is_empty()
        || !summary.relevant_files_entities.is_empty()
        || !summary.remaining_work.is_empty()
        || !summary.risks.is_empty()
}

fn push_unique(target: &mut Vec<String>, items: Vec<String>) {
    for item in items {
        let trimmed = item.trim();
        if trimmed.is_empty() || target.iter().any(|existing| existing == trimmed) {
            continue;
        }
        target.push(trimmed.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::rebuild_hot_context;
    use crate::agent::compaction::{classify_hot_memory, CompactionSummary};
    use crate::agent::memory::AgentMessage;

    #[test]
    fn rebuild_hot_context_inserts_structured_summary_before_recent_raw_window() {
        let messages = vec![
            AgentMessage::topic_agents_md("# Topic AGENTS\nPreserve identity."),
            AgentMessage::user_task("Ship stage 8"),
            AgentMessage::user("Older request"),
            AgentMessage::assistant("Older response"),
            AgentMessage::user("Recent request 1"),
            AgentMessage::assistant("Recent response 1"),
            AgentMessage::user("Recent request 2"),
            AgentMessage::assistant("Recent response 2"),
        ];
        let snapshot = classify_hot_memory(&messages);
        let summary = CompactionSummary {
            goal: "Ship stage 8".to_string(),
            decisions: vec!["Rebuild hot context around a structured summary.".to_string()],
            remaining_work: vec!["Wire runner lifecycle next.".to_string()],
            ..CompactionSummary::default()
        };

        let (rebuilt, outcome) =
            rebuild_hot_context(&snapshot, &messages, Some(summary.clone()), None);

        assert!(outcome.applied);
        assert!(outcome.inserted_summary);
        assert_eq!(outcome.dropped_indices, vec![2, 3]);
        assert_eq!(outcome.preserved_recent_indices, vec![4, 5, 6, 7]);
        assert_eq!(rebuilt.len(), 7);
        assert_eq!(
            rebuilt[0].resolved_kind(),
            crate::agent::AgentMessageKind::TopicAgentsMd
        );
        assert_eq!(
            rebuilt[1].resolved_kind(),
            crate::agent::AgentMessageKind::UserTask
        );
        assert_eq!(
            rebuilt[2].resolved_kind(),
            crate::agent::AgentMessageKind::Summary
        );
        assert_eq!(rebuilt[2].summary_payload(), Some(&summary));
        assert_eq!(rebuilt[3].content, "Recent request 1");
        assert_eq!(rebuilt[6].content, "Recent response 2");
    }

    #[test]
    fn rebuild_hot_context_merges_existing_structured_summary() {
        let existing = CompactionSummary {
            goal: "Ship stage 7".to_string(),
            constraints: vec!["Keep AGENTS.md pinned.".to_string()],
            discoveries: vec!["Compaction moved out of memory.rs.".to_string()],
            ..CompactionSummary::default()
        };
        let messages = vec![
            AgentMessage::from_compaction_summary(existing.clone()),
            AgentMessage::user_task("Ship stage 8"),
            AgentMessage::user("Older request"),
            AgentMessage::assistant("Older response"),
            AgentMessage::user("Recent request 1"),
            AgentMessage::assistant("Recent response 1"),
            AgentMessage::user("Recent request 2"),
            AgentMessage::assistant("Recent response 2"),
        ];
        let snapshot = classify_hot_memory(&messages);
        let new_summary = CompactionSummary {
            goal: "Ship stage 8".to_string(),
            decisions: vec!["Use a first-class summary entry.".to_string()],
            remaining_work: vec!["Integrate pre-iteration compaction.".to_string()],
            ..CompactionSummary::default()
        };

        let (rebuilt, outcome) =
            rebuild_hot_context(&snapshot, &messages, Some(new_summary.clone()), None);

        assert!(outcome.applied);
        assert_eq!(outcome.dropped_indices, vec![0, 2, 3]);
        let merged_summary = rebuilt[1].summary_payload().expect("merged summary");
        assert_eq!(merged_summary.goal, "Ship stage 8");
        assert!(merged_summary
            .constraints
            .iter()
            .any(|item| item.contains("AGENTS.md")));
        assert!(merged_summary
            .discoveries
            .iter()
            .any(|item| item.contains("memory.rs")));
        assert!(merged_summary
            .decisions
            .iter()
            .any(|item| item.contains("first-class summary")));
    }

    #[test]
    fn rebuild_hot_context_is_noop_without_old_history() {
        let messages = vec![
            AgentMessage::user("Recent request 1"),
            AgentMessage::assistant("Recent response 1"),
            AgentMessage::user("Recent request 2"),
            AgentMessage::assistant("Recent response 2"),
        ];
        let snapshot = classify_hot_memory(&messages);

        let (rebuilt, outcome) = rebuild_hot_context(
            &snapshot,
            &messages,
            Some(CompactionSummary {
                goal: "Ship stage 8".to_string(),
                ..CompactionSummary::default()
            }),
            None,
        );

        assert!(!outcome.applied);
        assert_eq!(rebuilt.len(), messages.len());
        assert_eq!(rebuilt[0].content, messages[0].content);
        assert_eq!(rebuilt[3].content, messages[3].content);
    }

    #[test]
    fn rebuild_hot_context_inserts_archive_reference_after_summary() {
        let messages = vec![
            AgentMessage::user_task("Ship stage 10"),
            AgentMessage::user("Older request"),
            AgentMessage::assistant("Older response"),
            AgentMessage::user("Recent request 1"),
            AgentMessage::assistant("Recent response 1"),
            AgentMessage::user("Recent request 2"),
            AgentMessage::assistant("Recent response 2"),
        ];
        let snapshot = classify_hot_memory(&messages);
        let archive_ref = crate::agent::ArchiveRef {
            archive_id: "archive-1".to_string(),
            created_at: 1,
            title: "Compacted history: Ship stage 10".to_string(),
            storage_key: "archive/topic/flow/history-archive-1.json".to_string(),
        };

        let (rebuilt, outcome) = rebuild_hot_context(
            &snapshot,
            &messages,
            Some(CompactionSummary {
                goal: "Ship stage 10".to_string(),
                ..CompactionSummary::default()
            }),
            Some(archive_ref.clone()),
        );

        assert!(outcome.applied);
        assert!(outcome.inserted_archive_reference);
        assert_eq!(
            rebuilt[1].resolved_kind(),
            crate::agent::AgentMessageKind::Summary
        );
        assert_eq!(rebuilt[2].archive_ref_payload(), Some(&archive_ref));
        assert!(rebuilt[2].content.contains("[archived context chunk]"));
    }
}
