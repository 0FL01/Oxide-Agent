//! Runtime-owned manager for detached background workers.

use oxide_agent_core::agent::TaskId;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{error, warn};

#[derive(Default)]
struct WorkerManagerState {
    workers: HashMap<TaskId, JoinHandle<()>>,
}

/// Errors returned by worker manager operations.
#[derive(Debug, PartialEq, Eq)]
pub enum WorkerManagerError {
    /// A worker is already registered for this task.
    WorkerAlreadyRunning(TaskId),
    /// The configured worker limit has been reached.
    WorkerLimitReached {
        /// Maximum number of concurrently tracked workers.
        limit: usize,
    },
}

impl fmt::Display for WorkerManagerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkerAlreadyRunning(task_id) => {
                write!(f, "worker already running for task: {task_id}")
            }
            Self::WorkerLimitReached { limit } => {
                write!(f, "worker limit reached: {limit}")
            }
        }
    }
}

impl std::error::Error for WorkerManagerError {}

/// Runtime-owned detached worker registry keyed by task identifier.
pub struct WorkerManager {
    max_workers: usize,
    state: RwLock<WorkerManagerState>,
}

impl WorkerManager {
    /// Create a manager with a fixed limit for concurrently tracked workers.
    #[must_use]
    pub fn new(max_workers: usize) -> Self {
        Self {
            max_workers,
            state: RwLock::new(WorkerManagerState::default()),
        }
    }

    /// Return the configured concurrent worker limit.
    #[must_use]
    pub const fn max_workers(&self) -> usize {
        self.max_workers
    }

    /// Spawn and track a detached worker owned by the runtime.
    pub async fn spawn<F>(&self, task_id: TaskId, worker: F) -> Result<(), WorkerManagerError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn_inner(task_id, worker, || {}).await
    }

    async fn spawn_inner<F, H>(
        &self,
        task_id: TaskId,
        worker: F,
        pre_admission_hook: H,
    ) -> Result<(), WorkerManagerError>
    where
        F: Future<Output = ()> + Send + 'static,
        H: FnOnce(),
    {
        let (completed, result) = {
            let mut state = self.state.write().await;
            let mut completed = Self::take_completed_workers(&mut state);

            pre_admission_hook();
            completed.extend(Self::take_completed_workers(&mut state));

            let result = if state.workers.contains_key(&task_id) {
                Err(WorkerManagerError::WorkerAlreadyRunning(task_id))
            } else if state.workers.len() >= self.max_workers {
                Err(WorkerManagerError::WorkerLimitReached {
                    limit: self.max_workers,
                })
            } else {
                state.workers.insert(task_id, tokio::spawn(worker));
                Ok(())
            };

            (completed, result)
        };

        Self::await_completed_workers(completed).await;
        result
    }

    #[cfg(test)]
    async fn spawn_with_pre_admission_hook<F, H>(
        &self,
        task_id: TaskId,
        worker: F,
        pre_admission_hook: H,
    ) -> Result<(), WorkerManagerError>
    where
        F: Future<Output = ()> + Send + 'static,
        H: FnOnce(),
    {
        self.spawn_inner(task_id, worker, pre_admission_hook).await
    }

    fn take_completed_workers(state: &mut WorkerManagerState) -> Vec<(TaskId, JoinHandle<()>)> {
        let completed_ids = state
            .workers
            .iter()
            .filter_map(|(task_id, handle)| handle.is_finished().then_some(*task_id))
            .collect::<Vec<_>>();

        completed_ids
            .into_iter()
            .filter_map(|task_id| {
                state
                    .workers
                    .remove(&task_id)
                    .map(|handle| (task_id, handle))
            })
            .collect()
    }

    async fn await_completed_workers(completed: Vec<(TaskId, JoinHandle<()>)>) -> usize {
        let completed_count = completed.len();

        for (task_id, handle) in completed {
            match handle.await {
                Ok(()) => {}
                Err(error) if error.is_cancelled() => {
                    warn!(task_id = %task_id, "Background worker was cancelled before cleanup");
                }
                Err(error) => {
                    error!(task_id = %task_id, error = %error, "Background worker failed");
                }
            }
        }

        completed_count
    }

    /// Remove completed workers and absorb their join outcomes.
    pub async fn cleanup_completed(&self) -> usize {
        let completed = {
            let mut state = self.state.write().await;
            Self::take_completed_workers(&mut state)
        };

        Self::await_completed_workers(completed).await
    }

    /// Return the number of active tracked workers after cleanup.
    pub async fn active_count(&self) -> usize {
        self.cleanup_completed().await;

        let state = self.state.read().await;
        state.workers.len()
    }

    /// Return true when the task currently has a tracked active worker.
    pub async fn contains(&self, task_id: &TaskId) -> bool {
        self.cleanup_completed().await;

        let state = self.state.read().await;
        state.workers.contains_key(task_id)
    }
}

