//! Detached runtime executor for long-running agent tasks.

use crate::{
    task_registry::{TaskRecord, TaskRegistry, TaskRegistryError},
    worker_manager::{WorkerManager, WorkerManagerError},
};
use anyhow::Result;
use async_trait::async_trait;
use oxide_agent_core::agent::{SessionId, TaskId, TaskMetadata, TaskSnapshot, TaskState};
use oxide_agent_core::storage::{StorageError, StorageProvider};
use std::fmt;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::error;

const CREATED_EVENT_SEQUENCE: u64 = 1;
const RUNNING_EVENT_SEQUENCE: u64 = 2;
const TERMINAL_EVENT_SEQUENCE: u64 = 3;

/// Options required to construct a detached task executor.
pub struct TaskExecutorOptions {
    /// Runtime task registry used for task creation and lifecycle updates.
    pub task_registry: Arc<TaskRegistry>,
    /// Runtime-owned worker manager that tracks detached Tokio tasks.
    pub worker_manager: Arc<WorkerManager>,
    /// Persistent task storage used for restart-safe snapshots.
    pub storage: Arc<dyn StorageProvider>,
}

/// Submission payload for a detached task.
#[derive(Clone, Debug)]
pub struct DetachedTaskSubmission {
    /// Owning session for the task.
    pub session_id: SessionId,
    /// Transport-agnostic task input.
    pub task: String,
}

/// Execution request passed into runtime-integrated task backends.
#[derive(Clone, Debug)]
pub struct TaskExecutionRequest {
    /// Stable background task identifier.
    pub task_id: TaskId,
    /// Owning session for this execution.
    pub session_id: SessionId,
    /// Task input to execute.
    pub task: String,
    /// Task-scoped cancellation token owned by the runtime.
    pub cancellation_token: Arc<CancellationToken>,
}

/// Transport-agnostic task execution backend used by the detached executor.
#[async_trait]
pub trait TaskExecutionBackend: Send + Sync + 'static {
    /// Execute the task payload until completion or failure.
    async fn execute(&self, request: TaskExecutionRequest) -> Result<()>;
}

/// Errors returned by detached task executor operations.
#[derive(Debug)]
pub enum TaskExecutorError {
    /// Task registry mutation failed.
    TaskRegistry(TaskRegistryError),
    /// Worker admission or tracking failed.
    WorkerManager(WorkerManagerError),
    /// Task snapshot persistence failed.
    Storage(StorageError),
    /// Task registry did not contain a cancellation token for a created task.
    MissingCancellationToken(TaskId),
}

impl fmt::Display for TaskExecutorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TaskRegistry(error) => write!(f, "{error}"),
            Self::WorkerManager(error) => write!(f, "{error}"),
            Self::Storage(error) => write!(f, "{error}"),
            Self::MissingCancellationToken(task_id) => {
                write!(f, "missing cancellation token for task: {task_id}")
            }
        }
    }
}

impl std::error::Error for TaskExecutorError {}

impl From<TaskRegistryError> for TaskExecutorError {
    fn from(value: TaskRegistryError) -> Self {
        Self::TaskRegistry(value)
    }
}

impl From<WorkerManagerError> for TaskExecutorError {
    fn from(value: WorkerManagerError) -> Self {
        Self::WorkerManager(value)
    }
}

impl From<StorageError> for TaskExecutorError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

/// Runtime-owned detached executor for long-running task execution.
pub struct TaskExecutor {
    task_registry: Arc<TaskRegistry>,
    worker_manager: Arc<WorkerManager>,
    storage: Arc<dyn StorageProvider>,
}

impl TaskExecutor {
    /// Create a detached task executor.
    #[must_use]
    pub fn new(options: TaskExecutorOptions) -> Self {
        Self {
            task_registry: options.task_registry,
            worker_manager: options.worker_manager,
            storage: options.storage,
        }
    }

