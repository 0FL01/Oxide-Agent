//! Recovery module for malformed LLM responses
//!
//! Handles sanitization of XML tags, JSON extraction, and recovery of malformed tool calls.

use crate::agent::compaction::AgentMessageKind;
use crate::agent::memory::{AgentMessage, MessageRole};
use crate::llm::{ToolCall, ToolCallFunction};
use lazy_regex::regex;
use serde_json::Value;
use std::collections::HashSet;
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

fn repair_assistant_tool_batch(
    messages: &[AgentMessage],
    assistant_index: usize,
    allow_terminal_incomplete_batch: bool,
) -> (Vec<AgentMessage>, usize, HistoryRepairOutcome) {
    let assistant = messages[assistant_index].clone();
    let mut outcome = HistoryRepairOutcome::default();
    let Some(tool_calls) = assistant.tool_calls.clone() else {
        return (vec![assistant], assistant_index + 1, outcome);
    };

    let mut expected_ids = HashSet::new();
    let mut valid_tool_calls = Vec::with_capacity(tool_calls.len());
    for tool_call in tool_calls {
        let tool_call_id = tool_call.id.trim();
        if tool_call_id.is_empty() || !expected_ids.insert(tool_call.id.clone()) {
            outcome.applied = true;
            outcome.trimmed_tool_calls = outcome.trimmed_tool_calls.saturating_add(1);
            continue;
        }
        valid_tool_calls.push(tool_call);
    }

    let mut repaired_tool_results = Vec::new();
    let mut seen_result_ids = HashSet::new();
    let mut cursor = assistant_index + 1;
    while cursor < messages.len()
        && messages[cursor].resolved_kind() == AgentMessageKind::ToolResult
    {
        let tool_result = &messages[cursor];
        let Some(tool_call_id) = tool_result
            .tool_call_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            outcome.applied = true;
            outcome.dropped_tool_results = outcome.dropped_tool_results.saturating_add(1);
            cursor += 1;
            continue;
        };

        if !expected_ids.contains(tool_call_id) || !seen_result_ids.insert(tool_call_id.to_string())
        {
            outcome.applied = true;
            outcome.dropped_tool_results = outcome.dropped_tool_results.saturating_add(1);
            cursor += 1;
            continue;
        }

        repaired_tool_results.push(tool_result.clone());
        cursor += 1;
    }

    let terminal_batch = cursor == messages.len();
    let preserve_incomplete_batch = allow_terminal_incomplete_batch && terminal_batch;
    if !preserve_incomplete_batch {
        let original_tool_call_count = valid_tool_calls.len();
        valid_tool_calls.retain(|tool_call| seen_result_ids.contains(&tool_call.id));
        if valid_tool_calls.len() != original_tool_call_count {
            outcome.applied = true;
            outcome.trimmed_tool_calls = outcome
                .trimmed_tool_calls
                .saturating_add(original_tool_call_count.saturating_sub(valid_tool_calls.len()));
        }
    }

    let mut repaired_batch = Vec::new();
    if valid_tool_calls.is_empty() {
        if assistant.content.trim().is_empty() {
            outcome.applied = true;
            outcome.dropped_tool_call_messages =
                outcome.dropped_tool_call_messages.saturating_add(1);
        } else {
            let mut converted = assistant;
            converted.kind = AgentMessageKind::AssistantResponse;
            converted.role = MessageRole::Assistant;
            converted.tool_calls = None;
            outcome.applied = true;
            outcome.converted_tool_call_messages =
                outcome.converted_tool_call_messages.saturating_add(1);
            repaired_batch.push(converted);
        }
    } else {
        let mut repaired_assistant = assistant;
        repaired_assistant.tool_calls = Some(valid_tool_calls);
        repaired_batch.push(repaired_assistant);
    }

    repaired_batch.extend(repaired_tool_results);
    (repaired_batch, cursor, outcome)
}

/// Sanitize leaked control XML tags from text.
///
/// This removes known control tags that may have leaked from malformed LLM responses.
/// Examples: `<tool_call>`, `</tool_call>`, `<filepath>`, `<arg_key>`, etc.
///
/// This function is public to allow reuse in integration tests, progress tracking,
/// todo descriptions, and other agent components that need protection from XML leaks.
pub fn sanitize_xml_tags(text: &str) -> String {
    control_xml_tag_pattern()
        .replace_all(text, " ")
        .trim()
        .to_string()
}

