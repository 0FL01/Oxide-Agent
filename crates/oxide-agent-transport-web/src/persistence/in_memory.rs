use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use oxide_agent_web_contracts::{
    PersistedTaskEvent, SessionSummary, TaskEventsResponse, TaskStatus, WebSessionRecord,
    WebTaskRecord,
};
use tokio::sync::RwLock;

use super::{
    LoginIndexRecord, ValidateWebRecord, WebAuthSessionRecord, WebSessionContextKeys,
    WebTaskEventState, WebTaskFileBlob, WebTaskFileRecord, WebUiStore, WebUiStoreError,
    WebUiStoreResult, WebUserRecord,
};

type SessionKey = (i64, String);
type TaskKey = (i64, String, String);
type TaskFileKey = (i64, String, String, String);

#[derive(Default)]
pub struct InMemoryWebUiStore {
    users: RwLock<HashMap<i64, WebUserRecord>>,
    login_index: RwLock<HashMap<String, LoginIndexRecord>>,
    auth_sessions: RwLock<HashMap<String, WebAuthSessionRecord>>,
    sessions: RwLock<HashMap<SessionKey, WebSessionRecord>>,
    tasks: RwLock<HashMap<TaskKey, WebTaskRecord>>,
    events: RwLock<HashMap<TaskKey, Vec<PersistedTaskEvent>>>,
    task_files: RwLock<HashMap<TaskFileKey, WebTaskFileBlob>>,
}

impl InMemoryWebUiStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn session_key(user_id: i64, session_id: &str) -> SessionKey {
        (user_id, session_id.to_string())
    }

    fn task_key(user_id: i64, session_id: &str, task_id: &str) -> TaskKey {
        (user_id, session_id.to_string(), task_id.to_string())
    }

    fn task_file_key(user_id: i64, session_id: &str, task_id: &str, file_id: &str) -> TaskFileKey {
        (
            user_id,
            session_id.to_string(),
            task_id.to_string(),
            file_id.to_string(),
        )
    }
}

#[async_trait]
impl WebUiStore for InMemoryWebUiStore {
    async fn users_count(&self) -> WebUiStoreResult<u64> {
        Ok(self.users.read().await.len() as u64)
    }

    async fn save_user(&self, record: WebUserRecord) -> WebUiStoreResult<()> {
        record.validate_web_record()?;
        let normalized_login = record.normalized_login.clone();
        let user_id = record.user_id;

        {
            let login_index = self.login_index.read().await;
            if let Some(existing) = login_index.get(&normalized_login) {
                if existing.user_id != user_id {
                    return Err(WebUiStoreError::Conflict(format!(
                        "login {normalized_login} already belongs to another user"
                    )));
                }
            }
        }

        self.users.write().await.insert(user_id, record);
        self.login_index.write().await.insert(
            normalized_login.clone(),
            LoginIndexRecord {
                schema_version: super::WEB_AUTH_SCHEMA_VERSION,
                normalized_login,
                user_id,
            },
        );
        Ok(())
    }

    async fn load_user(&self, user_id: i64) -> WebUiStoreResult<Option<WebUserRecord>> {
        Ok(self.users.read().await.get(&user_id).cloned())
    }

    async fn load_login_index(
        &self,
        normalized_login: &str,
    ) -> WebUiStoreResult<Option<LoginIndexRecord>> {
        Ok(self.login_index.read().await.get(normalized_login).cloned())
    }

    async fn save_auth_session(&self, record: WebAuthSessionRecord) -> WebUiStoreResult<()> {
        record.validate_web_record()?;
        self.auth_sessions
            .write()
            .await
            .insert(record.session_token_hash.clone(), record);
        Ok(())
    }

    async fn load_auth_session(
        &self,
        session_token_hash: &str,
    ) -> WebUiStoreResult<Option<WebAuthSessionRecord>> {
        Ok(self
            .auth_sessions
            .read()
            .await
            .get(session_token_hash)
            .cloned())
    }

