//! Agent executor using Rig framework
//!
//! Handles the iterative execution of tasks using Devstral model
//! with progress reporting and timeout management.

use super::memory::AgentMessage;
use super::session::AgentSession;
use crate::config::{AGENT_MODEL, AGENT_TIMEOUT_SECS};
use crate::llm::LlmClient;
use anyhow::{anyhow, Result};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, instrument, trace};

/// Progress update sent during task execution
#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    pub step: String,
    pub progress_percent: u8,
    pub is_final: bool,
}

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

    /// The final result is sent as the last update with `is_final = true`.
    #[instrument(skip(self), fields(user_id = self.session.user_id, chat_id = self.session.chat_id, task_id = %self.session.current_task_id.as_deref().unwrap_or("none")))]
    pub async fn execute_with_progress(
        &mut self,
        task: &str,
    ) -> Result<mpsc::Receiver<ProgressUpdate>> {
        // Start the task (generates task_id)
        self.session.start_task();
        let task_id = self.session.current_task_id.clone().unwrap_or_default();

        info!(task = %task, task_id = %task_id, "Starting agent task with progress reporting");
        let (tx, rx) = mpsc::channel::<ProgressUpdate>(32);

        // Add user message to memory
        self.session.memory.add_message(AgentMessage::user(task));

        // Clone what we need for the async task
        let llm_client = self.llm_client.clone();
        let task_str = task.to_string();
        let memory_messages = self.session.memory.get_messages().to_vec();

        // Spawn the execution task
        let tx_clone = tx.clone();
        let task_id_clone = task_id.clone();
        tokio::spawn(async move {
            let start = std::time::Instant::now();
            let result = Self::run_agent_loop(
                llm_client,
                task_str,
                memory_messages,
                tx_clone,
                task_id_clone.clone(),
            )
            .await;

            let duration = start.elapsed();
            match result {
                Ok(response) => {
                    info!(task_id = %task_id_clone, duration_ms = duration.as_millis(), "Agent task completed successfully");
                    let _ = tx
                        .send(ProgressUpdate {
                            step: response,
                            progress_percent: 100,
                            is_final: true,
                        })
                        .await;
                }
                Err(e) => {
                    error!(task_id = %task_id_clone, error = %e, duration_ms = duration.as_millis(), "Agent task failed");
                    let _ = tx
                        .send(ProgressUpdate {
                            step: format!("❌ Ошибка: {}", e),
                            progress_percent: 100,
                            is_final: true,
                        })
                        .await;
                }
            }
        });

        Ok(rx)
    }

    /// Execute a task with iterative tool calling (agentic loop)
    pub async fn execute(&mut self, task: &str) -> Result<String> {
        use super::tools::{execute_tool, get_agent_tools};
        use crate::llm::Message;

        const MAX_ITERATIONS: usize = 10;

        // Start the task
        self.session.start_task();
        let task_id = self.session.current_task_id.clone().unwrap_or_default();

        info!(task = %task, task_id = %task_id, "Starting agent task with tool calling");

        // Add user message to memory
        self.session.memory.add_message(AgentMessage::user(task));

        // Create the system prompt and get tools
        let system_prompt = Self::create_agent_system_prompt();
        let tools = get_agent_tools();

        // Build initial messages from memory
        let mut messages: Vec<Message> = self
            .session
            .memory
            .get_messages()
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
                }
            })
            .collect();

        // Execute with timeout
        let timeout_duration = Duration::from_secs(AGENT_TIMEOUT_SECS);

        let result = timeout(timeout_duration, async {
            // Agentic loop
            for iteration in 0..MAX_ITERATIONS {
                debug!(
                    task_id = %task_id,
                    iteration = iteration,
                    messages_count = messages.len(),
                    "Agent loop iteration"
                );

                // Call LLM with tools
                let response = self
                    .llm_client
                    .chat_with_tools(&system_prompt, &messages, &tools, AGENT_MODEL)
                    .await
                    .map_err(|e| anyhow!("LLM call failed: {}", e))?;

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
                    return Ok(final_response);
                }

                // Add assistant message with tool calls placeholder
                // (We need to record that assistant requested tools)
                let tool_names: Vec<String> = response
                    .tool_calls
                    .iter()
                    .map(|tc| tc.function.name.clone())
                    .collect();
                messages.push(Message::assistant(&format!(
                    "[Вызов инструментов: {}]",
                    tool_names.join(", ")
                )));

                // Ensure sandbox is running
                let sandbox = self
                    .session
                    .ensure_sandbox()
                    .await
                    .map_err(|e| anyhow!("Failed to create sandbox: {}", e))?;

                // Execute each tool call
                for tool_call in &response.tool_calls {
                    info!(
                        task_id = %task_id,
                        tool = %tool_call.function.name,
                        "Executing tool"
                    );

                    let result = execute_tool(
                        sandbox,
                        &tool_call.function.name,
                        &tool_call.function.arguments,
                    )
                    .await;

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
                MAX_ITERATIONS
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

    /// Run the agent loop with progress updates
    #[instrument(skip(llm_client, memory_messages, progress_tx), fields(task_id = %task_id))]
    async fn run_agent_loop(
        llm_client: Arc<LlmClient>,
        task: String,
        memory_messages: Vec<AgentMessage>,
        progress_tx: mpsc::Sender<ProgressUpdate>,
        task_id: String,
    ) -> Result<String> {
        debug!(task_id = %task_id, "Analyzing task via LLM");
        // Send initial progress
        let _ = progress_tx
            .send(ProgressUpdate {
                step: "🔄 Анализирую задачу...".to_string(),
                progress_percent: 10,
                is_final: false,
            })
            .await;

        let system_prompt = Self::create_agent_system_prompt();

        // Build LLM messages from memory
        let mut messages: Vec<crate::llm::Message> = Vec::new();
        for msg in &memory_messages {
            let role = match msg.role {
                super::memory::MessageRole::User => "user",
                super::memory::MessageRole::Assistant => "assistant",
                super::memory::MessageRole::System => "system",
            };
            messages.push(crate::llm::Message {
                role: role.to_string(),
                content: msg.content.clone(),
                tool_call_id: None,
                name: None,
            });
        }

        // Update progress
        let _ = progress_tx
            .send(ProgressUpdate {
                step: "🧠 Выполняю задачу...".to_string(),
                progress_percent: 30,
                is_final: false,
            })
            .await;

        // Call the LLM
        let call_start = std::time::Instant::now();
        let response = llm_client
            .chat_completion(&system_prompt, &messages, &task, AGENT_MODEL)
            .await
            .map_err(|e| anyhow!("LLM call failed: {}", e))?;
        let call_duration = call_start.elapsed();

        debug!(
            task_id = %task_id,
            duration_ms = call_duration.as_millis(),
            "LLM call completed"
        );

        // Update progress before finalizing
        let _ = progress_tx
            .send(ProgressUpdate {
                step: "✅ Формирую ответ...".to_string(),
                progress_percent: 90,
                is_final: false,
            })
            .await;

        trace!(response = ?response, "LLM Response received");
        Ok(response)
    }

    /// Create the system prompt for the agent
    fn create_agent_system_prompt() -> String {
        r#"Ты - AI-агент с доступом к изолированной среде выполнения (sandbox).

## Доступные инструменты:
- **execute_command**: выполнить bash-команду в sandbox (доступны: python3, pip, curl, wget, date, cat, ls, grep и другие стандартные утилиты)
- **write_file**: записать содержимое в файл
- **read_file**: прочитать содержимое файла

## Важные правила:
- Если нужны реальные данные (дата, время, сетевые запросы) - ИСПОЛЬЗУЙ ИНСТРУМЕНТЫ, не объясняй как это сделать
- Если нужна текущая дата - вызови execute_command с командой `date`
- Для вычислений используй Python: execute_command с `python3 -c "..."`
- Результаты выполнения инструментов будут возвращены тебе автоматически
- После получения результата инструмента - проанализируй его и дай окончательный ответ

## Формат ответа (когда даёшь окончательный ответ):
- Кратко опиши выполненные шаги
- Дай чёткий результат
- Используй markdown для форматирования"#
            .to_string()
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
