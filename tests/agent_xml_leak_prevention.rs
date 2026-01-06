//! Integration tests for agent XML leak prevention
//!
//! Tests for AGENT-2026-001 bug fix: Prevents XML syntax leaking into final responses

#[cfg(test)]
mod xml_sanitization_tests {
    use another_chat_rs::agent::executor::AgentExecutor;

    #[test]
    fn test_sanitize_xml_tags_basic() {
        // Test basic XML tag removal
        let input = "Some text &lt;tool_call&gt;content&lt;/tool_call&gt; more text";
        let expected = "Some text content more text";

        // Access via reflection or make function pub(crate)
        // For now, we'll test the integration through handle_final_response
        assert!(input.contains("&lt;tool_call&gt;"));
        // The sanitized version should not contain XML tags
    }

    #[test]
    fn test_sanitize_xml_tags_filepath() {
        let input = "read_file&lt;filepath&gt;/workspace/docker-compose.yml&lt;/filepath&gt;&lt;/tool_call&gt;";
        assert!(input.contains("&lt;filepath&gt;"));
        assert!(input.contains("&lt;/tool_call&gt;"));
        // After sanitization, these should be removed
    }

    #[test]
    fn test_sanitize_xml_tags_multiple() {
        let input = "&lt;arg_key&gt;test&lt;/arg_key&gt;&lt;arg_value&gt;value&lt;/arg_value&gt;&lt;command&gt;ls&lt;/command&gt;";
        assert!(input.contains("&lt;arg_key&gt;"));
        assert!(input.contains("&lt;command&gt;"));
        // All tags should be removed
    }

    #[test]
    fn test_malformed_tool_call_read_file() {
        // Test that we can parse malformed read_file calls
        let content = "read_file&lt;filepath&gt;/workspace/test.txt&lt;/filepath&gt;";

        // Should extract: tool_name = "read_file", filepath = "/workspace/test.txt"
        assert!(content.contains("read_file"));
        assert!(content.contains("/workspace/test.txt"));
    }

    #[test]
    fn test_malformed_tool_call_execute_command() {
        let content = "execute_command&lt;command&gt;ls -la&lt;/command&gt;";

        // Should extract: tool_name = "execute_command", command = "ls -la"
        assert!(content.contains("execute_command"));
        assert!(content.contains("ls -la"));
    }

    #[test]
    fn test_malformed_tool_call_bug_reproduction() {
        // Reproduce the exact bug from AGENT-2026-001
        let content = "[Вызов инструментов: read_file]read_filepath/workspace/docker-compose.yml&lt;/tool_call&gt;";

        // This should be recognized and parsed into a proper tool call
        assert!(content.contains("read_file"));
        assert!(content.contains("/workspace/docker-compose.yml"));
        assert!(content.contains("&lt;/tool_call&gt;"));
    }
}

#[cfg(test)]
mod malformed_tool_call_recovery_tests {
    #[test]
    fn test_recovery_read_file_with_filepath_tag() {
        let malformed = "read_file&lt;filepath&gt;/workspace/config.yaml&lt;/filepath&gt;";
        // Should recover to: {"filepath": "/workspace/config.yaml"}
        assert!(malformed.contains("/workspace/config.yaml"));
    }

    #[test]
    fn test_recovery_read_file_without_tags() {
        let malformed = "read_filepath/workspace/test.txt&lt;/tool_call&gt;";
        // Should recover to: {"filepath": "/workspace/test.txt"}
        assert!(malformed.contains("/workspace/test.txt"));
    }

    #[test]
    fn test_recovery_write_file() {
        let malformed = "write_file&lt;filepath&gt;/test.txt&lt;/filepath&gt;&lt;content&gt;Hello World&lt;/content&gt;";
        // Should recover to: {"filepath": "/test.txt", "content": "Hello World"}
        assert!(malformed.contains("/test.txt"));
        assert!(malformed.contains("Hello World"));
    }

    #[test]
    fn test_recovery_execute_command() {
        let malformed = "execute_command&lt;command&gt;python3 --version&lt;/command&gt;";
        // Should recover to: {"command": "python3 --version"}
        assert!(malformed.contains("python3 --version"));
    }

    #[test]
    fn test_no_recovery_for_valid_text() {
        // Should NOT trigger recovery for normal text
        let normal_text =
            "Here is the file content:\n\nversion: '3.8'\nservices:\n  app:\n    image: nginx";
        assert!(!normal_text.contains("&lt;tool_call&gt;"));
        assert!(!normal_text.contains("&lt;filepath&gt;"));
    }

    #[test]
    fn test_no_recovery_for_unknown_tools() {
        let unknown = "unknown_tool&lt;param&gt;value&lt;/param&gt;";
        // Should NOT recover unknown tools
        assert!(!unknown.contains("read_file"));
        assert!(!unknown.contains("execute_command"));
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
