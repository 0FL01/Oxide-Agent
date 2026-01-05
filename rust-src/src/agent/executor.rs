//! Agent executor module
//!
//! Handles the iterative execution of tasks using LLM with tool calling.

use super::hooks::{CompletionCheckHook, HookContext, HookEvent, HookRegistry, HookResult};
use super::memory::AgentMessage;
use super::session::AgentSession;
use crate::agent::providers::TodoList;
use crate::config::{
    AGENT_CONTINUATION_LIMIT, AGENT_MAX_ITERATIONS, AGENT_MODEL, AGENT_TIMEOUT_SECS,
};
use crate::llm::{LlmClient, Message, ToolCall, ToolDefinition};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, instrument, warn};

/// Agent executor that runs tasks iteratively
pub struct AgentExecutor {
    llm_client: Arc<LlmClient>,
    session: AgentSession,
    hook_registry: HookRegistry,
}

/// Context for the agent execution loop to reduce argument count
struct AgentLoopContext<'a> {
    system_prompt: &'a str,
    tools: &'a [ToolDefinition],
    registry: &'a super::registry::ToolRegistry,
    progress_tx: Option<&'a tokio::sync::mpsc::Sender<super::progress::AgentEvent>>,
    todos_arc: &'a Arc<Mutex<TodoList>>,
    task_id: &'a str,
    messages: &'a mut Vec<Message>,
}

