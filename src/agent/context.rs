//! Agent context abstractions for core runner logic.
//!
//! Provides a lightweight context trait for agent execution that
//! decouples the runner from session-specific infrastructure.

use super::memory::AgentMemory;
use super::session::AgentSession;
use crate::config::AGENT_MAX_TOKENS;
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;

/// Minimal context interface needed by the agent runner.
pub trait AgentContext: Send {
    /// Access immutable agent memory.
    fn memory(&self) -> &AgentMemory;
    /// Access mutable agent memory.
    fn memory_mut(&mut self) -> &mut AgentMemory;
    /// Access the cancellation token for this run.
    fn cancellation_token(&self) -> &CancellationToken;
    /// Check if a skill has already been loaded into context.
    fn is_skill_loaded(&self, name: &str) -> bool;
    /// Register a skill as loaded and update token accounting.
    fn register_loaded_skill(&mut self, name: &str, token_count: usize) -> bool;
}

/// Ephemeral session used for isolated sub-agent execution.
pub struct EphemeralSession {
    memory: AgentMemory,
    cancellation_token: CancellationToken,
    loaded_skills: HashSet<String>,
    skill_token_count: usize,
}

impl EphemeralSession {
    /// Create a new ephemeral session with default token limits.
    #[must_use]
    pub fn new(max_tokens: usize) -> Self {
        Self {
            memory: AgentMemory::new(max_tokens),
            cancellation_token: CancellationToken::new(),
            loaded_skills: HashSet::new(),
            skill_token_count: 0,
        }
    }

    /// Create a new ephemeral session with a child token linked to the parent.
    ///
    /// When the parent token is cancelled, the child token is also cancelled,
    /// ensuring sub-agents stop when the parent agent is cancelled.
    #[must_use]
    pub fn with_parent_token(max_tokens: usize, parent: &CancellationToken) -> Self {
        Self {
            memory: AgentMemory::new(max_tokens),
            cancellation_token: parent.child_token(),
            loaded_skills: HashSet::new(),
            skill_token_count: 0,
        }
    }

    /// Convenience constructor with default agent limits.
    #[must_use]
    pub fn with_default_limits() -> Self {
        Self::new(AGENT_MAX_TOKENS)
    }

    /// Access the internal cancellation token mutably if needed.
    pub fn cancellation_token_mut(&mut self) -> &mut CancellationToken {
        &mut self.cancellation_token
    }

    /// Get total tokens used by loaded skills.
    #[must_use]
    pub const fn skill_token_count(&self) -> usize {
        self.skill_token_count
    }
}

impl AgentContext for AgentSession {
    fn memory(&self) -> &AgentMemory {
        &self.memory
    }

    fn memory_mut(&mut self) -> &mut AgentMemory {
        &mut self.memory
    }

    fn cancellation_token(&self) -> &CancellationToken {
        &self.cancellation_token
    }

    fn is_skill_loaded(&self, name: &str) -> bool {
        self.is_skill_loaded(name)
    }

    fn register_loaded_skill(&mut self, name: &str, token_count: usize) -> bool {
        self.register_loaded_skill(name, token_count)
    }
}

impl AgentContext for EphemeralSession {
    fn memory(&self) -> &AgentMemory {
        &self.memory
    }

    fn memory_mut(&mut self) -> &mut AgentMemory {
        &mut self.memory
    }

    fn cancellation_token(&self) -> &CancellationToken {
        &self.cancellation_token
    }

    fn is_skill_loaded(&self, name: &str) -> bool {
        self.loaded_skills.contains(name)
    }

    fn register_loaded_skill(&mut self, name: &str, token_count: usize) -> bool {
        if self.loaded_skills.insert(name.to_string()) {
            self.skill_token_count = self.skill_token_count.saturating_add(token_count);
            return true;
        }

        false
    }
}
