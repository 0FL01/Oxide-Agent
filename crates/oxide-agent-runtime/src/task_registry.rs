//! In-memory registry for runtime task metadata and cancellation.

use crate::task_events::{NoopTaskEventPublisher, SharedTaskEventPublisher};
use oxide_agent_core::agent::{
    SessionId, TaskEvent, TaskEventKind, TaskId, TaskMetadata, TaskState, TaskStateTransitionError,
};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
struct TaskEntry {
    metadata: TaskMetadata,
    last_event_sequence: u64,
    session_id: SessionId,
    cancellation_token: Arc<CancellationToken>,
}

#[derive(Default)]
struct TaskRegistryState {
    tasks: HashMap<TaskId, TaskEntry>,
    session_tasks: HashMap<SessionId, Vec<TaskId>>,
}

/// Task metadata plus its owning session identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRecord {
    /// Stable task metadata.
    pub metadata: TaskMetadata,
    /// Session that owns the task.
    pub session_id: SessionId,
}

/// Result of a task state transition together with its event sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskStateUpdate {
    /// Updated task record after the transition.
    pub record: TaskRecord,
    /// Event sequence assigned to the published lifecycle event.
    pub event_sequence: u64,
}

/// Outcome of a runtime task cancellation request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskCancellation {
    /// Cancellation transitioned the task into a terminal cancelled state.
    Cancelled(TaskStateUpdate),
    /// Cancellation was requested after the task already reached a terminal state.
    AlreadyTerminal(TaskStateUpdate),
}

impl From<TaskEntry> for TaskRecord {
    fn from(value: TaskEntry) -> Self {
        Self {
            metadata: value.metadata,
            session_id: value.session_id,
        }
    }
}

impl TaskStateUpdate {
    fn from_entry(entry: &TaskEntry) -> Self {
        Self {
            record: TaskRecord::from(entry.clone()),
            event_sequence: entry.last_event_sequence,
        }
    }
}

/// Errors returned by task registry mutations.
#[derive(Debug, PartialEq, Eq)]
pub enum TaskRegistryError {
    /// The requested task does not exist in the registry.
    TaskNotFound(TaskId),
    /// The requested state transition violates the task state machine.
    InvalidStateTransition(TaskStateTransitionError),
}

