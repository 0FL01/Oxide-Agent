//! Jira MCP provider for Jira Server 7.5.0 integration.
//!
//! Provides tools for reading, writing, and schema discovery via MCP protocol.

mod client;
mod config;
mod types;

pub use config::JiraMcpConfig;

// Placeholder for now - will be implemented in Slice 3
pub struct JiraMcpProvider;
