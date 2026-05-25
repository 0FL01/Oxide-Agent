//! Deterministic executor registry for the async tool runtime.

use super::executor::ToolExecutor;
use super::invocation::ToolInvocation;
use super::normalizer::OutputNormalizer;
use super::output::ToolOutput;
use super::types::ToolName;
use crate::llm::ToolDefinition;
use std::collections::BTreeMap;
use std::sync::Arc;
use thiserror::Error;

/// Registry construction error.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// Two executors attempted to register the same canonical name.
    #[error("duplicate tool registration: {name}")]
    DuplicateTool {
        /// Duplicate tool name.
        name: ToolName,
    },
}

/// Deterministic exact-name executor registry.
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<ToolName, Arc<dyn ToolExecutor>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    /// Register one executor.
    ///
    /// # Errors
    ///
    /// Returns `RegistryError::DuplicateTool` if the tool name already exists.
    pub fn register(&mut self, executor: Arc<dyn ToolExecutor>) -> Result<(), RegistryError> {
        let name = executor.name();
        if self.tools.contains_key(&name) {
            return Err(RegistryError::DuplicateTool { name });
        }

        self.tools.insert(name, executor);
        Ok(())
    }

    /// Lookup executor by exact canonical name.
    #[must_use]
    pub fn get(&self, name: &ToolName) -> Option<Arc<dyn ToolExecutor>> {
        self.tools.get(name).map(Arc::clone)
    }

    /// Deterministic tool definitions ordered by canonical tool name.
    #[must_use]
    pub fn specs(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|executor| executor.spec())
            .collect()
    }

    /// Deterministic tool names ordered by canonical name.
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tools
            .keys()
            .map(|name| name.as_str().to_string())
            .collect()
    }

    /// Execute by exact name or normalize unknown/error outcomes into output.
    #[must_use]
    pub async fn execute_or_normalize(
        &self,
        invocation: ToolInvocation,
        normalizer: &OutputNormalizer,
    ) -> ToolOutput {
        let Some(executor) = self.get(&invocation.tool_name) else {
            return normalizer.unknown_tool(&invocation, &self.tool_names());
        };

        match executor.execute(invocation.clone()).await {
            Ok(output) => output,
            Err(error) => normalizer.executor_error(&invocation, error),
        }
    }

    /// Number of registered executors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry has no executors.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::config::{ToolRuntimeConfig, ToolTimeoutConfig};
    use crate::agent::tool_runtime::invocation::{
        ModelMetadata, ProviderMetadata, ToolExecutionContext,
    };
    use crate::agent::tool_runtime::normalizer::ToolRuntimeError;
    use crate::agent::tool_runtime::output::ToolOutputStatus;
    use crate::agent::tool_runtime::types::{ToolBatchId, ToolCallId, TurnId};
    use crate::llm::InvocationId;
    use async_trait::async_trait;
    use chrono::Utc;
    use serde_json::json;
    use std::path::PathBuf;

    struct StaticExecutor {
        name: ToolName,
        result: StaticResult,
    }

    enum StaticResult {
        Success,
        Error(ToolRuntimeError),
    }

    #[async_trait]
    impl ToolExecutor for StaticExecutor {
        fn name(&self) -> ToolName {
            self.name.clone()
        }

        fn spec(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.as_str().to_string(),
                description: format!("{} test tool", self.name),
                parameters: json!({ "type": "object" }),
            }
        }

        async fn execute(
            &self,
            invocation: ToolInvocation,
        ) -> Result<ToolOutput, ToolRuntimeError> {
            match &self.result {
                StaticResult::Success => Ok(OutputNormalizer::new(ToolRuntimeConfig::default())
                    .success(&invocation, "ok", "")),
                StaticResult::Error(error) => Err(error.clone()),
            }
        }
    }

    #[test]
    fn duplicate_registration_fails_fast() {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(success_executor("read_file")))
            .expect("first registration");
        let error = registry
            .register(Arc::new(success_executor("read_file")))
            .expect_err("duplicate fails");

        assert_eq!(
            error,
            RegistryError::DuplicateTool {
                name: ToolName::from("read_file")
            }
        );
    }

    #[test]
    fn specs_are_deterministic_by_tool_name() {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(success_executor("write_file")))
            .expect("write registers");
        registry
            .register(Arc::new(success_executor("read_file")))
            .expect("read registers");

        let names: Vec<String> = registry.specs().into_iter().map(|spec| spec.name).collect();

        assert_eq!(names, vec!["read_file", "write_file"]);
    }

    #[tokio::test]
    async fn unknown_tool_returns_paired_output() {
        let registry = ToolRegistry::new();
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig::default());
        let invocation = test_invocation("missing_tool");

        let output = registry.execute_or_normalize(invocation, &normalizer).await;

        assert_eq!(output.status, ToolOutputStatus::UnknownTool);
        assert_eq!(output.tool_call_id.as_str(), "call_missing_tool");
    }

    #[tokio::test]
    async fn executor_error_is_normalized() {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(StaticExecutor {
                name: ToolName::from("read_file"),
                result: StaticResult::Error(ToolRuntimeError::InvalidArguments(
                    "bad args".to_string(),
                )),
            }))
            .expect("registers");
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig::default());

        let output = registry
            .execute_or_normalize(test_invocation("read_file"), &normalizer)
            .await;

        assert_eq!(output.status, ToolOutputStatus::InvalidArguments);
        assert_eq!(output.tool_call_id.as_str(), "call_read_file");
    }

    #[tokio::test]
    async fn registered_executor_runs_by_exact_name() {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(success_executor("read_file")))
            .expect("registers");
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig::default());

        let output = registry
            .execute_or_normalize(test_invocation("read_file"), &normalizer)
            .await;

        assert_eq!(output.status, ToolOutputStatus::Success);
        assert_eq!(output.stdout.text.as_deref(), Some("ok"));
    }

    fn success_executor(name: &str) -> StaticExecutor {
        StaticExecutor {
            name: ToolName::from(name),
            result: StaticResult::Success,
        }
    }

    fn test_invocation(tool_name: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(42),
            turn_id: TurnId::from("turn_1"),
            batch_id: ToolBatchId::from("batch_1"),
            batch_index: 0,
            invocation_id: InvocationId::from(format!("invocation_{tool_name}")),
            tool_call_id: ToolCallId::from(format!("call_{tool_name}")),
            provider_tool_call_id: None,
            tool_name: ToolName::from(tool_name),
            raw_provider_payload: json!({}),
            raw_arguments: "{}".to_string(),
            normalized_arguments: json!({}),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            timeout: ToolTimeoutConfig::default(),
            execution_context: ToolExecutionContext::new(PathBuf::from(".oxide/tool-artifacts")),
            provider_metadata: ProviderMetadata {
                provider: "opencode-go".to_string(),
                protocol: "chat_like".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "deepseek-v4-flash".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: now,
            started_at: Some(now),
        }
    }
}
