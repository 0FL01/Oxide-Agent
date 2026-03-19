use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::get_object::GetObjectError;
use thiserror::Error;

/// Errors that can occur during storage operations.
#[derive(Error, Debug)]
pub enum StorageError {
    /// Error retrieving object from S3.
    #[error("S3 Get error: {0}")]
    S3Get(Box<SdkError<GetObjectError>>),
    /// Error putting object into S3.
    #[error("S3 put error: {0}")]
    S3Put(String),
    /// Error during JSON serialization or deserialization.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// Standard I/O error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// Configuration error (missing credentials, etc.).
    #[error("Configuration error: {0}")]
    Config(String),
    /// Invalid storage input.
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    /// Duplicate topic prompt content across context and AGENTS.md stores.
    #[error(
        "duplicate topic prompt content for topic {topic_id}: {attempted_kind} matches existing {existing_kind}; store AGENTS.md only in topic_agents_md and keep topic_context for short operational context"
    )]
    DuplicateTopicPromptContent {
        /// Stable topic identifier.
        topic_id: String,
        /// Existing store containing the same normalized payload.
        existing_kind: String,
        /// Store that attempted to write the duplicate payload.
        attempted_kind: String,
    },
    /// Optimistic concurrency retries exhausted.
    #[error("Concurrent update conflict for key {key} after {attempts} attempts")]
    ConcurrencyConflict {
        /// Storage object key that could not be updated.
        key: String,
        /// Number of retry attempts performed.
        attempts: usize,
    },
}
