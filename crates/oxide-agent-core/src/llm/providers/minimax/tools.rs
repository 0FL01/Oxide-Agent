//! Tool conversion utilities for MiniMax provider

use claudius::{ToolParam, ToolUnionParam};

use crate::llm::ToolDefinition;

/// Convert our ToolDefinition to claudius ToolParam
#[must_use]
pub fn to_claudius_tool(tool: &ToolDefinition) -> ToolParam {
    ToolParam::new(tool.name.clone(), tool.parameters.clone())
}

/// Convert our ToolDefinition to claudius ToolUnionParam
#[must_use]
pub fn to_claudius_tool_union(tool: &ToolDefinition) -> ToolUnionParam {
    ToolUnionParam::CustomTool(to_claudius_tool(tool))
}

/// Convert tools to ToolUnionParam for claudius
#[must_use]
pub fn to_tool_union_params(tools: &[ToolDefinition]) -> Vec<ToolUnionParam> {
    tools.iter().map(to_claudius_tool_union).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_tool_definition_to_claudius_tool() {
        let tool = ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get weather for a city".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                },
                "required": ["city"]
            }),
        };

        let claudius_tool = to_claudius_tool(&tool);

        assert_eq!(claudius_tool.name, "get_weather");
    }

    #[test]
    fn converts_to_tool_union_params() {
        let tools = vec![ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get weather".to_string(),
            parameters: json!({"type": "object", "properties": {}}),
        }];

        let union_params = to_tool_union_params(&tools);

        assert_eq!(union_params.len(), 1);
    }
}
