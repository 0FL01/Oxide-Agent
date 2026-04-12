//! Deterministic cleanup for stale tool error/retry chains.

use super::budget::estimate_message_tokens;
use super::types::{AgentMessageKind, CompactionSnapshot, ErrorRetryCollapseOutcome};
use crate::agent::memory::AgentMessage;
use serde_json::Value;
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttemptStatus {
    Success,
    Failure,
    Unknown,
}

#[derive(Debug, Clone)]
struct ToolAttempt {
    assistant_index: usize,
    result_index: usize,
    tool_name: String,
    status: AttemptStatus,
}

/// Remove contiguous failed retries when a later attempt of the same tool succeeds.
#[must_use]
pub fn collapse_error_retries(
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
    allow_without_summary_boundary: bool,
) -> (Vec<AgentMessage>, ErrorRetryCollapseOutcome) {
    let latest_summary_boundary = snapshot
        .entries
        .iter()
        .rev()
        .find(|entry| entry.kind == AgentMessageKind::Summary)
        .map(|entry| entry.index);

    if latest_summary_boundary.is_none() && !allow_without_summary_boundary {
        return (messages.to_vec(), ErrorRetryCollapseOutcome::default());
    }

    let attempts = collect_attempts(snapshot, messages, latest_summary_boundary);
    if attempts.len() < 2 {
        return (messages.to_vec(), ErrorRetryCollapseOutcome::default());
    }

    let mut drop_indices = vec![false; messages.len()];
    let mut collapsed_attempt_count = 0usize;

    let mut start = 0usize;
    while start < attempts.len() {
        let mut end = start + 1;
        while end < attempts.len()
            && attempts[end - 1].result_index + 1 == attempts[end].assistant_index
            && attempts[end - 1].tool_name == attempts[end].tool_name
        {
            end += 1;
        }

        let group = &attempts[start..end];
        if group.len() >= 2
            && group
                .last()
                .is_some_and(|attempt| attempt.status == AttemptStatus::Success)
            && group[..group.len() - 1]
                .iter()
                .all(|attempt| attempt.status == AttemptStatus::Failure)
        {
            for attempt in &group[..group.len() - 1] {
                drop_indices[attempt.assistant_index] = true;
                drop_indices[attempt.result_index] = true;
                collapsed_attempt_count = collapsed_attempt_count.saturating_add(1);
            }
        }

        start = end;
    }

    let dropped_indices: Vec<usize> = drop_indices
        .iter()
        .enumerate()
        .filter_map(|(index, should_drop)| should_drop.then_some(index))
        .collect();
    if dropped_indices.is_empty() {
        return (messages.to_vec(), ErrorRetryCollapseOutcome::default());
    }

    let reclaimed_tokens = dropped_indices
        .iter()
        .filter_map(|index| messages.get(*index))
        .map(estimate_message_tokens)
        .sum();
    let reclaimed_chars = dropped_indices
        .iter()
        .filter_map(|index| messages.get(*index))
        .map(|message| message.content.chars().count())
        .sum();
    let rewritten: Vec<AgentMessage> = messages
        .iter()
        .enumerate()
        .filter(|(index, _)| !drop_indices[*index])
        .map(|(_, message)| message.clone())
        .collect();

    warn!(
        collapsed_attempt_count,
        dropped_message_count = dropped_indices.len(),
        reclaimed_tokens,
        reclaimed_chars,
        dropped_indices = ?dropped_indices,
        "Compaction collapsed stale tool error retries"
    );

    (
        rewritten,
        ErrorRetryCollapseOutcome {
            applied: true,
            collapsed_attempt_count,
            dropped_message_count: dropped_indices.len(),
            reclaimed_tokens,
            reclaimed_chars,
            dropped_indices,
        },
    )
}

fn collect_attempts(
    snapshot: &CompactionSnapshot,
    messages: &[AgentMessage],
    latest_summary_boundary: Option<usize>,
) -> Vec<ToolAttempt> {
    let mut attempts = Vec::new();
    let mut index = 0usize;

    while index + 1 < snapshot.entries.len() {
        let assistant_entry = &snapshot.entries[index];
        let result_entry = &snapshot.entries[index + 1];

        if assistant_entry.kind != AgentMessageKind::AssistantToolCall
            || result_entry.kind != AgentMessageKind::ToolResult
            || assistant_entry.preserve_in_raw_window
            || result_entry.preserve_in_raw_window
            || latest_summary_boundary.is_some_and(|boundary| {
                assistant_entry.index >= boundary || result_entry.index >= boundary
            })
        {
            index += 1;
            continue;
        }

        let Some(assistant_message) = messages.get(assistant_entry.index) else {
            index += 1;
            continue;
        };
        let Some(result_message) = messages.get(result_entry.index) else {
            index += 1;
            continue;
        };

        let Some(tool_calls) = assistant_message.tool_calls.as_ref() else {
            index += 1;
            continue;
        };
        if tool_calls.len() != 1 {
            index += 1;
            continue;
        }

        let Some(correlations) = assistant_message.resolved_tool_call_correlations() else {
            index += 1;
            continue;
        };
        if correlations.len() != 1 {
            index += 1;
            continue;
        }

        let expected_invocation = &correlations[0].invocation_id;
        let Some(result_correlation) = result_message.resolved_tool_call_correlation() else {
            index += 1;
            continue;
        };
        if &result_correlation.invocation_id != expected_invocation {
            index += 1;
            continue;
        }

        let tool_name = tool_calls[0].function.name.clone();
        if result_message.tool_name.as_deref() != Some(tool_name.as_str()) {
            index += 1;
            continue;
        }

        attempts.push(ToolAttempt {
            assistant_index: assistant_entry.index,
            result_index: result_entry.index,
            status: classify_attempt_status(&tool_name, result_message),
            tool_name,
        });
        index += 2;
    }

    attempts
}

