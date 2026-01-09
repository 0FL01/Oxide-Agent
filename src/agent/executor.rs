//! Agent executor module
//!
//! Handles the iterative execution of tasks using LLM with tool calling.
//! This module is the main coordinator - delegates to submodules for:
//! - recovery: XML/JSON sanitization and malformed response recovery
//! - prompt: System prompt composition
//! - tool_bridge: Tool execution with timeout and progress events

use super::hooks::{CompletionCheckHook, HookContext, HookEvent, HookRegistry, HookResult};
use super::loop_detection::{LoopDetectionConfig, LoopDetectionService, LoopType};
use super::memory::AgentMessage;
use super::prompt::create_agent_system_prompt;
use super::providers::TodoList;
use super::recovery::{
    contains_xml_tags, looks_like_tool_call_text, sanitize_leaked_xml, sanitize_tool_calls,
    try_parse_malformed_tool_call,
};
use super::session::AgentSession;
use super::skills::SkillRegistry;
use super::tool_bridge::{execute_tool_calls, sync_todos_from_arc, ToolExecutionContext};
use crate::agent::progress::AgentEvent;
use crate::config::{
    get_agent_model, AGENT_CONTINUATION_LIMIT, AGENT_MAX_ITERATIONS, AGENT_TIMEOUT_SECS,
};
use crate::llm::{ChatResponse, LlmClient, Message, ToolCall, ToolDefinition};
use anyhow::{anyhow, Result};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{debug, info, warn};

// Re-export sanitize_xml_tags for backward compatibility
pub use super::recovery::sanitize_xml_tags as public_sanitize_xml_tags;

/// Agent executor that runs tasks iteratively
pub struct AgentExecutor {
    llm_client: Arc<LlmClient>,
    session: AgentSession,
    hook_registry: HookRegistry,
    loop_detector: Arc<Mutex<LoopDetectionService>>,
    loop_detection_disabled_next_run: bool,
    skill_registry: Option<SkillRegistry>,
}

/// Context for the agent execution loop to reduce argument count
struct AgentLoopContext<'a> {
    system_prompt: &'a str,
    tools: &'a [ToolDefinition],
    registry: &'a super::registry::ToolRegistry,
    progress_tx: Option<&'a tokio::sync::mpsc::Sender<AgentEvent>>,
    todos_arc: &'a Arc<Mutex<TodoList>>,
    task_id: &'a str,
    messages: &'a mut Vec<Message>,
}

struct CancelCleanupContext<'a> {
    progress_tx: Option<&'a tokio::sync::mpsc::Sender<AgentEvent>>,
    todos_arc: Option<&'a Arc<Mutex<TodoList>>>,
}

