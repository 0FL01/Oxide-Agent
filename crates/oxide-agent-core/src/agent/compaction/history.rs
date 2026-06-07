//! Deterministic history construction for runtime/session-level compaction.

use super::budget::count_tokens_cached;
use super::{AgentMessageKind, CompactedSummaryMetadata, OXIDE_COMPACTED_SUMMARY_PREFIX};
use crate::agent::memory::{AgentMessage, MessageRole};
use crate::agent::recovery::repair_agent_message_history_runtime;
use std::collections::{HashMap, HashSet};
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

/// Current compacted summary extracted from previous history during re-compaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviousCompactedSummary {
    /// Raw summary message text.
    pub content: String,
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

/// Returns true for the current compacted-summary entry.
#[must_use]
pub fn is_any_compaction_summary_message(message: &AgentMessage) -> bool {
    is_current_compacted_summary_message(message)
}

/// Extract the newest previous summary as input signal for the next compact prompt.
#[must_use]
pub fn extract_previous_compacted_summary(
    messages: &[AgentMessage],
) -> Option<PreviousCompactedSummary> {
    messages.iter().rev().find_map(|message| {
        is_any_compaction_summary_message(message).then(|| PreviousCompactedSummary {
            content: message.content.trim().to_string(),
        })
    })
}

/// Best-effort minimum number of recent user turns to retain when they fit.
const MIN_RECENT_ROUNDS: usize = 3;

/// Build a deterministic replacement history with one authoritative compacted summary.
///
/// This builder keeps pinned model-visible state plus a recent raw tail that
/// best-effort retains up to [`MIN_RECENT_ROUNDS`] recent user-turn anchors.
/// Complete tool-call pairs (AssistantToolCall + matching ToolResult) are
/// retained when they fall within the recent budget or minimum floor.
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

    // Collect recent tail in two phases.
    //
    // Phase 1 — budget-constrained: collect messages from the end that fit the
    // token budget. Tool-call batches are collected atomically so the tail never
    // contains orphaned tool results or partially-trimmed tool calls.
    //
    // Phase 2 — minimum floor: if fewer than MIN_RECENT_ROUNDS user turns made
    // it through, continue collecting only while the target budget still allows
    // it. The summary carries older oversized context; the raw tail must not
    // become a no-op compaction by dragging the entire tool loop back in.

    let mut collected_indices: HashSet<usize> = HashSet::new();
    let mut user_rounds = 0usize;

    // --- Phase 1: budget-constrained ---
    for (index, message) in request.messages.iter().enumerate().rev() {
        if collected_indices.contains(&index) {
            continue;
        }
        let Some(candidate) = recent_tail_candidate_indices(
            request.messages,
            index,
            message,
            terminal_tool_batch_index,
        ) else {
            continue;
        };
        let tokens = candidate
            .iter()
            .filter(|index| !collected_indices.contains(index))
            .map(|index| message_tokens(&request.messages[*index]))
            .sum::<usize>();
        let fits_budget = request.target_token_budget == 0
            || used_tokens.saturating_add(tokens) <= request.target_token_budget;

        if fits_budget {
            include_candidate_indices(
                request.messages,
                candidate,
                &mut collected_indices,
                &mut used_tokens,
                &mut user_rounds,
            );
        }
    }

    // --- Phase 2: minimum floor (budget-respecting) ---
    if user_rounds < MIN_RECENT_ROUNDS {
        for (index, message) in request.messages.iter().enumerate().rev() {
            if collected_indices.contains(&index) {
                continue;
            }
            let Some(candidate) = recent_tail_candidate_indices(
                request.messages,
                index,
                message,
                terminal_tool_batch_index,
            ) else {
                continue;
            };
            let tokens = candidate
                .iter()
                .filter(|index| !collected_indices.contains(index))
                .map(|index| message_tokens(&request.messages[*index]))
                .sum::<usize>();
            let fits_budget = request.target_token_budget == 0
                || used_tokens.saturating_add(tokens) <= request.target_token_budget;
            if !fits_budget {
                continue;
            }
            include_candidate_indices(
                request.messages,
                candidate,
                &mut collected_indices,
                &mut used_tokens,
                &mut user_rounds,
            );

            if user_rounds >= MIN_RECENT_ROUNDS {
                break;
            }
        }
    }

    let mut collected_indices = collected_indices.into_iter().collect::<Vec<_>>();
    collected_indices.sort_unstable();
    let recent_tail = collected_indices
        .into_iter()
        .map(|index| request.messages[index].clone());
    replacement.extend(recent_tail);
    deduplicate_superseded_read_file_results(&mut replacement);

    let (validated, repair_outcome) = repair_agent_message_history_runtime(&replacement);
    if repair_outcome.applied || validated.len() != replacement.len() {
        return Err(CompactedHistoryBuildError::InvalidToolHistory);
    }

    Ok(replacement)
}

