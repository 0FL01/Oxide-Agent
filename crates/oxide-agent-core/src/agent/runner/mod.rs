//! Agent runner module.
//!
//! Encapsulates the core agent execution loop, independent of session/UI concerns.

mod execution;
mod hooks;
mod llm_calls;
mod loop_detection;
mod model_routes;
mod response_dispatch;
mod responses;
mod runtime_compaction;
#[cfg(test)]
mod test_support;
mod token_snapshots;
mod tools;
mod types;

use crate::agent::hooks::HookRegistry;
use crate::agent::loop_detection::{LoopDetectionConfig, LoopDetectionService};
use crate::agent::memory::AgentMessage;
use crate::llm::{LlmClient, Message};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::time::{Duration, timeout};

pub(crate) use types::AgentRunnerContextBase;
pub(crate) use types::TimedRunResult;
pub use types::{AgentRunResult, AgentRunnerConfig, AgentRunnerContext};

/// Agent runner that executes the core loop.
pub struct AgentRunner {
    llm_client: Arc<LlmClient>,
    hook_registry: HookRegistry,
    loop_detector: Arc<Mutex<LoopDetectionService>>,
    loop_detection_disabled_next_run: bool,
    route_failover_state: RouteFailoverState,
}

#[derive(Debug, Default)]
struct RouteFailoverState {
    route_quarantine: HashMap<String, Instant>,
    fallback_cursor: usize,
}

impl AgentRunner {
    /// Create a new agent runner.
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>) -> Self {
        let loop_config = Arc::new(LoopDetectionConfig::from_env());
        // Note: Using .clone() here for trait object coercion to Arc<dyn LoopScoutClient>
        #[allow(clippy::clone_on_ref_ptr)]
        let loop_detector = Arc::new(Mutex::new(LoopDetectionService::new(
            llm_client.clone(),
            loop_config,
        )));

        Self {
            llm_client,
            hook_registry: HookRegistry::new(),
            loop_detector,
            loop_detection_disabled_next_run: false,
            route_failover_state: RouteFailoverState::default(),
        }
    }

    /// Register a new hook.
    pub fn register_hook(&mut self, hook: Box<dyn crate::agent::hooks::Hook>) {
        self.hook_registry.register(hook);
    }

    #[cfg(test)]
    pub(crate) fn has_registered_hook(&self, name: &str) -> bool {
        self.hook_registry.contains_hook(name)
    }

    /// Get access to the internal LLM client.
    #[must_use]
    pub fn llm_client(&self) -> Arc<LlmClient> {
        Arc::clone(&self.llm_client)
    }

    /// Disable loop detection for the next execution attempt.
    pub fn disable_loop_detection_next_run(&mut self) {
        self.loop_detection_disabled_next_run = true;
    }

    /// Reset internal loop detector state.
    pub fn reset(&mut self) {
        self.loop_detection_disabled_next_run = false;
        self.route_failover_state = RouteFailoverState::default();
        if let Ok(mut detector) = self.loop_detector.try_lock() {
            detector.reset(String::new());
        }
    }

    /// Convert `AgentMessage` history to LLM Message format.
    ///
    /// Delegates to `CompactionRenderer::render` with empty compaction state,
    /// producing identity-equivalent output. This is the legacy entry point;
    /// production code should use `AgentMemory::rendered_messages()` instead.
    #[must_use]
    pub fn convert_memory_to_messages(messages: &[AgentMessage]) -> Vec<Message> {
        crate::agent::compaction::CompactionRenderer::render(
            messages,
            &crate::agent::compaction::CompactionState::default(),
            &crate::agent::compaction::RenderPolicy::default(),
        )
    }
}

pub(crate) async fn run_with_timeout(
    runner: &mut AgentRunner,
    ctx: &mut AgentRunnerContext<'_>,
    timeout_duration: Duration,
) -> TimedRunResult {
    match timeout(timeout_duration, runner.run(ctx)).await {
        Ok(Ok(result)) => result.into(),
        Ok(Err(error)) => TimedRunResult::Failed(error),
        Err(_) => TimedRunResult::TimedOut,
    }
}
