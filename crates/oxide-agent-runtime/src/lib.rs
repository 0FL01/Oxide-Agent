#![deny(missing_docs)]
//! Oxide Agent runtime helpers.
//!
//! Provides transport-agnostic runtime orchestration for the agent.

/// Agent runtime modules.
pub mod agent;
/// Session registry and lifecycle utilities.
pub mod session_registry;
/// Task event publishing abstractions.
pub mod task_events;
/// Detached runtime executor for long-running tasks.
pub mod task_executor;
/// Boot-time persisted task reconciliation.
pub mod task_recovery;
/// Task registry and lifecycle utilities.
pub mod task_registry;
/// Detached background worker lifecycle utilities.
pub mod worker_manager;

pub use agent::runtime::{
    spawn_progress_runtime, AgentTransport, DeliveryMode, ProgressRuntimeConfig,
};
pub use session_registry::SessionRegistry;
pub use task_events::{
    ChannelTaskEventPublisher, NoopTaskEventPublisher, SharedTaskEventPublisher, TaskEventPublisher,
};
pub use task_executor::{
    DetachedTaskSubmission, TaskExecutionBackend, TaskExecutionRequest, TaskExecutor,
    TaskExecutorError, TaskExecutorOptions,
};
pub use task_recovery::{TaskRecovery, TaskRecoveryError, TaskRecoveryOptions, TaskRecoveryReport};
pub use task_registry::{TaskRecord, TaskRegistry, TaskRegistryError};
pub use worker_manager::{WorkerManager, WorkerManagerError};
