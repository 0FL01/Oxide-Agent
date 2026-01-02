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
        Self { llm_client, session }
    }

    /// Get a reference to the session
    pub fn session(&self) -> &AgentSession {
        &self.session
    }

    /// Get a mutable reference to the session
    pub fn session_mut(&mut self) -> &mut AgentSession {
        &mut self.session
    }

    /// Execute a task with progress reporting
    ///
    /// Returns a channel receiver for progress updates and spawns the execution task.
    /// The final result is sent as the last update with `is_final = true`.
    pub async fn execute_with_progress(
        &mut self,
        task: &str,
    ) -> Result<mpsc::Receiver<ProgressUpdate>> {
        let (tx, rx) = mpsc::channel::<ProgressUpdate>(32);

        // Start the task
        self.session.start_task();

        // Add user message to memory
        self.session.memory.add_message(AgentMessage::user(task));

        // Clone what we need for the async task
        let llm_client = self.llm_client.clone();
        let task_str = task.to_string();
        let memory_messages = self.session.memory.get_messages().to_vec();

        // Spawn the execution task
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            let result = Self::run_agent_loop(llm_client, task_str, memory_messages, tx_clone).await;

            match result {
                Ok(response) => {
                    let _ = tx.send(ProgressUpdate {
                        step: response,
                        progress_percent: 100,
                        is_final: true,
                    }).await;
                }
                Err(e) => {
                    let _ = tx.send(ProgressUpdate {
                        step: format!("❌ Ошибка: {}", e),
                        progress_percent: 100,
                        is_final: true,
                    }).await;
                }
            }
        });

        Ok(rx)
    }

    /// Execute a task synchronously (blocking until complete or timeout)
    pub async fn execute(&mut self, task: &str) -> Result<String> {
        // Start the task
        self.session.start_task();

        // Add user message to memory
        self.session.memory.add_message(AgentMessage::user(task));

        // Create the system prompt for the agent
        let system_prompt = Self::create_agent_system_prompt();

        // Build conversation history
        let history = self.build_history_for_llm();

        // Execute with timeout
        let timeout_duration = Duration::from_secs(AGENT_TIMEOUT_SECS);

        match timeout(timeout_duration, self.call_agent(&system_prompt, &history, task)).await {
            Ok(result) => {
                match result {
                    Ok(response) => {
                        // Add assistant response to memory
                        self.session.memory.add_message(AgentMessage::assistant(&response));
                        self.session.complete();
                        Ok(response)
                    }
                    Err(e) => {
                        self.session.fail(e.to_string());
                        Err(e)
                    }
                }
            }
            Err(_) => {
                self.session.timeout();
                Err(anyhow!("Задача превысила лимит времени ({} минут)", AGENT_TIMEOUT_SECS / 60))
            }
        }
    }

    /// Run the agent loop with progress updates
    async fn run_agent_loop(
        llm_client: Arc<LlmClient>,
        task: String,
        memory_messages: Vec<AgentMessage>,
        progress_tx: mpsc::Sender<ProgressUpdate>,
    ) -> Result<String> {
        // Send initial progress
        let _ = progress_tx.send(ProgressUpdate {
            step: "🔄 Анализирую задачу...".to_string(),
            progress_percent: 10,
            is_final: false,
        }).await;

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
            });
        }

        // Update progress
        let _ = progress_tx.send(ProgressUpdate {
            step: "🧠 Выполняю задачу...".to_string(),
            progress_percent: 30,
            is_final: false,
        }).await;

        // Call the LLM
        let response = llm_client
            .chat_completion(&system_prompt, &messages, &task, AGENT_MODEL)
            .await
            .map_err(|e| anyhow!("LLM call failed: {}", e))?;

        // Update progress before finalizing
        let _ = progress_tx.send(ProgressUpdate {
            step: "✅ Формирую ответ...".to_string(),
            progress_percent: 90,
            is_final: false,
        }).await;

        Ok(response)
    }

    /// Call the agent LLM
    async fn call_agent(
        &self,
        system_prompt: &str,
        history: &[crate::llm::Message],
        user_message: &str,
    ) -> Result<String> {
        self.llm_client
            .chat_completion(system_prompt, history, user_message, AGENT_MODEL)
            .await
            .map_err(|e| anyhow!("Agent LLM call failed: {}", e))
    }

    /// Build LLM message history from agent memory
    fn build_history_for_llm(&self) -> Vec<crate::llm::Message> {
        self.session
            .memory
            .get_messages()
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    super::memory::MessageRole::User => "user",
                    super::memory::MessageRole::Assistant => "assistant",
                    super::memory::MessageRole::System => "system",
                };
                crate::llm::Message {
                    role: role.to_string(),
                    content: msg.content.clone(),
                }
            })
            .collect()
    }

    /// Create the system prompt for the agent
    fn create_agent_system_prompt() -> String {
        r#"Ты - AI-агент, специализирующийся на решении сложных задач.

## Твои возможности:
- Декомпозиция сложных задач на подзадачи
- Пошаговое решение с объяснениями
- Анализ и структурирование информации
- Генерация кода, текстов, планов

## Формат ответа:
1. **Анализ задачи** - кратко опиши понимание задачи
2. **План решения** - перечисли шаги (если задача сложная)
3. **Решение** - выполни задачу пошагово
4. **Итог** - краткое резюме результата

## Важно:
- Будь конкретным и практичным
- Если нужна дополнительная информация - запроси её
- При ошибках объясняй причину и предлагай альтернативы
- Используй markdown для форматирования"#.to_string()
    }

    /// Cancel the current task
    pub fn cancel(&mut self) {
        self.session.fail("Задача отменена пользователем".to_string());
    }

    /// Reset the executor and session
    pub fn reset(&mut self) {
        self.session.reset();
    }

    /// Check if the session is timed out
    pub fn is_timed_out(&self) -> bool {
        self.session.is_timed_out()
    }
}