    /// Create a task record and start detached execution under the runtime worker manager.
    pub async fn submit<B>(
        &self,
        submission: DetachedTaskSubmission,
        backend: Arc<B>,
    ) -> Result<TaskRecord, TaskExecutorError>
    where
        B: TaskExecutionBackend,
    {
        let record = self.task_registry.create(submission.session_id).await;
        let task_id = record.metadata.id;
        let cancellation_token = self
            .task_registry
            .get_cancellation_token(&task_id)
            .await
            .ok_or(TaskExecutorError::MissingCancellationToken(task_id));

        let cancellation_token = match cancellation_token {
            Ok(token) => token,
            Err(error) => {
                self.task_registry.remove(&task_id).await;
                return Err(error);
            }
        };

        if let Err(error) = self
            .persist_snapshot(
                &record.metadata,
                record.session_id,
                &submission.task,
                CREATED_EVENT_SEQUENCE,
            )
            .await
        {
            self.task_registry.remove(&task_id).await;
            return Err(error);
        }

        let run = DetachedTaskRun {
            task_registry: Arc::clone(&self.task_registry),
            storage: Arc::clone(&self.storage),
            task_id,
            session_id: record.session_id,
            task: submission.task.clone(),
            cancellation_token,
        };

        if let Err(error) = self
            .worker_manager
            .spawn(task_id, async move {
                let panic_run = run.clone();
                let join_result = tokio::spawn(async move { run.execute(backend).await }).await;

                match join_result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        error!(task_id = %task_id, error = %error, "Detached task worker failed");
                    }
                    Err(join_error) => {
                        panic_run.persist_panic_failure().await;
                        error!(
                            task_id = %task_id,
                            error = %join_error,
                            "Detached task worker panicked"
                        );
                    }
                }
            })
            .await
        {
            self.persist_pre_start_failure(&record.metadata, record.session_id, &submission.task)
                .await;
            return Err(error.into());
        }

        Ok(record)
    }

    async fn persist_snapshot(
        &self,
        metadata: &TaskMetadata,
        session_id: SessionId,
        task: &str,
        last_event_sequence: u64,
    ) -> Result<(), TaskExecutorError> {
        let snapshot = TaskSnapshot::new(
            metadata.clone(),
            session_id,
            task.to_string(),
            last_event_sequence,
        );
        self.storage.save_task_snapshot(&snapshot).await?;
        Ok(())
    }

    async fn persist_pre_start_failure(
        &self,
        metadata: &TaskMetadata,
        session_id: SessionId,
        task: &str,
    ) {
        let snapshot = pre_start_failure_snapshot(metadata, session_id, task);

        if let Err(error) = self.storage.save_task_snapshot(&snapshot).await {
            error!(
                task_id = %metadata.id,
                error = %error,
                "Failed to persist terminal snapshot after task admission failure"
            );
        }

        self.task_registry.remove(&metadata.id).await;
    }
}

#[derive(Clone)]
struct DetachedTaskRun {
    task_registry: Arc<TaskRegistry>,
    storage: Arc<dyn StorageProvider>,
    task_id: TaskId,
    session_id: SessionId,
    task: String,
    cancellation_token: Arc<CancellationToken>,
}

impl DetachedTaskRun {
    async fn execute<B>(self, backend: Arc<B>) -> Result<(), TaskExecutorError>
    where
        B: TaskExecutionBackend,
    {
        let running_record = self
            .task_registry
            .update_state(&self.task_id, TaskState::Running)
            .await?;

        if let Err(error) = self
            .persist_snapshot(&running_record.metadata, RUNNING_EVENT_SEQUENCE)
            .await
        {
            self.persist_failed_start().await;
            return Err(error);
        }

        let execution_result = backend
            .execute(TaskExecutionRequest {
                task_id: self.task_id,
                session_id: self.session_id,
                task: self.task.clone(),
                cancellation_token: Arc::clone(&self.cancellation_token),
            })
            .await;

        let terminal_state = match execution_result {
            Ok(()) => TaskState::Completed,
            Err(_) if self.cancellation_token.is_cancelled() => TaskState::Cancelled,
            Err(_) => TaskState::Failed,
        };

        let terminal_record = self
            .task_registry
            .update_state(&self.task_id, terminal_state)
            .await?;
        self.persist_snapshot(&terminal_record.metadata, TERMINAL_EVENT_SEQUENCE)
            .await
    }

    async fn persist_snapshot(
        &self,
        metadata: &TaskMetadata,
        last_event_sequence: u64,
    ) -> Result<(), TaskExecutorError> {
        let snapshot = TaskSnapshot::new(
            metadata.clone(),
            self.session_id,
            self.task.clone(),
            last_event_sequence,
        );
        self.storage.save_task_snapshot(&snapshot).await?;
        Ok(())
    }

