//! Life runtime handle: the composition point between the gateway and the worker.
//!
//! The gateway writes turns and queues inputs. The runtime handle wakes the
//! worker by claiming the input and starting a run. If a run is already active,
//! the input remains queued and the active run id is returned.
//!
//! The handle does not spawn execution — it returns [`WakeOutcome::Started`]
//! with the claimed run so the caller (transport binary) can spawn the worker
//! in its own tokio runtime. This keeps `oxide-agent-life` free of a tokio
//! dependency.

use async_trait::async_trait;
use thiserror::Error;

use crate::domain::{InputId, LifeRun, PrincipalUserId, RunId, TimestampMillis};
use crate::storage::{ClaimedLifeInputRun, LifeStorageError, LifeStorageRepository};
use crate::worker::{LifeWorkerClock, LifeWorkerError};

/// Outcome of waking the life runtime for a queued input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeOutcome {
    /// A new run was started. The caller should spawn the worker to execute
    /// the claimed run.
    Started {
        /// Newly started run id.
        run_id: RunId,
        /// Claimed input + run context for the worker. Boxed to keep the
        /// enum small since `ClaimedLifeInputRun` is a large struct.
        claimed: Box<ClaimedLifeInputRun>,
    },
    /// The input remains queued because another run is already active. The
    /// active worker will claim it as a separate run after the current run
    /// completes.
    AttachedToActive {
        /// Active run id after which this input can be claimed as its own run.
        run_id: RunId,
    },
}

/// Runtime errors.
#[derive(Debug, Error)]
pub enum LifeRuntimeError {
    /// Durable storage failed.
    #[error(transparent)]
    Storage(#[from] LifeStorageError),
    /// Clock failed.
    #[error("life runtime clock error: {0}")]
    Clock(String),
    /// The input could not be claimed and no active run was found. This
    /// indicates the input was already consumed or vanished between queue
    /// and wake.
    #[error(
        "life input {input_id} was not claimed and no active run exists for principal {principal_user_id}"
    )]
    NotClaimedAndNoActiveRun {
        /// Input that could not be claimed.
        input_id: InputId,
        /// Principal without an active run.
        principal_user_id: PrincipalUserId,
    },
}

/// Result alias for runtime operations.
pub type LifeRuntimeResult<T> = Result<T, LifeRuntimeError>;

