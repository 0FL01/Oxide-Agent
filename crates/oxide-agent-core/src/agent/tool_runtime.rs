//! Task-local runtime metadata for tool execution.

use crate::config::ModelInfo;
use std::future::Future;

tokio::task_local! {
    static ACTIVE_TOOL_MODEL_ROUTE: ModelInfo;
}

/// Run a future with a task-local active model route visible to tool providers.
pub(crate) async fn scope_tool_model_route<F, T>(route: ModelInfo, future: F) -> T
where
    F: Future<Output = T>,
{
    ACTIVE_TOOL_MODEL_ROUTE.scope(route, future).await
}

/// Returns the current task-local model route when tool execution is scoped.
/// Reserved for future use by tool providers that need to access the active model route.
#[must_use]
#[allow(dead_code)]
pub(crate) fn current_tool_model_route() -> Option<ModelInfo> {
    ACTIVE_TOOL_MODEL_ROUTE.try_with(Clone::clone).ok()
}
