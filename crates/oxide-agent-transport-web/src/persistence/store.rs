use std::fmt;

use async_trait::async_trait;
use oxide_agent_web_contracts::{
    PersistedTaskEvent, TaskEventsResponse, WebSessionRecord, WebTaskRecord,
};

use super::{
    LoginIndexRecord, WebAuthSessionRecord, WebTaskFileBlob, WebTaskFileRecord, WebUserRecord,
};

pub type WebUiStoreResult<T> = Result<T, WebUiStoreError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebUiStoreError {
    Conflict(String),
    Unavailable(String),
}

impl fmt::Display for WebUiStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(message) => write!(f, "web UI store conflict: {message}"),
            Self::Unavailable(message) => write!(f, "web UI store unavailable: {message}"),
        }
    }
}

impl std::error::Error for WebUiStoreError {}

#[async_trait]
pub trait WebUiStore: Send + Sync {
    async fn users_count(&self) -> WebUiStoreResult<u64>;

    async fn save_user(&self, record: WebUserRecord) -> WebUiStoreResult<()>;

    async fn load_user(&self, user_id: i64) -> WebUiStoreResult<Option<WebUserRecord>>;

    async fn load_login_index(
        &self,
        normalized_login: &str,
    ) -> WebUiStoreResult<Option<LoginIndexRecord>>;

    async fn save_auth_session(&self, record: WebAuthSessionRecord) -> WebUiStoreResult<()>;

    async fn load_auth_session(
        &self,
        session_token_hash: &str,
    ) -> WebUiStoreResult<Option<WebAuthSessionRecord>>;

    async fn revoke_auth_session(
        &self,
        session_token_hash: &str,
        revoked_at: chrono::DateTime<chrono::Utc>,
    ) -> WebUiStoreResult<bool>;

    async fn revoke_auth_sessions_for_user_except(
        &self,
        user_id: i64,
        keep_session_token_hash: &str,
        revoked_at: chrono::DateTime<chrono::Utc>,
    ) -> WebUiStoreResult<u64>;

    async fn save_session(&self, record: WebSessionRecord) -> WebUiStoreResult<()>;

    async fn load_session(
        &self,
        user_id: i64,
        session_id: &str,
    ) -> WebUiStoreResult<Option<WebSessionRecord>>;

    async fn list_sessions(&self, user_id: i64) -> WebUiStoreResult<Vec<WebSessionRecord>>;

    async fn list_due_auto_title_sessions(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        limit: usize,
    ) -> WebUiStoreResult<Vec<WebSessionRecord>>;

    async fn delete_session(&self, user_id: i64, session_id: &str) -> WebUiStoreResult<bool>;

    async fn save_task(&self, record: WebTaskRecord) -> WebUiStoreResult<()>;

    async fn load_task(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
    ) -> WebUiStoreResult<Option<WebTaskRecord>>;

    async fn list_tasks(
        &self,
        user_id: i64,
        session_id: &str,
    ) -> WebUiStoreResult<Vec<WebTaskRecord>>;

    async fn append_task_events(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        events: Vec<PersistedTaskEvent>,
    ) -> WebUiStoreResult<()>;

    async fn list_task_events(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> WebUiStoreResult<TaskEventsResponse>;

    async fn save_task_file(
        &self,
        record: WebTaskFileRecord,
        content: Vec<u8>,
    ) -> WebUiStoreResult<()>;

    async fn load_task_file(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        file_id: &str,
    ) -> WebUiStoreResult<Option<WebTaskFileBlob>>;

    async fn mark_unfinished_tasks_interrupted(
        &self,
        message: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> WebUiStoreResult<Vec<WebTaskRecord>>;
}
