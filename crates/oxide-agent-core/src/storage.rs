//! Storage layer for user data and chat history
//!
//! Provides a persistent storage implementation using Cloudflare R2 / AWS S3.

use crate::agent::memory::AgentMemory;
use crate::config::AgentSettings;
use async_trait::async_trait;
use aws_credential_types::Credentials;
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::put_object::PutObjectError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use aws_types::region::Region;
use thiserror::Error;
use tracing::{error, info, warn};

use moka::future::Cache;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio::time::sleep;
use uuid::Uuid;

const AGENT_PROFILE_SCHEMA_VERSION: u32 = 1;
const TOPIC_BINDING_SCHEMA_VERSION: u32 = 1;
const AUDIT_EVENT_SCHEMA_VERSION: u32 = 1;
const CONTROL_PLANE_RMW_MAX_RETRIES: usize = 5;
const CONTROL_PLANE_RMW_RETRY_BACKOFF_MS: u64 = 25;

/// Errors that can occur during storage operations
#[derive(Error, Debug)]
pub enum StorageError {
    /// Error retrieving object from S3
    #[error("S3 Get error: {0}")]
    S3Get(Box<SdkError<GetObjectError>>),
    /// Error putting object into S3
    #[error("S3 put error: {0}")]
    S3Put(String),
    /// Error during JSON serialization or deserialization
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// Standard I/O error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// Configuration error (missing credentials, etc.)
    #[error("Configuration error: {0}")]
    Config(String),
    /// Optimistic concurrency retries exhausted.
    #[error("Concurrent update conflict for key {key} after {attempts} attempts")]
    ConcurrencyConflict {
        /// Storage object key that could not be updated.
        key: String,
        /// Number of retry attempts performed.
        attempts: usize,
    },
}

/// User-specific configuration persisted in storage
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct UserConfig {
    /// Custom system prompt
    pub system_prompt: Option<String>,
    /// Selected LLM model name
    pub model_name: Option<String>,
    /// Current dialogue state
    pub state: Option<String>,
    /// Active chat UUID for chat mode context isolation
    pub current_chat_uuid: Option<String>,
}

/// Agent profile record persisted in control-plane storage.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentProfileRecord {
    /// Record schema version for forward-compatible evolution.
    pub schema_version: u32,
    /// Logical record revision incremented on each upsert.
    pub version: u64,
    /// User owning this profile.
    pub user_id: i64,
    /// Stable agent identifier.
    pub agent_id: String,
    /// Arbitrary profile payload.
    pub profile: serde_json::Value,
    /// Creation timestamp (unix seconds).
    pub created_at: i64,
    /// Last update timestamp (unix seconds).
    pub updated_at: i64,
}

/// Topic binding record persisted in control-plane storage.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TopicBindingRecord {
    /// Record schema version for forward-compatible evolution.
    pub schema_version: u32,
    /// Logical record revision incremented on each upsert.
    pub version: u64,
    /// User owning this topic binding.
    pub user_id: i64,
    /// Stable topic identifier.
    pub topic_id: String,
    /// Agent identifier bound to topic.
    pub agent_id: String,
    /// Creation timestamp (unix seconds).
    pub created_at: i64,
    /// Last update timestamp (unix seconds).
    pub updated_at: i64,
}

/// Audit event record persisted in control-plane storage.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AuditEventRecord {
    /// Record schema version for forward-compatible evolution.
    pub schema_version: u32,
    /// Monotonic sequence per user audit stream.
    pub version: u64,
    /// Stable unique event identifier.
    pub event_id: String,
    /// User associated with the event.
    pub user_id: i64,
    /// Optional topic associated with the event.
    pub topic_id: Option<String>,
    /// Optional agent associated with the event.
    pub agent_id: Option<String>,
    /// Event action name.
    pub action: String,
    /// Arbitrary event payload.
    pub payload: serde_json::Value,
    /// Creation timestamp (unix seconds).
    pub created_at: i64,
}

/// Parameters for agent profile upsert.
#[derive(Debug, Clone)]
pub struct UpsertAgentProfileOptions {
    /// User owning this profile.
    pub user_id: i64,
    /// Stable agent identifier.
    pub agent_id: String,
    /// Arbitrary profile payload.
    pub profile: serde_json::Value,
}

