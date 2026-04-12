//! Types for loop detection.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Types of detected loops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopType {
    /// Repeated identical tool calls.
    ToolCallLoop,
    /// Repeated identical content chunks.
    ContentLoop,
    /// LLM-detected cognitive loop.
    CognitiveLoop,
}

/// Loop detection event metadata.
#[derive(Debug, Clone)]
pub struct LoopDetectedEvent {
    /// Loop type.
    pub loop_type: LoopType,
    /// Session identifier.
    pub session_id: String,
    /// Iteration where the loop was detected.
    pub iteration: usize,
    /// Event timestamp.
    pub timestamp: DateTime<Utc>,
}

/// Errors produced by loop detection components.
#[derive(Debug, Error)]
pub enum LoopDetectionError {
    /// LLM request or response errors.
    #[error("LLM loop check failed: {0}")]
    LlmFailure(String),
}
