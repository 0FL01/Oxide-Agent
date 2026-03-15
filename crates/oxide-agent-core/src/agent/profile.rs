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

const TOPIC_AGENT_MANAGEABLE_HOOKS: &[&str] = &[
    "workload_distributor",
    "delegation_guard",
    "search_budget",
    "timeout_report",
];

const TOPIC_AGENT_PROTECTED_HOOKS: &[&str] = &["completion_check", "tool_access_policy"];

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

    /// Merge additional tools into the allowlist when one is configured.
    #[must_use]
    pub fn with_additional_allowed_tools<I, S>(mut self, allowed_tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        if let Some(existing_allowed_tools) = self.allowed_tools.as_mut() {
            existing_allowed_tools.extend(allowed_tools.into_iter().map(Into::into));
        }
        self
    }
}

/// Hook access policy derived from an agent profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookAccessPolicy {
    enabled_hooks: Option<HashSet<String>>,
    disabled_hooks: HashSet<String>,
}

impl HookAccessPolicy {
    /// Create a new hook access policy.
    #[must_use]
    pub fn new(enabled_hooks: Option<HashSet<String>>, disabled_hooks: HashSet<String>) -> Self {
        Self {
            enabled_hooks,
            disabled_hooks,
        }
    }

    /// Returns the allowlist, if configured.
    #[must_use]
    pub fn enabled_hooks(&self) -> Option<&HashSet<String>> {
        self.enabled_hooks.as_ref()
    }

    /// Returns the explicit blocklist.
    #[must_use]
    pub fn disabled_hooks(&self) -> &HashSet<String> {
        &self.disabled_hooks
    }

    /// Returns true when the hook is allowed by this policy.
    #[must_use]
    pub fn allows(&self, hook_name: &str) -> bool {
        if self.disabled_hooks.contains(hook_name) {
            return false;
        }

        match self.enabled_hooks.as_ref() {
            Some(enabled) => enabled.contains(hook_name),
            None => true,
        }
    }

    /// Merge an additional blocklist into the policy.
    #[must_use]
    pub fn with_additional_disabled_hooks<I, S>(mut self, disabled_hooks: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.disabled_hooks
            .extend(disabled_hooks.into_iter().map(Into::into));
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

/// Hooks that managers may enable or disable for topic agents.
#[must_use]
pub fn topic_agent_manageable_hooks() -> Vec<String> {
    TOPIC_AGENT_MANAGEABLE_HOOKS
        .iter()
        .map(|hook| (*hook).to_string())
        .collect()
}

/// Hooks that remain always active for topic agents.
#[must_use]
pub fn topic_agent_protected_hooks() -> Vec<String> {
    TOPIC_AGENT_PROTECTED_HOOKS
        .iter()
        .map(|hook| (*hook).to_string())
        .collect()
}

/// All visible main-agent hooks for topic agents.
#[must_use]
pub fn topic_agent_all_hooks() -> Vec<String> {
    TOPIC_AGENT_MANAGEABLE_HOOKS
        .iter()
        .chain(TOPIC_AGENT_PROTECTED_HOOKS.iter())
        .map(|hook| (*hook).to_string())
        .collect()
}

/// Parsed agent profile settings used at execution time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedAgentProfile {
    /// Optional prompt instructions contributed by the profile.
    pub prompt_instructions: Option<String>,
    /// Tool access policy for this profile.
    pub tool_policy: ToolAccessPolicy,
    /// Hook access policy for this profile.
    pub hook_policy: HookAccessPolicy,
}

/// Execution profile applied to a live executor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentExecutionProfile {
    agent_id: Option<String>,
    prompt_instructions: Option<String>,
    tool_policy: ToolAccessPolicy,
    hook_policy: HookAccessPolicy,
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
            hook_policy: HookAccessPolicy::default(),
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

    /// Hook policy for this execution.
    #[must_use]
    pub fn hook_policy(&self) -> &HookAccessPolicy {
        &self.hook_policy
    }

    /// Attach hook policy settings to the execution profile.
    #[must_use]
    pub fn with_hook_policy(mut self, hook_policy: HookAccessPolicy) -> Self {
        self.hook_policy = hook_policy;
        self
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
        hook_policy: HookAccessPolicy::new(
            parse_tool_name_set(value, "enabledHooks", "enabled_hooks"),
            parse_tool_name_set(value, "disabledHooks", "disabled_hooks").unwrap_or_default(),
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
        topic_agent_manageable_hooks, topic_agent_protected_hooks, AgentExecutionProfile,
        HookAccessPolicy, ToolAccessPolicy,
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
            "blockedTools": ["delegate_to_sub_agent"],
            "enabledHooks": ["workload_distributor", "search_budget"],
            "disabledHooks": ["delegation_guard"]
        }));

        assert_eq!(parsed.prompt_instructions.as_deref(), Some("you are infra"));
        assert!(parsed.tool_policy.allows("todos_write"));
        assert!(!parsed.tool_policy.allows("delegate_to_sub_agent"));
        assert!(!parsed.tool_policy.allows("unknown_tool"));
        assert!(parsed.hook_policy.allows("workload_distributor"));
        assert!(!parsed.hook_policy.allows("delegation_guard"));
        assert!(!parsed.hook_policy.allows("timeout_report"));
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

    #[test]
    fn additional_disabled_hooks_override_existing_policy() {
        let policy = HookAccessPolicy::new(
            Some(HashSet::from([
                "delegation_guard".to_string(),
                "workload_distributor".to_string(),
            ])),
            HashSet::new(),
        )
        .with_additional_disabled_hooks(["delegation_guard"]);

        assert!(policy.allows("workload_distributor"));
        assert!(!policy.allows("delegation_guard"));
    }

    #[test]
    fn topic_agent_hook_catalogs_expose_manageable_and_protected_sets() {
        let manageable = topic_agent_manageable_hooks();
        let protected = topic_agent_protected_hooks();

        assert!(manageable.iter().any(|hook| hook == "workload_distributor"));
        assert!(manageable.iter().any(|hook| hook == "timeout_report"));
        assert!(protected.iter().any(|hook| hook == "completion_check"));
        assert!(protected.iter().any(|hook| hook == "tool_access_policy"));
        assert!(manageable.iter().all(|hook| !protected.contains(hook)));
    }
}
