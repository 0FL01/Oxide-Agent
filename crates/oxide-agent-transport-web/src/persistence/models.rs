use chrono::{DateTime, Utc};
use oxide_agent_web_contracts::{PersistedTaskEvent, UserRole, WebSessionRecord, WebTaskRecord};
use serde::{Deserialize, Serialize};

use super::{WebUiStoreError, WebUiStoreResult};

pub const WEB_AUTH_SCHEMA_VERSION: u32 = 1;
pub const WEB_SESSION_SCHEMA_VERSION: u32 = 1;
pub const WEB_TASK_SCHEMA_VERSION: u32 = 1;
pub const WEB_EVENT_SCHEMA_VERSION: u32 = 1;
pub const WEB_EVENT_CHUNK_SCHEMA_VERSION: u32 = 1;

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

pub(crate) trait ValidateWebRecord {
    fn validate_web_record(&self) -> WebUiStoreResult<()>;
}

impl ValidateWebRecord for WebUserRecord {
    fn validate_web_record(&self) -> WebUiStoreResult<()> {
        validate_schema_version("web user", self.schema_version, WEB_AUTH_SCHEMA_VERSION)
    }
}

impl ValidateWebRecord for LoginIndexRecord {
    fn validate_web_record(&self) -> WebUiStoreResult<()> {
        validate_schema_version(
            "web login index",
            self.schema_version,
            WEB_AUTH_SCHEMA_VERSION,
        )
    }
}

impl ValidateWebRecord for WebAuthSessionRecord {
    fn validate_web_record(&self) -> WebUiStoreResult<()> {
        validate_schema_version(
            "web auth session",
            self.schema_version,
            WEB_AUTH_SCHEMA_VERSION,
        )
    }
}

impl ValidateWebRecord for WebSessionRecord {
    fn validate_web_record(&self) -> WebUiStoreResult<()> {
        validate_schema_version(
            "web session",
            self.schema_version,
            WEB_SESSION_SCHEMA_VERSION,
        )
    }
}

impl ValidateWebRecord for WebTaskRecord {
    fn validate_web_record(&self) -> WebUiStoreResult<()> {
        validate_schema_version("web task", self.schema_version, WEB_TASK_SCHEMA_VERSION)
    }
}

impl ValidateWebRecord for PersistedTaskEvent {
    fn validate_web_record(&self) -> WebUiStoreResult<()> {
        validate_schema_version(
            "web task event",
            self.schema_version,
            WEB_EVENT_SCHEMA_VERSION,
        )
    }
}

impl ValidateWebRecord for WebTaskEventChunkRecord {
    fn validate_web_record(&self) -> WebUiStoreResult<()> {
        validate_schema_version(
            "web task event chunk",
            self.schema_version,
            WEB_EVENT_CHUNK_SCHEMA_VERSION,
        )?;
        for event in &self.events {
            event.validate_web_record()?;
        }
        Ok(())
    }
}

fn validate_schema_version(record_type: &str, actual: u32, expected: u32) -> WebUiStoreResult<()> {
    if actual == expected {
        return Ok(());
    }
    Err(WebUiStoreError::Unavailable(format!(
        "unsupported {record_type} schema_version {actual}; expected {expected}"
    )))
}