/// Check if text contains XML-like tags.
pub fn contains_xml_tags(text: &str) -> bool {
    control_xml_tag_pattern().is_match(text)
}

fn control_xml_tag_pattern() -> &'static regex::Regex {
    regex!(
        r"</?(?:tool_call|tool_name|filepath|arg_key|arg_value|command|query|url|content|directory|path|arg_key_[0-9]+|arg_value_[0-9]+|arg[0-9]+)>"
    )
}

/// Sanitize tool call by detecting malformed LLM responses where JSON arguments are placed in tool name
/// Returns (`corrected_name`, `corrected_arguments`)
pub fn sanitize_tool_call(name: &str, arguments: &str) -> (String, String) {
    let xml_sanitized_name = sanitize_xml_tags(name);
    let trimmed_name = xml_sanitized_name.trim();

    // PATTERN 1: Check if name looks like it contains JSON object (starts with { and has "todos" key)
    // Example: `{"todos": [{"description": "...", "status": "..."}]}`
    if trimmed_name.starts_with('{') && trimmed_name.contains("\"todos\"") {
        warn!(
            tool_name = %name,
            sanitized_name = %xml_sanitized_name,
            "Detected malformed tool call: JSON object in tool name field"
        );

        // Try to extract first valid JSON object from name
        let Some(json_str) = extract_first_json(trimmed_name) else {
            // If we can't parse, fall back to original
            warn!("Failed to extract JSON from malformed tool name");
            return (name.to_string(), arguments.to_string());
        };

        // Parse JSON to check structure
        if let Ok(parsed) = serde_json::from_str::<Value>(&json_str) {
            if parsed.is_object() && parsed.get("todos").is_some() {
                warn!("Correcting malformed tool call to 'write_todos' with extracted arguments");
                return ("write_todos".to_string(), json_str);
            }
        }
    }

    // PATTERN 2: Check if name contains "todos" followed by JSON array
    // Example: `"todos [{"description": "...", "status": "in_progress"}, ...]"`
    // Example: `"write_todos [...]"`
    if (trimmed_name.contains("todos") || trimmed_name.contains("write_todos"))
        && trimmed_name.contains('[')
    {
        // Extract the base tool name (everything before '[')
        if let Some(bracket_pos) = trimmed_name.find('[') {
            let base_name = trimmed_name[..bracket_pos].trim();
            let json_part = trimmed_name[bracket_pos..].trim();

            // Validate base name is one of the expected variants
            if base_name == "todos" || base_name == "write_todos" {
                warn!(
                    tool_name = %name,
                    sanitized_name = %xml_sanitized_name,
                    base_name = %base_name,
                    "Detected malformed tool call: JSON array appended to tool name"
                );

                // Try to parse the JSON array part and wrap it in the expected structure
                if let Ok(parsed_array) = serde_json::from_str::<Value>(json_part) {
                    if parsed_array.is_array() {
                        // Construct the proper arguments structure: {"todos": [...]}
                        let corrected_args = serde_json::json!({
                            "todos": parsed_array
                        });

                        if let Ok(args_str) = serde_json::to_string(&corrected_args) {
                            warn!(
                                corrected_name = "write_todos",
                                "Correcting malformed tool call: extracted array and wrapped in proper structure"
                            );
                            return ("write_todos".to_string(), args_str);
                        }
                    }
                }

                // If JSON parsing failed, log and fall back
                warn!(
                    json_part = %json_part,
                    "Failed to parse JSON array from malformed tool name"
                );
            }
        }
    }

    // Return unchanged if no issues detected
    if xml_sanitized_name != name {
        let normalized_name = normalize_tool_name(trimmed_name, name);
        return (normalized_name, arguments.to_string());
    }

    (name.to_string(), arguments.to_string())
}

