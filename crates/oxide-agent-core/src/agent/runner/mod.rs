//! Agent runner module.
//!
//! Encapsulates the core agent execution loop, independent of session/UI concerns.

mod execution;
mod hooks;
mod loop_detection;
mod responses;
mod tools;
mod types;

use crate::agent::hooks::HookRegistry;
use crate::agent::loop_detection::{LoopDetectionConfig, LoopDetectionService};
use crate::agent::memory::AgentMessage;
use crate::agent::narrator::Narrator;
use crate::llm::{LlmClient, Message};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

pub use types::{AgentRunResult, AgentRunnerConfig, AgentRunnerContext};

/// Agent runner that executes the core loop.
pub struct AgentRunner {
    llm_client: Arc<LlmClient>,
    hook_registry: HookRegistry,
    loop_detector: Arc<Mutex<LoopDetectionService>>,
    loop_detection_disabled_next_run: bool,
    narrator: Arc<Narrator>,
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
        let loop_detector = Arc::new(Mutex::new(LoopDetectionService::new(
            llm_client.clone(),
            loop_config,
        )));

        let narrator = Arc::new(Narrator::new(llm_client.clone()));

        Self {
            llm_client,
            hook_registry: HookRegistry::new(),
            loop_detector,
            loop_detection_disabled_next_run: false,
            narrator,
            route_failover_state: RouteFailoverState::default(),
        }
    }

    /// Register a new hook.
    pub fn register_hook(&mut self, hook: Box<dyn crate::agent::hooks::Hook>) {
        self.hook_registry.register(hook);
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
    #[must_use]
    pub fn convert_memory_to_messages(messages: &[AgentMessage]) -> Vec<Message> {
        messages
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    crate::agent::memory::MessageRole::User => "user",
                    crate::agent::memory::MessageRole::Assistant => "assistant",
                    crate::agent::memory::MessageRole::System => "system",
                    crate::agent::memory::MessageRole::Tool => "tool",
                };
                Message {
                    role: role.to_string(),
                    content: msg.content.clone(),
                    tool_call_id: msg.tool_call_id.clone(),
                    tool_call_correlation: msg.resolved_tool_call_correlation(),
                    name: msg.tool_name.clone(),
                    tool_calls: msg.tool_calls.clone(),
                    tool_call_correlations: msg.resolved_tool_call_correlations(),
                }
            })
            .collect()
    }
}
