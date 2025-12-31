use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Default)]
pub enum State {
    #[default]
    Start,
    EditingPrompt,
}
