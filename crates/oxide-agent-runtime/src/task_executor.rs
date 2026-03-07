//! Detached runtime executor for long-running agent tasks.

use crate::{
    task_registry::{TaskCancellation, TaskRecord, TaskRegistry, TaskRegistryError},
    worker_manager::{WorkerManager, WorkerManagerError},
};
use anyhow::Result;
use async_trait::async_trait;
use oxide_agent_core::agent::{
    PendingInput, PendingInputValidationError, SessionId, TaskEvent, TaskEventKind, TaskId,
    TaskMetadata, TaskSnapshot, TaskState, TaskStateTransitionError,
};
use oxide_agent_core::storage::{StorageError, StorageProvider};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio_util::sync::CancellationToken;
use tracing::error;

const CREATED_EVENT_SEQUENCE: u64 = 1;

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
    /// Optional resume payload provided after waiting for external input.
    pub resume_input: Option<String>,
    /// Task-scoped cancellation token owned by the runtime.
    pub cancellation_token: Arc<CancellationToken>,
}

/// Transport-agnostic task execution backend used by the detached executor.
#[async_trait]
pub trait TaskExecutionBackend: Send + Sync + 'static {
    /// Execute the task payload until completion or failure.
    async fn execute(&self, request: TaskExecutionRequest) -> Result<TaskExecutionOutcome>;
}

/// Runtime-visible execution outcome returned by task backends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskExecutionOutcome {
    /// Backend completed task work without requiring external input.
    Completed,
    /// Backend paused task execution and requires a pending HITL response.
    WaitingInput(PendingInput),
}

/// Errors returned by detached task executor operations.
#[derive(Debug)]
pub enum TaskExecutorError {
    /// Another live task already owns the same session.
    SessionTaskAlreadyRunning(SessionId),
    /// Task registry mutation failed.
    TaskRegistry(TaskRegistryError),
    /// Worker admission or tracking failed.
    WorkerManager(WorkerManagerError),
    /// Task snapshot persistence failed.
    Storage(StorageError),
    /// Task registry did not contain a cancellation token for a created task.
    MissingCancellationToken(TaskId),
    /// Task snapshot for a runtime-owned task was missing during persistence.
    MissingTaskSnapshot(TaskId),
    /// Backend requested waiting-input state with an invalid payload.
    InvalidPendingInput(PendingInputValidationError),
    /// Resume was requested for a task that is not waiting for input.
    ResumeInvalidState {
        /// Task identifier that rejected resume.
        task_id: TaskId,
        /// Current lifecycle state that cannot be resumed.
        state: TaskState,
    },
    /// Resume was requested for a waiting task without a pending payload.
    MissingPendingInput(TaskId),
}

impl fmt::Display for TaskExecutorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionTaskAlreadyRunning(session_id) => {
                write!(f, "task already running for session: {session_id}")
            }
            Self::TaskRegistry(error) => write!(f, "{error}"),
            Self::WorkerManager(error) => write!(f, "{error}"),
            Self::Storage(error) => write!(f, "{error}"),
            Self::MissingCancellationToken(task_id) => {
                write!(f, "missing cancellation token for task: {task_id}")
            }
            Self::MissingTaskSnapshot(task_id) => {
                write!(f, "missing task snapshot for task: {task_id}")
            }
            Self::InvalidPendingInput(error) => write!(f, "{error}"),
            Self::ResumeInvalidState { task_id, state } => {
                write!(f, "task {task_id} cannot resume from state {state:?}")
            }
            Self::MissingPendingInput(task_id) => {
                write!(f, "missing pending input for waiting task: {task_id}")
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
    session_gate: SessionActionGate,
    task_registry: Arc<TaskRegistry>,
    worker_manager: Arc<WorkerManager>,
    storage: Arc<dyn StorageProvider>,
}

struct SessionActionGate {
    locks: Mutex<HashMap<SessionId, Arc<Mutex<()>>>>,
}

impl SessionActionGate {
    fn new() -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
        }
    }

    async fn lock(&self, session_id: SessionId) -> OwnedMutexGuard<()> {
        let session_lock = {
            let mut locks = self.locks.lock().await;
            Arc::clone(
                locks
                    .entry(session_id)
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };

        session_lock.lock_owned().await
    }
}

impl TaskExecutor {
    /// Create a detached task executor.
    #[must_use]
    pub fn new(options: TaskExecutorOptions) -> Self {
        Self {
            session_gate: SessionActionGate::new(),
            task_registry: options.task_registry,
            worker_manager: options.worker_manager,
            storage: options.storage,
        }
    }

    /// Serialize all runtime admission and destructive session actions for a session.
    pub async fn with_session_gate<F, Fut, T>(&self, session_id: SessionId, action: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let _session_guard = self.session_gate.lock(session_id).await;
        action().await
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
        let session_id = submission.session_id;
        self.with_session_gate(session_id, || async move {
            self.submit_with_session_gate_held(submission, backend)
                .await
        })
        .await
    }

