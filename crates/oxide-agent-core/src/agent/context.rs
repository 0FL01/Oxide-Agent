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
    /// Get elapsed time in seconds since task start.
    fn elapsed_secs(&self) -> u64;
    /// Get current delegation depth for this agent run.
    fn delegation_depth(&self) -> usize;
}

/// Ephemeral session used for isolated sub-agent execution.
pub struct EphemeralSession {
    memory: AgentMemory,
    cancellation_token: CancellationToken,
    loaded_skills: HashSet<String>,
    skill_token_count: usize,
    started_at: std::time::Instant,
    delegation_depth: usize,
}

impl EphemeralSession {
    /// Create a new ephemeral session with default token limits.
    #[must_use]
    pub fn new(max_tokens: usize) -> Self {
        Self::with_depth(max_tokens, 1)
    }

    /// Create a new ephemeral session with explicit delegation depth.
    #[must_use]
    pub fn with_depth(max_tokens: usize, delegation_depth: usize) -> Self {
        Self {
            memory: AgentMemory::new(max_tokens),
            cancellation_token: CancellationToken::new(),
            loaded_skills: HashSet::new(),
            skill_token_count: 0,
            started_at: std::time::Instant::now(),
            delegation_depth,
        }
    }

    /// Create a new ephemeral session with a child token linked to the parent.
    ///
    /// When the parent token is cancelled, the child token is also cancelled,
    /// ensuring sub-agents stop when the parent agent is cancelled.
    #[must_use]
    pub fn with_parent_token(max_tokens: usize, parent: &CancellationToken) -> Self {
        Self::with_parent_token_and_depth(max_tokens, parent, 1)
    }

    /// Create a new ephemeral session with a child token and explicit delegation depth.
    #[must_use]
    pub fn with_parent_token_and_depth(
        max_tokens: usize,
        parent: &CancellationToken,
        delegation_depth: usize,
    ) -> Self {
        Self {
            memory: AgentMemory::new(max_tokens),
            cancellation_token: parent.child_token(),
            loaded_skills: HashSet::new(),
            skill_token_count: 0,
            started_at: std::time::Instant::now(),
            delegation_depth,
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

    fn elapsed_secs(&self) -> u64 {
        self.elapsed_secs()
    }

    fn delegation_depth(&self) -> usize {
        self.delegation_depth()
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

    fn elapsed_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    fn delegation_depth(&self) -> usize {
        self.delegation_depth
    }
}

#[cfg(test)]
mod tests {
    use super::EphemeralSession;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn ephemeral_session_default_depth_is_one() {
        let session = EphemeralSession::new(1024);
        assert_eq!(session.delegation_depth, 1);
    }

    #[test]
    fn ephemeral_session_carries_explicit_depth() {
        let parent = CancellationToken::new();
        let session = EphemeralSession::with_parent_token_and_depth(1024, &parent, 2);
        assert_eq!(session.delegation_depth, 2);
    }
}
