//! Recovery module for malformed LLM responses
//!
//! Handles sanitization of XML tags, JSON extraction, and recovery of malformed tool calls.

use crate::llm::{ToolCall, ToolCallFunction};
use lazy_regex::regex;
use serde_json::Value;
use tracing::warn;

/// Sanitize XML-like tags from text
///
/// This removes any XML-like tags that may have leaked from malformed LLM responses.
/// Examples: `<tool_call>`, `</tool_call>`, `<filepath>`, `<arg_key>`, etc.
///
/// This function is public to allow reuse in integration tests, progress tracking,
/// todo descriptions, and other agent components that need protection from XML leaks.
pub fn sanitize_xml_tags(text: &str) -> String {
    // Pattern to match opening and closing XML tags: <tag_name> or </tag_name>
    // Matches lowercase letters, digits, underscores in tag names
    let xml_tag_pattern = regex!(r"</?[a-z_][a-z0-9_]*>");

    xml_tag_pattern.replace_all(text, " ").trim().to_string()
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
/// - "read_file<filepath>/workspace/docker-compose.yml</tool_call>"
/// - "[Вызов инструментов: read_file]read_filepath..."
/// - "execute_command<command>ls -la</command>"
///
/// BUGFIX AGENT-2026-001: Extended to support ytdlp tools
pub fn try_parse_malformed_tool_call(content: &str) -> Option<ToolCall> {
    const TOOL_NAMES: [&str; 11] = [
        "read_file",
        "write_file",
        "execute_command",
        "list_files",
        "send_file_to_user",
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
/// - "[Вызов инструментов: ytdlp_get_video_metadataurl...]"
/// - "[Tool calls: read_file]read_filepath..."
/// - "ytdlp_download_videourl..."
pub fn looks_like_tool_call_text(text: &str) -> bool {
    // Pattern 1: Explicit tool call markers in Russian or English
    if text.contains("[Tool call") || text.contains("Tool calls:") {
        return true;
    }

    // Check for Russian markers
    if text.contains("Вызов инструмент") {
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
    let xml_tag_pattern = regex!(r"</?[a-z_][a-z0-9_]*>");
    if !xml_tag_pattern.is_match(final_response) {
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
            {"description": "Обновление yt-dlp до последней версии", "status": "in_progress"},
            {"description": "Тестирование новой версии", "status": "pending"},
            {"description": "Документирование изменений", "status": "pending"}
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

    // Tests for BUGFIX AGENT-2026-001: looks_like_tool_call_text
    #[test]
    fn test_looks_like_tool_call_text_with_russian_marker() {
        let input = "[Вызов инструментов: ytdlp_get_video_metadataurl...]";
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
        let input = "Вот результат выполнения задачи без вызова инструментов.";
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
}
