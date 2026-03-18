//! Hot-memory classifier for Agent Mode compaction planning.

use super::budget::estimate_message_tokens;
use super::types::{
    AgentMessageKind, ClassifiedMemoryEntry, CompactionClassSummary, CompactionRetention,
    CompactionSnapshot, RecentRawWindow,
};
use crate::agent::memory::AgentMessage;

const RECENT_USER_TURN_LIMIT: usize = 2;
const RECENT_ASSISTANT_TURN_LIMIT: usize = 2;
const RECENT_TOOL_INTERACTION_LIMIT: usize = 4;

/// Classify the current hot memory into Stage 4 compaction buckets.
#[must_use]
pub fn classify_hot_memory(messages: &[AgentMessage]) -> CompactionSnapshot {
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
        tool_interaction_indices: collect_recent_indices(
            messages,
            RECENT_TOOL_INTERACTION_LIMIT,
            |kind| {
                matches!(
                    kind,
                    AgentMessageKind::AssistantToolCall | AgentMessageKind::ToolResult
                )
            },
        ),
    };
    let mut snapshot = CompactionSnapshot {
        recent_raw_window,
        ..CompactionSnapshot::default()
    };

    for (index, message) in messages.iter().enumerate() {
        let kind = message.resolved_kind();
        let retention = kind.retention();
        let entry = ClassifiedMemoryEntry {
            index,
            kind,
            retention,
            estimated_tokens: estimate_message_tokens(message),
            content_chars: message.content.chars().count(),
            has_reasoning: message.reasoning.is_some(),
            tool_name: message.tool_name.clone(),
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
    use super::classify_hot_memory;
    use crate::agent::compaction::{AgentMessageKind, CompactionRetention};
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

        let snapshot = classify_hot_memory(&messages);

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
            },
            AgentMessage {
                kind: AgentMessageKind::Legacy,
                role: MessageRole::Tool,
                content: "tool output".to_string(),
                reasoning: None,
                tool_call_id: Some("call-1".to_string()),
                tool_name: Some("execute_command".to_string()),
                tool_calls: None,
            },
            AgentMessage {
                kind: AgentMessageKind::Legacy,
                role: MessageRole::Assistant,
                content: "done".to_string(),
                reasoning: None,
                tool_call_id: None,
                tool_name: None,
                tool_calls: None,
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
