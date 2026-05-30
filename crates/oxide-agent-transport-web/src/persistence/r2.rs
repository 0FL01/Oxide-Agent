use async_trait::async_trait;
use chrono::{DateTime, Utc};
use oxide_agent_web_contracts::{
    PersistedTaskEvent, TaskEventsResponse, TaskStatus, WebSessionRecord, WebTaskRecord,
};
use serde::{de::DeserializeOwned, Serialize};

use super::{
    LoginIndexRecord, ValidateWebRecord, WebAuthSessionRecord, WebTaskEventChunkRecord,
    WebTaskFileBlob, WebTaskFileRecord, WebUiStore, WebUiStoreError, WebUiStoreResult,
    WebUserRecord, WEB_AUTH_SCHEMA_VERSION, WEB_EVENT_CHUNK_SCHEMA_VERSION,
};

const TASK_EVENT_CHUNK_SIZE: u64 = 100;
const WEB_USERS_PREFIX: &str = "web/auth/v1/users/";
const WEB_LOGIN_INDEX_PREFIX: &str = "web/auth/v1/login_index/";
const WEB_BROWSER_SESSIONS_PREFIX: &str = "web/auth/v1/browser_sessions/";
const ALL_USERS_PREFIX: &str = "users/";

struct ObjectStoreWebUiStore<S> {
    object_store: S,
}

impl<S> ObjectStoreWebUiStore<S> {
    #[must_use]
    pub fn new(object_store: S) -> Self {
        Self { object_store }
    }
}

#[cfg(feature = "storage-s3-r2")]
pub struct R2WebUiStore {
    inner: ObjectStoreWebUiStore<R2WebObjectStore>,
}

#[cfg(feature = "storage-s3-r2")]
struct R2WebObjectStore {
    storage: std::sync::Arc<oxide_agent_core::storage::R2Storage>,
}

#[cfg(feature = "storage-s3-r2")]
impl R2WebObjectStore {
    #[must_use]
    pub fn new(storage: std::sync::Arc<oxide_agent_core::storage::R2Storage>) -> Self {
        Self { storage }
    }
}

#[cfg(feature = "storage-s3-r2")]
impl R2WebUiStore {
    #[must_use]
    pub fn new(storage: std::sync::Arc<oxide_agent_core::storage::R2Storage>) -> Self {
        Self {
            inner: ObjectStoreWebUiStore::new(R2WebObjectStore::new(storage)),
        }
    }
}

#[cfg(feature = "storage-s3-r2")]
#[async_trait]
impl WebUiStore for R2WebUiStore {
    async fn users_count(&self) -> WebUiStoreResult<u64> {
        self.inner.users_count().await
    }

    async fn save_user(&self, record: WebUserRecord) -> WebUiStoreResult<()> {
        self.inner.save_user(record).await
    }

    async fn load_user(&self, user_id: i64) -> WebUiStoreResult<Option<WebUserRecord>> {
        self.inner.load_user(user_id).await
    }

    async fn load_login_index(
        &self,
        normalized_login: &str,
    ) -> WebUiStoreResult<Option<LoginIndexRecord>> {
        self.inner.load_login_index(normalized_login).await
    }

    async fn save_auth_session(&self, record: WebAuthSessionRecord) -> WebUiStoreResult<()> {
        self.inner.save_auth_session(record).await
    }

    async fn load_auth_session(
        &self,
        session_token_hash: &str,
    ) -> WebUiStoreResult<Option<WebAuthSessionRecord>> {
        self.inner.load_auth_session(session_token_hash).await
    }

    async fn revoke_auth_session(
        &self,
        session_token_hash: &str,
        revoked_at: DateTime<Utc>,
    ) -> WebUiStoreResult<bool> {
        self.inner
            .revoke_auth_session(session_token_hash, revoked_at)
            .await
    }

    async fn revoke_auth_sessions_for_user_except(
        &self,
        user_id: i64,
        keep_session_token_hash: &str,
        revoked_at: DateTime<Utc>,
    ) -> WebUiStoreResult<u64> {
        self.inner
            .revoke_auth_sessions_for_user_except(user_id, keep_session_token_hash, revoked_at)
            .await
    }

    async fn save_session(&self, record: WebSessionRecord) -> WebUiStoreResult<()> {
        self.inner.save_session(record).await
    }

    async fn load_session(
        &self,
        user_id: i64,
        session_id: &str,
    ) -> WebUiStoreResult<Option<WebSessionRecord>> {
        self.inner.load_session(user_id, session_id).await
    }

    async fn list_sessions(&self, user_id: i64) -> WebUiStoreResult<Vec<WebSessionRecord>> {
        self.inner.list_sessions(user_id).await
    }

    async fn delete_session(&self, user_id: i64, session_id: &str) -> WebUiStoreResult<bool> {
        self.inner.delete_session(user_id, session_id).await
    }

    async fn save_task(&self, record: WebTaskRecord) -> WebUiStoreResult<()> {
        self.inner.save_task(record).await
    }

    async fn load_task(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
    ) -> WebUiStoreResult<Option<WebTaskRecord>> {
        self.inner.load_task(user_id, session_id, task_id).await
    }