/// Parameters for topic binding upsert.
#[derive(Debug, Clone)]
pub struct UpsertTopicBindingOptions {
    /// User owning this topic binding.
    pub user_id: i64,
    /// Stable topic identifier.
    pub topic_id: String,
    /// Agent identifier bound to topic.
    pub agent_id: String,
}

/// Parameters for audit append operation.
#[derive(Debug, Clone)]
pub struct AppendAuditEventOptions {
    /// User associated with the event.
    pub user_id: i64,
    /// Optional topic associated with the event.
    pub topic_id: Option<String>,
    /// Optional agent associated with the event.
    pub agent_id: Option<String>,
    /// Event action name.
    pub action: String,
    /// Arbitrary event payload.
    pub payload: serde_json::Value,
}

/// Interface for storage providers
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait StorageProvider: Send + Sync {
    /// Get user configuration
    async fn get_user_config(&self, user_id: i64) -> Result<UserConfig, StorageError>;
    /// Update user configuration
    async fn update_user_config(
        &self,
        user_id: i64,
        config: UserConfig,
    ) -> Result<(), StorageError>;
    /// Update user system prompt
    async fn update_user_prompt(
        &self,
        user_id: i64,
        system_prompt: String,
    ) -> Result<(), StorageError>;
    /// Get user system prompt
    async fn get_user_prompt(&self, user_id: i64) -> Result<Option<String>, StorageError>;
    /// Update user model
    async fn update_user_model(&self, user_id: i64, model_name: String)
        -> Result<(), StorageError>;
    /// Get user model
    async fn get_user_model(&self, user_id: i64) -> Result<Option<String>, StorageError>;
    /// Update user state
    async fn update_user_state(&self, user_id: i64, state: String) -> Result<(), StorageError>;
    /// Get user state
    async fn get_user_state(&self, user_id: i64) -> Result<Option<String>, StorageError>;
    /// Save message to chat history
    async fn save_message(
        &self,
        user_id: i64,
        role: String,
        content: String,
    ) -> Result<(), StorageError>;
    /// Get chat history for a user
    async fn get_chat_history(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<Message>, StorageError>;
    /// Clear chat history for a user
    async fn clear_chat_history(&self, user_id: i64) -> Result<(), StorageError>;
    /// Save message to chat history scoped by chat UUID
    async fn save_message_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
        role: String,
        content: String,
    ) -> Result<(), StorageError>;
    /// Get chat history for a user scoped by chat UUID
    async fn get_chat_history_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
        limit: usize,
    ) -> Result<Vec<Message>, StorageError>;
    /// Clear chat history for a user scoped by chat UUID
    async fn clear_chat_history_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
    ) -> Result<(), StorageError>;
    /// Save agent memory to storage
    async fn save_agent_memory(
        &self,
        user_id: i64,
        memory: &AgentMemory,
    ) -> Result<(), StorageError>;
    /// Load agent memory from storage
    async fn load_agent_memory(&self, user_id: i64) -> Result<Option<AgentMemory>, StorageError>;
    /// Clear agent memory for a user
    async fn clear_agent_memory(&self, user_id: i64) -> Result<(), StorageError>;
    /// Clear all context (history and memory) for a user
    async fn clear_all_context(&self, user_id: i64) -> Result<(), StorageError>;
    /// Check connection to storage
    async fn check_connection(&self) -> Result<(), String>;
    /// Get an agent profile record.
    async fn get_agent_profile(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<Option<AgentProfileRecord>, StorageError>;
    /// Upsert an agent profile record.
    async fn upsert_agent_profile(
        &self,
        options: UpsertAgentProfileOptions,
    ) -> Result<AgentProfileRecord, StorageError>;
    /// Delete an agent profile record.
    async fn delete_agent_profile(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<(), StorageError>;
    /// Get a topic binding record.
    async fn get_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicBindingRecord>, StorageError>;
    /// Upsert a topic binding record.
    async fn upsert_topic_binding(
        &self,
        options: UpsertTopicBindingOptions,
    ) -> Result<TopicBindingRecord, StorageError>;
    /// Delete a topic binding record.
    async fn delete_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError>;
    /// Append an audit event to stream.
    async fn append_audit_event(
        &self,
        options: AppendAuditEventOptions,
    ) -> Result<AuditEventRecord, StorageError>;
    /// List recent audit events for a user.
    async fn list_audit_events(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError>;
}

/// A message in the chat history
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    /// Role of the message sender (user or assistant)
    pub role: String,
    /// Text content of the message
    pub content: String,
}

/// R2-backed storage implementation
pub struct R2Storage {
    client: Client,
    bucket: String,
    cache: Cache<String, Arc<Vec<u8>>>,
    control_plane_locks: ControlPlaneLocks,
}

/// Process-local per-key lock registry for control-plane RMW operations.
///
/// Limitation: this lock only serializes operations inside a single process.
/// It does not provide cross-process or cross-instance mutual exclusion.
#[derive(Default)]
struct ControlPlaneLocks {
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl ControlPlaneLocks {
    fn new() -> Self {
        Self::default()
    }

    async fn acquire(&self, key: String) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(key)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        lock.lock_owned().await
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
            .region(Region::new("auto"))
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

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(body_bytes))
            .content_type("application/json")
            .send()
            .await
            .map_err(|e| StorageError::S3Put(e.to_string()))?;

        Ok(())
    }

    async fn save_json_conditionally<T: serde::Serialize + Sync>(
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
                Ok(true)
            }
            Err(err) if is_precondition_failed_put_error(&err) => {
                self.cache.invalidate(key).await;
                Ok(false)
            }
            Err(err) => Err(StorageError::S3Put(err.to_string())),
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
            match serde_json::from_slice(&cached_data) {
                Ok(data) => return Ok(Some(data)),
                Err(e) => {
                    warn!("Cache deserialization failed for {}: {}", key, e);
                    // Fallback to S3 if cache is corrupted, but also remove from cache
                    self.cache.invalidate(key).await;
                }
            }
        }

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

                let json_data = serde_json::from_slice(&data)?;
                Ok(Some(json_data))
            }
            Err(SdkError::ServiceError(err)) if err.err().is_no_such_key() => Ok(None),
            Err(e) => Err(StorageError::S3Get(Box::new(e))),
        }
    }

    async fn load_json_with_etag<T: serde::de::DeserializeOwned>(
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

                let json_data = serde_json::from_slice(&data)?;
                Ok((Some(json_data), etag))
            }
            Err(SdkError::ServiceError(err)) if err.err().is_no_such_key() => {
                self.cache.invalidate(key).await;
                Ok((None, None))
            }
            Err(e) => Err(StorageError::S3Get(Box::new(e))),
        }
    }

    /// Delete object from R2
    ///
    /// # Errors
    ///
    /// Returns an error if S3 deletion fails.
    pub async fn delete_object(&self, key: &str) -> Result<(), StorageError> {
        // Invalidate cache
        self.cache.invalidate(key).await;

        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| StorageError::S3Put(e.to_string()))?;

        Ok(())
    }

    /// Atomically modify user config using a closure.
    ///
    /// # Errors
    ///
    /// Returns an error if modification or saving fails.
    pub async fn modify_user_config<F>(&self, user_id: i64, modifier: F) -> Result<(), StorageError>
    where
        F: FnOnce(&mut UserConfig),
    {
        let mut config = self.get_user_config(user_id).await?;
        modifier(&mut config);
        self.update_user_config(user_id, config).await
    }
}

