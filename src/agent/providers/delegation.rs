//! Delegation provider for sub-agent execution.
//!
//! Exposes `delegate_to_sub_agent` tool that runs an isolated agent loop
//! with a lightweight model and restricted toolset.

use crate::agent::context::{AgentContext, EphemeralSession};
use crate::agent::hooks::{CompletionCheckHook, SubAgentSafetyConfig, SubAgentSafetyHook};
use crate::agent::memory::AgentMessage;
use crate::agent::prompt::create_sub_agent_system_prompt;
use crate::agent::provider::ToolProvider;
use crate::agent::providers::{FileHosterProvider, SandboxProvider, TodosProvider, YtdlpProvider};
use crate::agent::registry::ToolRegistry;
use crate::agent::runner::{AgentRunner, AgentRunnerConfig, AgentRunnerContext};
use crate::config::{
    AGENT_CONTINUATION_LIMIT, SUB_AGENT_MAX_ITERATIONS, SUB_AGENT_MAX_TOKENS, SUB_AGENT_MODEL_ZAI,
    SUB_AGENT_TIMEOUT_SECS,
};
use crate::llm::ToolDefinition;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};
use uuid::Uuid;

#[cfg(feature = "tavily")]
use crate::agent::providers::TavilyProvider;

const BLOCKED_SUB_AGENT_TOOLS: &[&str] = &["delegate_to_sub_agent", "send_file_to_user"];

/// Provider for sub-agent delegation tool.
pub struct DelegationProvider {
    llm_client: Arc<crate::llm::LlmClient>,
    user_id: i64,
}

impl DelegationProvider {
    /// Create a new delegation provider.
    #[must_use]
    pub fn new(llm_client: Arc<crate::llm::LlmClient>, user_id: i64) -> Self {
        Self {
            llm_client,
            user_id,
        }
    }

    fn blocked_tool_set() -> HashSet<String> {
        BLOCKED_SUB_AGENT_TOOLS
            .iter()
            .map(|tool| (*tool).to_string())
            .collect()
    }

    fn build_sub_agent_providers(
        &self,
        todos_arc: Arc<Mutex<crate::agent::providers::TodoList>>,
    ) -> Vec<Box<dyn ToolProvider>> {
        #[cfg(feature = "tavily")]
        let mut providers: Vec<Box<dyn ToolProvider>> = vec![
            Box::new(TodosProvider::new(todos_arc)),
            Box::new(SandboxProvider::new(self.user_id)),
            Box::new(FileHosterProvider::new(self.user_id)),
            Box::new(YtdlpProvider::new(self.user_id)),
        ];

        #[cfg(not(feature = "tavily"))]
        let providers: Vec<Box<dyn ToolProvider>> = vec![
            Box::new(TodosProvider::new(todos_arc)),
            Box::new(SandboxProvider::new(self.user_id)),
            Box::new(FileHosterProvider::new(self.user_id)),
            Box::new(YtdlpProvider::new(self.user_id)),
        ];

        #[cfg(feature = "tavily")]
        if let Ok(tavily_key) = std::env::var("TAVILY_API_KEY") {
            if !tavily_key.is_empty() {
                if let Ok(provider) = TavilyProvider::new(&tavily_key) {
                    providers.push(Box::new(provider));
                }
            }
        }

        providers
    }

    fn build_registry(
        &self,
        allowed_tools: &HashSet<String>,
        providers: Vec<Box<dyn ToolProvider>>,
    ) -> ToolRegistry {
        let allowed = Arc::new(allowed_tools.clone());
        let mut registry = ToolRegistry::new();
        for provider in providers {
            registry.register(Box::new(RestrictedToolProvider::new(
                provider,
                Arc::clone(&allowed),
            )));
        }
        registry
    }
}

