//! History repair and JSON extraction for LLM responses.
//!
//! Handles tool-call/tool-result pairing repair, orphan pruning, and
//! deterministic JSON extraction from noisy provider output.

use crate::agent::compaction::AgentMessageKind;
use crate::agent::memory::{AgentMessage, MessageRole};
use crate::llm::{ToolCall, ToolCallCorrelation};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use tracing::warn;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// Summary of local history repairs applied before retrying an LLM call.
pub struct HistoryRepairOutcome {
    /// Whether any message was rewritten or removed.
    pub applied: bool,
    /// Number of invalid tool result messages removed.
    pub dropped_tool_results: usize,
    /// Number of assistant tool calls trimmed out of a batch.
    pub trimmed_tool_calls: usize,
    /// Number of assistant tool-call messages converted back to plain assistant text.
    pub converted_tool_call_messages: usize,
    /// Number of assistant tool-call messages dropped entirely.
    pub dropped_tool_call_messages: usize,
    /// Number of tool calls with empty wire IDs (provider_tool_call_id) repaired.
    pub repaired_empty_wire_ids: usize,
}

#[must_use]
/// Repair locally inconsistent tool-call history so the runner can retry safely.
pub fn repair_agent_message_history(
    messages: &[AgentMessage],
) -> (Vec<AgentMessage>, HistoryRepairOutcome) {
    repair_agent_message_history_with_policy(messages, false)
}

#[must_use]
/// Repair history after routine memory mutations while preserving the active open tool batch.
pub fn repair_agent_message_history_runtime(
    messages: &[AgentMessage],
) -> (Vec<AgentMessage>, HistoryRepairOutcome) {
    repair_agent_message_history_with_policy(messages, true)
}

#[must_use]
/// Repair history for a specific provider request policy.
pub fn repair_agent_message_history_for_provider(
    messages: &[AgentMessage],
    strict_tool_history: bool,
) -> (Vec<AgentMessage>, HistoryRepairOutcome) {
    repair_agent_message_history_with_policy(messages, !strict_tool_history)
}

#[must_use]
/// Remove tool calls and tool results for tools that are no longer available.
///
/// After filtering by the current tool catalog/policy, the function runs the
/// standard runtime history repair pass to drop any now-orphaned tool results.
pub fn prune_tool_history_by_availability(
    messages: &[AgentMessage],
    available_tools: &HashSet<String>,
) -> (Vec<AgentMessage>, HistoryRepairOutcome) {
    let mut rewritten = Vec::with_capacity(messages.len());
    let mut outcome = HistoryRepairOutcome::default();

    for message in messages {
        match message.resolved_kind() {
            AgentMessageKind::AssistantToolCall => {
                let Some(tool_calls) = message.tool_calls.as_ref() else {
                    rewritten.push(message.clone());
                    continue;
                };

                let correlations = message
                    .resolved_tool_call_correlations()
                    .unwrap_or_else(|| tool_calls.iter().map(ToolCall::correlation).collect());

                let retained_pairs = tool_calls
                    .iter()
                    .cloned()
                    .zip(correlations.into_iter())
                    .filter(|(tool_call, _)| available_tools.contains(&tool_call.function.name))
                    .collect::<Vec<_>>();
                let retained_tool_calls = retained_pairs
                    .iter()
                    .map(|(tool_call, _)| tool_call.clone())
                    .collect::<Vec<_>>();
                let retained_tool_call_correlations = retained_pairs
                    .into_iter()
                    .map(|(_, correlation)| correlation)
                    .collect::<Vec<_>>();

                if retained_tool_calls.len() == tool_calls.len() {
                    rewritten.push(message.clone());
                    continue;
                }

                outcome.applied = true;
                outcome.trimmed_tool_calls = outcome
                    .trimmed_tool_calls
                    .saturating_add(tool_calls.len().saturating_sub(retained_tool_calls.len()));

                if retained_tool_calls.is_empty() {
                    if message.content.trim().is_empty() {
                        outcome.dropped_tool_call_messages =
                            outcome.dropped_tool_call_messages.saturating_add(1);
                        continue;
                    }

                    let mut converted = message.clone();
                    converted.kind = AgentMessageKind::AssistantResponse;
                    converted.role = MessageRole::Assistant;
                    converted.tool_calls = None;
                    converted.tool_call_correlations = None;
                    outcome.converted_tool_call_messages =
                        outcome.converted_tool_call_messages.saturating_add(1);
                    rewritten.push(converted);
                    continue;
                }

                let mut rewritten_message = message.clone();
                rewritten_message.tool_calls = Some(retained_tool_calls);
                rewritten_message.tool_call_correlations = Some(retained_tool_call_correlations);
                rewritten.push(rewritten_message);
            }
            AgentMessageKind::ToolResult => {
                let keep = message
                    .tool_name
                    .as_ref()
                    .is_some_and(|tool_name| available_tools.contains(tool_name));
                if keep {
                    rewritten.push(message.clone());
                } else {
                    outcome.applied = true;
                    outcome.dropped_tool_results = outcome.dropped_tool_results.saturating_add(1);
                }
            }
            _ => rewritten.push(message.clone()),
        }
    }

    let (repaired, repair_outcome) = repair_agent_message_history_runtime(&rewritten);
    outcome.applied |= repair_outcome.applied;
    outcome.dropped_tool_results = outcome
        .dropped_tool_results
        .saturating_add(repair_outcome.dropped_tool_results);
    outcome.trimmed_tool_calls = outcome
        .trimmed_tool_calls
        .saturating_add(repair_outcome.trimmed_tool_calls);
    outcome.converted_tool_call_messages = outcome
        .converted_tool_call_messages
        .saturating_add(repair_outcome.converted_tool_call_messages);
    outcome.dropped_tool_call_messages = outcome
        .dropped_tool_call_messages
        .saturating_add(repair_outcome.dropped_tool_call_messages);
    outcome.repaired_empty_wire_ids = outcome
        .repaired_empty_wire_ids
        .saturating_add(repair_outcome.repaired_empty_wire_ids);

    (repaired, outcome)
}

