//! DB-backed life worker contracts and orchestration skeleton.
//!
//! The worker owns run state. Transports and the gateway submit only inputs; this
//! module claims queued input from Postgres, starts a persisted run under the
//! active generation, exposes the stable life hot-memory scope, and records
//! transport-neutral run events.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::domain::{
    EventId, InputId, LifeEvent, LifeInput, MemoryScope, PrincipalUserId, RunId, TimestampMillis,
    TurnId,
};
use crate::storage::{ClaimedLifeInputRun, LifeStorageError, LifeStorageRepository};

/// Stable life context key for hot-memory checkpoints.
pub const LIFE_CONTEXT_KEY: &str = "life";

/// Stable life flow id for the main permanent-life thread.
pub const LIFE_FLOW_ID: &str = "main";

/// Command to process a queued principal input.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessPrincipalInput {
    /// Principal owner.
    pub principal_user_id: PrincipalUserId,
    /// Input id to process.
    pub input_id: InputId,
}

/// Stable hot-memory scope for life-mode final checkpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StableLifeMemoryScope {
    /// User/principal id used by `agent_memory_snapshots.user_id`.
    pub user_id: i64,
    /// Stable life context key.
    pub context_key: String,
    /// Stable life flow id.
    pub flow_id: String,
}

impl StableLifeMemoryScope {
    /// Builds the PRD-mandated stable life scope.
    #[must_use]
    pub fn for_principal(principal_user_id: PrincipalUserId) -> Self {
        Self {
            user_id: principal_user_id.get(),
            context_key: LIFE_CONTEXT_KEY.to_owned(),
            flow_id: LIFE_FLOW_ID.to_owned(),
        }
    }
}

/// Claimed run context after the worker has loaded the active generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedLifeRun {
    /// Run id.
    pub run_id: RunId,
    /// Active scope for memory reads in this run.
    pub memory_scope: MemoryScope,
    /// Stable hot-memory checkpoint scope.
    pub stable_memory_scope: StableLifeMemoryScope,
    /// Claimed input that started the run.
    pub input: LifeInput,
}

