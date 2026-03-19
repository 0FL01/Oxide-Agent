use serde::{Deserialize, Serialize};

/// Agent flow metadata persisted per topic-scoped flow.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct AgentFlowRecord {
    /// Record schema version for forward-compatible evolution.
    pub schema_version: u32,
    /// User owning this flow.
    pub user_id: i64,
    /// Transport context key the flow belongs to.
    pub context_key: String,
    /// Stable flow identifier.
    pub flow_id: String,
    /// Creation timestamp (unix seconds).
    pub created_at: i64,
    /// Last update timestamp (unix seconds).
    pub updated_at: i64,
}