fn classify_attempt_status(tool_name: &str, result_message: &AgentMessage) -> AttemptStatus {
    if result_message.is_pruned() || result_message.is_externalized() {
        return AttemptStatus::Unknown;
    }

    let content = result_message.content.trim();
    if content.is_empty() {
        return AttemptStatus::Unknown;
    }

    if let Some(status) = classify_json_status(content) {
        return status;
    }

    if matches_plain_failure(content) {
        return AttemptStatus::Failure;
    }

    match tool_name {
        "execute_command" => AttemptStatus::Success,
        "read_file" if !content.starts_with("Error reading file:") => AttemptStatus::Success,
        "write_file" if !content.starts_with("Error writing file:") => AttemptStatus::Success,
        _ => AttemptStatus::Unknown,
    }
}

fn classify_json_status(content: &str) -> Option<AttemptStatus> {
    let value = serde_json::from_str::<Value>(content).ok()?;
    let object = value.as_object()?;

    if object
        .get("approval_required")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some(AttemptStatus::Unknown);
    }

    if let Some(exit_code) = object.get("exit_code").and_then(Value::as_i64) {
        return Some(if exit_code == 0 {
            AttemptStatus::Success
        } else {
            AttemptStatus::Failure
        });
    }

    if let Some(ok) = object.get("ok").and_then(Value::as_bool) {
        return Some(if ok {
            AttemptStatus::Success
        } else {
            AttemptStatus::Failure
        });
    }

    if let Some(status) = object.get("status").and_then(Value::as_str) {
        return Some(match status {
            "ok" | "success" | "completed" => AttemptStatus::Success,
            "error" | "failed" | "failure" | "timeout" => AttemptStatus::Failure,
            _ => AttemptStatus::Unknown,
        });
    }

    None
}

fn matches_plain_failure(content: &str) -> bool {
    content.starts_with("Tool execution error:")
        || content.starts_with("Command execution failed:")
        || content.starts_with("Command failed (exit code ")
        || (content.starts_with("Tool '") && content.contains("' timed out ("))
}

#[cfg(test)]
mod tests {
    use super::collapse_error_retries;
    use crate::agent::compaction::{classify_hot_memory_with_policy, CompactionPolicy};
    use crate::agent::memory::AgentMessage;
    use crate::llm::{ToolCall, ToolCallFunction};

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
    fn collapse_error_retries_removes_failed_execute_command_attempts_before_success() {
        let policy = CompactionPolicy {
            protected_tool_window_tokens: 1,
            ..CompactionPolicy::default()
        };
        let messages = vec![
            AgentMessage::assistant_with_tools(
                "try-1",
                vec![tool_call("call-1", "execute_command")],
            ),
            AgentMessage::tool(
                "call-1",
                "execute_command",
                "Command failed (exit code 2): grep: invalid option -- 'Q'",
            ),
            AgentMessage::assistant_with_tools(
                "try-2",
                vec![tool_call("call-2", "execute_command")],
            ),
            AgentMessage::tool(
                "call-2",
                "execute_command",
                "Command failed (exit code 2): grep: invalid option -- 'P'",
            ),
            AgentMessage::assistant_with_tools(
                "try-3",
                vec![tool_call("call-3", "execute_command")],
            ),
            AgentMessage::tool("call-3", "execute_command", "src/main.rs\nsrc/lib.rs"),
            AgentMessage::summary("[Previous context compressed]\n- older work"),
            AgentMessage::assistant_with_tools("recent", vec![tool_call("call-4", "search")]),
            AgentMessage::tool("call-4", "search", "recent result"),
        ];

        let snapshot = classify_hot_memory_with_policy(&messages, &policy, None);
        let (rewritten, outcome) = collapse_error_retries(&snapshot, &messages, false);

        assert!(outcome.applied);
        assert_eq!(outcome.collapsed_attempt_count, 2);
        assert_eq!(outcome.dropped_message_count, 4);
        assert_eq!(outcome.dropped_indices, vec![0, 1, 2, 3]);
        assert_eq!(rewritten.len(), 5);
        assert_eq!(rewritten[0].content, "try-3");
        assert_eq!(rewritten[1].content, "src/main.rs\nsrc/lib.rs");
    }

