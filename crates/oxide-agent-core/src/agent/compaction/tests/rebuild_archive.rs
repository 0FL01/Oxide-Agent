use super::fixtures::{fallback_summarizer_service, manual_request};
use crate::agent::compaction::AgentMessageKind;
use crate::agent::memory::AgentMessage;
use crate::agent::providers::{TodoItem, TodoStatus};
use crate::agent::{AgentContext, EphemeralSession};
use crate::llm::{ToolCall, ToolCallFunction};

#[tokio::test]
async fn manual_compaction_preserves_pinned_live_context_todos_and_recent_turns() {
    let mut session = EphemeralSession::new(256);
    session.memory_mut().todos.update(vec![
        TodoItem {
            description: "Keep current task preserved".to_string(),
            status: TodoStatus::InProgress,
        },
        TodoItem {
            description: "Verify compaction layout".to_string(),
            status: TodoStatus::Pending,
        },
    ]);

    for message in [
        AgentMessage::topic_agents_md("# Topic AGENTS\nPreserve operator instructions."),
        AgentMessage::system_context("Base execution policy"),
        AgentMessage::user_task("Ship stage 1"),
        AgentMessage::runtime_context("User asked to keep the active task."),
        AgentMessage::approval_replay("Replay the approved SSH action exactly once."),
        AgentMessage::skill_context("[Loaded skill: release]"),
        AgentMessage::infra_status("SSH target validated"),
        AgentMessage::user("Older request about compaction hardening."),
        AgentMessage::assistant_with_reasoning(
            "Older response with findings.",
            "Need to preserve all protected live state.",
        ),
        AgentMessage::user("Recent request 1."),
        AgentMessage::assistant("Recent response 1."),
        AgentMessage::user("Recent request 2."),
        AgentMessage::assistant("Recent response 2."),
    ] {
        session.memory_mut().add_message(message);
    }

    let expected_todos = session.memory().todos.clone();
    let service = fallback_summarizer_service();
    let outcome = service
        .prepare_for_run(&manual_request("Ship stage 1"), &mut session)
        .await
        .expect("manual compaction succeeds");

    assert!(outcome.applied);
    assert!(outcome.summary_generation.attempted);
    assert!(outcome.summary_generation.used_fallback);
    assert!(outcome.rebuild.applied);
    assert_eq!(outcome.archive_persistence.archived_chunk_count, 1);

    let actual_todos: Vec<(&str, TodoStatus)> = session
        .memory()
        .todos
        .items
        .iter()
        .map(|item| (item.description.as_str(), item.status.clone()))
        .collect();
    let expected_todos: Vec<(&str, TodoStatus)> = expected_todos
        .items
        .iter()
        .map(|item| (item.description.as_str(), item.status.clone()))
        .collect();
    assert_eq!(actual_todos, expected_todos);
    assert_eq!(
        session
            .memory()
            .todos
            .current_task()
            .map(|item| item.description.as_str()),
        Some("Keep current task preserved")
    );

    let messages = session.memory().get_messages();
    let kinds: Vec<AgentMessageKind> = messages.iter().map(AgentMessage::resolved_kind).collect();
    assert_eq!(
        kinds,
        vec![
            AgentMessageKind::TopicAgentsMd,
            AgentMessageKind::SystemContext,
            AgentMessageKind::UserTask,
            AgentMessageKind::RuntimeContext,
            AgentMessageKind::ApprovalReplay,
            AgentMessageKind::SkillContext,
            AgentMessageKind::InfraStatus,
            AgentMessageKind::Summary,
            AgentMessageKind::ArchiveReference,
            AgentMessageKind::UserTurn,
            AgentMessageKind::AssistantResponse,
            AgentMessageKind::UserTurn,
            AgentMessageKind::AssistantResponse,
        ]
    );
    assert_eq!(messages[9].content, "Recent request 1.");
    assert_eq!(messages[10].content, "Recent response 1.");
    assert_eq!(messages[11].content, "Recent request 2.");
    assert_eq!(messages[12].content, "Recent response 2.");
    assert!(messages.iter().all(|message| {
        !message
            .content
            .contains("Older request about compaction hardening.")
    }));
}

