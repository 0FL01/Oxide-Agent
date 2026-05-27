use chrono::{DateTime, Utc};
use oxide_agent_web_contracts::{PersistedTaskEvent, UserRole};
use serde::{Deserialize, Serialize};

pub const WEB_AUTH_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WebUserRecord {
    pub schema_version: u32,
    pub user_id: i64,
    pub login: String,
    pub normalized_login: String,
    pub password_hash: String,
    pub role: UserRole,
    pub status: WebUserStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebUserStatus {
    Active,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LoginIndexRecord {
    pub schema_version: u32,
    pub normalized_login: String,
    pub user_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WebAuthSessionRecord {
    pub schema_version: u32,
    pub session_token_hash: String,
    pub user_id: i64,
    pub csrf_token: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WebTaskEventChunkRecord {
    pub schema_version: u32,
    pub user_id: i64,
    pub session_id: String,
    pub task_id: String,
    pub chunk_no: u64,
    pub events: Vec<PersistedTaskEvent>,
}