/// Narrow store boundary for the runtime handle.
#[async_trait]
pub trait LifeRuntimeStore: Send + Sync {
    /// Atomically claim a queued input and start a running run.
    async fn claim_input_and_start_run(
        &self,
        principal_user_id: PrincipalUserId,
        input_id: InputId,
        run_id: RunId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeRuntimeResult<Option<ClaimedLifeInputRun>>;

    /// Find the currently running run for a principal, if any.
    async fn find_active_run(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeRuntimeResult<Option<LifeRun>>;
}

#[async_trait]
impl<T> LifeRuntimeStore for T
where
    T: LifeStorageRepository + Send + Sync,
{
    async fn claim_input_and_start_run(
        &self,
        principal_user_id: PrincipalUserId,
        input_id: InputId,
        run_id: RunId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeRuntimeResult<Option<ClaimedLifeInputRun>> {
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

    async fn find_active_run(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeRuntimeResult<Option<LifeRun>> {
        LifeStorageRepository::find_active_run(self, principal_user_id)
            .await
            .map_err(Into::into)
    }
}

/// Life runtime handle owned by the transport binary.
///
/// The handle is the bridge between the gateway (which queues inputs) and the
/// worker (which executes runs). It claims queued inputs, starts runs, links
/// turns, and returns the outcome so the caller can spawn execution.
pub struct LifeRuntimeHandle<S, C = crate::worker::SystemLifeWorkerClock> {
    store: S,
    clock: C,
    worker_id: String,
}

impl<S> LifeRuntimeHandle<S, crate::worker::SystemLifeWorkerClock> {
    /// Creates a handle with the system clock.
    #[must_use]
    pub fn new(store: S, worker_id: impl Into<String>) -> Self {
        Self {
            store,
            clock: crate::worker::SystemLifeWorkerClock,
            worker_id: worker_id.into(),
        }
    }
}

impl<S, C> LifeRuntimeHandle<S, C> {
    /// Creates a handle with an explicit clock.
    #[must_use]
    pub const fn new_with_clock(store: S, clock: C, worker_id: String) -> Self {
        Self {
            store,
            clock,
            worker_id,
        }
    }
}

fn clock_error(error: LifeWorkerError) -> LifeRuntimeError {
    LifeRuntimeError::Clock(error.to_string())
}

impl<S, C> LifeRuntimeHandle<S, C>
where
    S: LifeRuntimeStore,
    C: LifeWorkerClock,
{
    /// Wakes the runtime to process a queued input.
    ///
    /// Tries to claim the input and start a new run. If a run is already
    /// active for this principal, the input stays queued and the active run
    /// id is returned. The originating turn is linked to the run on claim.
    pub async fn wake(
        &self,
        principal_user_id: PrincipalUserId,
        input_id: InputId,
    ) -> LifeRuntimeResult<WakeOutcome> {
        let now = self.clock.now().map_err(clock_error)?;
        let run_id = RunId::new_v4();

        match self
            .store
            .claim_input_and_start_run(principal_user_id, input_id, run_id, &self.worker_id, now)
            .await?
        {
            Some(claimed) => {
                let run_id = claimed.run.run_id;
                Ok(WakeOutcome::Started {
                    run_id,
                    claimed: Box::new(claimed),
                })
            }
            None => {
                let active = self.store.find_active_run(principal_user_id).await?;
                let run_id = active.map(|run| run.run_id).ok_or(
                    LifeRuntimeError::NotClaimedAndNoActiveRun {
                        input_id,
                        principal_user_id,
                    },
                )?;
                Ok(WakeOutcome::AttachedToActive { run_id })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::domain::{LifeInput, LifeInputStatus, LifeRun, LifeRunStatus, TurnId};
    use crate::storage::LifeStorageError;

    #[derive(Clone, Default)]
    struct FakeRuntimeStore {
        claim_result: Arc<Mutex<Option<ClaimedLifeInputRun>>>,
        active_run: Arc<Mutex<Option<LifeRun>>>,
        claim_called: Arc<Mutex<bool>>,
    }

    impl FakeRuntimeStore {
        fn with_claim(claimed: ClaimedLifeInputRun) -> Self {
            Self {
                claim_result: Arc::new(Mutex::new(Some(claimed))),
                active_run: Arc::new(Mutex::new(None)),
                claim_called: Arc::new(Mutex::new(false)),
            }
        }

        fn with_active_run(run: LifeRun) -> Self {
            Self {
                claim_result: Arc::new(Mutex::new(None)),
                active_run: Arc::new(Mutex::new(Some(run))),
                claim_called: Arc::new(Mutex::new(false)),
            }
        }
    }

    #[async_trait]
    impl LifeRuntimeStore for FakeRuntimeStore {
        async fn claim_input_and_start_run(
            &self,
            _principal_user_id: PrincipalUserId,
            _input_id: InputId,
            _run_id: RunId,
            _worker_id: &str,
            _now: TimestampMillis,
        ) -> LifeRuntimeResult<Option<ClaimedLifeInputRun>> {
            *self.claim_called.lock().expect("lock") = true;
            Ok(self.claim_result.lock().expect("lock").take())
        }

        async fn find_active_run(
            &self,
            _principal_user_id: PrincipalUserId,
        ) -> LifeRuntimeResult<Option<LifeRun>> {
            Ok(self.active_run.lock().expect("lock").clone())
        }
    }

    #[derive(Debug, Copy, Clone)]
    struct FixedClock(TimestampMillis);

    impl LifeWorkerClock for FixedClock {
        fn now(&self) -> Result<TimestampMillis, LifeWorkerError> {
            Ok(self.0)
        }
    }

    #[tokio::test]
    async fn wake_starts_new_run_and_links_turn() {
        let principal = PrincipalUserId::new(100500).expect("positive principal");
        let turn_id = TurnId::new_v4();
        let input_id = InputId::new_v4();
        let run_id = RunId::new_v4();
        let now = TimestampMillis::new(42);

        let claimed = ClaimedLifeInputRun {
            input: LifeInput {
                input_id,
                principal_user_id: principal,
                turn_id,
                status: LifeInputStatus::Claimed,
                claimed_by: Some("worker-a".to_owned()),
                claimed_at: Some(now),
                created_at: now,
                updated_at: now,
            },
            run: LifeRun {
                run_id,
                principal_user_id: principal,
                status: LifeRunStatus::Running,
                started_at: Some(now),
                finished_at: None,
                last_checkpoint_at: None,
                error_text: None,
                lease_owner: Some("worker-a".to_owned()),
                lease_expires_at: Some(TimestampMillis::new(now.get() + 900_000)),
                last_heartbeat_at: Some(now),
                created_at: now,
                updated_at: now,
            },
            user_content: "test user content".to_owned(),
        };

        let store = FakeRuntimeStore::with_claim(claimed);
        let handle = LifeRuntimeHandle::new_with_clock(
            store.clone(),
            FixedClock(now),
            "worker-a".to_owned(),
        );

        let outcome = handle
            .wake(principal, input_id)
            .await
            .expect("wake should start a run");

        let WakeOutcome::Started {
            run_id: returned_run_id,
            ..
        } = outcome
        else {
            panic!("expected Started outcome");
        };
        assert_eq!(returned_run_id, run_id);

        // Originating turn/run association is guaranteed by the storage claim
        // transaction; the runtime handle only exposes the claimed run to the
        // caller for execution.
    }

    #[tokio::test]
    async fn wake_returns_active_run_when_already_running() {
        let principal = PrincipalUserId::new(100501).expect("positive principal");
        let active_run_id = RunId::new_v4();
        let now = TimestampMillis::new(42);

        let active_run = LifeRun {
            run_id: active_run_id,
            principal_user_id: principal,
            status: LifeRunStatus::Running,
            started_at: Some(now),
            finished_at: None,
            last_checkpoint_at: None,
            error_text: None,
            lease_owner: Some("worker-a".to_owned()),
            lease_expires_at: Some(TimestampMillis::new(now.get() + 900_000)),
            last_heartbeat_at: Some(now),
            created_at: now,
            updated_at: now,
        };

        let store = FakeRuntimeStore::with_active_run(active_run);
        let handle = LifeRuntimeHandle::new_with_clock(
            store.clone(),
            FixedClock(now),
            "worker-a".to_owned(),
        );

        let outcome = handle
            .wake(principal, InputId::new_v4())
            .await
            .expect("wake should return active run");

        let WakeOutcome::AttachedToActive { run_id } = outcome else {
            panic!("expected AttachedToActive outcome");
        };
        assert_eq!(run_id, active_run_id);

        // No claim means no storage-side association is created by this handle.
    }

    #[tokio::test]
    async fn wake_errors_when_not_claimed_and_no_active_run() {
        let principal = PrincipalUserId::new(100502).expect("positive principal");
        let store = FakeRuntimeStore::default();
        let handle = LifeRuntimeHandle::new_with_clock(
            store,
            FixedClock(TimestampMillis::new(42)),
            "worker-a".to_owned(),
        );

        let error = handle
            .wake(principal, InputId::new_v4())
            .await
            .expect_err("should error when not claimed and no active run");

        assert!(matches!(
            error,
            LifeRuntimeError::NotClaimedAndNoActiveRun { .. }
        ));
    }

    #[tokio::test]
    async fn wake_propagates_storage_errors() {
        let principal = PrincipalUserId::new(100503).expect("positive principal");

        struct ErrorStore;
        #[async_trait]
        impl LifeRuntimeStore for ErrorStore {
            async fn claim_input_and_start_run(
                &self,
                _principal_user_id: PrincipalUserId,
                _input_id: InputId,
                _run_id: RunId,
                _worker_id: &str,
                _now: TimestampMillis,
            ) -> LifeRuntimeResult<Option<ClaimedLifeInputRun>> {
                Err(LifeStorageError::Database("connection lost".to_owned()).into())
            }
            async fn find_active_run(
                &self,
                _principal_user_id: PrincipalUserId,
            ) -> LifeRuntimeResult<Option<LifeRun>> {
                Ok(None)
            }
        }

        let handle = LifeRuntimeHandle::new_with_clock(
            ErrorStore,
            FixedClock(TimestampMillis::new(42)),
            "worker-a".to_owned(),
        );

        let error = handle
            .wake(principal, InputId::new_v4())
            .await
            .expect_err("should propagate storage error");

        assert!(matches!(error, LifeRuntimeError::Storage(_)));
    }
}
