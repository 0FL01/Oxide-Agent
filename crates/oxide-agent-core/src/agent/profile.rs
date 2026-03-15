//! Agent execution profile and tool access policy helpers.

use crate::llm::ToolDefinition;
use serde_json::Value;
use std::collections::HashSet;

const TOPIC_AGENT_DEFAULT_BLOCKED_TOOLS: &[&str] = &[
    "ytdlp_get_video_metadata",
    "ytdlp_download_transcript",
    "ytdlp_search_videos",
    "ytdlp_download_video",
    "ytdlp_download_audio",
];

const MANAGER_DEFAULT_BLOCKED_TOOLS: &[&str] = &[
    "delegate_to_sub_agent",
    "ytdlp_get_video_metadata",
    "ytdlp_download_transcript",
    "ytdlp_search_videos",
    "ytdlp_download_video",
    "ytdlp_download_audio",
];

/// Tool access policy derived from an agent profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolAccessPolicy {
    allowed_tools: Option<HashSet<String>>,
    blocked_tools: HashSet<String>,
}

impl ToolAccessPolicy {
    /// Create a new tool access policy.
    #[must_use]
    pub fn new(allowed_tools: Option<HashSet<String>>, blocked_tools: HashSet<String>) -> Self {
        Self {
            allowed_tools,
            blocked_tools,
        }
    }

    /// Returns the allowlist, if configured.
    #[must_use]
    pub fn allowed_tools(&self) -> Option<&HashSet<String>> {
        self.allowed_tools.as_ref()
    }

    /// Returns the explicit blocklist.
    #[must_use]
    pub fn blocked_tools(&self) -> &HashSet<String> {
        &self.blocked_tools
    }

    /// Returns true when the tool is allowed by this policy.
    #[must_use]
    pub fn allows(&self, tool_name: &str) -> bool {
        if self.blocked_tools.contains(tool_name) {
            return false;
        }

        match self.allowed_tools.as_ref() {
            Some(allowed) => allowed.contains(tool_name),
            None => true,
        }
    }

    /// Filter tool definitions for prompt exposure.
    #[must_use]
    pub fn filter_definitions(&self, tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
        tools
            .into_iter()
            .filter(|tool| self.allows(&tool.name))
            .collect()
    }

    /// Merge an additional blocklist into the policy.
    #[must_use]
    pub fn with_additional_blocked_tools<I, S>(mut self, blocked_tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.blocked_tools
            .extend(blocked_tools.into_iter().map(Into::into));
        self
    }
}

/// Default blocked tools for manager-mode agent sessions.
#[must_use]
pub fn manager_default_blocked_tools() -> Vec<String> {
    MANAGER_DEFAULT_BLOCKED_TOOLS
        .iter()
        .map(|tool| (*tool).to_string())
        .collect()
}

/// Default blocked tools for newly provisioned topic agents.
#[must_use]
pub fn topic_agent_default_blocked_tools() -> Vec<String> {
    TOPIC_AGENT_DEFAULT_BLOCKED_TOOLS
        .iter()
        .map(|tool| (*tool).to_string())
        .collect()
}

/// Parsed agent profile settings used at execution time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedAgentProfile {
    /// Optional prompt instructions contributed by the profile.
    pub prompt_instructions: Option<String>,
    /// Tool access policy for this profile.
    pub tool_policy: ToolAccessPolicy,
}

/// Execution profile applied to a live executor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentExecutionProfile {
    agent_id: Option<String>,
    prompt_instructions: Option<String>,
    tool_policy: ToolAccessPolicy,
}

impl AgentExecutionProfile {
    /// Create a new execution profile.
    #[must_use]
    pub fn new(
        agent_id: Option<String>,
        prompt_instructions: Option<String>,
        tool_policy: ToolAccessPolicy,
    ) -> Self {
        Self {
            agent_id,
            prompt_instructions,
            tool_policy,
        }
    }

    /// Current logical agent identifier.
    #[must_use]
    pub fn agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }

    /// Optional prompt instructions applied for this execution.
    #[must_use]
    pub fn prompt_instructions(&self) -> Option<&str> {
        self.prompt_instructions.as_deref()
    }

    /// Tool policy for this execution.
    #[must_use]
    pub fn tool_policy(&self) -> &ToolAccessPolicy {
        &self.tool_policy
    }
}

