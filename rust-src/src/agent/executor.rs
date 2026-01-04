//! Agent executor module
//!
//! Handles the iterative execution of tasks using LLM with tool calling.

use super::memory::AgentMessage;
use super::session::AgentSession;
use crate::config::{AGENT_MAX_ITERATIONS, AGENT_MODEL, AGENT_TIMEOUT_SECS};
use crate::llm::{LlmClient, Message};
use anyhow::{anyhow, Result};
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, instrument};

/// Agent executor that runs tasks iteratively
pub struct AgentExecutor {
    llm_client: Arc<LlmClient>,
    session: AgentSession,
}

impl AgentExecutor {
    /// Create a new agent executor
    pub fn new(llm_client: Arc<LlmClient>, session: AgentSession) -> Self {
        Self {
            llm_client,
            session,
        }
    }

    /// Get a reference to the session
    pub fn session(&self) -> &AgentSession {
        &self.session
    }

    /// Get a mutable reference to the session
    pub fn session_mut(&mut self) -> &mut AgentSession {
        &mut self.session
    }

    /// Convert AgentMessage history to LLM Message format
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

    /// Execute a task with iterative tool calling (agentic loop)
    #[instrument(skip(self, progress_tx), fields(user_id = self.session.user_id, chat_id = self.session.chat_id))]
    pub async fn execute(
        &mut self,
        task: &str,
        progress_tx: Option<tokio::sync::mpsc::Sender<super::progress::AgentEvent>>,
    ) -> Result<String> {
        use super::progress::AgentEvent;
        use super::providers::SandboxProvider;
        #[cfg(feature = "tavily")]
        use super::providers::TavilyProvider;
        use super::registry::ToolRegistry;

        // Start the task
        self.session.start_task();
        let task_id = self.session.current_task_id.clone().unwrap_or_default();

        info!(task = %task, task_id = %task_id, "Starting agent task with tool calling");

        // Add user message to memory
        self.session.memory.add_message(AgentMessage::user(task));

        // Create the system prompt
        let system_prompt = Self::create_agent_system_prompt();

        // Build tool registry with providers
        let mut registry = ToolRegistry::new();

        // Add sandbox provider
        registry.register(Box::new(SandboxProvider::new(self.session.user_id)));

        // Add Tavily provider if API key is set (requires "tavily" feature)
        #[cfg(feature = "tavily")]
        if let Ok(tavily_key) = std::env::var("TAVILY_API_KEY") {
            if !tavily_key.is_empty() {
                match TavilyProvider::new(&tavily_key) {
                    Ok(provider) => registry.register(Box::new(provider)),
                    Err(e) => debug!("Failed to create Tavily provider: {}", e),
                }
            }
        }

        let tools = registry.all_tools();

        // Build initial messages from memory using helper
        let mut messages = Self::convert_memory_to_messages(self.session.memory.get_messages());

        // Execute with timeout
        let timeout_duration = Duration::from_secs(AGENT_TIMEOUT_SECS);

        let result = timeout(timeout_duration, async {
            // Agentic loop
            for iteration in 0..AGENT_MAX_ITERATIONS {
                debug!(
                    task_id = %task_id,
                    iteration = iteration,
                    messages_count = messages.len(),
                    "Agent loop iteration"
                );

                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(AgentEvent::Thinking).await;
                }

                // Call LLM with tools
                let response = match self
                    .llm_client
                    .chat_with_tools(&system_prompt, &messages, &tools, AGENT_MODEL)
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        if let Some(ref tx) = progress_tx {
                            let _ = tx
                                .send(AgentEvent::Error(format!("LLM call failed: {}", e)))
                                .await;
                        }
                        return Err(anyhow!("LLM call failed: {}", e));
                    }
                };

                debug!(
                    task_id = %task_id,
                    tool_calls_count = response.tool_calls.len(),
                    finish_reason = %response.finish_reason,
                    "LLM response received"
                );

                // Check if there are no tool calls - this means final answer
                if response.tool_calls.is_empty() {
                    let final_response = response
                        .content
                        .unwrap_or_else(|| "Задача выполнена, но ответ пуст.".to_string());

                    // Add assistant response to memory
                    self.session
                        .memory
                        .add_message(AgentMessage::assistant(&final_response));
                    self.session.complete();

                    info!(
                        task_id = %task_id,
                        iterations = iteration + 1,
                        "Agent task completed successfully"
                    );

                    if let Some(ref tx) = progress_tx {
                        let _ = tx.send(AgentEvent::Finished).await;
                    }

                    return Ok(final_response);
                }

                // Add assistant message with tool calls placeholder
                // (We need to record that assistant requested tools)
                let tool_names: Vec<String> = response
                    .tool_calls
                    .iter()
                    .map(|tc| tc.function.name.clone())
                    .collect();
                messages.push(Message::assistant_with_tools(
                    &format!("[Вызов инструментов: {}]", tool_names.join(", ")),
                    response.tool_calls.clone(),
                ));

                // Execute each tool call via registry
                for tool_call in &response.tool_calls {
                    info!(
                        task_id = %task_id,
                        tool = %tool_call.function.name,
                        "Executing tool"
                    );

                    if let Some(ref tx) = progress_tx {
                        let _ = tx
                            .send(AgentEvent::ToolCall {
                                name: tool_call.function.name.clone(),
                                input: tool_call.function.arguments.clone(),
                            })
                            .await;
                    }

                    let result = match registry
                        .execute(&tool_call.function.name, &tool_call.function.arguments)
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => format!("Ошибка выполнения инструмента: {}", e),
                    };

                    if let Some(ref tx) = progress_tx {
                        let _ = tx
                            .send(AgentEvent::ToolResult {
                                name: tool_call.function.name.clone(),
                                output: result.clone(),
                            })
                            .await;
                    }

                    debug!(
                        task_id = %task_id,
                        tool = %tool_call.function.name,
                        result_len = result.len(),
                        "Tool execution completed"
                    );

                    // Add tool result to messages
                    messages.push(Message::tool(
                        &tool_call.id,
                        &tool_call.function.name,
                        &result,
                    ));
                }
            }

            // Max iterations reached
            self.session.fail("Превышен лимит итераций".to_string());
            Err(anyhow!(
                "Агент превысил лимит итераций ({}). Возможно, задача слишком сложная.",
                AGENT_MAX_ITERATIONS
            ))
        })
        .await;

        match result {
            Ok(inner_result) => inner_result,
            Err(_) => {
                self.session.timeout();
                Err(anyhow!(
                    "Задача превысила лимит времени ({} минут)",
                    AGENT_TIMEOUT_SECS / 60
                ))
            }
        }
    }

    /// Create the system prompt for the agent
    fn create_agent_system_prompt() -> String {
        let now = chrono::Local::now();
        let current_date = now.format("%Y-%m-%d %H:%M:%S").to_string();
        let current_day = now.format("%A").to_string();

        // Russian translation for the day of the week
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
            "### ТЕКУЩАЯ ДАТА И ВРЕМЯ\nСегодня: {}, {}\nВАЖНО: Всегда используй эту дату как текущую. Если результаты поиска (web_search) содержат фразы 'сегодня', 'завтра' или даты, которые противоречат этой, считай результаты поиска устаревшими и интерпретируй их относительно указанной выше даты.\n\n",
            current_date, current_day_ru
        );

        // Попытка прочитать промпт из файла AGENT.md
        let base_prompt = match std::fs::read_to_string("AGENT.md") {
            Ok(prompt) => {
                debug!("Loaded agent system prompt from AGENT.md");
                prompt
            }
            Err(e) => {
                error!(
                    "Failed to load AGENT.md: {}. Using default fallback prompt.",
                    e
                );
                // Fallback prompt on error
                r#"Ты - AI-агент с доступом к изолированной среде выполнения (sandbox) и веб-поиску.

## Доступные инструменты:
- **execute_command**: выполнить bash-команду в sandbox (доступны: python3, pip, curl, wget, date, cat, ls, grep и другие стандартные утилиты)
- **write_file**: записать содержимое в файл
- **read_file**: прочитать содержимое файла
- **web_search**: поиск информации в интернете
- **web_extract**: извлечение текста из веб-страниц

## Важные правила:
- Если нужны реальные данные (дата, время, сетевые запросы) - ИСПОЛЬЗУЙ ИНСТРУМЕНТЫ, не объясняй как это сделать
- Если нужна текущая дата - вызови execute_command с командой `date`
- Для вычислений используй Python: execute_command с `python3 -c "..."`
- Результаты выполнения инструментов будут возвращены тебе автоматически
- После получения результата инструмента - проанализируй его и дай окончательный ответ

## Формат ответа (когда даёшь окончательный ответ):
- Кратко опиши выполненные шаги
- Дай чёткий результат
- Используй markdown для форматирования"#.to_string()
            }
        };

        format!("{}{}", date_context, base_prompt)
    }

    /// Cancel the current task
    pub fn cancel(&mut self) {
        self.session
            .fail("Задача отменена пользователем".to_string());
    }

    /// Reset the executor and session
    pub async fn reset(&mut self) {
        self.session.reset().await;
    }

    /// Check if the session is timed out
    pub fn is_timed_out(&self) -> bool {
        self.session.is_timed_out()
    }
}
