//! Hot-context rebuild after pruning and summary compaction.

use super::archive::ArchiveRef;
use super::budget::estimate_message_tokens;
use super::summary::{
    collect_existing_structured_summaries, merge_summaries_bounded, normalize_summary,
};
use super::types::{
    AgentMessageKind, BreadcrumbCard, CompactionRetention, CompactionSnapshot, CompactionSummary,
    RebuildOutcome,
};
use crate::agent::memory::AgentMessage;
use crate::llm::InvocationId;
use tracing::warn;

const POST_RUN_RECENT_RAW_TARGET_TOKENS: usize = 8 * 1024;
const POST_RUN_MAX_RECENT_USER_REQUESTS: usize = 2;
const POST_RUN_MAX_RECENT_ASSISTANT_UPDATES: usize = 2;
const POST_RUN_MAX_RECENT_TOOL_OUTCOMES: usize = 3;
const POST_RUN_MAX_AUTHORITATIVE_ITEMS: usize = 6;

/// PostRun truncate: rebuild a compact retained working set instead of wiping memory down to
/// summary-only state.
#[must_use]
pub fn truncate_to_working_set(
    messages: &[AgentMessage],
    summary: Option<CompactionSummary>,
    archive_ref: Option<ArchiveRef>,
) -> (Vec<AgentMessage>, RebuildOutcome) {
    let summary = match summary.and_then(normalize_summary) {
        Some(s) => s,
        None => return (messages.to_vec(), RebuildOutcome::default()),
    };

    let total_before = messages.len();
    let mut rebuilt = Vec::new();
    let mut preserved = vec![false; messages.len()];

    if let Some((index, message)) = messages
        .iter()
        .enumerate()
        .find(|(_, message)| message.resolved_kind() == AgentMessageKind::TopicAgentsMd)
    {
        preserved[index] = true;
        rebuilt.push(message.clone());
    }

    if let Some(index) = latest_matching_index(messages, |kind| kind == AgentMessageKind::UserTask)
    {
        if !preserved[index] {
            preserved[index] = true;
            rebuilt.push(messages[index].clone());
        }
    }

    rebuilt.push(AgentMessage::from_breadcrumb_card(build_breadcrumb_card(
        messages, &summary,
    )));
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

    let preserved_recent_indices = collect_post_run_raw_tail_indices(messages, &preserved);
    for index in &preserved_recent_indices {
        preserved[*index] = true;
        rebuilt.push(messages[*index].clone());
    }

    let snapshot = CompactionSnapshot {
        entries: messages
            .iter()
            .enumerate()
            .map(|(index, message)| super::types::ClassifiedMemoryEntry {
                index,
                kind: message.resolved_kind(),
                retention: message.retention(),
                estimated_tokens: estimate_message_tokens(message),
                content_chars: message.content.chars().count(),
                has_reasoning: message.reasoning.is_some(),
                tool_name: message.tool_name.clone(),
                is_externalized: message.is_externalized(),
                archive_ref: message.archive_ref_payload().cloned().or_else(|| {
                    message
                        .externalized_payload
                        .as_ref()
                        .map(|payload| payload.archive_ref.clone())
                }),
                is_pruned: message.is_pruned(),
                preserve_in_raw_window: preserved_recent_indices.contains(&index),
            })
            .collect(),
        ..CompactionSnapshot::default()
    };

    remove_orphaned_tool_results(&snapshot, messages, &mut preserved, &mut rebuilt);

    let outcome = RebuildOutcome {
        applied: true,
        inserted_summary: true,
        inserted_breadcrumb: true,
        inserted_archive_reference,
        dropped_message_count: preserved
            .iter()
            .filter(|is_preserved| !**is_preserved)
            .count(),
        dropped_indices: preserved
            .iter()
            .enumerate()
            .filter_map(|(index, is_preserved)| (!is_preserved).then_some(index))
            .collect(),
        preserved_recent_indices,
    };

    warn!(
        inserted_summary = outcome.inserted_summary,
        inserted_breadcrumb = outcome.inserted_breadcrumb,
        inserted_archive_reference = outcome.inserted_archive_reference,
        dropped_message_count = outcome.dropped_message_count,
        messages_before = total_before,
        messages_after = rebuilt.len(),
        preserved_recent_count = outcome.preserved_recent_indices.len(),
        "PostRun truncate-to-working-set: hot context replaced with retained working set"
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
        inserted_breadcrumb: false,
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
            inserted_breadcrumb = outcome.inserted_breadcrumb,
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

fn build_breadcrumb_card(messages: &[AgentMessage], summary: &CompactionSummary) -> BreadcrumbCard {
    BreadcrumbCard {
        current_goal: if summary.goal.trim().is_empty() {
            latest_text(messages, |kind| kind == AgentMessageKind::UserTask).unwrap_or_default()
        } else {
            summary.goal.trim().to_string()
        },
        authoritative_state: summary
            .constraints
            .iter()
            .chain(summary.decisions.iter())
            .chain(summary.relevant_files_entities.iter())
            .map(|item| item.trim())
            .filter(|item| !item.is_empty())
            .take(POST_RUN_MAX_AUTHORITATIVE_ITEMS)
            .map(ToOwned::to_owned)
            .collect(),
        recent_user_requests: collect_recent_texts(
            messages,
            POST_RUN_MAX_RECENT_USER_REQUESTS,
            |kind| {
                matches!(
                    kind,
                    AgentMessageKind::UserTask
                        | AgentMessageKind::UserTurn
                        | AgentMessageKind::RuntimeContext
                )
            },
            |message| truncate_chars(&message.content, 240),
        ),
        recent_assistant_updates: collect_recent_texts(
            messages,
            POST_RUN_MAX_RECENT_ASSISTANT_UPDATES,
            |kind| {
                matches!(
                    kind,
                    AgentMessageKind::AssistantResponse | AgentMessageKind::AssistantReasoning
                )
            },
            |message| truncate_chars(&message.content, 240),
        ),
        recent_tool_outcomes: collect_recent_texts(
            messages,
            POST_RUN_MAX_RECENT_TOOL_OUTCOMES,
            |kind| kind == AgentMessageKind::ToolResult,
            |message| {
                let tool_name = message.tool_name.as_deref().unwrap_or("tool");
                format!("{tool_name}: {}", truncate_chars(&message.content, 240))
            },
        ),
        next_steps: summary
            .remaining_work
            .iter()
            .map(|item| item.trim())
            .filter(|item| !item.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        open_questions: summary
            .risks
            .iter()
            .map(|item| item.trim())
            .filter(|item| !item.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    }
}

fn collect_post_run_raw_tail_indices(messages: &[AgentMessage], preserved: &[bool]) -> Vec<usize> {
    let mut selected = Vec::new();
    let mut protected_tokens = 0usize;

    for (index, message) in messages.iter().enumerate().rev() {
        if preserved.get(index).copied().unwrap_or(false) {
            continue;
        }
        if !is_post_run_tail_kind(message.resolved_kind()) {
            continue;
        }

        let estimated_tokens = estimate_message_tokens(message);
        if !selected.is_empty()
            && protected_tokens.saturating_add(estimated_tokens) > POST_RUN_RECENT_RAW_TARGET_TOKENS
        {
            continue;
        }

        selected.push(index);
        protected_tokens = protected_tokens.saturating_add(estimated_tokens);
    }

    ensure_latest_anchor(&mut selected, messages, preserved, |kind| {
        matches!(
            kind,
            AgentMessageKind::UserTurn | AgentMessageKind::RuntimeContext
        )
    });
    ensure_latest_anchor(&mut selected, messages, preserved, |kind| {
        matches!(
            kind,
            AgentMessageKind::AssistantResponse | AgentMessageKind::AssistantReasoning
        )
    });

    selected.sort_unstable();
    selected.dedup();
    selected
}

fn ensure_latest_anchor(
    selected: &mut Vec<usize>,
    messages: &[AgentMessage],
    preserved: &[bool],
    predicate: impl Fn(AgentMessageKind) -> bool,
) {
    let Some(index) = messages
        .iter()
        .enumerate()
        .rev()
        .find(|(index, message)| {
            !preserved.get(*index).copied().unwrap_or(false) && predicate(message.resolved_kind())
        })
        .map(|(index, _)| index)
    else {
        return;
    };

    if !selected.contains(&index) {
        selected.push(index);
    }
}

fn is_post_run_tail_kind(kind: AgentMessageKind) -> bool {
    matches!(
        kind,
        AgentMessageKind::UserTask
            | AgentMessageKind::RuntimeContext
            | AgentMessageKind::UserTurn
            | AgentMessageKind::AssistantResponse
            | AgentMessageKind::AssistantReasoning
            | AgentMessageKind::AssistantToolCall
            | AgentMessageKind::ToolResult
    )
}

fn latest_matching_index(
    messages: &[AgentMessage],
    predicate: impl Fn(AgentMessageKind) -> bool,
) -> Option<usize> {
    messages
        .iter()
        .enumerate()
        .rev()
        .find(|(_, message)| predicate(message.resolved_kind()))
        .map(|(index, _)| index)
}

fn latest_text(
    messages: &[AgentMessage],
    predicate: impl Fn(AgentMessageKind) -> bool,
) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| predicate(message.resolved_kind()))
        .map(|message| truncate_chars(&message.content, 240))
}

fn collect_recent_texts(
    messages: &[AgentMessage],
    limit: usize,
    predicate: impl Fn(AgentMessageKind) -> bool,
    render: impl Fn(&AgentMessage) -> String,
) -> Vec<String> {
    let mut items = messages
        .iter()
        .rev()
        .filter(|message| predicate(message.resolved_kind()))
        .map(render)
        .filter(|item| !item.trim().is_empty())
        .take(limit)
        .collect::<Vec<_>>();
    items.reverse();
    items
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
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
    use super::{rebuild_hot_context, remove_orphaned_tool_results, truncate_to_working_set};
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
                breadcrumb_card: None,
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
                breadcrumb_card: None,
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
    fn truncate_to_working_set_preserves_breadcrumb_summary_archive_and_recent_tail() {
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
            truncate_to_working_set(&messages, Some(summary.clone()), Some(archive_ref.clone()));

        assert!(outcome.applied);
        assert!(outcome.inserted_summary);
        assert!(outcome.inserted_breadcrumb);
        assert!(outcome.inserted_archive_reference);
        assert!(outcome.dropped_message_count >= 1);
        assert!(!outcome.preserved_recent_indices.is_empty());
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
            crate::agent::AgentMessageKind::Breadcrumb
        );
        assert_eq!(
            rebuilt[3].resolved_kind(),
            crate::agent::AgentMessageKind::Summary
        );
        assert_eq!(rebuilt[3].summary_payload(), Some(&summary));
        assert_eq!(rebuilt[4].archive_ref_payload(), Some(&archive_ref));
        assert!(rebuilt
            .iter()
            .any(|message| message.content == "Recent request 2"));
        assert!(rebuilt
            .iter()
            .any(|message| message.content == "Recent response 2"));
    }

    #[test]
    fn truncate_to_working_set_without_archive_ref_keeps_breadcrumb_and_summary() {
        let messages = vec![
            AgentMessage::user("Request"),
            AgentMessage::assistant("Response"),
        ];
        let summary = CompactionSummary {
            goal: "Task done".to_string(),
            ..CompactionSummary::default()
        };

        let (rebuilt, outcome) = truncate_to_working_set(&messages, Some(summary), None);

        assert!(outcome.applied);
        assert!(outcome.inserted_summary);
        assert!(outcome.inserted_breadcrumb);
        assert!(!outcome.inserted_archive_reference);
        assert_eq!(rebuilt.len(), 4);
        assert_eq!(
            rebuilt[0].resolved_kind(),
            crate::agent::AgentMessageKind::Breadcrumb
        );
        assert_eq!(
            rebuilt[1].resolved_kind(),
            crate::agent::AgentMessageKind::Summary
        );
        assert_eq!(rebuilt[2].content, "Request");
        assert_eq!(rebuilt[3].content, "Response");
    }

    #[test]
    fn truncate_to_working_set_is_noop_without_summary() {
        let messages = vec![
            AgentMessage::user("Request"),
            AgentMessage::assistant("Response"),
        ];

        let (rebuilt, outcome) = truncate_to_working_set(&messages, None, None);

        assert!(!outcome.applied);
        assert_eq!(rebuilt.len(), 2);
    }
}