fn include_candidate_indices(
    messages: &[AgentMessage],
    candidate: Vec<usize>,
    collected_indices: &mut HashSet<usize>,
    used_tokens: &mut usize,
    user_rounds: &mut usize,
) {
    for index in candidate {
        if collected_indices.insert(index) {
            let message = &messages[index];
            if message.resolved_kind() == AgentMessageKind::UserTurn {
                *user_rounds = user_rounds.saturating_add(1);
            }
            *used_tokens = used_tokens.saturating_add(message_tokens(message));
        }
    }
}

fn recent_tail_candidate_indices(
    messages: &[AgentMessage],
    index: usize,
    message: &AgentMessage,
    terminal_tool_batch_index: Option<usize>,
) -> Option<Vec<usize>> {
    if is_pinned(message) || is_any_compaction_summary_message(message) {
        return None;
    }

    match message.role {
        MessageRole::User => {
            (message.resolved_kind() == AgentMessageKind::UserTurn).then_some(vec![index])
        }
        MessageRole::Assistant => match message.resolved_kind() {
            AgentMessageKind::AssistantResponse | AgentMessageKind::AssistantReasoning
                if message.tool_calls.is_none() =>
            {
                Some(vec![index])
            }
            AgentMessageKind::AssistantToolCall if terminal_tool_batch_index == Some(index) => {
                Some(vec![index])
            }
            AgentMessageKind::AssistantToolCall => completed_tool_batch_indices(messages, index),
            _ => None,
        },
        MessageRole::Tool => tool_batch_start_for_result(messages, index)
            .and_then(|assistant_index| completed_tool_batch_indices(messages, assistant_index))
            .filter(|indices| indices.contains(&index)),
        MessageRole::System => None,
    }
}

fn completed_tool_batch_indices(
    messages: &[AgentMessage],
    assistant_index: usize,
) -> Option<Vec<usize>> {
    let assistant = messages.get(assistant_index)?;
    if assistant.resolved_kind() != AgentMessageKind::AssistantToolCall {
        return None;
    }

    let expected_ids = assistant_tool_invocation_ids(assistant)?;
    if expected_ids.is_empty() || expected_ids.iter().any(|id| id.trim().is_empty()) {
        return None;
    }

    let expected_set = expected_ids.iter().cloned().collect::<HashSet<_>>();
    if expected_set.len() != expected_ids.len() {
        return None;
    }
    let mut seen_ids = HashSet::new();
    let mut indices = vec![assistant_index];
    let mut cursor = assistant_index + 1;
    while cursor < messages.len()
        && messages[cursor].resolved_kind() == AgentMessageKind::ToolResult
    {
        if let Some(invocation_id) = tool_result_invocation_id(&messages[cursor])
            && expected_set.contains(&invocation_id)
            && seen_ids.insert(invocation_id)
        {
            indices.push(cursor);
        }
        cursor += 1;
    }

    (seen_ids.len() == expected_set.len()).then_some(indices)
}

fn tool_batch_start_for_result(messages: &[AgentMessage], result_index: usize) -> Option<usize> {
    if messages.get(result_index)?.resolved_kind() != AgentMessageKind::ToolResult {
        return None;
    }

    let mut cursor = result_index;
    while cursor > 0 && messages[cursor - 1].resolved_kind() == AgentMessageKind::ToolResult {
        cursor -= 1;
    }
    let assistant_index = cursor.checked_sub(1)?;
    (messages[assistant_index].resolved_kind() == AgentMessageKind::AssistantToolCall)
        .then_some(assistant_index)
}

fn assistant_tool_invocation_ids(message: &AgentMessage) -> Option<Vec<String>> {
    message
        .resolved_tool_call_correlations()
        .map(|correlations| {
            correlations
                .into_iter()
                .map(|correlation| correlation.invocation_id.into_inner())
                .collect()
        })
}

fn tool_result_invocation_id(message: &AgentMessage) -> Option<String> {
    message
        .resolved_tool_call_correlation()
        .map(|correlation| correlation.invocation_id.into_inner())
}

