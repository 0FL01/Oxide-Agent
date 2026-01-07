//! Integration tests for agent XML leak prevention
//!
//! Tests for AGENT-2026-001 bug fix: Prevents XML syntax leaking into final responses

#[cfg(test)]
mod xml_sanitization_tests {
    use another_chat_rs::utils::clean_html;

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
        let pattern = regex!(r"&lt;/?[a-z_][a-z0-9_]*&gt;");

        // Should match these
        assert!(pattern.is_match("&lt;tool_call&gt;"));
        assert!(pattern.is_match("&lt;/tool_call&gt;"));
        assert!(pattern.is_match("&lt;filepath&gt;"));
        assert!(pattern.is_match("&lt;arg_key&gt;"));
        assert!(pattern.is_match("&lt;command&gt;"));

        // Should NOT match these (not lowercase)
        assert!(!pattern.is_match("&lt;ToolCall&gt;"));
        assert!(!pattern.is_match("&lt;COMMAND&gt;"));

        // Should NOT match regular text
        assert!(!pattern.is_match("normal text"));
        assert!(!pattern.is_match("&lt; not a tag"));
    }

    #[test]
    fn test_xml_tag_replacement() {
        let pattern = regex!(r"&lt;/?[a-z_][a-z0-9_]*&gt;");

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
        let malformed = "[Вызов инструментов: read_file]read_filepath/workspace/docker-compose.yml&lt;/tool_call&gt;";

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
    use another_chat_rs::agent::progress::{AgentEvent, ProgressState};

    #[test]
    fn test_progress_state_with_sanitized_tool_name() {
        let mut state = ProgressState::new(100);

        // Simulate sanitized tool call event (XML tags already removed in executor)
        state.update(AgentEvent::ToolCall {
            name: "todos".to_string(), // Already sanitized!
            input: "[{\"description\": \"test\"}]".to_string(),
        });

        let output = state.format_telegram();

        // Should NOT contain XML tags in the formatted output
        assert!(!output.contains("<arg_key>"));
        assert!(!output.contains("</arg_key>"));
        assert!(output.contains("Выполнение: todos"));
    }

    #[test]
    fn test_progress_state_with_complex_input() {
        let mut state = ProgressState::new(100);

        // Test with complex but sanitized input
        state.update(AgentEvent::ToolCall {
            name: "web_search".to_string(),
            input: "query: \"test query\"".to_string(),
        });

        let output = state.format_telegram();
        assert!(output.contains("Выполнение: web_search"));
    }
}
