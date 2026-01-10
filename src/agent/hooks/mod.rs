//! Agent Hooks Module
//!
//! Provides a hook system for intercepting and customizing agent behavior
//! at various points in the agent lifecycle.

pub mod completion;
pub mod delegation_guard;
pub mod registry;
pub mod sub_agent_safety;
pub mod types;
pub mod workload;

pub use completion::CompletionCheckHook;
pub use delegation_guard::DelegationGuardHook;
pub use registry::{Hook, HookRegistry};
pub use sub_agent_safety::{SubAgentSafetyConfig, SubAgentSafetyHook};
pub use types::{HookContext, HookEvent, HookResult};
pub use workload::WorkloadDistributorHook;