    async fn persist_failed_start(&self) {
        match self
            .task_registry
            .update_state(&self.task_id, TaskState::Failed)
            .await
        {
            Ok(record) => {
                if let Err(error) = self
                    .persist_snapshot(&record.metadata, TERMINAL_EVENT_SEQUENCE)
                    .await
                {
                    error!(
                        task_id = %self.task_id,
                        error = %error,
                        "Failed to persist terminal snapshot after running checkpoint failure"
                    );
                }
            }
            Err(error) => {
                error!(
                    task_id = %self.task_id,
                    error = %error,
                    "Failed to mark task as failed after running checkpoint failure"
                );
            }
        }
    }

    async fn persist_panic_failure(&self) {
        match self
            .task_registry
            .update_state(&self.task_id, TaskState::Failed)
            .await
        {
            Ok(record) => {
                if let Err(error) = self
                    .persist_snapshot(&record.metadata, TERMINAL_EVENT_SEQUENCE)
                    .await
                {
                    error!(
                        task_id = %self.task_id,
                        error = %error,
                        "Failed to persist terminal snapshot after backend panic"
                    );
                }
            }
            Err(error) => {
                error!(
                    task_id = %self.task_id,
                    error = %error,
                    "Failed to mark task as failed after backend panic"
                );
            }
        }
    }
}

fn failed_metadata(metadata: &TaskMetadata) -> TaskMetadata {
    let mut failed_metadata = metadata.clone();
    failed_metadata.state = TaskState::Failed;
    failed_metadata
}

fn pre_start_failure_snapshot(
    metadata: &TaskMetadata,
    session_id: SessionId,
    task: &str,
) -> TaskSnapshot {
    let failed_metadata = failed_metadata(metadata);
    let mut snapshot = TaskSnapshot::new(
        failed_metadata,
        session_id,
        task.to_string(),
        CREATED_EVENT_SEQUENCE,
    );
    snapshot.metadata.updated_at = snapshot.checkpoint.persisted_at;
    snapshot
}

#[cfg(test)]
mod tests {
    use super::{
        DetachedTaskSubmission, TaskExecutionBackend, TaskExecutionRequest, TaskExecutor,
        TaskExecutorOptions, CREATED_EVENT_SEQUENCE,
    };
    use crate::{TaskExecutorError, TaskRegistry, WorkerManager};
    use anyhow::{anyhow, Result as AnyResult};
    use async_trait::async_trait;
    use oxide_agent_core::agent::{AgentMemory, SessionId, TaskId, TaskSnapshot, TaskState};
    use oxide_agent_core::storage::{Message, StorageError, StorageProvider, UserConfig};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::{Mutex, Notify};
    use tokio::time::{sleep, timeout, Duration};

    #[derive(Clone, Copy)]
    enum BackendOutcome {
        Complete,
        Fail,
        Panic,
    }

    #[derive(Clone)]
    struct ControlledBackend {
        outcome: BackendOutcome,
        started: Arc<Notify>,
        release: Arc<Notify>,
    }

    impl ControlledBackend {
        fn new(outcome: BackendOutcome) -> Self {
            Self {
                outcome,
                started: Arc::new(Notify::new()),
                release: Arc::new(Notify::new()),
            }
        }

        async fn wait_started(&self) {
            let waited = timeout(Duration::from_secs(5), self.started.notified()).await;
            assert!(waited.is_ok(), "backend did not start");
        }

        fn release(&self) {
            self.release.notify_waiters();
        }
    }

    #[async_trait]
    impl TaskExecutionBackend for ControlledBackend {
        async fn execute(&self, request: TaskExecutionRequest) -> AnyResult<()> {
            self.started.notify_waiters();

            if matches!(self.outcome, BackendOutcome::Panic) {
                panic!("simulated backend panic");
            }

            self.release.notified().await;

            if request.cancellation_token.is_cancelled() {
                return Err(anyhow!("task cancelled"));
            }

            match self.outcome {
                BackendOutcome::Complete => Ok(()),
                BackendOutcome::Fail => Err(anyhow!("simulated backend failure")),
                BackendOutcome::Panic => unreachable!("panic outcome returns before release"),
            }
        }
    }

