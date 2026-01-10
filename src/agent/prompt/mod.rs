//! Prompt module
//!
//! Contains prompt composition logic for the agent.

pub mod composer;

pub use composer::{create_agent_system_prompt, create_sub_agent_system_prompt};