fn is_pinned(message: &AgentMessage) -> bool {
    matches!(
        message.resolved_kind(),
        AgentMessageKind::TopicAgentsMd
            | AgentMessageKind::UserTask
            | AgentMessageKind::RuntimeContext
            | AgentMessageKind::ApprovalReplay
            | AgentMessageKind::InfraStatus
    )
}

#[derive(Debug, Clone)]
struct FileToolAction {
    name: String,
    path: String,
}

fn deduplicate_superseded_read_file_results(messages: &mut [AgentMessage]) {
    let actions = file_tool_actions_by_invocation(messages);
    let mut latest_read_by_path = HashSet::<String>::new();
    let mut mutated_since_latest_read = HashSet::<String>::new();
    let mut deduplicated_indices = Vec::new();

    for (index, message) in messages.iter().enumerate().rev() {
        let Some(action) = message
            .tool_call_id
            .as_deref()
            .and_then(|tool_call_id| actions.get(tool_call_id))
        else {
            continue;
        };

        match action.name.as_str() {
            "read_file" => {
                if latest_read_by_path.contains(&action.path)
                    && !mutated_since_latest_read.contains(&action.path)
                {
                    deduplicated_indices.push((index, action.path.clone()));
                }
                latest_read_by_path.insert(action.path.clone());
                mutated_since_latest_read.remove(&action.path);
            }
            "write_file" | "apply_file_edit" => {
                mutated_since_latest_read.insert(action.path.clone());
            }
            _ => {}
        }
    }

    for (index, path) in deduplicated_indices {
        if let Some(message) = messages.get_mut(index) {
            message.content = format!(
                "[deduplicated tool result]\ntool: read_file\npath: {path}\nreason: a newer read_file result for this path is retained later in context."
            );
        }
    }
}

fn file_tool_actions_by_invocation(messages: &[AgentMessage]) -> HashMap<String, FileToolAction> {
    let mut actions = HashMap::new();
    for message in messages {
        let Some(tool_calls) = message.tool_calls.as_ref() else {
            continue;
        };

        for tool_call in tool_calls {
            let name = tool_call.function.name.as_str();
            if !matches!(name, "read_file" | "write_file" | "apply_file_edit") {
                continue;
            }
            let Some(path) = file_tool_path(&tool_call.function.arguments) else {
                continue;
            };
            actions.insert(
                tool_call.invocation_id().into_inner(),
                FileToolAction {
                    name: name.to_string(),
                    path,
                },
            );
        }
    }
    actions
}

