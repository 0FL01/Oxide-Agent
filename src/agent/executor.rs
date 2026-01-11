//! Agent executor module
//!
//! Handles orchestration around the core agent runner, including
//! session lifecycle, skill prompts, and tool registry setup.

use super::hooks::{CompletionCheckHook, DelegationGuardHook, WorkloadDistributorHook};
use super::memory::AgentMessage;
use super::prompt::create_agent_system_prompt;
use super::runner::{AgentRunner, AgentRunnerConfig, AgentRunnerContext};
use super::session::AgentSession;
use super::skills::SkillRegistry;
use crate::agent::progress::AgentEvent;
use crate::config::AGENT_TIMEOUT_SECS;
use crate::llm::LlmClient;
use anyhow::{anyhow, Result};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

// Re-export sanitize_xml_tags for backward compatibility
pub use super::recovery::sanitize_xml_tags as public_sanitize_xml_tags;

/// Agent executor that runs tasks iteratively
pub struct AgentExecutor {
    runner: AgentRunner,
    session: AgentSession,
    skill_registry: Option<SkillRegistry>,
}

impl AgentExecutor {
    /// Create a new agent executor
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>, session: AgentSession) -> Self {
        let mut runner = AgentRunner::new(llm_client.clone());
        runner.register_hook(Box::new(CompletionCheckHook::new()));
        runner.register_hook(Box::new(WorkloadDistributorHook::new()));
        runner.register_hook(Box::new(DelegationGuardHook::new()));

        let skill_registry = match SkillRegistry::from_env(llm_client.clone()) {
            Ok(Some(registry)) => {
                info!(
                    skills_dir = %registry.skills_dir().display(),
                    "Skills system active"
                );
                Some(registry)
            }
            Ok(None) => {
                info!("Skills system inactive, will use AGENT.md or fallback prompt");
                None
            }
            Err(err) => {
                warn!(error = %err, "Failed to initialize skills registry, falling back to AGENT.md");
                None
            }
        };

        Self {
            runner,
            session,
            skill_registry,
        }
    }

    /// Get a reference to the session
    #[must_use]
    pub const fn session(&self) -> &AgentSession {
        &self.session
    }

    /// Get a mutable reference to the session
    pub const fn session_mut(&mut self) -> &mut AgentSession {
        &mut self.session
    }

    /// Disable loop detection for the next execution attempt.
    pub fn disable_loop_detection_next_run(&mut self) {
        self.runner.disable_loop_detection_next_run();
    }

    /// Get the last task text, if available.
    #[must_use]
    pub fn last_task(&self) -> Option<&str> {
        self.session.last_task.as_deref()
    }

    /// Execute a task with iterative tool calling (agentic loop)
    ///
    /// # Errors
    ///
    /// Returns an error if the LLM call fails, tool execution fails, or the iteration/timeout limits are exceeded.
    #[tracing::instrument(skip(self, progress_tx), fields(user_id = self.session.user_id, chat_id = self.session.chat_id))]
    pub async fn execute(
        &mut self,
        task: &str,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<String> {
        #[cfg(feature = "tavily")]
        use super::providers::TavilyProvider;
        use super::providers::{
            DelegationProvider, FileHosterProvider, SandboxProvider, TodosProvider, YtdlpProvider,
        };
        use super::registry::ToolRegistry;

        self.session.start_task();
        let task_id = self.session.current_task_id.clone().unwrap_or_default();
        self.session.remember_task(task);
        info!(
            task = %task,
            task_id = %task_id,
            memory_messages = self.session.memory.get_messages().len(),
            memory_tokens = self.session.memory.token_count(),
            "Starting agent task"
        );

        self.session.memory.add_message(AgentMessage::user(task));

        let todos_arc = Arc::new(Mutex::new(self.session.memory.todos.clone()));

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TodosProvider::new(Arc::clone(&todos_arc))));
        let sandbox_provider = if let Some(ref tx) = progress_tx {
            SandboxProvider::new(self.session.user_id).with_progress_tx(tx.clone())
        } else {
            SandboxProvider::new(self.session.user_id)
        };
        registry.register(Box::new(sandbox_provider));
        registry.register(Box::new(FileHosterProvider::new(self.session.user_id)));

        let ytdlp_provider = if let Some(ref tx) = progress_tx {
            YtdlpProvider::new(self.session.user_id).with_progress_tx(tx.clone())
        } else {
            YtdlpProvider::new(self.session.user_id)
        };
        registry.register(Box::new(ytdlp_provider));

        registry.register(Box::new(DelegationProvider::new(
            self.runner.llm_client(),
            self.session.user_id,
        )));

        #[cfg(feature = "tavily")]
        if let Ok(tavily_key) = std::env::var("TAVILY_API_KEY") {
            if !tavily_key.is_empty() {
                if let Ok(p) = TavilyProvider::new(&tavily_key) {
                    registry.register(Box::new(p));
                }
            }
        }

        let tools = registry.all_tools();
        let system_prompt = create_agent_system_prompt(
            task,
            &tools,
            self.skill_registry.as_mut(),
            &mut self.session,
        )
        .await;
        let mut messages =
            AgentRunner::convert_memory_to_messages(self.session.memory.get_messages());

        let mut ctx = AgentRunnerContext {
            task,
            system_prompt: &system_prompt,
            tools: &tools,
            registry: &registry,
            progress_tx: progress_tx.as_ref(),
            todos_arc: &todos_arc,
            task_id: &task_id,
            messages: &mut messages,
            agent: &mut self.session,
            skill_registry: self.skill_registry.as_mut(),
            config: AgentRunnerConfig::default(),
        };

        let timeout_duration = Duration::from_secs(AGENT_TIMEOUT_SECS);
        match timeout(timeout_duration, self.runner.run(&mut ctx)).await {
            Ok(inner) => match inner {
                Ok(res) => {
                    self.session.complete();
                    Ok(res)
                }
                Err(e) => {
                    self.session.fail(e.to_string());
                    Err(e)
                }
            },
            Err(_) => {
                self.session.timeout();
                Err(anyhow!(
                    "Task exceeded timeout limit ({} minutes)",
                    AGENT_TIMEOUT_SECS / 60
                ))
            }
        }
    }

    /// Check if the task has been cancelled
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.session.cancellation_token.is_cancelled()
    }

    /// Reset the executor and session
    pub fn reset(&mut self) {
        self.session.reset();
        self.runner.reset();
    }

    /// Check if the session is timed out
    #[must_use]
    pub fn is_timed_out(&self) -> bool {
        self.session.is_timed_out()
    }
}

// All tests have been moved to recovery.rs and other specific modules