fn repair_agent_message_history_with_policy(
    messages: &[AgentMessage],
    allow_terminal_incomplete_batch: bool,
) -> (Vec<AgentMessage>, HistoryRepairOutcome) {
    let mut repaired = Vec::with_capacity(messages.len());
    let mut outcome = HistoryRepairOutcome::default();
    let mut index = 0;

    while index < messages.len() {
        let message = &messages[index];
        if message.resolved_kind() == AgentMessageKind::AssistantToolCall {
            let (mut repaired_batch, next_index, batch_outcome) =
                repair_assistant_tool_batch(messages, index, allow_terminal_incomplete_batch);
            repaired.append(&mut repaired_batch);
            outcome.applied |= batch_outcome.applied;
            outcome.dropped_tool_results += batch_outcome.dropped_tool_results;
            outcome.trimmed_tool_calls += batch_outcome.trimmed_tool_calls;
            outcome.converted_tool_call_messages += batch_outcome.converted_tool_call_messages;
            outcome.dropped_tool_call_messages += batch_outcome.dropped_tool_call_messages;
            outcome.repaired_empty_wire_ids += batch_outcome.repaired_empty_wire_ids;
            index = next_index;
            continue;
        }

        if message.resolved_kind() == AgentMessageKind::ToolResult {
            outcome.applied = true;
            outcome.dropped_tool_results = outcome.dropped_tool_results.saturating_add(1);
            index += 1;
            continue;
        }

        repaired.push(message.clone());
        index += 1;
    }

    (repaired, outcome)
}

/// Results from validating tool calls in an assistant message.
struct ValidatedToolCalls {
    calls: Vec<ToolCall>,
    correlations: Vec<ToolCallCorrelation>,
    expected_ids: HashSet<crate::llm::InvocationId>,
}

/// Extract and validate tool calls from an assistant message.
fn extract_valid_tool_calls(
    assistant: &AgentMessage,
    outcome: &mut HistoryRepairOutcome,
) -> Option<ValidatedToolCalls> {
    let tool_calls = assistant.tool_calls.clone()?;
    let correlations = assistant
        .resolved_tool_call_correlations()
        .unwrap_or_else(|| tool_calls.iter().map(ToolCall::correlation).collect());

    let mut expected_ids = HashSet::new();
    let mut valid_calls = Vec::with_capacity(tool_calls.len());
    let mut valid_correlations = Vec::with_capacity(tool_calls.len());

    for (tool_call, mut correlation) in tool_calls.into_iter().zip(correlations) {
        let id = correlation.invocation_id.as_str().trim();
        if id.is_empty() || !expected_ids.insert(correlation.invocation_id.clone()) {
            outcome.applied = true;
            outcome.trimmed_tool_calls = outcome.trimmed_tool_calls.saturating_add(1);
            continue;
        }

        if correlation.wire_tool_call_id().is_empty() {
            let id_str = correlation.invocation_id.as_str().to_string();
            warn!(
                invocation_id = %id_str,
                "Repairing empty wire_id in stored tool call correlation"
            );
            correlation = correlation.with_provider_tool_call_id(id_str);
            outcome.applied = true;
            outcome.repaired_empty_wire_ids = outcome.repaired_empty_wire_ids.saturating_add(1);
        }

        valid_calls.push(tool_call);
        valid_correlations.push(correlation);
    }

    Some(ValidatedToolCalls {
        calls: valid_calls,
        correlations: valid_correlations,
        expected_ids,
    })
}

