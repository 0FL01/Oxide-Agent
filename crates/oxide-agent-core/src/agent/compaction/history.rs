//! Deterministic history construction for runtime/session-level compaction.

use super::budget::count_tokens_cached;
use super::{
    AgentMessageKind, CompactedSummaryMetadata, LEGACY_BREADCRUMB_PREFIX,
    LEGACY_COMPACTION_SUMMARY_PREFIX, OXIDE_COMPACTED_SUMMARY_PREFIX,
};
use crate::agent::memory::{AgentMessage, MessageRole};
use crate::agent::recovery::repair_agent_message_history_runtime;
use thiserror::Error;

/// Request for constructing a compacted replacement history.
#[derive(Debug, Clone, Copy)]
pub struct BuildCompactedHistoryRequest<'a> {
    /// Source hot-memory messages. This slice is never mutated.
    pub messages: &'a [AgentMessage],
    /// Plain text handoff summary returned by the compact backend.
    pub summary_text: &'a str,
    /// Summary metadata embedded in the replacement summary message.
    pub metadata: &'a CompactedSummaryMetadata,
    /// Best-effort target budget for the replacement history.
    pub target_token_budget: usize,
}

/// Summary extracted from the previous history during lazy migration/re-compaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviousCompactedSummary {
    /// Raw summary message text.
    pub content: String,
    /// Whether the source used the new summary prefix.
    pub current_format: bool,
}

/// Errors that prevent safe compacted history construction.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CompactedHistoryBuildError {
    /// Compact backend returned empty text.
    #[error("compacted summary text is empty")]
    EmptySummary,
    /// The summary message alone is larger than the target budget.
    #[error("compacted summary exceeds target token budget")]
    SummaryExceedsBudget,
    /// Pinned state plus the summary cannot fit the target budget.
    #[error("compacted replacement exceeds target token budget")]
    ReplacementExceedsBudget,
    /// Builder produced invalid tool-call history.
    #[error("compacted history failed tool-pair validation")]
    InvalidToolHistory,
}

/// Returns true for the current authoritative compacted-summary marker.
#[must_use]
pub fn is_current_compacted_summary_message(message: &AgentMessage) -> bool {
    message
        .content
        .trim_start()
        .starts_with(OXIDE_COMPACTED_SUMMARY_PREFIX)
}

/// Returns true for any current or legacy compaction/breadcrumb summary entry.
#[must_use]
pub fn is_any_compaction_summary_message(message: &AgentMessage) -> bool {
    if is_current_compacted_summary_message(message) {
        return true;
    }

    let content = message.content.trim_start();
    content.starts_with(LEGACY_COMPACTION_SUMMARY_PREFIX)
        || content.starts_with(LEGACY_BREADCRUMB_PREFIX)
        || message.structured_summary.is_some()
        || message.breadcrumb_card.is_some()
        || matches!(
            message.resolved_kind(),
            AgentMessageKind::Summary | AgentMessageKind::Breadcrumb
        )
}

/// Extract the newest previous summary as input signal for the next compact prompt.
#[must_use]
pub fn extract_previous_compacted_summary(
    messages: &[AgentMessage],
) -> Option<PreviousCompactedSummary> {
    messages.iter().rev().find_map(|message| {
        is_any_compaction_summary_message(message).then(|| PreviousCompactedSummary {
            content: message.content.trim().to_string(),
            current_format: is_current_compacted_summary_message(message),
        })
    })
}