#[cfg(test)]
mod tests {
    use super::{WorkerManager, WorkerManagerError};
    use oxide_agent_core::agent::TaskId;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::{Duration, Instant};
    use tokio::sync::oneshot;
    use tokio::task::yield_now;

    #[tokio::test]
    async fn worker_manager_spawns_and_tracks_detached_worker() {
        let manager = WorkerManager::new(2);
        let task_id = TaskId::new();
        let (release_tx, release_rx) = oneshot::channel();

        let spawn_result = manager
            .spawn(task_id, async move {
                let _ = release_rx.await;
            })
            .await;

        assert!(spawn_result.is_ok());
        assert!(manager.contains(&task_id).await);
        assert_eq!(manager.active_count().await, 1);

        let send_result = release_tx.send(());
        assert!(send_result.is_ok());
    }

    #[tokio::test]
    async fn worker_manager_cleans_up_completed_workers() {
        let manager = WorkerManager::new(1);
        let task_id = TaskId::new();
        let (done_tx, done_rx) = oneshot::channel();

        let spawn_result = manager
            .spawn(task_id, async move {
                let _ = done_tx.send(());
            })
            .await;

        assert!(spawn_result.is_ok());
        let done_result = done_rx.await;
        assert!(done_result.is_ok());

        yield_now().await;

        assert_eq!(manager.cleanup_completed().await, 1);
        assert_eq!(manager.active_count().await, 0);
        assert!(!manager.contains(&task_id).await);
    }

    #[tokio::test]
    async fn worker_manager_enforces_worker_limit() {
        let manager = WorkerManager::new(1);
        let first_task_id = TaskId::new();
        let second_task_id = TaskId::new();
        let (_release_tx, release_rx) = oneshot::channel::<()>();

        let first_spawn = manager
            .spawn(first_task_id, async move {
                let _ = release_rx.await;
            })
            .await;
        assert!(first_spawn.is_ok());

        let second_spawn = manager.spawn(second_task_id, async {}).await;
        assert_eq!(
            second_spawn,
            Err(WorkerManagerError::WorkerLimitReached { limit: 1 })
        );
    }

    #[tokio::test]
    async fn worker_manager_rejects_duplicate_task_ids() {
        let manager = WorkerManager::new(2);
        let task_id = TaskId::new();
        let (_release_tx, release_rx) = oneshot::channel::<()>();

        let first_spawn = manager
            .spawn(task_id, async move {
                let _ = release_rx.await;
            })
            .await;
        assert!(first_spawn.is_ok());

        let duplicate_spawn = manager.spawn(task_id, async {}).await;
        assert_eq!(
            duplicate_spawn,
            Err(WorkerManagerError::WorkerAlreadyRunning(task_id))
        );
    }

    #[tokio::test]
    async fn worker_manager_isolates_failed_worker_cleanup() {
        let manager = WorkerManager::new(1);
        let failed_task_id = TaskId::new();

        let failed_spawn = manager
            .spawn(failed_task_id, async move {
                panic!("simulated worker panic");
            })
            .await;
        assert!(failed_spawn.is_ok());

        yield_now().await;

        assert_eq!(manager.cleanup_completed().await, 1);
        assert_eq!(manager.active_count().await, 0);

        let healthy_task_id = TaskId::new();
        let (done_tx, done_rx) = oneshot::channel();
        let healthy_spawn = manager
            .spawn(healthy_task_id, async move {
                let _ = done_tx.send(());
            })
            .await;
        assert!(healthy_spawn.is_ok());

        let done_result = done_rx.await;
        assert!(done_result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn worker_manager_allows_spawn_when_worker_finishes_during_admission() {
        let manager = WorkerManager::new(1);
        let first_task_id = TaskId::new();
        let second_task_id = TaskId::new();
        let finished = Arc::new(AtomicBool::new(false));
        let finished_worker = Arc::clone(&finished);
        let (release_first_tx, release_first_rx) = oneshot::channel();
        let (_release_second_tx, release_second_rx) = oneshot::channel::<()>();

        let first_spawn = manager
            .spawn(first_task_id, async move {
                let _ = release_first_rx.await;
                finished_worker.store(true, Ordering::SeqCst);
            })
            .await;
        assert!(first_spawn.is_ok());

        let second_spawn = manager
            .spawn_with_pre_admission_hook(
                second_task_id,
                async move {
                    let _ = release_second_rx.await;
                },
                move || {
                    let send_result = release_first_tx.send(());
                    assert!(send_result.is_ok());

                    let start = Instant::now();
                    while !finished.load(Ordering::SeqCst) {
                        assert!(
                            start.elapsed() < Duration::from_secs(1),
                            "timed out waiting for first worker to finish"
                        );
                        std::thread::yield_now();
                    }

                    std::thread::sleep(Duration::from_millis(10));
                },
            )
            .await;

        assert!(second_spawn.is_ok());
        assert!(manager.contains(&second_task_id).await);
        assert!(!manager.contains(&first_task_id).await);
    }
}
