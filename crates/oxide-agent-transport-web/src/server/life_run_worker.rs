//! Background polling worker for life run execution.
//!
//! The web process is the sole executor for life mode. Inputs submitted via
//! the web UI are woken inline by the HTTP route. Inputs submitted by other
//! transports (e.g. Telegram) are written to the shared Postgres database but
//! nobody wakes the executor. This worker polls for queued inputs across all
//! principals, claims them, and spawns execution via [`LifeWorker`].

use oxide_agent_life::domain::{RunId, TimestampMillis};
use oxide_agent_life::storage::{LifeStorageRepository, SqlxLifeStorage};
use oxide_agent_life::worker::{LifeWorker, SystemLifeWorkerClock};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};

use super::AppState;
use super::types::LifeExecutor;

const LIFE_RUN_WORKER_ID: &str = "web-life-run-worker";
const LIFE_RUN_WORKER_IDLE_SLEEP: Duration = Duration::from_secs(2);

/// Spawns the background life run polling worker.
///
/// Does nothing if life storage or worker are not configured (non-sqlx mode).
pub(crate) fn spawn_life_run_worker(state: AppState) {
    let Some(life_storage) = state.life_storage() else {
        return;
    };
    let Some(life_worker) = state.life_worker() else {
        return;
    };

    let life_storage = life_storage.as_ref().clone();
    tokio::spawn(async move {
        info!("Life run worker started");
        run_life_run_loop(life_storage, life_worker).await;
    });
}

async fn run_life_run_loop(
    life_storage: SqlxLifeStorage,
    life_worker: Arc<LifeWorker<SqlxLifeStorage, LifeExecutor, SystemLifeWorkerClock>>,
) {
    loop {
        tokio::time::sleep(LIFE_RUN_WORKER_IDLE_SLEEP).await;

        let principals = match life_storage.find_principals_with_queued_inputs().await {
            Ok(principals) => principals,
            Err(err) => {
                error!("Life run worker poll query failed: {err}");
                continue;
            }
        };

        for principal in principals {
            let now = now_millis();
            let run_id = RunId::new_v4();

            match LifeStorageRepository::claim_next_queued_input_and_start_run(
                &life_storage,
                principal,
                run_id,
                LIFE_RUN_WORKER_ID,
                now,
            )
            .await
            {
                Ok(Some(claimed)) => {
                    let worker = life_worker.clone();
                    tokio::spawn(async move {
                        if let Err(err) = worker.execute_claimed_run(claimed).await {
                            error!("Life run execution failed for principal {principal}: {err}");
                        }
                    });
                }
                Ok(None) => {
                    // No queued input or an active run already exists — the
                    // active run's worker will chain to follow-up inputs
                    // after completion.
                }
                Err(err) => {
                    error!("Life run worker claim failed for principal {principal}: {err}");
                }
            }
        }
    }
}

fn now_millis() -> TimestampMillis {
    TimestampMillis::new(chrono::Utc::now().timestamp_millis())
}
