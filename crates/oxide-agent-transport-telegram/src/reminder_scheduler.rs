//! In-memory reminder due queue for the Telegram transport.

use async_trait::async_trait;
use oxide_agent_core::agent::providers::{ReminderScheduleEvent, ReminderScheduleNotifier};
use oxide_agent_core::storage::{
    ReminderJobRecord, ReminderJobStatus, StorageError, StorageProvider,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

const DEFAULT_REBUILD_LIMIT: usize = 4_096;

type ReminderKey = (i64, String);

#[derive(Default)]
struct ReminderSchedulerState {
    records: HashMap<ReminderKey, ReminderJobRecord>,
}

/// Shared handle for the Telegram reminder due queue.
#[derive(Clone)]
pub struct ReminderSchedulerHandle {
    allowed_users: Arc<HashSet<i64>>,
    rebuild_limit: usize,
    state: Arc<Mutex<ReminderSchedulerState>>,
    wakeup: Arc<Notify>,
}

impl ReminderSchedulerHandle {
    /// Create a new empty reminder scheduler handle for the provided users.
    #[must_use]
    pub fn new<I>(allowed_users: I) -> Self
    where
        I: IntoIterator<Item = i64>,
    {
        Self {
            allowed_users: Arc::new(allowed_users.into_iter().collect()),
            rebuild_limit: DEFAULT_REBUILD_LIMIT,
            state: Arc::new(Mutex::new(ReminderSchedulerState::default())),
            wakeup: Arc::new(Notify::new()),
        }
    }

    /// Return the user ids covered by this scheduler.
    #[must_use]
    pub fn allowed_user_ids(&self) -> Vec<i64> {
        self.allowed_users.iter().copied().collect()
    }

    /// Rebuild the in-memory due queue from storage for all configured users.
    ///
    /// Returns the total number of scheduled reminder records tracked after the rebuild.
    pub async fn bootstrap_from_storage(
        &self,
        storage: &Arc<dyn StorageProvider>,
    ) -> Result<usize, StorageError> {
        let mut total = 0;
        for user_id in self.allowed_user_ids() {
            total += self.reconcile_user_from_storage(storage, user_id).await?;
        }
        Ok(total)
    }

    /// Rebuild the in-memory due queue for a single user.
    ///
    /// Returns the number of scheduled reminder records tracked for that user.
    pub async fn reconcile_user_from_storage(
        &self,
        storage: &Arc<dyn StorageProvider>,
        user_id: i64,
    ) -> Result<usize, StorageError> {
        if !self.allowed_users.contains(&user_id) {
            return Ok(0);
        }

        let records = storage
            .list_reminder_jobs(
                user_id,
                None,
                Some(vec![ReminderJobStatus::Scheduled]),
                self.rebuild_limit,
            )
            .await?;

        let mut state = self.state.lock().await;
        state
            .records
            .retain(|(record_user_id, _), _| *record_user_id != user_id);
        let tracked = records.len();
        for record in records {
            state
                .records
                .insert((record.user_id, record.reminder_id.clone()), record);
        }
        drop(state);
        self.wakeup.notify_one();
        Ok(tracked)
    }

    /// Insert or update a reminder record in the in-memory due queue.
    pub async fn upsert_record(&self, record: ReminderJobRecord) {
        if !self.allowed_users.contains(&record.user_id) {
            return;
        }

        let mut state = self.state.lock().await;
        let key = (record.user_id, record.reminder_id.clone());
        if record.status == ReminderJobStatus::Scheduled {
            state.records.insert(key, record);
        } else {
            state.records.remove(&key);
        }
        drop(state);
        self.wakeup.notify_one();
    }

    /// Remove a reminder record from the in-memory due queue.
    pub async fn delete_record(&self, user_id: i64, reminder_id: &str) {
        let mut state = self.state.lock().await;
        state.records.remove(&(user_id, reminder_id.to_string()));
        drop(state);
        self.wakeup.notify_one();
    }

    /// Return the next due timestamp currently tracked in memory.
    pub async fn next_due_at(&self) -> Option<i64> {
        let state = self.state.lock().await;
        state
            .records
            .values()
            .map(|record| record.next_run_at)
            .min()
    }

    /// Return up to `limit` reminders that are due at `now`, ordered by earliest run time.
    pub async fn take_due_batch(&self, now: i64, limit: usize) -> Vec<ReminderJobRecord> {
        let state = self.state.lock().await;
        let mut due: Vec<_> = state
            .records
            .values()
            .filter(|record| record.is_due(now))
            .cloned()
            .collect();
        due.sort_by(|left, right| {
            left.next_run_at
                .cmp(&right.next_run_at)
                .then_with(|| left.created_at.cmp(&right.created_at))
        });
        due.truncate(limit);
        due
    }

    /// Return the number of reminders currently tracked in memory.
    pub async fn tracked_count(&self) -> usize {
        let state = self.state.lock().await;
        state.records.len()
    }

    /// Wait until the scheduler receives an in-memory change notification.
    pub async fn wait_for_change(&self) {
        self.wakeup.notified().await;
    }
}

#[async_trait]
impl ReminderScheduleNotifier for ReminderSchedulerHandle {
    async fn notify(&self, event: ReminderScheduleEvent) {
        match event {
            ReminderScheduleEvent::Upsert(record) => self.upsert_record(*record).await,
            ReminderScheduleEvent::Delete {
                user_id,
                reminder_id,
            } => self.delete_record(user_id, &reminder_id).await,
        }
    }
}
