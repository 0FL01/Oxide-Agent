//! Agent executor module
//!
//! Handles the iterative execution of tasks using LLM with tool calling.
//! This module is the main coordinator - delegates to submodules for:
//! - structured_output: strict JSON schema parsing and validation
//! - prompt: System prompt composition
//! - tool_bridge: Tool execution with timeout and progress events

use super::hooks::{CompletionCheckHook, HookContext, HookEvent, HookRegistry, HookResult};
use super::loop_detection::{LoopDetectionConfig, LoopDetectionService, LoopType};
use super::memory::AgentMessage;
use super::narrator::Narrator;
use super::prompt::create_agent_system_prompt;
use super::providers::TodoList;
use super::session::AgentSession;
use super::skills::SkillRegistry;
use super::structured_output::{parse_structured_output, StructuredOutputError, ValidatedToolCall};
use super::tool_bridge::{execute_tool_calls, sync_todos_from_arc, ToolExecutionContext};
use crate::agent::progress::AgentEvent;
use crate::config::{
    get_agent_model, AGENT_CONTINUATION_LIMIT, AGENT_MAX_ITERATIONS, AGENT_TIMEOUT_SECS,
};
use crate::llm::{ChatResponse, LlmClient, Message, ToolCall, ToolCallFunction, ToolDefinition};
use anyhow::{anyhow, Result};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{debug, info, warn};
use uuid::Uuid;

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
    narrator: Arc<Narrator>,
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

        let narrator = Arc::new(Narrator::new(llm_client.clone()));

        Self {
            llm_client,
            session,
            hook_registry,
            loop_detector,
            loop_detection_disabled_next_run: false,
            skill_registry,
            narrator,
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
        // Build system prompt using the prompt module (includes tool schema)
        let system_prompt = create_agent_system_prompt(
            task,
            &tools,
            self.skill_registry.as_mut(),
            &mut self.session,
        )
        .await;
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
                // Prefer API value if available for display
                let display_tokens = self
                    .session
                    .memory
                    .api_token_count()
                    .unwrap_or(current_tokens);

                let _ = tx
                    .send(AgentEvent::Thinking {
                        tokens: display_tokens,
                    })
                    .await;
            }

            if self.llm_loop_detected(iteration).await {
                return Err(self
                    .loop_detected_error(LoopType::CognitiveLoop, iteration, ctx)
                    .await);
            }

            let response = self.call_llm_with_tools(ctx).await?;
            if let Some(result) = self
                .handle_llm_response(response, iteration, &mut continuation_count, ctx)
                .await?
            {
                return Ok(result);
            }
        }

        self.session.fail("Превышен лимит итераций".to_string());
        Err(anyhow!(
            "Агент превысил лимит итераций ({AGENT_MAX_ITERATIONS})."
        ))
    }

    async fn call_llm_with_tools(&self, ctx: &AgentLoopContext<'_>) -> Result<ChatResponse> {
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

        response.map_err(|e| anyhow!("LLM call failed: {e}"))
    }

    async fn handle_llm_response(
        &mut self,
        mut response: ChatResponse,
        iteration: usize,
        continuation_count: &mut usize,
        ctx: &mut AgentLoopContext<'_>,
    ) -> Result<Option<String>> {
        self.preprocess_llm_response(&mut response, ctx).await;

        let raw_json = response
            .content
            .clone()
            .unwrap_or_default()
            .trim()
            .to_string();

        let parsed = match parse_structured_output(&raw_json, ctx.tools) {
            Ok(parsed) => parsed,
            Err(err) => {
                self.handle_structured_output_error(err, &raw_json, continuation_count, ctx)
                    .await;
                return Ok(None);
            }
        };

        let tool_calls = parsed
            .tool_call
            .map(|tool_call| vec![self.build_tool_call(tool_call)])
            .unwrap_or_default();

        // Async narrative generation (non-blocking sidecar LLM)
        self.spawn_narrative_task(
            response.reasoning_content.as_deref(),
            &tool_calls,
            ctx.progress_tx,
        );

        if tool_calls.is_empty() {
            let final_answer = parsed
                .final_answer
                .unwrap_or_else(|| "Задача выполнена, но ответ пуст.".to_string());

            if self.content_loop_detected(&final_answer).await {
                return Err(self
                    .loop_detected_error(LoopType::ContentLoop, iteration, ctx)
                    .await);
            }

            return self
                .handle_final_response(
                    final_answer,
                    &raw_json,
                    response.reasoning_content,
                    iteration,
                    continuation_count,
                    ctx,
                )
                .await;
        }

        if self.tool_loop_detected(&tool_calls).await {
            return Err(self
                .loop_detected_error(LoopType::ToolCallLoop, iteration, ctx)
                .await);
        }

        self.record_assistant_tool_call(&raw_json, &tool_calls, ctx);
        self.execute_tools(tool_calls, ctx).await?;
        Ok(None)
    }

    /// Preprocesses the LLM response: token sync and reasoning logging
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

        if response.content.is_none() {
            warn!(model = %get_agent_model(), "Model returned empty content");
        }
    }

    /// Spawns async narrative generation task (non-blocking sidecar LLM)
    fn spawn_narrative_task(
        &self,
        reasoning: Option<&str>,
        tool_calls: &[ToolCall],
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) {
        let Some(tx) = progress_tx else { return };

        let narrator = Arc::clone(&self.narrator);
        let reasoning = reasoning.map(str::to_string);
        let tool_calls = tool_calls.to_vec();
        let tx = tx.clone();

        tokio::spawn(async move {
            if let Some(narrative) = narrator.generate(reasoning.as_deref(), &tool_calls).await {
                let _ = tx
                    .send(AgentEvent::Narrative {
                        headline: narrative.headline,
                        content: narrative.content,
                    })
                    .await;
            }
        });
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

    fn build_tool_call(&self, tool_call: ValidatedToolCall) -> ToolCall {
        ToolCall {
            id: format!("call_{}", Uuid::new_v4()),
            function: ToolCallFunction {
                name: tool_call.name,
                arguments: tool_call.arguments_json,
            },
            is_recovered: false,
        }
    }

    fn record_assistant_tool_call(
        &mut self,
        raw_json: &str,
        tool_calls: &[ToolCall],
        ctx: &mut AgentLoopContext<'_>,
    ) {
        let tool_calls_vec = tool_calls.to_vec();
        ctx.messages.push(Message::assistant_with_tools(
            raw_json,
            tool_calls_vec.clone(),
        ));
        self.session
            .memory
            .add_message(AgentMessage::assistant_with_tools(
                raw_json.to_string(),
                tool_calls_vec,
            ));
    }

    async fn handle_structured_output_error(
        &self,
        error: StructuredOutputError,
        raw_json: &str,
        continuation_count: &mut usize,
        ctx: &mut AgentLoopContext<'_>,
    ) {
        warn!(
            error = %error,
            raw_preview = %crate::utils::truncate_str(raw_json, 200),
            "Structured output validation failed"
        );

        *continuation_count += 1;
        if let Some(tx) = ctx.progress_tx {
            let _ = tx
                .send(AgentEvent::Continuation {
                    reason: "Некорректный JSON-ответ, повторяю попытку...".to_string(),
                    count: *continuation_count,
                })
                .await;
        }

        let response_preview = crate::utils::truncate_str(raw_json, 400);
        let system_message = format!(
            "[СИСТЕМА: Ваш предыдущий ответ не соответствует строгой JSON-схеме.\nОшибка: {}\nОтвет: {}\nВерните ТОЛЬКО валидный JSON по указанной схеме без markdown, XML или текста вне JSON.]",
            error.message(),
            response_preview
        );
        ctx.messages.push(Message::system(&system_message));
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

    fn save_final_response(&mut self, raw_json: &str, reasoning: Option<String>) {
        if let Some(reasoning_content) = reasoning {
            self.session
                .memory
                .add_message(AgentMessage::assistant_with_reasoning(
                    raw_json,
                    reasoning_content,
                ));
        } else {
            self.session
                .memory
                .add_message(AgentMessage::assistant(raw_json));
        }
    }

    async fn handle_final_response(
        &mut self,
        final_answer: String,
        raw_json: &str,
        reasoning: Option<String>,
        iteration: usize,
        continuation_count: &mut usize,
        ctx: &mut AgentLoopContext<'_>,
    ) -> Result<Option<String>> {
        self.bail_if_cancelled(ctx).await?;

        let final_response = final_answer;

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
            ctx.messages.push(Message::assistant(raw_json));
            ctx.messages.push(Message::system(&format!(
                "[СИСТЕМА: {reason}]\n\n{}",
                context.unwrap_or_default()
            )));
            return Ok(None);
        }

        self.save_final_response(raw_json, reasoning);

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
            self.load_skill_context_for_tool(&tool_call.function.name, ctx)
                .await?;
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
