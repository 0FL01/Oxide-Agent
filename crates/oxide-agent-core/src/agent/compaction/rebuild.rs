//! Hot-context rebuild after pruning and summary compaction.

use super::archive::ArchiveRef;
use super::summary::{
    collect_existing_structured_summaries, merge_summaries_bounded, normalize_summary,
};
use super::types::{
    AgentMessageKind, CompactionRetention, CompactionSnapshot, CompactionSummary, RebuildOutcome,
};
use crate::agent::memory::AgentMessage;
use crate::llm::InvocationId;
use tracing::warn;

/// Aggressive PostRun truncate: replace entire hot context with summary + archive ref.
///
/// Unlike [`rebuild_hot_context`] which preserves Pinned/ProtectedLive/recent-window entries,
/// this function wipes the message list clean and inserts only the session summary and an
/// optional archive reference. System prompt, tools schema, AGENTS.md, and memory retrieval
/// context are all re-injected by `prepare_execution()` on the next task, so they do not
/// need to survive the truncation.
#[must_use]
pub fn truncate_to_summary(
    messages: &[AgentMessage],
    summary: Option<CompactionSummary>,
    archive_ref: Option<ArchiveRef>,
) -> (Vec<AgentMessage>, RebuildOutcome) {
    let summary = match summary.and_then(normalize_summary) {
        Some(s) => s,
        None => return (messages.to_vec(), RebuildOutcome::default()),
    };

    let total_before = messages.len();
    let mut rebuilt = Vec::with_capacity(2);

    rebuilt.push(AgentMessage::from_compaction_summary(summary));

    let inserted_archive_reference = if let Some(archive_ref) = archive_ref {
        rebuilt.push(AgentMessage::archive_reference_with_ref(
            format_archive_reference(&archive_ref),
            Some(archive_ref),
        ));
        true
    } else {
        false
    };

    let outcome = RebuildOutcome {
        applied: true,
        inserted_summary: true,
        inserted_archive_reference,
        dropped_message_count: total_before.saturating_sub(rebuilt.len()),
        dropped_indices: (0..total_before).collect(),
        preserved_recent_indices: Vec::new(),
    };

    warn!(
        inserted_summary = outcome.inserted_summary,
        inserted_archive_reference = outcome.inserted_archive_reference,
        dropped_message_count = outcome.dropped_message_count,
        messages_before = total_before,
        messages_after = rebuilt.len(),
        "PostRun truncate-to-summary: hot context replaced with summary"
    );

    (rebuilt, outcome)
}

/// Rebuild hot memory into pinned, live, structured-summary, and recent-raw slices.
#[must_use]
pub fn rebuild_hot_context(
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
    new_summary: Option<CompactionSummary>,
    archive_ref: Option<ArchiveRef>,
) -> (Vec<AgentMessage>, RebuildOutcome) {
    let existing_structured_summaries = collect_existing_structured_summaries(snapshot, messages);
    let summary = new_summary
        .and_then(normalize_summary)
        .or_else(|| merge_summaries_bounded(existing_structured_summaries, None));
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

    // FIX: Remove orphaned tool results whose corresponding tool_calls were dropped by compaction.
    remove_orphaned_tool_results(snapshot, messages, &mut preserved, &mut rebuilt);

    outcome.dropped_indices = preserved
        .iter()
        .enumerate()
        .filter_map(|(index, is_preserved)| (!is_preserved).then_some(index))
        .collect();
    outcome.dropped_message_count = outcome.dropped_indices.len();
    outcome.applied = outcome.dropped_message_count > 0 || has_existing_structured_summary;

    if outcome.applied {
        warn!(
            inserted_summary = outcome.inserted_summary,
            inserted_archive_reference = outcome.inserted_archive_reference,
            dropped_message_count = outcome.dropped_message_count,
            dropped_indices = ?outcome.dropped_indices,
            preserved_recent_count = outcome.preserved_recent_indices.len(),
            preserved_recent_indices = ?outcome.preserved_recent_indices,
            "Compaction rebuilt hot context"
        );
    }

    (rebuilt, outcome)
}

