use super::{
    build_reminder_job_record, current_timestamp_unix_secs, reminder_job_key, reminder_jobs_prefix,
    with_next_reminder_version, CreateReminderJobOptions, R2Storage, ReminderJobRecord,
    ReminderJobStatus, StorageError,
};
use uuid::Uuid;

impl R2Storage {
    pub(super) async fn create_reminder_job_inner(
        &self,
        options: CreateReminderJobOptions,
    ) -> Result<ReminderJobRecord, StorageError> {
        let reminder_id = Uuid::new_v4().to_string();
        let key = reminder_job_key(options.user_id, &reminder_id);
        let now = current_timestamp_unix_secs();
        let record = build_reminder_job_record(options, reminder_id, now);
        self.save_json(&key, &record).await?;
        Ok(record)
    }

    pub(super) async fn get_reminder_job_inner(
        &self,
        user_id: i64,
        reminder_id: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.load_json(&reminder_job_key(user_id, &reminder_id))
            .await
    }

    pub(super) async fn list_reminder_jobs_inner(
        &self,
        user_id: i64,
        context_key: Option<String>,
        statuses: Option<Vec<ReminderJobStatus>>,
        limit: usize,
    ) -> Result<Vec<ReminderJobRecord>, StorageError> {
        let mut records = self
            .list_json_under_prefix::<ReminderJobRecord>(&reminder_jobs_prefix(user_id))
            .await?;

        if let Some(context_key) = context_key.as_ref() {
            records.retain(|record| record.context_key == *context_key);
        }

        if let Some(statuses) = statuses.as_ref() {
            records.retain(|record| statuses.contains(&record.status));
        }

        records.sort_by(|left, right| {
            right
                .next_run_at
                .cmp(&left.next_run_at)
                .then_with(|| right.created_at.cmp(&left.created_at))
        });
        if records.len() > limit {
            records.truncate(limit);
        }
        Ok(records)
    }

    pub(super) async fn list_due_reminder_jobs_inner(
        &self,
        user_id: i64,
        now: i64,
        limit: usize,
    ) -> Result<Vec<ReminderJobRecord>, StorageError> {
        let mut records = self
            .list_json_under_prefix::<ReminderJobRecord>(&reminder_jobs_prefix(user_id))
            .await?;
        records.retain(|record| record.is_due(now));
        records.sort_by(|left, right| {
            left.next_run_at
                .cmp(&right.next_run_at)
                .then_with(|| left.created_at.cmp(&right.created_at))
        });
        if records.len() > limit {
            records.truncate(limit);
        }
        Ok(records)
    }

    pub(super) async fn claim_reminder_job_inner(
        &self,
        user_id: i64,
        reminder_id: String,
        lease_until: i64,
        now: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if !record.is_due(now) {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                lease_until: Some(lease_until),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    pub(super) async fn reschedule_reminder_job_inner(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        last_run_at: Option<i64>,
        last_error: Option<String>,
        increment_run_count: bool,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Scheduled {
                return None;
            }
            let run_count = if increment_run_count {
                record.run_count.saturating_add(1)
            } else {
                record.run_count
            };
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Scheduled,
                next_run_at,
                lease_until: None,
                last_run_at: last_run_at.or(record.last_run_at),
                last_error: last_error.clone(),
                run_count,
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    pub(super) async fn complete_reminder_job_inner(
        &self,
        user_id: i64,
        reminder_id: String,
        completed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Scheduled {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Completed,
                lease_until: None,
                last_run_at: Some(completed_at),
                last_error: None,
                run_count: record.run_count.saturating_add(1),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    pub(super) async fn fail_reminder_job_inner(
        &self,
        user_id: i64,
        reminder_id: String,
        failed_at: i64,
        error: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Scheduled {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Failed,
                lease_until: None,
                last_run_at: Some(failed_at),
                last_error: Some(error.clone()),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    pub(super) async fn cancel_reminder_job_inner(
        &self,
        user_id: i64,
        reminder_id: String,
        cancelled_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Scheduled {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Cancelled,
                lease_until: None,
                last_run_at: record.last_run_at.or(Some(cancelled_at)),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    pub(super) async fn pause_reminder_job_inner(
        &self,
        user_id: i64,
        reminder_id: String,
        paused_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Scheduled {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Paused,
                lease_until: None,
                last_run_at: record.last_run_at.or(Some(paused_at)),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    pub(super) async fn resume_reminder_job_inner(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        resumed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Paused {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Scheduled,
                next_run_at,
                lease_until: None,
                last_run_at: record.last_run_at.or(Some(resumed_at)),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    pub(super) async fn retry_reminder_job_inner(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        retried_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Failed {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Scheduled,
                next_run_at,
                lease_until: None,
                last_run_at: record.last_run_at.or(Some(retried_at)),
                last_error: None,
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    pub(super) async fn delete_reminder_job_inner(
        &self,
        user_id: i64,
        reminder_id: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&reminder_job_key(user_id, &reminder_id))
            .await
    }
}