/// Build a deterministic replacement history with one authoritative compacted summary.
///
/// This first-stage builder intentionally keeps the retained raw tail narrow:
/// pinned model-visible state plus recent real user/assistant text. Tool calls
/// are not retained unless later controller work can prove the active turn is
/// at a safe boundary and can preserve complete pairs.
pub fn build_compacted_history(
    request: BuildCompactedHistoryRequest<'_>,
) -> Result<Vec<AgentMessage>, CompactedHistoryBuildError> {
    let summary_text = request.summary_text.trim();
    if summary_text.is_empty() {
        return Err(CompactedHistoryBuildError::EmptySummary);
    }

    let summary = AgentMessage::compacted_summary(summary_text, request.metadata);
    let summary_tokens = message_tokens(&summary);
    if request.target_token_budget > 0 && summary_tokens > request.target_token_budget {
        return Err(CompactedHistoryBuildError::SummaryExceedsBudget);
    }

    let mut replacement = Vec::new();
    let mut used_tokens = 0usize;

    for message in request.messages.iter().filter(|message| is_pinned(message)) {
        if is_any_compaction_summary_message(message) {
            continue;
        }
        used_tokens = used_tokens.saturating_add(message_tokens(message));
        replacement.push(message.clone());
    }

    used_tokens = used_tokens.saturating_add(summary_tokens);
    replacement.push(summary);
    if request.target_token_budget > 0 && used_tokens > request.target_token_budget {
        return Err(CompactedHistoryBuildError::ReplacementExceedsBudget);
    }

    let terminal_tool_batch_index = terminal_open_tool_batch_index(request.messages);
    let mut recent_tail = Vec::new();
    for (index, message) in request.messages.iter().enumerate().rev() {
        if is_pinned(message)
            || is_any_compaction_summary_message(message)
            || !is_recent_raw_candidate(message, index, terminal_tool_batch_index)
        {
            continue;
        }

        let tokens = message_tokens(message);
        if request.target_token_budget > 0
            && used_tokens.saturating_add(tokens) > request.target_token_budget
        {
            continue;
        }

        used_tokens = used_tokens.saturating_add(tokens);
        recent_tail.push(message.clone());
    }

    recent_tail.reverse();
    replacement.extend(recent_tail);

    let (validated, repair_outcome) = repair_agent_message_history_runtime(&replacement);
    if repair_outcome.applied || validated.len() != replacement.len() {
        return Err(CompactedHistoryBuildError::InvalidToolHistory);
    }

    Ok(replacement)
}

fn is_pinned(message: &AgentMessage) -> bool {
    matches!(
        message.resolved_kind(),
        AgentMessageKind::TopicAgentsMd
            | AgentMessageKind::UserTask
            | AgentMessageKind::RuntimeContext
            | AgentMessageKind::SkillContext
            | AgentMessageKind::ApprovalReplay
            | AgentMessageKind::InfraStatus
            | AgentMessageKind::ArchiveReference
    )
}

fn terminal_open_tool_batch_index(messages: &[AgentMessage]) -> Option<usize> {
    let index = messages
        .iter()
        .enumerate()
        .rev()
        .find(|(_, message)| !is_pinned(message) && !is_any_compaction_summary_message(message))
        .map(|(index, _)| index)?;

    matches!(
        messages[index].resolved_kind(),
        AgentMessageKind::AssistantToolCall
    )
    .then_some(index)
}

fn is_recent_raw_candidate(
    message: &AgentMessage,
    index: usize,
    terminal_tool_batch_index: Option<usize>,
) -> bool {
    match message.role {
        MessageRole::User => matches!(message.resolved_kind(), AgentMessageKind::UserTurn),
        MessageRole::Assistant => {
            if terminal_tool_batch_index == Some(index) {
                return true;
            }

            matches!(
                message.resolved_kind(),
                AgentMessageKind::AssistantResponse | AgentMessageKind::AssistantReasoning
            ) && message.tool_calls.is_none()
        }
        MessageRole::System | MessageRole::Tool => false,
    }
}