#[tokio::test]
async fn manual_compaction_preserves_recent_raw_window_order_with_tool_interactions() {
    let mut session = EphemeralSession::new(256);
    for message in [
        AgentMessage::user_task("Investigate tool-heavy stage 1 run"),
        AgentMessage::user("Older request that should be compacted."),
        AgentMessage::assistant("Older response that should be compacted."),
        AgentMessage::assistant_with_tools(
            "Calling search",
            vec![ToolCall::new(
                "call-1".to_string(),
                ToolCallFunction {
                    name: "search".to_string(),
                    arguments: "{}".to_string(),
                },
                false,
            )],
        ),
        AgentMessage::tool("call-1", "search", "search result 1"),
        AgentMessage::user("Recent request 1."),
        AgentMessage::assistant("Recent response 1."),
        AgentMessage::assistant_with_tools(
            "Calling read_file",
            vec![ToolCall::new(
                "call-2".to_string(),
                ToolCallFunction {
                    name: "read_file".to_string(),
                    arguments: "{}".to_string(),
                },
                false,
            )],
        ),
        AgentMessage::tool("call-2", "read_file", "file result 2"),
        AgentMessage::user("Recent request 2."),
        AgentMessage::assistant("Recent response 2."),
    ] {
        session.memory_mut().add_message(message);
    }

    let service = fallback_summarizer_service();
    let outcome = service
        .prepare_for_run(
            &manual_request("Investigate tool-heavy stage 1 run"),
            &mut session,
        )
        .await
        .expect("manual compaction succeeds");

    assert!(outcome.applied);
    assert!(outcome.rebuild.applied);

    let messages = session.memory().get_messages();
    let tail: Vec<(AgentMessageKind, String)> = messages[3..]
        .iter()
        .map(|message| (message.resolved_kind(), message.content.clone()))
        .collect();
    assert_eq!(
        tail,
        vec![
            (
                AgentMessageKind::AssistantToolCall,
                "Calling search".to_string()
            ),
            (AgentMessageKind::ToolResult, "search result 1".to_string()),
            (AgentMessageKind::UserTurn, "Recent request 1.".to_string()),
            (
                AgentMessageKind::AssistantResponse,
                "Recent response 1.".to_string()
            ),
            (
                AgentMessageKind::AssistantToolCall,
                "Calling read_file".to_string()
            ),
            (AgentMessageKind::ToolResult, "file result 2".to_string()),
            (AgentMessageKind::UserTurn, "Recent request 2.".to_string()),
            (
                AgentMessageKind::AssistantResponse,
                "Recent response 2.".to_string()
            ),
        ]
    );
}

#[tokio::test]
async fn repeated_manual_compaction_is_idempotent_and_does_not_duplicate_summary_or_archive_ref() {
    let mut session = EphemeralSession::new(256);
    for message in [
        AgentMessage::topic_agents_md("# Topic AGENTS\nKeep identity stable."),
        AgentMessage::user_task("Ship stage 1 safely"),
        AgentMessage::user("Older request to compact."),
        AgentMessage::assistant("Older response to compact."),
        AgentMessage::user("Recent request 1."),
        AgentMessage::assistant("Recent response 1."),
        AgentMessage::user("Recent request 2."),
        AgentMessage::assistant("Recent response 2."),
    ] {
        session.memory_mut().add_message(message);
    }

    let service = fallback_summarizer_service();
    let request = manual_request("Ship stage 1 safely");

    let first = service
        .prepare_for_run(&request, &mut session)
        .await
        .expect("first compaction succeeds");
    assert!(first.applied);

    let layout_after_first: Vec<(AgentMessageKind, String)> = session
        .memory()
        .get_messages()
        .iter()
        .map(|message| (message.resolved_kind(), message.content.clone()))
        .collect();

    let second = service
        .prepare_for_run(&request, &mut session)
        .await
        .expect("second compaction succeeds");

    assert!(!second.applied);
    assert!(!second.summary_generation.attempted);
    assert!(!second.archive_persistence.attempted);
    assert!(!second.rebuild.applied);

    let messages = session.memory().get_messages();
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.resolved_kind() == AgentMessageKind::Summary)
            .count(),
        1
    );
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.resolved_kind() == AgentMessageKind::ArchiveReference)
            .count(),
        1
    );

    let layout_after_second: Vec<(AgentMessageKind, String)> = messages
        .iter()
        .map(|message| (message.resolved_kind(), message.content.clone()))
        .collect();
    assert_eq!(layout_after_second, layout_after_first);
}