impl AgentExecutor {
    /// Create a new agent executor
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>, session: AgentSession) -> Self {
        // Initialize hook registry with default hooks
        let mut hook_registry = HookRegistry::new();
        hook_registry.register(Box::new(CompletionCheckHook::new()));

        Self {
            llm_client,
            session,
            hook_registry,
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

    /// Convert `AgentMessage` history to LLM Message format
    fn convert_memory_to_messages(messages: &[AgentMessage]) -> Vec<Message> {
        messages
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    super::memory::MessageRole::User => "user",
                    super::memory::MessageRole::Assistant => "assistant",
                    super::memory::MessageRole::System => "system",
                };
                Message {
                    role: role.to_string(),
                    content: msg.content.clone(),
                    tool_call_id: None,
                    name: None,
                    tool_calls: None,
                }
            })
            .collect()
    }

    /// Sanitize tool call by detecting malformed LLM responses where JSON arguments are placed in tool name
    /// Returns (`corrected_name`, `corrected_arguments`)
    fn sanitize_tool_call(name: &str, arguments: &str) -> (String, String) {
        let trimmed_name = name.trim();

        // Check if name looks like it contains JSON (starts with { and has "todos" key)
        if trimmed_name.starts_with('{') && trimmed_name.contains("\"todos\"") {
            warn!(
                tool_name = %name,
                "Detected malformed tool call: JSON arguments in tool name field"
            );

            // Try to extract first valid JSON object from name
            let Some(json_str) = Self::extract_first_json(trimmed_name) else {
                // If we can't parse, fall back to original
                warn!("Failed to extract JSON from malformed tool name");
                return (name.to_string(), arguments.to_string());
            };

            // Parse JSON to check structure
            if let Ok(parsed) = serde_json::from_str::<Value>(&json_str) {
                if parsed.is_object() && parsed.get("todos").is_some() {
                    warn!(
                        "Correcting malformed tool call to 'write_todos' with extracted arguments"
                    );
                    return ("write_todos".to_string(), json_str);
                }
            }
        }

        // Return unchanged if no issues detected
        (name.to_string(), arguments.to_string())
    }

    /// Extract first valid JSON object from a string
    /// This handles cases where JSON is followed by extra text
    fn extract_first_json(input: &str) -> Option<String> {
        let mut depth = 0;
        let mut start_idx = None;
        let mut in_string = false;
        let mut escaped = false;

        for (i, ch) in input.char_indices() {
            match ch {
                '{' if !in_string => {
                    if start_idx.is_none() {
                        start_idx = Some(i);
                    }
                    depth += 1;
                }
                '}' if !in_string => {
                    if depth == 1 {
                        if let Some(start) = start_idx {
                            // Found complete object
                            let json_str = input[start..=i].trim();
                            // Validate it's actually JSON
                            if serde_json::from_str::<Value>(json_str).is_ok() {
                                return Some(json_str.to_string());
                            }
                        }
                    }
                    depth -= 1;
                    if depth == 0 {
                        start_idx = None;
                    }
                }
                '"' if !escaped => {
                    in_string = !in_string;
                }
                '\\' if in_string => {
                    escaped = !escaped;
                }
                _ => {}
            }
            if ch != '\\' {
                escaped = false;
            }
        }

        None
    }

    /// Execute a task with iterative tool calling (agentic loop)
    ///
    /// # Errors
    ///
    /// Returns an error if the LLM call fails, tool execution fails, or the iteration/timeout limits are exceeded.
    #[instrument(skip(self, progress_tx), fields(user_id = self.session.user_id, chat_id = self.session.chat_id))]
    pub async fn execute(
        &mut self,
        task: &str,
        progress_tx: Option<tokio::sync::mpsc::Sender<super::progress::AgentEvent>>,
    ) -> Result<String> {
        #[cfg(feature = "tavily")]
        use super::providers::TavilyProvider;
        use super::providers::{SandboxProvider, TodosProvider};
        use super::registry::ToolRegistry;

        self.session.start_task();
        let task_id = self.session.current_task_id.clone().unwrap_or_default();
        info!(task = %task, task_id = %task_id, "Starting agent task");

        self.session.memory.add_message(AgentMessage::user(task));
        let system_prompt = Self::create_agent_system_prompt();
        let todos_arc = Arc::new(Mutex::new(self.session.memory.todos.clone()));

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TodosProvider::new(Arc::clone(&todos_arc))));
        let sandbox_provider = if let Some(ref tx) = progress_tx {
            SandboxProvider::new(self.session.user_id).with_progress_tx(tx.clone())
        } else {
            SandboxProvider::new(self.session.user_id)
        };
        registry.register(Box::new(sandbox_provider));

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

        let result = timeout(timeout_duration, self.run_loop(&mut ctx)).await;

        if let Ok(inner) = result {
            inner
        } else {
            self.session.timeout();
            Err(anyhow!(
                "Задача превысила лимит времени ({} минут)",
                AGENT_TIMEOUT_SECS / 60
            ))
        }
    }

    async fn run_loop(&mut self, ctx: &mut AgentLoopContext<'_>) -> Result<String> {
        use super::progress::AgentEvent;
        let mut continuation_count: usize = 0;

        for iteration in 0..AGENT_MAX_ITERATIONS {
            debug!(task_id = %ctx.task_id, iteration = iteration, "Agent loop iteration");

            if let Some(tx) = ctx.progress_tx {
                let _ = tx.send(AgentEvent::Thinking).await;
            }

            let response = self
                .llm_client
                .chat_with_tools(ctx.system_prompt, ctx.messages, ctx.tools, AGENT_MODEL)
                .await;

            if let Err(ref e) = response {
                if let Some(tx) = ctx.progress_tx {
                    let _ = tx
                        .send(AgentEvent::Error(format!("LLM call failed: {e}")))
                        .await;
                }
            }

            let response = response.map_err(|e| anyhow!("LLM call failed: {e}"))?;

            if response.tool_calls.is_empty() {
                match self
                    .handle_final_response(
                        response.content,
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

            self.execute_tool_calls(response.tool_calls, ctx).await?;
        }

        self.session.fail("Превышен лимит итераций".to_string());
        Err(anyhow!(
            "Агент превысил лимит итераций ({AGENT_MAX_ITERATIONS})."
        ))
    }

    async fn handle_final_response(
        &mut self,
        content: Option<String>,
        iteration: usize,
        continuation_count: &mut usize,
        ctx: &mut AgentLoopContext<'_>,
    ) -> Result<Option<String>> {
        use super::progress::AgentEvent;
        let final_response =
            content.unwrap_or_else(|| "Задача выполнена, но ответ пуст.".to_string());

        {
            let current_todos = ctx.todos_arc.lock().await;
            self.session.memory.todos = (*current_todos).clone();
        }

        let hook_context = HookContext::new(
            &self.session.memory.todos,
            iteration,
            *continuation_count,
            AGENT_CONTINUATION_LIMIT,
        );
        let hook_result = self.hook_registry.execute(
            &HookEvent::AfterAgent {
                response: final_response.clone(),
            },
            &hook_context,
        );

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

        self.session
            .memory
            .add_message(AgentMessage::assistant(&final_response));
        self.session.complete();
        if let Some(tx) = ctx.progress_tx {
            let _ = tx.send(AgentEvent::Finished).await;
        }
        Ok(Some(final_response))
    }

    async fn execute_tool_calls(
        &mut self,
        tool_calls: Vec<ToolCall>,
        ctx: &mut AgentLoopContext<'_>,
    ) -> Result<()> {
        use super::progress::AgentEvent;
        let tool_names: Vec<String> = tool_calls
            .iter()
            .map(|tc| tc.function.name.clone())
            .collect();
        ctx.messages.push(Message::assistant_with_tools(
            &format!("[Вызов инструментов: {}]", tool_names.join(", ")),
            tool_calls.clone(),
        ));

        for tool_call in tool_calls {
            let (name, args) =
                Self::sanitize_tool_call(&tool_call.function.name, &tool_call.function.arguments);
            if let Some(tx) = ctx.progress_tx {
                let _ = tx
                    .send(AgentEvent::ToolCall {
                        name: name.clone(),
                        input: args.clone(),
                    })
                    .await;
            }

            let result = match ctx.registry.execute(&name, &args).await {
                Ok(r) => r,
                Err(e) => format!("Ошибка выполнения инструмента: {e}"),
            };

            if name == "write_todos" {
                {
                    let current_todos = ctx.todos_arc.lock().await;
                    self.session.memory.todos = current_todos.clone();
                }
                if let Some(tx) = ctx.progress_tx {
                    let _ = tx
                        .send(AgentEvent::TodosUpdated {
                            todos: self.session.memory.todos.clone(),
                        })
                        .await;
                }
            }

            if let Some(tx) = ctx.progress_tx {
                let _ = tx
                    .send(AgentEvent::ToolResult {
                        name: name.clone(),
                        output: result.clone(),
                    })
                    .await;
            }
            ctx.messages
                .push(Message::tool(&tool_call.id, &name, &result));
        }
        Ok(())
    }

    /// Create the system prompt for the agent
    fn create_agent_system_prompt() -> String {
        let now = chrono::Local::now();
        let current_date = now.format("%Y-%m-%d %H:%M:%S").to_string();
        let current_day = now.format("%A").to_string();

        let current_day_ru = match current_day.as_str() {
            "Monday" => "понедельник",
            "Tuesday" => "вторник",
            "Wednesday" => "среда",
            "Thursday" => "четверг",
            "Friday" => "пятница",
            "Saturday" => "суббота",
            "Sunday" => "воскресенье",
            _ => &current_day,
        };

        let date_context = format!(
            "### ТЕКУЩАЯ ДАТА И ВРЕМЯ\nСегодня: {current_date}, {current_day_ru}\nВАЖНО: Всегда используй эту дату как текущую. Если результаты поиска (web_search) содержат фразы 'сегодня', 'завтра' или даты, которые противоречат этой, считай результаты поиска устаревшими и интерпретируй их относительно указанной выше даты.\n\n"
        );

        let base_prompt = match std::fs::read_to_string("AGENT.md") {
            Ok(prompt) => prompt,
            Err(e) => {
                error!("Failed to load AGENT.md: {e}. Using default fallback prompt.");
                r"Ты - AI-агент с доступом к изолированной среде выполнения (sandbox) и веб-поиску.
## Доступные инструменты:
- **execute_command**: выполнить bash-команду в sandbox
- **write_file**: записать содержимое в файл
- **read_file**: прочитать содержимое файла
- **web_search**: поиск информации в интернете
- **web_extract**: извлечение текста из веб-страниц
- **write_todos**: создать или обновить список задач
## Важные правила:
- Если нужны реальные данные - ИСПОЛЬЗУЙ ИНСТРУМЕНТЫ
- Для вычислений используй Python
- После получения результата инструмента - проанализируй его и дай окончательный ответ
- Для СЛОЖНЫХ запросов ОБЯЗАТЕЛЬНО используй write_todos для создания плана
## Формат ответа:
- Кратко опиши выполненные шаги
- Дай чёткий результат
- Используй markdown"
                    .to_string()
            }
        };

        format!("{date_context}{base_prompt}")
    }

    /// Cancel the current task
    pub fn cancel(&mut self) {
        self.session
            .fail("Задача отменена пользователем".to_string());
    }

    /// Reset the executor and session
    pub fn reset(&mut self) {
        self.session.reset();
    }

    /// Check if the session is timed out
    #[must_use]
    pub fn is_timed_out(&self) -> bool {
        self.session.is_timed_out()
    }
}
