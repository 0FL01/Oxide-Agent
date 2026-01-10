//! Agent Hooks Module
//!
//! Provides a hook system for intercepting and customizing agent behavior
//! at various points in the agent lifecycle.

pub mod completion;
pub mod complexity;
pub mod registry;
pub mod sub_agent_safety;
pub mod types;

pub use completion::CompletionCheckHook;
pub use complexity::ComplexityAnalyzerHook;
pub use registry::{Hook, HookRegistry};
pub use sub_agent_safety::{SubAgentSafetyConfig, SubAgentSafetyHook};
pub use types::{HookContext, HookEvent, HookResult};