#[async_trait]
impl StorageProvider for R2Storage {
    /// Get user configuration
    async fn get_user_config(&self, user_id: i64) -> Result<UserConfig, StorageError> {
        Ok(self
            .load_json(&user_config_key(user_id))
            .await?
            .unwrap_or_default())
    }

    /// Update user configuration
    async fn update_user_config(
        &self,
        user_id: i64,
        config: UserConfig,
    ) -> Result<(), StorageError> {
        self.save_json(&user_config_key(user_id), &config).await
    }

    /// Update user system prompt
    async fn update_user_prompt(
        &self,
        user_id: i64,
        system_prompt: String,
    ) -> Result<(), StorageError> {
        self.modify_user_config(user_id, |config| {
            config.system_prompt = Some(system_prompt);
        })
        .await
    }

    /// Get user system prompt
    async fn get_user_prompt(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        let config = self.get_user_config(user_id).await?;
        Ok(config.system_prompt)
    }

    /// Update user model
    async fn update_user_model(
        &self,
        user_id: i64,
        model_name: String,
    ) -> Result<(), StorageError> {
        self.modify_user_config(user_id, |config| {
            config.model_name = Some(model_name);
        })
        .await
    }

    /// Get user model
    async fn get_user_model(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        let config = self.get_user_config(user_id).await?;
        Ok(config.model_name)
    }

