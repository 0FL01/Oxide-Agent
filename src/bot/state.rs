use serde::{Deserialize, Serialize};

/// Type of destructive action requiring confirmation
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConfirmationType {
    /// Clear agent memory (history)
    ClearMemory,
    /// Recreate agent container
    RecreateContainer,
}

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
    /// Normal chat mode with management buttons
    ChatMode,
    /// Confirmation for destructive agent actions
    AgentConfirmation(ConfirmationType),
}