    /// Submit a task while the caller already holds the session gate.
    pub async fn submit_with_session_gate_held<B>(
        &self,
        submission: DetachedTaskSubmission,
        backend: Arc<B>,
    ) -> Result<TaskRecord, TaskExecutorError>
    where
        B: TaskExecutionBackend,
    {
        let session_id = submission.session_id;

        if self.has_active_task_for_session(session_id).await {
            return Err(TaskExecutorError::SessionTaskAlreadyRunning(session_id));
        }

        let record = self.task_registry.create(session_id).await;
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
            .persist_checkpoint(
                &record.metadata,
                record.session_id,
                &submission.task,
                CREATED_EVENT_SEQUENCE,
                TaskEventKind::Created,
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
            resume_input: None,
            skip_running_checkpoint: false,
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

    async fn has_active_task_for_session(&self, session_id: SessionId) -> bool {
        self.task_registry
            .latest_non_terminal_by_session(&session_id)
            .await
            .is_some()
    }

    /// Cancel a runtime-owned task by task identifier.
    pub async fn cancel_task(&self, task_id: &TaskId) -> Result<TaskRecord, TaskExecutorError> {
        match self.task_registry.cancel(task_id).await? {
            TaskCancellation::Cancelled(update) => {
                self.persist_cancelled_snapshot(&update.record, update.event_sequence, true)
                    .await?;
                Ok(update.record)
            }
            TaskCancellation::AlreadyTerminal(update) => {
                if update.record.metadata.state == TaskState::Cancelled {
                    self.persist_cancelled_snapshot(&update.record, update.event_sequence, false)
                        .await?;
                }
                Ok(update.record)
            }
        }
    }

    /// Resume a waiting task by transitioning it back to running and restarting detached execution.
    pub async fn resume_task<B>(
        &self,
        task_id: &TaskId,
        input: String,
        backend: Arc<B>,
    ) -> Result<TaskRecord, TaskExecutorError>
    where
        B: TaskExecutionBackend,
    {
        let Some(record) = self.task_registry.get(task_id).await else {
            return Err(TaskExecutorError::TaskRegistry(
                TaskRegistryError::TaskNotFound(*task_id),
            ));
        };

        self.with_session_gate(record.session_id, || async move {
            self.resume_task_with_session_gate_held(task_id, input, backend)
                .await
        })
        .await
    }

    /// Resume a waiting task while the caller already holds the session gate.
    pub async fn resume_task_with_session_gate_held<B>(
        &self,
        task_id: &TaskId,
        input: String,
        backend: Arc<B>,
    ) -> Result<TaskRecord, TaskExecutorError>
    where
        B: TaskExecutionBackend,
    {
        let task_id_value = *task_id;
        let Some(record) = self.task_registry.get(task_id).await else {
            return Err(TaskExecutorError::TaskRegistry(
                TaskRegistryError::TaskNotFound(task_id_value),
            ));
        };

        if record.metadata.state != TaskState::WaitingInput {
            return Err(TaskExecutorError::ResumeInvalidState {
                task_id: task_id_value,
                state: record.metadata.state,
            });
        }

        if record.pending_input.is_none() {
            return Err(TaskExecutorError::MissingPendingInput(task_id_value));
        }

        let task = self.load_task_payload(task_id_value).await?;
        let running_update = self
            .task_registry
            .update_state(task_id, TaskState::Running)
            .await?;
        self.save_snapshot(
            &running_update.record.metadata,
            running_update.record.session_id,
            &task,
            running_update.event_sequence,
            None,
        )
        .await?;

        let cancellation_token = self
            .task_registry
            .get_cancellation_token(task_id)
            .await
            .ok_or(TaskExecutorError::MissingCancellationToken(task_id_value))?;

        let run = DetachedTaskRun {
            task_registry: Arc::clone(&self.task_registry),
            storage: Arc::clone(&self.storage),
            task_id: task_id_value,
            session_id: running_update.record.session_id,
            task: task.clone(),
            cancellation_token,
            resume_input: Some(input),
            skip_running_checkpoint: true,
        };

        if let Err(error) = self
            .worker_manager
            .spawn(task_id_value, async move {
                let panic_run = run.clone();
                let join_result = tokio::spawn(async move { run.execute(backend).await }).await;

                match join_result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        error!(task_id = %task_id_value, error = %error, "Detached task worker failed");
                    }
                    Err(join_error) => {
                        panic_run.persist_panic_failure().await;
                        error!(
                            task_id = %task_id_value,
                            error = %join_error,
                            "Detached task worker panicked"
                        );
                    }
                }
            })
            .await
        {
            if let Ok(failed_update) = self
                .task_registry
                .update_state(task_id, TaskState::Failed)
                .await
            {
                let _ = self
                    .save_snapshot(
                        &failed_update.record.metadata,
                        failed_update.record.session_id,
                        &task,
                        failed_update.event_sequence,
                        None,
                    )
                    .await;
            }
            return Err(error.into());
        }

        Ok(running_update.record)
    }

    async fn persist_cancelled_snapshot(
        &self,
        record: &TaskRecord,
        last_event_sequence: u64,
        append_event: bool,
    ) -> Result<(), TaskExecutorError> {
        let task = self.load_task_payload(record.metadata.id).await?;
        if append_event {
            self.append_task_event(
                record.metadata.id,
                last_event_sequence,
                TaskEventKind::StateChanged,
                record.metadata.state,
            )
            .await?;
        }

        self.save_snapshot(
            &record.metadata,
            record.session_id,
            &task,
            last_event_sequence,
            None,
        )
        .await
    }

    async fn persist_checkpoint(
        &self,
        metadata: &TaskMetadata,
        session_id: SessionId,
        task: &str,
        last_event_sequence: u64,
        event_kind: TaskEventKind,
    ) -> Result<(), TaskExecutorError> {
        self.append_task_event(metadata.id, last_event_sequence, event_kind, metadata.state)
            .await?;

        self.save_snapshot(metadata, session_id, task, last_event_sequence, None)
            .await
    }

