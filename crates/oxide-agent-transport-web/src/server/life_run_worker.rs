//! Background polling worker for life run execution.
//!
//! The web process is the sole executor for life mode. Inputs submitted via
//! the web UI are woken inline by the HTTP route. Inputs submitted by other
//! transports (e.g. Telegram) are written to the shared Postgres database but
//! nobody wakes the executor. This worker polls for queued inputs across all
//! principals and asks [`LifeWorker`] to claim and execute them with its own
//! durable lease owner id.

use oxide_agent_life::storage::{LifeStorageRepository, SqlxLifeStorage};
use oxide_agent_life::worker::{LifeWorker, SystemLifeWorkerClock};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};

use super::AppState;
use super::types::LifeExecutor;

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
            let worker = life_worker.clone();
            tokio::spawn(async move {
                if let Err(err) = worker.process_next_queued_input(principal).await {
                    error!("Life run worker processing failed for principal {principal}: {err}");
                }
            });
        }
    }
}
