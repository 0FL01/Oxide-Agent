//! Property tests for remediated subsystems (V2: parse_structured_output).
//!
//! After Phase 1 (G1), the prose-wrap recovery path was removed.
//! `parse_structured_output` should:
//! - Return `Err` for non-JSON input (no prose-wrap fallback)
//! - Return `Ok` for valid `StructuredOutput` JSON

use oxide_agent_core::agent::structured_output::parse_structured_output;
use oxide_agent_core::llm::ToolDefinition;
use proptest::prop_assert;

proptest::proptest! {
    /// Any random non-JSON string that doesn't contain a valid JSON object
    /// should produce an Err (no prose-wrap after G1).
    #[test]
    fn proptest_non_json_returns_err(
        text in proptest::string::string_regex("[a-zA-Z0-9 .,!?;:'\"-]{1,100}").expect("valid regex")
    ) {
        // Skip strings that happen to be valid JSON objects
        if text.trim_start().starts_with('{') {
            return Ok(()); // proptest returns Ok(()) to skip
        }

        let tools: Vec<ToolDefinition> = vec![];
        let result = parse_structured_output(&text, &tools);
        prop_assert!(
            result.is_err(),
            "expected Err for non-JSON input: {:?}",
            text
        );
    }

    /// Any valid StructuredOutput JSON with final_answer should parse successfully.
    #[test]
    fn proptest_valid_structured_output_parses(
        thought in "[a-z ]{5,50}",
        answer in "[a-z0-9 .]{5,100}"
    ) {
        let json = format!(
            r#"{{"thought":"{}","final_answer":"{}"}}"#,
            thought, answer
        );
        let tools: Vec<ToolDefinition> = vec![];
        let result = parse_structured_output(&json, &tools);
        prop_assert!(
            result.is_ok(),
            "expected Ok for valid StructuredOutput JSON: {:?}, error: {:?}",
            json,
            result.err()
        );
    }

    /// Code-fenced valid JSON should still parse (fence-stripping is a deterministic lexer fix).
    #[test]
    fn proptest_fenced_json_parses(
        thought in "[a-z ]{5,50}",
        answer in "[a-z0-9 .]{5,100}"
    ) {
        let json = format!(
            r#"```json
{{"thought":"{}","final_answer":"{}"}}
```"#,
            thought, answer
        );
        let tools: Vec<ToolDefinition> = vec![];
        let result = parse_structured_output(&json, &tools);
        prop_assert!(
            result.is_ok(),
            "expected Ok for fenced JSON: error: {:?}",
            result.err()
        );
    }
}
