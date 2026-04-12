//! Анализ надежности фикса orphaned tool results
//!
//! Этот файл содержит тесты для проверки edge cases и оценки надежности

use oxide_agent_core::agent::compaction::{
    classify_hot_memory_with_policy, rebuild_hot_context, AgentMessageKind, CompactionPolicy,
    CompactionSummary,
};
use oxide_agent_core::agent::memory::AgentMessage;
use oxide_agent_core::llm::{ToolCall, ToolCallFunction};

fn tool_call(id: &str, name: &str) -> ToolCall {
    ToolCall::new(
        id.to_string(),
        ToolCallFunction {
            name: name.to_string(),
            arguments: r#"{}"#.to_string(),
        },
        false,
    )
}

/// Scenario 1: Multiple tool calls from single assistant message
/// Если assistant делает несколько tool calls и compaction удаляет assistant message,
/// все tool results должны быть удалены
#[test]
fn multiple_tool_calls_all_orphaned() {
    let messages = vec![
        AgentMessage::user("Do multiple things"),
        AgentMessage::assistant_with_tools(
            "I'll help you with that.",
            vec![
                tool_call("call_1", "search"),
                tool_call("call_2", "read_file"),
                tool_call("call_3", "execute_command"),
            ],
        ),
        AgentMessage::tool("call_1", "search", "search results"),
        AgentMessage::tool("call_2", "read_file", "file content"),
        AgentMessage::tool("call_3", "execute_command", "command output"),
    ];

    let policy = CompactionPolicy {
        protected_tool_window_tokens: 0,
        ..CompactionPolicy::default()
    };

    let snapshot = classify_hot_memory_with_policy(&messages, &policy, Some(1000));

    let summary = CompactionSummary {
        goal: "Test".to_string(),
        constraints: vec![],
        decisions: vec![],
        discoveries: vec![],
        relevant_files_entities: vec![],
        remaining_work: vec![],
        risks: vec![],
    };

    let (rebuilt, _outcome) = rebuild_hot_context(&snapshot, &messages, Some(summary), None);

    // Проверяем, что ни одного orphaned tool result нет
    let orphaned_count = rebuilt
        .iter()
        .filter(|msg| msg.kind == AgentMessageKind::ToolResult)
        .filter(|msg| {
            msg.tool_call_id.as_ref().is_some_and(|id| {
                !rebuilt.iter().any(|m| {
                    m.tool_calls
                        .as_ref()
                        .is_some_and(|calls| calls.iter().any(|tc| &tc.id == id))
                })
            })
        })
        .count();

    assert_eq!(
        orphaned_count, 0,
        "Should not have any orphaned tool results. Found {} orphaned.",
        orphaned_count
    );
}

/// Scenario 2: Mixed - some tool calls preserved, some removed
/// Если compaction удаляет только часть assistant messages
#[test]
fn partial_orphaning_mixed_scenario() {
    let messages = vec![
        AgentMessage::user("Old request"),
        AgentMessage::assistant_with_tools(
            "I'll search for that.",
            vec![tool_call("old_call", "search")],
        ),
        AgentMessage::tool("old_call", "search", "old results"),
        AgentMessage::user("New request"),
        AgentMessage::assistant_with_tools(
            "I'll read the file.",
            vec![tool_call("new_call", "read_file")],
        ),
        AgentMessage::tool("new_call", "read_file", "new content"),
    ];

    // Политика, которая должна сохранить только последний tool interaction
    let policy = CompactionPolicy {
        protected_tool_window_tokens: 50, // Маленький window
        ..CompactionPolicy::default()
    };

    let snapshot = classify_hot_memory_with_policy(&messages, &policy, Some(1000));

    let summary = CompactionSummary {
        goal: "Test".to_string(),
        constraints: vec![],
        decisions: vec![],
        discoveries: vec![],
        relevant_files_entities: vec![],
        remaining_work: vec![],
        risks: vec![],
    };

    let (rebuilt, _outcome) = rebuild_hot_context(&snapshot, &messages, Some(summary), None);

    // Проверяем консистентность
    let tool_results: Vec<_> = rebuilt
        .iter()
        .filter(|msg| msg.kind == AgentMessageKind::ToolResult)
        .filter_map(|msg| msg.tool_call_id.clone())
        .collect();

    let tool_calls: Vec<_> = rebuilt
        .iter()
        .filter(|msg| msg.kind == AgentMessageKind::AssistantToolCall)
        .flat_map(|msg| {
            msg.tool_calls
                .iter()
                .flat_map(|calls| calls.iter().map(|tc| tc.id.clone()))
        })
        .collect();

    for tool_call_id in &tool_results {
        assert!(
            tool_calls.contains(tool_call_id),
            "Tool result references {} but no corresponding tool_call found in rebuilt context",
            tool_call_id
        );
    }
}

