use super::control_plane::normalize_topic_prompt_payload;
use super::{
    keys::{reminder_job_key, topic_agents_md_key, topic_context_key},
    r2::R2Storage,
    reminder::ReminderJobRecord,
    telemetry::StorageOperation,
    utils::{
        current_timestamp_unix_secs, is_precondition_failed_put_error,
        should_retry_control_plane_rmw, CONTROL_PLANE_RMW_MAX_RETRIES,
        CONTROL_PLANE_RMW_RETRY_BACKOFF_MS,
    },
    StorageError, TopicAgentsMdRecord, TopicContextRecord,
};
use crate::config::AgentSettings;
use crate::storage::StorageProvider;
use aws_credential_types::Credentials;
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use aws_types::region::Region;
use moka::future::Cache;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio::time::sleep;
use tracing::warn;

/// Process-local per-key lock registry for control-plane RMW operations.
///
/// Limitation: this lock only serializes operations inside a single process.
/// It does not provide cross-process or cross-instance mutual exclusion.
#[derive(Default)]
pub(super) struct ControlPlaneLocks {
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl ControlPlaneLocks {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) async fn acquire(&self, key: String) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.locks.lock().await;
            Arc::clone(locks.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))))
        };

        lock.lock_owned().await
    }
}

#[derive(Clone, Copy)]
pub(super) enum TopicPromptStoreKind {
    Context,
    AgentsMd,
}

impl TopicPromptStoreKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Context => "topic_context",
            Self::AgentsMd => "topic_agents_md",
        }
    }
}

impl R2Storage {
    /// Create a new R2 storage instance
    ///
    /// # Errors
    ///
    /// Returns an error if R2 configuration is missing.
    pub async fn new(settings: &AgentSettings) -> Result<Self, StorageError> {
        let endpoint_url = settings
            .r2_endpoint_url
            .as_ref()
            .ok_or_else(|| StorageError::Config("R2_ENDPOINT_URL is missing".into()))?;
        let access_key = settings
            .r2_access_key_id
            .as_ref()
            .ok_or_else(|| StorageError::Config("R2_ACCESS_KEY_ID is missing".into()))?;
        let secret_key = settings
            .r2_secret_access_key
            .as_ref()
            .ok_or_else(|| StorageError::Config("R2_SECRET_ACCESS_KEY is missing".into()))?;
        let bucket = settings
            .r2_bucket_name
            .as_ref()
            .ok_or_else(|| StorageError::Config("R2_BUCKET_NAME is missing".into()))?;

        let credentials = Credentials::new(access_key, secret_key, None, None, "r2-storage");

        let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .credentials_provider(credentials)
            .region(Region::new(settings.r2_region.clone()))
            .load()
            .await;

        let s3_config = aws_sdk_s3::config::Builder::from(&sdk_config)
            .endpoint_url(endpoint_url)
            .force_path_style(true)
            .build();

        let client = Client::from_conf(s3_config);

        let cache = Cache::builder()
            .max_capacity(10_000)
            .time_to_live(Duration::from_secs(60 * 60)) // 1 hour
            .time_to_idle(Duration::from_secs(30 * 60)) // 30 minutes
            .build();

        Ok(Self {
            client,
            bucket: bucket.clone(),
            cache,
            control_plane_locks: ControlPlaneLocks::new(),
            telemetry: Default::default(),
        })
    }

    /// Save data as JSON to R2
    ///
    /// # Errors
    ///
    /// Returns an error if JSON serialization or S3 upload fails.
    pub async fn save_json<T: serde::Serialize + Sync>(
        &self,
        key: &str,
        data: &T,
    ) -> Result<(), StorageError> {
        let body_str = serde_json::to_string_pretty(data)?;
        let body_bytes = body_str.into_bytes();

        // Write-Through: Update cache immediately
        self.cache
            .insert(key.to_string(), Arc::new(body_bytes.clone()))
            .await;

        match self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(body_bytes))
            .content_type("application/json")
            .send()
            .await
        {
            Ok(_) => {
                self.telemetry
                    .record_operation(StorageOperation::Put, key, "ok");
            }
            Err(error) => {
                self.telemetry
                    .record_operation(StorageOperation::Put, key, "error");
                return Err(StorageError::S3Put(error.to_string()));
            }
        }