    async fn revoke_auth_session(
        &self,
        session_token_hash: &str,
        revoked_at: DateTime<Utc>,
    ) -> WebUiStoreResult<bool> {
        let mut auth_sessions = self.auth_sessions.write().await;
        let Some(record) = auth_sessions.get_mut(session_token_hash) else {
            return Ok(false);
        };
        record.revoked_at = Some(revoked_at);
        Ok(true)
    }

    async fn revoke_auth_sessions_for_user_except(
        &self,
        user_id: i64,
        keep_session_token_hash: &str,
        revoked_at: DateTime<Utc>,
    ) -> WebUiStoreResult<u64> {
        let mut revoked = 0;
        for record in self.auth_sessions.write().await.values_mut() {
            if record.user_id == user_id
                && record.session_token_hash != keep_session_token_hash
                && record.revoked_at.is_none()
            {
                record.revoked_at = Some(revoked_at);
                revoked += 1;
            }
        }
        Ok(revoked)
    }

    async fn save_session(&self, record: WebSessionRecord) -> WebUiStoreResult<()> {
        record.validate_web_record()?;
        self.sessions.write().await.insert(
            Self::session_key(record.user_id, &record.session_id),
            record,
        );
        Ok(())
    }

    async fn load_session(
        &self,
        user_id: i64,
        session_id: &str,
    ) -> WebUiStoreResult<Option<WebSessionRecord>> {
        Ok(self
            .sessions
            .read()
            .await
            .get(&Self::session_key(user_id, session_id))
            .cloned())
    }

    async fn list_sessions(&self, user_id: i64) -> WebUiStoreResult<Vec<WebSessionRecord>> {
        let mut sessions = self
            .sessions
            .read()
            .await
            .values()
            .filter(|record| record.user_id == user_id)
            .cloned()
            .collect::<Vec<_>>();
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }

    async fn list_session_summaries(&self, user_id: i64) -> WebUiStoreResult<Vec<SessionSummary>> {
        let sessions = self.list_sessions(user_id).await?;
        Ok(sessions
            .into_iter()
            .map(|record| SessionSummary {
                session_id: record.session_id,
                title: record.title,
                model_selection: record.model_selection,
                agent_profile_id: record.agent_profile_id,
                last_preview: record.last_preview,
                active_task_id: record.active_task_id,
                last_task_status: record.last_task_status,
                created_at: record.created_at,
                updated_at: record.updated_at,
            })
            .collect())
    }