fn message_tokens(message: &AgentMessage) -> usize {
    let mut tokens = count_tokens_cached(&message.content);
    if let Some(reasoning) = &message.reasoning {
        tokens = tokens.saturating_add(count_tokens_cached(reasoning));
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::compaction::{
        ArchiveRef, CompactionBackend, CompactionPhase, CompactionReason, CompactionSummary,
    };
    use crate::agent::recovery::repair_agent_message_history_runtime;
    use crate::llm::{ToolCall, ToolCallFunction};

    fn metadata(previous_summary_detected: bool) -> CompactedSummaryMetadata {
        CompactedSummaryMetadata {
            generation: 2,
            reason: CompactionReason::Manual,
            phase: CompactionPhase::Manual,
            token_before: 100,
            token_after: 10,
            history_items_before: 5,
            history_items_after: 2,
            provider: "mock".to_string(),
            route: "mock-compact".to_string(),
            backend: CompactionBackend::LocalLlmSummary,
            created_at: "2026-05-21T20:05:00+03:00".to_string(),
            previous_summary_detected,
            repair_applied: false,
        }
    }

    #[test]
    fn detects_current_and_legacy_summary_messages() {
        let current = AgentMessage::compacted_summary("state", &metadata(false));
        assert!(is_current_compacted_summary_message(&current));
        assert!(is_any_compaction_summary_message(&current));

        let legacy = AgentMessage::summary("[COMPACTION_SUMMARY]\nGoal:\nship");
        assert!(!is_current_compacted_summary_message(&legacy));
        assert!(is_any_compaction_summary_message(&legacy));

        let breadcrumb = AgentMessage::summary("[BREADCRUMB_CARD]\nNext Steps:\n- continue");
        assert!(is_any_compaction_summary_message(&breadcrumb));
    }

    #[test]
    fn detects_structured_legacy_summary_payload() {
        let legacy = AgentMessage::from_compaction_summary(CompactionSummary {
            goal: "ship".to_string(),
            ..CompactionSummary::default()
        });

        assert!(is_any_compaction_summary_message(&legacy));
        let extracted = extract_previous_compacted_summary(&[legacy]).expect("summary found");
        assert!(!extracted.current_format);
        assert!(extracted.content.contains("[COMPACTION_SUMMARY]"));
    }

    #[test]
    fn builds_history_with_one_current_summary_and_no_old_summaries() {
        let messages = vec![
            AgentMessage::topic_agents_md("# Topic"),
            AgentMessage::user_task("Implement compaction"),
            AgentMessage::summary("[COMPACTION_SUMMARY]\nOld"),
            AgentMessage::user("latest request"),
            AgentMessage::assistant("latest answer"),
            AgentMessage::summary("[BREADCRUMB_CARD]\nOld breadcrumb"),
        ];

        let replacement = build_compacted_history(BuildCompactedHistoryRequest {
            messages: &messages,
            summary_text: "Current progress and remaining work.",
            metadata: &metadata(true),
            target_token_budget: 10_000,
        })
        .expect("history builds");

        assert_eq!(
            replacement
                .iter()
                .filter(|message| is_current_compacted_summary_message(message))
                .count(),
            1
        );
        assert!(replacement
            .iter()
            .all(|message| !message.content.contains("[BREADCRUMB_CARD]")));
        assert!(replacement
            .iter()
            .all(|message| !message.content.contains("[COMPACTION_SUMMARY]")));
        assert!(replacement
            .iter()
            .any(|message| message.resolved_kind() == AgentMessageKind::TopicAgentsMd));
        assert!(replacement
            .iter()
            .any(|message| message.resolved_kind() == AgentMessageKind::UserTask));
        assert!(replacement
            .iter()
            .any(|message| message.content == "latest request"));
    }

    #[test]
    fn lazy_migration_preserves_old_archive_references_without_old_summaries() {
        let archive_ref = ArchiveRef {
            archive_id: "archive-legacy".to_string(),
            created_at: 1_779_341_200,
            title: "Legacy compacted chunk".to_string(),
            storage_key: "users/1/compaction/archive-legacy.json".to_string(),
        };
        let fixture_json = serde_json::json!([
            {
                "kind": "Summary",
                "role": "System",
                "content": "[COMPACTION_SUMMARY]\nGoal:\nship legacy migration",
                "reasoning": null,
                "tool_call_id": null,
                "tool_call_correlation": null,
                "tool_name": null,
                "tool_calls": null,
                "tool_call_correlations": null,
                "externalized_payload": null,
                "pruned_artifact": null,
                "structured_summary": {
                    "goal": "ship legacy migration",
                    "constraints": [],
                    "decisions": [],
                    "discoveries": [],
                    "relevant_files_entities": [],
                    "remaining_work": [],
                    "risks": []
                },
                "archive_ref": null,
                "breadcrumb_card": null
            },
            {
                "kind": "ArchiveReference",
                "role": "System",
                "content": "[ARCHIVE_REFERENCE]\nLegacy compacted chunk",
                "reasoning": null,
                "tool_call_id": null,
                "tool_call_correlation": null,
                "tool_name": null,
                "tool_calls": null,
                "tool_call_correlations": null,
                "externalized_payload": null,
                "pruned_artifact": null,
                "structured_summary": null,
                "archive_ref": archive_ref,
                "breadcrumb_card": null
            },
            {
                "kind": "UserTurn",
                "role": "User",
                "content": "continue from migrated session",
                "reasoning": null,
                "tool_call_id": null,
                "tool_call_correlation": null,
                "tool_name": null,
                "tool_calls": null,
                "tool_call_correlations": null,
                "externalized_payload": null,
                "pruned_artifact": null,
                "structured_summary": null,
                "archive_ref": null,
                "breadcrumb_card": null
            }
        ]);
        let messages: Vec<AgentMessage> =
            serde_json::from_value(fixture_json).expect("legacy fixture deserializes");
        assert_eq!(
            messages[1].archive_ref_payload(),
            Some(&archive_ref),
            "old archive refs must remain deserializable"
        );
        assert!(extract_previous_compacted_summary(&messages).is_some());

        let replacement = build_compacted_history(BuildCompactedHistoryRequest {
            messages: &messages,
            summary_text: "Migrated summary with current state.",
            metadata: &metadata(true),
            target_token_budget: 10_000,
        })
        .expect("legacy fixture compacts safely");

        assert_eq!(
            replacement
                .iter()
                .filter(|message| is_current_compacted_summary_message(message))
                .count(),
            1
        );
        assert!(replacement
            .iter()
            .all(|message| !message.content.contains("[COMPACTION_SUMMARY]")));
        assert!(replacement
            .iter()
            .any(|message| message.archive_ref_payload() == Some(&archive_ref)));
        assert!(replacement
            .iter()
            .any(|message| message.content == "continue from migrated session"));
    }

    #[test]
    fn preserves_approval_replay_messages() {
        let messages = vec![
            AgentMessage::user("Resume approved SSH action."),
            AgentMessage::approval_replay(
                "Retry exact SSH call with approval_request_id='req-1' and approval_token='token-1'.",
            ),
            AgentMessage::assistant("Continuing after approval."),
        ];

        let replacement = build_compacted_history(BuildCompactedHistoryRequest {
            messages: &messages,
            summary_text: "Approval replay is pending and must be preserved.",
            metadata: &metadata(false),
            target_token_budget: 10_000,
        })
        .expect("history builds with approval replay");

        assert!(replacement.iter().any(|message| {
            message.resolved_kind() == AgentMessageKind::ApprovalReplay
                && message.content.contains("approval_request_id='req-1'")
                && message.content.contains("approval_token='token-1'")
        }));
    }

    #[test]
    fn preserves_terminal_open_tool_batch_for_compress_result_continuation() {
        let compress_call = ToolCall::new(
            "call-compress".to_string(),
            ToolCallFunction {
                name: "compress".to_string(),
                arguments: "{}".to_string(),
            },
            false,
        );
        let messages = vec![
            AgentMessage::user("Please compact now."),
            AgentMessage::assistant_with_tools("Calling compress", vec![compress_call]),
        ];

        let replacement = build_compacted_history(BuildCompactedHistoryRequest {
            messages: &messages,
            summary_text: "Current compacted state.",
            metadata: &metadata(false),
            target_token_budget: 10_000,
        })
        .expect("history builds with terminal open tool batch");

        assert!(replacement
            .iter()
            .any(|message| message.resolved_kind() == AgentMessageKind::AssistantToolCall));

        let mut with_result = replacement;
        with_result.push(AgentMessage::tool(
            "call-compress",
            "compress",
            r#"{"ok":true}"#,
        ));
        let (validated, repair_outcome) = repair_agent_message_history_runtime(&with_result);

        assert!(
            !repair_outcome.applied,
            "compress result should not become orphaned after compaction"
        );
        assert_eq!(validated.len(), with_result.len());
    }

    #[test]
    fn rejects_empty_summary() {
        let err = build_compacted_history(BuildCompactedHistoryRequest {
            messages: &[],
            summary_text: "   ",
            metadata: &metadata(false),
            target_token_budget: 10_000,
        })
        .expect_err("empty summary rejected");

        assert_eq!(err, CompactedHistoryBuildError::EmptySummary);
    }
}
