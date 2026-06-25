//! Life-mode configuration contract.

use serde::{Deserialize, Serialize};

/// Runtime configuration owned by the life bounded context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeConfig {
    /// Whether life mode is enabled for the embedding binary.
    pub enabled: bool,
    /// Stable context key used for `AgentMemoryScope`.
    pub context_key: String,
    /// Stable flow id used for `AgentMemoryScope`.
    pub flow_id: String,
    /// Optional worker identity stored in claimed queue rows.
    pub worker_id: Option<String>,
}

impl Default for LifeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            context_key: "life".to_owned(),
            flow_id: "main".to_owned(),
            worker_id: None,
        }
    }
}