    #[derive(Default)]
    struct TestStorage {
        snapshots: Mutex<HashMap<TaskId, TaskSnapshot>>,
        snapshot_history: Mutex<HashMap<TaskId, Vec<TaskSnapshot>>>,
        fail_save_calls: Mutex<Vec<usize>>,
        save_calls: Mutex<usize>,
    }

    impl TestStorage {
        fn fail_on_save_call(call: usize) -> Self {
            Self {
                snapshots: Mutex::new(HashMap::new()),
                snapshot_history: Mutex::new(HashMap::new()),
                fail_save_calls: Mutex::new(vec![call]),
                save_calls: Mutex::new(0),
            }
        }

        fn fail_on_save_calls(calls: Vec<usize>) -> Self {
            Self {
                snapshots: Mutex::new(HashMap::new()),
                snapshot_history: Mutex::new(HashMap::new()),
                fail_save_calls: Mutex::new(calls),
                save_calls: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl StorageProvider for TestStorage {
        async fn get_user_config(&self, _user_id: i64) -> Result<UserConfig, StorageError> {
            Ok(UserConfig::default())
        }

        async fn update_user_config(
            &self,
            _user_id: i64,
            _config: UserConfig,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn update_user_prompt(
            &self,
            _user_id: i64,
            _prompt: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_prompt(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn update_user_model(
            &self,
            _user_id: i64,
            _model: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_model(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn update_user_state(
            &self,
            _user_id: i64,
            _state: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_state(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn save_message(
            &self,
            _user_id: i64,
            _role: String,
            _content: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_chat_history(
            &self,
            _user_id: i64,
            _limit: usize,
        ) -> Result<Vec<Message>, StorageError> {
            Ok(Vec::new())
        }

        async fn clear_chat_history(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_message_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
            _role: String,
            _content: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_chat_history_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
            _limit: usize,
        ) -> Result<Vec<Message>, StorageError> {
            Ok(Vec::new())
        }

        async fn clear_chat_history_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_agent_memory(
            &self,
            _user_id: i64,
            _memory: &AgentMemory,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn load_agent_memory(
            &self,
            _user_id: i64,
        ) -> Result<Option<AgentMemory>, StorageError> {
            Ok(None)
        }

        async fn clear_agent_memory(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn clear_all_context(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_task_snapshot(&self, snapshot: &TaskSnapshot) -> Result<(), StorageError> {
            let call = {
                let mut save_calls = self.save_calls.lock().await;
                *save_calls += 1;
                *save_calls
            };

            let should_fail = {
                let mut fail_save_calls = self.fail_save_calls.lock().await;
                if let Some(index) = fail_save_calls
                    .iter()
                    .position(|candidate| *candidate == call)
                {
                    fail_save_calls.remove(index);
                    true
                } else {
                    false
                }
            };

            if should_fail {
                return Err(StorageError::Unsupported(format!(
                    "simulated snapshot save failure on call {call}"
                )));
            }

            self.snapshots
                .lock()
                .await
                .insert(snapshot.metadata.id, snapshot.clone());
            self.snapshot_history
                .lock()
                .await
                .entry(snapshot.metadata.id)
                .or_default()
                .push(snapshot.clone());
            Ok(())
        }

        async fn load_task_snapshot(
            &self,
            task_id: TaskId,
        ) -> Result<Option<TaskSnapshot>, StorageError> {
            Ok(self.snapshots.lock().await.get(&task_id).cloned())
        }

        async fn list_task_snapshots(&self) -> Result<Vec<TaskSnapshot>, StorageError> {
            let snapshots = self.snapshots.lock().await;
            let mut values = snapshots.values().cloned().collect::<Vec<_>>();
            values.sort_by(|left, right| {
                left.metadata
                    .created_at
                    .cmp(&right.metadata.created_at)
                    .then_with(|| left.metadata.id.as_uuid().cmp(&right.metadata.id.as_uuid()))
            });
            Ok(values)
        }

        async fn check_connection(&self) -> Result<(), String> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn detached_executor_transitions_to_completed_and_persists_checkpoints() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::default());
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager: Arc::clone(&worker_manager),
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });
        let backend = Arc::new(ControlledBackend::new(BackendOutcome::Complete));

        let record = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(42),
                    task: "rebuild index".to_string(),
                },
                Arc::clone(&backend),
            )
            .await;
        assert!(record.is_ok(), "submit failed: {record:?}");
        let task_id = match record {
            Ok(record) => record.metadata.id,
            Err(error) => panic!("unexpected submit error: {error}"),
        };

        backend.wait_started().await;
        wait_for_state(&registry, task_id, TaskState::Running).await;

        assert!(worker_manager.contains(&task_id).await);

        let running_snapshot = storage.load_task_snapshot(task_id).await;
        assert!(running_snapshot.is_ok(), "failed to load running snapshot");
        let running_snapshot = match running_snapshot {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => panic!("running snapshot missing"),
            Err(error) => panic!("failed to load running snapshot: {error}"),
        };
        assert_eq!(running_snapshot.metadata.state, TaskState::Running);
        assert_eq!(running_snapshot.checkpoint.state, TaskState::Running);

        backend.release();
        wait_for_state(&registry, task_id, TaskState::Completed).await;

        let completed_snapshot = storage.load_task_snapshot(task_id).await;
        assert!(
            completed_snapshot.is_ok(),
            "failed to load completed snapshot"
        );
        let completed_snapshot = match completed_snapshot {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => panic!("completed snapshot missing"),
            Err(error) => panic!("failed to load completed snapshot: {error}"),
        };
        assert_eq!(completed_snapshot.metadata.state, TaskState::Completed);
        assert_eq!(completed_snapshot.checkpoint.state, TaskState::Completed);
    }

    #[tokio::test]
    async fn detached_executor_transitions_to_failed_and_persists_terminal_checkpoint() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::default());
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager: Arc::clone(&worker_manager),
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });
        let backend = Arc::new(ControlledBackend::new(BackendOutcome::Fail));

        let record = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(7),
                    task: "sync backlog".to_string(),
                },
                Arc::clone(&backend),
            )
            .await;
        assert!(record.is_ok(), "submit failed: {record:?}");
        let task_id = match record {
            Ok(record) => record.metadata.id,
            Err(error) => panic!("unexpected submit error: {error}"),
        };

        backend.wait_started().await;
        backend.release();

        wait_for_state(&registry, task_id, TaskState::Failed).await;

        let failed_snapshot = storage.load_task_snapshot(task_id).await;
        assert!(failed_snapshot.is_ok(), "failed to load failed snapshot");
        let failed_snapshot = match failed_snapshot {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => panic!("failed snapshot missing"),
            Err(error) => panic!("failed to load failed snapshot: {error}"),
        };
        assert_eq!(failed_snapshot.metadata.state, TaskState::Failed);
        assert_eq!(failed_snapshot.checkpoint.state, TaskState::Failed);
    }

    #[tokio::test]
    async fn detached_executor_transitions_to_cancelled_when_runtime_token_is_cancelled() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::default());
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager: Arc::clone(&worker_manager),
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });
        let backend = Arc::new(ControlledBackend::new(BackendOutcome::Complete));

