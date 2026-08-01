//! DB-backed life worker contracts and orchestration skeleton.
//!
//! The worker owns run state. Transports and the gateway submit only inputs; this
//! module claims queued input from Postgres, starts a persisted run, exposes the
//! stable life execution, and records transport-neutral run events.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

use crate::domain::{
    EventId, InputId, LifeEvent, LifeInput, PrincipalUserId, RunId, TimestampMillis,
};
use crate::storage::LIFE_RUN_LEASE_MILLIS;
use crate::storage::{
    CancelLifeRunOutcome, ClaimedLifeInputRun, LifeStorageError, LifeStorageRepository,
};

/// Stable life context key for hot-memory checkpoints.
pub const LIFE_CONTEXT_KEY: &str = "life";

/// Stable life flow id for the main permanent-life thread.
pub const LIFE_FLOW_ID: &str = "main";

/// Heartbeat interval for active run leases.
///
/// The interval is intentionally shorter than the durable lease duration so a
/// live worker refreshes ownership before another claim can reap it.
pub const LIFE_RUN_HEARTBEAT_INTERVAL_MILLIS: u64 = (LIFE_RUN_LEASE_MILLIS as u64) / 3;

/// Command to process a queued principal input.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessPrincipalInput {
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Input id to process.
    pub input_id: InputId,
}

/// Claimed run context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedLifeRun {
    /// Run id.
    pub run_id: RunId,
    /// Claimed input that started the run.
    pub input: LifeInput,
    /// User turn content loaded from `life_turns` at claim time.
    pub user_content: String,
}

impl From<ClaimedLifeInputRun> for ClaimedLifeRun {
    fn from(value: ClaimedLifeInputRun) -> Self {
        Self {
            run_id: value.run.run_id,
            input: value.input,
            user_content: value.user_content,
        }
    }
}

/// Worker execution context passed to the runtime/executor seam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeWorkerRunContext {
    /// Original worker command.
    pub command: ProcessPrincipalInput,
    /// Persisted running run.
    pub run: ClaimedLifeRun,
}

/// Outcome of one executor/run attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifeRunExecutionOutcome {
    /// Timestamp at which the final checkpoint was durably persisted.
    pub final_checkpoint_at: TimestampMillis,
}

/// Worker result for one command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifeWorkerProcessResult {
    /// No work was claimed because the input was absent, not queued, or another run is active.
    NotClaimed,
    /// A run executed and was marked completed.
    Completed {
        /// Completed run id.
        run_id: RunId,
    },
    /// A run was cancelled by an owner request.
    Cancelled {
        /// Cancelled run id.
        run_id: RunId,
    },
}

/// Worker orchestration errors.
#[derive(Debug, Error)]
pub enum LifeWorkerError {
    /// Durable storage failed.
    #[error(transparent)]
    Storage(#[from] LifeStorageError),
    /// Clock failed.
    #[error("life worker clock error: {0}")]
    Clock(String),
    /// Executor failed.
    #[error("life worker executor error: {0}")]
    Executor(String),
    /// Executor observed an owner-requested cancellation.
    #[error("life worker run cancelled for run {run_id}")]
    Cancelled {
        /// Cancelled run id.
        run_id: RunId,
    },
    /// The running lease is no longer owned by this worker.
    #[error("life worker run lease lost for run {run_id}")]
    LostLease {
        /// Run whose lease could not be extended.
        run_id: RunId,
    },
}

/// Result alias for worker operations.
pub type LifeWorkerResult<T> = Result<T, LifeWorkerError>;

/// Clock seam for deterministic worker tests.
pub trait LifeWorkerClock: Send + Sync {
    /// Current time in milliseconds.
    fn now(&self) -> LifeWorkerResult<TimestampMillis>;
}

/// System clock implementation.
#[derive(Debug, Copy, Clone, Default)]
pub struct SystemLifeWorkerClock;

impl LifeWorkerClock for SystemLifeWorkerClock {
    fn now(&self) -> LifeWorkerResult<TimestampMillis> {
        let duration = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| LifeWorkerError::Clock(error.to_string()))?;
        let millis = i64::try_from(duration.as_millis())
            .map_err(|error| LifeWorkerError::Clock(error.to_string()))?;
        Ok(TimestampMillis::new(millis))
    }
}

/// Runtime/executor seam used by the DB-backed worker.
#[async_trait]
pub trait LifeRunExecutor: Send + Sync {
    /// Executes a claimed life run. Real AgentExecutor integration is a later adapter.
    async fn execute_life_run(
        &self,
        context: LifeWorkerRunContext,
    ) -> LifeWorkerResult<LifeRunExecutionOutcome>;
}

