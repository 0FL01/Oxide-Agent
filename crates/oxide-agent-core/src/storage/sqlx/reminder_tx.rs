use super::helpers::{db_error, enum_to_sql, u32_to_i32, u64_to_i64};
use super::rows::row_to_reminder_job;
use super::{ReminderJobRecord, SqlxStorage, StorageError};
use crate::storage::utils::current_timestamp_unix_secs;
use sqlx_core::{query::query, transaction::Transaction};
use sqlx_postgres::Postgres;

pub(super) async fn insert_reminder_job_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    record: &ReminderJobRecord,
) -> Result<(), StorageError> {
    query::<Postgres>(
        r#"
        INSERT INTO reminder_jobs (
            user_id, reminder_id, context_key, flow_id, chat_id, thread_id,
            thread_kind, task_prompt, schedule_kind, status, next_run_at,
            interval_secs, cron_expression, timezone, lease_until, last_run_at,
            last_error, run_count, version, schema_version, created_at, updated_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
            $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22
        )
        "#,
    )
    .bind(record.user_id)
    .bind(&record.reminder_id)
    .bind(&record.context_key)
    .bind(&record.flow_id)
    .bind(record.chat_id)
    .bind(record.thread_id)
    .bind(enum_to_sql(&record.thread_kind, "reminder thread kind")?)
    .bind(&record.task_prompt)
    .bind(enum_to_sql(
        &record.schedule_kind,
        "reminder schedule kind",
    )?)
    .bind(enum_to_sql(&record.status, "reminder status")?)
    .bind(record.next_run_at)
    .bind(
        record
            .interval_secs
            .map(|value| u64_to_i64(value, "reminder interval_secs"))
            .transpose()?,
    )
    .bind(&record.cron_expression)
    .bind(&record.timezone)
    .bind(record.lease_until)
    .bind(record.last_run_at)
    .bind(&record.last_error)
    .bind(u64_to_i64(record.run_count, "reminder run_count")?)
    .bind(u64_to_i64(record.version, "reminder version")?)
    .bind(u32_to_i32(
        record.schema_version,
        "reminder schema_version",
    )?)
    .bind(record.created_at)
    .bind(record.updated_at)
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    Ok(())
}

pub(super) async fn update_reminder_job_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    record: &ReminderJobRecord,
) -> Result<(), StorageError> {
    query::<Postgres>(
        r#"
        UPDATE reminder_jobs
        SET context_key = $3,
            flow_id = $4,
            chat_id = $5,
            thread_id = $6,
            thread_kind = $7,
            task_prompt = $8,
            schedule_kind = $9,
            status = $10,
            next_run_at = $11,
            interval_secs = $12,
            cron_expression = $13,
            timezone = $14,
            lease_until = $15,
            last_run_at = $16,
            last_error = $17,
            run_count = $18,
            version = $19,
            schema_version = $20,
            updated_at = $21
        WHERE user_id = $1 AND reminder_id = $2
        "#,
    )
    .bind(record.user_id)
    .bind(&record.reminder_id)
    .bind(&record.context_key)
    .bind(&record.flow_id)
    .bind(record.chat_id)
    .bind(record.thread_id)
    .bind(enum_to_sql(&record.thread_kind, "reminder thread kind")?)
    .bind(&record.task_prompt)
    .bind(enum_to_sql(
        &record.schedule_kind,
        "reminder schedule kind",
    )?)
    .bind(enum_to_sql(&record.status, "reminder status")?)
    .bind(record.next_run_at)
    .bind(
        record
            .interval_secs
            .map(|value| u64_to_i64(value, "reminder interval_secs"))
            .transpose()?,
    )
    .bind(&record.cron_expression)
    .bind(&record.timezone)
    .bind(record.lease_until)
    .bind(record.last_run_at)
    .bind(&record.last_error)
    .bind(u64_to_i64(record.run_count, "reminder run_count")?)
    .bind(u64_to_i64(record.version, "reminder version")?)
    .bind(u32_to_i32(
        record.schema_version,
        "reminder schema_version",
    )?)
    .bind(record.updated_at)
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    Ok(())
}

pub(super) async fn get_reminder_job_for_update(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
    reminder_id: &str,
) -> Result<Option<ReminderJobRecord>, StorageError> {
    let row = query::<Postgres>(
        r#"
        SELECT user_id, reminder_id, context_key, flow_id, chat_id, thread_id,
               thread_kind, task_prompt, schedule_kind, status, next_run_at,
               interval_secs, cron_expression, timezone, lease_until,
               last_run_at, last_error, run_count, version, schema_version,
               created_at, updated_at
        FROM reminder_jobs
        WHERE user_id = $1 AND reminder_id = $2
        FOR UPDATE
        "#,
    )
    .bind(user_id)
    .bind(reminder_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_error)?;
    row.map(|row| row_to_reminder_job(&row)).transpose()
}

pub(super) async fn mutate_reminder_job<F>(
    storage: &SqlxStorage,
    user_id: i64,
    reminder_id: &str,
    mutate: F,
) -> Result<Option<ReminderJobRecord>, StorageError>
where
    F: FnOnce(ReminderJobRecord, i64) -> Option<ReminderJobRecord> + Send,
{
    let mut tx = storage.pool.begin().await.map_err(db_error)?;
    let Some(record) = get_reminder_job_for_update(&mut tx, user_id, reminder_id).await? else {
        tx.commit().await.map_err(db_error)?;
        return Ok(None);
    };
    let mutation_now = current_timestamp_unix_secs();
    let Some(updated) = mutate(record, mutation_now) else {
        tx.commit().await.map_err(db_error)?;
        return Ok(None);
    };
    update_reminder_job_in_tx(&mut tx, &updated).await?;
    tx.commit().await.map_err(db_error)?;
    Ok(Some(updated))
}
