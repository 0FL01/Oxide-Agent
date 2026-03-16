//! Docker sandbox management for Agent Mode
//!
//! Provides isolated execution environments for agents using Docker containers.

pub mod manager;
pub mod scope;

pub use manager::{ExecResult, SandboxContainerRecord, SandboxManager};
pub use scope::SandboxScope;