        Ok(())
    }

    /// Save raw UTF-8 text to R2.
    pub async fn save_text(&self, key: &str, data: &str) -> Result<(), StorageError> {
        let body_bytes = data.as_bytes().to_vec();

        self.cache
            .insert(key.to_string(), Arc::new(body_bytes.clone()))
            .await;

        match self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(body_bytes))
            .content_type("text/plain; charset=utf-8")
            .send()
            .await
        {
            Ok(_) => {
                self.telemetry
                    .record_operation(StorageOperation::Put, key, "ok");
            }
            Err(error) => {
                self.telemetry
                    .record_operation(StorageOperation::Put, key, "error");
                return Err(StorageError::S3Put(error.to_string()));
            }
        }

        Ok(())
    }

    pub(super) async fn save_json_conditionally<T: serde::Serialize + Sync>(
        &self,
        key: &str,
        data: &T,
        expected_etag: Option<&str>,
    ) -> Result<bool, StorageError> {
        let body_str = serde_json::to_string_pretty(data)?;
        let body_bytes = body_str.into_bytes();

        let request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(body_bytes.clone()))
            .content_type("application/json");

        let request = match expected_etag {
            Some(etag) => request.if_match(etag),
            None => request.if_none_match("*"),
        };

        match request.send().await {
            Ok(_) => {
                self.cache
                    .insert(key.to_string(), Arc::new(body_bytes.clone()))
                    .await;
                self.telemetry
                    .record_operation(StorageOperation::Put, key, "ok");
                Ok(true)
            }
            Err(err) if is_precondition_failed_put_error(&err) => {
                self.cache.invalidate(key).await;
                self.telemetry
                    .record_operation(StorageOperation::Put, key, "precondition_failed");
                Ok(false)
            }
            Err(err) => {
                self.telemetry
                    .record_operation(StorageOperation::Put, key, "error");
                Err(StorageError::S3Put(err.to_string()))
            }
        }
    }

    /// Load data from JSON in R2
    ///
    /// # Errors
    ///
    /// Returns an error if S3 download or JSON deserialization fails.
    pub async fn load_json<T: serde::de::DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<T>, StorageError> {
        // Read-Through: Check cache first
        if let Some(cached_data) = self.cache.get(key).await {
            self.telemetry.record_cache_hit(key);
            match serde_json::from_slice(&cached_data) {
                Ok(data) => return Ok(Some(data)),
                Err(e) => {
                    warn!("Cache deserialization failed for {}: {}", key, e);
                    // Fallback to S3 if cache is corrupted, but also remove from cache
                    self.cache.invalidate(key).await;
                }
            }
        }

        self.telemetry.record_cache_miss(key);

        let result = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await;

        match result {
            Ok(output) => {
                let data = output
                    .body
                    .collect()
                    .await
                    .map_err(|e| StorageError::Io(std::io::Error::other(e)))?
                    .into_bytes();

                // Read-Through: Populate cache on miss
                self.cache
                    .insert(key.to_string(), Arc::new(data.to_vec()))
                    .await;
                self.telemetry
                    .record_operation(StorageOperation::Get, key, "ok");

                let json_data = serde_json::from_slice(&data)?;
                Ok(Some(json_data))
            }
            Err(SdkError::ServiceError(err)) if err.err().is_no_such_key() => {
                self.telemetry
                    .record_operation(StorageOperation::Get, key, "not_found");
                Ok(None)
            }
            Err(e) => {
                self.telemetry
                    .record_operation(StorageOperation::Get, key, "error");
                Err(StorageError::S3Get(Box::new(e)))
            }
        }
    }

    /// Load raw UTF-8 text from R2.
    pub async fn load_text(&self, key: &str) -> Result<Option<String>, StorageError> {
        if let Some(cached_data) = self.cache.get(key).await {
            self.telemetry.record_cache_hit(key);
            return String::from_utf8(cached_data.to_vec())
                .map(Some)
                .map_err(|err| {
                    StorageError::Config(format!(
                        "stored secret at key '{key}' is not valid UTF-8: {err}"
                    ))
                });
        }

        self.telemetry.record_cache_miss(key);

        let result = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await;

        match result {
            Ok(output) => {
                let data = output
                    .body
                    .collect()
                    .await
                    .map_err(|e| StorageError::Io(std::io::Error::other(e)))?
                    .into_bytes();

                self.cache
                    .insert(key.to_string(), Arc::new(data.to_vec()))
                    .await;
                self.telemetry
                    .record_operation(StorageOperation::Get, key, "ok");

                String::from_utf8(data.to_vec()).map(Some).map_err(|err| {
                    StorageError::Config(format!(
                        "stored secret at key '{key}' is not valid UTF-8: {err}"
                    ))
                })
            }
            Err(SdkError::ServiceError(err)) if err.err().is_no_such_key() => {
                self.telemetry
                    .record_operation(StorageOperation::Get, key, "not_found");
                Ok(None)
            }
            Err(e) => {
                self.telemetry
                    .record_operation(StorageOperation::Get, key, "error");
                Err(StorageError::S3Get(Box::new(e)))
            }
        }
    }

    pub(super) async fn load_json_with_etag<T: serde::de::DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<(Option<T>, Option<String>), StorageError> {
        let result = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await;

        match result {
            Ok(output) => {
                let etag = output.e_tag().map(ToOwned::to_owned);
                let data = output
                    .body
                    .collect()
                    .await
                    .map_err(|e| StorageError::Io(std::io::Error::other(e)))?
                    .into_bytes();

                self.cache
                    .insert(key.to_string(), Arc::new(data.to_vec()))
                    .await;
                self.telemetry
                    .record_operation(StorageOperation::Get, key, "ok");

                let json_data = serde_json::from_slice(&data)?;
                Ok((Some(json_data), etag))
            }
            Err(SdkError::ServiceError(err)) if err.err().is_no_such_key() => {
                self.cache.invalidate(key).await;
                self.telemetry
                    .record_operation(StorageOperation::Get, key, "not_found");
                Ok((None, None))
            }
            Err(e) => {
                self.telemetry
                    .record_operation(StorageOperation::Get, key, "error");
                Err(StorageError::S3Get(Box::new(e)))
            }
        }
    }

    pub(super) async fn ensure_topic_prompt_not_duplicated(
        &self,
        user_id: i64,
        topic_id: &str,
        attempted_kind: TopicPromptStoreKind,
        candidate: &str,
    ) -> Result<(), StorageError> {
        let normalized_candidate = normalize_topic_prompt_payload(candidate);
        let existing = match attempted_kind {
            TopicPromptStoreKind::Context => self
                .load_json::<TopicAgentsMdRecord>(&topic_agents_md_key(user_id, topic_id))
                .await?
                .map(|record| (TopicPromptStoreKind::AgentsMd, record.agents_md)),
            TopicPromptStoreKind::AgentsMd => self
                .load_json::<TopicContextRecord>(&topic_context_key(user_id, topic_id))
                .await?
                .map(|record| (TopicPromptStoreKind::Context, record.context)),
        };

        if let Some((existing_kind, existing_content)) = existing {
            if normalize_topic_prompt_payload(&existing_content) == normalized_candidate {
                return Err(StorageError::DuplicateTopicPromptContent {
                    topic_id: topic_id.to_string(),
                    existing_kind: existing_kind.as_str().to_string(),
                    attempted_kind: attempted_kind.as_str().to_string(),
                });
            }
        }

        Ok(())
    }

    /// Delete object from R2
    ///
    /// # Errors
    ///
    /// Returns an error if S3 deletion fails.
    pub async fn delete_object(&self, key: &str) -> Result<(), StorageError> {
        // Invalidate cache
        self.cache.invalidate(key).await;

        match self
            .client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(_) => {
                self.telemetry
                    .record_operation(StorageOperation::Delete, key, "ok");
            }
            Err(error) => {
                self.telemetry
                    .record_operation(StorageOperation::Delete, key, "error");
                return Err(StorageError::S3Put(error.to_string()));
            }
        }

        Ok(())
    }

    pub(super) async fn delete_prefix(&self, prefix: &str) -> Result<(), StorageError> {
        let mut continuation_token: Option<String> = None;

        loop {
            let response = match self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix)
                .set_continuation_token(continuation_token.clone())
                .send()
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    self.telemetry
                        .record_operation(StorageOperation::List, prefix, "error");
                    return Err(StorageError::S3Put(error.to_string()));
                }
            };

            self.telemetry
                .record_operation(StorageOperation::List, prefix, "ok");

            for object in response.contents() {
                if let Some(key) = object.key() {
                    self.delete_object(key).await?;
                }
            }

            if !response.is_truncated().unwrap_or(false) {
                break;
            }

            continuation_token = response.next_continuation_token().map(str::to_string);
        }

        Ok(())
    }

    pub(super) async fn list_json_under_prefix<T: serde::de::DeserializeOwned>(
        &self,
        prefix: &str,
    ) -> Result<Vec<T>, StorageError> {
        let mut continuation_token: Option<String> = None;
        let mut records = Vec::new();

        loop {
            let response = match self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix)
                .set_continuation_token(continuation_token.clone())
                .send()
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    self.telemetry
                        .record_operation(StorageOperation::List, prefix, "error");
                    return Err(StorageError::S3Put(error.to_string()));
                }
            };

            self.telemetry
                .record_operation(StorageOperation::List, prefix, "ok");

            for object in response.contents() {
                let Some(key) = object.key() else {
                    continue;
                };
                if let Some(record) = self.load_json::<T>(key).await? {
                    records.push(record);
                }
            }

            if !response.is_truncated().unwrap_or(false) {
                break;
            }

            continuation_token = response.next_continuation_token().map(str::to_string);
        }

        Ok(records)
    }

    pub(super) async fn list_keys_under_prefix(
        &self,
        prefix: &str,
    ) -> Result<Vec<String>, StorageError> {
        let mut continuation_token: Option<String> = None;
        let mut keys = Vec::new();

        loop {
            let response = match self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix)
                .set_continuation_token(continuation_token.clone())
                .send()
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    self.telemetry
                        .record_operation(StorageOperation::List, prefix, "error");
                    return Err(StorageError::S3Put(error.to_string()));
                }
            };

            self.telemetry
                .record_operation(StorageOperation::List, prefix, "ok");

            for object in response.contents() {
                let Some(key) = object.key() else {
                    continue;
                };
                keys.push(key.to_string());
            }

            if !response.is_truncated().unwrap_or(false) {
                break;
            }

            continuation_token = response.next_continuation_token().map(str::to_string);
        }

        Ok(keys)
    }

    pub(super) async fn mutate_reminder_job<F>(
        &self,
        user_id: i64,
        reminder_id: &str,
        mutator: F,
    ) -> Result<Option<ReminderJobRecord>, StorageError>
    where
        F: Fn(ReminderJobRecord, i64) -> Option<ReminderJobRecord>,
    {
        let key = reminder_job_key(user_id, reminder_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (existing, etag) = self.load_json_with_etag::<ReminderJobRecord>(&key).await?;
            let Some(existing) = existing else {
                return Ok(None);
            };
            let now = current_timestamp_unix_secs();
            let Some(record) = mutator(existing, now) else {
                return Ok(None);
            };

            if self
                .save_json_conditionally(&key, &record, etag.as_deref())
                .await?
            {
                return Ok(Some(record));
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "reminder job optimistic concurrency conflict, retrying"
                );
                sleep(Duration::from_millis(
                    CONTROL_PLANE_RMW_RETRY_BACKOFF_MS * attempt as u64,
                ))
                .await;
            }
        }

        Err(StorageError::ConcurrencyConflict {
            key,
            attempts: CONTROL_PLANE_RMW_MAX_RETRIES,
        })
    }

    /// Atomically modify user config using a closure.
    ///
    /// # Errors
    ///
    /// Returns an error if modification or saving fails.
    pub async fn modify_user_config<F>(&self, user_id: i64, modifier: F) -> Result<(), StorageError>
    where
        F: FnOnce(&mut super::UserConfig),
    {
        super::telemetry::with_storage_reason("modify_user_config", async {
            let mut config = self.get_user_config(user_id).await?;
            modifier(&mut config);
            self.update_user_config(user_id, config).await
        })
        .await
    }
}
