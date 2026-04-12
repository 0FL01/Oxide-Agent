//! Tool access policy hook.

use super::registry::Hook;
use super::types::{HookContext, HookEvent, HookResult};
use crate::agent::profile::ToolAccessPolicy;
use std::sync::{Arc, RwLock};

/// Hook that blocks tool calls forbidden by the active execution profile.
pub struct ToolAccessPolicyHook {
    policy: Arc<RwLock<ToolAccessPolicy>>,
}

impl ToolAccessPolicyHook {
    /// Create a new hook backed by shared policy state.
    #[must_use]
    pub fn new(policy: Arc<RwLock<ToolAccessPolicy>>) -> Self {
        Self { policy }
    }
}

impl Hook for ToolAccessPolicyHook {
    fn name(&self) -> &'static str {
        "tool_access_policy"
    }

    fn handle(&self, event: &HookEvent, _context: &HookContext) -> HookResult {
        let HookEvent::BeforeTool { tool_name, .. } = event else {
            return HookResult::Continue;
        };

        let policy = match self.policy.read() {
            Ok(policy) => policy,
            Err(_) => {
                return HookResult::Block {
                    reason: "Tool policy is unavailable for this session".to_string(),
                };
            }
        };

        if policy.allows(tool_name) {
            return HookResult::Continue;
        }

        let reason = if policy.blocked_tools().contains(tool_name) {
            format!("Tool '{tool_name}' is blocked by the current agent profile")
        } else {
            format!("Tool '{tool_name}' is not allowed by the current agent profile")
        };

        HookResult::Block { reason }
    }
}

#[cfg(test)]
mod tests {
    use super::ToolAccessPolicyHook;
    use crate::agent::hooks::{Hook, HookContext, HookEvent, HookResult};
    use crate::agent::profile::ToolAccessPolicy;
    use crate::agent::providers::TodoList;
    use crate::agent::AgentMemory;
    use std::collections::HashSet;
    use std::sync::{Arc, RwLock};

    fn hook_context<'a>(memory: &'a AgentMemory, todos: &'a TodoList) -> HookContext<'a> {
        HookContext::new(todos, memory, 0, 0, 4)
    }

    #[test]
    fn blocks_non_allowlisted_tool() {
        let policy =
            ToolAccessPolicy::new(Some(HashSet::from(["exec".to_string()])), HashSet::new());
        let hook = ToolAccessPolicyHook::new(Arc::new(RwLock::new(policy)));
        let memory = AgentMemory::new(1024);
        let todos = TodoList::new();

        let result = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "sudo-exec".to_string(),
                arguments: "{}".to_string(),
            },
            &hook_context(&memory, &todos),
        );

        assert!(matches!(result, HookResult::Block { .. }));
    }

    #[test]
    fn blocks_explicitly_blocked_tool() {
        let policy =
            ToolAccessPolicy::new(None, HashSet::from(["delegate_to_sub_agent".to_string()]));
        let hook = ToolAccessPolicyHook::new(Arc::new(RwLock::new(policy)));
        let memory = AgentMemory::new(1024);
        let todos = TodoList::new();

        let result = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "delegate_to_sub_agent".to_string(),
                arguments: "{}".to_string(),
            },
            &hook_context(&memory, &todos),
        );

        assert!(matches!(result, HookResult::Block { .. }));
    }

    #[test]
    fn allows_tool_when_policy_permits_it() {
        let policy = ToolAccessPolicy::new(None, HashSet::new());
        let hook = ToolAccessPolicyHook::new(Arc::new(RwLock::new(policy)));
        let memory = AgentMemory::new(1024);
        let todos = TodoList::new();

        let result = hook.handle(
            &HookEvent::BeforeTool {
                tool_name: "execute_command".to_string(),
                arguments: "{}".to_string(),
            },
            &hook_context(&memory, &todos),
        );

        assert!(matches!(result, HookResult::Continue));
    }
}