impl fmt::Display for TaskRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TaskNotFound(task_id) => write!(f, "task not found: {task_id}"),
            Self::InvalidStateTransition(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TaskRegistryError {}

impl From<TaskStateTransitionError> for TaskRegistryError {
    fn from(value: TaskStateTransitionError) -> Self {
        Self::InvalidStateTransition(value)
    }
}

/// In-memory runtime registry for tasks, ownership, and cancellation tokens.
pub struct TaskRegistry {
    state: RwLock<TaskRegistryState>,
    publisher: SharedTaskEventPublisher,
}

impl Default for TaskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskRegistry {
    /// Create a new empty task registry.
    #[must_use]
    pub fn new() -> Self {
        Self::with_event_publisher(Arc::new(NoopTaskEventPublisher))
    }

    /// Create a new empty task registry with a task event publisher.
    #[must_use]
    pub fn with_event_publisher(publisher: SharedTaskEventPublisher) -> Self {
        Self {
            state: RwLock::new(TaskRegistryState::default()),
            publisher,
        }
    }

    /// Create a new pending task for the provided session.
    pub async fn create(&self, session_id: SessionId) -> TaskRecord {
        let metadata = TaskMetadata::new();
        let task_id = metadata.id;
        let event = TaskEvent::new(task_id, 1, TaskEventKind::Created, metadata.state, None);
        let entry = TaskEntry {
            metadata,
            last_event_sequence: event.sequence,
            session_id,
            cancellation_token: Arc::new(CancellationToken::new()),
        };
        let record = {
            let mut state = self.state.write().await;
            state.tasks.insert(task_id, entry.clone());
            state
                .session_tasks
                .entry(session_id)
                .or_default()
                .push(task_id);

            TaskRecord::from(entry)
        };

        self.publisher.publish(event).await;

        record
    }

    /// Restore a persisted task record into the runtime registry without emitting new events.
    pub async fn restore(
        &self,
        metadata: TaskMetadata,
        session_id: SessionId,
        last_event_sequence: u64,
    ) -> TaskRecord {
        let task_id = metadata.id;
        let entry = TaskEntry {
            metadata,
            last_event_sequence,
            session_id,
            cancellation_token: Arc::new(CancellationToken::new()),
        };
        let record = TaskRecord::from(entry.clone());

        let mut state = self.state.write().await;
        state.tasks.insert(task_id, entry);
        let mut task_ids = state
            .session_tasks
            .get(&session_id)
            .cloned()
            .unwrap_or_default();
        if !task_ids.contains(&task_id) {
            task_ids.push(task_id);
        }
        sort_task_ids(&mut task_ids, &state.tasks);
        state.session_tasks.insert(session_id, task_ids);

        record
    }

    /// Get a task record by task identifier.
    pub async fn get(&self, task_id: &TaskId) -> Option<TaskRecord> {
        let state = self.state.read().await;
        state.tasks.get(task_id).cloned().map(TaskRecord::from)
    }

    /// Get a task state update view for the current registry entry.
    pub async fn get_update(&self, task_id: &TaskId) -> Option<TaskStateUpdate> {
        let state = self.state.read().await;
        state.tasks.get(task_id).map(TaskStateUpdate::from_entry)
    }

    /// Return the owning session identifier for a task.
    pub async fn session_id_for_task(&self, task_id: &TaskId) -> Option<SessionId> {
        let state = self.state.read().await;
        state.tasks.get(task_id).map(|entry| entry.session_id)
    }

    /// List all known tasks.
    pub async fn list(&self) -> Vec<TaskRecord> {
        let state = self.state.read().await;
        let mut records = state
            .tasks
            .values()
            .cloned()
            .map(TaskRecord::from)
            .collect::<Vec<_>>();
        sort_task_records(&mut records);
        records
    }

    /// List all tasks owned by a session.
    pub async fn list_by_session(&self, session_id: &SessionId) -> Vec<TaskRecord> {
        let state = self.state.read().await;
        let mut records = state
            .session_tasks
            .get(session_id)
            .into_iter()
            .flatten()
            .filter_map(|task_id| state.tasks.get(task_id).cloned())
            .map(TaskRecord::from)
            .collect::<Vec<_>>();
        sort_task_records(&mut records);
        records
    }

    /// Return the latest non-terminal task owned by a session.
    pub async fn latest_non_terminal_by_session(
        &self,
        session_id: &SessionId,
    ) -> Option<TaskRecord> {
        let state = self.state.read().await;
        state
            .session_tasks
            .get(session_id)
            .into_iter()
            .flatten()
            .rev()
            .filter_map(|task_id| state.tasks.get(task_id))
            .find(|entry| !entry.metadata.state.is_terminal())
            .cloned()
            .map(TaskRecord::from)
    }

    /// Transition a task to a new lifecycle state.
    pub async fn update_state(
        &self,
        task_id: &TaskId,
        next_state: TaskState,
    ) -> Result<TaskStateUpdate, TaskRegistryError> {
        let (update, event) = {
            let mut state = self.state.write().await;
            let entry = state
                .tasks
                .get_mut(task_id)
                .ok_or(TaskRegistryError::TaskNotFound(*task_id))?;
            entry.metadata.transition_to(next_state)?;
            entry.last_event_sequence += 1;

            let event = TaskEvent::new(
                entry.metadata.id,
                entry.last_event_sequence,
                TaskEventKind::StateChanged,
                entry.metadata.state,
                None,
            );

            (TaskStateUpdate::from_entry(entry), event)
        };

        self.publisher.publish(event).await;

        Ok(update)
    }

    /// Request cancellation for a task.
    pub async fn cancel(&self, task_id: &TaskId) -> Result<TaskCancellation, TaskRegistryError> {
        let (outcome, event) = {
            let mut state = self.state.write().await;
            let entry = state
                .tasks
                .get_mut(task_id)
                .ok_or(TaskRegistryError::TaskNotFound(*task_id))?;

            if entry.metadata.state.is_terminal() {
                (
                    TaskCancellation::AlreadyTerminal(TaskStateUpdate::from_entry(entry)),
                    None,
                )
            } else {
                entry.cancellation_token.cancel();
                entry.metadata.transition_to(TaskState::Cancelled)?;
                entry.last_event_sequence += 1;

                let event = TaskEvent::new(
                    entry.metadata.id,
                    entry.last_event_sequence,
                    TaskEventKind::StateChanged,
                    entry.metadata.state,
                    None,
                );

                (
                    TaskCancellation::Cancelled(TaskStateUpdate::from_entry(entry)),
                    Some(event),
                )
            }
        };

        if let Some(event) = event {
            self.publisher.publish(event).await;
        }

        Ok(outcome)
    }

    /// Renew the cancellation token for a task.
    pub async fn renew_cancellation_token(&self, task_id: &TaskId) -> bool {
        let mut state = self.state.write().await;
        if let Some(entry) = state.tasks.get_mut(task_id) {
            entry.cancellation_token = Arc::new(CancellationToken::new());
            true
        } else {
            false
        }
    }

    /// Get the task-scoped cancellation token.
    pub async fn get_cancellation_token(&self, task_id: &TaskId) -> Option<Arc<CancellationToken>> {
        let state = self.state.read().await;
        state
            .tasks
            .get(task_id)
            .map(|entry| Arc::clone(&entry.cancellation_token))
    }

    /// Remove a task from the runtime registry.
    pub async fn remove(&self, task_id: &TaskId) -> Option<TaskRecord> {
        let mut state = self.state.write().await;
        let entry = state.tasks.remove(task_id)?;

        if let Some(task_ids) = state.session_tasks.get_mut(&entry.session_id) {
            task_ids.retain(|candidate| candidate != task_id);
            if task_ids.is_empty() {
                state.session_tasks.remove(&entry.session_id);
            }
        }

        Some(TaskRecord::from(entry))
    }
}

fn sort_task_records(records: &mut [TaskRecord]) {
    records.sort_by(|left, right| {
        left.metadata
            .created_at
            .cmp(&right.metadata.created_at)
            .then_with(|| {
                let left_id = left.metadata.id.as_uuid();
                let right_id = right.metadata.id.as_uuid();
                left_id.cmp(&right_id)
            })
    });
}

fn sort_task_ids(task_ids: &mut [TaskId], tasks: &HashMap<TaskId, TaskEntry>) {
    task_ids.sort_by(|left, right| {
        let left_entry = tasks.get(left);
        let right_entry = tasks.get(right);

        match (left_entry, right_entry) {
            (Some(left_entry), Some(right_entry)) => left_entry
                .metadata
                .created_at
                .cmp(&right_entry.metadata.created_at)
                .then_with(|| left.as_uuid().cmp(&right.as_uuid())),
            _ => left.as_uuid().cmp(&right.as_uuid()),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{TaskCancellation, TaskRegistry, TaskRegistryError};
    use crate::task_events::{ChannelTaskEventPublisher, TaskEventPublisher};
    use async_trait::async_trait;
    use oxide_agent_core::agent::{SessionId, TaskEventKind, TaskState, TaskStateTransitionError};
    use std::sync::Arc;
    use tokio::sync::{mpsc::unbounded_channel, Notify};
    use tokio::task::yield_now;
    use tokio::time::{timeout, Duration};

    #[derive(Debug)]
    struct BlockingTaskEventPublisher {
        publish_started: Arc<Notify>,
        release_publish: Arc<Notify>,
    }

    #[async_trait]
    impl TaskEventPublisher for BlockingTaskEventPublisher {
        async fn publish(&self, _event: oxide_agent_core::agent::TaskEvent) {
            self.publish_started.notify_one();
            self.release_publish.notified().await;
        }
    }

    #[tokio::test]
    async fn task_registry_creates_and_updates_task_state() {
        let registry = TaskRegistry::new();
        let session_id = SessionId::from(42);

        let created = registry.create(session_id).await;
        assert_eq!(created.session_id, session_id);
        assert_eq!(created.metadata.state, TaskState::Pending);

        let running = registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await;
        assert!(
            matches!(running, Ok(update) if update.record.metadata.state == TaskState::Running)
        );

        let fetched = registry.get(&created.metadata.id).await;
        assert!(matches!(fetched, Some(record) if record.metadata.state == TaskState::Running));
    }

    #[tokio::test]
    async fn task_registry_lists_tasks_by_session_and_globally() {
        let registry = TaskRegistry::new();
        let first_session = SessionId::from(1);
        let second_session = SessionId::from(2);

        let first = registry.create(first_session).await;
        let second = registry.create(first_session).await;
        let third = registry.create(second_session).await;

        let by_session = registry.list_by_session(&first_session).await;
        assert_eq!(by_session.len(), 2);
        assert!(by_session[0].metadata.created_at <= by_session[1].metadata.created_at);
        assert!(by_session
            .iter()
            .all(|record| record.session_id == first_session));
        assert!(by_session
            .iter()
            .any(|record| record.metadata.id == first.metadata.id));
        assert!(by_session
            .iter()
            .any(|record| record.metadata.id == second.metadata.id));

        let all_tasks = registry.list().await;
        assert_eq!(all_tasks.len(), 3);
        assert!(all_tasks
            .iter()
            .any(|record| record.metadata.id == third.metadata.id));
    }

    #[tokio::test]
    async fn task_registry_keeps_visible_tasks_consistent_during_concurrent_access() {
        let registry = Arc::new(TaskRegistry::new());
        let session_id = SessionId::from(55);

        let mut create_handles = Vec::new();
        for _ in 0..32 {
            let registry = Arc::clone(&registry);
            create_handles.push(tokio::spawn(
                async move { registry.create(session_id).await },
            ));
        }

        let reader_registry = Arc::clone(&registry);
        let reader_handle = tokio::spawn(async move {
            let mut observed = 0usize;

            while observed < 32 {
                let tasks = reader_registry.list_by_session(&session_id).await;
                observed = observed.max(tasks.len());

                for task in tasks {
                    assert_eq!(
                        reader_registry.session_id_for_task(&task.metadata.id).await,
                        Some(session_id)
                    );
                    assert!(reader_registry
                        .get_cancellation_token(&task.metadata.id)
                        .await
                        .is_some());
                    let cancellation = reader_registry.cancel(&task.metadata.id).await;
                    assert!(cancellation.is_ok());
                }

                yield_now().await;
            }
        });

        for handle in create_handles {
            let result = handle.await;
            assert!(result.is_ok());
        }
        assert!(reader_handle.await.is_ok());

        let tasks = registry.list_by_session(&session_id).await;
        assert_eq!(tasks.len(), 32);
        for task in tasks {
            assert_eq!(
                registry.session_id_for_task(&task.metadata.id).await,
                Some(session_id)
            );
            assert!(registry
                .get_cancellation_token(&task.metadata.id)
                .await
                .is_some());
        }
    }

    #[tokio::test]
    async fn task_registry_lists_session_tasks_in_deterministic_order() {
        let registry = TaskRegistry::new();
        let session_id = SessionId::from(77);

        let first = registry.create(session_id).await;
        let second = registry.create(session_id).await;
        let third = registry.create(session_id).await;

        let listed = registry.list_by_session(&session_id).await;

        assert_eq!(
            listed
                .iter()
                .map(|record| record.metadata.id)
                .collect::<Vec<_>>(),
            vec![first.metadata.id, second.metadata.id, third.metadata.id]
        );
    }

    #[tokio::test]
    async fn task_registry_manages_task_cancellation_tokens() {
        let registry = TaskRegistry::new();
        let created = registry.create(SessionId::from(7)).await;

        let initial_token = registry.get_cancellation_token(&created.metadata.id).await;
        assert!(matches!(initial_token, Some(ref token) if !token.is_cancelled()));

        let cancellation = registry.cancel(&created.metadata.id).await;
        assert!(matches!(cancellation, Ok(TaskCancellation::Cancelled(_))));

        let cancelled_token = registry.get_cancellation_token(&created.metadata.id).await;
        assert!(matches!(cancelled_token, Some(ref token) if token.is_cancelled()));

        assert!(
            registry
                .renew_cancellation_token(&created.metadata.id)
                .await
        );

        let renewed_token = registry.get_cancellation_token(&created.metadata.id).await;
        assert!(matches!(renewed_token, Some(ref token) if !token.is_cancelled()));

        assert!(matches!(
            (initial_token, renewed_token),
            (Some(initial_token), Some(renewed_token)) if !Arc::ptr_eq(&initial_token, &renewed_token)
        ));
    }

    #[tokio::test]
    async fn task_registry_rejects_invalid_transitions() {
        let registry = TaskRegistry::new();
        let created = registry.create(SessionId::from(11)).await;

        let result = registry
            .update_state(&created.metadata.id, TaskState::Completed)
            .await;

        assert_eq!(
            result,
            Err(TaskRegistryError::InvalidStateTransition(
                TaskStateTransitionError::InvalidTransition {
                    from: TaskState::Pending,
                    to: TaskState::Completed,
                }
            ))
        );
    }

    #[tokio::test]
    async fn task_registry_keeps_concurrent_task_updates_isolated() {
        let registry = Arc::new(TaskRegistry::new());
        let first = registry.create(SessionId::from(100)).await;
        let second = registry.create(SessionId::from(200)).await;

        let first_registry = Arc::clone(&registry);
        let first_task_id = first.metadata.id;
        let first_handle = tokio::spawn(async move {
            first_registry
                .update_state(&first_task_id, TaskState::Running)
                .await
        });

        let second_registry = Arc::clone(&registry);
        let second_task_id = second.metadata.id;
        let second_handle = tokio::spawn(async move {
            second_registry
                .update_state(&second_task_id, TaskState::Running)
                .await
        });

        let first_result = first_handle.await;
        let second_result = second_handle.await;

        assert!(matches!(
            first_result,
            Ok(Ok(update)) if update.record.metadata.state == TaskState::Running
        ));
        assert!(matches!(
            second_result,
            Ok(Ok(update)) if update.record.metadata.state == TaskState::Running
        ));
        assert_eq!(
            registry.session_id_for_task(&first.metadata.id).await,
            Some(SessionId::from(100))
        );
        assert_eq!(
            registry.session_id_for_task(&second.metadata.id).await,
            Some(SessionId::from(200))
        );
    }

    #[tokio::test]
    async fn task_events_are_published_for_task_lifecycle() {
        let (sender, mut receiver) = unbounded_channel();
        let registry =
            TaskRegistry::with_event_publisher(Arc::new(ChannelTaskEventPublisher::new(sender)));

        let created = registry.create(SessionId::from(9)).await;
        let running = registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await;

        assert!(running.is_ok());

        let created_event = receiver.recv().await;
        assert!(matches!(
            created_event,
            Some(event)
                if event.task_id == created.metadata.id
                    && event.sequence == 1
                    && event.kind == TaskEventKind::Created
                    && event.state == TaskState::Pending
        ));

        let running_event = receiver.recv().await;
        assert!(matches!(
            running_event,
            Some(event)
                if event.task_id == created.metadata.id
                    && event.sequence == 2
                    && event.kind == TaskEventKind::StateChanged
                    && event.state == TaskState::Running
        ));
    }

    #[tokio::test]
    async fn task_events_keep_sequences_isolated_per_task() {
        let (sender, mut receiver) = unbounded_channel();
        let registry =
            TaskRegistry::with_event_publisher(Arc::new(ChannelTaskEventPublisher::new(sender)));

        let first = registry.create(SessionId::from(1)).await;
        let second = registry.create(SessionId::from(2)).await;

        let first_running = registry
            .update_state(&first.metadata.id, TaskState::Running)
            .await;
        let second_running = registry
            .update_state(&second.metadata.id, TaskState::Running)
            .await;

        assert!(first_running.is_ok());
        assert!(second_running.is_ok());

        let mut events = Vec::new();
        for _ in 0..4 {
            let event = receiver.recv().await;
            assert!(event.is_some());
            if let Some(event) = event {
                events.push(event);
            }
        }

        let first_sequences = events
            .iter()
            .filter(|event| event.task_id == first.metadata.id)
            .map(|event| event.sequence)
            .collect::<Vec<_>>();
        let second_sequences = events
            .iter()
            .filter(|event| event.task_id == second.metadata.id)
            .map(|event| event.sequence)
            .collect::<Vec<_>>();

        assert_eq!(first_sequences, vec![1, 2]);
        assert_eq!(second_sequences, vec![1, 2]);
    }

    #[tokio::test]
    async fn task_registry_cancellation_transitions_active_task_to_terminal_cancelled() {
        let registry = TaskRegistry::new();
        let created = registry.create(SessionId::from(17)).await;
        let running = registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await;
        assert!(running.is_ok());

        let cancellation = registry.cancel(&created.metadata.id).await;
        assert!(matches!(
            cancellation,
            Ok(TaskCancellation::Cancelled(update))
                if update.record.metadata.state == TaskState::Cancelled && update.event_sequence == 3
        ));

        let stored = registry.get(&created.metadata.id).await;
        assert!(matches!(
            stored,
            Some(record) if record.metadata.state == TaskState::Cancelled
        ));
    }

    #[tokio::test]
    async fn task_registry_cancellation_returns_existing_terminal_state_without_new_event() {
        let (sender, mut receiver) = unbounded_channel();
        let registry =
            TaskRegistry::with_event_publisher(Arc::new(ChannelTaskEventPublisher::new(sender)));
        let created = registry.create(SessionId::from(18)).await;
        let running = registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await;
        assert!(running.is_ok());
        let completed = registry
            .update_state(&created.metadata.id, TaskState::Completed)
            .await;
        assert!(completed.is_ok());

        for _ in 0..3 {
            let event = receiver.recv().await;
            assert!(event.is_some());
        }

        let cancellation = registry.cancel(&created.metadata.id).await;
        assert!(matches!(
            cancellation,
            Ok(TaskCancellation::AlreadyTerminal(record))
                if record.record.metadata.state == TaskState::Completed
        ));
        assert!(timeout(Duration::from_millis(100), receiver.recv())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn task_registry_does_not_cancel_token_for_late_terminal_cancellation() {
        let registry = TaskRegistry::new();
        let created = registry.create(SessionId::from(19)).await;
        let task_id = created.metadata.id;

        let token = registry.get_cancellation_token(&task_id).await;
        assert!(matches!(token, Some(ref token) if !token.is_cancelled()));

        let running = registry.update_state(&task_id, TaskState::Running).await;
        assert!(running.is_ok());
        let completed = registry.update_state(&task_id, TaskState::Completed).await;
        assert!(completed.is_ok());

        let cancellation = registry.cancel(&task_id).await;
        assert!(matches!(
            cancellation,
            Ok(TaskCancellation::AlreadyTerminal(update))
                if update.record.metadata.state == TaskState::Completed
        ));

        let token = registry.get_cancellation_token(&task_id).await;
        assert!(matches!(token, Some(ref token) if !token.is_cancelled()));
    }

    #[tokio::test]
    async fn task_registry_returns_latest_non_terminal_task_for_session() {
        let registry = TaskRegistry::new();
        let session_id = SessionId::from(20);
        let first = registry.create(session_id).await;
        let second = registry.create(session_id).await;
        let third = registry.create(session_id).await;

        let first_cancel = registry.cancel(&first.metadata.id).await;
        assert!(matches!(first_cancel, Ok(TaskCancellation::Cancelled(_))));
        let third_cancel = registry.cancel(&third.metadata.id).await;
        assert!(matches!(third_cancel, Ok(TaskCancellation::Cancelled(_))));

        let latest = registry.latest_non_terminal_by_session(&session_id).await;
        assert!(matches!(latest, Some(record) if record.metadata.id == second.metadata.id));

        let second_cancel = registry.cancel(&second.metadata.id).await;
        assert!(matches!(second_cancel, Ok(TaskCancellation::Cancelled(_))));
        assert!(registry
            .latest_non_terminal_by_session(&session_id)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn task_registry_create_releases_lock_before_publishing() {
        let publish_started = Arc::new(Notify::new());
        let release_publish = Arc::new(Notify::new());
        let registry = Arc::new(TaskRegistry::with_event_publisher(Arc::new(
            BlockingTaskEventPublisher {
                publish_started: Arc::clone(&publish_started),
                release_publish: Arc::clone(&release_publish),
            },
        )));
        let session_id = SessionId::from(321);

        let create_registry = Arc::clone(&registry);
        let create_handle = tokio::spawn(async move { create_registry.create(session_id).await });

        publish_started.notified().await;

        let list_result = timeout(
            Duration::from_millis(200),
            registry.list_by_session(&session_id),
        )
        .await;
        assert!(matches!(list_result, Ok(records) if records.len() == 1));

        release_publish.notify_waiters();

        let create_result = create_handle.await;
        assert!(matches!(create_result, Ok(record) if record.session_id == session_id));
    }
}