fn format_archive_reference(archive_ref: &ArchiveRef) -> String {
    format!(
        "[archived context chunk]\narchive_id: {}\ntitle: {}\nstorage_key: {}",
        archive_ref.archive_id, archive_ref.title, archive_ref.storage_key
    )
}

/// Remove orphaned tool results whose corresponding tool_calls were dropped by compaction.
///
/// When compaction removes an assistant message with tool_calls, we must also remove
/// the tool result messages that reference those tool_call_ids. Otherwise, LLM providers
/// like MiniMax will receive tool results referencing non-existent tool_uses, causing
/// "tool id not found" errors (error 2013).
fn remove_orphaned_tool_results(
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
    preserved: &mut [bool],
    rebuilt: &mut Vec<AgentMessage>,
) {
    let dropped_tool_call_ids: std::collections::HashSet<InvocationId> = snapshot
        .entries
        .iter()
        .filter(|entry| !preserved.get(entry.index).copied().unwrap_or(true))
        .filter(|entry| entry.kind == AgentMessageKind::AssistantToolCall)
        .filter_map(|entry| messages.get(entry.index))
        .filter_map(|msg| msg.resolved_tool_call_correlations())
        .flat_map(|correlations| {
            correlations
                .into_iter()
                .map(|correlation| correlation.invocation_id)
        })
        .collect();

    if !dropped_tool_call_ids.is_empty() {
        // Mark tool results referencing dropped tool_calls as not preserved
        for entry in &snapshot.entries {
            if entry.kind == AgentMessageKind::ToolResult {
                if let Some(msg) = messages.get(entry.index) {
                    if let Some(invocation_id) = msg
                        .resolved_tool_call_correlation()
                        .map(|correlation| correlation.invocation_id)
                    {
                        if dropped_tool_call_ids.contains(&invocation_id) {
                            if let Some(preserved_flag) = preserved.get_mut(entry.index) {
                                *preserved_flag = false;
                                warn!(
                                    invocation_id = %invocation_id,
                                    message_index = entry.index,
                                    "Removing orphaned tool result - corresponding tool_call was compacted"
                                );
                            }
                        }
                    }
                }
            }
        }

        // Remove orphaned tool results from rebuilt vector
        rebuilt.retain(|msg| {
            if msg.kind == AgentMessageKind::ToolResult {
                if let Some(invocation_id) = msg
                    .resolved_tool_call_correlation()
                    .map(|correlation| correlation.invocation_id)
                {
                    !dropped_tool_call_ids.contains(&invocation_id)
                } else {
                    true
                }
            } else {
                true
            }
        });
    }
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

#[cfg(test)]
mod tests {
    use super::{rebuild_hot_context, remove_orphaned_tool_results, truncate_to_summary};
    use crate::agent::compaction::{
        classify_hot_memory, AgentMessageKind, ClassifiedMemoryEntry, CompactionRetention,
        CompactionSnapshot, CompactionSummary,
    };
    use crate::agent::memory::AgentMessage;
    use crate::llm::{ToolCall, ToolCallCorrelation, ToolCallFunction};

    fn tool_call(id: &str, name: &str) -> ToolCall {
        ToolCall::new(
            id.to_string(),
            ToolCallFunction {
                name: name.to_string(),
                arguments: "{}".to_string(),
            },
            false,
        )
    }

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
    fn rebuild_hot_context_replaces_existing_structured_summary_with_authoritative_update() {
        let existing = CompactionSummary {
            goal: "Ship stage 7".to_string(),
            constraints: vec!["Keep AGENTS.md pinned.".to_string()],
            discoveries: vec!["Compaction moved out of memory.rs.".to_string()],
            risks: vec!["Old risk".to_string()],
            ..CompactionSummary::default()
        };
        let messages = vec![
            AgentMessage::from_compaction_summary(existing),
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

        let (rebuilt, outcome) = rebuild_hot_context(&snapshot, &messages, Some(new_summary), None);

        assert!(outcome.applied);
        assert_eq!(outcome.dropped_indices, vec![0, 2, 3]);
        let summary = rebuilt[1].summary_payload().expect("replacement summary");
        assert_eq!(summary.goal, "Ship stage 8");
        assert_eq!(
            summary.decisions,
            vec!["Use a first-class summary entry.".to_string()]
        );
        assert_eq!(
            summary.remaining_work,
            vec!["Integrate pre-iteration compaction.".to_string()]
        );
        assert!(summary.constraints.is_empty());
        assert!(summary.discoveries.is_empty());
        assert!(summary.risks.is_empty());
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

    #[test]
    fn remove_orphaned_tool_results_drops_results_for_compacted_tool_batch() {
        let messages = vec![
            AgentMessage::assistant_with_tools(
                "Calling tools",
                vec![tool_call("call-1", "search")],
            ),
            AgentMessage::tool("call-1", "search", "result-1"),
            AgentMessage::user("Recent request"),
        ];
        let snapshot = CompactionSnapshot {
            entries: vec![
                ClassifiedMemoryEntry {
                    index: 0,
                    kind: AgentMessageKind::AssistantToolCall,
                    retention: CompactionRetention::CompactableHistory,
                    estimated_tokens: 10,
                    content_chars: messages[0].content.len(),
                    has_reasoning: false,
                    tool_name: None,
                    is_externalized: false,
                    archive_ref: None,
                    is_pruned: false,
                    preserve_in_raw_window: false,
                },
                ClassifiedMemoryEntry {
                    index: 1,
                    kind: AgentMessageKind::ToolResult,
                    retention: CompactionRetention::PrunableArtifact,
                    estimated_tokens: 10,
                    content_chars: messages[1].content.len(),
                    has_reasoning: false,
                    tool_name: Some("search".to_string()),
                    is_externalized: false,
                    archive_ref: None,
                    is_pruned: false,
                    preserve_in_raw_window: true,
                },
                ClassifiedMemoryEntry {
                    index: 2,
                    kind: AgentMessageKind::UserTurn,
                    retention: CompactionRetention::CompactableHistory,
                    estimated_tokens: 10,
                    content_chars: messages[2].content.len(),
                    has_reasoning: false,
                    tool_name: None,
                    is_externalized: false,
                    archive_ref: None,
                    is_pruned: false,
                    preserve_in_raw_window: true,
                },
            ],
            ..CompactionSnapshot::default()
        };
        let mut preserved = vec![false, true, true];
        let mut rebuilt = vec![messages[1].clone(), messages[2].clone()];

        remove_orphaned_tool_results(&snapshot, &messages, &mut preserved, &mut rebuilt);

        assert_eq!(preserved, vec![false, false, true]);
        assert_eq!(rebuilt.len(), 1);
        assert_eq!(rebuilt[0].content, "Recent request");
    }

    #[test]
    fn remove_orphaned_tool_results_matches_on_invocation_id_not_raw_wire_id() {
        let correlation =
            ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("provider-call-1");
        let messages = vec![
            AgentMessage {
                kind: AgentMessageKind::AssistantToolCall,
                role: crate::agent::memory::MessageRole::Assistant,
                content: "Calling tools".to_string(),
                reasoning: None,
                tool_call_id: None,
                tool_call_correlation: None,
                tool_name: None,
                tool_calls: Some(vec![ToolCall::new(
                    "provider-a".to_string(),
                    ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                    false,
                )]),
                tool_call_correlations: Some(vec![correlation.clone()]),
                externalized_payload: None,
                pruned_artifact: None,
                structured_summary: None,
                archive_ref: None,
            },
            AgentMessage {
                kind: AgentMessageKind::ToolResult,
                role: crate::agent::memory::MessageRole::Tool,
                content: "result".to_string(),
                reasoning: None,
                tool_call_id: Some("provider-b".to_string()),
                tool_call_correlation: Some(correlation),
                tool_name: Some("search".to_string()),
                tool_calls: None,
                tool_call_correlations: None,
                externalized_payload: None,
                pruned_artifact: None,
                structured_summary: None,
                archive_ref: None,
            },
        ];
        let snapshot = CompactionSnapshot {
            entries: vec![
                ClassifiedMemoryEntry {
                    index: 0,
                    kind: AgentMessageKind::AssistantToolCall,
                    retention: CompactionRetention::CompactableHistory,
                    estimated_tokens: 10,
                    content_chars: messages[0].content.len(),
                    has_reasoning: false,
                    tool_name: None,
                    is_externalized: false,
                    archive_ref: None,
                    is_pruned: false,
                    preserve_in_raw_window: false,
                },
                ClassifiedMemoryEntry {
                    index: 1,
                    kind: AgentMessageKind::ToolResult,
                    retention: CompactionRetention::PrunableArtifact,
                    estimated_tokens: 10,
                    content_chars: messages[1].content.len(),
                    has_reasoning: false,
                    tool_name: Some("search".to_string()),
                    is_externalized: false,
                    archive_ref: None,
                    is_pruned: false,
                    preserve_in_raw_window: true,
                },
            ],
            ..CompactionSnapshot::default()
        };
        let mut preserved = vec![false, true];
        let mut rebuilt = vec![messages[1].clone()];

        remove_orphaned_tool_results(&snapshot, &messages, &mut preserved, &mut rebuilt);

        assert_eq!(preserved, vec![false, false]);
        assert!(rebuilt.is_empty());
    }

    #[test]
    fn truncate_to_summary_replaces_everything_with_summary_and_archive_ref() {
        let messages = vec![
            AgentMessage::topic_agents_md("# Topic AGENTS\nPreserve identity."),
            AgentMessage::system_context("Execution policy"),
            AgentMessage::user_task("Ship stage 8"),
            AgentMessage::user("Older request"),
            AgentMessage::assistant("Older response"),
            AgentMessage::user("Recent request 1"),
            AgentMessage::assistant("Recent response 1"),
            AgentMessage::user("Recent request 2"),
            AgentMessage::assistant("Recent response 2"),
        ];
        let summary = CompactionSummary {
            goal: "Ship stage 8".to_string(),
            decisions: vec!["Truncated entire hot context.".to_string()],
            remaining_work: vec!["Wire runner lifecycle next.".to_string()],
            ..CompactionSummary::default()
        };
        let archive_ref = crate::agent::ArchiveRef {
            archive_id: "archive-1".to_string(),
            created_at: 1,
            title: "Compacted history: Ship stage 8".to_string(),
            storage_key: "archive/topic/flow/history-archive-1.json".to_string(),
        };

        let (rebuilt, outcome) =
            truncate_to_summary(&messages, Some(summary.clone()), Some(archive_ref.clone()));

        assert!(outcome.applied);
        assert!(outcome.inserted_summary);
        assert!(outcome.inserted_archive_reference);
        assert_eq!(outcome.dropped_message_count, 7);
        assert!(outcome.preserved_recent_indices.is_empty());
        assert_eq!(rebuilt.len(), 2);
        assert_eq!(
            rebuilt[0].resolved_kind(),
            crate::agent::AgentMessageKind::Summary
        );
        assert_eq!(rebuilt[0].summary_payload(), Some(&summary));
        assert_eq!(rebuilt[1].archive_ref_payload(), Some(&archive_ref));
    }

    #[test]
    fn truncate_to_summary_without_archive_ref_produces_single_message() {
        let messages = vec![
            AgentMessage::user("Request"),
            AgentMessage::assistant("Response"),
        ];
        let summary = CompactionSummary {
            goal: "Task done".to_string(),
            ..CompactionSummary::default()
        };

        let (rebuilt, outcome) = truncate_to_summary(&messages, Some(summary), None);

        assert!(outcome.applied);
        assert!(outcome.inserted_summary);
        assert!(!outcome.inserted_archive_reference);
        assert_eq!(rebuilt.len(), 1);
        assert_eq!(
            rebuilt[0].resolved_kind(),
            crate::agent::AgentMessageKind::Summary
        );
    }

    #[test]
    fn truncate_to_summary_is_noop_without_summary() {
        let messages = vec![
            AgentMessage::user("Request"),
            AgentMessage::assistant("Response"),
        ];

        let (rebuilt, outcome) = truncate_to_summary(&messages, None, None);

        assert!(!outcome.applied);
        assert_eq!(rebuilt.len(), 2);
    }
}