impl AgentExecutor {
    /// Create a new agent executor
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>, session: AgentSession) -> Self {
        // Initialize hook registry with default hooks
        let mut hook_registry = HookRegistry::new();
        hook_registry.register(Box::new(CompletionCheckHook::new()));

        let loop_config = Arc::new(LoopDetectionConfig::from_env());
        let loop_detector = Arc::new(Mutex::new(LoopDetectionService::new(
            llm_client.clone(),
            loop_config,
        )));

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
            llm_client,
            session,
            hook_registry,
            loop_detector,
            loop_detection_disabled_next_run: false,
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
        self.loop_detection_disabled_next_run = true;
    }

    /// Get the last task text, if available.
    #[must_use]
    pub fn last_task(&self) -> Option<&str> {
        self.session.last_task.as_deref()
    }

    /// Convert `AgentMessage` history to LLM Message format
    fn convert_memory_to_messages(messages: &[AgentMessage]) -> Vec<Message> {
        messages
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    super::memory::MessageRole::User => "user",
                    super::memory::MessageRole::Assistant => "assistant",
                    super::memory::MessageRole::System => "system",
                    super::memory::MessageRole::Tool => "tool",
                };
                Message {
                    role: role.to_string(),
                    content: msg.content.clone(),
                    tool_call_id: msg.tool_call_id.clone(),
                    name: msg.tool_name.clone(),
                    tool_calls: msg.tool_calls.clone(),
                }
            })
            .collect()
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
        use super::providers::{FileHosterProvider, SandboxProvider, TodosProvider, YtdlpProvider};
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

        {
            let mut detector = self.loop_detector.lock().await;
            detector.reset(task_id.clone());
            if self.loop_detection_disabled_next_run {
                detector.disable_for_session();
                self.loop_detection_disabled_next_run = false;
            }
        }

        // Build system prompt using the prompt module
        let system_prompt =
            create_agent_system_prompt(task, self.skill_registry.as_mut(), &mut self.session).await;

        let todos_arc = Arc::new(Mutex::new(self.session.memory.todos.clone()));

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TodosProvider::new(Arc::clone(&todos_arc))));
        let sandbox_provider = if let Some(ref tx) = progress_tx {
            SandboxProvider::new(self.session.user_id).with_progress_tx(tx.clone())
        } else {
            SandboxProvider::new(self.session.user_id)
        };
        registry.register(Box::new(sandbox_provider));

        // Register FileHosterProvider for uploading large files (GoFile)
        registry.register(Box::new(FileHosterProvider::new(self.session.user_id)));

        // Register YtdlpProvider for video platform tools
        let ytdlp_provider = if let Some(ref tx) = progress_tx {
            YtdlpProvider::new(self.session.user_id).with_progress_tx(tx.clone())
        } else {
            YtdlpProvider::new(self.session.user_id)
        };
        registry.register(Box::new(ytdlp_provider));

        #[cfg(feature = "tavily")]
        if let Ok(tavily_key) = std::env::var("TAVILY_API_KEY") {
            if !tavily_key.is_empty() {
                if let Ok(p) = TavilyProvider::new(&tavily_key) {
                    registry.register(Box::new(p));
                }
            }
        }

        let tools = registry.all_tools();
        let mut messages = Self::convert_memory_to_messages(self.session.memory.get_messages());
        let timeout_duration = Duration::from_secs(AGENT_TIMEOUT_SECS);

        let mut ctx = AgentLoopContext {
            system_prompt: &system_prompt,
            tools: &tools,
            registry: &registry,
            progress_tx: progress_tx.as_ref(),
            todos_arc: &todos_arc,
            task_id: &task_id,
            messages: &mut messages,
        };

        match timeout(timeout_duration, self.run_loop(&mut ctx)).await {
            Ok(inner) => match inner {
                Ok(res) => Ok(res),
                Err(e) => {
                    self.session.fail(e.to_string());
                    Err(e)
                }
            },
            Err(_) => {
                self.session.timeout();
                Err(anyhow!(
                    "Задача превысила лимит времени ({} минут)",
                    AGENT_TIMEOUT_SECS / 60
                ))
            }
        }
    }

    async fn run_loop(&mut self, ctx: &mut AgentLoopContext<'_>) -> Result<String> {
        let mut continuation_count: usize = 0;

        for iteration in 0..AGENT_MAX_ITERATIONS {
            // Check for cancellation at the start of each iteration
            if self.session.cancellation_token.is_cancelled() {
                return Err(self
                    .cancelled_error(CancelCleanupContext {
                        progress_tx: ctx.progress_tx,
                        todos_arc: Some(ctx.todos_arc),
                    })
                    .await);
            }

            debug!(task_id = %ctx.task_id, iteration = iteration, "Agent loop iteration");

            if let Some(tx) = ctx.progress_tx {
                let current_tokens = self.session.memory.token_count();
                let _ = tx
                    .send(AgentEvent::Thinking {
                        tokens: current_tokens,
                    })
                    .await;
            }

            if self.llm_loop_detected(iteration).await {
                return Err(self
                    .loop_detected_error(LoopType::CognitiveLoop, iteration, ctx)
                    .await);
            }

            let response = self
                .llm_client
                .chat_with_tools(
                    ctx.system_prompt,
                    ctx.messages,
                    ctx.tools,
                    get_agent_model(),
                )
                .await;

            if let Err(ref e) = response {
                if let Some(tx) = ctx.progress_tx {
                    let _ = tx
                        .send(AgentEvent::Error(format!("LLM call failed: {e}")))
                        .await;
                }
            }

            let mut response = response.map_err(|e| anyhow!("LLM call failed: {e}"))?;

            self.preprocess_llm_response(&mut response, ctx).await;

            let tool_calls = sanitize_tool_calls(response.tool_calls);

            if tool_calls.is_empty() {
                if let Some(content) = response.content.as_ref() {
                    if self.content_loop_detected(content).await {
                        return Err(self
                            .loop_detected_error(LoopType::ContentLoop, iteration, ctx)
                            .await);
                    }
                }

                match self
                    .handle_final_response(
                        response.content,
                        response.reasoning_content,
                        iteration,
                        &mut continuation_count,
                        ctx,
                    )
                    .await?
                {
                    Some(res) => return Ok(res),
                    None => continue,
                }
            }

            if self.tool_loop_detected(&tool_calls).await {
                return Err(self
                    .loop_detected_error(LoopType::ToolCallLoop, iteration, ctx)
                    .await);
            }

            self.execute_tools(tool_calls, ctx).await?;
        }

        self.session.fail("Превышен лимит итераций".to_string());
        Err(anyhow!(
            "Агент превысил лимит итераций ({AGENT_MAX_ITERATIONS})."
        ))
    }

    /// Preprocesses the LLM response: token sync, reasoning logging, and malformed call recovery
    async fn preprocess_llm_response(
        &mut self,
        response: &mut ChatResponse,
        ctx: &AgentLoopContext<'_>,
    ) {
        // Synchronize token count with API billing data
        if let Some(u) = &response.usage {
            self.session
                .memory
                .sync_token_count(u.total_tokens as usize);
        }

        // Log reasoning/thinking if present and send to progress
        if let Some(ref reasoning) = response.reasoning_content {
            debug!(reasoning_len = reasoning.len(), "Model reasoning received");

            // Send reasoning summary to progress display
            if let Some(tx) = ctx.progress_tx {
                let summary = super::thoughts::extract_reasoning_summary(reasoning, 100);
                let _ = tx.send(AgentEvent::Reasoning { summary }).await;
            }
        }

        // RECOVERY: Try to parse malformed tool calls from content
        // This handles cases where LLM generates XML-like syntax instead of proper JSON tool calls
        if response.tool_calls.is_empty() && response.content.is_some() {
            if let Some(content_str) = response.content.as_ref() {
                if let Some(recovered_call) = try_parse_malformed_tool_call(content_str) {
                    warn!(
                        model = %get_agent_model(),
                        tool_name = %recovered_call.function.name,
                        "METRIC: Recovered malformed tool call from content"
                    );
                    response.tool_calls.push(recovered_call);
                    response.content =
                        Some("[Tool call recovered from malformed response]".to_string());
                }
            }
        }
    }

    async fn cancelled_error(&mut self, ctx: CancelCleanupContext<'_>) -> anyhow::Error {
        self.session.memory.todos.clear();
        if let Some(todos_arc) = ctx.todos_arc {
            let mut todos = todos_arc.lock().await;
            todos.clear();
        }

        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::TodosUpdated {
                    todos: TodoList::new(),
                })
                .await;
            let _ = tx.send(AgentEvent::Cancelled).await;
        }

        anyhow!("Задача отменена пользователем")
    }

    async fn loop_detected_error(
        &self,
        loop_type: LoopType,
        iteration: usize,
        ctx: &AgentLoopContext<'_>,
    ) -> anyhow::Error {
        let event = {
            let detector = self.loop_detector.lock().await;
            detector.create_event(loop_type, iteration)
        };

        warn!(
            session_id = %event.session_id,
            loop_type = ?event.loop_type,
            iteration = event.iteration,
            "Loop detected in agent execution"
        );

        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::LoopDetected {
                    loop_type,
                    iteration,
                })
                .await;
        }

        anyhow!("Loop detected: {:?}", loop_type)
    }

    async fn llm_loop_detected(&self, iteration: usize) -> bool {
        let mut detector = self.loop_detector.lock().await;
        match detector
            .check_llm_periodic(&self.session.memory, iteration)
            .await
        {
            Ok(detected) => detected,
            Err(err) => {
                warn!(error = %err, "LLM loop check failed, continuing without LLM signal");
                false
            }
        }
    }

    async fn content_loop_detected(&self, content: &str) -> bool {
        if looks_like_tool_call_text(content) || contains_xml_tags(content) {
            let mut detector = self.loop_detector.lock().await;
            detector.reset_content_tracking();
            debug!(
                content_preview = %crate::utils::truncate_str(content, 80),
                "Skipping content loop detection for recovery-like content"
            );
            return false;
        }

        let mut detector = self.loop_detector.lock().await;
        match detector.check_content(content) {
            Ok(detected) => detected,
            Err(err) => {
                warn!(error = %err, "Content loop check failed, continuing");
                false
            }
        }
    }

    async fn tool_loop_detected(&self, tool_calls: &[ToolCall]) -> bool {
        let mut detector = self.loop_detector.lock().await;
        for tool_call in tool_calls {
            // Skip recovered tool calls to prevent false positive loop detection
            if tool_call.is_recovered {
                debug!(
                    tool_name = %tool_call.function.name,
                    "Skipping recovered tool call in loop detection"
                );
                detector.reset_content_tracking();
                continue;
            }
            match detector.check_tool_call(&tool_call.function.name, &tool_call.function.arguments)
            {
                Ok(true) => return true,
                Ok(false) => {}
                Err(err) => {
                    warn!(error = %err, "Tool loop check failed, continuing");
                }
            }
        }
        false
    }

    async fn bail_if_cancelled(&mut self, ctx: &mut AgentLoopContext<'_>) -> Result<()> {
        if !self.session.cancellation_token.is_cancelled() {
            return Ok(());
        }

        Err(self
            .cancelled_error(CancelCleanupContext {
                progress_tx: ctx.progress_tx,
                todos_arc: Some(ctx.todos_arc),
            })
            .await)
    }

    async fn force_continuation_due_to_bad_response(
        &self,
        continuation_count: &mut usize,
        ctx: &mut AgentLoopContext<'_>,
    ) {
        warn!("Response became empty after sanitization, forcing iteration to get real answer");
        *continuation_count += 1;
        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::Continuation {
                    reason: "Обнаружена ошибка генерации, повторяю попытку...".to_string(),
                    count: *continuation_count,
                })
                .await;
        }
        ctx.messages.push(Message::system(
            "[СИСТЕМА: Ваш предыдущий ответ содержал служебный XML-синтаксис вместо нормального текста. \
            ВАЖНО: \
            1. НЕ используйте XML-теги (<tool_call>, <filepath>, <arg_key> и т.д.) \
            2. НЕ повторяйте вызовы инструментов - они уже выполнены \
            3. Используйте ТОЛЬКО результаты уже выполненных инструментов \
            4. Отформатируйте ответ в виде обычного текста или markdown \
            Пожалуйста, предоставьте полноценный текстовый ответ на запрос пользователя.]",
        ));
    }

    fn after_agent_hook_result(
        &self,
        iteration: usize,
        continuation_count: usize,
        final_response: &str,
    ) -> HookResult {
        let hook_context = HookContext::new(
            &self.session.memory.todos,
            iteration,
            continuation_count,
            AGENT_CONTINUATION_LIMIT,
        );
        self.hook_registry.execute(
            &HookEvent::AfterAgent {
                response: final_response.to_string(),
            },
            &hook_context,
        )
    }

    fn save_final_response(&mut self, final_response: &str, reasoning: Option<String>) {
        if let Some(reasoning_content) = reasoning {
            self.session
                .memory
                .add_message(AgentMessage::assistant_with_reasoning(
                    final_response,
                    reasoning_content,
                ));
        } else {
            self.session
                .memory
                .add_message(AgentMessage::assistant(final_response));
        }
    }

    async fn handle_final_response(
        &mut self,
        content: Option<String>,
        reasoning: Option<String>,
        iteration: usize,
        continuation_count: &mut usize,
        ctx: &mut AgentLoopContext<'_>,
    ) -> Result<Option<String>> {
        self.bail_if_cancelled(ctx).await?;

        let mut final_response =
            content.unwrap_or_else(|| "Задача выполнена, но ответ пуст.".to_string());

        let xml_sanitized = sanitize_leaked_xml(iteration, &mut final_response);

        // BUGFIX AGENT-2026-001: Improved detection of malformed tool calls after XML sanitization
        // If XML was sanitized, check both for empty responses AND tool-like text patterns
        if xml_sanitized {
            // Check 1: Response became too short after sanitization
            if final_response.trim().len() < 10 {
                self.force_continuation_due_to_bad_response(continuation_count, ctx)
                    .await;
                return Ok(None);
            }

            // Check 2: Response contains tool call patterns (e.g., "[Вызов инструментов: ytdlp_...]")
            if looks_like_tool_call_text(&final_response) {
                warn!(
                    model = %crate::config::get_agent_model(),
                    iteration = iteration,
                    response_preview = %crate::utils::truncate_str(&final_response, 100),
                    "Detected tool call pattern in sanitized response, forcing continuation"
                );
                self.force_continuation_due_to_bad_response(continuation_count, ctx)
                    .await;
                return Ok(None);
            }
        }

        sync_todos_from_arc(&mut self.session, ctx.todos_arc).await;
        let hook_result =
            self.after_agent_hook_result(iteration, *continuation_count, &final_response);

        if let HookResult::ForceIteration { reason, context } = hook_result {
            *continuation_count += 1;
            if let Some(tx) = ctx.progress_tx {
                let _ = tx
                    .send(AgentEvent::Continuation {
                        reason: reason.clone(),
                        count: *continuation_count,
                    })
                    .await;
            }
            ctx.messages.push(Message::assistant(&final_response));
            ctx.messages.push(Message::system(&format!(
                "[СИСТЕМА: {reason}]\n\n{}",
                context.unwrap_or_default()
            )));
            return Ok(None);
        }

        self.save_final_response(&final_response, reasoning);

        self.session.complete();
        if let Some(tx) = ctx.progress_tx {
            let _ = tx.send(AgentEvent::Finished).await;
        }
        Ok(Some(final_response))
    }

    async fn execute_tools(
        &mut self,
        tool_calls: Vec<ToolCall>,
        ctx: &mut AgentLoopContext<'_>,
    ) -> Result<()> {
        // Load skill context for each tool before execution
        for tool_call in &tool_calls {
            let (name, _) = super::recovery::sanitize_tool_call(
                &tool_call.function.name,
                &tool_call.function.arguments,
            );
            self.load_skill_context_for_tool(&name, ctx).await?;
        }

        let mut tool_ctx = ToolExecutionContext {
            registry: ctx.registry,
            progress_tx: ctx.progress_tx,
            todos_arc: ctx.todos_arc,
            messages: ctx.messages,
        };

        execute_tool_calls(tool_calls, &mut self.session, &mut tool_ctx).await
    }

    async fn load_skill_context_for_tool(
        &mut self,
        tool_name: &str,
        ctx: &mut AgentLoopContext<'_>,
    ) -> Result<()> {
        let Some(registry) = self.skill_registry.as_mut() else {
            return Ok(());
        };

        let skill = match registry.load_skill_for_tool(tool_name).await {
            Ok(skill) => skill,
            Err(err) => {
                warn!(tool_name = %tool_name, error = %err, "Failed to load skill for tool");
                return Ok(());
            }
        };

        let Some(skill) = skill else {
            return Ok(());
        };

        if self.session.is_skill_loaded(&skill.metadata.name) {
            return Ok(());
        }

        let context_message = format!(
            "[Загружен модуль: {}]\n{}",
            skill.metadata.name, skill.content
        );

        self.session
            .memory
            .add_message(AgentMessage::system(context_message.clone()));
        ctx.messages.push(Message::system(&context_message));

        if self
            .session
            .register_loaded_skill(&skill.metadata.name, skill.token_count)
        {
            info!(
                skill = %skill.metadata.name,
                total_tokens = self.session.skill_token_count(),
                "Dynamic skill loaded"
            );
        }

        Ok(())
    }

    /// Check if the task has been cancelled
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.session.cancellation_token.is_cancelled()
    }

    /// Reset the executor and session
    pub fn reset(&mut self) {
        self.session.reset();
        self.loop_detection_disabled_next_run = false;
        if let Ok(mut detector) = self.loop_detector.try_lock() {
            detector.reset(String::new());
        }
    }

    /// Check if the session is timed out
    #[must_use]
    pub fn is_timed_out(&self) -> bool {
        self.session.is_timed_out()
    }
}

// All tests have been moved to recovery.rs and other specific modules
