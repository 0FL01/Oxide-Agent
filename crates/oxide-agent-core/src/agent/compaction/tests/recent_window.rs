use crate::agent::compaction::{
    classify_hot_memory_with_policy, AgentMessageKind, CompactionPolicy,
};
use crate::agent::memory::AgentMessage;
use crate::llm::{ToolCall, ToolCallFunction};

fn tool_call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        function: ToolCallFunction {
            name: name.to_string(),
            arguments: "{}".to_string(),
        },
        is_recovered: false,
    }
}

#[test]
fn recent_window_keeps_only_last_two_user_and_assistant_turns() {
    let policy = CompactionPolicy {
        protected_tool_window_tokens: 4,
        ..CompactionPolicy::default()
    };
    let messages = vec![
        AgentMessage::user("user-1"),
        AgentMessage::assistant("assistant-1"),
        AgentMessage::user("user-2"),
        AgentMessage::assistant_with_reasoning("assistant-2", "thinking"),
        AgentMessage::user("user-3"),
        AgentMessage::assistant("assistant-3"),
    ];

    let snapshot = classify_hot_memory_with_policy(&messages, &policy, None);

    assert_eq!(snapshot.recent_raw_window.user_turn_indices, vec![2, 4]);
    assert_eq!(
        snapshot.recent_raw_window.assistant_turn_indices,
        vec![3, 5]
    );
    assert!(!snapshot.entries[0].preserve_in_raw_window);
    assert!(!snapshot.entries[1].preserve_in_raw_window);
    assert!(snapshot.entries[4].preserve_in_raw_window);
    assert!(snapshot.entries[5].preserve_in_raw_window);
}

#[test]
fn recent_window_keeps_tool_interactions_within_token_budget() {
    let policy = CompactionPolicy {
        protected_tool_window_tokens: 64,
        ..CompactionPolicy::default()
    };
    let messages = vec![
        AgentMessage::assistant_with_tools("call-1", vec![tool_call("call-1", "search")]),
        AgentMessage::tool("call-1", "search", "result-1"),
        AgentMessage::assistant_with_tools("call-2", vec![tool_call("call-2", "search")]),
        AgentMessage::tool("call-2", "search", "result-2"),
        AgentMessage::assistant_with_tools("call-3", vec![tool_call("call-3", "search")]),
        AgentMessage::tool("call-3", "search", "result-3"),
    ];

    let snapshot = classify_hot_memory_with_policy(&messages, &policy, None);

    assert_eq!(
        snapshot.recent_raw_window.tool_interaction_indices,
        vec![0, 1, 2, 3, 4, 5]
    );
    assert!(snapshot.entries[0].preserve_in_raw_window);
    assert!(snapshot.entries[1].preserve_in_raw_window);
    assert!(snapshot.entries[2].preserve_in_raw_window);
    assert!(snapshot.entries[5].preserve_in_raw_window);
}

#[test]
fn assistant_tool_calls_do_not_consume_assistant_turn_slots() {
    let policy = CompactionPolicy {
        protected_tool_window_tokens: 64,
        ..CompactionPolicy::default()
    };
    let messages = vec![
        AgentMessage::assistant("assistant-1"),
        AgentMessage::assistant_with_tools("tool-call", vec![tool_call("call-1", "search")]),
        AgentMessage::tool("call-1", "search", "result-1"),
        AgentMessage::assistant_with_reasoning("assistant-2", "thinking"),
        AgentMessage::assistant("assistant-3"),
    ];

    let snapshot = classify_hot_memory_with_policy(&messages, &policy, None);

    assert_eq!(
        snapshot.recent_raw_window.assistant_turn_indices,
        vec![3, 4]
    );
    assert_eq!(
        snapshot.recent_raw_window.tool_interaction_indices,
        vec![1, 2]
    );
    assert_eq!(
        snapshot.entries[1].kind,
        AgentMessageKind::AssistantToolCall
    );
    assert!(!snapshot
        .recent_raw_window
        .assistant_turn_indices
        .contains(&1));
}
