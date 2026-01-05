use serde::{Deserialize, Serialize};

/// Represents the current state of the user dialogue
#[derive(Clone, Serialize, Deserialize, Default)]
pub enum State {
    /// Initial state, normal chat
    #[default]
    Start,
    /// User is editing the system prompt
    EditingPrompt,
    /// Agent mode for complex task execution
    AgentMode,
    /// Confirmation for wiping agent memory/container
    AgentWipeConfirmation,
}