#[async_trait]
impl ToolProvider for DelegationProvider {
    fn name(&self) -> &'static str {
        "delegation"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "delegate_to_sub_agent".to_string(),
            description: "Делегировать черновую работу легковесному саб-агенту. \
Передавай краткую, четкую задачу и список разрешенных инструментов. \
Можно добавить дополнительный контекст (например, выдержку из навыка)."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Задача для саб-агента"
                    },
                    "tools": {
                        "type": "array",
                        "description": "Белый список разрешенных инструментов",
                        "items": {"type": "string"}
                    },
                    "context": {
                        "type": "string",
                        "description": "Дополнительный контекст (опционально)"
                    }
                },
                "required": ["task", "tools"]
            }),
        }]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        tool_name == "delegate_to_sub_agent"
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        if tool_name != "delegate_to_sub_agent" {
            return Err(anyhow!("Unknown delegation tool: {tool_name}"));
        }

        let args: DelegateToSubAgentArgs = serde_json::from_str(arguments)?;
        if args.task.trim().is_empty() {
            return Err(anyhow!("Sub-agent task cannot be empty"));
        }
        if args.tools.is_empty() {
            return Err(anyhow!("Sub-agent tools whitelist cannot be empty"));
        }

        let DelegateToSubAgentArgs {
            task,
            tools: requested_tools,
            context,
        } = args;

        let mut sub_session = EphemeralSession::new(SUB_AGENT_MAX_TOKENS);
        sub_session
            .memory_mut()
            .add_message(AgentMessage::user(task.as_str()));

        let todos_arc = Arc::new(Mutex::new(sub_session.memory().todos.clone()));
        let providers = self.build_sub_agent_providers(Arc::clone(&todos_arc));
        let available_tools: HashSet<String> = providers
            .iter()
            .flat_map(|provider| provider.tools())
            .map(|tool| tool.name)
            .collect();

        let blocked = Self::blocked_tool_set();
        let requested: HashSet<String> = requested_tools.into_iter().collect();
        let allowed: HashSet<String> = requested
            .iter()
            .filter(|name| !blocked.contains(*name))
            .filter(|name| available_tools.contains(*name))
            .cloned()
            .collect();

        if allowed.is_empty() {
            return Err(anyhow!(
                "No allowed tools left after filtering (blocked or unavailable)"
            ));
        }

        let registry = self.build_registry(&allowed, providers);
        let tools = registry.all_tools();

        let mut messages =
            AgentRunner::convert_memory_to_messages(sub_session.memory().get_messages());

        let task_id = format!("sub-{}", Uuid::new_v4());
        let system_prompt =
            create_sub_agent_system_prompt(task.as_str(), &tools, context.as_deref());

        let mut runner = AgentRunner::new(self.llm_client.clone());
        runner.register_hook(Box::new(CompletionCheckHook::new()));
        runner.register_hook(Box::new(SubAgentSafetyHook::new(SubAgentSafetyConfig {
            max_iterations: SUB_AGENT_MAX_ITERATIONS,
            max_tokens: SUB_AGENT_MAX_TOKENS,
            blocked_tools: blocked,
        })));

        let mut ctx = AgentRunnerContext {
            task: task.as_str(),
            system_prompt: &system_prompt,
            tools: &tools,
            registry: &registry,
            progress_tx: None,
            todos_arc: &todos_arc,
            task_id: &task_id,
            messages: &mut messages,
            agent: &mut sub_session,
            skill_registry: None,
            config: AgentRunnerConfig::new(
                SUB_AGENT_MODEL_ZAI,
                SUB_AGENT_MAX_ITERATIONS,
                AGENT_CONTINUATION_LIMIT,
            ),
        };

        info!(task_id = %task_id, "Running sub-agent delegation");

        let timeout_duration = Duration::from_secs(SUB_AGENT_TIMEOUT_SECS);
        match timeout(timeout_duration, runner.run(&mut ctx)).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(err)) => Err(anyhow!("Sub-agent failed: {err}")),
            Err(_) => Err(anyhow!(
                "Sub-agent timed out after {} seconds",
                SUB_AGENT_TIMEOUT_SECS
            )),
        }
    }
}

#[derive(Debug, Deserialize)]
struct DelegateToSubAgentArgs {
    task: String,
    tools: Vec<String>,
    #[serde(default)]
    context: Option<String>,
}

struct RestrictedToolProvider {
    inner: Box<dyn ToolProvider>,
    allowed_tools: Arc<HashSet<String>>,
}

impl RestrictedToolProvider {
    fn new(inner: Box<dyn ToolProvider>, allowed_tools: Arc<HashSet<String>>) -> Self {
        Self {
            inner,
            allowed_tools,
        }
    }
}

#[async_trait]
impl ToolProvider for RestrictedToolProvider {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        self.inner
            .tools()
            .into_iter()
            .filter(|tool| self.allowed_tools.contains(&tool.name))
            .collect()
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        self.allowed_tools.contains(tool_name) && self.inner.can_handle(tool_name)
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        if !self.allowed_tools.contains(tool_name) {
            warn!(tool_name = %tool_name, "Tool blocked by delegation whitelist");
            return Err(anyhow!("Tool '{tool_name}' is not allowed for sub-agent"));
        }

        self.inner
            .execute(tool_name, arguments, cancellation_token)
            .await
    }
}
