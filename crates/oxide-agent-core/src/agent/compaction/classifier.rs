//! Hot-memory classifier for Agent Mode compaction planning.

use super::budget::estimate_message_tokens;
use super::types::{
    resolve_retention, AgentMessageKind, ClassifiedMemoryEntry, CompactionClassSummary,
    CompactionPolicy, CompactionRetention, CompactionSnapshot, RecentRawWindow,
};
use crate::agent::memory::AgentMessage;

const RECENT_USER_TURN_LIMIT: usize = 2;
const RECENT_ASSISTANT_TURN_LIMIT: usize = 2;

/// Classify the current hot memory into Stage 4 compaction buckets.
#[must_use]
pub fn classify_hot_memory(messages: &[AgentMessage]) -> CompactionSnapshot {
    classify_hot_memory_with_policy(messages, &CompactionPolicy::default(), None)
}

/// Classify hot memory using the provided policy and optional context window.
#[must_use]
pub fn classify_hot_memory_with_policy(
    messages: &[AgentMessage],
    policy: &CompactionPolicy,
    context_window_tokens: Option<usize>,
) -> CompactionSnapshot {
    let recent_raw_window = RecentRawWindow {
        user_turn_indices: collect_recent_indices(messages, RECENT_USER_TURN_LIMIT, |kind| {
            matches!(kind, AgentMessageKind::UserTurn)
        }),
        assistant_turn_indices: collect_recent_indices(
            messages,
            RECENT_ASSISTANT_TURN_LIMIT,
            |kind| {
                matches!(
                    kind,
                    AgentMessageKind::AssistantResponse | AgentMessageKind::AssistantReasoning
                )
            },
        ),
        tool_interaction_indices: collect_recent_tool_indices(
            messages,
            resolve_protected_tool_window_tokens(policy, context_window_tokens),
        ),
    };
    let mut snapshot = CompactionSnapshot {
        recent_raw_window,
        ..CompactionSnapshot::default()
    };

    for (index, message) in messages.iter().enumerate() {
        let kind = message.resolved_kind();
        let retention = resolve_retention(kind, message.tool_name.as_deref());
        let estimated_tokens = message
            .externalized_payload
            .as_ref()
            .filter(|_| !message.is_pruned())
            .map_or_else(
                || estimate_message_tokens(message),
                |payload| payload.estimated_tokens,
            );
        let content_chars = message
            .externalized_payload
            .as_ref()
            .filter(|_| !message.is_pruned())
            .map_or_else(
                || message.content.chars().count(),
                |payload| payload.original_chars,
            );
        let entry = ClassifiedMemoryEntry {
            index,
            kind,
            retention,
            estimated_tokens,
            content_chars,
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
            preserve_in_raw_window: snapshot.recent_raw_window.contains(index),
        };

        match retention {
            CompactionRetention::Pinned => record_summary(&mut snapshot.pinned, &entry),
            CompactionRetention::ProtectedLive => {
                record_summary(&mut snapshot.protected_live, &entry)
            }
            CompactionRetention::PrunableArtifact => {
                record_summary(&mut snapshot.prunable_artifacts, &entry)
            }
            CompactionRetention::CompactableHistory => {
                record_summary(&mut snapshot.compactable_history, &entry)
            }
        }

        snapshot.entries.push(entry);
    }

    snapshot
}

fn collect_recent_tool_indices(messages: &[AgentMessage], protected_tokens: usize) -> Vec<usize> {
    let mut indices = Vec::new();
    let mut protected = 0usize;

    for (index, message) in messages.iter().enumerate().rev() {
        let kind = message.resolved_kind();
        if !matches!(
            kind,
            AgentMessageKind::AssistantToolCall | AgentMessageKind::ToolResult
        ) {
            continue;
        }

        if !indices.is_empty() && protected >= protected_tokens {
            break;
        }

        indices.push(index);
        protected = protected.saturating_add(estimate_message_tokens(message));
    }

    indices.sort_unstable();
    indices
}

fn resolve_protected_tool_window_tokens(
    policy: &CompactionPolicy,
    context_window_tokens: Option<usize>,
) -> usize {
    match context_window_tokens {
        Some(context_window_tokens) => {
            let scaled_budget = context_window_tokens / 5;
            if scaled_budget == 0 {
                policy.protected_tool_window_tokens
            } else {
                policy.protected_tool_window_tokens.min(scaled_budget)
            }
        }
        None => policy.protected_tool_window_tokens,
    }
}

fn collect_recent_indices(
    messages: &[AgentMessage],
    limit: usize,
    predicate: impl Fn(AgentMessageKind) -> bool,
) -> Vec<usize> {
    let mut indices = Vec::with_capacity(limit);

    for (index, message) in messages.iter().enumerate().rev() {
        if predicate(message.resolved_kind()) {
            indices.push(index);
            if indices.len() == limit {
                break;
            }
        }
    }

    indices.sort_unstable();
    indices
}

