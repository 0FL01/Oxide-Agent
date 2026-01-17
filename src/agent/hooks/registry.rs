//! Hook Registry - manages and executes hooks
//!
//! Provides the `Hook` trait and `HookRegistry` for registering and
//! executing hooks at various points in the agent lifecycle.

use super::types::{HookContext, HookEvent, HookResult};
use tracing::{debug, info};

/// Trait for implementing hooks
pub trait Hook: Send + Sync {
    /// Name of the hook for logging and debugging
    fn name(&self) -> &'static str;

    /// Handle a hook event and return the result
    ///
    /// Hooks should return `HookResult::Continue` if they don't need
    /// to modify the behavior. Any other result will affect execution.
    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult;
}

/// Registry that manages multiple hooks
pub struct HookRegistry {
    hooks: Vec<Box<dyn Hook>>,
}

impl HookRegistry {
    /// Create a new empty hook registry
    #[must_use]
    pub const fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Register a new hook
    pub fn register(&mut self, hook: Box<dyn Hook>) {
        info!(hook = hook.name(), "Registered hook");
        self.hooks.push(hook);
    }

    /// Execute all hooks for an event
    ///
    /// Hooks are executed in registration order. The first non-Continue
    /// result stops the chain and is returned.
    pub fn execute(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        for hook in &self.hooks {
            let result = hook.handle(event, context);

            match &result {
                HookResult::Continue => {
                    debug!(hook = hook.name(), "Hook returned Continue");
                }
                HookResult::InjectContext(ctx) => {
                    debug!(
                        hook = hook.name(),
                        context_len = ctx.len(),
                        "Hook injecting context"
                    );
                    return result;
                }
                HookResult::ForceIteration { reason, .. } => {
                    info!(
                        hook = hook.name(),
                        reason = %reason,
                        "Hook forcing iteration"
                    );
                    return result;
                }
                HookResult::Block { reason } => {
                    info!(
                        hook = hook.name(),
                        reason = %reason,
                        "Hook blocking action"
                    );
                    return result;
                }
            }
        }

        HookResult::Continue
    }

    /// Check if any hooks are registered
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Get the number of registered hooks
    #[must_use]
    pub fn len(&self) -> usize {
        self.hooks.len()
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::providers::TodoList;
    use crate::config::AGENT_CONTINUATION_LIMIT;

    struct TestHook {
        name: &'static str,
        result: HookResult,
    }

    impl Hook for TestHook {
        fn name(&self) -> &'static str {
            self.name
        }

        fn handle(&self, _event: &HookEvent, _context: &HookContext) -> HookResult {
            self.result.clone()
        }
    }

    #[test]
    fn test_empty_registry() {
        let registry = HookRegistry::new();
        let todos = TodoList::new();
        let context = HookContext::new(&todos, 0, 0, AGENT_CONTINUATION_LIMIT);
        let event = HookEvent::AfterAgent {
            response: "test".to_string(),
        };

        let result = registry.execute(&event, &context);
        assert!(matches!(result, HookResult::Continue));
    }

    #[test]
    fn test_hook_chain_stops_on_non_continue() {
        let mut registry = HookRegistry::new();

        registry.register(Box::new(TestHook {
            name: "first",
            result: HookResult::Continue,
        }));

        registry.register(Box::new(TestHook {
            name: "second",
            result: HookResult::ForceIteration {
                reason: "test".to_string(),
                context: None,
            },
        }));

        registry.register(Box::new(TestHook {
            name: "third",
            result: HookResult::Continue,
        }));

        let todos = TodoList::new();
        let context = HookContext::new(&todos, 0, 0, AGENT_CONTINUATION_LIMIT);
        let event = HookEvent::AfterAgent {
            response: "test".to_string(),
        };

        let result = registry.execute(&event, &context);
        assert!(matches!(result, HookResult::ForceIteration { .. }));
    }
}