fn file_tool_path(arguments: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|value| {
            value
                .get("path")
                .and_then(|path| path.as_str())
                .map(str::trim)
                .map(str::to_string)
        })
        .filter(|path| !path.is_empty())
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
    use crate::agent::compaction::{CompactionBackend, CompactionPhase, CompactionReason};
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
            wiki_memory_lookup_available: false,
        }
    }

    #[test]
    fn detects_current_summary_messages() {
        let current = AgentMessage::compacted_summary("state", &metadata(false));
        assert!(is_current_compacted_summary_message(&current));
        assert!(is_any_compaction_summary_message(&current));

        let old_text_shape = AgentMessage::summary("[COMPACTION_SUMMARY]\nGoal:\nship");
        assert!(!is_any_compaction_summary_message(&old_text_shape));
    }

    #[test]
    fn builds_history_with_one_current_summary() {
        let messages = vec![
            AgentMessage::topic_agents_md("# Topic"),
            AgentMessage::user_task("Implement compaction"),
            AgentMessage::compacted_summary("Old current-format summary.", &metadata(false)),
            AgentMessage::user("latest request"),
            AgentMessage::assistant("latest answer"),
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
        assert!(
            replacement
                .iter()
                .all(|message| !message.content.contains("Old current-format summary."))
        );
        assert!(
            replacement
                .iter()
                .any(|message| message.resolved_kind() == AgentMessageKind::TopicAgentsMd)
        );
        assert!(
            replacement
                .iter()
                .any(|message| message.resolved_kind() == AgentMessageKind::UserTask)
        );
        assert!(
            replacement
                .iter()
                .any(|message| message.content == "latest request")
        );
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

        assert!(
            replacement
                .iter()
                .any(|message| message.resolved_kind() == AgentMessageKind::AssistantToolCall)
        );

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
    fn skips_complete_tool_batch_when_pair_cannot_fit_budget() {
        let search_call = ToolCall::new(
            "call-search".to_string(),
            ToolCallFunction {
                name: "search".to_string(),
                arguments: r#"{"query":"pricing"}"#.to_string(),
            },
            false,
        );
        let messages = vec![
            AgentMessage::user("round 1"),
            AgentMessage::assistant("answer 1"),
            AgentMessage::user("round 2"),
            AgentMessage::assistant("answer 2"),
            AgentMessage::user("round 3"),
            AgentMessage::assistant("answer 3"),
            AgentMessage::assistant_with_tools(
                format!(
                    "Calling search. {}",
                    "large assistant payload ".repeat(20_000)
                ),
                vec![search_call],
            ),
            AgentMessage::tool("call-search", "search", "small result"),
        ];

        let replacement = build_compacted_history(BuildCompactedHistoryRequest {
            messages: &messages,
            summary_text: "Current compacted state.",
            metadata: &metadata(false),
            target_token_budget: 2_000,
        })
        .expect("history builds without orphaning tool result");

        assert!(
            !replacement
                .iter()
                .any(|message| message.resolved_kind() == AgentMessageKind::ToolResult)
        );
        assert!(
            !replacement
                .iter()
                .any(|message| message.resolved_kind() == AgentMessageKind::AssistantToolCall)
        );

        let (validated, repair_outcome) = repair_agent_message_history_runtime(&replacement);
        assert!(!repair_outcome.applied);
        assert_eq!(validated.len(), replacement.len());
    }

    #[test]
    fn minimum_floor_respects_budget_for_oversized_tool_batch() {
        let search_call = ToolCall::new(
            "call-search".to_string(),
            ToolCallFunction {
                name: "search".to_string(),
                arguments: r#"{"query":"pricing"}"#.to_string(),
            },
            false,
        );
        let messages = vec![
            AgentMessage::user("Need fresh pricing data."),
            AgentMessage::assistant_with_tools(
                format!(
                    "Calling search. {}",
                    "large assistant payload ".repeat(20_000)
                ),
                vec![search_call],
            ),
            AgentMessage::tool("call-search", "search", "small result"),
        ];

        let replacement = build_compacted_history(BuildCompactedHistoryRequest {
            messages: &messages,
            summary_text: "Current compacted state.",
            metadata: &metadata(false),
            target_token_budget: 2_000,
        })
        .expect("minimum floor skips oversized complete tool batch");

        assert!(
            replacement
                .iter()
                .any(|message| message.content == "Need fresh pricing data.")
        );
        assert!(
            !replacement
                .iter()
                .any(|message| message.resolved_kind() == AgentMessageKind::AssistantToolCall)
        );
        assert!(
            !replacement
                .iter()
                .any(|message| message.resolved_kind() == AgentMessageKind::ToolResult)
        );
        assert!(replacement.iter().map(message_tokens).sum::<usize>() <= 2_000);

        let (validated, repair_outcome) = repair_agent_message_history_runtime(&replacement);
        assert!(!repair_outcome.applied);
        assert_eq!(validated.len(), replacement.len());
    }

    #[test]
    fn minimum_floor_does_not_reinflate_tool_heavy_single_turn_history() {
        let mut messages = vec![
            AgentMessage::user_task("Research current pricing."),
            AgentMessage::user("Find current pricing details."),
        ];
        for index in 0..12 {
            let call_id = format!("call-search-{index}");
            let search_call = ToolCall::new(
                call_id.clone(),
                ToolCallFunction {
                    name: "search".to_string(),
                    arguments: format!(r#"{{"query":"pricing {index}"}}"#),
                },
                false,
            );
            messages.push(AgentMessage::assistant_with_tools(
                format!("Searching batch {index}."),
                vec![search_call],
            ));
            messages.push(AgentMessage::tool(
                &call_id,
                "search",
                &format!("result {index}: {}", "large tool result ".repeat(500)),
            ));
        }

        let replacement = build_compacted_history(BuildCompactedHistoryRequest {
            messages: &messages,
            summary_text: "Current compacted pricing research state.",
            metadata: &metadata(false),
            target_token_budget: 2_000,
        })
        .expect("history compacts tool-heavy single-turn loop");

        assert!(
            replacement.len() < messages.len(),
            "minimum floor must not re-add every skipped tool batch"
        );
        assert!(replacement.iter().map(message_tokens).sum::<usize>() <= 2_000);

        let (validated, repair_outcome) = repair_agent_message_history_runtime(&replacement);
        assert!(!repair_outcome.applied);
        assert_eq!(validated.len(), replacement.len());
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
