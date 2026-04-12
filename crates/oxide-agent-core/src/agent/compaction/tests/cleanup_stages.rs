use super::fixtures::{
    cleanup_policy, manual_request, pre_iteration_request, ErrorPayloadSink, RecordingArchiveSink,
    RecordingPayloadSink,
};
use crate::agent::compaction::CompactionPolicy;
use crate::agent::memory::AgentMessage;
use crate::agent::{AgentContext, CompactionService, EphemeralSession};
use crate::llm::{ToolCall, ToolCallFunction};
use std::sync::Arc;

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

#[tokio::test]
async fn cleanup_externalizes_recent_large_tool_result_without_pruning_recent_window() {
    let payload_sink = Arc::new(RecordingPayloadSink::new(true));
    let archive_sink = Arc::new(RecordingArchiveSink::default());
    let service = CompactionService::new(cleanup_policy())
        .with_payload_sink(payload_sink.clone())
        .with_archive_sink(archive_sink.clone());
    let mut session = EphemeralSession::new(20_000);

    for message in [
        AgentMessage::user_task("Inspect fresh tool output"),
        AgentMessage::assistant_with_tools(
            "Call read_file",
            vec![tool_call("call-1", "read_file")],
        ),
        AgentMessage::tool("call-1", "read_file", &"A".repeat(512)),
    ] {
        session.memory_mut().add_message(message);
    }

    let outcome = service
        .prepare_for_run(&manual_request("Inspect fresh tool output"), &mut session)
        .await
        .expect("cleanup succeeds");

    assert!(outcome.externalization.applied);
    assert_eq!(outcome.externalization.externalized_count, 1);
    assert!(!outcome.pruning.applied);

    let tool_message = &session.memory().get_messages()[2];
    assert!(tool_message.is_externalized());
    assert!(!tool_message.is_pruned());
    assert!(tool_message.content.contains("[externalized tool result]"));
    assert_eq!(payload_sink.records().len(), 1);
    assert_eq!(archive_sink.records().len(), 1);
}

#[tokio::test]
async fn cleanup_prunes_old_externalized_tool_and_preserves_archive_ref() {
    let payload_sink = Arc::new(RecordingPayloadSink::new(true));
    let archive_sink = Arc::new(RecordingArchiveSink::default());
    let service = CompactionService::new(cleanup_policy())
        .with_payload_sink(payload_sink.clone())
        .with_archive_sink(archive_sink.clone());
    let mut session = EphemeralSession::new(20_000);

    for message in [
        AgentMessage::user_task("Inspect stale tool output"),
        AgentMessage::assistant_with_tools(
            "Call read_file",
            vec![tool_call("call-1", "read_file")],
        ),
        AgentMessage::tool("call-1", "read_file", &"A".repeat(512)),
        AgentMessage::summary("[Previous context compressed]\n- earlier work preserved"),
        AgentMessage::assistant_with_tools("Call search 1", vec![tool_call("call-2", "search")]),
        AgentMessage::tool("call-2", "search", "short-1"),
        AgentMessage::assistant_with_tools("Call search 2", vec![tool_call("call-3", "search")]),
        AgentMessage::tool("call-3", "search", "short-2"),
        AgentMessage::assistant_with_tools("Call search 3", vec![tool_call("call-4", "search")]),
        AgentMessage::tool("call-4", "search", "short-3"),
        AgentMessage::assistant_with_tools("Call search 4", vec![tool_call("call-5", "search")]),
        AgentMessage::tool("call-5", "search", "short-4"),
    ] {
        session.memory_mut().add_message(message);
    }

    let outcome = service
        .prepare_for_run(&manual_request("Inspect stale tool output"), &mut session)
        .await
        .expect("cleanup succeeds");

    assert!(outcome.externalization.applied);
    assert!(outcome.pruning.applied);
    assert_eq!(outcome.externalization.externalized_count, 1);
    assert_eq!(outcome.pruning.pruned_count, 1);

    let tool_message = &session.memory().get_messages()[2];
    let externalized_payload = tool_message
        .externalized_payload
        .as_ref()
        .expect("externalized payload retained");
    let pruned_artifact = tool_message
        .pruned_artifact
        .as_ref()
        .expect("pruned artifact retained");
    assert!(tool_message.is_externalized());
    assert!(tool_message.is_pruned());
    assert!(tool_message.content.contains("[pruned tool result]"));
    assert_eq!(
        pruned_artifact.archive_ref.as_ref(),
        Some(&externalized_payload.archive_ref)
    );
    assert_eq!(payload_sink.records().len(), 1);
    assert_eq!(archive_sink.records().len(), 1);
}