        let record = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(99),
                    task: "generate report".to_string(),
                },
                Arc::clone(&backend),
            )
            .await;
        assert!(record.is_ok(), "submit failed: {record:?}");
        let task_id = match record {
            Ok(record) => record.metadata.id,
            Err(error) => panic!("unexpected submit error: {error}"),
        };

        backend.wait_started().await;
        let cancelled = registry.cancel(&task_id).await;
        assert!(cancelled, "task cancellation was not requested");
        backend.release();

        wait_for_state(&registry, task_id, TaskState::Cancelled).await;

        let cancelled_snapshot = storage.load_task_snapshot(task_id).await;
        assert!(
            cancelled_snapshot.is_ok(),
            "failed to load cancelled snapshot"
        );
        let cancelled_snapshot = match cancelled_snapshot {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => panic!("cancelled snapshot missing"),
            Err(error) => panic!("failed to load cancelled snapshot: {error}"),
        };
        assert_eq!(cancelled_snapshot.metadata.state, TaskState::Cancelled);
        assert_eq!(cancelled_snapshot.checkpoint.state, TaskState::Cancelled);
    }

    #[tokio::test]
    async fn detached_executor_removes_pending_task_when_initial_snapshot_persist_fails() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::fail_on_save_call(1));
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager,
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });

        let result = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(5),
                    task: "persist pending snapshot".to_string(),
                },
                Arc::new(ControlledBackend::new(BackendOutcome::Complete)),
            )
            .await;

        assert!(matches!(result, Err(TaskExecutorError::Storage(_))));
        assert!(registry.list().await.is_empty());
        assert!(storage.snapshots.lock().await.is_empty());
    }

    #[tokio::test]
    async fn detached_executor_marks_pre_start_failure_without_pending_orphan() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(0));
        let storage = Arc::new(TestStorage::default());
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager,
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });

        let result = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(6),
                    task: "admission failure".to_string(),
                },
                Arc::new(ControlledBackend::new(BackendOutcome::Complete)),
            )
            .await;

        assert!(matches!(result, Err(TaskExecutorError::WorkerManager(_))));
        assert!(registry.list().await.is_empty());

        let snapshot_history = storage.snapshot_history.lock().await;
        let history = snapshot_history.values().next();
        assert!(matches!(history, Some(history) if history.len() == 2));

        let snapshots = storage.snapshots.lock().await;
        assert_eq!(snapshots.len(), 1);
        let snapshot = snapshots.values().next();
        assert!(matches!(snapshot, Some(snapshot) if snapshot.metadata.state == TaskState::Failed));
        assert!(
            matches!(snapshot, Some(snapshot) if snapshot.checkpoint.state == TaskState::Failed)
        );
        assert!(matches!(
            snapshot,
            Some(snapshot) if snapshot.checkpoint.last_event_sequence == CREATED_EVENT_SEQUENCE
        ));
        assert!(matches!(
            snapshot,
            Some(snapshot) if snapshot.metadata.updated_at == snapshot.checkpoint.persisted_at
        ));
        assert!(matches!(
            history,
            Some(history)
                if history[1].metadata.updated_at > history[0].metadata.updated_at
        ));
    }

    #[tokio::test]
    async fn detached_executor_maps_backend_panic_to_failed_terminal_snapshot() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::default());
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager: Arc::clone(&worker_manager),
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });
        let backend = Arc::new(ControlledBackend::new(BackendOutcome::Panic));

        let record = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(8),
                    task: "panic backend".to_string(),
                },
                backend,
            )
            .await;
        assert!(record.is_ok(), "submit failed: {record:?}");
        let task_id = match record {
            Ok(record) => record.metadata.id,
            Err(error) => panic!("unexpected submit error: {error}"),
        };

        wait_for_state(&registry, task_id, TaskState::Failed).await;
        assert!(!worker_manager.contains(&task_id).await);

        let failed_snapshot = storage.load_task_snapshot(task_id).await;
        assert!(failed_snapshot.is_ok(), "failed to load failed snapshot");
        let failed_snapshot = match failed_snapshot {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => panic!("failed snapshot missing"),
            Err(error) => panic!("failed to load failed snapshot: {error}"),
        };
        assert_eq!(failed_snapshot.metadata.state, TaskState::Failed);
        assert_eq!(failed_snapshot.checkpoint.state, TaskState::Failed);
    }

    #[tokio::test]
    async fn detached_executor_removes_pending_task_when_compensation_snapshot_fails() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(0));
        let storage = Arc::new(TestStorage::fail_on_save_calls(vec![2]));
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager,
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });

        let result = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(11),
                    task: "compensation failure".to_string(),
                },
                Arc::new(ControlledBackend::new(BackendOutcome::Complete)),
            )
            .await;

        assert!(matches!(result, Err(TaskExecutorError::WorkerManager(_))));
        assert!(registry.list().await.is_empty());

        let snapshots = storage.snapshots.lock().await;
        assert_eq!(snapshots.len(), 1);
        let snapshot = snapshots.values().next();
        assert!(
            matches!(snapshot, Some(snapshot) if snapshot.metadata.state == TaskState::Pending)
        );
    }

    async fn wait_for_state(registry: &TaskRegistry, task_id: TaskId, expected: TaskState) {
        let waited = timeout(Duration::from_secs(5), async {
            loop {
                let record = registry.get(&task_id).await;
                if let Some(record) = record {
                    if record.metadata.state == expected {
                        break;
                    }
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        assert!(waited.is_ok(), "task did not reach {expected:?}");
    }
}
