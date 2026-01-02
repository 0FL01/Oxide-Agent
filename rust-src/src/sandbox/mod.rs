//! Docker sandbox management for Agent Mode
//!
//! Provides isolated execution environments for agents using Docker containers.

pub mod manager;

pub use manager::{ExecResult, SandboxManager};