#[tokio::test]
async fn cleanup_is_idempotent_after_externalize_and_prune() {
    let payload_sink = Arc::new(RecordingPayloadSink::new(true));
    let archive_sink = Arc::new(RecordingArchiveSink::default());
    let service = CompactionService::new(cleanup_policy())
        .with_payload_sink(payload_sink.clone())
        .with_archive_sink(archive_sink.clone());
    let mut session = EphemeralSession::new(20_000);

    for message in [
        AgentMessage::user_task("Inspect repeated cleanup"),
        AgentMessage::assistant_with_tools(
            "Call read_file",
            vec![tool_call("call-1", "read_file")],
        ),
        AgentMessage::tool("call-1", "read_file", &"A".repeat(512)),
        AgentMessage::assistant_with_tools("Call search 1", vec![tool_call("call-2", "search")]),
        AgentMessage::tool("call-2", "search", "short-1"),
        AgentMessage::assistant_with_tools("Call search 2", vec![tool_call("call-3", "search")]),
        AgentMessage::tool("call-3", "search", "short-2"),
        AgentMessage::assistant_with_tools("Call search 3", vec![tool_call("call-4", "search")]),
        AgentMessage::tool("call-4", "search", "short-3"),
        AgentMessage::assistant_with_tools("Call search 4", vec![tool_call("call-5", "search")]),
        AgentMessage::tool("call-5", "search", "short-4"),
    ] {
        session.memory_mut().add_message(message);
    }

    let first = service
        .prepare_for_run(&manual_request("Inspect repeated cleanup"), &mut session)
        .await
        .expect("first cleanup succeeds");
    assert!(first.applied);

    let layout_after_first: Vec<String> = session
        .memory()
        .get_messages()
        .iter()
        .map(|message| message.content.clone())
        .collect();

    let second = service
        .prepare_for_run(&manual_request("Inspect repeated cleanup"), &mut session)
        .await
        .expect("second cleanup succeeds");

    assert!(!second.applied);
    assert!(!second.externalization.applied);
    assert!(!second.pruning.applied);
    assert_eq!(payload_sink.records().len(), 1);
    assert_eq!(archive_sink.records().len(), 1);

    let layout_after_second: Vec<String> = session
        .memory()
        .get_messages()
        .iter()
        .map(|message| message.content.clone())
        .collect();
    assert_eq!(layout_after_second, layout_after_first);
}

#[tokio::test]
async fn cleanup_preserves_inline_fallback_when_pruning_after_non_persisted_externalization() {
    let payload_sink = Arc::new(RecordingPayloadSink::new(false));
    let archive_sink = Arc::new(RecordingArchiveSink::default());
    let service = CompactionService::new(cleanup_policy())
        .with_payload_sink(payload_sink.clone())
        .with_archive_sink(archive_sink.clone());
    let mut session = EphemeralSession::new(20_000);
    let original_payload = "A".repeat(512);

    for message in [
        AgentMessage::user_task("Inspect fallback preservation"),
        AgentMessage::assistant_with_tools(
            "Call read_file",
            vec![tool_call("call-1", "read_file")],
        ),
        AgentMessage::tool("call-1", "read_file", &original_payload),
        AgentMessage::summary("[Previous context compressed]\n- earlier work preserved"),
        AgentMessage::assistant_with_tools("Call search 1", vec![tool_call("call-2", "search")]),
        AgentMessage::tool("call-2", "search", "short-1"),
        AgentMessage::assistant_with_tools("Call search 2", vec![tool_call("call-3", "search")]),
        AgentMessage::tool("call-3", "search", "short-2"),
        AgentMessage::assistant_with_tools("Call search 3", vec![tool_call("call-4", "search")]),
        AgentMessage::tool("call-4", "search", "short-3"),
        AgentMessage::assistant_with_tools("Call search 4", vec![tool_call("call-5", "search")]),
        AgentMessage::tool("call-5", "search", "short-4"),
    ] {
        session.memory_mut().add_message(message);
    }

    let outcome = service
        .prepare_for_run(
            &manual_request("Inspect fallback preservation"),
            &mut session,
        )
        .await
        .expect("cleanup succeeds");

    assert!(outcome.externalization.applied);
    assert!(outcome.pruning.applied);

    let tool_message = &session.memory().get_messages()[2];
    let externalized_payload = tool_message
        .externalized_payload
        .as_ref()
        .expect("externalized payload retained");
    let pruned_artifact = tool_message
        .pruned_artifact
        .as_ref()
        .expect("pruned artifact retained");
    assert_eq!(
        externalized_payload.inline_fallback.as_deref(),
        Some(original_payload.as_str())
    );
    assert_eq!(
        pruned_artifact.archive_ref.as_ref(),
        Some(&externalized_payload.archive_ref)
    );
    assert_eq!(payload_sink.records().len(), 1);
    assert_eq!(archive_sink.records().len(), 1);
}