/// `LifeRunExecutor` is object-safe, so `Arc<dyn LifeRunExecutor>` is the
/// natural way to share a single executor instance across spawned tasks.
#[async_trait]
impl LifeRunExecutor for std::sync::Arc<dyn LifeRunExecutor> {
    async fn execute_life_run(
        &self,
        context: LifeWorkerRunContext,
    ) -> LifeWorkerResult<LifeRunExecutionOutcome> {
        self.as_ref().execute_life_run(context).await
    }
}

/// Narrow worker storage boundary.
#[async_trait]
pub trait LifeWorkerStore: Send + Sync {
    /// Atomically claim one queued input and create a running run.
    async fn claim_input_and_start_run(
        &self,
        principal_user_id: PrincipalUserId,
        input_id: InputId,
        run_id: RunId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeWorkerResult<Option<ClaimedLifeInputRun>>;

    /// Marks a claimed input as consumed.
    async fn mark_input_consumed(
        &self,
        input_id: InputId,
        now: TimestampMillis,
    ) -> LifeWorkerResult<()>;

    /// Atomically claims the oldest queued input for a principal and starts a new running run.
    async fn claim_next_queued_input_and_start_run(
        &self,
        principal_user_id: PrincipalUserId,
        run_id: RunId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeWorkerResult<Option<ClaimedLifeInputRun>>;

    /// Extends a running run lease owned by `worker_id`.
    async fn heartbeat_run_lease(
        &self,
        run_id: RunId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeWorkerResult<bool>;

    /// Appends a run event.
    async fn append_event(&self, event: &LifeEvent) -> LifeWorkerResult<()>;

    /// Returns next event sequence.
    async fn next_event_seq(&self, run_id: RunId) -> LifeWorkerResult<i64>;

    /// Marks a run completed.
    async fn complete_run(
        &self,
        run_id: RunId,
        finished_at: TimestampMillis,
        last_checkpoint_at: TimestampMillis,
    ) -> LifeWorkerResult<bool>;

    /// Marks a run failed.
    async fn fail_run(
        &self,
        run_id: RunId,
        finished_at: TimestampMillis,
        error_text: &str,
    ) -> LifeWorkerResult<bool>;

    /// Cancels a run.
    async fn cancel_run(
        &self,
        principal_user_id: PrincipalUserId,
        run_id: RunId,
        cancelled_at: TimestampMillis,
    ) -> LifeWorkerResult<CancelLifeRunOutcome>;
}

#[async_trait]
impl<T> LifeWorkerStore for T
where
    T: LifeStorageRepository,
{
    async fn claim_input_and_start_run(
        &self,
        principal_user_id: PrincipalUserId,
        input_id: InputId,
        run_id: RunId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeWorkerResult<Option<ClaimedLifeInputRun>> {
        LifeStorageRepository::claim_input_and_start_run(
            self,
            principal_user_id,
            input_id,
            run_id,
            worker_id,
            now,
        )
        .await
        .map_err(Into::into)
    }

    async fn mark_input_consumed(
        &self,
        input_id: InputId,
        now: TimestampMillis,
    ) -> LifeWorkerResult<()> {
        LifeStorageRepository::mark_input_consumed(self, input_id, now)
            .await
            .map_err(Into::into)
    }

    async fn claim_next_queued_input_and_start_run(
        &self,
        principal_user_id: PrincipalUserId,
        run_id: RunId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeWorkerResult<Option<ClaimedLifeInputRun>> {
        LifeStorageRepository::claim_next_queued_input_and_start_run(
            self,
            principal_user_id,
            run_id,
            worker_id,
            now,
        )
        .await
        .map_err(Into::into)
    }

    async fn heartbeat_run_lease(
        &self,
        run_id: RunId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeWorkerResult<bool> {
        LifeStorageRepository::heartbeat_run_lease(self, run_id, worker_id, now)
            .await
            .map_err(Into::into)
    }

    async fn append_event(&self, event: &LifeEvent) -> LifeWorkerResult<()> {
        LifeStorageRepository::append_event(self, event)
            .await
            .map_err(Into::into)
    }

    async fn next_event_seq(&self, run_id: RunId) -> LifeWorkerResult<i64> {
        LifeStorageRepository::next_event_seq(self, run_id)
            .await
            .map_err(Into::into)
    }

    async fn complete_run(
        &self,
        run_id: RunId,
        finished_at: TimestampMillis,
        last_checkpoint_at: TimestampMillis,
    ) -> LifeWorkerResult<bool> {
        LifeStorageRepository::complete_run(self, run_id, finished_at, last_checkpoint_at)
            .await
            .map_err(Into::into)
    }

    async fn fail_run(
        &self,
        run_id: RunId,
        finished_at: TimestampMillis,
        error_text: &str,
    ) -> LifeWorkerResult<bool> {
        LifeStorageRepository::fail_run(self, run_id, finished_at, error_text)
            .await
            .map_err(Into::into)
    }

    async fn cancel_run(
        &self,
        principal_user_id: PrincipalUserId,
        run_id: RunId,
        cancelled_at: TimestampMillis,
    ) -> LifeWorkerResult<CancelLifeRunOutcome> {
        LifeStorageRepository::cancel_run(self, principal_user_id, run_id, cancelled_at)
            .await
            .map_err(Into::into)
    }
}

/// DB-backed life worker.
pub struct LifeWorker<S, E, C = SystemLifeWorkerClock> {
    store: S,
    executor: E,
    clock: C,
    worker_id: String,
}

impl<S, E> LifeWorker<S, E, SystemLifeWorkerClock> {
    /// Creates a worker with the system clock.
    pub fn new(store: S, executor: E, worker_id: impl Into<String>) -> Self {
        Self::new_with_clock(store, executor, SystemLifeWorkerClock, worker_id)
    }
}

impl<S, E, C> LifeWorker<S, E, C> {
    /// Creates a worker with an explicit clock.
    pub fn new_with_clock(store: S, executor: E, clock: C, worker_id: impl Into<String>) -> Self {
        Self {
            store,
            executor,
            clock,
            worker_id: worker_id.into(),
        }
    }

    /// Durable lease owner id used for run claims and heartbeats.
    #[must_use]
    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }
}

impl<S, E, C> LifeWorker<S, E, C>
where
    S: LifeWorkerStore,
    E: LifeRunExecutor,
    C: LifeWorkerClock,
{
    /// Processes one queued life input if it can be claimed safely.
    ///
    /// Claims the input, starts a run, then delegates to [`execute_claimed_run`].
    /// Returns `NotClaimed` if another run is already active or the input is no
    /// longer queued.
    pub async fn process_principal_input(
        &self,
        command: ProcessPrincipalInput,
    ) -> LifeWorkerResult<LifeWorkerProcessResult> {
        let started_at = self.clock.now()?;
        let run_id = RunId::new_v4();
        let Some(claimed) = self
            .store
            .claim_input_and_start_run(
                command.principal_user_id,
                command.input_id,
                run_id,
                &self.worker_id,
                started_at,
            )
            .await?
        else {
            return Ok(LifeWorkerProcessResult::NotClaimed);
        };

        self.execute_claimed_run(claimed).await
    }

    /// Claims and processes the oldest queued input for a principal, if any.
    ///
    /// This keeps durable run lease ownership inside the worker boundary: callers
    /// provide only the principal whose queue should be drained, while the worker
    /// supplies its own `worker_id` to the storage claim and to all heartbeats.
    pub async fn process_next_queued_input(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeWorkerResult<LifeWorkerProcessResult> {
        let started_at = self.clock.now()?;
        let run_id = RunId::new_v4();
        let Some(claimed) = self
            .store
            .claim_next_queued_input_and_start_run(
                principal_user_id,
                run_id,
                &self.worker_id,
                started_at,
            )
            .await?
        else {
            return Ok(LifeWorkerProcessResult::NotClaimed);
        };

        self.execute_claimed_run(claimed).await
    }

    /// Executes an already-claimed run to completion.
    ///
    /// This is the entry point for the runtime handle path: the handle claims
    /// the input and starts the run, then the caller spawns this method.
    /// The method executes exactly one claimed input, marks only that input
    /// consumed after executor success, then claims any queued follow-up as a
    /// separate run. This preserves the one-input-one-run boundary and makes
    /// silent follow-up consumption impossible.
    pub async fn execute_claimed_run(
        &self,
        claimed: ClaimedLifeInputRun,
    ) -> LifeWorkerResult<LifeWorkerProcessResult> {
        let mut claimed = claimed;
        loop {
            if claimed.run.lease_owner.as_deref() != Some(self.worker_id.as_str()) {
                return Err(LifeWorkerError::LostLease {
                    run_id: claimed.run.run_id,
                });
            }

            let started_at = match claimed.run.started_at {
                Some(ts) => ts,
                None => self.clock.now()?,
            };
            let claimed_run = ClaimedLifeRun::from(claimed);
            let principal_user_id = claimed_run.input.principal_user_id;

            let heartbeat_at = self.clock.now()?;
            let still_owned = self
                .store
                .heartbeat_run_lease(claimed_run.run_id, &self.worker_id, heartbeat_at)
                .await?;
            if !still_owned {
                return Err(LifeWorkerError::LostLease {
                    run_id: claimed_run.run_id,
                });
            }

            self.append_event(claimed_run.run_id, "run_started", started_at)
                .await?;

            let context = LifeWorkerRunContext {
                command: ProcessPrincipalInput {
                    principal_user_id,
                    input_id: claimed_run.input.input_id,
                },
                run: claimed_run.clone(),
            };

            let mut execution = std::pin::pin!(self.executor.execute_life_run(context));
            let execution_result = loop {
                tokio::select! {
                    result = &mut execution => break result,
                    () = tokio::time::sleep(std::time::Duration::from_millis(LIFE_RUN_HEARTBEAT_INTERVAL_MILLIS)) => {
                        let heartbeat_at = self.clock.now()?;
                        let still_owned = self
                            .store
                            .heartbeat_run_lease(claimed_run.run_id, &self.worker_id, heartbeat_at)
                            .await?;
                        if !still_owned {
                            return Err(LifeWorkerError::LostLease { run_id: claimed_run.run_id });
                        }
                    }
                }
            };

            match execution_result {
                Ok(outcome) => {
                    let finished_at = self.clock.now()?;
                    self.store
                        .mark_input_consumed(claimed_run.input.input_id, finished_at)
                        .await?;
                    let completed = self
                        .store
                        .complete_run(claimed_run.run_id, finished_at, outcome.final_checkpoint_at)
                        .await?;
                    if !completed {
                        return Ok(LifeWorkerProcessResult::Cancelled {
                            run_id: claimed_run.run_id,
                        });
                    }
                    self.append_event(claimed_run.run_id, "run_completed", finished_at)
                        .await?;

                    let completed = LifeWorkerProcessResult::Completed {
                        run_id: claimed_run.run_id,
                    };
                    let next_run_id = RunId::new_v4();
                    let Some(next_claimed) = self
                        .store
                        .claim_next_queued_input_and_start_run(
                            principal_user_id,
                            next_run_id,
                            &self.worker_id,
                            finished_at,
                        )
                        .await?
                    else {
                        return Ok(completed);
                    };
                    claimed = next_claimed;
                }
                Err(LifeWorkerError::Cancelled { .. }) => {
                    let finished_at = self.clock.now()?;
                    self.store
                        .mark_input_consumed(claimed_run.input.input_id, finished_at)
                        .await?;
                    let cancelled = self
                        .store
                        .cancel_run(principal_user_id, claimed_run.run_id, finished_at)
                        .await?;
                    if matches!(cancelled, CancelLifeRunOutcome::Cancelled) {
                        self.append_event(claimed_run.run_id, "run_cancelled", finished_at)
                            .await?;
                    }
                    return Ok(LifeWorkerProcessResult::Cancelled {
                        run_id: claimed_run.run_id,
                    });
                }
                Err(error) => {
                    let error_text = error.to_string();
                    let finished_at = self.clock.now()?;
                    let failed = self
                        .store
                        .fail_run(claimed_run.run_id, finished_at, &error_text)
                        .await?;
                    if !failed {
                        return Ok(LifeWorkerProcessResult::Cancelled {
                            run_id: claimed_run.run_id,
                        });
                    }
                    self.append_event(claimed_run.run_id, "run_failed", finished_at)
                        .await?;
                    return Err(LifeWorkerError::Executor(error_text));
                }
            }
        }
    }

    async fn append_event(
        &self,
        run_id: RunId,
        kind: &str,
        created_at: TimestampMillis,
    ) -> LifeWorkerResult<()> {
        let seq = self.store.next_event_seq(run_id).await?;
        let event = LifeEvent {
            event_id: EventId::new_v4(),
            run_id,
            seq,
            kind: kind.to_owned(),
            payload: json!({}),
            created_at,
        };
        self.store.append_event(&event).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::domain::{LifeInputStatus, LifeRun, TurnId};

    #[tokio::test]
    async fn worker_claims_and_completes_run() {
        let principal = PrincipalUserId::new(100500).expect("positive principal");
        let input_id = InputId::new_v4();
        let store = FakeWorkerStore::with_claim(principal, input_id);
        let executor = RecordingExecutor::success(TimestampMillis::new(30));
        let seen_context = Arc::clone(&executor.seen_context);
        let worker = LifeWorker::new_with_clock(
            store.clone(),
            executor,
            FixedWorkerClock::new([
                TimestampMillis::new(10),
                TimestampMillis::new(20),
                TimestampMillis::new(30),
            ]),
            "worker-a",
        );

        let result = worker
            .process_principal_input(ProcessPrincipalInput {
                principal_user_id: principal,
                input_id,
            })
            .await
            .expect("worker should process input");

        let LifeWorkerProcessResult::Completed { .. } = result else {
            panic!("expected completed result");
        };
        assert_eq!(store.completed_runs.lock().expect("lock").len(), 1);
        assert_eq!(*store.consumed_inputs.lock().expect("lock"), vec![input_id]);
        assert_eq!(store.event_kinds(), vec!["run_started", "run_completed"]);

        let context = seen_context
            .lock()
            .expect("lock")
            .clone()
            .expect("executor should see context");
        assert_eq!(context.run.user_content, "test user content");
    }

    #[tokio::test]
    async fn worker_returns_not_claimed_when_input_is_unavailable() {
        let principal = PrincipalUserId::new(100501).expect("positive principal");
        let input_id = InputId::new_v4();
        let store = FakeWorkerStore::without_claim();
        let worker = LifeWorker::new_with_clock(
            store.clone(),
            RecordingExecutor::success(TimestampMillis::new(30)),
            FixedWorkerClock::new([TimestampMillis::new(10)]),
            "worker-a",
        );

        let result = worker
            .process_principal_input(ProcessPrincipalInput {
                principal_user_id: principal,
                input_id,
            })
            .await
            .expect("worker should not error");

        assert_eq!(result, LifeWorkerProcessResult::NotClaimed);
        assert!(store.events.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn worker_marks_run_failed_when_executor_fails() {
        let principal = PrincipalUserId::new(100502).expect("positive principal");
        let input_id = InputId::new_v4();
        let store = FakeWorkerStore::with_claim(principal, input_id);
        let worker = LifeWorker::new_with_clock(
            store.clone(),
            RecordingExecutor::failure("executor boom"),
            FixedWorkerClock::new([
                TimestampMillis::new(10),
                TimestampMillis::new(20),
                TimestampMillis::new(30),
            ]),
            "worker-a",
        );

        let error = worker
            .process_principal_input(ProcessPrincipalInput {
                principal_user_id: principal,
                input_id,
            })
            .await
            .expect_err("executor failure should propagate");
        assert!(error.to_string().contains("executor boom"));
        assert_eq!(store.failed_runs.lock().expect("lock").len(), 1);
        assert_eq!(store.event_kinds(), vec!["run_started", "run_failed"]);
    }

    #[tokio::test]
    async fn worker_marks_run_cancelled_when_executor_is_cancelled() {
        let principal = PrincipalUserId::new(100506).expect("positive principal");
        let input_id = InputId::new_v4();
        let store = FakeWorkerStore::with_claim(principal, input_id);
        let run_id = store
            .claim
            .lock()
            .expect("lock")
            .as_ref()
            .expect("claim should exist")
            .run
            .run_id;
        let worker = LifeWorker::new_with_clock(
            store.clone(),
            RecordingExecutor::cancelled(run_id),
            FixedWorkerClock::new([
                TimestampMillis::new(10),
                TimestampMillis::new(20),
                TimestampMillis::new(30),
            ]),
            "worker-a",
        );

        let result = worker
            .process_principal_input(ProcessPrincipalInput {
                principal_user_id: principal,
                input_id,
            })
            .await
            .expect("cancelled run should be handled as terminal");

        assert_eq!(result, LifeWorkerProcessResult::Cancelled { run_id });
        assert_eq!(*store.cancelled_runs.lock().expect("lock"), vec![run_id]);
        assert_eq!(*store.consumed_inputs.lock().expect("lock"), vec![input_id]);
        assert!(store.completed_runs.lock().expect("lock").is_empty());
        assert!(store.failed_runs.lock().expect("lock").is_empty());
        assert_eq!(store.event_kinds(), vec!["run_started", "run_cancelled"]);
    }

    #[tokio::test]
    async fn execute_claimed_run_executes_follow_up_inputs_as_separate_runs() {
        let principal = PrincipalUserId::new(100503).expect("positive principal");
        let input_id = InputId::new_v4();
        let store = FakeWorkerStore::with_claim(principal, input_id);

        // Simulate a follow-up input that was queued while the first run was active.
        let follow_up_turn_id = TurnId::new_v4();
        let follow_up_run_id = RunId::new_v4();
        let follow_up_input = LifeInput {
            input_id: InputId::new_v4(),
            principal_user_id: principal,
            turn_id: follow_up_turn_id,
            status: LifeInputStatus::Queued,
            claimed_by: None,
            claimed_at: None,
            created_at: TimestampMillis::new(12),
            updated_at: TimestampMillis::new(12),
        };
        store
            .queued_claims
            .lock()
            .expect("lock")
            .push(ClaimedLifeInputRun {
                input: follow_up_input.clone(),
                run: LifeRun {
                    run_id: follow_up_run_id,
                    principal_user_id: principal,
                    status: crate::domain::LifeRunStatus::Running,
                    started_at: Some(TimestampMillis::new(20)),
                    finished_at: None,
                    last_checkpoint_at: None,
                    error_text: None,
                    lease_owner: Some("worker-a".to_owned()),
                    lease_expires_at: Some(TimestampMillis::new(20 + LIFE_RUN_LEASE_MILLIS)),
                    last_heartbeat_at: Some(TimestampMillis::new(20)),
                    created_at: TimestampMillis::new(20),
                    updated_at: TimestampMillis::new(20),
                },
                user_content: "follow-up content".to_owned(),
            });

        // Extract the claimed run for direct execute_claimed_run call.
        let claimed = store
            .claim
            .lock()
            .expect("lock")
            .take()
            .expect("claim should be present");
        let expected_run_id = claimed.run.run_id;

        let executor = RecordingExecutor::success(TimestampMillis::new(30));
        let seen_contexts = Arc::clone(&executor.seen_contexts);
        let worker = LifeWorker::new_with_clock(
            store.clone(),
            executor,
            FixedWorkerClock::new([
                TimestampMillis::new(20),
                TimestampMillis::new(40),
                TimestampMillis::new(50),
                TimestampMillis::new(60),
            ]),
            "worker-a",
        );

        let result = worker
            .execute_claimed_run(claimed)
            .await
            .expect("worker should execute claimed run");

        let LifeWorkerProcessResult::Completed { run_id, .. } = result else {
            panic!("expected completed result");
        };
        assert_eq!(run_id, follow_up_run_id);

        // Each input is executed and consumed by its own claimed run; the
        // durable turn/run association is owned by the storage claim boundary.
        let consumed = store.consumed_inputs.lock().expect("lock").clone();
        assert_eq!(consumed, vec![input_id, follow_up_input.input_id]);

        let contexts = seen_contexts.lock().expect("lock").clone();
        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts[0].run.run_id, expected_run_id);
        assert_eq!(contexts[1].run.run_id, follow_up_run_id);
        assert_eq!(contexts[1].run.input.turn_id, follow_up_turn_id);
        assert_eq!(contexts[0].run.user_content, "test user content");
        assert_eq!(contexts[1].run.user_content, "follow-up content");

        assert_eq!(
            store.event_kinds(),
            vec![
                "run_started",
                "run_completed",
                "run_started",
                "run_completed"
            ]
        );
    }

    #[tokio::test]
    async fn process_next_queued_input_claims_with_worker_owned_id() {
        let principal = PrincipalUserId::new(100504).expect("positive principal");
        let now = TimestampMillis::new(10);
        let input_id = InputId::new_v4();
        let turn_id = TurnId::new_v4();
        let run_id = RunId::new_v4();
        let store = FakeWorkerStore::without_claim();
        store
            .queued_claims
            .lock()
            .expect("lock")
            .push(ClaimedLifeInputRun {
                input: LifeInput {
                    input_id,
                    principal_user_id: principal,
                    turn_id,
                    status: LifeInputStatus::Queued,
                    claimed_by: None,
                    claimed_at: None,
                    created_at: now,
                    updated_at: now,
                },
                run: LifeRun {
                    run_id,
                    principal_user_id: principal,
                    status: crate::domain::LifeRunStatus::Running,
                    started_at: Some(now),
                    finished_at: None,
                    last_checkpoint_at: None,
                    error_text: None,
                    lease_owner: Some("worker-a".to_owned()),
                    lease_expires_at: Some(TimestampMillis::new(now.get() + LIFE_RUN_LEASE_MILLIS)),
                    last_heartbeat_at: Some(now),
                    created_at: now,
                    updated_at: now,
                },
                user_content: "queued content".to_owned(),
            });

        let worker = LifeWorker::new_with_clock(
            store.clone(),
            RecordingExecutor::success(TimestampMillis::new(30)),
            FixedWorkerClock::new([
                TimestampMillis::new(10),
                TimestampMillis::new(20),
                TimestampMillis::new(30),
            ]),
            "worker-a",
        );

        let result = worker
            .process_next_queued_input(principal)
            .await
            .expect("worker should process queued input");

        let LifeWorkerProcessResult::Completed {
            run_id: completed_run,
            ..
        } = result
        else {
            panic!("expected completed result");
        };
        assert_eq!(completed_run, run_id);
        let claim_worker_ids = store.claim_next_worker_ids.lock().expect("lock").clone();
        assert_eq!(claim_worker_ids, vec!["worker-a".to_owned(); 2]);
        assert_eq!(store.event_kinds(), vec!["run_started", "run_completed"]);
    }

    #[tokio::test]
    async fn execute_claimed_run_rejects_foreign_lease_before_side_effects() {
        let principal = PrincipalUserId::new(100505).expect("positive principal");
        let input_id = InputId::new_v4();
        let store = FakeWorkerStore::with_claim(principal, input_id);
        let mut claimed = store
            .claim
            .lock()
            .expect("lock")
            .take()
            .expect("claim should be present");
        let run_id = claimed.run.run_id;
        claimed.run.lease_owner = Some("other-worker".to_owned());

        let worker = LifeWorker::new_with_clock(
            store.clone(),
            RecordingExecutor::success(TimestampMillis::new(30)),
            FixedWorkerClock::new(Vec::<TimestampMillis>::new()),
            "worker-a",
        );

        let error = worker
            .execute_claimed_run(claimed)
            .await
            .expect_err("foreign lease owner should fail before execution");
        assert!(matches!(
            error,
            LifeWorkerError::LostLease { run_id: observed } if observed == run_id
        ));
        assert!(store.events.lock().expect("lock").is_empty());
        assert!(store.consumed_inputs.lock().expect("lock").is_empty());
        assert!(store.completed_runs.lock().expect("lock").is_empty());
        assert!(store.failed_runs.lock().expect("lock").is_empty());
    }

    #[derive(Clone)]
    struct FakeWorkerStore {
        claim: Arc<Mutex<Option<ClaimedLifeInputRun>>>,
        events: Arc<Mutex<Vec<LifeEvent>>>,
        completed_runs: Arc<Mutex<Vec<RunId>>>,
        failed_runs: Arc<Mutex<Vec<RunId>>>,
        cancelled_runs: Arc<Mutex<Vec<RunId>>>,
        consumed_inputs: Arc<Mutex<Vec<InputId>>>,
        queued_claims: Arc<Mutex<Vec<ClaimedLifeInputRun>>>,
        claim_next_worker_ids: Arc<Mutex<Vec<String>>>,
    }

    impl FakeWorkerStore {
        fn with_claim(principal_user_id: PrincipalUserId, input_id: InputId) -> Self {
            let now = TimestampMillis::new(10);
            let run = LifeRun {
                run_id: RunId::new_v4(),
                principal_user_id,
                status: crate::domain::LifeRunStatus::Running,
                started_at: Some(now),
                finished_at: None,
                last_checkpoint_at: None,
                error_text: None,
                lease_owner: Some("worker-a".to_owned()),
                lease_expires_at: Some(TimestampMillis::new(now.get() + LIFE_RUN_LEASE_MILLIS)),
                last_heartbeat_at: Some(now),
                created_at: now,
                updated_at: now,
            };
            let input = LifeInput {
                input_id,
                principal_user_id,
                turn_id: TurnId::new_v4(),
                status: LifeInputStatus::Claimed,
                claimed_by: Some("worker-a".to_owned()),
                claimed_at: Some(now),
                created_at: now,
                updated_at: now,
            };
            Self::new(Some(ClaimedLifeInputRun {
                input,
                run,
                user_content: "test user content".to_owned(),
            }))
        }

        fn without_claim() -> Self {
            Self::new(None)
        }

        fn new(claim: Option<ClaimedLifeInputRun>) -> Self {
            Self {
                claim: Arc::new(Mutex::new(claim)),
                events: Arc::new(Mutex::new(Vec::new())),
                completed_runs: Arc::new(Mutex::new(Vec::new())),
                failed_runs: Arc::new(Mutex::new(Vec::new())),
                cancelled_runs: Arc::new(Mutex::new(Vec::new())),
                consumed_inputs: Arc::new(Mutex::new(Vec::new())),
                queued_claims: Arc::new(Mutex::new(Vec::new())),
                claim_next_worker_ids: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn event_kinds(&self) -> Vec<String> {
            self.events
                .lock()
                .expect("lock")
                .iter()
                .map(|event| event.kind.clone())
                .collect()
        }
    }

    #[async_trait]
    impl LifeWorkerStore for FakeWorkerStore {
        async fn claim_input_and_start_run(
            &self,
            _principal_user_id: PrincipalUserId,
            _input_id: InputId,
            _run_id: RunId,
            _worker_id: &str,
            _now: TimestampMillis,
        ) -> LifeWorkerResult<Option<ClaimedLifeInputRun>> {
            Ok(self.claim.lock().expect("lock").take())
        }

        async fn mark_input_consumed(
            &self,
            input_id: InputId,
            _now: TimestampMillis,
        ) -> LifeWorkerResult<()> {
            self.consumed_inputs.lock().expect("lock").push(input_id);
            Ok(())
        }

        async fn claim_next_queued_input_and_start_run(
            &self,
            _principal_user_id: PrincipalUserId,
            _run_id: RunId,
            worker_id: &str,
            _now: TimestampMillis,
        ) -> LifeWorkerResult<Option<ClaimedLifeInputRun>> {
            self.claim_next_worker_ids
                .lock()
                .expect("lock")
                .push(worker_id.to_owned());
            Ok(self.queued_claims.lock().expect("lock").pop())
        }

        async fn heartbeat_run_lease(
            &self,
            _run_id: RunId,
            _worker_id: &str,
            _now: TimestampMillis,
        ) -> LifeWorkerResult<bool> {
            Ok(true)
        }

        async fn append_event(&self, event: &LifeEvent) -> LifeWorkerResult<()> {
            self.events.lock().expect("lock").push(event.clone());
            Ok(())
        }

        async fn next_event_seq(&self, _run_id: RunId) -> LifeWorkerResult<i64> {
            let len = self.events.lock().expect("lock").len();
            i64::try_from(len).map_err(|error| LifeWorkerError::Clock(error.to_string()))
        }

        async fn complete_run(
            &self,
            run_id: RunId,
            _finished_at: TimestampMillis,
            _last_checkpoint_at: TimestampMillis,
        ) -> LifeWorkerResult<bool> {
            self.completed_runs.lock().expect("lock").push(run_id);
            Ok(true)
        }

        async fn fail_run(
            &self,
            run_id: RunId,
            _finished_at: TimestampMillis,
            _error_text: &str,
        ) -> LifeWorkerResult<bool> {
            self.failed_runs.lock().expect("lock").push(run_id);
            Ok(true)
        }

        async fn cancel_run(
            &self,
            _principal_user_id: PrincipalUserId,
            run_id: RunId,
            _cancelled_at: TimestampMillis,
        ) -> LifeWorkerResult<CancelLifeRunOutcome> {
            self.cancelled_runs.lock().expect("lock").push(run_id);
            Ok(CancelLifeRunOutcome::Cancelled)
        }
    }

    struct RecordingExecutor {
        outcome: Result<LifeRunExecutionOutcome, LifeWorkerError>,
        seen_context: Arc<Mutex<Option<LifeWorkerRunContext>>>,
        seen_contexts: Arc<Mutex<Vec<LifeWorkerRunContext>>>,
    }

    impl RecordingExecutor {
        fn success(final_checkpoint_at: TimestampMillis) -> Self {
            Self {
                outcome: Ok(LifeRunExecutionOutcome {
                    final_checkpoint_at,
                }),
                seen_context: Arc::new(Mutex::new(None)),
                seen_contexts: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn failure(message: &str) -> Self {
            Self {
                outcome: Err(LifeWorkerError::Executor(message.to_owned())),
                seen_context: Arc::new(Mutex::new(None)),
                seen_contexts: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn cancelled(run_id: RunId) -> Self {
            Self {
                outcome: Err(LifeWorkerError::Cancelled { run_id }),
                seen_context: Arc::new(Mutex::new(None)),
                seen_contexts: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl LifeRunExecutor for RecordingExecutor {
        async fn execute_life_run(
            &self,
            context: LifeWorkerRunContext,
        ) -> LifeWorkerResult<LifeRunExecutionOutcome> {
            *self.seen_context.lock().expect("lock") = Some(context);
            self.seen_contexts.lock().expect("lock").push(
                self.seen_context
                    .lock()
                    .expect("lock")
                    .clone()
                    .expect("context"),
            );
            match &self.outcome {
                Ok(outcome) => Ok(outcome.clone()),
                Err(LifeWorkerError::Cancelled { run_id }) => {
                    Err(LifeWorkerError::Cancelled { run_id: *run_id })
                }
                Err(error) => Err(LifeWorkerError::Executor(error.to_string())),
            }
        }
    }

    struct FixedWorkerClock {
        values: Mutex<Vec<TimestampMillis>>,
    }

    impl FixedWorkerClock {
        fn new(values: impl Into<Vec<TimestampMillis>>) -> Self {
            let mut values = values.into();
            values.reverse();
            Self {
                values: Mutex::new(values),
            }
        }
    }

    impl LifeWorkerClock for FixedWorkerClock {
        fn now(&self) -> LifeWorkerResult<TimestampMillis> {
            self.values
                .lock()
                .expect("lock")
                .pop()
                .ok_or_else(|| LifeWorkerError::Clock("fixed clock exhausted".to_owned()))
        }
    }
}