fn record_summary(summary: &mut CompactionClassSummary, entry: &ClassifiedMemoryEntry) {
    summary.message_count = summary.message_count.saturating_add(1);
    summary.token_count = summary.token_count.saturating_add(entry.estimated_tokens);
    summary.message_indices.push(entry.index);
}

#[cfg(test)]
mod tests {
    use super::{classify_hot_memory, classify_hot_memory_with_policy};
    use crate::agent::compaction::{AgentMessageKind, CompactionPolicy, CompactionRetention};
    use crate::agent::memory::{AgentMessage, MessageRole};
    use crate::llm::{ToolCall, ToolCallFunction};

    #[test]
    fn classify_hot_memory_splits_entries_into_expected_buckets() {
        let messages = vec![
            AgentMessage::topic_agents_md("# Topic AGENTS\nStay safe."),
            AgentMessage::summary("[Previous context compressed]\n- done"),
            AgentMessage::archive_reference("archive://chunk-1"),
            AgentMessage::system_context("Base execution policy"),
            AgentMessage::user_task("Ship stage 4"),
            AgentMessage::runtime_context("User added a new deadline"),
            AgentMessage::skill_context("[Loaded skill: release]"),
            AgentMessage::approval_replay("Approval granted for exact SSH replay"),
            AgentMessage::infra_status("SSH target validated"),
            AgentMessage::assistant_with_tools(
                "Calling tools",
                vec![ToolCall {
                    id: "call-1".to_string(),
                    function: ToolCallFunction {
                        name: "execute_command".to_string(),
                        arguments: "{\"command\":\"cargo check\"}".to_string(),
                    },
                    is_recovered: false,
                }],
            ),
            AgentMessage::tool("call-1", "execute_command", "stdout"),
            AgentMessage::user("Earlier user turn"),
            AgentMessage::assistant("Earlier assistant turn"),
        ];

        let snapshot = classify_hot_memory(&messages);

        assert_eq!(snapshot.pinned.message_indices, vec![0, 1, 2]);
        assert_eq!(
            snapshot.protected_live.message_indices,
            vec![3, 4, 5, 6, 7, 8]
        );
        assert_eq!(snapshot.prunable_artifacts.message_indices, vec![10]);
        assert_eq!(
            snapshot.compactable_history.message_indices,
            vec![9, 11, 12]
        );
        assert_eq!(snapshot.entries[0].retention, CompactionRetention::Pinned);
        assert_eq!(
            snapshot.entries[10].retention,
            CompactionRetention::PrunableArtifact
        );
        assert_eq!(
            snapshot.entries[12].kind,
            AgentMessageKind::AssistantResponse
        );
    }

    #[test]
    fn classify_hot_memory_marks_recent_raw_window() {
        let policy = CompactionPolicy {
            protected_tool_window_tokens: 32,
            ..CompactionPolicy::default()
        };
        let messages = vec![
            AgentMessage::user("user-1"),
            AgentMessage::assistant("assistant-1"),
            AgentMessage::assistant_with_tools(
                "tool-call-1",
                vec![ToolCall {
                    id: "call-1".to_string(),
                    function: ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                    is_recovered: false,
                }],
            ),
            AgentMessage::tool("call-1", "search", "result-1"),
            AgentMessage::user("user-2"),
            AgentMessage::assistant_with_reasoning("assistant-2", "thinking"),
            AgentMessage::assistant_with_tools(
                "tool-call-2",
                vec![ToolCall {
                    id: "call-2".to_string(),
                    function: ToolCallFunction {
                        name: "read_file".to_string(),
                        arguments: "{}".to_string(),
                    },
                    is_recovered: false,
                }],
            ),
            AgentMessage::tool("call-2", "read_file", "result-2"),
            AgentMessage::user("user-3"),
            AgentMessage::assistant("assistant-3"),
        ];

        let snapshot = classify_hot_memory_with_policy(&messages, &policy, None);

        assert_eq!(snapshot.recent_raw_window.user_turn_indices, vec![4, 8]);
        assert_eq!(
            snapshot.recent_raw_window.assistant_turn_indices,
            vec![5, 9]
        );
        assert_eq!(
            snapshot.recent_raw_window.tool_interaction_indices,
            vec![2, 3, 6, 7]
        );
        assert!(snapshot.entries[4].preserve_in_raw_window);
        assert!(snapshot.entries[7].preserve_in_raw_window);
        assert!(snapshot.entries[9].preserve_in_raw_window);
        assert!(!snapshot.entries[0].preserve_in_raw_window);
    }

