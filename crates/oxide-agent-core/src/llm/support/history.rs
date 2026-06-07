use std::collections::HashSet;

use super::super::{InvocationId, LlmError, Message, ProviderCapabilities, ToolCallCorrelation};

/// Stable system-message content prefixes that are cache-friendly.
///
/// Messages starting with these prefixes are placed before the date suffix in
/// the assembled system prompt, extending the cacheable prefix. All other
/// system messages are placed after the date suffix.
const STABLE_SYSTEM_PREFIXES: &[&str] = &["[TOPIC_AGENTS_MD]", "[OXIDE_COMPACTED_SUMMARY_V1]"];

/// Returns `true` if the system message content starts with a known stable prefix.
fn is_stable_system_message(content: &str) -> bool {
    let trimmed = content.trim();
    STABLE_SYSTEM_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

#[must_use]
/// Fold system-role messages into the system prompt with cache-aware ordering.
///
/// Assembly order: `system_prompt` + stable system messages + `date_suffix` + volatile
/// system messages. Non-system messages remain in the returned vector.
///
/// Stable messages (topic AGENTS.md, compacted summaries) extend the cacheable
/// prefix. Volatile messages (retry notes, temporal context) go after the date
/// suffix so they don't break the cache boundary.
pub(crate) fn fold_system_messages_into_prompt(
    system_prompt: &str,
    date_suffix: &str,
    messages: &[Message],
) -> (String, Vec<Message>) {
    let mut stable_parts: Vec<&str> = Vec::new();
    let mut volatile_parts: Vec<&str> = Vec::new();
    let mut normalized_messages = Vec::with_capacity(messages.len());

    for message in messages {
        if message.role == "system" {
            let content = message.content.trim();
            if content.is_empty() {
                continue;
            }

            if is_stable_system_message(content) {
                stable_parts.push(content);
            } else {
                volatile_parts.push(content);
            }
            continue;
        }

        normalized_messages.push(message.clone());
    }

    // Assemble: base + stable + date + volatile
    let mut result = String::with_capacity(
        system_prompt.len()
            + stable_parts.iter().map(|s| s.len() + 2).sum::<usize>()
            + date_suffix.len()
            + 2
            + volatile_parts.iter().map(|s| s.len() + 2).sum::<usize>(),
    );
    result.push_str(system_prompt.trim());

    for part in &stable_parts {
        result.push_str("\n\n");
        result.push_str(part);
    }

    let date_trimmed = date_suffix.trim();
    if !date_trimmed.is_empty() {
        result.push_str("\n\n");
        result.push_str(date_trimmed);
    }

    for part in &volatile_parts {
        result.push_str("\n\n");
        result.push_str(part);
    }

    (result, normalized_messages)
}

fn extract_expected_invocation_ids(message: &Message) -> Result<HashSet<InvocationId>, LlmError> {
    let mut expected_ids = HashSet::new();

    for correlation in message
        .resolved_tool_call_correlations()
        .unwrap_or_default()
    {
        let invocation_id = correlation.invocation_id.as_str().trim();
        if invocation_id.is_empty() {
            return Err(LlmError::RepairableHistory(
                "assistant tool call has an empty invocation_id".to_string(),
            ));
        }
        if !expected_ids.insert(correlation.invocation_id.clone()) {
            return Err(LlmError::RepairableHistory(format!(
                "assistant tool call batch contains duplicate invocation_id `{}`",
                correlation.invocation_id
            )));
        }
        if has_empty_explicit_provider_tool_call_id(&correlation) {
            return Err(LlmError::RepairableHistory(format!(
                "assistant tool call `{}` has an empty provider_tool_call_id",
                correlation.invocation_id
            )));
        }
    }

    Ok(expected_ids)
}

fn validate_tool_result_sequence(
    messages: &[Message],
    start_index: usize,
    expected_ids: &HashSet<InvocationId>,
) -> Result<(usize, HashSet<InvocationId>), LlmError> {
    let mut seen_results = HashSet::new();
    let mut cursor = start_index;

    while cursor < messages.len() && messages[cursor].role == "tool" {
        let result = &messages[cursor];
        let Some(result_correlation) = result.resolved_tool_call_correlation() else {
            return Err(LlmError::RepairableHistory(
                "tool result is missing invocation_id".to_string(),
            ));
        };

        if has_empty_explicit_provider_tool_call_id(&result_correlation) {
            return Err(LlmError::RepairableHistory(format!(
                "tool result for invocation_id `{}` has an empty provider_tool_call_id",
                result_correlation.invocation_id
            )));
        }

        let Some(invocation_id) = Some(result_correlation.invocation_id.clone())
            .filter(|id| !id.as_str().trim().is_empty())
        else {
            return Err(LlmError::RepairableHistory(
                "tool result is missing invocation_id".to_string(),
            ));
        };

        if !expected_ids.contains(&invocation_id) {
            return Err(LlmError::RepairableHistory(format!(
                "tool result references unknown invocation_id `{invocation_id}`"
            )));
        }

        if !seen_results.insert(invocation_id.clone()) {
            return Err(LlmError::RepairableHistory(format!(
                "tool result for invocation_id `{invocation_id}` is duplicated"
            )));
        }

        cursor += 1;
    }

    Ok((cursor, seen_results))
}

fn check_batch_completion(
    cursor: usize,
    messages_len: usize,
    expected_ids: &HashSet<InvocationId>,
    seen_results: &HashSet<InvocationId>,
    capabilities: ProviderCapabilities,
) -> Result<(), LlmError> {
    let batch_is_terminal = cursor == messages_len;
    let should_require_complete_batch = capabilities.strict_tool_history() || !batch_is_terminal;

    if should_require_complete_batch && seen_results.len() != expected_ids.len() {
        return Err(LlmError::RepairableHistory(format!(
            "assistant tool call batch is incomplete for {} tool history: {} tool calls but {} tool results",
            capabilities.tool_history_label(),
            expected_ids.len(),
            seen_results.len()
        )));
    }

    Ok(())
}

fn orphaned_tool_result_error(message: &Message) -> LlmError {
    let detail = message
        .resolved_tool_call_correlation()
        .map(|correlation| correlation.invocation_id)
        .filter(|id| !id.as_str().trim().is_empty())
        .map_or_else(
            || "orphaned tool result without invocation_id".to_string(),
            |invocation_id| {
                format!(
                    "orphaned tool result references missing assistant tool call `{invocation_id}`"
                )
            },
        );
    LlmError::RepairableHistory(detail)
}

pub(crate) fn validate_tool_history(
    messages: &[Message],
    capabilities: ProviderCapabilities,
) -> Result<(), LlmError> {
    let mut index = 0;

    while index < messages.len() {
        let message = &messages[index];

        if message.role == "assistant"
            && let Some(tool_calls) = &message.tool_calls {
                if tool_calls.is_empty() {
                    return Err(LlmError::RepairableHistory(
                        "assistant tool call batch is empty".to_string(),
                    ));
                }

                let expected_ids = extract_expected_invocation_ids(message)?;
                let (cursor, seen_results) =
                    validate_tool_result_sequence(messages, index + 1, &expected_ids)?;
                check_batch_completion(
                    cursor,
                    messages.len(),
                    &expected_ids,
                    &seen_results,
                    capabilities,
                )?;

                index = cursor;
                continue;
            }

        if message.role == "tool" {
            return Err(orphaned_tool_result_error(message));
        }

        index += 1;
    }

    Ok(())
}

fn has_empty_explicit_provider_tool_call_id(correlation: &ToolCallCorrelation) -> bool {
    correlation
        .provider_tool_call_id
        .as_ref()
        .is_some_and(|provider_tool_call_id| provider_tool_call_id.as_str().trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::{fold_system_messages_into_prompt, validate_tool_history};
    use crate::llm::{
        LlmError, Message, ProviderCapabilities, ToolCall, ToolCallCorrelation, ToolCallFunction,
        ToolHistoryMode,
    };

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
    fn fold_system_messages_appends_system_history_to_prompt() {
        let messages = vec![
            Message::system("[TOPIC_AGENTS_MD]\nAlways start with TL;DR."),
            Message::user("Hello"),
            Message::system("[SYSTEM: retry with strict JSON]"),
            Message::assistant("Hi"),
        ];

        let (system_prompt, normalized_messages) =
            fold_system_messages_into_prompt("Base system prompt.", "", &messages);

        assert_eq!(
            system_prompt,
            "Base system prompt.\n\n[TOPIC_AGENTS_MD]\nAlways start with TL;DR.\n\n[SYSTEM: retry with strict JSON]"
        );
        assert_eq!(normalized_messages.len(), 2);
        assert_eq!(normalized_messages[0].role, "user");
        assert_eq!(normalized_messages[1].role, "assistant");
    }

    #[test]
    fn fold_system_messages_skips_empty_system_entries() {
        let messages = vec![Message::system("   "), Message::user("Hello")];

        let (system_prompt, normalized_messages) =
            fold_system_messages_into_prompt("Base system prompt.", "", &messages);

        assert_eq!(system_prompt, "Base system prompt.");
        assert_eq!(normalized_messages.len(), 1);
        assert_eq!(normalized_messages[0].role, "user");
    }

    #[test]
    fn fold_stable_system_messages_before_date_volatile_after() {
        let messages = vec![
            Message::system("[SYSTEM: retry with strict JSON]"),
            Message::user("Hello"),
            Message::system("[TOPIC_AGENTS_MD]\nAlways start with TL;DR."),
            Message::assistant("Hi"),
            Message::system("[OXIDE_COMPACTED_SUMMARY_V1]\ngeneration: 3\nsummary text"),
            Message::user("Next"),
            Message::system("[TEMPORAL_CONTEXT]\nResume after 3 hour pause"),
        ];

        let (result, normalized_messages) = fold_system_messages_into_prompt(
            "Base prompt.",
            "### CURRENT DATE AND TIME\n2026-01-01",
            &messages,
        );

        // Stable messages (TopicAgentsMd, Summary) come BEFORE date.
        // Volatile messages (SYSTEM retry, TEMPORAL_CONTEXT) come AFTER date.
        let stable_idx = result.find("[TOPIC_AGENTS_MD]").unwrap();
        let summary_idx = result.find("[OXIDE_COMPACTED_SUMMARY_V1]").unwrap();
        let date_idx = result.find("### CURRENT DATE AND TIME").unwrap();
        let retry_idx = result.find("[SYSTEM: retry").unwrap();
        let temporal_idx = result.find("[TEMPORAL_CONTEXT]").unwrap();

        assert!(
            stable_idx < date_idx,
            "stable TopicAgentsMd must be before date_suffix, but {stable_idx} > {date_idx}"
        );
        assert!(
            summary_idx < date_idx,
            "stable Summary must be before date_suffix, but {summary_idx} > {date_idx}"
        );
        assert!(
            retry_idx > date_idx,
            "volatile retry must be after date_suffix, but {retry_idx} < {date_idx}"
        );
        assert!(
            temporal_idx > date_idx,
            "volatile temporal context must be after date_suffix, but {temporal_idx} < {date_idx}"
        );

        assert_eq!(normalized_messages.len(), 3); // user, assistant, user
    }

    #[test]
    fn fold_all_volatile_when_no_stable_prefixes() {
        let messages = vec![
            Message::system("[SYSTEM: retry note]"),
            Message::user("Hello"),
        ];

        let (result, _normalized) = fold_system_messages_into_prompt("Base.", "DATE", &messages);

        // All system messages are volatile (no stable prefix match) — all after date.
        let date_idx = result.find("DATE").unwrap();
        let retry_idx = result.find("[SYSTEM: retry").unwrap();
        assert!(
            retry_idx > date_idx,
            "volatile retry should be after date when no stable messages present"
        );
    }

    #[test]
    fn validate_tool_history_rejects_orphaned_tool_result() {
        let messages = vec![
            Message::user("hi"),
            Message::tool("call-1", "search", "result"),
        ];

        let error = validate_tool_history(
            &messages,
            ProviderCapabilities::new(ToolHistoryMode::Strict, true, true),
        )
        .expect_err("history must be rejected");
        assert!(matches!(error, LlmError::RepairableHistory(_)));
    }

    #[test]
    fn validate_tool_history_rejects_incomplete_parallel_batch() {
        let messages = vec![
            Message::assistant_with_tools(
                "calling tools",
                vec![
                    tool_call("call-1", "search"),
                    tool_call("call-2", "read_file"),
                ],
            ),
            Message::tool("call-1", "search", "result"),
        ];

        let error = validate_tool_history(
            &messages,
            ProviderCapabilities::new(ToolHistoryMode::Strict, true, true),
        )
        .expect_err("history must be rejected");
        assert!(matches!(error, LlmError::RepairableHistory(_)));
    }

    #[test]
    fn validate_tool_history_rejects_duplicate_tool_call_ids_in_assistant_batch() {
        let messages = vec![Message::assistant_with_tools(
            "calling tools",
            vec![
                tool_call("call-1", "search"),
                tool_call("call-1", "read_file"),
            ],
        )];

        let error = validate_tool_history(
            &messages,
            ProviderCapabilities::new(ToolHistoryMode::Strict, true, true),
        )
        .expect_err("history must be rejected");

        assert!(matches!(error, LlmError::RepairableHistory(_)));
    }

    #[test]
    fn validate_tool_history_rejects_duplicate_tool_results_for_same_call() {
        let messages = vec![
            Message::assistant_with_tools("calling tools", vec![tool_call("call-1", "search")]),
            Message::tool("call-1", "search", "result-1"),
            Message::tool("call-1", "search", "result-2"),
        ];

        let error = validate_tool_history(
            &messages,
            ProviderCapabilities::new(ToolHistoryMode::Strict, true, true),
        )
        .expect_err("history must be rejected");

        assert!(matches!(error, LlmError::RepairableHistory(_)));
    }

    #[test]
    fn validate_tool_history_allows_terminal_open_batch_for_best_effort_provider() {
        let messages = vec![
            Message::assistant_with_tools(
                "calling tools",
                vec![
                    tool_call("call-1", "search"),
                    tool_call("call-2", "read_file"),
                ],
            ),
            Message::tool("call-1", "search", "result"),
        ];

        let result = validate_tool_history(
            &messages,
            ProviderCapabilities::new(ToolHistoryMode::BestEffort, true, true),
        );

        assert!(
            result.is_ok(),
            "best-effort providers should allow an open terminal batch"
        );
    }

    #[test]
    fn validate_tool_history_rejects_nonterminal_open_batch_even_for_best_effort_provider() {
        let messages = vec![
            Message::assistant_with_tools(
                "calling tools",
                vec![
                    tool_call("call-1", "search"),
                    tool_call("call-2", "read_file"),
                ],
            ),
            Message::tool("call-1", "search", "result"),
            Message::user("follow up"),
        ];

        let error = validate_tool_history(
            &messages,
            ProviderCapabilities::new(ToolHistoryMode::BestEffort, true, true),
        )
        .expect_err("history must be rejected");
        assert!(matches!(error, LlmError::RepairableHistory(_)));
    }

    #[test]
    fn validate_tool_history_matches_on_invocation_id_not_raw_wire_id() {
        let correlation =
            ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("provider-call-1");
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: "calling tools".to_string(),
                content_parts: Vec::new(),
                reasoning_content: None,
                tool_call_id: None,
                tool_call_correlation: None,
                name: None,
                tool_calls: Some(vec![tool_call("provider-a", "search")]),
                tool_call_correlations: Some(vec![correlation.clone()]),
            },
            Message {
                role: "tool".to_string(),
                content: "result".to_string(),
                content_parts: Vec::new(),
                reasoning_content: None,
                tool_call_id: Some("provider-b".to_string()),
                tool_call_correlation: Some(correlation),
                name: Some("search".to_string()),
                tool_calls: None,
                tool_call_correlations: None,
            },
        ];

        let result = validate_tool_history(
            &messages,
            ProviderCapabilities::new(ToolHistoryMode::Strict, true, true),
        );

        assert!(
            result.is_ok(),
            "canonical invocation ids should drive matching"
        );
    }

    #[test]
    fn validate_tool_history_rejects_empty_explicit_provider_tool_call_id_in_assistant_batch() {
        let messages = vec![Message {
            role: "assistant".to_string(),
            content: "calling tools".to_string(),
            content_parts: Vec::new(),
            reasoning_content: None,
            tool_call_id: None,
            tool_call_correlation: None,
            name: None,
            tool_calls: Some(vec![tool_call("call-1", "search")]),
            tool_call_correlations: Some(vec![
                ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("")
            ]),
        }];

        let error = validate_tool_history(
            &messages,
            ProviderCapabilities::new(ToolHistoryMode::Strict, true, true),
        )
        .expect_err("history must be rejected");

        assert!(matches!(error, LlmError::RepairableHistory(_)));
    }

    #[test]
    fn validate_tool_history_rejects_empty_explicit_provider_tool_call_id_in_tool_result() {
        let assistant_correlation =
            ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("provider-call-1");
        let tool_result_correlation =
            ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("");
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: "calling tools".to_string(),
                content_parts: Vec::new(),
                reasoning_content: None,
                tool_call_id: None,
                tool_call_correlation: None,
                name: None,
                tool_calls: Some(vec![tool_call("call-1", "search")]),
                tool_call_correlations: Some(vec![assistant_correlation]),
            },
            Message {
                role: "tool".to_string(),
                content: "result".to_string(),
                content_parts: Vec::new(),
                reasoning_content: None,
                tool_call_id: Some("invoke-1".to_string()),
                tool_call_correlation: Some(tool_result_correlation),
                name: Some("search".to_string()),
                tool_calls: None,
                tool_call_correlations: None,
            },
        ];

        let error = validate_tool_history(
            &messages,
            ProviderCapabilities::new(ToolHistoryMode::Strict, true, true),
        )
        .expect_err("history must be rejected");

        assert!(matches!(error, LlmError::RepairableHistory(_)));
    }
}