#[tokio::test]
async fn cleanup_leaves_message_untouched_when_payload_sink_errors() {
    let archive_sink = Arc::new(RecordingArchiveSink::default());
    let service = CompactionService::new(cleanup_policy())
        .with_payload_sink(Arc::new(ErrorPayloadSink))
        .with_archive_sink(archive_sink.clone());
    let mut session = EphemeralSession::new(20_000);
    let original_payload = "A".repeat(512);

    for message in [
        AgentMessage::user_task("Inspect sink failure"),
        AgentMessage::assistant_with_tools(
            "Call read_file",
            vec![tool_call("call-1", "read_file")],
        ),
        AgentMessage::tool("call-1", "read_file", &original_payload),
    ] {
        session.memory_mut().add_message(message);
    }

    let outcome = service
        .prepare_for_run(&pre_iteration_request("Inspect sink failure"), &mut session)
        .await
        .expect("cleanup succeeds despite sink error");

    assert!(!outcome.applied);
    assert!(!outcome.externalization.applied);
    assert!(!outcome.pruning.applied);
    let tool_message = &session.memory().get_messages()[2];
    assert_eq!(tool_message.content, original_payload);
    assert!(!tool_message.is_externalized());
    assert!(!tool_message.is_pruned());
    assert!(archive_sink.records().is_empty());
}

#[tokio::test]
async fn cleanup_collapses_failed_retry_chain_before_success() {
    let service = CompactionService::new(CompactionPolicy {
        externalize_threshold_tokens: usize::MAX,
        externalize_threshold_chars: usize::MAX,
        prune_min_tokens: usize::MAX,
        prune_min_chars: usize::MAX,
        protected_tool_window_tokens: 1,
        ..CompactionPolicy::default()
    });
    let mut session = EphemeralSession::new(20_000);

    for message in [
        AgentMessage::assistant_with_tools(
            "Call grep attempt 1",
            vec![tool_call("call-1", "execute_command")],
        ),
        AgentMessage::tool(
            "call-1",
            "execute_command",
            "Command failed (exit code 2): grep: invalid option -- 'Q'",
        ),
        AgentMessage::assistant_with_tools(
            "Call grep attempt 2",
            vec![tool_call("call-2", "execute_command")],
        ),
        AgentMessage::tool(
            "call-2",
            "execute_command",
            "Command failed (exit code 2): grep: invalid option -- 'P'",
        ),
        AgentMessage::assistant_with_tools(
            "Call grep attempt 3",
            vec![tool_call("call-3", "execute_command")],
        ),
        AgentMessage::tool("call-3", "execute_command", "src/main.rs\nsrc/lib.rs"),
        AgentMessage::assistant_with_tools(
            "Keep recent raw window",
            vec![tool_call("call-4", "search")],
        ),
        AgentMessage::tool("call-4", "search", "recent result"),
    ] {
        session.memory_mut().add_message(message);
    }

    let outcome = service
        .prepare_for_run(&manual_request("Collapse stale retries"), &mut session)
        .await
        .expect("cleanup succeeds");

    assert!(outcome.applied);
    assert!(outcome.error_retry_collapse.applied);
    assert_eq!(outcome.error_retry_collapse.collapsed_attempt_count, 2);
    assert_eq!(
        outcome.error_retry_collapse.dropped_indices,
        vec![0, 1, 2, 3]
    );

    let messages = session.memory().get_messages();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].content, "Call grep attempt 3");
    assert_eq!(messages[1].content, "src/main.rs\nsrc/lib.rs");
}
