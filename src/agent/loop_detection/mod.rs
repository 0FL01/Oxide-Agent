//! Loop detection subsystem for agent execution.
//!
//! Provides deterministic and LLM-based detectors with a coordinating service.

mod config;
mod content_detector;
mod llm_detector;
mod service;
mod tool_detector;
mod types;

pub use config::LoopDetectionConfig;
pub use service::LoopDetectionService;
pub use types::{LoopDetectedEvent, LoopDetectionError, LoopType};