    /// Update user state
    async fn update_user_state(&self, user_id: i64, state: String) -> Result<(), StorageError> {
        self.modify_user_config(user_id, |config| {
            config.state = Some(state);
        })
        .await
    }

    /// Get user state
    async fn get_user_state(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        let config = self.get_user_config(user_id).await?;
        Ok(config.state)
    }

    /// Save message to chat history
    async fn save_message(
        &self,
        user_id: i64,
        role: String,
        content: String,
    ) -> Result<(), StorageError> {
        let key = user_history_key(user_id);
        let mut history: Vec<Message> = self.load_json(&key).await?.unwrap_or_default();
        history.push(Message { role, content });
        self.save_json(&key, &history).await
    }

    /// Get chat history for a user
    async fn get_chat_history(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<Message>, StorageError> {
        let history: Vec<Message> = self
            .load_json(&user_history_key(user_id))
            .await?
            .unwrap_or_default();
        let start = history.len().saturating_sub(limit);
        Ok(history[start..].to_vec())
    }

    /// Clear chat history for a user
    async fn clear_chat_history(&self, user_id: i64) -> Result<(), StorageError> {
        self.delete_object(&user_history_key(user_id)).await
    }

    /// Save message to chat history for a specific chat UUID
    async fn save_message_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
        role: String,
        content: String,
    ) -> Result<(), StorageError> {
        let key = user_chat_history_key(user_id, &chat_uuid);
        let mut history: Vec<Message> = self.load_json(&key).await?.unwrap_or_default();
        history.push(Message { role, content });
        self.save_json(&key, &history).await
    }

