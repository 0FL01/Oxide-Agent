use chrono::{DateTime, Utc};
use oxide_agent_core::agent::progress::FileDeliveryKind;
use oxide_agent_web_contracts::{
    AgentEffort, ModelSelection, PersistedTaskEvent, UserRole, WebSessionRecord, WebTaskRecord,
};
use serde::{Deserialize, Serialize};

use super::{WebUiStoreError, WebUiStoreResult};

pub const WEB_AUTH_SCHEMA_VERSION: u32 = 1;
pub const WEB_SESSION_SCHEMA_VERSION: u32 = 1;
pub const WEB_TASK_SCHEMA_VERSION: u32 = 1;
pub const WEB_EVENT_SCHEMA_VERSION: u32 = 1;
pub const WEB_EVENT_CHUNK_SCHEMA_VERSION: u32 = 1;
pub const WEB_TASK_FILE_SCHEMA_VERSION: u32 = 1;

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
    #[serde(default)]
    pub default_model_selection: Option<ModelSelection>,
    #[serde(default)]
    pub default_agent_profile_id: Option<String>,
    #[serde(default)]
    pub default_effort: Option<AgentEffort>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WebTaskFileRecord {
    pub schema_version: u32,
    pub user_id: i64,
    pub session_id: String,
    pub task_id: String,
    pub file_id: String,
    pub file_name: String,
    pub content_type: String,
    pub size_bytes: u64,
    pub delivery_kind: FileDeliveryKind,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebTaskFileBlob {
    pub record: WebTaskFileRecord,
    pub content: Vec<u8>,
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

impl ValidateWebRecord for WebTaskFileRecord {
    fn validate_web_record(&self) -> WebUiStoreResult<()> {
        validate_schema_version(
            "web task file",
            self.schema_version,
            WEB_TASK_FILE_SCHEMA_VERSION,
        )
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
