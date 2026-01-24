use insta::assert_snapshot;
use oxide_agent_core::agent::prompt::composer::{
    build_structured_output_instructions, get_fallback_prompt,
};
use oxide_agent_core::llm::ToolDefinition;
use serde_json::json;

#[test]
fn test_fallback_prompt_snapshot() {
    assert_snapshot!(get_fallback_prompt());
}

#[test]
fn test_structured_output_instructions_snapshot() {
    let tools = vec![ToolDefinition {
        name: "test_tool".to_string(),
        description: "A test tool".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "arg1": { "type": "string" }
            }
        }),
    }];
    assert_snapshot!(build_structured_output_instructions(&tools));
}