    async fn list_tasks(
        &self,
        user_id: i64,
        session_id: &str,
    ) -> WebUiStoreResult<Vec<WebTaskRecord>> {
        self.inner.list_tasks(user_id, session_id).await
    }

    async fn append_task_events(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        events: Vec<PersistedTaskEvent>,
    ) -> WebUiStoreResult<()> {
        self.inner
            .append_task_events(user_id, session_id, task_id, events)
            .await
    }

    async fn list_task_events(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> WebUiStoreResult<TaskEventsResponse> {
        self.inner
            .list_task_events(user_id, session_id, task_id, after_seq, limit)
            .await
    }

    async fn save_task_file(
        &self,
        record: WebTaskFileRecord,
        content: Vec<u8>,
    ) -> WebUiStoreResult<()> {
        self.inner.save_task_file(record, content).await
    }

    async fn load_task_file(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        file_id: &str,
    ) -> WebUiStoreResult<Option<WebTaskFileBlob>> {
        self.inner
            .load_task_file(user_id, session_id, task_id, file_id)
            .await
    }

    async fn mark_unfinished_tasks_interrupted(
        &self,
        message: &str,
        now: DateTime<Utc>,
    ) -> WebUiStoreResult<Vec<WebTaskRecord>> {
        self.inner
            .mark_unfinished_tasks_interrupted(message, now)
            .await
    }
}

#[async_trait]
trait WebObjectStore: Send + Sync {
    async fn save_json<T>(&self, key: &str, data: &T) -> WebUiStoreResult<()>
    where
        T: Serialize + Sync + Send;

    async fn load_json<T>(&self, key: &str) -> WebUiStoreResult<Option<T>>
    where
        T: DeserializeOwned + Send;

    async fn save_bytes(
        &self,
        key: &str,
        content: &[u8],
        content_type: &str,
    ) -> WebUiStoreResult<()>;

    async fn load_bytes(&self, key: &str) -> WebUiStoreResult<Option<Vec<u8>>>;

    async fn delete_object(&self, key: &str) -> WebUiStoreResult<()>;

    async fn delete_prefix(&self, prefix: &str) -> WebUiStoreResult<()>;

    async fn list_json_under_prefix<T>(&self, prefix: &str) -> WebUiStoreResult<Vec<T>>
    where
        T: DeserializeOwned + Send;

    async fn list_keys_under_prefix(&self, prefix: &str) -> WebUiStoreResult<Vec<String>>;
}

#[cfg(feature = "storage-s3-r2")]
#[async_trait]
impl WebObjectStore for R2WebObjectStore {
    async fn save_json<T>(&self, key: &str, data: &T) -> WebUiStoreResult<()>
    where
        T: Serialize + Sync + Send,
    {
        self.storage.save_json(key, data).await.map_err(r2_error)
    }

    async fn load_json<T>(&self, key: &str) -> WebUiStoreResult<Option<T>>
    where
        T: DeserializeOwned + Send,
    {
        self.storage.load_json(key).await.map_err(r2_error)
    }

    async fn save_bytes(
        &self,
        key: &str,
        content: &[u8],
        content_type: &str,
    ) -> WebUiStoreResult<()> {
        self.storage
            .save_bytes(key, content, content_type)
            .await
            .map_err(r2_error)
    }

    async fn load_bytes(&self, key: &str) -> WebUiStoreResult<Option<Vec<u8>>> {
        self.storage.load_bytes(key).await.map_err(r2_error)
    }

    async fn delete_object(&self, key: &str) -> WebUiStoreResult<()> {
        self.storage.delete_object(key).await.map_err(r2_error)
    }

    async fn delete_prefix(&self, prefix: &str) -> WebUiStoreResult<()> {
        self.storage.delete_prefix(prefix).await.map_err(r2_error)
    }

    async fn list_json_under_prefix<T>(&self, prefix: &str) -> WebUiStoreResult<Vec<T>>
    where
        T: DeserializeOwned + Send,
    {
        self.storage
            .list_json_under_prefix(prefix)
            .await
            .map_err(r2_error)
    }

