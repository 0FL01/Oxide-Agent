//! Database helpers: error mapping, advisory locks, row accessors,
//! JSON/enum conversions, and integer range casts.

use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx_core::{
    decode::Decode, error::Error as SqlxError, query::query, row::Row,
    transaction::Transaction, types::Type,
};
use sqlx_postgres::{PgRow, Postgres};

use super::StorageError;

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

pub(super) fn db_error(error: SqlxError) -> StorageError {
    StorageError::Database(error.to_string())
}

// ---------------------------------------------------------------------------
// User row bootstrap
// ---------------------------------------------------------------------------

pub(super) async fn ensure_user_row_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i64,
) -> Result<(), StorageError> {
    query::<Postgres>(
        r#"
        INSERT INTO users (user_id)
        VALUES ($1)
        ON CONFLICT (user_id) DO UPDATE SET updated_at = NOW()
        "#,
    )
    .bind(user_id)
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Advisory locks
// ---------------------------------------------------------------------------

pub(super) async fn advisory_xact_lock(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
) -> Result<(), StorageError> {
    query::<Postgres>("SELECT pg_advisory_xact_lock($1)")
        .bind(advisory_lock_key(key))
        .execute(&mut **tx)
        .await
        .map_err(db_error)?;
    Ok(())
}

pub(super) fn advisory_lock_key(key: &str) -> i64 {
    let digest = Sha256::digest(key.as_bytes());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(bytes)
}

// ---------------------------------------------------------------------------
// Row accessors
// ---------------------------------------------------------------------------

pub(super) fn row_value<T>(row: &PgRow, column: &str) -> Result<T, StorageError>
where
    for<'r> T: Decode<'r, Postgres> + Type<Postgres>,
{
    row.try_get(column).map_err(db_error)
}

pub(super) fn from_json<T>(value: Value, _name: &str) -> Result<T, StorageError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(value).map_err(StorageError::Json)
}

// ---------------------------------------------------------------------------
// Enum ↔ SQL string conversions
// ---------------------------------------------------------------------------

pub(super) fn enum_to_sql<T>(value: &T, name: &str) -> Result<String, StorageError>
where
    T: Serialize,
{
    match serde_json::to_value(value)? {
        Value::String(value) => Ok(value),
        other => Err(StorageError::InvalidInput(format!(
            "{name} must serialize to a string, got {other}"
        ))),
    }
}

pub(super) fn enum_from_sql<T>(value: &str, name: &str) -> Result<T, StorageError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(Value::String(value.to_string())).map_err(|error| {
        StorageError::InvalidInput(format!("invalid {name} value `{value}`: {error}"))
    })
}

pub(super) fn enum_vec_to_sql<T>(values: &[T], name: &str) -> Result<Vec<String>, StorageError>
where
    T: Serialize,
{
    values
        .iter()
        .map(|value| enum_to_sql(value, name))
        .collect()
}

pub(super) fn enum_vec_from_sql<T>(values: Vec<String>, name: &str) -> Result<Vec<T>, StorageError>
where
    T: DeserializeOwned,
{
    values
        .iter()
        .map(|value| enum_from_sql(value, name))
        .collect()
}

// ---------------------------------------------------------------------------
// Integer range casts
// ---------------------------------------------------------------------------

pub(super) fn u32_to_i32(value: u32, field: &str) -> Result<i32, StorageError> {
    i32::try_from(value)
        .map_err(|_| StorageError::InvalidInput(format!("{field} value {value} exceeds i32 range")))
}

pub(super) fn i32_to_u32(value: i32, field: &str) -> Result<u32, StorageError> {
    u32::try_from(value).map_err(|_| {
        StorageError::Database(format!(
            "{field} value {value} cannot be represented as u32"
        ))
    })
}

pub(super) fn u64_to_i64(value: u64, field: &str) -> Result<i64, StorageError> {
    i64::try_from(value)
        .map_err(|_| StorageError::InvalidInput(format!("{field} value {value} exceeds i64 range")))
}

pub(super) fn i64_to_u64(value: i64, field: &str) -> Result<u64, StorageError> {
    u64::try_from(value).map_err(|_| {
        StorageError::Database(format!(
            "{field} value {value} cannot be represented as u64"
        ))
    })
}

pub(super) fn i64_to_u32(value: i64, field: &str) -> Result<u32, StorageError> {
    u32::try_from(value).map_err(|_| {
        StorageError::Database(format!(
            "{field} value {value} cannot be represented as u32"
        ))
    })
}

pub(super) fn u16_to_i32(value: u16) -> i32 {
    i32::from(value)
}

pub(super) fn i32_to_u16(value: i32, field: &str) -> Result<u16, StorageError> {
    u16::try_from(value).map_err(|_| {
        StorageError::Database(format!(
            "{field} value {value} cannot be represented as u16"
        ))
    })
}

pub(super) fn usize_to_i64(value: usize, field: &str) -> Result<i64, StorageError> {
    i64::try_from(value)
        .map_err(|_| StorageError::InvalidInput(format!("{field} value {value} exceeds i64 range")))
}