fn normalize_tool_name(sanitized_name: &str, original_name: &str) -> String {
    let mut tokens = sanitized_name.split_whitespace();
    let Some(first) = tokens.next() else {
        warn!(
            tool_name = %original_name,
            sanitized_name = %sanitized_name,
            "Sanitized tool name is empty"
        );
        return String::new();
    };

    if tokens.next().is_some() {
        warn!(
            tool_name = %original_name,
            sanitized_name = %sanitized_name,
            normalized_name = %first,
            "Sanitized tool name contained extra tokens; using first token"
        );
    }

    first.to_string()
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
                if depth == 1 {
                    if let Some(start) = start_idx {
                        // Found complete object
                        let json_str = input[start..=i].trim();
                        // Validate it's actually JSON
                        if serde_json::from_str::<Value>(json_str).is_ok() {
                            return Some(json_str.to_string());
                        }
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
    if block.is_empty() {
        None
    } else {
        Some(block)
    }
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

/// Sanitize a vector of tool calls
pub fn sanitize_tool_calls(tool_calls: Vec<ToolCall>) -> Vec<ToolCall> {
    tool_calls
        .into_iter()
        .map(|call| {
            let (name, arguments) =
                sanitize_tool_call(&call.function.name, &call.function.arguments);
            ToolCall {
                id: call.id,
                function: ToolCallFunction { name, arguments },
                is_recovered: call.is_recovered,
            }
        })
        .collect()
}

/// Try to parse a malformed tool call from content text
///
/// This handles cases where the LLM generates XML-like syntax instead of proper JSON tool calls.
/// Example inputs:
/// - "read_file<filepath>/workspace/docker-compose.yml"
/// - "[Call tools: read_file]read_filepath..."
/// - "execute_command<command>ls -la</command>"
///
/// BUGFIX AGENT-2026-001: Extended to support ytdlp tools
pub fn try_parse_malformed_tool_call(content: &str) -> Option<ToolCall> {
    const TOOL_NAMES: [&str; 12] = [
        "read_file",
        "write_file",
        "execute_command",
        "list_files",
        "send_file_to_user",
        "recreate_sandbox",
        "upload_file",
        // BUGFIX AGENT-2026-001: Add ytdlp tools to malformed call recovery
        "ytdlp_get_video_metadata",
        "ytdlp_download_transcript",
        "ytdlp_search_videos",
        "ytdlp_download_video",
        "ytdlp_download_audio",
    ];

    for tool_name in TOOL_NAMES {
        if !content.contains(tool_name) {
            continue;
        }

        let Some(arguments) = extract_malformed_tool_arguments(tool_name, content) else {
            continue;
        };

        return build_recovered_tool_call(tool_name, arguments);
    }

    None
}

fn extract_malformed_tool_arguments(tool_name: &str, content: &str) -> Option<Value> {
    match tool_name {
        "read_file" => extract_read_file_arguments(content),
        "write_file" => extract_write_file_arguments(content),
        "execute_command" => extract_execute_command_arguments(content),
        "list_files" => extract_list_files_arguments(content),
        "send_file_to_user" => extract_send_file_to_user_arguments(content),
        "recreate_sandbox" => extract_recreate_sandbox_arguments(content),
        "upload_file" => extract_upload_file_arguments(content),
        "ytdlp_get_video_metadata" => extract_ytdlp_url_arguments(content, tool_name),
        "ytdlp_download_transcript" => extract_ytdlp_url_arguments(content, tool_name),
        "ytdlp_search_videos" => extract_ytdlp_search_arguments(content),
        "ytdlp_download_video" => extract_ytdlp_url_arguments(content, tool_name),
        "ytdlp_download_audio" => extract_ytdlp_url_arguments(content, tool_name),
        _ => None,
    }
}

/// Check if an extracted argument is valid (not garbage like `]`).
/// Returns true if the argument is at least 2 characters and contains alphanumeric.
fn is_valid_argument(arg: &str) -> bool {
    arg.len() >= 2 && arg.chars().any(|c| c.is_alphanumeric())
}

fn build_recovered_tool_call(tool_name: &str, arguments: Value) -> Option<ToolCall> {
    use uuid::Uuid;

    let arguments_str = serde_json::to_string(&arguments).ok()?;

    warn!(
        tool_name = tool_name,
        arguments = %arguments_str,
        "Recovered malformed tool call from content"
    );

    Some(ToolCall {
        id: format!("recovered_{}", Uuid::new_v4()),
        function: ToolCallFunction {
            name: tool_name.to_string(),
            arguments: arguments_str,
        },
        is_recovered: true,
    })
}

fn extract_tag_value<'a>(content: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = content.find(&open)? + open.len();
    let after_open = &content[start..];
    let end = after_open.find("</").unwrap_or(after_open.len());
    let value = after_open[..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn extract_token_after_tool_name<'a>(
    content: &'a str,
    tool_name: &str,
    optional_prefix: Option<&str>,
) -> Option<&'a str> {
    let idx = content.find(tool_name)?;
    let mut after = &content[idx + tool_name.len()..];
    after = after.trim_start();
    if let Some(prefix) = optional_prefix {
        after = after.strip_prefix(prefix).unwrap_or(after).trim_start();
    }

    let end = after
        .char_indices()
        .find(|(_, ch)| ch.is_whitespace() || *ch == '<')
        .map_or(after.len(), |(i, _)| i);
    let token = after[..end].trim();
    if token.is_empty() || !is_valid_argument(token) {
        None
    } else {
        Some(token)
    }
}

fn extract_read_file_arguments(content: &str) -> Option<Value> {
    if let Some(path) = extract_tag_value(content, "filepath") {
        return Some(serde_json::json!({ "path": path }));
    }

    extract_token_after_tool_name(content, "read_file", Some("path"))
        .map(|path| serde_json::json!({ "path": path }))
}

fn extract_write_file_arguments(content: &str) -> Option<Value> {
    let path = extract_tag_value(content, "filepath")?;
    let file_content = extract_tag_value(content, "content").unwrap_or("");
    Some(serde_json::json!({ "path": path, "content": file_content }))
}

fn extract_execute_command_arguments(content: &str) -> Option<Value> {
    if let Some(command) = extract_tag_value(content, "command") {
        return Some(serde_json::json!({ "command": command }));
    }

    extract_token_after_tool_name(content, "execute_command", Some("command"))
        .map(|command| serde_json::json!({ "command": command }))
}

fn extract_list_files_arguments(content: &str) -> Option<Value> {
    let path = extract_tag_value(content, "directory").unwrap_or("");
    Some(serde_json::json!({ "path": path }))
}

fn extract_send_file_to_user_arguments(content: &str) -> Option<Value> {
    if let Some(path) =
        extract_tag_value(content, "filepath").or_else(|| extract_tag_value(content, "path"))
    {
        return Some(serde_json::json!({ "path": path }));
    }

    None
}

fn extract_recreate_sandbox_arguments(content: &str) -> Option<Value> {
    let trimmed = content.trim();
    if trimmed.starts_with("recreate_sandbox")
        || trimmed.contains("[Call tools: recreate_sandbox]")
        || trimmed.contains("[Tool calls: recreate_sandbox]")
    {
        Some(serde_json::json!({}))
    } else {
        None
    }
}

fn extract_upload_file_arguments(content: &str) -> Option<Value> {
    if let Some(path) =
        extract_tag_value(content, "filepath").or_else(|| extract_tag_value(content, "path"))
    {
        return Some(serde_json::json!({ "path": path }));
    }

    None
}

fn extract_ytdlp_url_arguments(content: &str, tool_name: &str) -> Option<Value> {
    if let Some(url) = extract_tag_value(content, "url") {
        return Some(serde_json::json!({ "url": url }));
    }

    extract_token_after_tool_name(content, tool_name, Some("url"))
        .map(|url| serde_json::json!({ "url": url }))
}

fn extract_ytdlp_search_arguments(content: &str) -> Option<Value> {
    if let Some(query) = extract_tag_value(content, "query") {
        return Some(serde_json::json!({ "query": query }));
    }

    extract_token_after_tool_name(content, "ytdlp_search_videos", Some("query"))
        .map(|query| serde_json::json!({ "query": query }))
}

/// Check if text looks like a malformed tool call attempt
///
/// This detects patterns that indicate the LLM tried to call a tool but failed to use
/// proper JSON format. Examples:
/// - "[Call tools: ytdlp_get_video_metadataurl...]"
/// - "[Tool calls: read_file]read_filepath..."
/// - "ytdlp_download_videourl..."
pub fn looks_like_tool_call_text(text: &str) -> bool {
    // Pattern 1: Explicit tool call markers in Russian or English
    if text.contains("[Tool call") || text.contains("Tool calls:") {
        return true;
    }

    // Check for Russian markers
    if text.contains("Call tools") {
        return true;
    }

    // Pattern 2: Known tool names (simple contains check for malformed cases)
    let tool_names = [
        "ytdlp_get_video_metadata",
        "ytdlp_download_transcript",
        "ytdlp_search_videos",
        "ytdlp_download_video",
        "ytdlp_download_audio",
        "write_file",
        "read_file",
        "execute_command",
        "web_search",
        "web_extract",
        "list_files",
        "send_file_to_user",
        "upload_file",
        "write_todos",
    ];

    for tool_name in &tool_names {
        if text.contains(tool_name) {
            return true;
        }
    }

    false
}

/// Sanitize leaked XML from final response and return whether sanitization occurred
pub fn sanitize_leaked_xml(iteration: usize, final_response: &mut String) -> bool {
    if !control_xml_tag_pattern().is_match(final_response) {
        return false;
    }

    let original_len = final_response.len();
    warn!(
        model = %crate::config::get_agent_model(),
        iteration = iteration,
        "Detected leaked XML syntax in final response, sanitizing output"
    );

    // Remove all XML-like tags
    *final_response = sanitize_xml_tags(final_response);

    tracing::debug!(
        original_len = original_len,
        sanitized_len = final_response.len(),
        "XML tags removed from response"
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_sanitize_tool_call_normal() {
        let (name, args) = sanitize_tool_call("write_todos", "{}");
        assert_eq!(name, "write_todos");
        assert_eq!(args, "{}");
    }

    #[test]
    fn test_sanitize_tool_call_json_object_in_name() {
        let malformed_name = r#"{"todos": [{"description": "Task 1", "status": "pending"}]}"#;
        let (name, args) = sanitize_tool_call(malformed_name, "{}");

        assert_eq!(name, "write_todos");
        // Should extract the JSON and use it as arguments
        assert!(args.contains("\"todos\""));
        assert!(args.contains("Task 1"));
    }

    #[test]
    fn test_sanitize_tool_call_array_appended_to_todos() {
        let malformed_name = r#"todos [{"description": "Task 1", "status": "in_progress"}]"#;
        let (name, args) = sanitize_tool_call(malformed_name, "{}");

        assert_eq!(name, "write_todos");
        // Should wrap array in proper structure
        let parsed = serde_json::from_str::<serde_json::Value>(&args)
            .expect("Failed to parse corrected arguments");
        assert!(parsed.get("todos").is_some());
        assert!(parsed["todos"].is_array());
    }

    #[test]
    fn test_sanitize_tool_call_array_appended_to_write_todos() {
        let malformed_name =
            r#"write_todos [{"description": "Update deps", "status": "completed"}]"#;
        let (name, args) = sanitize_tool_call(malformed_name, "{}");

        assert_eq!(name, "write_todos");
        let parsed = serde_json::from_str::<serde_json::Value>(&args)
            .expect("Failed to parse corrected arguments");
        assert!(parsed.get("todos").is_some());
        assert!(parsed["todos"].is_array());
        assert_eq!(parsed["todos"][0]["description"], "Update deps");
    }

    #[test]
    fn test_sanitize_tool_call_complex_array() {
        let malformed_name = r#"todos [
            {"description": "Update yt-dlp to latest version", "status": "in_progress"},
            {"description": "Test new version", "status": "pending"},
            {"description": "Document changes", "status": "pending"}
        ]"#;
        let (name, args) = sanitize_tool_call(malformed_name, "{}");

        assert_eq!(name, "write_todos");
        let parsed = serde_json::from_str::<serde_json::Value>(&args)
            .expect("Failed to parse corrected arguments");
        assert!(parsed["todos"].is_array());
        let array = parsed["todos"]
            .as_array()
            .expect("todos should be an array");
        assert_eq!(array.len(), 3);
    }

    #[test]
    fn test_sanitize_tool_call_invalid_json() {
        let malformed_name = "todos [invalid json}";
        let (name, args) = sanitize_tool_call(malformed_name, "{}");

        // Should fall back to original when JSON is invalid
        assert_eq!(name, "todos [invalid json}");
        assert_eq!(args, "{}");
    }

    #[test]
    fn test_sanitize_tool_call_other_tools_unchanged() {
        let (name, args) = sanitize_tool_call("execute_command", r#"{"command": "ls"}"#);
        assert_eq!(name, "execute_command");
        assert_eq!(args, r#"{"command": "ls"}"#);
    }

    #[test]
    fn test_sanitize_tool_call_strips_xml_from_name() {
        let (name, args) = sanitize_tool_call("command</arg_key><arg_value>cd", "{}");
        assert_eq!(name, "command");
        assert_eq!(args, "{}");
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

    // Tests for sanitize_xml_tags function
    #[test]
    fn test_sanitize_xml_tags_basic() {
        let input = "Some text <tool_call>content</tool_call> more text";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "Some text  content  more text");
    }

    #[test]
    fn test_sanitize_xml_tags_filepath() {
        let input = "read_file<filepath>/workspace/docker-compose.yml</filepath></tool_call>";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "read_file /workspace/docker-compose.yml");
    }

    #[test]
    fn test_sanitize_xml_tags_multiple() {
        let input = "<arg_key>test</arg_key><arg_value>value</arg_value><command>ls</command>";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "test  value  ls");
    }

    #[test]
    fn test_sanitize_xml_tags_malformed_tool_call() {
        // Real-world example from bug report
        let input = "todos</arg_key><arg_value>[{\"description\": \"test\"}]";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "todos  [{\"description\": \"test\"}]");
        assert!(!result.contains("</arg_key>"));
        assert!(!result.contains("<arg_value>"));
    }

    #[test]
    fn test_sanitize_xml_tags_preserves_content() {
        let input = "Normal text without tags";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_sanitize_xml_tags_preserves_valid_comparison() {
        // Should preserve mathematical comparisons
        let input = "Check if x < 5 and y > 3";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_sanitize_xml_tags_preserves_html_tags() {
        let input = "<html><head><meta charset=\"utf-8\"></head><body><div>ok</div></body></html>";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_contains_xml_tags_detects_tags() {
        let input = "read_file<filepath>/workspace/file.txt</filepath>";
        assert!(contains_xml_tags(input));
    }

    #[test]
    fn test_contains_xml_tags_ignores_plain_text() {
        let input = "Plain response without tags.";
        assert!(!contains_xml_tags(input));
    }

    #[test]
    fn test_contains_xml_tags_ignores_html() {
        let input = "<div><span>ok</span></div>";
        assert!(!contains_xml_tags(input));
    }

    #[test]
    fn test_sanitize_xml_tags_only_lowercase() {
        // Should only match lowercase XML tags
        let input = "Text <ToolCall>content</ToolCall> <COMMAND>ls</COMMAND>";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, input); // Uppercase tags are preserved
    }

    #[test]
    fn test_sanitize_xml_tags_with_underscores() {
        let input = "<tool_name>search</tool_name><arg_key_1>value</arg_key_1>";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "search  value");
    }

    #[test]
    fn test_sanitize_xml_tags_with_numbers() {
        let input = "<arg1>first</arg1><arg2>second</arg2>";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "first  second");
    }

    #[test]
    fn test_looks_like_tool_call_text_with_russian_marker() {
        let input = "[Tool calls: ytdlp_get_video_metadataurl...]";
        assert!(looks_like_tool_call_text(input));
    }

    #[test]
    fn test_looks_like_tool_call_text_with_english_marker() {
        let input = "[Tool calls: read_file]read_filepath...";
        assert!(looks_like_tool_call_text(input));
    }

    #[test]
    fn test_looks_like_tool_call_text_with_ytdlp_tool_name() {
        let input = "ytdlp_get_video_metadataurl...";
        assert!(looks_like_tool_call_text(input));
    }

    #[test]
    fn test_looks_like_tool_call_text_with_other_tool_names() {
        assert!(looks_like_tool_call_text("execute_command ls"));
        assert!(looks_like_tool_call_text("read_file /path/to/file"));
        assert!(looks_like_tool_call_text("write_todos [...]"));
    }

    #[test]
    fn test_looks_like_tool_call_text_normal_text() {
        let input = "This is a normal response with some information about the task.";
        assert!(!looks_like_tool_call_text(input));
    }

    #[test]
    fn test_looks_like_tool_call_text_normal_russian_text() {
        let input = "Here is the result of task execution without tool calls.";
        assert!(!looks_like_tool_call_text(input));
    }

    // Tests for BUGFIX AGENT-2026-001: try_parse_malformed_tool_call with ytdlp
    #[test]
    fn test_try_parse_malformed_ytdlp_get_video_metadata() {
        let input = "ytdlp_get_video_metadata<url>https://youtube.com/watch?v=xxx</url>";
        let result = try_parse_malformed_tool_call(input);

        assert!(result.is_some());
        let tool_call = result.expect("tool_call should be Some");
        assert_eq!(tool_call.function.name, "ytdlp_get_video_metadata");

        let args: serde_json::Value = serde_json::from_str(&tool_call.function.arguments)
            .expect("arguments should be valid JSON");
        assert_eq!(args["url"], "https://youtube.com/watch?v=xxx");
    }

    #[test]
    fn test_try_parse_malformed_ytdlp_without_tags() {
        let input = "ytdlp_get_video_metadataurl https://youtube.com/watch?v=xxx";
        let result = try_parse_malformed_tool_call(input);

        assert!(result.is_some());
        let tool_call = result.expect("tool_call should be Some");
        assert_eq!(tool_call.function.name, "ytdlp_get_video_metadata");
    }

    #[test]
    fn test_try_parse_malformed_ytdlp_download_transcript() {
        let input = "ytdlp_download_transcripturlhttps://youtube.com/watch?v=yyy";
        let result = try_parse_malformed_tool_call(input);

        assert!(result.is_some());
        let tool_call = result.expect("tool_call should be Some");
        assert_eq!(tool_call.function.name, "ytdlp_download_transcript");
    }

    // Tests for is_valid_argument function (BUG-2026-0108-001 fix)
    #[test]
    fn test_is_valid_argument_rejects_single_bracket() {
        assert!(!is_valid_argument("]"));
        assert!(!is_valid_argument("["));
        assert!(!is_valid_argument("}"));
        assert!(!is_valid_argument("{"));
    }

    #[test]
    fn test_is_valid_argument_rejects_short() {
        assert!(!is_valid_argument(""));
        assert!(!is_valid_argument("a"));
        assert!(!is_valid_argument("1"));
    }

    #[test]
    fn test_is_valid_argument_rejects_special_only() {
        assert!(!is_valid_argument("][]"));
        assert!(!is_valid_argument("]]]"));
        assert!(!is_valid_argument("..."));
    }

    #[test]
    fn test_is_valid_argument_accepts_valid() {
        assert!(is_valid_argument("ls"));
        assert!(is_valid_argument("/path/to/file"));
        assert!(is_valid_argument("https://example.com"));
        assert!(is_valid_argument("git status"));
    }

    #[test]
    fn test_malformed_tool_call_rejects_bracket_argument() {
        // This should NOT create a recovered tool call because `]` is invalid
        let input = "execute_command]";
        let result = try_parse_malformed_tool_call(input);
        assert!(
            result.is_none(),
            "Should reject malformed call with `]` argument"
        );
    }

    #[test]
    fn test_recovered_tool_call_has_is_recovered_flag() {
        let input = "read_file<filepath>/workspace/test.rs</filepath>";
        let result = try_parse_malformed_tool_call(input);

        assert!(result.is_some());
        let tool_call = result.expect("tool_call should be Some");
        assert!(
            tool_call.is_recovered,
            "Recovered tool call should have is_recovered=true"
        );
    }

    #[test]
    fn test_try_parse_malformed_recreate_sandbox_without_args() {
        let input = "recreate_sandbox";
        let result = try_parse_malformed_tool_call(input);

        assert!(result.is_some());
        let tool_call = result.expect("tool_call should be Some");
        assert_eq!(tool_call.function.name, "recreate_sandbox");
        assert_eq!(tool_call.function.arguments, "{}");
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
}