/// Parse an arbitrary JSON agent profile payload into execution settings.
#[must_use]
pub fn parse_agent_profile(value: &Value) -> ParsedAgentProfile {
    ParsedAgentProfile {
        prompt_instructions: parse_prompt_instructions(value),
        tool_policy: ToolAccessPolicy::new(
            parse_tool_name_set(value, "allowedTools", "allowed_tools"),
            parse_tool_name_set(value, "blockedTools", "blocked_tools").unwrap_or_default(),
        ),
    }
}

fn parse_prompt_instructions(value: &Value) -> Option<String> {
    let camel = value.get("systemPrompt").and_then(Value::as_str);
    let snake = value.get("system_prompt").and_then(Value::as_str);

    [camel, snake]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|prompt| !prompt.is_empty())
        .map(str::to_string)
}

fn parse_tool_name_set(value: &Value, camel_key: &str, snake_key: &str) -> Option<HashSet<String>> {
    let array = value
        .get(camel_key)
        .and_then(Value::as_array)
        .or_else(|| value.get(snake_key).and_then(Value::as_array))?;

    let tools = array
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|tool| !tool.is_empty())
        .map(str::to_string)
        .collect::<HashSet<_>>();

    Some(tools)
}

#[cfg(test)]
mod tests {
    use super::{
        manager_default_blocked_tools, parse_agent_profile, topic_agent_default_blocked_tools,
        AgentExecutionProfile, ToolAccessPolicy,
    };
    use crate::llm::ToolDefinition;
    use serde_json::json;
    use std::collections::HashSet;

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("tool {name}"),
            parameters: json!({ "type": "object" }),
        }
    }

    #[test]
    fn parse_agent_profile_supports_prompt_and_tool_policy() {
        let parsed = parse_agent_profile(&json!({
            "systemPrompt": "  you are infra  ",
            "allowedTools": ["todos_write", "execute_command", "execute_command"],
            "blockedTools": ["delegate_to_sub_agent"]
        }));

        assert_eq!(parsed.prompt_instructions.as_deref(), Some("you are infra"));
        assert!(parsed.tool_policy.allows("todos_write"));
        assert!(!parsed.tool_policy.allows("delegate_to_sub_agent"));
        assert!(!parsed.tool_policy.allows("unknown_tool"));
    }

    #[test]
    fn blocked_tools_override_allowlist() {
        let allowed = HashSet::from(["exec".to_string(), "sudo-exec".to_string()]);
        let blocked = HashSet::from(["sudo-exec".to_string()]);
        let policy = ToolAccessPolicy::new(Some(allowed), blocked);

        assert!(policy.allows("exec"));
        assert!(!policy.allows("sudo-exec"));
    }

    #[test]
    fn filter_definitions_respects_policy() {
        let policy =
            ToolAccessPolicy::new(Some(HashSet::from(["exec".to_string()])), HashSet::new());
        let filtered = policy.filter_definitions(vec![tool("exec"), tool("sudo-exec")]);

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "exec");
    }

    #[test]
    fn execution_profile_exposes_prompt_and_agent_id() {
        let profile = AgentExecutionProfile::new(
            Some("infra-agent".to_string()),
            Some("infra only".to_string()),
            ToolAccessPolicy::default(),
        );

        assert_eq!(profile.agent_id(), Some("infra-agent"));
        assert_eq!(profile.prompt_instructions(), Some("infra only"));
    }

    #[test]
    fn additional_blocked_tools_override_existing_policy() {
        let policy = ToolAccessPolicy::new(
            Some(HashSet::from([
                "delegate_to_sub_agent".to_string(),
                "execute_command".to_string(),
            ])),
            HashSet::new(),
        )
        .with_additional_blocked_tools(manager_default_blocked_tools());

        assert!(policy.allows("execute_command"));
        assert!(!policy.allows("delegate_to_sub_agent"));
    }

    #[test]
    fn topic_agent_default_blocklist_contains_ytdlp_tools_only() {
        let blocked = topic_agent_default_blocked_tools();

        assert!(blocked.iter().all(|tool| tool.starts_with("ytdlp_")));
        assert!(!blocked.iter().any(|tool| tool == "delegate_to_sub_agent"));
    }
}