    #[test]
    fn collapse_error_retries_keeps_sequences_when_last_attempt_is_not_success() {
        let policy = CompactionPolicy {
            protected_tool_window_tokens: 1,
            ..CompactionPolicy::default()
        };
        let messages = vec![
            AgentMessage::summary("[Previous context compressed]\n- older work"),
            AgentMessage::assistant_with_tools(
                "try-1",
                vec![tool_call("call-1", "execute_command")],
            ),
            AgentMessage::tool(
                "call-1",
                "execute_command",
                "Command failed (exit code 2): grep: invalid option -- 'Q'",
            ),
            AgentMessage::assistant_with_tools(
                "try-2",
                vec![tool_call("call-2", "execute_command")],
            ),
            AgentMessage::tool(
                "call-2",
                "execute_command",
                "Command failed (exit code 2): grep: invalid option -- 'P'",
            ),
        ];

        let snapshot = classify_hot_memory_with_policy(&messages, &policy, None);
        let (rewritten, outcome) = collapse_error_retries(&snapshot, &messages, false);

        assert!(!outcome.applied);
        assert_eq!(rewritten.len(), messages.len());
    }

    #[test]
    fn collapse_error_retries_protects_recent_raw_window() {
        let policy = CompactionPolicy {
            protected_tool_window_tokens: 256,
            ..CompactionPolicy::default()
        };
        let messages = vec![
            AgentMessage::summary("[Previous context compressed]\n- older work"),
            AgentMessage::assistant_with_tools(
                "try-1",
                vec![tool_call("call-1", "execute_command")],
            ),
            AgentMessage::tool(
                "call-1",
                "execute_command",
                "Command failed (exit code 2): grep: invalid option -- 'Q'",
            ),
            AgentMessage::assistant_with_tools(
                "try-2",
                vec![tool_call("call-2", "execute_command")],
            ),
            AgentMessage::tool("call-2", "execute_command", "src/main.rs\nsrc/lib.rs"),
        ];

        let snapshot = classify_hot_memory_with_policy(&messages, &policy, None);
        let (rewritten, outcome) = collapse_error_retries(&snapshot, &messages, false);

        assert!(!outcome.applied);
        assert_eq!(rewritten.len(), messages.len());
    }

    #[test]
    fn collapse_error_retries_skips_multi_call_batches() {
        let policy = CompactionPolicy {
            protected_tool_window_tokens: 1,
            ..CompactionPolicy::default()
        };
        let messages = vec![
            AgentMessage::summary("[Previous context compressed]\n- older work"),
            AgentMessage::assistant_with_tools(
                "try-1",
                vec![
                    tool_call("call-1", "execute_command"),
                    tool_call("call-2", "search"),
                ],
            ),
            AgentMessage::tool(
                "call-1",
                "execute_command",
                "Command failed (exit code 2): grep: invalid option -- 'Q'",
            ),
            AgentMessage::assistant_with_tools(
                "try-2",
                vec![tool_call("call-3", "execute_command")],
            ),
            AgentMessage::tool("call-3", "execute_command", "src/main.rs\nsrc/lib.rs"),
        ];

        let snapshot = classify_hot_memory_with_policy(&messages, &policy, None);
        let (rewritten, outcome) = collapse_error_retries(&snapshot, &messages, false);

        assert!(!outcome.applied);
        assert_eq!(rewritten.len(), messages.len());
    }

    #[test]
    fn collapse_error_retries_handles_json_status_results() {
        let policy = CompactionPolicy {
            protected_tool_window_tokens: 1,
            ..CompactionPolicy::default()
        };
        let messages = vec![
            AgentMessage::assistant_with_tools("try-1", vec![tool_call("call-1", "ssh_exec")]),
            AgentMessage::tool(
                "call-1",
                "ssh_exec",
                r#"{"ok":true,"stdout":"","stderr":"denied","exit_code":1}"#,
            ),
            AgentMessage::assistant_with_tools("try-2", vec![tool_call("call-2", "ssh_exec")]),
            AgentMessage::tool(
                "call-2",
                "ssh_exec",
                r#"{"ok":true,"stdout":"done","stderr":"","exit_code":0}"#,
            ),
            AgentMessage::summary("[Previous context compressed]\n- older work"),
            AgentMessage::assistant_with_tools("recent", vec![tool_call("call-3", "search")]),
            AgentMessage::tool("call-3", "search", "recent result"),
        ];

        let snapshot = classify_hot_memory_with_policy(&messages, &policy, None);
        let (rewritten, outcome) = collapse_error_retries(&snapshot, &messages, true);

        assert!(outcome.applied);
        assert_eq!(outcome.collapsed_attempt_count, 1);
        assert_eq!(rewritten.len(), 5);
        assert_eq!(rewritten[0].content, "try-2");
        assert_eq!(
            rewritten[1].content,
            r#"{"ok":true,"stdout":"done","stderr":"","exit_code":0}"#
        );
    }
}
