//! Тест на orphaned tool results - воспроизведение дефекта MiniMax
//!
//! Дефект: при compaction удаляется assistant message с tool_calls,
//! но соответствующий tool result остается, создавая невалидный контекст.
//! Это приводит к ошибке MiniMax: "tool result's tool id ... not found"

use oxide_agent_core::agent::compaction::{
    classify_hot_memory_with_policy, rebuild_hot_context, AgentMessageKind, CompactionPolicy,
    CompactionSummary,
};
use oxide_agent_core::agent::memory::AgentMessage;
use oxide_agent_core::llm::{ToolCall, ToolCallFunction};

/// Создает тестовый ToolCall с заданным id
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

/// Проверяет, что tool_call_id присутствует в сообщениях
fn has_tool_call_id(messages: &[AgentMessage], tool_call_id: &str) -> bool {
    messages.iter().any(|msg| {
        msg.tool_call_id
            .as_ref()
            .map(|id| id == tool_call_id)
            .unwrap_or(false)
    })
}

/// Проверяет, что assistant message с данным tool_call_id присутствует
fn has_assistant_with_tool_call(messages: &[AgentMessage], tool_call_id: &str) -> bool {
    messages.iter().any(|msg| {
        if msg.kind == AgentMessageKind::AssistantToolCall {
            msg.tool_calls
                .as_ref()
                .map(|calls| calls.iter().any(|tc| tc.id == tool_call_id))
                .unwrap_or(false)
        } else {
            false
        }
    })
}

#[test]
fn compaction_creates_orphaned_tool_results() {
    // Arrange: Создаем историю с tool call и результатом
    let messages = vec![
        AgentMessage::user("What's the weather in Tokyo?"),
        AgentMessage::assistant_with_tools(
            "I'll check the weather for you.",
            vec![tool_call("call_abc123", "get_weather")],
        ),
        AgentMessage::tool(
            "call_abc123",
            "get_weather",
            r#"{"temperature": 22, "condition": "sunny"}"#,
        ),
    ];

    // Проверяем начальное состояние
    assert!(
        has_assistant_with_tool_call(&messages, "call_abc123"),
        "Initial state should have assistant with tool_call"
    );
    assert!(
        has_tool_call_id(&messages, "call_abc123"),
        "Initial state should have tool result with tool_call_id"
    );

    // Act: Применяем compaction с политикой, которая удалит старые сообщения
    let policy = CompactionPolicy {
        protected_tool_window_tokens: 0, // Минимальный protection window
        ..CompactionPolicy::default()
    };

    let snapshot = classify_hot_memory_with_policy(
        &messages,
        &policy,
        Some(1000), // Маленький контекст window для forced compaction
    );

    let summary = CompactionSummary {
        goal: "Summary of previous conversation".to_string(),
        constraints: vec![],
        decisions: vec![],
        discoveries: vec![],
        relevant_files_entities: vec![],
        remaining_work: vec![],
        risks: vec![],
    };

    let (rebuilt, outcome) = rebuild_hot_context(&snapshot, &messages, Some(summary), None);

    // Assert: Проверяем, что compaction что-то удалил
    assert!(
        outcome.applied,
        "Compaction should have been applied with these settings"
    );
    assert!(
        !outcome.dropped_indices.is_empty(),
        "Some messages should have been dropped"
    );

    // CRITICAL: Проверяем наличие orphaned tool results
    let has_orphaned_tool_result = has_tool_call_id(&rebuilt, "call_abc123")
        && !has_assistant_with_tool_call(&rebuilt, "call_abc123");

    println!("Rebuilt messages:");
    for (i, msg) in rebuilt.iter().enumerate() {
        println!(
            "  [{}] {:?} - kind: {:?}, tool_call_id: {:?}",
            i, msg.role, msg.kind, msg.tool_call_id
        );
    }
    println!("Dropped indices: {:?}", outcome.dropped_indices);
    println!("Has orphaned tool result: {}", has_orphaned_tool_result);

    // Этот assertion покажет дефект - он должен упасть до фикса
    if has_orphaned_tool_result {
        panic!(
            "DEFECT DETECTED: Compaction created orphaned tool result!\n\
             Tool result references 'call_abc123' but corresponding assistant message was dropped.\n\
             This causes MiniMax error: 'tool result's tool id(call_abc123) not found'"
        );
    }
}

#[test]
fn compaction_preserves_tool_pairs_in_recent_window() {
    // Arrange: Создаем историю, где tool call должен остаться в recent window
    let messages = vec![
        AgentMessage::user("Old question 1"),
        AgentMessage::assistant("Old answer 1"),
        AgentMessage::user("What's the weather in Tokyo?"),
        AgentMessage::assistant_with_tools(
            "I'll check the weather for you.",
            vec![tool_call("call_recent", "get_weather")],
        ),
        AgentMessage::tool("call_recent", "get_weather", r#"{"temperature": 22}"#),
    ];

    // Act: Применяем compaction с большим protection window
    let policy = CompactionPolicy {
        protected_tool_window_tokens: 10000, // Большой protection window
        ..CompactionPolicy::default()
    };

    let snapshot = classify_hot_memory_with_policy(
        &messages,
        &policy,
        Some(100000), // Большой контекст
    );

    let (rebuilt, _outcome) = rebuild_hot_context(
        &snapshot, &messages, None, // No summary needed
        None,
    );

    // Assert: Tool interaction должен остаться в recent window
    assert!(
        has_assistant_with_tool_call(&rebuilt, "call_recent"),
        "Recent assistant with tool_call should be preserved"
    );
    assert!(
        has_tool_call_id(&rebuilt, "call_recent"),
        "Recent tool result should be preserved"
    );

    // Проверяем, что нет orphaned results
    let has_orphaned = has_tool_call_id(&rebuilt, "call_recent")
        && !has_assistant_with_tool_call(&rebuilt, "call_recent");

    assert!(
        !has_orphaned,
        "Should not have orphaned tool results when pair is preserved"
    );
}