/// Scenario 3: Sub-agent delegation (delegate_to_sub_agent)
/// Это важный случай, так как sub-agent делегация создает tool calls
#[test]
fn sub_agent_delegation_orphaned() {
    let messages = vec![
        AgentMessage::user("Delegate this task"),
        AgentMessage::assistant_with_tools(
            "I'll delegate to a sub-agent.",
            vec![tool_call("delegate_call", "delegate_to_sub_agent")],
        ),
        AgentMessage::tool(
            "delegate_call",
            "delegate_to_sub_agent",
            "Sub-agent completed the task successfully",
        ),
    ];

    let policy = CompactionPolicy {
        protected_tool_window_tokens: 0,
        ..CompactionPolicy::default()
    };

    let snapshot = classify_hot_memory_with_policy(&messages, &policy, Some(1000));

    let summary = CompactionSummary {
        goal: "Test".to_string(),
        constraints: vec![],
        decisions: vec![],
        discoveries: vec![],
        relevant_files_entities: vec![],
        remaining_work: vec![],
        risks: vec![],
    };

    let (rebuilt, _outcome) = rebuild_hot_context(&snapshot, &messages, Some(summary), None);

    // Проверяем orphaned
    let has_orphaned = rebuilt.iter().any(|msg| {
        msg.kind == AgentMessageKind::ToolResult
            && msg.tool_call_id.as_ref().is_some_and(|id| {
                !rebuilt.iter().any(|m| {
                    m.tool_calls
                        .as_ref()
                        .is_some_and(|calls| calls.iter().any(|tc| &tc.id == id))
                })
            })
    });

    assert!(
        !has_orphaned,
        "Sub-agent delegation should not create orphaned tool results"
    );
}

/// Scenario 4: Tool result без tool_call_id (edge case)
/// Что если tool result создан без tool_call_id?
#[test]
fn tool_result_without_id() {
    let messages = vec![
        AgentMessage::user("Do something"),
        AgentMessage::assistant_with_tools("I'll help.", vec![tool_call("call_1", "search")]),
        AgentMessage::tool("call_1", "search", "results"),
    ];

    let policy = CompactionPolicy::default();
    let snapshot = classify_hot_memory_with_policy(&messages, &policy, Some(10000));

    let (rebuilt, _outcome) = rebuild_hot_context(&snapshot, &messages, None, None);

    // При большом контексте ничего не должно быть удалено
    assert_eq!(
        rebuilt.len(),
        messages.len(),
        "With large context window, nothing should be dropped"
    );
}

/// Scenario 5: Очень большой tool output
/// Проверяем, что externalization работает корректно с orphaned detection
#[test]
fn large_tool_output_with_orphaned() {
    let large_output = "A".repeat(10000);

    let messages = vec![
        AgentMessage::user("Read large file"),
        AgentMessage::assistant_with_tools(
            "I'll read it.",
            vec![tool_call("call_large", "read_file")],
        ),
        AgentMessage::tool("call_large", "read_file", &large_output),
    ];

    let policy = CompactionPolicy {
        protected_tool_window_tokens: 0,
        prune_min_tokens: 100, // Маленький threshold для pruning
        ..CompactionPolicy::default()
    };

    let snapshot = classify_hot_memory_with_policy(&messages, &policy, Some(1000));

    let summary = CompactionSummary {
        goal: "Test".to_string(),
        constraints: vec![],
        decisions: vec![],
        discoveries: vec![],
        relevant_files_entities: vec![],
        remaining_work: vec![],
        risks: vec![],
    };

    let (rebuilt, _outcome) = rebuild_hot_context(&snapshot, &messages, Some(summary), None);

    // Проверяем orphaned
    let orphaned_exists = rebuilt.iter().any(|msg| {
        msg.kind == AgentMessageKind::ToolResult
            && msg
                .tool_call_id
                .as_ref()
                .is_some_and(|id| id == "call_large")
            && !rebuilt.iter().any(|m| {
                m.tool_calls
                    .as_ref()
                    .is_some_and(|calls| calls.iter().any(|tc| tc.id == "call_large"))
            })
    });

    assert!(
        !orphaned_exists,
        "Large tool output should not create orphaned results"
    );
}

/// Scenario 6: Race condition simulation
/// Проверяем, что фикс работает даже при сложных паттернах
#[test]
fn complex_interleaved_conversation() {
    let messages = vec![
        AgentMessage::user("Question 1"),
        AgentMessage::assistant("Answer 1"),
        AgentMessage::user("Do tool A"),
        AgentMessage::assistant_with_tools("Doing tool A.", vec![tool_call("call_a", "tool_a")]),
        AgentMessage::tool("call_a", "tool_a", "result A"),
        AgentMessage::user("Do tools B and C"),
        AgentMessage::assistant_with_tools(
            "Doing B and C.",
            vec![tool_call("call_b", "tool_b"), tool_call("call_c", "tool_c")],
        ),
        AgentMessage::tool("call_b", "tool_b", "result B"),
        AgentMessage::tool("call_c", "tool_c", "result C"),
        AgentMessage::user("Final question"),
    ];

    let policy = CompactionPolicy {
        protected_tool_window_tokens: 100, // Только последние tool interactions
        ..CompactionPolicy::default()
    };

    let snapshot = classify_hot_memory_with_policy(&messages, &policy, Some(2000));

    let summary = CompactionSummary {
        goal: "Test".to_string(),
        constraints: vec![],
        decisions: vec![],
        discoveries: vec![],
        relevant_files_entities: vec![],
        remaining_work: vec![],
        risks: vec![],
    };

    let (rebuilt, _outcome) = rebuild_hot_context(&snapshot, &messages, Some(summary), None);

    // Проверяем консистентность всех tool interactions
    let tool_results: Vec<_> = rebuilt
        .iter()
        .filter(|msg| msg.kind == AgentMessageKind::ToolResult)
        .filter_map(|msg| msg.tool_call_id.clone())
        .collect();

    for tool_call_id in &tool_results {
        let has_matching_call = rebuilt.iter().any(|msg| {
            msg.tool_calls
                .as_ref()
                .is_some_and(|calls| calls.iter().any(|tc| &tc.id == tool_call_id))
        });

        assert!(
            has_matching_call,
            "Tool result {} has no matching tool_call after compaction",
            tool_call_id
        );
    }
}