    async fn save_snapshot(
        &self,
        metadata: &TaskMetadata,
        session_id: SessionId,
        task: &str,
        last_event_sequence: u64,
        pending_input: Option<PendingInput>,
    ) -> Result<(), TaskExecutorError> {
        let mut snapshot = TaskSnapshot::new(
            metadata.clone(),
            session_id,
            task.to_string(),
            last_event_sequence,
        );
        snapshot.pending_input = pending_input;
        self.storage.save_task_snapshot(&snapshot).await?;
        Ok(())
    }

    async fn load_task_payload(&self, task_id: TaskId) -> Result<String, TaskExecutorError> {
        let snapshot = self
            .storage
            .load_task_snapshot(task_id)
            .await?
            .ok_or(TaskExecutorError::MissingTaskSnapshot(task_id))?;
        Ok(snapshot.task)
    }

    async fn append_task_event(
        &self,
        task_id: TaskId,
        sequence: u64,
        kind: TaskEventKind,
        state: TaskState,
    ) -> Result<(), TaskExecutorError> {
        self.storage
            .append_task_event(
                task_id,
                TaskEvent::new(task_id, sequence, kind, state, None),
            )
            .await?;
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
    resume_input: Option<String>,
    skip_running_checkpoint: bool,
}

impl DetachedTaskRun {
    async fn execute<B>(self, backend: Arc<B>) -> Result<(), TaskExecutorError>
    where
        B: TaskExecutionBackend,
    {
        if !self.skip_running_checkpoint {
            let running_update = match self
                .task_registry
                .update_state(&self.task_id, TaskState::Running)
                .await
            {
                Ok(update) => update,
                Err(error) if cancellation_already_won(&error) => return Ok(()),
                Err(error) => return Err(error.into()),
            };

            if let Err(error) = self
                .persist_snapshot(
                    &running_update.record.metadata,
                    running_update.event_sequence,
                    None,
                )
                .await
            {
                self.persist_failed_start().await;
                return Err(error);
            }
        }

        let execution_result = backend
            .execute(TaskExecutionRequest {
                task_id: self.task_id,
                session_id: self.session_id,
                task: self.task.clone(),
                resume_input: self.resume_input.clone(),
                cancellation_token: Arc::clone(&self.cancellation_token),
            })
            .await;

        if self.cancellation_token.is_cancelled() {
            return self.persist_terminal_state(TaskState::Cancelled).await;
        }

        match execution_result {
            Ok(TaskExecutionOutcome::Completed) => {
                self.persist_terminal_state(TaskState::Completed).await
            }
            Ok(TaskExecutionOutcome::WaitingInput(pending_input)) => {
                let waiting_result = self.persist_waiting_input(pending_input).await;
                if let Err(error) = waiting_result {
                    self.persist_terminal_state(TaskState::Failed).await?;
                    return Err(error);
                }

                Ok(())
            }
            Err(_) => self.persist_terminal_state(TaskState::Failed).await,
        }
    }

    async fn persist_snapshot(
        &self,
        metadata: &TaskMetadata,
        last_event_sequence: u64,
        pending_input: Option<PendingInput>,
    ) -> Result<(), TaskExecutorError> {
        self.append_task_event(
            metadata.id,
            last_event_sequence,
            TaskEventKind::StateChanged,
            metadata.state,
        )
        .await?;

        self.save_snapshot(last_event_sequence, metadata, pending_input)
            .await
    }

    async fn append_task_event(
        &self,
        task_id: TaskId,
        sequence: u64,
        kind: TaskEventKind,
        state: TaskState,
    ) -> Result<(), TaskExecutorError> {
        self.storage
            .append_task_event(
                task_id,
                TaskEvent::new(task_id, sequence, kind, state, None),
            )
            .await?;
        Ok(())
    }

    async fn save_snapshot(
        &self,
        last_event_sequence: u64,
        metadata: &TaskMetadata,
        pending_input: Option<PendingInput>,
    ) -> Result<(), TaskExecutorError> {
        let mut snapshot = TaskSnapshot::new(
            metadata.clone(),
            self.session_id,
            self.task.clone(),
            last_event_sequence,
        );
        snapshot.pending_input = pending_input;
        self.storage.save_task_snapshot(&snapshot).await?;
        Ok(())
    }

    async fn persist_waiting_input(
        &self,
        pending_input: PendingInput,
    ) -> Result<(), TaskExecutorError> {
        pending_input
            .validate()
            .map_err(TaskExecutorError::InvalidPendingInput)?;

        let waiting_update = self
            .task_registry
            .enter_waiting_input(&self.task_id, pending_input)
            .await?;
        self.persist_snapshot(
            &waiting_update.record.metadata,
            waiting_update.event_sequence,
            waiting_update.record.pending_input,
        )
        .await
    }

