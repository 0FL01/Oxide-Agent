use crate::agent::memory::AgentMemory;
use crate::config::Settings;
use aws_credential_types::Credentials;
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use aws_types::region::Region;
use thiserror::Error;
use tracing::{error, info};

use serde::{Deserialize, Serialize};

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("S3 Get error: {0}")]
    S3Get(Box<SdkError<GetObjectError>>),
    #[error("S3 put error: {0}")]
    S3Put(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Configuration error: {0}")]
    Config(String),
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct UserConfig {
    pub system_prompt: Option<String>,
    pub model_name: Option<String>,
    pub state: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
}

pub struct R2Storage {
    client: Client,
    bucket: String,
}

impl R2Storage {
    /// Create a new R2 storage instance
    ///
    /// # Errors
    ///
    /// Returns an error if R2 configuration is missing.
    pub async fn new(settings: &Settings) -> Result<Self, StorageError> {
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

        Ok(Self {
            client,
            bucket: bucket.clone(),
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
        let body = serde_json::to_string_pretty(data)?;

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(body.into_bytes()))
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
                    .map_err(|e| StorageError::Io(std::io::Error::other(e)))?;
                let json_data = serde_json::from_slice(&data.into_bytes())?;
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
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| StorageError::S3Put(e.to_string()))?;

        Ok(())
    }

    // --- High-level User Config Functions ---

    /// Get user configuration
    ///
    /// # Errors
    ///
    /// Returns an error if configuration loading fails.
    pub async fn get_user_config(&self, user_id: i64) -> Result<UserConfig, StorageError> {
        Ok(self
            .load_json(&user_config_key(user_id))
            .await?
            .unwrap_or_default())
    }

    /// Update user configuration
    ///
    /// # Errors
    ///
    /// Returns an error if saving fails.
    pub async fn update_user_config(
        &self,
        user_id: i64,
        config: UserConfig,
    ) -> Result<(), StorageError> {
        self.save_json(&user_config_key(user_id), &config).await
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

    /// Update user system prompt
    ///
    /// # Errors
    ///
    /// Returns an error if saving fails.
    pub async fn update_user_prompt(
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
    ///
    /// # Errors
    ///
    /// Returns an error if configuration loading fails.
    pub async fn get_user_prompt(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        let config = self.get_user_config(user_id).await?;
        Ok(config.system_prompt)
    }

    /// Update user model
    ///
    /// # Errors
    ///
    /// Returns an error if saving fails.
    pub async fn update_user_model(
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
    ///
    /// # Errors
    ///
    /// Returns an error if configuration loading fails.
    pub async fn get_user_model(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        let config = self.get_user_config(user_id).await?;
        Ok(config.model_name)
    }

    /// Update user state
    ///
    /// # Errors
    ///
    /// Returns an error if saving fails.
    pub async fn update_user_state(&self, user_id: i64, state: String) -> Result<(), StorageError> {
        self.modify_user_config(user_id, |config| {
            config.state = Some(state);
        })
        .await
    }

    /// Get user state
    ///
    /// # Errors
    ///
    /// Returns an error if configuration loading fails.
    pub async fn get_user_state(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        let config = self.get_user_config(user_id).await?;
        Ok(config.state)
    }

    // --- History Functions ---

    /// Save message to chat history
    ///
    /// # Errors
    ///
    /// Returns an error if history loading or saving fails.
    pub async fn save_message(
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
    ///
    /// # Errors
    ///
    /// Returns an error if history loading fails.
    pub async fn get_chat_history(
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
    ///
    /// # Errors
    ///
    /// Returns an error if deletion fails.
    pub async fn clear_chat_history(&self, user_id: i64) -> Result<(), StorageError> {
        self.delete_object(&user_history_key(user_id)).await
    }

    // --- Agent Memory Functions ---

    /// Save agent memory to storage
    ///
    /// # Errors
    ///
    /// Returns an error if saving fails.
    pub async fn save_agent_memory(
        &self,
        user_id: i64,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        self.save_json(&user_agent_memory_key(user_id), memory)
            .await
    }

    /// Load agent memory from storage
    ///
    /// # Errors
    ///
    /// Returns an error if loading fails.
    pub async fn load_agent_memory(
        &self,
        user_id: i64,
    ) -> Result<Option<AgentMemory>, StorageError> {
        self.load_json(&user_agent_memory_key(user_id)).await
    }

    /// Clear agent memory for a user
    ///
    /// # Errors
    ///
    /// Returns an error if deletion fails.
    pub async fn clear_agent_memory(&self, user_id: i64) -> Result<(), StorageError> {
        self.delete_object(&user_agent_memory_key(user_id)).await
    }

    /// Clear all context (history and memory) for a user
    ///
    /// # Errors
    ///
    /// Returns an error if clearing history or memory fails.
    pub async fn clear_all_context(&self, user_id: i64) -> Result<(), StorageError> {
        self.clear_chat_history(user_id).await?;
        self.clear_agent_memory(user_id).await?;
        Ok(())
    }

    /// Check connection to R2 storage
    ///
    /// # Errors
    ///
    /// Returns an error if listing buckets fails.
    pub async fn check_connection(&self) -> Result<(), String> {
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

#[must_use]
pub fn user_config_key(user_id: i64) -> String {
    format!("users/{user_id}/config.json")
}

#[must_use]
pub fn user_history_key(user_id: i64) -> String {
    format!("users/{user_id}/history.json")
}

#[must_use]
pub fn user_agent_memory_key(user_id: i64) -> String {
    format!("users/{user_id}/agent_memory.json")
}
