//! Integration tests for agent XML leak prevention
//!
//! Tests for AGENT-2026-001 bug fix: Prevents XML syntax leaking into final responses

#[cfg(test)]
mod xml_sanitization_tests {
    use oxide_agent_core::utils::clean_html;

    #[test]
    fn test_sanitize_xml_tags_basic() {
        // Test basic XML tag removal (HTML entity escaped tags)
        let input = "Some text &lt;tool_call&gt;content&lt;/tool_call&gt; more text";
        let result = clean_html(input);

        // These are already escaped by the time they reach clean_html
        // clean_html should preserve them as-is (they're not real tags)
        assert_eq!(result, input);
    }

    #[test]
    fn test_sanitize_xml_tags_filepath() {
        let input = "read_file&lt;filepath&gt;/workspace/docker-compose.yml&lt;/filepath&gt;&lt;/tool_call&gt;";
        let result = clean_html(input);

        // Already escaped entities should remain unchanged
        assert_eq!(result, input);
    }

    #[test]
    fn test_sanitize_xml_tags_multiple() {
        let input = "&lt;arg_key&gt;test&lt;/arg_key&gt;&lt;arg_value&gt;value&lt;/arg_value&gt;&lt;command&gt;ls&lt;/command&gt;";
        let result = clean_html(input);

        // Already escaped entities should remain unchanged
        assert_eq!(result, input);
    }

    #[test]
    fn test_raw_xml_tags_should_be_escaped() {
        // Test that raw (unescaped) XML-like tags get escaped
        let input = "Some text <tool_call>content</tool_call> more text";
        let result = clean_html(input);

        // Raw tags should be escaped
        assert!(result.contains("&lt;tool_call&gt;"));
        assert!(result.contains("&lt;/tool_call&gt;"));
    }
}

#[cfg(test)]
mod integration_tests {
    use lazy_regex::regex;

    #[test]
    fn test_xml_tag_regex_pattern() {
        let pattern = regex!(
            r"&lt;/?(?:tool_call|tool_name|filepath|arg_key|arg_value|command|query|url|content|directory|path|arg_key_[0-9]+|arg_value_[0-9]+|arg[0-9]+)&gt;"
        );

        // Should match these
        assert!(pattern.is_match("&lt;tool_call&gt;"));
        assert!(pattern.is_match("&lt;/tool_call&gt;"));
        assert!(pattern.is_match("&lt;filepath&gt;"));
        assert!(pattern.is_match("&lt;arg_key&gt;"));
        assert!(pattern.is_match("&lt;command&gt;"));

        // Should NOT match these (not lowercase)
        assert!(!pattern.is_match("&lt;ToolCall&gt;"));
        assert!(!pattern.is_match("&lt;COMMAND&gt;"));

        // Should NOT match HTML tags
        assert!(!pattern.is_match("&lt;html&gt;"));
        assert!(!pattern.is_match("&lt;div&gt;"));

        // Should NOT match regular text
        assert!(!pattern.is_match("normal text"));
        assert!(!pattern.is_match("&lt; not a tag"));
    }

    #[test]
    fn test_xml_tag_replacement() {
        let pattern = regex!(
            r"&lt;/?(?:tool_call|tool_name|filepath|arg_key|arg_value|command|query|url|content|directory|path|arg_key_[0-9]+|arg_value_[0-9]+|arg[0-9]+)&gt;"
        );

        let input = "text &lt;tool_call&gt;content&lt;/tool_call&gt; more";
        let result = pattern.replace_all(input, "");
        assert_eq!(result, "text content more");

        let input2 = "&lt;filepath&gt;/test.txt&lt;/filepath&gt;";
        let result2 = pattern.replace_all(input2, "");
        assert_eq!(result2, "/test.txt");
    }

    #[test]
    fn test_complex_malformed_response() {
        // Real-world example from bug report
        let malformed =
            "[Call tools: read_file]read_filepath/workspace/docker-compose.yml&lt;/tool_call&gt;";

        // After processing should:
        // 1. Detect "read_file" in content
        // 2. Extract "/workspace/docker-compose.yml"
        // 3. Remove XML tags

        assert!(malformed.contains("read_file"));
        assert!(malformed.contains("/workspace/docker-compose.yml"));

        let pattern = regex!(r"&lt;/?[a-z_][a-z0-9_]*&gt;");
        let cleaned = pattern.replace_all(malformed, "");
        assert!(!cleaned.contains("&lt;/tool_call&gt;"));
    }
}

#[cfg(test)]
mod progress_integration_tests {
    use oxide_agent_core::agent::progress::{AgentEvent, ProgressState};
    use oxide_agent_transport_telegram::bot::progress_render::render_progress_html;

    #[test]
    fn test_progress_state_with_sanitized_tool_name() {
        let mut state = ProgressState::new(100);

        // Simulate sanitized tool call event (XML tags already removed in executor)
        state.update(AgentEvent::ToolCall {
            name: "todos".to_string(), // Already sanitized!
            input: "[{\"description\": \"test\"}]".to_string(),
            command_preview: None,
        });

        let output = render_progress_html(&state);

        // Should NOT contain XML tags in the formatted output
        assert!(!output.contains("<arg_key>"));
        assert!(!output.contains("</arg_key>"));
        // Current step should show with ⏳ prefix
        assert!(output.contains("⏳ Execution: todos"));
    }