    async fn persist_failed_start(&self) {
        match self
            .task_registry
            .update_state(&self.task_id, TaskState::Failed)
            .await
        {
            Ok(update) => {
                if let Err(error) = self
                    .persist_snapshot(&update.record.metadata, update.event_sequence, None)
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
        if let Err(error) = self.persist_terminal_state(TaskState::Failed).await {
            error!(
                task_id = %self.task_id,
                error = %error,
                "Failed to persist terminal snapshot after backend panic"
            );
        }
    }

    async fn persist_terminal_state(
        &self,
        terminal_state: TaskState,
    ) -> Result<(), TaskExecutorError> {
        match self
            .task_registry
            .update_state(&self.task_id, terminal_state)
            .await
        {
            Ok(update) => {
                self.persist_snapshot(&update.record.metadata, update.event_sequence, None)
                    .await
            }
            Err(error)
                if terminal_transition_is_already_committed(&error)
                    && terminal_state == TaskState::Cancelled =>
            {
                self.persist_committed_cancelled_state().await
            }
            Err(error) if terminal_transition_is_already_committed(&error) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    async fn persist_committed_cancelled_state(&self) -> Result<(), TaskExecutorError> {
        let Some(update) = self.task_registry.get_update(&self.task_id).await else {
            return Err(TaskExecutorError::MissingTaskSnapshot(self.task_id));
        };

        if update.record.metadata.state != TaskState::Cancelled {
            return Ok(());
        }

        self.save_snapshot(update.event_sequence, &update.record.metadata, None)
            .await
    }
}

fn cancellation_already_won(error: &TaskRegistryError) -> bool {
    matches!(
        error,
        TaskRegistryError::InvalidStateTransition(TaskStateTransitionError::InvalidTransition {
            from: TaskState::Cancelled,
            to: TaskState::Running,
        })
    )
}

fn terminal_transition_is_already_committed(error: &TaskRegistryError) -> bool {
    matches!(
        error,
        TaskRegistryError::InvalidStateTransition(TaskStateTransitionError::InvalidTransition {
            from,
            ..
        }) if from.is_terminal()
    )
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
        DetachedTaskSubmission, TaskExecutionBackend, TaskExecutionOutcome, TaskExecutionRequest,
        TaskExecutor, TaskExecutorOptions, CREATED_EVENT_SEQUENCE,
    };
    use crate::{TaskExecutorError, TaskRegistry, WorkerManager};
    use anyhow::{anyhow, Result as AnyResult};
    use async_trait::async_trait;
    use oxide_agent_core::agent::{
        AgentMemory, PendingInput, PendingInputKind, PendingTextInput, SessionId, TaskEvent,
        TaskId, TaskSnapshot, TaskState,
    };
    use oxide_agent_core::storage::{Message, StorageError, StorageProvider, UserConfig};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::{Barrier, Mutex, Notify};
    use tokio::time::{sleep, timeout, Duration};

    #[derive(Clone)]
    enum BackendOutcome {
        Complete,
        Fail,
        Panic,
        WaitingInput,
    }

    #[derive(Clone)]
    struct ControlledBackend {
        outcome: BackendOutcome,
        pending_input: Option<PendingInput>,
        started: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[derive(Clone)]
    struct ResumeBackend {
        expected_resume_input: String,
    }

    impl ControlledBackend {
        fn new(outcome: BackendOutcome) -> Self {
            Self {
                outcome,
                pending_input: None,
                started: Arc::new(Notify::new()),
                release: Arc::new(Notify::new()),
            }
        }

        fn waiting_input(pending_input: PendingInput) -> Self {
            Self {
                outcome: BackendOutcome::WaitingInput,
                pending_input: Some(pending_input),
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
        async fn execute(&self, request: TaskExecutionRequest) -> AnyResult<TaskExecutionOutcome> {
            self.started.notify_waiters();

            if matches!(self.outcome, BackendOutcome::Panic) {
                panic!("simulated backend panic");
            }

            self.release.notified().await;

            if request.cancellation_token.is_cancelled() {
                return Err(anyhow!("task cancelled"));
            }

            match self.outcome {
                BackendOutcome::Complete => Ok(TaskExecutionOutcome::Completed),
                BackendOutcome::Fail => Err(anyhow!("simulated backend failure")),
                BackendOutcome::Panic => unreachable!("panic outcome returns before release"),
                BackendOutcome::WaitingInput => {
                    if let Some(pending_input) = self.pending_input.clone() {
                        Ok(TaskExecutionOutcome::WaitingInput(pending_input))
                    } else {
                        Err(anyhow!("missing waiting-input payload"))
                    }
                }
            }
        }
    }

    #[async_trait]
    impl TaskExecutionBackend for ResumeBackend {
        async fn execute(&self, request: TaskExecutionRequest) -> AnyResult<TaskExecutionOutcome> {
            let Some(resume_input) = request.resume_input.as_deref() else {
                return Err(anyhow!("missing resume input"));
            };
            if resume_input != self.expected_resume_input {
                return Err(anyhow!("unexpected resume input payload"));
            }
            Ok(TaskExecutionOutcome::Completed)
        }
    }

    #[derive(Default)]
    struct TestStorage {
        snapshots: Mutex<HashMap<TaskId, TaskSnapshot>>,
        snapshot_history: Mutex<HashMap<TaskId, Vec<TaskSnapshot>>>,
        events: Mutex<HashMap<TaskId, Vec<TaskEvent>>>,
        fail_save_calls: Mutex<Vec<usize>>,
        fail_save_states: Mutex<Vec<TaskState>>,
        save_calls: Mutex<usize>,
    }

    impl TestStorage {
        fn fail_on_save_call(call: usize) -> Self {
            Self {
                snapshots: Mutex::new(HashMap::new()),
                snapshot_history: Mutex::new(HashMap::new()),
                events: Mutex::new(HashMap::new()),
                fail_save_calls: Mutex::new(vec![call]),
                fail_save_states: Mutex::new(Vec::new()),
                save_calls: Mutex::new(0),
            }
        }

        fn fail_on_save_calls(calls: Vec<usize>) -> Self {
            Self {
                snapshots: Mutex::new(HashMap::new()),
                snapshot_history: Mutex::new(HashMap::new()),
                events: Mutex::new(HashMap::new()),
                fail_save_calls: Mutex::new(calls),
                fail_save_states: Mutex::new(Vec::new()),
                save_calls: Mutex::new(0),
            }
        }

        fn fail_on_save_state(state: TaskState) -> Self {
            Self {
                snapshots: Mutex::new(HashMap::new()),
                snapshot_history: Mutex::new(HashMap::new()),
                events: Mutex::new(HashMap::new()),
                fail_save_calls: Mutex::new(Vec::new()),
                fail_save_states: Mutex::new(vec![state]),
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

            let should_fail_call = {
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

            let should_fail_state = {
                let mut fail_save_states = self.fail_save_states.lock().await;
                if let Some(index) = fail_save_states
                    .iter()
                    .position(|candidate| *candidate == snapshot.metadata.state)
                {
                    fail_save_states.remove(index);
                    true
                } else {
                    false
                }
            };

            if should_fail_call || should_fail_state {
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

        async fn append_task_event(
            &self,
            task_id: TaskId,
            event: TaskEvent,
        ) -> Result<(), StorageError> {
            self.events
                .lock()
                .await
                .entry(task_id)
                .or_default()
                .push(event);
            Ok(())
        }

        async fn load_task_events(&self, task_id: TaskId) -> Result<Vec<TaskEvent>, StorageError> {
            Ok(self
                .events
                .lock()
                .await
                .get(&task_id)
                .cloned()
                .unwrap_or_default())
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
        let cancelled = executor.cancel_task(&task_id).await;
        assert!(matches!(
            cancelled,
            Ok(record) if record.metadata.state == TaskState::Cancelled
        ));
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
    async fn detached_executor_transitions_to_waiting_input_and_persists_pending_payload() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::default());
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager: Arc::clone(&worker_manager),
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });
        let pending_input = sample_pending_input();
        let backend = Arc::new(ControlledBackend::waiting_input(pending_input.clone()));

        let submitted = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(110),
                    task: "request approval".to_string(),
                },
                Arc::clone(&backend),
            )
            .await;
        assert!(submitted.is_ok(), "submit failed: {submitted:?}");
        let task_id = match submitted {
            Ok(record) => record.metadata.id,
            Err(error) => panic!("unexpected submit error: {error}"),
        };

        backend.wait_started().await;
        backend.release();

        wait_for_state(&registry, task_id, TaskState::WaitingInput).await;

        let waiting_record = registry.get(&task_id).await;
        assert!(matches!(
            waiting_record,
            Some(record)
                if record.metadata.state == TaskState::WaitingInput
                    && record.pending_input == Some(pending_input.clone())
        ));

        let snapshot = storage.load_task_snapshot(task_id).await;
        assert!(matches!(
            snapshot,
            Ok(Some(snapshot))
                if snapshot.metadata.state == TaskState::WaitingInput
                    && snapshot.checkpoint.state == TaskState::WaitingInput
                    && snapshot.pending_input == Some(pending_input)
        ));

        assert!(!worker_manager.contains(&task_id).await);
    }

    #[tokio::test]
    async fn detached_executor_rejects_new_submit_when_previous_task_waits_for_input() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::default());
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager: Arc::clone(&worker_manager),
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });
        let session_id = SessionId::from(111);

        let waiting_backend = Arc::new(ControlledBackend::waiting_input(sample_pending_input()));
        let first = executor
            .submit(
                DetachedTaskSubmission {
                    session_id,
                    task: "initial request".to_string(),
                },
                Arc::clone(&waiting_backend),
            )
            .await;
        assert!(first.is_ok(), "first submit failed: {first:?}");
        let first_task_id = match first {
            Ok(record) => record.metadata.id,
            Err(error) => panic!("unexpected submit error: {error}"),
        };

        waiting_backend.wait_started().await;
        waiting_backend.release();
        wait_for_state(&registry, first_task_id, TaskState::WaitingInput).await;

        let second = executor
            .submit(
                DetachedTaskSubmission {
                    session_id,
                    task: "should be blocked".to_string(),
                },
                Arc::new(ControlledBackend::new(BackendOutcome::Complete)),
            )
            .await;

        assert!(matches!(
            second,
            Err(TaskExecutorError::SessionTaskAlreadyRunning(rejected_session_id))
                if rejected_session_id == session_id
        ));
    }

    #[tokio::test]
    async fn hitl_resume_restarts_waiting_task_and_clears_pending_input() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::default());
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager: Arc::clone(&worker_manager),
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });
        let waiting_backend = Arc::new(ControlledBackend::waiting_input(sample_pending_input()));