    async fn list_keys_under_prefix(&self, prefix: &str) -> WebUiStoreResult<Vec<String>> {
        self.storage
            .list_keys_under_prefix(prefix)
            .await
            .map_err(r2_error)
    }
}

#[cfg(feature = "storage-s3-r2")]
fn r2_error(error: oxide_agent_core::storage::StorageError) -> WebUiStoreError {
    WebUiStoreError::Unavailable(error.to_string())
}

#[async_trait]
impl<S> WebUiStore for ObjectStoreWebUiStore<S>
where
    S: WebObjectStore,
{
    async fn users_count(&self) -> WebUiStoreResult<u64> {
        Ok(self
            .object_store
            .list_keys_under_prefix(WEB_USERS_PREFIX)
            .await?
            .into_iter()
            .filter(|key| key.ends_with(".json"))
            .count() as u64)
    }

    async fn save_user(&self, record: WebUserRecord) -> WebUiStoreResult<()> {
        record.validate_web_record()?;
        ensure_login_available(&self.object_store, &record.normalized_login, record.user_id)
            .await?;
        if let Some(existing) = self.load_user(record.user_id).await? {
            if existing.normalized_login != record.normalized_login {
                self.object_store
                    .delete_object(&web_login_index_key(&existing.normalized_login))
                    .await?;
            }
        }

        let login_index = LoginIndexRecord {
            schema_version: WEB_AUTH_SCHEMA_VERSION,
            normalized_login: record.normalized_login.clone(),
            user_id: record.user_id,
        };
        self.object_store
            .save_json(&web_user_key(record.user_id), &record)
            .await?;
        self.object_store
            .save_json(
                &web_login_index_key(&login_index.normalized_login),
                &login_index,
            )
            .await
    }

    async fn load_user(&self, user_id: i64) -> WebUiStoreResult<Option<WebUserRecord>> {
        self.load_record(&web_user_key(user_id)).await
    }

    async fn load_login_index(
        &self,
        normalized_login: &str,
    ) -> WebUiStoreResult<Option<LoginIndexRecord>> {
        self.load_record(&web_login_index_key(normalized_login))
            .await
    }

    async fn save_auth_session(&self, record: WebAuthSessionRecord) -> WebUiStoreResult<()> {
        self.save_record(&web_auth_session_key(&record.session_token_hash), &record)
            .await
    }

    async fn load_auth_session(
        &self,
        session_token_hash: &str,
    ) -> WebUiStoreResult<Option<WebAuthSessionRecord>> {
        self.load_record(&web_auth_session_key(session_token_hash))
            .await
    }

    async fn revoke_auth_session(
        &self,
        session_token_hash: &str,
        revoked_at: DateTime<Utc>,
    ) -> WebUiStoreResult<bool> {
        let Some(mut record) = self.load_auth_session(session_token_hash).await? else {
            return Ok(false);
        };
        record.revoked_at = Some(revoked_at);
        self.save_auth_session(record).await?;
        Ok(true)
    }

    async fn revoke_auth_sessions_for_user_except(
        &self,
        user_id: i64,
        keep_session_token_hash: &str,
        revoked_at: DateTime<Utc>,
    ) -> WebUiStoreResult<u64> {
        let mut revoked = 0;
        let sessions = self
            .object_store
            .list_json_under_prefix::<WebAuthSessionRecord>(WEB_BROWSER_SESSIONS_PREFIX)
            .await?;
        for mut session in sessions {
            if session.user_id == user_id
                && session.session_token_hash != keep_session_token_hash
                && session.revoked_at.is_none()
            {
                session.revoked_at = Some(revoked_at);
                self.save_auth_session(session).await?;
                revoked += 1;
            }
        }
        Ok(revoked)
    }

    async fn save_session(&self, record: WebSessionRecord) -> WebUiStoreResult<()> {
        self.save_record(
            &web_session_key(record.user_id, &record.session_id),
            &record,
        )
        .await
    }

    async fn load_session(
        &self,
        user_id: i64,
        session_id: &str,
    ) -> WebUiStoreResult<Option<WebSessionRecord>> {
        self.load_record(&web_session_key(user_id, session_id))
            .await
    }

    async fn list_sessions(&self, user_id: i64) -> WebUiStoreResult<Vec<WebSessionRecord>> {
        let mut sessions = self
            .list_records::<WebSessionRecord>(&web_sessions_prefix(user_id))
            .await?;
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }

    async fn delete_session(&self, user_id: i64, session_id: &str) -> WebUiStoreResult<bool> {
        let key = web_session_key(user_id, session_id);
        if self.load_record::<WebSessionRecord>(&key).await?.is_none() {
            return Ok(false);
        }
        self.object_store.delete_object(&key).await?;
        self.object_store
            .delete_prefix(&web_tasks_prefix(user_id, session_id))
            .await?;
        self.object_store
            .delete_prefix(&web_task_events_prefix(user_id, session_id))
            .await?;
        self.object_store
            .delete_prefix(&web_task_files_prefix(user_id, session_id))
            .await?;
        let context_key = format!("web-session-{session_id}");
        let context_id =
            oxide_agent_core::agent::wiki_memory::scope::wiki_context_id(user_id, &context_key);
        let wiki_prefix = oxide_agent_core::storage::wiki_context_prefix("", &context_id);
        self.object_store.delete_prefix(&wiki_prefix).await?;
        Ok(true)
    }

    async fn save_task(&self, record: WebTaskRecord) -> WebUiStoreResult<()> {
        self.save_record(
            &web_task_key(record.user_id, &record.session_id, &record.task_id),
            &record,
        )
        .await
    }

    async fn load_task(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
    ) -> WebUiStoreResult<Option<WebTaskRecord>> {
        self.load_record(&web_task_key(user_id, session_id, task_id))
            .await
    }

    async fn list_tasks(
        &self,
        user_id: i64,
        session_id: &str,
    ) -> WebUiStoreResult<Vec<WebTaskRecord>> {
        let mut tasks = self
            .list_records::<WebTaskRecord>(&web_tasks_prefix(user_id, session_id))
            .await?;
        tasks.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(tasks)
    }

    async fn append_task_events(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        mut events: Vec<PersistedTaskEvent>,
    ) -> WebUiStoreResult<()> {
        events.sort_by_key(|event| event.seq);
        for events in
            events.chunk_by(|left, right| event_chunk_no(left.seq) == event_chunk_no(right.seq))
        {
            let chunk_no = event_chunk_no(events[0].seq);
            let mut chunk = self
                .load_event_chunk(user_id, session_id, task_id, chunk_no)
                .await?
                .unwrap_or_else(|| new_event_chunk(user_id, session_id, task_id, chunk_no));
            merge_event_chunk(&mut chunk.events, events);
            self.save_event_chunk(&chunk).await?;
        }
        Ok(())
    }

    async fn list_task_events(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> WebUiStoreResult<TaskEventsResponse> {
        let mut events = self
            .list_records::<WebTaskEventChunkRecord>(&web_task_event_chunks_prefix(
                user_id, session_id, task_id,
            ))
            .await?
            .into_iter()
            .flat_map(|chunk| chunk.events)
            .filter(|event| event.seq > after_seq)
            .collect::<Vec<_>>();
        events.sort_by_key(|event| event.seq);

        let has_more = events.len() > limit;
        events.truncate(limit);
        let last_seq = events.last().map_or(after_seq, |event| event.seq);
        Ok(TaskEventsResponse {
            events,
            last_seq,
            has_more,
        })
    }

    async fn save_task_file(
        &self,
        record: WebTaskFileRecord,
        content: Vec<u8>,
    ) -> WebUiStoreResult<()> {
        record.validate_web_record()?;
        if record.size_bytes != content.len() as u64 {
            return Err(WebUiStoreError::Unavailable(format!(
                "task file size mismatch for {}: metadata={}, content={}",
                record.file_id,
                record.size_bytes,
                content.len()
            )));
        }
        self.save_record(
            &web_task_file_key(
                record.user_id,
                &record.session_id,
                &record.task_id,
                &record.file_id,
            ),
            &record,
        )
        .await?;
        self.object_store
            .save_bytes(
                &web_task_file_blob_key(
                    record.user_id,
                    &record.session_id,
                    &record.task_id,
                    &record.file_id,
                ),
                &content,
                &record.content_type,
            )
            .await
    }

    async fn load_task_file(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        file_id: &str,
    ) -> WebUiStoreResult<Option<WebTaskFileBlob>> {
        let Some(record) = self
            .load_record::<WebTaskFileRecord>(&web_task_file_key(
                user_id, session_id, task_id, file_id,
            ))
            .await?
        else {
            return Ok(None);
        };
        let Some(content) = self
            .object_store
            .load_bytes(&web_task_file_blob_key(
                user_id, session_id, task_id, file_id,
            ))
            .await?
        else {
            return Err(WebUiStoreError::Unavailable(format!(
                "task file blob missing for {file_id}"
            )));
        };
        Ok(Some(WebTaskFileBlob { record, content }))
    }

    async fn mark_unfinished_tasks_interrupted(
        &self,
        message: &str,
        now: DateTime<Utc>,
    ) -> WebUiStoreResult<Vec<WebTaskRecord>> {
        let task_keys = self
            .object_store
            .list_keys_under_prefix(ALL_USERS_PREFIX)
            .await?
            .into_iter()
            .filter(|key| is_web_task_record_key(key))
            .collect::<Vec<_>>();

        let mut interrupted = Vec::new();
        for key in task_keys {
            let Some(mut task) = self.load_record::<WebTaskRecord>(&key).await? else {
                continue;
            };
            if !matches!(task.status, TaskStatus::Queued | TaskStatus::Running) {
                continue;
            }
            task.status = TaskStatus::Interrupted;
            task.error_message = Some(message.to_string());
            task.updated_at = now;
            task.finished_at = Some(now);
            self.save_task(task.clone()).await?;
            self.clear_interrupted_session_task(&task, now).await?;
            interrupted.push(task);
        }
        Ok(interrupted)
    }
}

impl<S> ObjectStoreWebUiStore<S>
where
    S: WebObjectStore,
{
    async fn load_event_chunk(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        chunk_no: u64,
    ) -> WebUiStoreResult<Option<WebTaskEventChunkRecord>> {
        self.load_record(&web_task_event_chunk_key(
            user_id, session_id, task_id, chunk_no,
        ))
        .await
    }

    async fn save_event_chunk(&self, chunk: &WebTaskEventChunkRecord) -> WebUiStoreResult<()> {
        self.save_record(
            &web_task_event_chunk_key(
                chunk.user_id,
                &chunk.session_id,
                &chunk.task_id,
                chunk.chunk_no,
            ),
            chunk,
        )
        .await
    }

    async fn save_record<T>(&self, key: &str, record: &T) -> WebUiStoreResult<()>
    where
        T: Serialize + Sync + Send + ValidateWebRecord,
    {
        record.validate_web_record()?;
        self.object_store.save_json(key, record).await
    }

    async fn load_record<T>(&self, key: &str) -> WebUiStoreResult<Option<T>>
    where
        T: DeserializeOwned + Send + ValidateWebRecord,
    {
        let Some(record) = self.object_store.load_json::<T>(key).await? else {
            return Ok(None);
        };
        record.validate_web_record()?;
        Ok(Some(record))
    }

    async fn list_records<T>(&self, prefix: &str) -> WebUiStoreResult<Vec<T>>
    where
        T: DeserializeOwned + Send + ValidateWebRecord,
    {
        let records = self
            .object_store
            .list_json_under_prefix::<T>(prefix)
            .await?;
        for record in &records {
            record.validate_web_record()?;
        }
        Ok(records)
    }

    async fn clear_interrupted_session_task(
        &self,
        task: &WebTaskRecord,
        now: DateTime<Utc>,
    ) -> WebUiStoreResult<()> {
        let Some(mut session) = self.load_session(task.user_id, &task.session_id).await? else {
            return Ok(());
        };
        if session.active_task_id.as_deref() != Some(task.task_id.as_str()) {
            return Ok(());
        }
        session.active_task_id = None;
        session.last_task_status = Some(TaskStatus::Interrupted);
        session.updated_at = now;
        self.save_session(session).await
    }
}

async fn ensure_login_available<S>(
    object_store: &S,
    normalized_login: &str,
    user_id: i64,
) -> WebUiStoreResult<()>
where
    S: WebObjectStore,
{
    if let Some(existing) = object_store
        .load_json::<LoginIndexRecord>(&web_login_index_key(normalized_login))
        .await?
    {
        existing.validate_web_record()?;
        if existing.user_id != user_id {
            return Err(WebUiStoreError::Conflict(format!(
                "login {normalized_login} already belongs to another user"
            )));
        }
    }
    Ok(())
}

fn new_event_chunk(
    user_id: i64,
    session_id: &str,
    task_id: &str,
    chunk_no: u64,
) -> WebTaskEventChunkRecord {
    WebTaskEventChunkRecord {
        schema_version: WEB_EVENT_CHUNK_SCHEMA_VERSION,
        user_id,
        session_id: session_id.to_string(),
        task_id: task_id.to_string(),
        chunk_no,
        events: Vec::new(),
    }
}

fn merge_event_chunk(existing: &mut Vec<PersistedTaskEvent>, incoming: &[PersistedTaskEvent]) {
    for event in incoming {
        if let Some(existing_event) = existing
            .iter_mut()
            .find(|existing_event| existing_event.seq == event.seq)
        {
            *existing_event = event.clone();
        } else {
            existing.push(event.clone());
        }
    }
    existing.sort_by_key(|event| event.seq);
}

fn event_chunk_no(seq: u64) -> u64 {
    seq.saturating_sub(1) / TASK_EVENT_CHUNK_SIZE
}

fn is_web_task_record_key(key: &str) -> bool {
    key.starts_with(ALL_USERS_PREFIX) && key.contains("/web/v1/tasks/") && key.ends_with(".json")
}

fn web_user_key(user_id: i64) -> String {
    format!("{WEB_USERS_PREFIX}{user_id}.json")
}

fn web_login_index_key(normalized_login: &str) -> String {
    format!("{WEB_LOGIN_INDEX_PREFIX}{normalized_login}.json")
}

fn web_auth_session_key(session_token_hash: &str) -> String {
    format!("{WEB_BROWSER_SESSIONS_PREFIX}{session_token_hash}.json")
}

fn web_sessions_prefix(user_id: i64) -> String {
    format!("users/{user_id}/web/v1/sessions/")
}

fn web_session_key(user_id: i64, session_id: &str) -> String {
    format!("{}{}.json", web_sessions_prefix(user_id), session_id)
}

fn web_tasks_prefix(user_id: i64, session_id: &str) -> String {
    format!("users/{user_id}/web/v1/tasks/{session_id}/")
}

fn web_task_key(user_id: i64, session_id: &str, task_id: &str) -> String {
    format!("{}{task_id}.json", web_tasks_prefix(user_id, session_id))
}

fn web_task_events_prefix(user_id: i64, session_id: &str) -> String {
    format!("users/{user_id}/web/v1/task_events/{session_id}/")
}

fn web_task_files_prefix(user_id: i64, session_id: &str) -> String {
    format!("users/{user_id}/web/v1/task_files/{session_id}/")
}

fn web_task_event_chunks_prefix(user_id: i64, session_id: &str, task_id: &str) -> String {
    format!("{}{task_id}/", web_task_events_prefix(user_id, session_id))
}

fn web_task_file_prefix(user_id: i64, session_id: &str, task_id: &str) -> String {
    format!("{}{task_id}/", web_task_files_prefix(user_id, session_id))
}

fn web_task_event_chunk_key(
    user_id: i64,
    session_id: &str,
    task_id: &str,
    chunk_no: u64,
) -> String {
    format!(
        "{}chunk-{chunk_no:012}.json",
        web_task_event_chunks_prefix(user_id, session_id, task_id)
    )
}

fn web_task_file_key(user_id: i64, session_id: &str, task_id: &str, file_id: &str) -> String {
    format!(
        "{}{}.json",
        web_task_file_prefix(user_id, session_id, task_id),
        file_id
    )
}

fn web_task_file_blob_key(user_id: i64, session_id: &str, task_id: &str, file_id: &str) -> String {
    format!(
        "{}{}.bin",
        web_task_file_prefix(user_id, session_id, task_id),
        file_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use oxide_agent_web_contracts::{TaskEventKind, UserRole};
    use std::collections::BTreeMap;
    use tokio::sync::RwLock;

    #[derive(Default)]
    struct InMemoryObjectStore {
        objects: RwLock<BTreeMap<String, serde_json::Value>>,
        blobs: RwLock<BTreeMap<String, Vec<u8>>>,
    }

    #[async_trait]
    impl WebObjectStore for InMemoryObjectStore {
        async fn save_json<T>(&self, key: &str, data: &T) -> WebUiStoreResult<()>
        where
            T: Serialize + Sync + Send,
        {
            let value = serde_json::to_value(data).map_err(test_store_error)?;
            self.objects.write().await.insert(key.to_string(), value);
            Ok(())
        }

        async fn load_json<T>(&self, key: &str) -> WebUiStoreResult<Option<T>>
        where
            T: DeserializeOwned + Send,
        {
            self.objects
                .read()
                .await
                .get(key)
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(test_store_error)
        }

        async fn save_bytes(
            &self,
            key: &str,
            content: &[u8],
            _content_type: &str,
        ) -> WebUiStoreResult<()> {
            self.blobs
                .write()
                .await
                .insert(key.to_string(), content.to_vec());
            Ok(())
        }

        async fn load_bytes(&self, key: &str) -> WebUiStoreResult<Option<Vec<u8>>> {
            Ok(self.blobs.read().await.get(key).cloned())
        }

        async fn delete_object(&self, key: &str) -> WebUiStoreResult<()> {
            self.objects.write().await.remove(key);
            self.blobs.write().await.remove(key);
            Ok(())
        }

        async fn delete_prefix(&self, prefix: &str) -> WebUiStoreResult<()> {
            self.objects
                .write()
                .await
                .retain(|key, _| !key.starts_with(prefix));
            self.blobs
                .write()
                .await
                .retain(|key, _| !key.starts_with(prefix));
            Ok(())
        }

        async fn list_json_under_prefix<T>(&self, prefix: &str) -> WebUiStoreResult<Vec<T>>
        where
            T: DeserializeOwned + Send,
        {
            self.objects
                .read()
                .await
                .iter()
                .filter(|(key, _)| key.starts_with(prefix))
                .map(|(_, value)| serde_json::from_value(value.clone()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(test_store_error)
        }

        async fn list_keys_under_prefix(&self, prefix: &str) -> WebUiStoreResult<Vec<String>> {
            Ok(self
                .objects
                .read()
                .await
                .keys()
                .filter(|key| key.starts_with(prefix))
                .cloned()
                .collect())
        }
    }

    fn test_store_error(error: impl std::fmt::Display) -> WebUiStoreError {
        WebUiStoreError::Unavailable(error.to_string())
    }

    #[test]
    fn r2_key_layout_matches_prd_prefixes() {
        assert_eq!(web_user_key(7), "web/auth/v1/users/7.json");
        assert_eq!(
            web_login_index_key("alice"),
            "web/auth/v1/login_index/alice.json"
        );
        assert_eq!(
            web_auth_session_key("hash"),
            "web/auth/v1/browser_sessions/hash.json"
        );
        assert_eq!(
            web_session_key(7, "session-1"),
            "users/7/web/v1/sessions/session-1.json"
        );
        assert_eq!(
            web_task_key(7, "session-1", "task-1"),
            "users/7/web/v1/tasks/session-1/task-1.json"
        );
        assert_eq!(
            web_task_event_chunk_key(7, "session-1", "task-1", 3),
            "users/7/web/v1/task_events/session-1/task-1/chunk-000000000003.json"
        );
        assert_eq!(
            web_task_file_key(7, "session-1", "task-1", "file-1"),
            "users/7/web/v1/task_files/session-1/task-1/file-1.json"
        );
        assert_eq!(
            web_task_file_blob_key(7, "session-1", "task-1", "file-1"),
            "users/7/web/v1/task_files/session-1/task-1/file-1.bin"
        );
    }

    #[tokio::test]
    async fn object_store_web_ui_store_round_trips_records_and_chunked_events() {
        let store = ObjectStoreWebUiStore::new(InMemoryObjectStore::default());
        let now = Utc::now();

        store.save_user(user_record(7, "alice", now)).await.unwrap();
        assert_eq!(store.users_count().await.unwrap(), 1);
        assert_eq!(
            store
                .load_login_index("alice")
                .await
                .unwrap()
                .map(|index| index.user_id),
            Some(7)
        );
        assert!(store.save_user(user_record(8, "alice", now)).await.is_err());

        store
            .save_auth_session(auth_session(7, "keep", now))
            .await
            .unwrap();
        store
            .save_auth_session(auth_session(7, "revoke", now))
            .await
            .unwrap();
        assert_eq!(
            store
                .revoke_auth_sessions_for_user_except(7, "keep", now + Duration::seconds(1))
                .await
                .unwrap(),
            1
        );
        assert!(store
            .load_auth_session("revoke")
            .await
            .unwrap()
            .and_then(|session| session.revoked_at)
            .is_some());

        let session = session_record(7, "session-1", now);
        store.save_session(session).await.unwrap();
        store
            .save_task(task_record(
                7,
                "session-1",
                "task-1",
                TaskStatus::Completed,
                now,
            ))
            .await
            .unwrap();
        store
            .append_task_events(
                7,
                "session-1",
                "task-1",
                vec![
                    event(7, "session-1", "task-1", 101),
                    event(7, "session-1", "task-1", 2),
                    event(7, "session-1", "task-1", 1),
                ],
            )
            .await
            .unwrap();
        store
            .save_task_file(
                WebTaskFileRecord {
                    schema_version: super::super::WEB_TASK_FILE_SCHEMA_VERSION,
                    user_id: 7,
                    session_id: "session-1".to_string(),
                    task_id: "task-1".to_string(),
                    file_id: "file-1".to_string(),
                    file_name: "report.txt".to_string(),
                    content_type: "text/plain".to_string(),
                    size_bytes: 5,
                    delivery_kind: oxide_agent_core::agent::progress::FileDeliveryKind::Document,
                    created_at: now,
                },
                b"hello".to_vec(),
            )
            .await
            .unwrap();

        let response = store
            .list_task_events(7, "session-1", "task-1", 1, 1)
            .await
            .unwrap();
        assert_eq!(response.events.len(), 1);
        assert_eq!(response.events[0].seq, 2);
        assert_eq!(response.last_seq, 2);
        assert!(response.has_more);
        let stored_file = store
            .load_task_file(7, "session-1", "task-1", "file-1")
            .await
            .unwrap()
            .expect("task file should exist");
        assert_eq!(stored_file.record.file_name, "report.txt");
        assert_eq!(stored_file.content, b"hello");

        assert!(store.delete_session(7, "session-1").await.unwrap());
        assert!(store.list_tasks(7, "session-1").await.unwrap().is_empty());
        assert!(store
            .list_task_events(7, "session-1", "task-1", 0, 10)
            .await
            .unwrap()
            .events
            .is_empty());
        assert!(store
            .load_task_file(7, "session-1", "task-1", "file-1")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn object_store_reconciles_unfinished_tasks_across_users() {
        let store = ObjectStoreWebUiStore::new(InMemoryObjectStore::default());
        let now = Utc::now();
        let reconcile_at = now + Duration::seconds(5);
        let mut session = session_record(7, "session-1", now);
        session.active_task_id = Some("running".to_string());
        store.save_session(session).await.unwrap();
        store
            .save_task(task_record(
                7,
                "session-1",
                "running",
                TaskStatus::Running,
                now,
            ))
            .await
            .unwrap();
        store
            .save_task(task_record(
                7,
                "session-1",
                "done",
                TaskStatus::Completed,
                now,
            ))
            .await
            .unwrap();
        store
            .save_task(task_record(8, "foreign", "queued", TaskStatus::Queued, now))
            .await
            .unwrap();

        let interrupted = store
            .mark_unfinished_tasks_interrupted("backend restarted", reconcile_at)
            .await
            .unwrap();
        assert_eq!(interrupted.len(), 2);
        assert_eq!(
            store
                .load_task(7, "session-1", "running")
                .await
                .unwrap()
                .map(|task| task.status),
            Some(TaskStatus::Interrupted)
        );
        assert_eq!(
            store
                .load_session(7, "session-1")
                .await
                .unwrap()
                .and_then(|session| session.active_task_id),
            None
        );
        assert_eq!(
            store
                .load_task(7, "session-1", "done")
                .await
                .unwrap()
                .map(|task| task.status),
            Some(TaskStatus::Completed)
        );
    }

    #[tokio::test]
    async fn object_store_rejects_unknown_schema_versions() {
        let store = ObjectStoreWebUiStore::new(InMemoryObjectStore::default());
        let now = Utc::now();

        let mut user = serde_json::to_value(user_record(7, "alice", now)).expect("user json");
        user["schema_version"] = serde_json::json!(99);
        store
            .object_store
            .objects
            .write()
            .await
            .insert(web_user_key(7), user);
        let error = store
            .load_user(7)
            .await
            .expect_err("unknown user schema version should fail safely");
        assert!(error.to_string().contains("schema_version 99"));

        let mut session =
            serde_json::to_value(session_record(7, "session-1", now)).expect("session json");
        session["schema_version"] = serde_json::json!(99);
        store
            .object_store
            .objects
            .write()
            .await
            .insert(web_session_key(7, "session-1"), session);
        let error = store
            .list_sessions(7)
            .await
            .expect_err("unknown session schema version should fail safely");
        assert!(error.to_string().contains("schema_version 99"));

        let mut event_chunk = new_event_chunk(7, "session-1", "task-1", 0);
        let mut event = event(7, "session-1", "task-1", 1);
        event.schema_version = 99;
        event_chunk.events.push(event);
        store
            .object_store
            .save_json(
                &web_task_event_chunk_key(7, "session-1", "task-1", 0),
                &event_chunk,
            )
            .await
            .expect("save raw chunk");
        let error = store
            .list_task_events(7, "session-1", "task-1", 0, 10)
            .await
            .expect_err("unknown event schema version should fail safely");
        assert!(error.to_string().contains("schema_version 99"));
    }

    #[tokio::test]
    async fn object_store_corrupt_records_return_errors_instead_of_panicking() {
        let store = ObjectStoreWebUiStore::new(InMemoryObjectStore::default());
        store
            .object_store
            .objects
            .write()
            .await
            .insert(web_user_key(7), serde_json::json!({ "schema_version": 1 }));

        let error = store
            .load_user(7)
            .await
            .expect_err("corrupt user record should fail safely");
        assert!(error.to_string().contains("missing field"));
    }

    fn user_record(user_id: i64, login: &str, now: DateTime<Utc>) -> WebUserRecord {
        WebUserRecord {
            schema_version: WEB_AUTH_SCHEMA_VERSION,
            user_id,
            login: login.to_string(),
            normalized_login: login.to_string(),
            password_hash: "argon2-hash".to_string(),
            role: UserRole::User,
            status: super::super::WebUserStatus::Active,
            created_at: now,
            updated_at: now,
            last_login_at: None,
        }
    }

    fn auth_session(
        user_id: i64,
        session_token_hash: &str,
        now: DateTime<Utc>,
    ) -> WebAuthSessionRecord {
        WebAuthSessionRecord {
            schema_version: WEB_AUTH_SCHEMA_VERSION,
            session_token_hash: session_token_hash.to_string(),
            user_id,
            csrf_token: "csrf".to_string(),
            created_at: now,
            last_seen_at: now,
            expires_at: now + Duration::days(1),
            revoked_at: None,
        }
    }

    fn session_record(user_id: i64, session_id: &str, now: DateTime<Utc>) -> WebSessionRecord {
        WebSessionRecord {
            schema_version: 1,
            session_id: session_id.to_string(),
            user_id,
            title: "Session".to_string(),
            context_key: format!("web-session-{session_id}"),
            agent_flow_id: "main".to_string(),
            created_at: now,
            updated_at: now,
            active_task_id: None,
            last_task_status: None,
            last_preview: None,
            manually_renamed: false,
        }
    }

    fn task_record(
        user_id: i64,
        session_id: &str,
        task_id: &str,
        status: TaskStatus,
        now: DateTime<Utc>,
    ) -> WebTaskRecord {
        WebTaskRecord {
            schema_version: 1,
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            user_id,
            version_group_id: task_id.to_string(),
            version_index: 1,
            parent_task_id: None,
            status,
            input_markdown: "Prompt".to_string(),
            attachments: Vec::new(),
            input_edited_at: None,
            final_response_markdown: status.is_terminal().then(|| "Done".to_string()),
            error_message: None,
            pending_user_input: None,
            last_progress: None,
            last_event_seq: 0,
            created_at: now,
            started_at: Some(now),
            updated_at: now,
            finished_at: status.is_terminal().then_some(now),
        }
    }

    fn event(user_id: i64, session_id: &str, task_id: &str, seq: u64) -> PersistedTaskEvent {
        PersistedTaskEvent {
            schema_version: 1,
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            user_id,
            seq,
            created_at: Utc::now(),
            kind: TaskEventKind::ToolResult,
            summary: format!("event-{seq}"),
            payload: serde_json::json!({ "seq": seq }),
            redacted: false,
            truncated: false,
        }
    }

    #[tokio::test]
    async fn delete_session_removes_wiki_context_objects() {
        let store = ObjectStoreWebUiStore::new(InMemoryObjectStore::default());
        let now = Utc::now();
        store
            .save_session(session_record(7, "s-wiki", now))
            .await
            .unwrap();

        // Insert wiki objects under the expected context prefix.
        let context_key = "web-session-s-wiki";
        let context_id =
            oxide_agent_core::agent::wiki_memory::scope::wiki_context_id(7, context_key);
        let wiki_prefix = oxide_agent_core::storage::wiki_context_prefix("", &context_id);
        let page_key = format!("{wiki_prefix}pages/runbook.md");
        let inbox_key = format!("{wiki_prefix}inbox/note.md");
        let overview_key = format!("{wiki_prefix}overview.md");

        store
            .object_store
            .save_json(&page_key, &"page content")
            .await
            .unwrap();
        store
            .object_store
            .save_json(&inbox_key, &"inbox content")
            .await
            .unwrap();
        store
            .object_store
            .save_json(&overview_key, &"overview content")
            .await
            .unwrap();

        // Insert a wiki object for a different session -- must survive.
        let foreign_context_id =
            oxide_agent_core::agent::wiki_memory::scope::wiki_context_id(7, "web-session-other");
        let foreign_prefix =
            oxide_agent_core::storage::wiki_context_prefix("", &foreign_context_id);
        let foreign_key = format!("{foreign_prefix}pages/runbook.md");
        store
            .object_store
            .save_json(&foreign_key, &"foreign content")
            .await
            .unwrap();

        assert!(store.delete_session(7, "s-wiki").await.unwrap());

        // Wiki objects for the deleted session must be gone.
        let remaining: Vec<String> = store
            .object_store
            .list_keys_under_prefix(&wiki_prefix)
            .await
            .unwrap();
        assert!(
            remaining.is_empty(),
            "wiki objects for deleted session should be removed, got: {remaining:?}"
        );

        // Foreign session wiki must be intact.
        let foreign_remaining: Vec<String> = store
            .object_store
            .list_keys_under_prefix(&foreign_prefix)
            .await
            .unwrap();
        assert_eq!(foreign_remaining, vec![foreign_key]);
    }

    #[tokio::test]
    async fn delete_session_returns_false_when_missing() {
        let store = ObjectStoreWebUiStore::new(InMemoryObjectStore::default());
        assert!(!store.delete_session(999, "nonexistent").await.unwrap());
    }
}