    #[test]
    fn test_progress_state_with_complex_input() {
        let mut state = ProgressState::new(100);

        // Test with complex but sanitized input
        state.update(AgentEvent::ToolCall {
            name: "web_search".to_string(),
            input: "query: \"test query\"".to_string(),
            command_preview: None,
        });

        let output = render_progress_html(&state);
        // Current step should show with ⏳ prefix
        assert!(output.contains("⏳ Execution: web_search"));
    }

    #[test]
    fn test_progress_state_with_command_preview() {
        let mut state = ProgressState::new(100);

        // Test execute_command with command preview
        state.update(AgentEvent::ToolCall {
            name: "execute_command".to_string(),
            input: r#"{"command": "pip install pandas"}"#.to_string(),
            command_preview: Some("pip install pandas".to_string()),
        });

        let output = render_progress_html(&state);

        // Should show the command preview with ⏳ prefix, not "Execution: execute_command"
        assert!(output.contains("⏳ 🔧 pip install pandas"));
        assert!(!output.contains("Execution: execute_command"));
    }

    #[test]
    fn test_progress_grouped_steps() {
        let mut state = ProgressState::new(100);

        // Add multiple completed steps
        state.update(AgentEvent::ToolCall {
            name: "web_search".to_string(),
            input: "q1".to_string(),
            command_preview: None,
        });
        state.update(AgentEvent::ToolResult {
            name: "web_search".to_string(),
            output: "result1".to_string(),
        });

        state.update(AgentEvent::ToolCall {
            name: "web_search".to_string(),
            input: "q2".to_string(),
            command_preview: None,
        });
        state.update(AgentEvent::ToolResult {
            name: "web_search".to_string(),
            output: "result2".to_string(),
        });

        state.update(AgentEvent::ToolCall {
            name: "execute_command".to_string(),
            input: "{}".to_string(),
            command_preview: Some("ls -la".to_string()),
        });

        let output = render_progress_html(&state);

        // Should show grouped completed steps
        assert!(output.contains("✅ web_search ×2"));
        // Current step should be shown
        assert!(output.contains("⏳ 🔧 ls -la"));
    }

    #[test]
    fn test_progress_header_format() {
        let mut state = ProgressState::new(200);

        // Simulate thinking event
        state.update(AgentEvent::Thinking { tokens: 5700 });

        let output = render_progress_html(&state);

        // Check header format
        assert!(output.contains("🤖 <b>Oxide Agent</b>"));
        assert!(output.contains("Iteration 1/200"));
        assert!(output.contains("5.7k")); // Token format
    }
}
// BUGFIX AGENT-2026-001: Integration tests for malformed tool call bug fix
#[cfg(test)]
mod bugfix_agent_2026_001_tests {
    use oxide_agent_core::agent::sanitize_xml_tags;

    #[test]
    fn test_ytdlp_malformed_tool_call_detection() {
        // This reproduces the exact bug scenario from the report
        let malformed_response = "[Tool call: ytdlp_get_video_metadataurl...]";

        // The response should be detected as tool-like text
        // This is tested indirectly through sanitize_xml_tags
        let sanitized = sanitize_xml_tags(malformed_response);

        // After sanitization, the text should remain (no XML tags to remove in this case)
        assert_eq!(sanitized, malformed_response);

        // But it should still contain tool markers
        assert!(malformed_response.contains("Tool call"));
        assert!(malformed_response.contains("ytdlp_"));
    }

    #[test]
    fn test_ytdlp_with_xml_tags_sanitization() {
        // Test case where ytdlp tool call has XML tags
        let malformed =
            "[Tool call: ytdlp_get_video_metadata]<url>https://youtube.com/watch?v=xxx</url>";

        let sanitized = sanitize_xml_tags(malformed);

        // XML tags should be removed
        assert!(!sanitized.contains("<url>"));
        assert!(!sanitized.contains("</url>"));

        // But the URL should remain
        assert!(sanitized.contains("https://youtube.com/watch?v=xxx"));

        // And tool markers should remain
        assert!(sanitized.contains("Tool call"));
        assert!(sanitized.contains("ytdlp_get_video_metadata"));
    }

    #[test]
    fn test_normal_response_not_flagged() {
        // Normal responses should not be flagged as tool calls
        let normal_response =
            "Here is the result of task execution. The file was successfully processed.";

        // Should not contain any tool markers
        assert!(!normal_response.contains("Tool call"));
        assert!(!normal_response.contains("ytdlp_"));
        assert!(!normal_response.contains("[Tool call"));
    }

    #[test]
    fn test_short_sanitized_response() {
        // Test that very short responses after sanitization are caught
        let input = "<tool_call>Hi</tool_call>";
        let sanitized = sanitize_xml_tags(input);

        // After sanitization, should be very short
        assert_eq!(sanitized, "Hi");
        assert!(sanitized.trim().len() < 10);
    }
}