/// Process tool result messages following an assistant batch.
fn process_tool_results(
    messages: &[AgentMessage],
    assistant_index: usize,
    expected_ids: &HashSet<crate::llm::InvocationId>,
    canonical: &HashMap<crate::llm::InvocationId, ToolCallCorrelation>,
    outcome: &mut HistoryRepairOutcome,
) -> (Vec<AgentMessage>, usize, HashSet<crate::llm::InvocationId>) {
    let mut repaired_results = Vec::new();
    let mut seen_result_ids = HashSet::new();
    let mut cursor = assistant_index + 1;

    while cursor < messages.len()
        && messages[cursor].resolved_kind() == AgentMessageKind::ToolResult
    {
        let tool_result = &messages[cursor];
        let Some(result_corr) = tool_result.resolved_tool_call_correlation() else {
            outcome.applied = true;
            outcome.dropped_tool_results = outcome.dropped_tool_results.saturating_add(1);
            cursor += 1;
            continue;
        };

        let inv_id = result_corr.invocation_id.clone();
        let Some(inv_id) = Some(inv_id).filter(|id| !id.as_str().trim().is_empty()) else {
            outcome.applied = true;
            outcome.dropped_tool_results = outcome.dropped_tool_results.saturating_add(1);
            cursor += 1;
            continue;
        };

        if !expected_ids.contains(&inv_id) || !seen_result_ids.insert(inv_id.clone()) {
            outcome.applied = true;
            outcome.dropped_tool_results = outcome.dropped_tool_results.saturating_add(1);
            cursor += 1;
            continue;
        }

        let Some(canonical_corr) = canonical.get(&inv_id) else {
            outcome.applied = true;
            outcome.dropped_tool_results = outcome.dropped_tool_results.saturating_add(1);
            cursor += 1;
            continue;
        };

        let mut repaired = tool_result.clone();
        if repaired.tool_call_correlation.as_ref() != Some(canonical_corr) {
            if result_corr.wire_tool_call_id().is_empty() {
                outcome.repaired_empty_wire_ids = outcome.repaired_empty_wire_ids.saturating_add(1);
            }
            repaired.tool_call_correlation = Some(canonical_corr.clone());
            if repaired
                .tool_call_id
                .as_deref()
                .is_none_or(|id| id.trim().is_empty())
            {
                repaired.tool_call_id = Some(inv_id.as_str().to_string());
            }
            outcome.applied = true;
        }

        repaired_results.push(repaired);
        cursor += 1;
    }

    (repaired_results, cursor, seen_result_ids)
}

/// Filter tool calls to only those with matching results.
fn filter_tool_calls_by_results(
    calls: Vec<ToolCall>,
    correlations: Vec<ToolCallCorrelation>,
    seen_results: &HashSet<crate::llm::InvocationId>,
    outcome: &mut HistoryRepairOutcome,
) -> (Vec<ToolCall>, Vec<ToolCallCorrelation>) {
    let original_count = calls.len();
    let pairs: Vec<_> = calls
        .into_iter()
        .zip(correlations)
        .filter(|(_, corr)| seen_results.contains(&corr.invocation_id))
        .collect();

    let filtered_calls: Vec<_> = pairs.iter().map(|(call, _)| call.clone()).collect();
    let filtered_corrs: Vec<_> = pairs.into_iter().map(|(_, corr)| corr).collect();

    if filtered_calls.len() != original_count {
        outcome.applied = true;
        outcome.trimmed_tool_calls = outcome
            .trimmed_tool_calls
            .saturating_add(original_count.saturating_sub(filtered_calls.len()));
    }

    (filtered_calls, filtered_corrs)
}

