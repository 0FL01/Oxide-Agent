//! In-memory registry for runtime task metadata and cancellation.

use oxide_agent_core::agent::{
    SessionId, TaskId, TaskMetadata, TaskState, TaskStateTransitionError,
};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
struct TaskEntry {
    metadata: TaskMetadata,
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

impl From<TaskEntry> for TaskRecord {
    fn from(value: TaskEntry) -> Self {
        Self {
            metadata: value.metadata,
            session_id: value.session_id,
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
        Self {
            state: RwLock::new(TaskRegistryState::default()),
        }
    }

    /// Create a new pending task for the provided session.
    pub async fn create(&self, session_id: SessionId) -> TaskRecord {
        let metadata = TaskMetadata::new();
        let task_id = metadata.id;
        let entry = TaskEntry {
            metadata,
            session_id,
            cancellation_token: Arc::new(CancellationToken::new()),
        };
        let record = TaskRecord::from(entry.clone());

        let mut state = self.state.write().await;
        state.tasks.insert(task_id, entry);
        state
            .session_tasks
            .entry(session_id)
            .or_default()
            .push(task_id);

        record
    }

    /// Get a task record by task identifier.
    pub async fn get(&self, task_id: &TaskId) -> Option<TaskRecord> {
        let state = self.state.read().await;
        state.tasks.get(task_id).cloned().map(TaskRecord::from)
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

    /// Transition a task to a new lifecycle state.
    pub async fn update_state(
        &self,
        task_id: &TaskId,
        next_state: TaskState,
    ) -> Result<TaskRecord, TaskRegistryError> {
        let mut state = self.state.write().await;
        let entry = state
            .tasks
            .get_mut(task_id)
            .ok_or(TaskRegistryError::TaskNotFound(*task_id))?;
        entry.metadata.transition_to(next_state)?;
        Ok(TaskRecord::from(entry.clone()))
    }

    /// Request cancellation for a task.
    pub async fn cancel(&self, task_id: &TaskId) -> bool {
        let state = self.state.read().await;
        if let Some(token) = state
            .tasks
            .get(task_id)
            .map(|entry| &entry.cancellation_token)
        {
            token.cancel();
            true
        } else {
            false
        }
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

#[cfg(test)]
mod tests {
    use super::{TaskRegistry, TaskRegistryError};
    use oxide_agent_core::agent::{SessionId, TaskState, TaskStateTransitionError};
    use std::sync::Arc;
    use tokio::task::yield_now;

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
        assert!(matches!(running, Ok(record) if record.metadata.state == TaskState::Running));

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
                    assert!(reader_registry.cancel(&task.metadata.id).await);
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

        assert!(registry.cancel(&created.metadata.id).await);

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
            Ok(Ok(record)) if record.metadata.state == TaskState::Running
        ));
        assert!(matches!(
            second_result,
            Ok(Ok(record)) if record.metadata.state == TaskState::Running
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
}
