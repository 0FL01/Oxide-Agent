use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Default)]
pub enum State {
    #[default]
    Start,
    EditingPrompt,
    /// Agent mode for complex task execution
    AgentMode,
}