/// Build the final repaired batch from processed components.
fn build_repaired_batch(
    assistant: AgentMessage,
    calls: Vec<ToolCall>,
    correlations: Vec<ToolCallCorrelation>,
    tool_results: Vec<AgentMessage>,
    outcome: &mut HistoryRepairOutcome,
) -> Vec<AgentMessage> {
    let mut batch = Vec::new();

    if calls.is_empty() {
        if assistant.content.trim().is_empty() {
            outcome.applied = true;
            outcome.dropped_tool_call_messages =
                outcome.dropped_tool_call_messages.saturating_add(1);
        } else {
            let mut converted = assistant;
            converted.kind = AgentMessageKind::AssistantResponse;
            converted.role = MessageRole::Assistant;
            converted.tool_calls = None;
            converted.tool_call_correlations = None;
            outcome.applied = true;
            outcome.converted_tool_call_messages =
                outcome.converted_tool_call_messages.saturating_add(1);
            batch.push(converted);
        }
    } else {
        let mut repaired = assistant;
        repaired.tool_calls = Some(calls);
        repaired.tool_call_correlations = Some(correlations);
        batch.push(repaired);
    }

    batch.extend(tool_results);
    batch
}

fn repair_assistant_tool_batch(
    messages: &[AgentMessage],
    assistant_index: usize,
    allow_terminal_incomplete_batch: bool,
) -> (Vec<AgentMessage>, usize, HistoryRepairOutcome) {
    let assistant = messages[assistant_index].clone();
    let mut outcome = HistoryRepairOutcome::default();

    let Some(validated) = extract_valid_tool_calls(&assistant, &mut outcome) else {
        return (vec![assistant], assistant_index + 1, outcome);
    };

    let canonical: HashMap<_, _> = validated
        .correlations
        .iter()
        .cloned()
        .map(|c| (c.invocation_id.clone(), c))
        .collect();

    let (tool_results, cursor, seen_results) = process_tool_results(
        messages,
        assistant_index,
        &validated.expected_ids,
        &canonical,
        &mut outcome,
    );

    let is_terminal = cursor == messages.len();
    let preserve_incomplete = allow_terminal_incomplete_batch && is_terminal;

    let (final_calls, final_corrs) = if preserve_incomplete {
        (validated.calls, validated.correlations)
    } else {
        filter_tool_calls_by_results(
            validated.calls,
            validated.correlations,
            &seen_results,
            &mut outcome,
        )
    };

    let batch = build_repaired_batch(
        assistant,
        final_calls,
        final_corrs,
        tool_results,
        &mut outcome,
    );
    (batch, cursor, outcome)
}

/// Extract first valid JSON object from a string
/// This handles cases where JSON is followed by extra text
pub fn extract_first_json(input: &str) -> Option<String> {
    let mut depth = 0;
    let mut start_idx = None;
    let mut in_string = false;
    let mut escaped = false;

    for (i, ch) in input.char_indices() {
        match ch {
            '{' if !in_string => {
                if start_idx.is_none() {
                    start_idx = Some(i);
                }
                depth += 1;
            }
            '}' if !in_string => {
                if depth == 1
                    && let Some(start) = start_idx
                {
                    // Found complete object
                    let json_str = input[start..=i].trim();
                    // Validate it's actually JSON
                    if serde_json::from_str::<Value>(json_str).is_ok() {
                        return Some(json_str.to_string());
                    }
                }
                depth -= 1;
                if depth == 0 {
                    start_idx = None;
                }
            }
            '"' if !escaped => {
                in_string = !in_string;
            }
            '\\' if in_string => {
                escaped = !escaped;
            }
            _ => {}
        }
        if ch != '\\' {
            escaped = false;
        }
    }

    None
}

/// Extract JSON content from markdown code fences.
pub fn extract_fenced_json(input: &str) -> Option<String> {
    let fence = "```";
    let start = input.find(fence)?;
    let after_start = &input[start + fence.len()..];
    let end = after_start.find(fence)?;
    let mut block = after_start[..end].trim().to_string();

    block = strip_fence_language(&block);
    if block.is_empty() { None } else { Some(block) }
}