        let submitted = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(113),
                    task: "hitl resume".to_string(),
                },
                Arc::clone(&waiting_backend),
            )
            .await;
        assert!(submitted.is_ok(), "submit failed: {submitted:?}");
        let task_id = match submitted {
            Ok(record) => record.metadata.id,
            Err(error) => panic!("unexpected submit error: {error}"),
        };

        waiting_backend.wait_started().await;
        waiting_backend.release();
        wait_for_state(&registry, task_id, TaskState::WaitingInput).await;

        let resumed = executor
            .resume_task(
                &task_id,
                "1,2".to_string(),
                Arc::new(ResumeBackend {
                    expected_resume_input: "1,2".to_string(),
                }),
            )
            .await;
        assert!(matches!(
            resumed,
            Ok(record)
                if record.metadata.state == TaskState::Running && record.pending_input.is_none()
        ));

        wait_for_state(&registry, task_id, TaskState::Completed).await;

        let snapshot = storage.load_task_snapshot(task_id).await;
        assert!(matches!(
            snapshot,
            Ok(Some(snapshot))
                if snapshot.metadata.state == TaskState::Completed
                    && snapshot.pending_input.is_none()
        ));
        assert!(!worker_manager.contains(&task_id).await);
    }

    #[tokio::test]
    async fn hitl_resume_rejects_duplicate_resume_requests() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::default());
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager: Arc::clone(&worker_manager),
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });
        let waiting_backend = Arc::new(ControlledBackend::waiting_input(sample_pending_input()));

        let submitted = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(114),
                    task: "duplicate resume".to_string(),
                },
                Arc::clone(&waiting_backend),
            )
            .await;
        assert!(submitted.is_ok(), "submit failed: {submitted:?}");
        let task_id = match submitted {
            Ok(record) => record.metadata.id,
            Err(error) => panic!("unexpected submit error: {error}"),
        };

        waiting_backend.wait_started().await;
        waiting_backend.release();
        wait_for_state(&registry, task_id, TaskState::WaitingInput).await;

        let running_backend = Arc::new(ControlledBackend::new(BackendOutcome::Complete));
        let first_resume = executor
            .resume_task(&task_id, "0".to_string(), Arc::clone(&running_backend))
            .await;
        assert!(matches!(
            first_resume,
            Ok(record)
                if record.metadata.state == TaskState::Running && record.pending_input.is_none()
        ));

        running_backend.wait_started().await;

        let duplicate_resume = executor
            .resume_task(&task_id, "0".to_string(), Arc::clone(&running_backend))
            .await;
        assert!(matches!(
            duplicate_resume,
            Err(TaskExecutorError::ResumeInvalidState {
                task_id: rejected_task_id,
                state: TaskState::Running,
            }) if rejected_task_id == task_id
        ));

        running_backend.release();
        wait_for_state(&registry, task_id, TaskState::Completed).await;
    }

    #[tokio::test]
    async fn hitl_resume_rejects_terminal_task_state() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::default());
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager: Arc::clone(&worker_manager),
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });
        let backend = Arc::new(ControlledBackend::new(BackendOutcome::Complete));

        let submitted = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(115),
                    task: "terminal resume".to_string(),
                },
                Arc::clone(&backend),
            )
            .await;
        assert!(submitted.is_ok(), "submit failed: {submitted:?}");
        let task_id = match submitted {
            Ok(record) => record.metadata.id,
            Err(error) => panic!("unexpected submit error: {error}"),
        };

        backend.wait_started().await;
        backend.release();
        wait_for_state(&registry, task_id, TaskState::Completed).await;

        let resume = executor
            .resume_task(
                &task_id,
                "ignored".to_string(),
                Arc::new(ResumeBackend {
                    expected_resume_input: "ignored".to_string(),
                }),
            )
            .await;
        assert!(matches!(
            resume,
            Err(TaskExecutorError::ResumeInvalidState {
                task_id: rejected_task_id,
                state: TaskState::Completed,
            }) if rejected_task_id == task_id
        ));
    }

    #[tokio::test]
    async fn detached_executor_rejects_invalid_pending_input_without_entering_waiting_state() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::default());
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager: Arc::clone(&worker_manager),
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });
        let backend = Arc::new(ControlledBackend::waiting_input(invalid_pending_input()));

        let submitted = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(112),
                    task: "invalid waiting payload".to_string(),
                },
                Arc::clone(&backend),
            )
            .await;
        assert!(submitted.is_ok(), "submit failed: {submitted:?}");
        let task_id = match submitted {
            Ok(record) => record.metadata.id,
            Err(error) => panic!("unexpected submit error: {error}"),
        };

        backend.wait_started().await;
        backend.release();

        wait_for_worker_completion(&worker_manager, task_id).await;
        wait_for_state(&registry, task_id, TaskState::Failed).await;

        let record = registry.get(&task_id).await;
        assert!(matches!(
            record,
            Some(record) if record.metadata.state == TaskState::Failed && record.pending_input.is_none()
        ));

        let snapshot = storage.load_task_snapshot(task_id).await;
        assert!(matches!(
            snapshot,
            Ok(Some(snapshot))
                if snapshot.metadata.state == TaskState::Failed
                    && snapshot.pending_input.is_none()
        ));
    }

    #[tokio::test]
    async fn cancellation_persists_terminal_snapshot_when_requested_before_worker_starts() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::default());
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager,
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });

        let record = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(101),
                    task: "cancel before start".to_string(),
                },
                Arc::new(ControlledBackend::new(BackendOutcome::Complete)),
            )
            .await;
        assert!(record.is_ok(), "submit failed: {record:?}");
        let task_id = match record {
            Ok(record) => record.metadata.id,
            Err(error) => panic!("unexpected submit error: {error}"),
        };

        let cancelled = executor.cancel_task(&task_id).await;
        assert!(matches!(
            cancelled,
            Ok(record) if record.metadata.state == TaskState::Cancelled
        ));

        wait_for_state(&registry, task_id, TaskState::Cancelled).await;

        let snapshot = storage.load_task_snapshot(task_id).await;
        assert!(snapshot.is_ok(), "failed to load cancelled snapshot");
        let snapshot = match snapshot {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => panic!("cancelled snapshot missing"),
            Err(error) => panic!("failed to load cancelled snapshot: {error}"),
        };
        assert_eq!(snapshot.metadata.state, TaskState::Cancelled);
        assert_eq!(snapshot.checkpoint.state, TaskState::Cancelled);
        assert_eq!(snapshot.checkpoint.last_event_sequence, 2);
    }

    #[tokio::test]
    async fn cancellation_returns_completed_record_when_completion_wins_race() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::default());
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager,
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });
        let backend = Arc::new(ControlledBackend::new(BackendOutcome::Complete));

        let record = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(102),
                    task: "complete first".to_string(),
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
        wait_for_state(&registry, task_id, TaskState::Completed).await;

        let cancelled = executor.cancel_task(&task_id).await;
        assert!(matches!(
            cancelled,
            Ok(record) if record.metadata.state == TaskState::Completed
        ));

        let snapshot = storage.load_task_snapshot(task_id).await;
        assert!(snapshot.is_ok(), "failed to load completed snapshot");
        let snapshot = match snapshot {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => panic!("completed snapshot missing"),
            Err(error) => panic!("failed to load completed snapshot: {error}"),
        };
        assert_eq!(snapshot.metadata.state, TaskState::Completed);
        assert_eq!(snapshot.checkpoint.state, TaskState::Completed);
    }

    #[tokio::test]
    async fn cancellation_retries_persist_cancelled_snapshot_after_transient_failure() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::fail_on_save_state(TaskState::Cancelled));
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager,
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });

        let record = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(103),
                    task: "retry cancelled snapshot".to_string(),
                },
                Arc::new(ControlledBackend::new(BackendOutcome::Complete)),
            )
            .await;
        assert!(record.is_ok(), "submit failed: {record:?}");
        let task_id = match record {
            Ok(record) => record.metadata.id,
            Err(error) => panic!("unexpected submit error: {error}"),
        };

        let first_cancel = executor.cancel_task(&task_id).await;
        assert!(matches!(first_cancel, Err(TaskExecutorError::Storage(_))));
        wait_for_state(&registry, task_id, TaskState::Cancelled).await;

        let snapshot = storage.load_task_snapshot(task_id).await;
        assert!(
            snapshot.is_ok(),
            "failed to load snapshot after failed cancel"
        );
        let snapshot = match snapshot {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => panic!("snapshot missing after failed cancel"),
            Err(error) => panic!("failed to load snapshot after failed cancel: {error}"),
        };
        assert_eq!(snapshot.metadata.state, TaskState::Pending);

        let retry_cancel = executor.cancel_task(&task_id).await;
        assert!(matches!(
            retry_cancel,
            Ok(record) if record.metadata.state == TaskState::Cancelled
        ));

        let snapshot = storage.load_task_snapshot(task_id).await;
        assert!(
            snapshot.is_ok(),
            "failed to load repaired cancelled snapshot"
        );
        let snapshot = match snapshot {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => panic!("repaired cancelled snapshot missing"),
            Err(error) => panic!("failed to load repaired cancelled snapshot: {error}"),
        };
        assert_eq!(snapshot.metadata.state, TaskState::Cancelled);
        assert_eq!(snapshot.checkpoint.state, TaskState::Cancelled);
        assert_eq!(snapshot.checkpoint.last_event_sequence, 2);
    }

    #[tokio::test]
    async fn cancellation_repairs_cancelled_snapshot_after_worker_completion() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::fail_on_save_state(TaskState::Cancelled));
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager,
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });
        let backend = Arc::new(ControlledBackend::new(BackendOutcome::Complete));

        let record = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(105),
                    task: "worker completion repair".to_string(),
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

        let cancelled = executor.cancel_task(&task_id).await;
        assert!(matches!(cancelled, Err(TaskExecutorError::Storage(_))));

        let stale_snapshot = storage.load_task_snapshot(task_id).await;
        assert!(matches!(
            stale_snapshot,
            Ok(Some(snapshot)) if snapshot.metadata.state == TaskState::Running
        ));

        backend.release();
        wait_for_state(&registry, task_id, TaskState::Cancelled).await;
        wait_for_snapshot_state(&storage, task_id, TaskState::Cancelled).await;

        let repaired_snapshot = storage.load_task_snapshot(task_id).await;
        assert!(matches!(
            repaired_snapshot,
            Ok(Some(snapshot))
                if snapshot.metadata.state == TaskState::Cancelled
                    && snapshot.checkpoint.state == TaskState::Cancelled
                    && snapshot.checkpoint.last_event_sequence == 3
        ));
    }

    #[tokio::test]
    async fn cancellation_persists_event_log_for_restart_repair_when_cancelled_snapshot_save_fails()
    {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::fail_on_save_state(TaskState::Cancelled));
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager,
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });

        let record = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(106),
                    task: "restart repair".to_string(),
                },
                Arc::new(ControlledBackend::new(BackendOutcome::Complete)),
            )
            .await;
        assert!(record.is_ok(), "submit failed: {record:?}");
        let task_id = match record {
            Ok(record) => record.metadata.id,
            Err(error) => panic!("unexpected submit error: {error}"),
        };

        let cancelled = executor.cancel_task(&task_id).await;
        assert!(matches!(cancelled, Err(TaskExecutorError::Storage(_))));

        let events = storage.load_task_events(task_id).await;
        assert!(matches!(
            events,
            Ok(events)
                if events.iter().any(|event| {
                    event.sequence == 2 && event.state == TaskState::Cancelled
                })
        ));
    }

    #[tokio::test]
    async fn completion_wins_late_cancellation_without_cancelling_runtime_token() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::default());
        let executor = TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager,
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        });
        let backend = Arc::new(ControlledBackend::new(BackendOutcome::Complete));

        let record = executor
            .submit(
                DetachedTaskSubmission {
                    session_id: SessionId::from(104),
                    task: "late completion wins".to_string(),
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
        wait_for_state(&registry, task_id, TaskState::Completed).await;

        let token = registry.get_cancellation_token(&task_id).await;
        assert!(matches!(token, Some(ref token) if !token.is_cancelled()));

        let cancelled = executor.cancel_task(&task_id).await;
        assert!(matches!(
            cancelled,
            Ok(record) if record.metadata.state == TaskState::Completed
        ));

        let token = registry.get_cancellation_token(&task_id).await;
        assert!(matches!(token, Some(ref token) if !token.is_cancelled()));
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

    #[tokio::test]
    async fn detached_executor_rejects_concurrent_submit_for_same_session() {
        let registry = Arc::new(TaskRegistry::new());
        let worker_manager = Arc::new(WorkerManager::new(2));
        let storage = Arc::new(TestStorage::default());
        let executor = Arc::new(TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&registry),
            worker_manager: Arc::clone(&worker_manager),
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
        }));
        let backend = Arc::new(ControlledBackend::new(BackendOutcome::Complete));
        let session_id = SessionId::from(12);
        let start_barrier = Arc::new(Barrier::new(3));

        let first_submission = DetachedTaskSubmission {
            session_id,
            task: "first".to_string(),
        };
        let second_submission = DetachedTaskSubmission {
            session_id,
            task: "second".to_string(),
        };

        let first_task = {
            let executor = Arc::clone(&executor);
            let backend = Arc::clone(&backend);
            let start_barrier = Arc::clone(&start_barrier);
            tokio::spawn(async move {
                start_barrier.wait().await;
                executor.submit(first_submission, backend).await
            })
        };
        let second_task = {
            let executor = Arc::clone(&executor);
            let backend = Arc::clone(&backend);
            let start_barrier = Arc::clone(&start_barrier);
            tokio::spawn(async move {
                start_barrier.wait().await;
                executor.submit(second_submission, backend).await
            })
        };

        start_barrier.wait().await;

        let first_result = first_task.await;
        assert!(first_result.is_ok(), "first submit task failed to join");
        let second_result = second_task.await;
        assert!(second_result.is_ok(), "second submit task failed to join");

        let first_result = match first_result {
            Ok(result) => result,
            Err(error) => panic!("unexpected join error: {error}"),
        };
        let second_result = match second_result {
            Ok(result) => result,
            Err(error) => panic!("unexpected join error: {error}"),
        };

        let successful_record = match (first_result, second_result) {
            (
                Ok(record),
                Err(TaskExecutorError::SessionTaskAlreadyRunning(rejected_session_id)),
            ) => {
                assert_eq!(rejected_session_id, session_id);
                record
            }
            (
                Err(TaskExecutorError::SessionTaskAlreadyRunning(rejected_session_id)),
                Ok(record),
            ) => {
                assert_eq!(rejected_session_id, session_id);
                record
            }
            (left, right) => panic!("unexpected concurrent submit results: {left:?} {right:?}"),
        };

        wait_for_state(&registry, successful_record.metadata.id, TaskState::Running).await;

        let session_records = registry.list_by_session(&session_id).await;
        assert_eq!(session_records.len(), 1);
        assert_eq!(
            session_records[0].metadata.id,
            successful_record.metadata.id
        );
        assert!(
            worker_manager
                .contains(&successful_record.metadata.id)
                .await
        );
        assert_eq!(worker_manager.active_count().await, 1);

        backend.release();
        wait_for_state(
            &registry,
            successful_record.metadata.id,
            TaskState::Completed,
        )
        .await;
    }

    fn sample_pending_input() -> PendingInput {
        PendingInput {
            request_id: "hitl-request-1".to_string(),
            prompt: "Please provide approval details".to_string(),
            kind: PendingInputKind::Text(PendingTextInput {
                min_length: Some(1),
                max_length: Some(512),
                multiline: true,
            }),
        }
    }

    fn invalid_pending_input() -> PendingInput {
        PendingInput {
            request_id: "".to_string(),
            prompt: "invalid".to_string(),
            kind: PendingInputKind::Text(PendingTextInput {
                min_length: Some(1),
                max_length: Some(16),
                multiline: false,
            }),
        }
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

    async fn wait_for_snapshot_state(storage: &TestStorage, task_id: TaskId, expected: TaskState) {
        let waited = timeout(Duration::from_secs(5), async {
            loop {
                let snapshot = storage.load_task_snapshot(task_id).await;
                if let Ok(Some(snapshot)) = snapshot {
                    if snapshot.metadata.state == expected {
                        break;
                    }
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        assert!(waited.is_ok(), "snapshot did not reach {expected:?}");
    }

    async fn wait_for_worker_completion(worker_manager: &WorkerManager, task_id: TaskId) {
        let waited = timeout(Duration::from_secs(5), async {
            loop {
                if !worker_manager.contains(&task_id).await {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        assert!(waited.is_ok(), "worker did not complete for task {task_id}");
    }
}