impl From<ClaimedLifeInputRun> for ClaimedLifeRun {
    fn from(value: ClaimedLifeInputRun) -> Self {
        Self {
            run_id: value.run.run_id,
            memory_scope: MemoryScope::new(
                value.run.principal_user_id,
                value.run.memory_generation_id,
            ),
            stable_memory_scope: StableLifeMemoryScope::for_principal(value.run.principal_user_id),
            input: value.input,
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
    /// Opaque agent memory snapshot to persist under the stable life scope.
    pub final_memory: Value,
    /// Snapshot schema version.
    pub final_memory_schema_version: i32,
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
        /// Active memory scope used by the run.
        memory_scope: MemoryScope,
        /// Stable hot-memory checkpoint scope.
        stable_memory_scope: StableLifeMemoryScope,
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

    /// Drains queued inputs for a principal at a runtime safe boundary.
    async fn drain_queued_inputs_for_run(
        &self,
        principal_user_id: PrincipalUserId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeWorkerResult<Vec<LifeInput>>;

    /// Links a transcript turn to a run by setting `life_turns.run_id`.
    async fn link_turn_to_run(&self, turn_id: TurnId, run_id: RunId) -> LifeWorkerResult<()>;

    /// Synchronously persists the stable life hot-memory checkpoint.
    async fn save_life_memory_checkpoint(
        &self,
        stable_scope: &StableLifeMemoryScope,
        memory: &Value,
        schema_version: i32,
        now: TimestampMillis,
    ) -> LifeWorkerResult<()>;

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
    ) -> LifeWorkerResult<()>;

    /// Marks a run failed.
    async fn fail_run(
        &self,
        run_id: RunId,
        finished_at: TimestampMillis,
        error_text: &str,
    ) -> LifeWorkerResult<()>;
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

    async fn drain_queued_inputs_for_run(
        &self,
        principal_user_id: PrincipalUserId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeWorkerResult<Vec<LifeInput>> {
        LifeStorageRepository::drain_queued_inputs_for_run(self, principal_user_id, worker_id, now)
            .await
            .map_err(Into::into)
    }

    async fn link_turn_to_run(&self, turn_id: TurnId, run_id: RunId) -> LifeWorkerResult<()> {
        LifeStorageRepository::link_turn_to_run(self, turn_id, run_id)
            .await
            .map_err(Into::into)
    }

    async fn save_life_memory_checkpoint(
        &self,
        stable_scope: &StableLifeMemoryScope,
        memory: &Value,
        schema_version: i32,
        now: TimestampMillis,
    ) -> LifeWorkerResult<()> {
        let principal_user_id =
            PrincipalUserId::new(stable_scope.user_id).map_err(LifeStorageError::Domain)?;
        LifeStorageRepository::save_life_memory_checkpoint(
            self,
            principal_user_id,
            &stable_scope.context_key,
            &stable_scope.flow_id,
            memory,
            schema_version,
            now,
        )
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
    ) -> LifeWorkerResult<()> {
        LifeStorageRepository::complete_run(self, run_id, finished_at, last_checkpoint_at)
            .await
            .map_err(Into::into)
    }

    async fn fail_run(
        &self,
        run_id: RunId,
        finished_at: TimestampMillis,
        error_text: &str,
    ) -> LifeWorkerResult<()> {
        LifeStorageRepository::fail_run(self, run_id, finished_at, error_text)
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

    /// Executes an already-claimed run to completion.
    ///
    /// This is the entry point for the runtime handle path: the handle claims
    /// the input and starts the run, then the caller spawns this method.
    /// The method drains follow-up queued inputs at the start of the run,
    /// links their turns, executes via the executor seam, persists the final
    /// checkpoint, and marks the run completed.
    pub async fn execute_claimed_run(
        &self,
        claimed: ClaimedLifeInputRun,
    ) -> LifeWorkerResult<LifeWorkerProcessResult> {
        let started_at = match claimed.run.started_at {
            Some(ts) => ts,
            None => self.clock.now()?,
        };
        let claimed_run = ClaimedLifeRun::from(claimed);
        let principal_user_id = claimed_run.input.principal_user_id;

        // Drain follow-up inputs queued while the run was being claimed.
        // Each drained input's turn is linked to this run for activity rendering.
        let drained = self
            .store
            .drain_queued_inputs_for_run(principal_user_id, &self.worker_id, started_at)
            .await?;
        for drained_input in &drained {
            self.store
                .link_turn_to_run(drained_input.turn_id, claimed_run.run_id)
                .await?;
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

        match self.executor.execute_life_run(context).await {
            Ok(outcome) => {
                let finished_at = self.clock.now()?;
                self.store
                    .save_life_memory_checkpoint(
                        &claimed_run.stable_memory_scope,
                        &outcome.final_memory,
                        outcome.final_memory_schema_version,
                        outcome.final_checkpoint_at,
                    )
                    .await?;
                self.store
                    .mark_input_consumed(claimed_run.input.input_id, finished_at)
                    .await?;
                self.store
                    .complete_run(claimed_run.run_id, finished_at, outcome.final_checkpoint_at)
                    .await?;
                self.append_event(claimed_run.run_id, "run_completed", finished_at)
                    .await?;
                Ok(LifeWorkerProcessResult::Completed {
                    run_id: claimed_run.run_id,
                    memory_scope: claimed_run.memory_scope,
                    stable_memory_scope: claimed_run.stable_memory_scope,
                })
            }
            Err(error) => {
                let error_text = error.to_string();
                let finished_at = self.clock.now()?;
                self.store
                    .fail_run(claimed_run.run_id, finished_at, &error_text)
                    .await?;
                self.append_event(claimed_run.run_id, "run_failed", finished_at)
                    .await?;
                Err(LifeWorkerError::Executor(error_text))
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
    use crate::domain::{LifeInputStatus, LifeRun, MemoryGenerationId, TurnId};

    type RecordedCheckpoint = (StableLifeMemoryScope, Value, i32, TimestampMillis);

    #[tokio::test]
    async fn worker_claims_run_and_uses_stable_life_scope() {
        let principal = PrincipalUserId::new(100500).expect("positive principal");
        let generation = MemoryGenerationId::new_v4();
        let input_id = InputId::new_v4();
        let store = FakeWorkerStore::with_claim(principal, generation, input_id);
        let executor = RecordingExecutor::success(TimestampMillis::new(30));
        let seen_context = Arc::clone(&executor.seen_context);
        let worker = LifeWorker::new_with_clock(
            store.clone(),
            executor,
            FixedWorkerClock::new([TimestampMillis::new(10), TimestampMillis::new(20)]),
            "worker-a",
        );

        let result = worker
            .process_principal_input(ProcessPrincipalInput {
                principal_user_id: principal,
                input_id,
            })
            .await
            .expect("worker should process input");

        let LifeWorkerProcessResult::Completed {
            memory_scope,
            stable_memory_scope,
            ..
        } = result
        else {
            panic!("expected completed result");
        };
        assert_eq!(memory_scope, MemoryScope::new(principal, generation));
        assert_eq!(stable_memory_scope.user_id, principal.get());
        assert_eq!(stable_memory_scope.context_key, LIFE_CONTEXT_KEY);
        assert_eq!(stable_memory_scope.flow_id, LIFE_FLOW_ID);
        assert_eq!(store.completed_runs.lock().expect("lock").len(), 1);
        assert_eq!(*store.consumed_inputs.lock().expect("lock"), vec![input_id]);
        let checkpoints = store.checkpoints.lock().expect("lock");
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].0.context_key, LIFE_CONTEXT_KEY);
        assert_eq!(checkpoints[0].0.flow_id, LIFE_FLOW_ID);
        assert_eq!(checkpoints[0].1, json!({"checkpoint": "final"}));
        assert_eq!(checkpoints[0].2, 1);
        assert_eq!(checkpoints[0].3, TimestampMillis::new(30));
        assert_eq!(store.event_kinds(), vec!["run_started", "run_completed"]);

        let context = seen_context
            .lock()
            .expect("lock")
            .clone()
            .expect("executor should see context");
        assert_eq!(
            context.run.memory_scope,
            MemoryScope::new(principal, generation)
        );
        assert_eq!(
            context.run.stable_memory_scope.context_key,
            LIFE_CONTEXT_KEY
        );
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
        let generation = MemoryGenerationId::new_v4();
        let input_id = InputId::new_v4();
        let store = FakeWorkerStore::with_claim(principal, generation, input_id);
        let worker = LifeWorker::new_with_clock(
            store.clone(),
            RecordingExecutor::failure("executor boom"),
            FixedWorkerClock::new([TimestampMillis::new(10), TimestampMillis::new(20)]),
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
    async fn execute_claimed_run_drains_follow_up_inputs_and_links_turns() {
        let principal = PrincipalUserId::new(100503).expect("positive principal");
        let generation = MemoryGenerationId::new_v4();
        let input_id = InputId::new_v4();
        let store = FakeWorkerStore::with_claim(principal, generation, input_id);

        // Simulate a follow-up input that was queued while the run was being claimed.
        let follow_up_turn_id = TurnId::new_v4();
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
            .drained_inputs
            .lock()
            .expect("lock")
            .push(follow_up_input);

        // Extract the claimed run for direct execute_claimed_run call.
        let claimed = store
            .claim
            .lock()
            .expect("lock")
            .take()
            .expect("claim should be present");
        let expected_run_id = claimed.run.run_id;

        let worker = LifeWorker::new_with_clock(
            store.clone(),
            RecordingExecutor::success(TimestampMillis::new(30)),
            FixedWorkerClock::new([TimestampMillis::new(20)]),
            "worker-a",
        );

        let result = worker
            .execute_claimed_run(claimed)
            .await
            .expect("worker should execute claimed run");

        let LifeWorkerProcessResult::Completed { run_id, .. } = result else {
            panic!("expected completed result");
        };
        assert_eq!(run_id, expected_run_id);

        // Follow-up input's turn should be linked to the run.
        let linked = store.linked_turns.lock().expect("lock").clone();
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].0, follow_up_turn_id);
        assert_eq!(linked[0].1, expected_run_id);

        assert_eq!(store.event_kinds(), vec!["run_started", "run_completed"]);
    }

    #[derive(Clone)]
    struct FakeWorkerStore {
        claim: Arc<Mutex<Option<ClaimedLifeInputRun>>>,
        events: Arc<Mutex<Vec<LifeEvent>>>,
        completed_runs: Arc<Mutex<Vec<RunId>>>,
        failed_runs: Arc<Mutex<Vec<RunId>>>,
        consumed_inputs: Arc<Mutex<Vec<InputId>>>,
        checkpoints: Arc<Mutex<Vec<RecordedCheckpoint>>>,
        drained_inputs: Arc<Mutex<Vec<LifeInput>>>,
        linked_turns: Arc<Mutex<Vec<(TurnId, RunId)>>>,
    }

    impl FakeWorkerStore {
        fn with_claim(
            principal_user_id: PrincipalUserId,
            memory_generation_id: MemoryGenerationId,
            input_id: InputId,
        ) -> Self {
            let now = TimestampMillis::new(10);
            let run = LifeRun {
                run_id: RunId::new_v4(),
                principal_user_id,
                memory_generation_id,
                status: crate::domain::LifeRunStatus::Running,
                started_at: Some(now),
                finished_at: None,
                last_checkpoint_at: None,
                error_text: None,
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
            Self::new(Some(ClaimedLifeInputRun { input, run }))
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
                consumed_inputs: Arc::new(Mutex::new(Vec::new())),
                checkpoints: Arc::new(Mutex::new(Vec::new())),
                drained_inputs: Arc::new(Mutex::new(Vec::new())),
                linked_turns: Arc::new(Mutex::new(Vec::new())),
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

        async fn drain_queued_inputs_for_run(
            &self,
            _principal_user_id: PrincipalUserId,
            _worker_id: &str,
            _now: TimestampMillis,
        ) -> LifeWorkerResult<Vec<LifeInput>> {
            let drained = self
                .drained_inputs
                .lock()
                .expect("lock")
                .drain(..)
                .collect();
            Ok(drained)
        }

        async fn link_turn_to_run(&self, turn_id: TurnId, run_id: RunId) -> LifeWorkerResult<()> {
            self.linked_turns
                .lock()
                .expect("lock")
                .push((turn_id, run_id));
            Ok(())
        }

        async fn save_life_memory_checkpoint(
            &self,
            stable_scope: &StableLifeMemoryScope,
            memory: &Value,
            schema_version: i32,
            now: TimestampMillis,
        ) -> LifeWorkerResult<()> {
            self.checkpoints.lock().expect("lock").push((
                stable_scope.clone(),
                memory.clone(),
                schema_version,
                now,
            ));
            Ok(())
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
        ) -> LifeWorkerResult<()> {
            self.completed_runs.lock().expect("lock").push(run_id);
            Ok(())
        }

        async fn fail_run(
            &self,
            run_id: RunId,
            _finished_at: TimestampMillis,
            _error_text: &str,
        ) -> LifeWorkerResult<()> {
            self.failed_runs.lock().expect("lock").push(run_id);
            Ok(())
        }
    }

    struct RecordingExecutor {
        outcome: Result<LifeRunExecutionOutcome, LifeWorkerError>,
        seen_context: Arc<Mutex<Option<LifeWorkerRunContext>>>,
    }

    impl RecordingExecutor {
        fn success(final_checkpoint_at: TimestampMillis) -> Self {
            Self {
                outcome: Ok(LifeRunExecutionOutcome {
                    final_checkpoint_at,
                    final_memory: json!({"checkpoint": "final"}),
                    final_memory_schema_version: 1,
                }),
                seen_context: Arc::new(Mutex::new(None)),
            }
        }

        fn failure(message: &str) -> Self {
            Self {
                outcome: Err(LifeWorkerError::Executor(message.to_owned())),
                seen_context: Arc::new(Mutex::new(None)),
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
            match &self.outcome {
                Ok(outcome) => Ok(outcome.clone()),
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
