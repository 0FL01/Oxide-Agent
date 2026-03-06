//! Storage layer for user data and chat history
//!
//! Provides a persistent storage implementation using Cloudflare R2 / AWS S3.

use crate::agent::memory::AgentMemory;
use crate::agent::task::{TaskEvent, TaskId, TaskSnapshot};
use crate::config::AgentSettings;
use async_trait::async_trait;
use aws_credential_types::Credentials;
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use aws_types::region::Region;
use thiserror::Error;
use tracing::{error, info, warn};

use moka::future::Cache;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

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
    /// Storage provider does not implement an optional capability.
    #[error("Unsupported storage operation: {0}")]
    Unsupported(String),
    /// Task event append violated the required contiguous sequence contract.
    #[error("invalid task event sequence for {task_id}: expected {expected}, got {actual}")]
    InvalidTaskEventSequence {
        /// Task identifier whose event log append was rejected.
        task_id: TaskId,
        /// Expected next contiguous sequence value.
        expected: u64,
        /// Provided sequence value.
        actual: u64,
    },
    /// Task event payload does not belong to the requested task stream.
    #[error("task event task id mismatch: expected {expected}, got {actual}")]
    TaskEventTaskMismatch {
        /// Task identifier of the append target.
        expected: TaskId,
        /// Task identifier carried by the event payload.
        actual: TaskId,
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
    /// Persist a task snapshot under the additive `tasks/` namespace for restart recovery.
    async fn save_task_snapshot(&self, snapshot: &TaskSnapshot) -> Result<(), StorageError> {
        let _ = snapshot;
        Err(StorageError::Unsupported(
            "task snapshot persistence".to_string(),
        ))
    }
    /// Load a task snapshot without requiring transport-specific data.
    async fn load_task_snapshot(
        &self,
        task_id: TaskId,
    ) -> Result<Option<TaskSnapshot>, StorageError> {
        let _ = task_id;
        Err(StorageError::Unsupported(
            "task snapshot loading".to_string(),
        ))
    }
    /// Append a baseline event entry to the task event log.
    async fn append_task_event(
        &self,
        task_id: TaskId,
        event: TaskEvent,
    ) -> Result<(), StorageError> {
        let _ = task_id;
        let _ = event;
        Err(StorageError::Unsupported(
            "task event persistence".to_string(),
        ))
    }
    /// Load the baseline task event log for replay or recovery.
    async fn load_task_events(&self, task_id: TaskId) -> Result<Vec<TaskEvent>, StorageError> {
        let _ = task_id;
        Err(StorageError::Unsupported("task event loading".to_string()))
    }
    /// Check connection to storage
    async fn check_connection(&self) -> Result<(), String>;
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

fn validate_task_event_sequence(
    task_id: TaskId,
    existing_events: &[TaskEvent],
    event: &TaskEvent,
) -> Result<(), StorageError> {
    if event.task_id != task_id {
        return Err(StorageError::TaskEventTaskMismatch {
            expected: task_id,
            actual: event.task_id,
        });
    }

    let expected = existing_events.last().map_or(1, |last| last.sequence + 1);

    if event.sequence == expected {
        Ok(())
    } else {
        Err(StorageError::InvalidTaskEventSequence {
            task_id,
            expected,
            actual: event.sequence,
        })
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

    /// Persist a task snapshot for restart recovery.
    async fn save_task_snapshot(&self, snapshot: &TaskSnapshot) -> Result<(), StorageError> {
        self.save_json(&task_snapshot_key(snapshot.metadata.id), snapshot)
            .await
    }

    /// Load a persisted task snapshot.
    async fn load_task_snapshot(
        &self,
        task_id: TaskId,
    ) -> Result<Option<TaskSnapshot>, StorageError> {
        self.load_json(&task_snapshot_key(task_id)).await
    }

    /// Append an event to the baseline task event log.
    async fn append_task_event(
        &self,
        task_id: TaskId,
        event: TaskEvent,
    ) -> Result<(), StorageError> {
        let key = task_event_log_key(task_id);
        let mut events: Vec<TaskEvent> = self.load_json(&key).await?.unwrap_or_default();
        validate_task_event_sequence(task_id, &events, &event)?;
        events.push(event);
        self.save_json(&key, &events).await
    }

    /// Load the baseline task event log.
    async fn load_task_events(&self, task_id: TaskId) -> Result<Vec<TaskEvent>, StorageError> {
        self.load_json(&task_event_log_key(task_id))
            .await
            .map(|events| events.unwrap_or_default())
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

/// Returns the R2 key for a persisted task snapshot.
#[must_use]
pub fn task_snapshot_key(task_id: TaskId) -> String {
    format!("tasks/{task_id}/snapshot.json")
}

/// Returns the R2 key for a task event log.
#[must_use]
pub fn task_event_log_key(task_id: TaskId) -> String {
    format!("tasks/{task_id}/events.json")
}

/// Generates a new random chat UUID (v4)
#[must_use]
pub fn generate_chat_uuid() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        generate_chat_uuid, task_event_log_key, task_snapshot_key, user_chat_history_key,
        user_config_key, user_history_key, Message, StorageError, StorageProvider, UserConfig,
    };
    use crate::agent::task::{TaskEvent, TaskEventKind, TaskMetadata, TaskSnapshot, TaskState};
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use uuid::Uuid;

    #[derive(Clone, Default)]
    struct InMemoryStorage {
        documents: Arc<Mutex<HashMap<String, serde_json::Value>>>,
    }

    impl InMemoryStorage {
        async fn save_json<T: serde::Serialize>(
            &self,
            key: String,
            value: &T,
        ) -> Result<(), StorageError> {
            let value = serde_json::to_value(value)?;
            self.documents.lock().await.insert(key, value);
            Ok(())
        }

        async fn load_json<T: serde::de::DeserializeOwned>(
            &self,
            key: String,
        ) -> Result<Option<T>, StorageError> {
            self.documents
                .lock()
                .await
                .get(&key)
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(StorageError::from)
        }
    }

    #[async_trait]
    impl StorageProvider for InMemoryStorage {
        async fn get_user_config(&self, _user_id: i64) -> Result<UserConfig, StorageError> {
            Ok(UserConfig::default())
        }

        async fn update_user_config(
            &self,
            _user_id: i64,
            _config: UserConfig,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn update_user_prompt(
            &self,
            _user_id: i64,
            _system_prompt: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_prompt(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn update_user_model(
            &self,
            _user_id: i64,
            _model_name: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_model(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn update_user_state(
            &self,
            _user_id: i64,
            _state: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_state(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn save_message(
            &self,
            _user_id: i64,
            _role: String,
            _content: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_chat_history(
            &self,
            _user_id: i64,
            _limit: usize,
        ) -> Result<Vec<Message>, StorageError> {
            Ok(Vec::new())
        }

        async fn clear_chat_history(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_message_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
            _role: String,
            _content: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_chat_history_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
            _limit: usize,
        ) -> Result<Vec<Message>, StorageError> {
            Ok(Vec::new())
        }

        async fn clear_chat_history_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_agent_memory(
            &self,
            _user_id: i64,
            _memory: &crate::agent::memory::AgentMemory,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn load_agent_memory(
            &self,
            _user_id: i64,
        ) -> Result<Option<crate::agent::memory::AgentMemory>, StorageError> {
            Ok(None)
        }

        async fn clear_agent_memory(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn clear_all_context(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_task_snapshot(&self, snapshot: &TaskSnapshot) -> Result<(), StorageError> {
            self.save_json(task_snapshot_key(snapshot.metadata.id), snapshot)
                .await
        }

        async fn load_task_snapshot(
            &self,
            task_id: crate::agent::task::TaskId,
        ) -> Result<Option<TaskSnapshot>, StorageError> {
            self.load_json(task_snapshot_key(task_id)).await
        }

        async fn append_task_event(
            &self,
            task_id: crate::agent::task::TaskId,
            event: TaskEvent,
        ) -> Result<(), StorageError> {
            let key = task_event_log_key(task_id);
            let mut events: Vec<TaskEvent> = self.load_json(key.clone()).await?.unwrap_or_default();
            super::validate_task_event_sequence(task_id, &events, &event)?;
            events.push(event);
            self.save_json(key, &events).await
        }

        async fn load_task_events(
            &self,
            task_id: crate::agent::task::TaskId,
        ) -> Result<Vec<TaskEvent>, StorageError> {
            self.load_json(task_event_log_key(task_id))
                .await
                .map(|events| events.unwrap_or_default())
        }

        async fn check_connection(&self) -> Result<(), String> {
            Ok(())
        }
    }

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
    fn task_snapshot_key_uses_dedicated_task_namespace() {
        let task_id = TaskMetadata::new().id;
        let key = task_snapshot_key(task_id);
        assert_eq!(key, format!("tasks/{task_id}/snapshot.json"));
    }

    #[test]
    fn task_event_log_key_uses_dedicated_task_namespace() {
        let task_id = TaskMetadata::new().id;
        let key = task_event_log_key(task_id);
        assert_eq!(key, format!("tasks/{task_id}/events.json"));
    }

    #[tokio::test]
    async fn storage_task_snapshot_roundtrip_works_without_transport_specific_data() {
        let storage = InMemoryStorage::default();
        let metadata = TaskMetadata::new();
        let task_id = metadata.id;
        let snapshot = TaskSnapshot::new(metadata, "synchronize backlog".to_string(), 2);

        let saved = storage.save_task_snapshot(&snapshot).await;
        assert!(saved.is_ok());

        let loaded = storage.load_task_snapshot(task_id).await;
        assert!(loaded.is_ok());
        assert_eq!(loaded.ok().flatten(), Some(snapshot));
    }

    #[tokio::test]
    async fn storage_task_events_append_and_load_in_order() {
        let storage = InMemoryStorage::default();
        let task_id = TaskMetadata::new().id;
        let created = TaskEvent::new(task_id, 1, TaskEventKind::Created, TaskState::Pending, None);
        let running = TaskEvent::new(
            task_id,
            2,
            TaskEventKind::StateChanged,
            TaskState::Running,
            Some("picked up after restart".to_string()),
        );

        let first = storage.append_task_event(task_id, created.clone()).await;
        let second = storage.append_task_event(task_id, running.clone()).await;
        let loaded = storage.load_task_events(task_id).await;

        assert!(first.is_ok());
        assert!(second.is_ok());
        assert_eq!(loaded.ok(), Some(vec![created, running]));
    }

    #[tokio::test]
    async fn storage_task_events_reject_duplicate_sequence_values() {
        let storage = InMemoryStorage::default();
        let task_id = TaskMetadata::new().id;
        let created = TaskEvent::new(task_id, 1, TaskEventKind::Created, TaskState::Pending, None);
        let duplicate = TaskEvent::new(
            task_id,
            1,
            TaskEventKind::StateChanged,
            TaskState::Running,
            Some("duplicate sequence".to_string()),
        );

        let first = storage.append_task_event(task_id, created).await;
        let duplicate_result = storage.append_task_event(task_id, duplicate).await;

        assert!(first.is_ok());
        assert!(matches!(
            duplicate_result,
            Err(StorageError::InvalidTaskEventSequence {
                task_id: actual_task_id,
                expected: 2,
                actual: 1,
            }) if actual_task_id == task_id
        ));
    }

    #[tokio::test]
    async fn storage_task_events_reject_out_of_order_sequence_values() {
        let storage = InMemoryStorage::default();
        let task_id = TaskMetadata::new().id;
        let skipped = TaskEvent::new(
            task_id,
            2,
            TaskEventKind::StateChanged,
            TaskState::Running,
            Some("skipped initial sequence".to_string()),
        );

        let result = storage.append_task_event(task_id, skipped).await;

        assert!(matches!(
            result,
            Err(StorageError::InvalidTaskEventSequence {
                task_id: actual_task_id,
                expected: 1,
                actual: 2,
            }) if actual_task_id == task_id
        ));
    }

    #[tokio::test]
    async fn storage_task_events_reject_mismatched_task_ids() {
        let storage = InMemoryStorage::default();
        let task_id = TaskMetadata::new().id;
        let other_task_id = TaskMetadata::new().id;
        let event = TaskEvent::new(
            other_task_id,
            1,
            TaskEventKind::Created,
            TaskState::Pending,
            None,
        );

        let result = storage.append_task_event(task_id, event).await;

        assert!(matches!(
            result,
            Err(StorageError::TaskEventTaskMismatch { expected, actual })
                if expected == task_id && actual == other_task_id
        ));
    }
}
