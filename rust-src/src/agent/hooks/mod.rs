//! Agent Hooks Module
//!
//! Provides a hook system for intercepting and customizing agent behavior
//! at various points in the agent lifecycle.

pub mod completion;
pub mod registry;
pub mod types;

pub use completion::CompletionCheckHook;
pub use registry::{Hook, HookRegistry};
pub use types::{HookContext, HookEvent, HookResult};
