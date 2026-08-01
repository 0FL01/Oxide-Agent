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
    /// Agent mode for complex task execution
    #[default]
    AgentMode,
    /// Confirmation for destructive agent actions
    AgentConfirmation(ConfirmationType),
}

#[cfg(test)]
mod tests {
    use super::State;

    #[test]
    fn agent_mode_is_the_default_ingress_state() {
        assert!(matches!(State::default(), State::AgentMode));
    }
}