    /// Get chat history for a specific chat UUID
    async fn get_chat_history_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
        limit: usize,
    ) -> Result<Vec<Message>, StorageError> {
        let history: Vec<Message> = self
            .load_json(&user_chat_history_key(user_id, &chat_uuid))
            .await?
            .unwrap_or_default();
        let start = history.len().saturating_sub(limit);
        Ok(history[start..].to_vec())
    }

    /// Clear chat history for a specific chat UUID
    async fn clear_chat_history_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&user_chat_history_key(user_id, &chat_uuid))
            .await
    }

    /// Save agent memory to storage
    async fn save_agent_memory(
        &self,
        user_id: i64,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        self.save_json(&user_agent_memory_key(user_id), memory)
            .await
    }

    /// Load agent memory from storage
    async fn load_agent_memory(&self, user_id: i64) -> Result<Option<AgentMemory>, StorageError> {
        self.load_json(&user_agent_memory_key(user_id)).await
    }

    /// Clear agent memory for a user
    async fn clear_agent_memory(&self, user_id: i64) -> Result<(), StorageError> {
        self.delete_object(&user_agent_memory_key(user_id)).await
    }

    /// Clear all context (history and memory) for a user
    async fn clear_all_context(&self, user_id: i64) -> Result<(), StorageError> {
        self.clear_chat_history(user_id).await?;
        self.clear_agent_memory(user_id).await?;
        Ok(())
    }

    /// Check connection to R2 storage
    async fn check_connection(&self) -> Result<(), String> {
        match self.client.list_buckets().send().await {
            Ok(_) => {
                info!("Successfully connected to R2 storage.");
                Ok(())
            }
            Err(e) => {
                let err_msg = format!("R2 connectivity test failed: {e:#?}");
                error!("{}", err_msg);
                Err(err_msg)
            }
        }
    }

    async fn get_agent_profile(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<Option<AgentProfileRecord>, StorageError> {
        self.load_json(&agent_profile_key(user_id, &agent_id)).await
    }

    async fn upsert_agent_profile(
        &self,
        options: UpsertAgentProfileOptions,
    ) -> Result<AgentProfileRecord, StorageError> {
        let key = agent_profile_key(options.user_id, &options.agent_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (existing, etag) = self.load_json_with_etag::<AgentProfileRecord>(&key).await?;
            let now = current_timestamp_unix_secs();
            let record = build_agent_profile_record(options.clone(), existing, now);

            if self
                .save_json_conditionally(&key, &record, etag.as_deref())
                .await?
            {
                return Ok(record);
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "agent profile optimistic concurrency conflict, retrying"
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

    async fn delete_agent_profile(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&agent_profile_key(user_id, &agent_id))
            .await
    }

    async fn get_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicBindingRecord>, StorageError> {
        self.load_json(&topic_binding_key(user_id, &topic_id)).await
    }

    async fn upsert_topic_binding(
        &self,
        options: UpsertTopicBindingOptions,
    ) -> Result<TopicBindingRecord, StorageError> {
        let key = topic_binding_key(options.user_id, &options.topic_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (existing, etag) = self.load_json_with_etag::<TopicBindingRecord>(&key).await?;
            let now = current_timestamp_unix_secs();
            let record = build_topic_binding_record(options.clone(), existing, now);

            if self
                .save_json_conditionally(&key, &record, etag.as_deref())
                .await?
            {
                return Ok(record);
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "topic binding optimistic concurrency conflict, retrying"
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

    async fn delete_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&topic_binding_key(user_id, &topic_id))
            .await
    }

    async fn append_audit_event(
        &self,
        options: AppendAuditEventOptions,
    ) -> Result<AuditEventRecord, StorageError> {
        let key = audit_events_key(options.user_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (current_events, etag) = self
                .load_json_with_etag::<Vec<AuditEventRecord>>(&key)
                .await?;
            let mut events = current_events.unwrap_or_default();
            let now = current_timestamp_unix_secs();
            let record = build_audit_event_record(
                options.clone(),
                events.last().map(|event| event.version),
                now,
                Uuid::new_v4().to_string(),
            );

            events.push(record.clone());
            if self
                .save_json_conditionally(&key, &events, etag.as_deref())
                .await?
            {
                return Ok(record);
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "audit stream optimistic concurrency conflict, retrying"
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

    async fn list_audit_events(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        let events: Vec<AuditEventRecord> = self
            .load_json(&audit_events_key(user_id))
            .await?
            .unwrap_or_default();
        let start = events.len().saturating_sub(limit);
        Ok(events[start..].to_vec())
    }
}

/// Returns the R2 key for a user's configuration file
#[must_use]
pub fn user_config_key(user_id: i64) -> String {
    format!("users/{user_id}/config.json")
}

/// Returns the R2 key for a user's chat history file
#[must_use]
pub fn user_history_key(user_id: i64) -> String {
    format!("users/{user_id}/history.json")
}

/// Returns the R2 key for a user's chat history file scoped by chat UUID
#[must_use]
pub fn user_chat_history_key(user_id: i64, chat_uuid: &str) -> String {
    format!("users/{user_id}/chats/{chat_uuid}/history.json")
}

/// Returns the R2 key for a user's agent memory file
#[must_use]
pub fn user_agent_memory_key(user_id: i64) -> String {
    format!("users/{user_id}/agent_memory.json")
}

/// Returns the R2 key for an agent profile record.
#[must_use]
pub fn agent_profile_key(user_id: i64, agent_id: &str) -> String {
    format!("users/{user_id}/control_plane/agent_profiles/{agent_id}.json")
}

/// Returns the R2 key for a topic binding record.
#[must_use]
pub fn topic_binding_key(user_id: i64, topic_id: &str) -> String {
    format!("users/{user_id}/control_plane/topic_bindings/{topic_id}.json")
}

/// Returns the R2 key for a user audit events stream.
#[must_use]
pub fn audit_events_key(user_id: i64) -> String {
    format!("users/{user_id}/control_plane/audit/events.json")
}

#[must_use]
fn build_agent_profile_record(
    options: UpsertAgentProfileOptions,
    existing: Option<AgentProfileRecord>,
    now: i64,
) -> AgentProfileRecord {
    match existing {
        Some(existing_record) => AgentProfileRecord {
            schema_version: AGENT_PROFILE_SCHEMA_VERSION,
            version: next_record_version(Some(existing_record.version)),
            user_id: options.user_id,
            agent_id: options.agent_id,
            profile: options.profile,
            created_at: existing_record.created_at,
            updated_at: now,
        },
        None => AgentProfileRecord {
            schema_version: AGENT_PROFILE_SCHEMA_VERSION,
            version: next_record_version(None),
            user_id: options.user_id,
            agent_id: options.agent_id,
            profile: options.profile,
            created_at: now,
            updated_at: now,
        },
    }
}

#[must_use]
fn build_topic_binding_record(
    options: UpsertTopicBindingOptions,
    existing: Option<TopicBindingRecord>,
    now: i64,
) -> TopicBindingRecord {
    match existing {
        Some(existing_record) => TopicBindingRecord {
            schema_version: TOPIC_BINDING_SCHEMA_VERSION,
            version: next_record_version(Some(existing_record.version)),
            user_id: options.user_id,
            topic_id: options.topic_id,
            agent_id: options.agent_id,
            created_at: existing_record.created_at,
            updated_at: now,
        },
        None => TopicBindingRecord {
            schema_version: TOPIC_BINDING_SCHEMA_VERSION,
            version: next_record_version(None),
            user_id: options.user_id,
            topic_id: options.topic_id,
            agent_id: options.agent_id,
            created_at: now,
            updated_at: now,
        },
    }
}

#[must_use]
fn build_audit_event_record(
    options: AppendAuditEventOptions,
    current_version: Option<u64>,
    now: i64,
    event_id: String,
) -> AuditEventRecord {
    AuditEventRecord {
        schema_version: AUDIT_EVENT_SCHEMA_VERSION,
        version: next_record_version(current_version),
        event_id,
        user_id: options.user_id,
        topic_id: options.topic_id,
        agent_id: options.agent_id,
        action: options.action,
        payload: options.payload,
        created_at: now,
    }
}

#[must_use]
fn next_record_version(current_version: Option<u64>) -> u64 {
    match current_version {
        Some(version) => version.saturating_add(1),
        None => 1,
    }
}

#[must_use]
fn should_retry_control_plane_rmw(attempt: usize) -> bool {
    attempt < CONTROL_PLANE_RMW_MAX_RETRIES
}

#[must_use]
fn current_timestamp_unix_secs() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(_) => 0,
    }
}

#[must_use]
fn is_precondition_failed_put_error(err: &SdkError<PutObjectError>) -> bool {
    match err {
        SdkError::ServiceError(service_err) => service_err.raw().status().as_u16() == 412,
        _ => false,
    }
}

/// Generates a new random chat UUID (v4)
#[must_use]
pub fn generate_chat_uuid() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        agent_profile_key, audit_events_key, build_agent_profile_record, build_audit_event_record,
        build_topic_binding_record, generate_chat_uuid, next_record_version,
        should_retry_control_plane_rmw, topic_binding_key, user_chat_history_key, user_config_key,
        user_history_key, AgentProfileRecord, AppendAuditEventOptions, ControlPlaneLocks,
        TopicBindingRecord, UpsertAgentProfileOptions, UpsertTopicBindingOptions, UserConfig,
    };
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::oneshot;
    use tokio::time::timeout;
    use uuid::Uuid;

    #[test]
    fn user_chat_history_key_uses_chat_uuid_namespace() {
        let key = user_chat_history_key(42, "chat-123");
        assert_eq!(key, "users/42/chats/chat-123/history.json");
    }

    #[test]
    fn legacy_user_history_key_stays_unchanged() {
        let key = user_history_key(42);
        assert_eq!(key, "users/42/history.json");
    }

    #[test]
    fn user_chat_history_key_isolated_by_user_and_chat_uuid() {
        let key_a = user_chat_history_key(1, "chat-a");
        let key_b = user_chat_history_key(1, "chat-b");
        let key_c = user_chat_history_key(2, "chat-a");

        assert_ne!(key_a, key_b);
        assert_ne!(key_a, key_c);
        assert_ne!(key_b, key_c);
    }

    #[test]
    fn generate_chat_uuid_returns_v4_uuid() {
        let chat_uuid = generate_chat_uuid();
        let parsed = Uuid::parse_str(&chat_uuid);
        assert!(parsed.is_ok());
        let version = parsed.map(|uuid| uuid.get_version_num());
        assert_eq!(version, Ok(4));
    }

    #[test]
    fn user_config_deserializes_without_current_chat_uuid() {
        let json = r#"{
            "system_prompt": "You are helpful",
            "model_name": "gpt",
            "state": "idle"
        }"#;

        let parsed: Result<UserConfig, serde_json::Error> = serde_json::from_str(json);
        assert!(parsed.is_ok());
        let config = parsed.ok();
        assert!(config.is_some());
        assert_eq!(config.and_then(|cfg| cfg.current_chat_uuid), None);
    }

    #[test]
    fn user_config_roundtrip_preserves_current_chat_uuid() {
        let config = UserConfig {
            system_prompt: Some("You are helpful".to_string()),
            model_name: Some("gpt".to_string()),
            state: Some("chat_mode".to_string()),
            current_chat_uuid: Some("123e4567-e89b-12d3-a456-426614174000".to_string()),
        };

        let json = serde_json::to_string(&config);
        assert!(json.is_ok());

        let parsed: Result<UserConfig, serde_json::Error> =
            serde_json::from_str(&json.unwrap_or_default());
        assert!(parsed.is_ok());

        let parsed = parsed.unwrap_or_default();
        assert_eq!(
            parsed.current_chat_uuid,
            Some("123e4567-e89b-12d3-a456-426614174000".to_string())
        );
    }

    #[test]
    fn user_config_key_stays_stable() {
        let key = user_config_key(42);
        assert_eq!(key, "users/42/config.json");
    }

    #[test]
    fn agent_profile_key_uses_control_plane_namespace() {
        let key = agent_profile_key(42, "agent-a");
        assert_eq!(key, "users/42/control_plane/agent_profiles/agent-a.json");
    }

    #[test]
    fn topic_binding_key_uses_control_plane_namespace() {
        let key = topic_binding_key(42, "topic-a");
        assert_eq!(key, "users/42/control_plane/topic_bindings/topic-a.json");
    }

    #[test]
    fn audit_events_key_uses_control_plane_namespace() {
        let key = audit_events_key(42);
        assert_eq!(key, "users/42/control_plane/audit/events.json");
    }

    #[test]
    fn next_record_version_starts_at_one() {
        assert_eq!(next_record_version(None), 1);
    }

    #[test]
    fn next_record_version_increments_existing_value() {
        assert_eq!(next_record_version(Some(7)), 8);
    }

    #[test]
    fn next_record_version_saturates_on_overflow_boundary() {
        assert_eq!(next_record_version(Some(u64::MAX)), u64::MAX);
    }

    #[test]
    fn upsert_agent_profile_increments_version_and_preserves_created_at() {
        let existing = AgentProfileRecord {
            schema_version: 1,
            version: 3,
            user_id: 7,
            agent_id: "agent-a".to_string(),
            profile: json!({"name": "before"}),
            created_at: 123,
            updated_at: 124,
        };

        let updated = build_agent_profile_record(
            UpsertAgentProfileOptions {
                user_id: 7,
                agent_id: "agent-a".to_string(),
                profile: json!({"name": "after"}),
            },
            Some(existing),
            999,
        );

        assert_eq!(updated.version, 4);
        assert_eq!(updated.created_at, 123);
        assert_eq!(updated.updated_at, 999);
    }

    #[test]
    fn upsert_agent_profile_initial_insert_starts_version_and_sets_timestamps() {
        let created = build_agent_profile_record(
            UpsertAgentProfileOptions {
                user_id: 7,
                agent_id: "agent-a".to_string(),
                profile: json!({"name": "new"}),
            },
            None,
            777,
        );

        assert_eq!(created.version, 1);
        assert_eq!(created.created_at, 777);
        assert_eq!(created.updated_at, 777);
    }

    #[test]
    fn upsert_topic_binding_increments_version_and_preserves_created_at() {
        let existing = TopicBindingRecord {
            schema_version: 1,
            version: 8,
            user_id: 7,
            topic_id: "topic-a".to_string(),
            agent_id: "agent-a".to_string(),
            created_at: 500,
            updated_at: 501,
        };

        let updated = build_topic_binding_record(
            UpsertTopicBindingOptions {
                user_id: 7,
                topic_id: "topic-a".to_string(),
                agent_id: "agent-b".to_string(),
            },
            Some(existing),
            1_000,
        );

        assert_eq!(updated.version, 9);
        assert_eq!(updated.created_at, 500);
        assert_eq!(updated.updated_at, 1_000);
        assert_eq!(updated.agent_id, "agent-b");
    }

    #[test]
    fn upsert_topic_binding_initial_insert_starts_version_and_sets_timestamps() {
        let created = build_topic_binding_record(
            UpsertTopicBindingOptions {
                user_id: 7,
                topic_id: "topic-a".to_string(),
                agent_id: "agent-a".to_string(),
            },
            None,
            2_000,
        );

        assert_eq!(created.version, 1);
        assert_eq!(created.created_at, 2_000);
        assert_eq!(created.updated_at, 2_000);
    }

    #[test]
    fn append_audit_event_versions_are_monotonic() {
        let first = build_audit_event_record(
            AppendAuditEventOptions {
                user_id: 9,
                topic_id: Some("topic-a".to_string()),
                agent_id: Some("agent-a".to_string()),
                action: "created".to_string(),
                payload: json!({"k": 1}),
            },
            None,
            10,
            "event-1".to_string(),
        );

        let second = build_audit_event_record(
            AppendAuditEventOptions {
                user_id: 9,
                topic_id: Some("topic-a".to_string()),
                agent_id: Some("agent-a".to_string()),
                action: "updated".to_string(),
                payload: json!({"k": 2}),
            },
            Some(first.version),
            11,
            "event-2".to_string(),
        );

        assert_eq!(first.version, 1);
        assert_eq!(second.version, 2);
    }

    #[test]
    fn append_audit_event_version_saturates_at_upper_bound() {
        let event = build_audit_event_record(
            AppendAuditEventOptions {
                user_id: 9,
                topic_id: None,
                agent_id: None,
                action: "updated".to_string(),
                payload: json!({"k": 2}),
            },
            Some(u64::MAX),
            11,
            "event-2".to_string(),
        );

        assert_eq!(event.version, u64::MAX);
    }

    #[test]
    fn control_plane_retry_policy_stops_at_max_attempt() {
        assert!(should_retry_control_plane_rmw(1));
        assert!(should_retry_control_plane_rmw(4));
        assert!(!should_retry_control_plane_rmw(5));
        assert!(!should_retry_control_plane_rmw(6));
    }

    #[tokio::test]
    async fn control_plane_lock_serializes_same_key_updates() {
        let locks = Arc::new(ControlPlaneLocks::new());
        let first_guard = locks
            .acquire("users/7/control_plane/topic_bindings/topic-a.json".to_string())
            .await;

        let locks_for_task = Arc::clone(&locks);
        let (tx, rx) = oneshot::channel();
        let join = tokio::spawn(async move {
            let _second_guard = locks_for_task
                .acquire("users/7/control_plane/topic_bindings/topic-a.json".to_string())
                .await;
            let _ = tx.send(());
        });

        let blocked_result = timeout(Duration::from_millis(50), rx).await;
        assert!(blocked_result.is_err());

        drop(first_guard);

        let join_result = timeout(Duration::from_secs(1), join).await;
        assert!(join_result.is_ok());
    }
}
