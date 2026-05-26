use serde::{Deserialize, Serialize};

/// Type of destructive action requiring confirmation
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConfirmationType {
    /// Clear agent memory (history)
    ClearMemory,
    /// Compact agent context
    CompactContext,
    /// Recreate agent container
    RecreateContainer,
}

/// Represents the current state of the user dialogue
#[derive(Clone, Serialize, Deserialize, Default)]
pub enum State {
    /// Initial state before agent access/context is resolved
    #[default]
    Start,
    /// Agent mode for complex task execution
    AgentMode,
    /// Confirmation for destructive agent actions
    AgentConfirmation(ConfirmationType),
}
