//! Adapter from legacy [`ToolProvider`] implementations to typed runtime executors.

use super::config::ToolRuntimeConfig;
use super::executor::ToolExecutor;
use super::invocation::ToolInvocation;
use super::normalizer::{OutputNormalizer, ToolRuntimeError};
use super::output::ToolOutput;
use super::types::ToolName;
use crate::agent::progress::AgentEvent;
use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{mpsc::Sender, Mutex};

struct ProviderRuntimeExecutor {
    provider: Arc<dyn ToolProvider>,
    name: ToolName,
    spec: ToolDefinition,
    progress_tx: Option<Sender<AgentEvent>>,
    execution_lock: Arc<Mutex<()>>,
}

/// Build typed runtime executors for every tool exposed by a legacy provider.
#[must_use]
pub fn provider_runtime_executors(
    provider: Arc<dyn ToolProvider>,
    progress_tx: Option<Sender<AgentEvent>>,
) -> Vec<Arc<dyn ToolExecutor>> {
    let execution_lock = Arc::new(Mutex::new(()));
    provider
        .tools()
        .into_iter()
        .map(|spec| {
            Arc::new(ProviderRuntimeExecutor {
                provider: Arc::clone(&provider),
                name: ToolName::from(spec.name.clone()),
                spec,
                progress_tx: progress_tx.clone(),
                execution_lock: Arc::clone(&execution_lock),
            }) as Arc<dyn ToolExecutor>
        })
        .collect()
}

#[async_trait]
impl ToolExecutor for ProviderRuntimeExecutor {
    fn name(&self) -> ToolName {
        self.name.clone()
    }

    fn spec(&self) -> ToolDefinition {
        self.spec.clone()
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let _guard = self.execution_lock.lock().await;
        let output = self
            .provider
            .execute(
                self.name.as_str(),
                &invocation.raw_arguments,
                self.progress_tx.as_ref(),
                Some(&invocation.cancellation_token),
            )
            .await
            .map_err(|error| ToolRuntimeError::Failure(error.to_string()))?;
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            artifact_dir: invocation.execution_context.artifact_dir.clone(),
            ..ToolRuntimeConfig::default()
        });
        Ok(normalizer.success(&invocation, &output, ""))
    }
}