    async fn list_session_context_keys(
        &self,
        user_id: i64,
    ) -> WebUiStoreResult<Vec<WebSessionContextKeys>> {
        let mut sessions = self
            .sessions
            .read()
            .await
            .values()
            .filter(|record| record.user_id == user_id)
            .map(|record| WebSessionContextKeys {
                context_key: record.context_key.clone(),
                context_keys: record.context_keys.clone(),
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|a, b| a.context_key.cmp(&b.context_key));
        Ok(sessions)
    }

    async fn list_due_auto_title_sessions(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> WebUiStoreResult<Vec<WebSessionRecord>> {
        let mut sessions = self
            .sessions
            .read()
            .await
            .values()
            .filter(|record| {
                record.auto_title_source_message.is_some()
                    && !record.manually_renamed
                    && record
                        .auto_title_next_attempt_at
                        .is_none_or(|next_attempt_at| next_attempt_at <= now)
            })
            .cloned()
            .collect::<Vec<_>>();
        sessions.sort_by(|a, b| {
            a.auto_title_next_attempt_at
                .cmp(&b.auto_title_next_attempt_at)
                .then_with(|| a.updated_at.cmp(&b.updated_at))
        });
        sessions.truncate(limit);
        Ok(sessions)
    }

    async fn delete_session(&self, user_id: i64, session_id: &str) -> WebUiStoreResult<bool> {
        let removed = self
            .sessions
            .write()
            .await
            .remove(&Self::session_key(user_id, session_id))
            .is_some();

        if removed {
            self.tasks
                .write()
                .await
                .retain(|(task_user_id, task_session_id, _), _| {
                    *task_user_id != user_id || task_session_id != session_id
                });
            self.events
                .write()
                .await
                .retain(|(event_user_id, event_session_id, _), _| {
                    *event_user_id != user_id || event_session_id != session_id
                });
            self.task_files
                .write()
                .await
                .retain(|(file_user_id, file_session_id, _, _), _| {
                    *file_user_id != user_id || file_session_id != session_id
                });
        }

        Ok(removed)
    }

    async fn save_task(&self, record: WebTaskRecord) -> WebUiStoreResult<()> {
        record.validate_web_record()?;
        self.tasks.write().await.insert(
            Self::task_key(record.user_id, &record.session_id, &record.task_id),
            record,
        );
        Ok(())
    }

    async fn load_task(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
    ) -> WebUiStoreResult<Option<WebTaskRecord>> {
        Ok(self
            .tasks
            .read()
            .await
            .get(&Self::task_key(user_id, session_id, task_id))
            .cloned())
    }

    async fn task_exists(&self, user_id: i64, session_id: &str) -> WebUiStoreResult<bool> {
        Ok(self
            .tasks
            .read()
            .await
            .values()
            .any(|record| record.user_id == user_id && record.session_id == session_id))
    }

    async fn load_task_event_state(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
    ) -> WebUiStoreResult<Option<WebTaskEventState>> {
        Ok(self
            .tasks
            .read()
            .await
            .get(&Self::task_key(user_id, session_id, task_id))
            .map(|record| WebTaskEventState {
                status: record.status,
                last_event_seq: record.last_event_seq,
            }))
    }

    async fn list_tasks(
        &self,
        user_id: i64,
        session_id: &str,
    ) -> WebUiStoreResult<Vec<WebTaskRecord>> {
        let mut tasks = self
            .tasks
            .read()
            .await
            .values()
            .filter(|record| record.user_id == user_id && record.session_id == session_id)
            .cloned()
            .collect::<Vec<_>>();
        tasks.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(tasks)
    }

    async fn list_recent_tasks_page(
        &self,
        user_id: i64,
        session_id: &str,
        offset: usize,
        limit: usize,
    ) -> WebUiStoreResult<Vec<WebTaskRecord>> {
        let mut tasks = self
            .tasks
            .read()
            .await
            .values()
            .filter(|record| record.user_id == user_id && record.session_id == session_id)
            .cloned()
            .collect::<Vec<_>>();
        tasks.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.task_id.cmp(&a.task_id))
        });
        let mut page = tasks
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        page.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.task_id.cmp(&b.task_id))
        });
        Ok(page)
    }

    async fn append_task_events(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        mut events: Vec<PersistedTaskEvent>,
    ) -> WebUiStoreResult<()> {
        for event in &events {
            event.validate_web_record()?;
        }
        let key = Self::task_key(user_id, session_id, task_id);
        events.sort_by_key(|event| event.seq);
        self.events
            .write()
            .await
            .entry(key)
            .or_default()
            .extend(events);
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
        let events = self.events.read().await;
        let all_events = events
            .get(&Self::task_key(user_id, session_id, task_id))
            .cloned()
            .unwrap_or_default();

        let matching = all_events
            .into_iter()
            .filter(|event| event.seq > after_seq)
            .collect::<Vec<_>>();
        let has_more = matching.len() > limit;
        let events = matching.into_iter().take(limit).collect::<Vec<_>>();
        let first_seq = events.first().map_or(after_seq, |event| event.seq);
        let last_seq = events.last().map_or(after_seq, |event| event.seq);

        Ok(TaskEventsResponse {
            events,
            first_seq,
            last_seq,
            has_more,
        })
    }

    async fn list_task_events_before(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        before_seq: u64,
        limit: usize,
    ) -> WebUiStoreResult<TaskEventsResponse> {
        let events = self.events.read().await;
        let mut matching = events
            .get(&Self::task_key(user_id, session_id, task_id))
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|event| event.seq < before_seq)
            .collect::<Vec<_>>();
        matching.sort_by(|a, b| b.seq.cmp(&a.seq));

        let has_more = matching.len() > limit;
        let mut events = matching.into_iter().take(limit).collect::<Vec<_>>();
        events.sort_by_key(|event| event.seq);
        let first_seq = events.first().map_or(before_seq, |event| event.seq);
        let last_seq = events.last().map_or(0, |event| event.seq);

        Ok(TaskEventsResponse {
            events,
            first_seq,
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
        self.task_files.write().await.insert(
            Self::task_file_key(
                record.user_id,
                &record.session_id,
                &record.task_id,
                &record.file_id,
            ),
            WebTaskFileBlob { record, content },
        );
        Ok(())
    }

    async fn load_task_file(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        file_id: &str,
    ) -> WebUiStoreResult<Option<WebTaskFileBlob>> {
        Ok(self
            .task_files
            .read()
            .await
            .get(&Self::task_file_key(user_id, session_id, task_id, file_id))
            .cloned())
    }

    async fn mark_unfinished_tasks_interrupted(
        &self,
        message: &str,
        now: DateTime<Utc>,
    ) -> WebUiStoreResult<Vec<WebTaskRecord>> {
        let mut interrupted = Vec::new();
        let mut interrupted_keys = Vec::new();

        {
            let mut tasks = self.tasks.write().await;
            for (key, task) in tasks.iter_mut() {
                if matches!(task.status, TaskStatus::Queued | TaskStatus::Running) {
                    task.status = TaskStatus::Interrupted;
                    task.error_message = Some(message.to_string());
                    task.updated_at = now;
                    task.finished_at = Some(now);
                    interrupted_keys.push(key.clone());
                    interrupted.push(task.clone());
                }
            }
        }

        if !interrupted_keys.is_empty() {
            let mut sessions = self.sessions.write().await;
            for (user_id, session_id, task_id) in interrupted_keys {
                for session in sessions.values_mut() {
                    if session.user_id == user_id
                        && session.session_id == session_id
                        && session.active_task_id.as_deref() == Some(task_id.as_str())
                    {
                        session.active_task_id = None;
                        session.last_task_status = Some(TaskStatus::Interrupted);
                        session.updated_at = now;
                    }
                }
            }
        }

        Ok(interrupted)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration, Utc};
    use oxide_agent_web_contracts::{
        PersistedTaskEvent, TaskEventKind, TaskStatus, UserRole, WebSessionRecord, WebTaskRecord,
    };

    use super::super::{WebAuthSessionRecord, WebUiStore, WebUserRecord, WebUserStatus};
    use super::InMemoryWebUiStore;

    fn user_record(user_id: i64, login: &str) -> WebUserRecord {
        let now = Utc::now();
        WebUserRecord {
            schema_version: 1,
            user_id,
            login: login.to_string(),
            normalized_login: login.to_ascii_lowercase(),
            password_hash: "argon2id$hash".to_string(),
            role: UserRole::User,
            status: WebUserStatus::Active,
            default_model_selection: None,
            default_agent_profile_id: None,
            default_effort: None,
            created_at: now,
            updated_at: now,
            last_login_at: None,
        }
    }

    fn session_record(
        user_id: i64,
        session_id: &str,
        updated_at: DateTime<Utc>,
    ) -> WebSessionRecord {
        WebSessionRecord {
            schema_version: 1,
            session_id: session_id.to_string(),
            user_id,
            title: format!("Session {session_id}"),
            context_key: format!("web-session-{session_id}"),
            context_keys: vec![format!("web-session-{session_id}")],
            agent_flow_id: "main".to_string(),
            model_selection: None,
            agent_profile_id: None,
            created_at: updated_at,
            updated_at,
            active_task_id: None,
            last_task_status: None,
            last_preview: None,
            manually_renamed: false,
            auto_title_source_message: None,
            auto_title_replaceable_title: None,
            auto_title_attempts: 0,
            auto_title_next_attempt_at: None,
            auto_title_last_error: None,
        }
    }

    fn task_record(
        user_id: i64,
        session_id: &str,
        task_id: &str,
        status: TaskStatus,
        created_at: DateTime<Utc>,
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
            input_markdown: "Investigate".to_string(),
            attachments: Vec::new(),
            input_edited_at: None,
            final_response_markdown: None,
            error_message: None,
            pending_user_input: None,
            last_progress: None,
            last_event_seq: 0,
            created_at,
            started_at: Some(created_at),
            updated_at: created_at,
            finished_at: None,
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
            kind: TaskEventKind::Thinking,
            summary: format!("event {seq}"),
            payload: serde_json::json!({ "seq": seq }),
            redacted: false,
            truncated: false,
        }
    }

    #[tokio::test]
    async fn users_and_auth_sessions_round_trip() {
        let store = InMemoryWebUiStore::new();
        let user = user_record(42, "Alice");

        store.save_user(user.clone()).await.expect("save user");
        assert_eq!(store.users_count().await.expect("count users"), 1);
        assert_eq!(store.load_user(42).await.expect("load user"), Some(user));
        assert_eq!(
            store
                .load_login_index("alice")
                .await
                .expect("load login index")
                .map(|record| record.user_id),
            Some(42)
        );

        let now = Utc::now();
        let auth_session = WebAuthSessionRecord {
            schema_version: 1,
            session_token_hash: "token-hash".to_string(),
            user_id: 42,
            csrf_token: "csrf".to_string(),
            created_at: now,
            last_seen_at: now,
            expires_at: now + Duration::hours(1),
            revoked_at: None,
        };

        store
            .save_auth_session(auth_session)
            .await
            .expect("save auth session");
        assert!(store
            .revoke_auth_session("token-hash", now)
            .await
            .expect("revoke auth session"));
        assert!(store
            .load_auth_session("token-hash")
            .await
            .expect("load auth session")
            .and_then(|record| record.revoked_at)
            .is_some());
    }

    #[tokio::test]
    async fn sessions_tasks_and_events_are_user_scoped() {
        let store = InMemoryWebUiStore::new();
        let now = Utc::now();

        store
            .save_session(session_record(1, "older", now - Duration::minutes(5)))
            .await
            .expect("save older session");
        store
            .save_session(session_record(1, "newer", now))
            .await
            .expect("save newer session");
        store
            .save_session(session_record(2, "foreign", now + Duration::minutes(1)))
            .await
            .expect("save foreign session");

        let sessions = store.list_sessions(1).await.expect("list sessions");
        assert_eq!(
            sessions
                .iter()
                .map(|session| session.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["newer", "older"]
        );

        let task_one = task_record(1, "newer", "task-1", TaskStatus::Completed, now);
        let task_two = task_record(
            1,
            "newer",
            "task-2",
            TaskStatus::Completed,
            now + Duration::seconds(1),
        );
        store.save_task(task_two).await.expect("save task two");
        store.save_task(task_one).await.expect("save task one");
        store
            .save_task(task_record(
                2,
                "foreign",
                "foreign-task",
                TaskStatus::Completed,
                now,
            ))
            .await
            .expect("save foreign task");

        let tasks = store.list_tasks(1, "newer").await.expect("list tasks");
        assert_eq!(
            tasks
                .iter()
                .map(|task| task.task_id.as_str())
                .collect::<Vec<_>>(),
            vec!["task-1", "task-2"]
        );
        assert!(store.task_exists(1, "newer").await.expect("task exists"));
        let task_state = store
            .load_task_event_state(1, "newer", "task-1")
            .await
            .expect("load task event state")
            .expect("task event state exists");
        assert_eq!(task_state.status, TaskStatus::Completed);
        assert_eq!(task_state.last_event_seq, 0);

        store
            .append_task_events(
                1,
                "newer",
                "task-1",
                vec![
                    event(1, "newer", "task-1", 3),
                    event(1, "newer", "task-1", 2),
                ],
            )
            .await
            .expect("append events");

        let response = store
            .list_task_events(1, "newer", "task-1", 1, 1)
            .await
            .expect("list events");
        assert_eq!(response.events.len(), 1);
        assert_eq!(response.events[0].seq, 2);
        assert_eq!(response.first_seq, 2);
        assert_eq!(response.last_seq, 2);
        assert!(response.has_more);

        let tail = store
            .list_task_events_before(1, "newer", "task-1", 4, 1)
            .await
            .expect("list event tail");
        assert_eq!(tail.events.len(), 1);
        assert_eq!(tail.events[0].seq, 3);
        assert_eq!(tail.first_seq, 3);
        assert_eq!(tail.last_seq, 3);
        assert!(tail.has_more);

        assert!(store
            .list_task_events(2, "foreign", "task-1", 0, 100)
            .await
            .expect("list foreign events")
            .events
            .is_empty());
    }

    #[tokio::test]
    async fn rejects_unknown_schema_versions_before_storing_records() {
        let store = InMemoryWebUiStore::new();
        let now = Utc::now();

        let mut user = user_record(42, "Alice");
        user.schema_version = 99;
        let error = store
            .save_user(user)
            .await
            .expect_err("unknown user schema version should fail");
        assert!(error.to_string().contains("schema_version 99"));
        assert_eq!(store.users_count().await.expect("count users"), 0);

        let mut session = session_record(42, "session-1", now);
        session.schema_version = 99;
        let error = store
            .save_session(session)
            .await
            .expect_err("unknown session schema version should fail");
        assert!(error.to_string().contains("schema_version 99"));

        let mut task = task_record(42, "session-1", "task-1", TaskStatus::Running, now);
        task.schema_version = 99;
        let error = store
            .save_task(task)
            .await
            .expect_err("unknown task schema version should fail");
        assert!(error.to_string().contains("schema_version 99"));

        let mut event = event(42, "session-1", "task-1", 1);
        event.schema_version = 99;
        let error = store
            .append_task_events(42, "session-1", "task-1", vec![event])
            .await
            .expect_err("unknown event schema version should fail");
        assert!(error.to_string().contains("schema_version 99"));
    }

    #[tokio::test]
    async fn startup_reconciliation_interrupts_queued_and_running_tasks() {
        let store = InMemoryWebUiStore::new();
        let now = Utc::now();
        let reconcile_at = now + Duration::minutes(1);
        let mut session = session_record(1, "session", now);
        session.active_task_id = Some("running".to_string());
        store.save_session(session).await.expect("save session");
        store
            .save_task(task_record(1, "session", "queued", TaskStatus::Queued, now))
            .await
            .expect("save queued task");
        store
            .save_task(task_record(
                1,
                "session",
                "running",
                TaskStatus::Running,
                now,
            ))
            .await
            .expect("save running task");
        store
            .save_task(task_record(
                1,
                "session",
                "completed",
                TaskStatus::Completed,
                now,
            ))
            .await
            .expect("save completed task");

        let interrupted = store
            .mark_unfinished_tasks_interrupted("backend restarted", reconcile_at)
            .await
            .expect("reconcile tasks");
        assert_eq!(interrupted.len(), 2);

        let running = store
            .load_task(1, "session", "running")
            .await
            .expect("load running task")
            .expect("running task exists");
        assert_eq!(running.status, TaskStatus::Interrupted);
        assert_eq!(running.error_message.as_deref(), Some("backend restarted"));

        let completed = store
            .load_task(1, "session", "completed")
            .await
            .expect("load completed task")
            .expect("completed task exists");
        assert_eq!(completed.status, TaskStatus::Completed);

        let session = store
            .load_session(1, "session")
            .await
            .expect("load session")
            .expect("session exists");
        assert_eq!(session.active_task_id, None);
        assert_eq!(session.last_task_status, Some(TaskStatus::Interrupted));
    }
}