    #[test]
    fn classify_hot_memory_protects_recent_tool_tokens_instead_of_fixed_count() {
        let policy = CompactionPolicy {
            protected_tool_window_tokens: 512,
            ..CompactionPolicy::default()
        };
        let messages = vec![
            AgentMessage::assistant_with_tools(
                "call-1",
                vec![ToolCall {
                    id: "call-1".to_string(),
                    function: ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                    is_recovered: false,
                }],
            ),
            AgentMessage::tool("call-1", "search", &"A".repeat(1_200)),
            AgentMessage::assistant_with_tools(
                "call-2",
                vec![ToolCall {
                    id: "call-2".to_string(),
                    function: ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                    is_recovered: false,
                }],
            ),
            AgentMessage::tool("call-2", "search", "short-2"),
            AgentMessage::assistant_with_tools(
                "call-3",
                vec![ToolCall {
                    id: "call-3".to_string(),
                    function: ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                    is_recovered: false,
                }],
            ),
            AgentMessage::tool("call-3", "search", "short-3"),
            AgentMessage::assistant_with_tools(
                "call-4",
                vec![ToolCall {
                    id: "call-4".to_string(),
                    function: ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                    is_recovered: false,
                }],
            ),
            AgentMessage::tool("call-4", "search", "short-4"),
            AgentMessage::assistant_with_tools(
                "call-5",
                vec![ToolCall {
                    id: "call-5".to_string(),
                    function: ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                    is_recovered: false,
                }],
            ),
            AgentMessage::tool("call-5", "search", "short-5"),
        ];

        let snapshot = classify_hot_memory_with_policy(&messages, &policy, None);

        assert_eq!(
            snapshot.recent_raw_window.tool_interaction_indices,
            vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
        );
        assert!(snapshot.entries[0].preserve_in_raw_window);
        assert!(snapshot.entries[1].preserve_in_raw_window);
    }

    #[test]
    fn classify_hot_memory_caps_tool_budget_relative_to_context_window() {
        let policy = CompactionPolicy {
            protected_tool_window_tokens: 40_000,
            ..CompactionPolicy::default()
        };
        let messages = vec![
            AgentMessage::assistant_with_tools(
                "call-1",
                vec![ToolCall {
                    id: "call-1".to_string(),
                    function: ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                    is_recovered: false,
                }],
            ),
            AgentMessage::tool(
                "call-1",
                "search",
                &(0..1_500)
                    .map(|index| format!("alpha_{index} beta_{index}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            AgentMessage::assistant_with_tools(
                "call-2",
                vec![ToolCall {
                    id: "call-2".to_string(),
                    function: ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                    is_recovered: false,
                }],
            ),
            AgentMessage::tool(
                "call-2",
                "search",
                &(0..1_500)
                    .map(|index| format!("gamma_{index} delta_{index}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            AgentMessage::assistant_with_tools(
                "call-3",
                vec![ToolCall {
                    id: "call-3".to_string(),
                    function: ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                    is_recovered: false,
                }],
            ),
            AgentMessage::tool(
                "call-3",
                "search",
                &(0..1_500)
                    .map(|index| format!("epsilon_{index} zeta_{index}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
        ];

        let snapshot = classify_hot_memory_with_policy(&messages, &policy, Some(4_096));

        assert!(snapshot.recent_raw_window.tool_interaction_indices.len() < messages.len());
        assert!(snapshot.entries[5].preserve_in_raw_window);
        assert!(!snapshot.entries[0].preserve_in_raw_window);
    }

    #[test]
    fn classify_hot_memory_uses_resolved_kind_for_legacy_messages() {
        let messages = vec![
            AgentMessage {
                kind: AgentMessageKind::Legacy,
                role: MessageRole::System,
                content: "[TOPIC_AGENTS_MD]\nPinned policy".to_string(),
                reasoning: None,
                tool_call_id: None,
                tool_name: None,
                tool_calls: None,
                externalized_payload: None,
                pruned_artifact: None,
                structured_summary: None,
                archive_ref: None,
            },
            AgentMessage {
                kind: AgentMessageKind::Legacy,
                role: MessageRole::Tool,
                content: "tool output".to_string(),
                reasoning: None,
                tool_call_id: Some("call-1".to_string()),
                tool_name: Some("execute_command".to_string()),
                tool_calls: None,
                externalized_payload: None,
                pruned_artifact: None,
                structured_summary: None,
                archive_ref: None,
            },
            AgentMessage {
                kind: AgentMessageKind::Legacy,
                role: MessageRole::Assistant,
                content: "done".to_string(),
                reasoning: None,
                tool_call_id: None,
                tool_name: None,
                tool_calls: None,
                externalized_payload: None,
                pruned_artifact: None,
                structured_summary: None,
                archive_ref: None,
            },
        ];

        let snapshot = classify_hot_memory(&messages);

        assert_eq!(snapshot.entries[0].kind, AgentMessageKind::TopicAgentsMd);
        assert_eq!(snapshot.entries[0].retention, CompactionRetention::Pinned);
        assert_eq!(snapshot.entries[1].kind, AgentMessageKind::ToolResult);
        assert_eq!(
            snapshot.entries[1].retention,
            CompactionRetention::PrunableArtifact
        );
        assert_eq!(
            snapshot.entries[2].kind,
            AgentMessageKind::AssistantResponse
        );
        assert_eq!(
            snapshot.entries[2].retention,
            CompactionRetention::CompactableHistory
        );
    }
}
