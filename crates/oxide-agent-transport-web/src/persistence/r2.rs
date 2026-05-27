use async_trait::async_trait;
use chrono::{DateTime, Utc};
use oxide_agent_web_contracts::{
    PersistedTaskEvent, TaskEventsResponse, TaskStatus, WebSessionRecord, WebTaskRecord,
};
use serde::{de::DeserializeOwned, Serialize};

use super::{
    LoginIndexRecord, WebAuthSessionRecord, WebTaskEventChunkRecord, WebUiStore, WebUiStoreError,
    WebUiStoreResult, WebUserRecord, WEB_AUTH_SCHEMA_VERSION,
};

const WEB_EVENT_CHUNK_SCHEMA_VERSION: u32 = 1;
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
        self.object_store.load_json(&web_user_key(user_id)).await
    }

    async fn load_login_index(
        &self,
        normalized_login: &str,
    ) -> WebUiStoreResult<Option<LoginIndexRecord>> {
        self.object_store
            .load_json(&web_login_index_key(normalized_login))
            .await
    }

    async fn save_auth_session(&self, record: WebAuthSessionRecord) -> WebUiStoreResult<()> {
        self.object_store
            .save_json(&web_auth_session_key(&record.session_token_hash), &record)
            .await
    }

    async fn load_auth_session(
        &self,
        session_token_hash: &str,
    ) -> WebUiStoreResult<Option<WebAuthSessionRecord>> {
        self.object_store
            .load_json(&web_auth_session_key(session_token_hash))
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
        self.object_store
            .save_json(
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
        self.object_store
            .load_json(&web_session_key(user_id, session_id))
            .await
    }

    async fn list_sessions(&self, user_id: i64) -> WebUiStoreResult<Vec<WebSessionRecord>> {
        let mut sessions = self
            .object_store
            .list_json_under_prefix::<WebSessionRecord>(&web_sessions_prefix(user_id))
            .await?;
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }

    async fn delete_session(&self, user_id: i64, session_id: &str) -> WebUiStoreResult<bool> {
        let key = web_session_key(user_id, session_id);
        if self
            .object_store
            .load_json::<WebSessionRecord>(&key)
            .await?
            .is_none()
        {
            return Ok(false);
        }
        self.object_store.delete_object(&key).await?;
        self.object_store
            .delete_prefix(&web_tasks_prefix(user_id, session_id))
            .await?;
        self.object_store
            .delete_prefix(&web_task_events_prefix(user_id, session_id))
            .await?;
        Ok(true)
    }

    async fn save_task(&self, record: WebTaskRecord) -> WebUiStoreResult<()> {
        self.object_store
            .save_json(
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
        self.object_store
            .load_json(&web_task_key(user_id, session_id, task_id))
            .await
    }

    async fn list_tasks(
        &self,
        user_id: i64,
        session_id: &str,
    ) -> WebUiStoreResult<Vec<WebTaskRecord>> {
        let mut tasks = self
            .object_store
            .list_json_under_prefix::<WebTaskRecord>(&web_tasks_prefix(user_id, session_id))
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
            .object_store
            .list_json_under_prefix::<WebTaskEventChunkRecord>(&web_task_event_chunks_prefix(
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
            let Some(mut task) = self.object_store.load_json::<WebTaskRecord>(&key).await? else {
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
        self.object_store
            .load_json(&web_task_event_chunk_key(
                user_id, session_id, task_id, chunk_no,
            ))
            .await
    }

    async fn save_event_chunk(&self, chunk: &WebTaskEventChunkRecord) -> WebUiStoreResult<()> {
        self.object_store
            .save_json(
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

fn web_task_event_chunks_prefix(user_id: i64, session_id: &str, task_id: &str) -> String {
    format!("{}{task_id}/", web_task_events_prefix(user_id, session_id))
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

        async fn delete_object(&self, key: &str) -> WebUiStoreResult<()> {
            self.objects.write().await.remove(key);
            Ok(())
        }

        async fn delete_prefix(&self, prefix: &str) -> WebUiStoreResult<()> {
            self.objects
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

        let response = store
            .list_task_events(7, "session-1", "task-1", 1, 1)
            .await
            .unwrap();
        assert_eq!(response.events.len(), 1);
        assert_eq!(response.events[0].seq, 2);
        assert_eq!(response.last_seq, 2);
        assert!(response.has_more);

        assert!(store.delete_session(7, "session-1").await.unwrap());
        assert!(store.list_tasks(7, "session-1").await.unwrap().is_empty());
        assert!(store
            .list_task_events(7, "session-1", "task-1", 0, 10)
            .await
            .unwrap()
            .events
            .is_empty());
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
            status,
            input_markdown: "Prompt".to_string(),
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
}