fn strip_fence_language(block: &str) -> String {
    let mut lines = block.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };

    let first_trim = first.trim();
    if first_trim.eq_ignore_ascii_case("json") || first_trim.eq_ignore_ascii_case("jsonc") {
        let rest = lines.collect::<Vec<_>>().join("\n");
        return rest.trim().to_string();
    }

    block.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolCallFunction;
    use std::collections::HashSet;

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
    fn test_extract_first_json_simple() {
        let input = r#"{"key": "value"}"#;
        let result = extract_first_json(input);
        assert!(result.is_some());
        if let Some(json) = result {
            assert_eq!(json, r#"{"key": "value"}"#);
        }
    }

    #[test]
    fn test_extract_first_json_with_trailing_text() {
        let input = r#"{"key": "value"} some extra text"#;
        let result = extract_first_json(input);
        assert!(result.is_some());
        if let Some(json) = result {
            assert_eq!(json, r#"{"key": "value"}"#);
        }
    }

    #[test]
    fn test_extract_first_json_nested() {
        let input = r#"{"outer": {"inner": "value"}}"#;
        let result = extract_first_json(input);
        assert!(result.is_some());
        if let Some(json) = result {
            let parsed = serde_json::from_str::<serde_json::Value>(&json)
                .expect("Failed to parse extracted JSON");
            assert_eq!(parsed["outer"]["inner"], "value");
        }
    }

    #[test]
    fn test_extract_first_json_invalid() {
        let input = "not json at all";
        let result = extract_first_json(input);
        assert!(result.is_none());
    }

    #[test]
    fn repair_agent_message_history_drops_orphaned_tool_results() {
        let messages = vec![
            AgentMessage::user("Question"),
            AgentMessage::tool("call-orphan", "search", "result"),
        ];

        let (repaired, outcome) = repair_agent_message_history(&messages);

        assert!(outcome.applied);
        assert_eq!(outcome.dropped_tool_results, 1);
        assert_eq!(repaired.len(), 1);
        assert_eq!(repaired[0].content, "Question");
    }

    #[test]
    fn repair_agent_message_history_trims_incomplete_parallel_batch() {
        let messages = vec![
            AgentMessage::assistant_with_tools(
                "Calling tools",
                vec![
                    tool_call("call-1", "search"),
                    tool_call("call-2", "read_file"),
                ],
            ),
            AgentMessage::tool("call-1", "search", "result-1"),
        ];

        let (repaired, outcome) = repair_agent_message_history(&messages);

        assert!(outcome.applied);
        assert_eq!(outcome.trimmed_tool_calls, 1);
        assert_eq!(repaired.len(), 2);
        let repaired_calls = repaired[0]
            .tool_calls
            .as_ref()
            .expect("assistant tool call must remain");
        assert_eq!(repaired_calls.len(), 1);
        assert_eq!(repaired_calls[0].id, "call-1");
        assert_eq!(repaired[1].tool_call_id.as_deref(), Some("call-1"));
    }

    #[test]
    fn prune_tool_history_by_availability_drops_stale_tool_results() {
        let messages = vec![
            AgentMessage::assistant_with_tools(
                "Calling tools",
                vec![
                    tool_call("call-1", "search"),
                    tool_call("call-2", "jira_read"),
                ],
            ),
            AgentMessage::tool("call-1", "search", "result-1"),
            AgentMessage::tool("call-2", "jira_read", "result-2"),
        ];
        let available = HashSet::from(["search".to_string()]);

        let (repaired, outcome) = prune_tool_history_by_availability(&messages, &available);

        assert!(outcome.applied);
        assert_eq!(outcome.dropped_tool_results, 1);
        assert_eq!(outcome.trimmed_tool_calls, 1);
        assert_eq!(repaired.len(), 2);
        let repaired_calls = repaired[0]
            .tool_calls
            .as_ref()
            .expect("assistant tool call must remain");
        assert_eq!(repaired_calls.len(), 1);
        assert_eq!(repaired_calls[0].function.name, "search");
        assert_eq!(repaired[1].tool_name.as_deref(), Some("search"));
    }

    #[test]
    fn prune_tool_history_by_availability_converts_empty_batch_to_assistant_text() {
        let messages = vec![AgentMessage::assistant_with_tools(
            "Need to check something first",
            vec![tool_call("call-1", "jira_read")],
        )];
        let available = HashSet::new();

        let (repaired, outcome) = prune_tool_history_by_availability(&messages, &available);

        assert!(outcome.applied);
        assert_eq!(outcome.trimmed_tool_calls, 1);
        assert_eq!(outcome.converted_tool_call_messages, 1);
        assert_eq!(repaired.len(), 1);
        assert_eq!(
            repaired[0].resolved_kind(),
            AgentMessageKind::AssistantResponse
        );
        assert!(repaired[0].tool_calls.is_none());
    }

    #[test]
    fn repair_agent_message_history_drops_duplicate_tool_results_from_same_batch() {
        let messages = vec![
            AgentMessage::assistant_with_tools(
                "Calling tools",
                vec![tool_call("call-1", "search")],
            ),
            AgentMessage::tool("call-1", "search", "result-1"),
            AgentMessage::tool("call-1", "search", "result-2"),
        ];

        let (repaired, outcome) = repair_agent_message_history(&messages);

        assert!(outcome.applied);
        assert_eq!(outcome.dropped_tool_results, 1);
        assert_eq!(repaired.len(), 2);
        assert_eq!(repaired[1].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(repaired[1].content, "result-1");
    }

    #[test]
    fn repair_agent_message_history_matches_on_invocation_id_not_raw_wire_id() {
        let correlation =
            ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("provider-call-1");
        let messages = vec![
            AgentMessage {
                kind: AgentMessageKind::AssistantToolCall,
                role: MessageRole::Assistant,
                content: "Calling tools".to_string(),
                created_at_unix: Some(1),
                reasoning: None,
                tool_call_id: None,
                tool_call_correlation: None,
                tool_name: None,
                tool_calls: Some(vec![ToolCall::new(
                    "provider-a".to_string(),
                    ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                    false,
                )]),
                tool_call_correlations: Some(vec![correlation.clone()]),
                attachments: Vec::new(),
                externalized_payload: None,
                pruned_artifact: None,
            },
            AgentMessage {
                kind: AgentMessageKind::ToolResult,
                role: MessageRole::Tool,
                content: "result".to_string(),
                created_at_unix: Some(2),
                reasoning: None,
                tool_call_id: Some("provider-b".to_string()),
                tool_call_correlation: Some(correlation),
                tool_name: Some("search".to_string()),
                tool_calls: None,
                tool_call_correlations: None,
                attachments: Vec::new(),
                externalized_payload: None,
                pruned_artifact: None,
            },
        ];

        let (repaired, outcome) = repair_agent_message_history(&messages);

        assert!(!outcome.applied);
        assert_eq!(repaired.len(), 2);
        assert_eq!(
            repaired[0]
                .resolved_tool_call_correlations()
                .expect("assistant correlations"),
            vec![
                ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("provider-call-1")
            ]
        );
    }

    #[test]
    fn repair_agent_message_history_repairs_empty_wire_ids() {
        // Test that empty provider_tool_call_id (wire_id) is repaired
        // This fixes "tool call id is invalid (2013)" errors with MiniMax
        let correlation =
            ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("".to_string()); // Empty wire_id
        let messages = vec![
            AgentMessage {
                kind: AgentMessageKind::AssistantToolCall,
                role: MessageRole::Assistant,
                content: "Calling tools".to_string(),
                created_at_unix: Some(1),
                reasoning: None,
                tool_call_id: None,
                tool_call_correlation: None,
                tool_name: None,
                tool_calls: Some(vec![ToolCall::new(
                    "call-1".to_string(),
                    ToolCallFunction {
                        name: "search".to_string(),
                        arguments: "{}".to_string(),
                    },
                    false,
                )]),
                tool_call_correlations: Some(vec![correlation.clone()]),
                attachments: Vec::new(),
                externalized_payload: None,
                pruned_artifact: None,
            },
            AgentMessage {
                kind: AgentMessageKind::ToolResult,
                role: MessageRole::Tool,
                content: "result".to_string(),
                created_at_unix: Some(2),
                reasoning: None,
                tool_call_id: Some("invoke-1".to_string()),
                tool_call_correlation: Some(correlation),
                tool_name: Some("search".to_string()),
                tool_calls: None,
                tool_call_correlations: None,
                attachments: Vec::new(),
                externalized_payload: None,
                pruned_artifact: None,
            },
        ];

        let (repaired, outcome) = repair_agent_message_history(&messages);

        assert!(outcome.applied);
        assert_eq!(outcome.repaired_empty_wire_ids, 2);
        assert_eq!(repaired.len(), 2);

        // Verify wire_id was repaired to invocation_id
        let repaired_correlations = repaired[0]
            .resolved_tool_call_correlations()
            .expect("assistant correlations");
        assert_eq!(repaired_correlations.len(), 1);
        assert_eq!(repaired_correlations[0].wire_tool_call_id(), "invoke-1");

        let repaired_tool_result_correlation = repaired[1]
            .resolved_tool_call_correlation()
            .expect("tool result correlation");
        assert_eq!(
            repaired_tool_result_correlation.wire_tool_call_id(),
            "invoke-1"
        );
    }
}
