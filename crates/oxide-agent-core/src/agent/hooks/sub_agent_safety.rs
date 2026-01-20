//! Sub-agent safety hook.
//!
//! Enforces token/iteration limits and blocks recursive delegation.

use super::registry::Hook;
use super::types::{HookContext, HookEvent, HookResult};
use std::collections::HashSet;

/// Configuration for sub-agent safety rules.
pub struct SubAgentSafetyConfig {
    /// Maximum iterations allowed for the sub-agent.
    pub max_iterations: usize,
    /// Maximum tokens allowed in memory.
    pub max_tokens: usize,
    /// Tool names blocked for sub-agents.
    pub blocked_tools: HashSet<String>,
}

/// Hook that enforces sub-agent safety limits.
pub struct SubAgentSafetyHook {
    config: SubAgentSafetyConfig,
}

impl SubAgentSafetyHook {
    /// Create a new safety hook with the provided configuration.
    #[must_use]
    pub fn new(config: SubAgentSafetyConfig) -> Self {
        Self { config }
    }
}

impl Hook for SubAgentSafetyHook {
    fn name(&self) -> &'static str {
        "sub_agent_safety"
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        match event {
            HookEvent::BeforeIteration { iteration } => {
                if *iteration >= self.config.max_iterations {
                    return HookResult::Block {
                        reason: format!(
                            "Sub-agent iteration limit reached ({})",
                            self.config.max_iterations
                        ),
                    };
                }

                if context.token_count >= self.config.max_tokens {
                    return HookResult::Block {
                        reason: format!(
                            "Sub-agent token limit reached ({})",
                            self.config.max_tokens
                        ),
                    };
                }
            }
            HookEvent::BeforeTool { tool_name, .. } => {
                if self.config.blocked_tools.contains(tool_name) {
                    return HookResult::Block {
                        reason: format!("Tool '{tool_name}' is blocked for sub-agents"),
                    };
                }
            }
            _ => {}
        }

        HookResult::Continue
    }
}
