use super::AgentExecutor;
use crate::agent::hooks::{Hook, HookContext, HookEvent, HookResult};
use crate::agent::profile::HookAccessPolicy;
use crate::agent::runner::AgentRunner;
use std::sync::{Arc, RwLock};

pub(super) struct PolicyControlledHook {
    name: &'static str,
    inner: Box<dyn Hook>,
    policy: Arc<RwLock<HookAccessPolicy>>,
}

impl PolicyControlledHook {
    pub(super) fn new(
        name: &'static str,
        inner: Box<dyn Hook>,
        policy: Arc<RwLock<HookAccessPolicy>>,
    ) -> Self {
        Self {
            name,
            inner,
            policy,
        }
    }
}

impl Hook for PolicyControlledHook {
    fn name(&self) -> &'static str {
        self.name
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        if let Ok(policy) = self.policy.read() {
            if !policy.allows(self.name) {
                return HookResult::Continue;
            }
        }

        self.inner.handle(event, context)
    }
}

impl AgentExecutor {
    pub(super) fn register_policy_controlled_hook<H>(
        runner: &mut AgentRunner,
        hook: H,
        policy: Arc<RwLock<HookAccessPolicy>>,
    ) where
        H: Hook + 'static,
    {
        let name = hook.name();
        runner.register_hook(Box::new(PolicyControlledHook::new(
            name,
            Box::new(hook),
            policy,
        )));
    }
}
