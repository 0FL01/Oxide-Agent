//! Agent executor module
//!
//! Handles the iterative execution of tasks using LLM with tool calling.

use super::hooks::{CompletionCheckHook, HookContext, HookEvent, HookRegistry, HookResult};
use super::memory::AgentMessage;
use super::session::AgentSession;
use crate::agent::providers::TodoList;
use crate::config::{
    get_agent_model, AGENT_CONTINUATION_LIMIT, AGENT_MAX_ITERATIONS, AGENT_TIMEOUT_SECS,
    AGENT_TOOL_TIMEOUT_SECS,
};
use crate::llm::{LlmClient, Message, ToolCall, ToolCallFunction, ToolDefinition};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, instrument, warn};

/// Sanitize XML-like tags from text
///
/// This removes any XML-like tags that may have leaked from malformed LLM responses.
/// Examples: `<tool_call>`, `</tool_call>`, `<filepath>`, `<arg_key>`, etc.
///
/// This function is public within the crate to allow reuse in progress tracking,
/// todo descriptions, and other agent components that need protection from XML leaks.
pub(crate) fn sanitize_xml_tags(text: &str) -> String {
    use lazy_regex::regex;

    // Pattern to match opening and closing XML tags: <tag_name> or </tag_name>
    // Matches lowercase letters, digits, underscores in tag names
    let xml_tag_pattern = regex!(r"</?[a-z_][a-z0-9_]*>");

    xml_tag_pattern.replace_all(text, "").to_string()
}

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

struct CancelCleanupContext<'a> {
    progress_tx: Option<&'a tokio::sync::mpsc::Sender<super::progress::AgentEvent>>,
    todos_arc: Option<&'a Arc<Mutex<TodoList>>>,
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

    /// Sanitize tool call by detecting malformed LLM responses where JSON arguments are placed in tool name
    /// Returns (`corrected_name`, `corrected_arguments`)
    fn sanitize_tool_call(name: &str, arguments: &str) -> (String, String) {
        let trimmed_name = name.trim();

        // PATTERN 1: Check if name looks like it contains JSON object (starts with { and has "todos" key)
        // Example: `{"todos": [{"description": "...", "status": "..."}]}`
        if trimmed_name.starts_with('{') && trimmed_name.contains("\"todos\"") {
            warn!(
                tool_name = %name,
                "Detected malformed tool call: JSON object in tool name field"
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

        // PATTERN 2: Check if name contains "todos" followed by JSON array
        // Example: `"todos [{"description": "...", "status": "in_progress"}, ...]"`
        // Example: `"write_todos [...]"`
        if (trimmed_name.contains("todos") || trimmed_name.contains("write_todos"))
            && trimmed_name.contains('[')
        {
            // Extract the base tool name (everything before '[')
            if let Some(bracket_pos) = trimmed_name.find('[') {
                let base_name = trimmed_name[..bracket_pos].trim();
                let json_part = trimmed_name[bracket_pos..].trim();

                // Validate base name is one of the expected variants
                if base_name == "todos" || base_name == "write_todos" {
                    warn!(
                        tool_name = %name,
                        base_name = %base_name,
                        "Detected malformed tool call: JSON array appended to tool name"
                    );

                    // Try to parse the JSON array part and wrap it in the expected structure
                    if let Ok(parsed_array) = serde_json::from_str::<Value>(json_part) {
                        if parsed_array.is_array() {
                            // Construct the proper arguments structure: {"todos": [...]}
                            let corrected_args = serde_json::json!({
                                "todos": parsed_array
                            });

                            if let Ok(args_str) = serde_json::to_string(&corrected_args) {
                                warn!(
                                    corrected_name = "write_todos",
                                    "Correcting malformed tool call: extracted array and wrapped in proper structure"
                                );
                                return ("write_todos".to_string(), args_str);
                            }
                        }
                    }

                    // If JSON parsing failed, log and fall back
                    warn!(
                        json_part = %json_part,
                        "Failed to parse JSON array from malformed tool name"
                    );
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

    /// Try to parse a malformed tool call from content text
    ///
    /// This handles cases where the LLM generates XML-like syntax instead of proper JSON tool calls.
    /// Example inputs:
    /// - "read_file<filepath>/workspace/docker-compose.yml</tool_call>"
    /// - "[Вызов инструментов: read_file]read_filepath..."
    /// - "execute_command<command>ls -la</command>"
    fn try_parse_malformed_tool_call(content: &str) -> Option<ToolCall> {
        use lazy_regex::regex;
        use uuid::Uuid;

        // List of known tool names to look for
        let tool_names = [
            "read_file",
            "write_file",
            "execute_command",
            "web_search",
            "web_extract",
            "list_files",
            "send_file_to_user",
            "upload_file",
            "write_todos",
        ];

        // Try to find a tool name in the content
        for tool_name in &tool_names {
            if !content.contains(tool_name) {
                continue;
            }

            // Try different patterns to extract arguments
            let arguments = match *tool_name {
                "read_file" => {
                    // Pattern: read_file<filepath>PATH</filepath> or read_filePATH</tool_call>
                    if let Some(caps) = regex!(r"read_file.*?<filepath>(.*?)</").captures(content) {
                        serde_json::json!({"path": caps.get(1).map(|m| m.as_str()).unwrap_or("")})
                    } else if let Some(caps) =
                        regex!(r"read_file(?:path)?([^\s<]+)").captures(content)
                    {
                        serde_json::json!({"path": caps.get(1).map(|m| m.as_str()).unwrap_or("")})
                    } else {
                        continue;
                    }
                }
                "write_file" => {
                    // Pattern: write_file<filepath>PATH</filepath><content>CONTENT</content>
                    let filepath = regex!(r"<filepath>(.*?)</")
                        .captures(content)
                        .and_then(|c| c.get(1))
                        .map(|m| m.as_str())
                        .unwrap_or("");
                    let file_content = regex!(r"<content>(.*?)</")
                        .captures(content)
                        .and_then(|c| c.get(1))
                        .map(|m| m.as_str())
                        .unwrap_or("");

                    if !filepath.is_empty() {
                        serde_json::json!({"path": filepath, "content": file_content})
                    } else {
                        continue;
                    }
                }
                "execute_command" => {
                    // Pattern: execute_command<command>CMD</command>
                    if let Some(caps) = regex!(r"<command>(.*?)</").captures(content) {
                        serde_json::json!({"command": caps.get(1).map(|m| m.as_str()).unwrap_or("")})
                    } else if let Some(caps) =
                        regex!(r"execute_command(?:command)?([^\s<]+)").captures(content)
                    {
                        serde_json::json!({"command": caps.get(1).map(|m| m.as_str()).unwrap_or("")})
                    } else {
                        continue;
                    }
                }
                "list_files" => {
                    // Pattern: list_files<directory>PATH</directory>
                    if let Some(caps) = regex!(r"<directory>(.*?)</").captures(content) {
                        serde_json::json!({"path": caps.get(1).map(|m| m.as_str()).unwrap_or("")})
                    } else {
                        // Default to current directory
                        serde_json::json!({"path": ""})
                    }
                }
                "send_file_to_user" => {
                    // Pattern: send_file_to_user<filepath>PATH</filepath> or send_file_to_user<path>PATH</path>
                    if let Some(caps) = regex!(r"<filepath>(.*?)</").captures(content) {
                        serde_json::json!({"path": caps.get(1).map(|m| m.as_str()).unwrap_or("")})
                    } else if let Some(caps) = regex!(r"<path>(.*?)</").captures(content) {
                        serde_json::json!({"path": caps.get(1).map(|m| m.as_str()).unwrap_or("")})
                    } else {
                        continue;
                    }
                }
                "upload_file" => {
                    // Pattern: upload_file<filepath>PATH</filepath> or upload_file<path>PATH</path>
                    if let Some(caps) = regex!(r"<filepath>(.*?)</").captures(content) {
                        serde_json::json!({"path": caps.get(1).map(|m| m.as_str()).unwrap_or("")})
                    } else if let Some(caps) = regex!(r"<path>(.*?)</").captures(content) {
                        serde_json::json!({"path": caps.get(1).map(|m| m.as_str()).unwrap_or("")})
                    } else {
                        continue;
                    }
                }
                _ => continue,
            };

            // Construct a valid ToolCall
            let arguments_str = serde_json::to_string(&arguments).ok()?;

            warn!(
                tool_name = tool_name,
                arguments = %arguments_str,
                "Recovered malformed tool call from content"
            );

            return Some(ToolCall {
                id: format!("recovered_{}", Uuid::new_v4()),
                function: ToolCallFunction {
                    name: tool_name.to_string(),
                    arguments: arguments_str,
                },
            });
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
        use super::providers::{FileHosterProvider, SandboxProvider, TodosProvider, YtdlpProvider};
        use super::registry::ToolRegistry;

        self.session.start_task();
        let task_id = self.session.current_task_id.clone().unwrap_or_default();
        info!(
            task = %task,
            task_id = %task_id,
            memory_messages = self.session.memory.get_messages().len(),
            memory_tokens = self.session.memory.token_count(),
            "Starting agent task"
        );

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

        // Register FileHosterProvider for uploading large files (Litterbox)
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
        use super::progress::AgentEvent;
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

            // Log reasoning/thinking if present
            if let Some(ref reasoning) = response.reasoning_content {
                debug!(reasoning_len = reasoning.len(), "Model reasoning received");
            }

            // RECOVERY: Try to parse malformed tool calls from content
            // This handles cases where LLM generates XML-like syntax instead of proper JSON tool calls
            if response.tool_calls.is_empty() && response.content.is_some() {
                if let Some(content_str) = response.content.as_ref() {
                    if let Some(recovered_call) = Self::try_parse_malformed_tool_call(content_str) {
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

            if response.tool_calls.is_empty() {
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

            self.execute_tool_calls(response.tool_calls, ctx).await?;
        }

        self.session.fail("Превышен лимит итераций".to_string());
        Err(anyhow!(
            "Агент превысил лимит итераций ({AGENT_MAX_ITERATIONS})."
        ))
    }

    async fn cancelled_error(&mut self, ctx: CancelCleanupContext<'_>) -> anyhow::Error {
        use super::progress::AgentEvent;

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

    fn sanitize_leaked_xml(iteration: usize, final_response: &mut String) -> bool {
        use lazy_regex::regex;

        // ANTI-LEAK PROTECTION: Detect and sanitize XML-like syntax leaking into final response
        // This handles malformed LLM responses where XML tags appear instead of proper tool calls
        let xml_tag_pattern = regex!(r"</?[a-z_][a-z0-9_]*>");
        if !xml_tag_pattern.is_match(final_response) {
            return false;
        }

        let original_len = final_response.len();
        warn!(
            model = %crate::config::get_agent_model(),
            iteration = iteration,
            "Detected leaked XML syntax in final response, sanitizing output"
        );

        // Remove all XML-like tags
        *final_response = sanitize_xml_tags(final_response);

        debug!(
            original_len = original_len,
            sanitized_len = final_response.len(),
            "XML tags removed from response"
        );
        true
    }

    async fn force_continuation_due_to_bad_response(
        &self,
        continuation_count: &mut usize,
        ctx: &mut AgentLoopContext<'_>,
    ) {
        use super::progress::AgentEvent;

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

    async fn sync_todos_from_arc(&mut self, todos_arc: &Arc<Mutex<TodoList>>) {
        let current_todos = todos_arc.lock().await;
        self.session.memory.todos = (*current_todos).clone();
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
        use super::progress::AgentEvent;
        self.bail_if_cancelled(ctx).await?;

        let mut final_response =
            content.unwrap_or_else(|| "Задача выполнена, но ответ пуст.".to_string());

        let xml_sanitized = Self::sanitize_leaked_xml(iteration, &mut final_response);
        if xml_sanitized && final_response.trim().len() < 10 {
            self.force_continuation_due_to_bad_response(continuation_count, ctx)
                .await;
            return Ok(None);
        }

        self.sync_todos_from_arc(ctx.todos_arc).await;
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

    async fn execute_tool_calls(
        &mut self,
        tool_calls: Vec<ToolCall>,
        ctx: &mut AgentLoopContext<'_>,
    ) -> Result<()> {
        let tool_names: Vec<String> = tool_calls
            .iter()
            .map(|tc| tc.function.name.clone())
            .collect();
        ctx.messages.push(Message::assistant_with_tools(
            &format!("[Вызов инструментов: {}]", tool_names.join(", ")),
            tool_calls.clone(),
        ));

        let assistant_msg = AgentMessage::assistant_with_tools(
            format!("[Вызов инструментов: {}]", tool_names.join(", ")),
            tool_calls.clone(),
        );
        self.session.memory.add_message(assistant_msg);

        for tool_call in tool_calls {
            self.execute_single_tool_call(tool_call, ctx).await?;
        }
        Ok(())
    }

    async fn execute_single_tool_call(
        &mut self,
        tool_call: ToolCall,
        ctx: &mut AgentLoopContext<'_>,
    ) -> Result<()> {
        use super::progress::AgentEvent;

        self.bail_if_cancelled(ctx).await?;

        let (name, args) =
            Self::sanitize_tool_call(&tool_call.function.name, &tool_call.function.arguments);

        info!(
            tool_name = %name,
            tool_args = %crate::utils::truncate_str(&args, 200),
            "Executing tool call"
        );

        if let Some(tx) = ctx.progress_tx {
            // Sanitize XML tags from tool name and input to prevent UI corruption
            // This protects against malformed LLM responses that leak XML syntax
            let _ = tx
                .send(AgentEvent::ToolCall {
                    name: sanitize_xml_tags(&name),
                    input: sanitize_xml_tags(&args),
                })
                .await;
        }

        // Execute tool with timeout and cancellation support
        let tool_timeout = Duration::from_secs(AGENT_TOOL_TIMEOUT_SECS);
        let result = {
            use tokio::select;
            select! {
                biased;
                _ = self.session.cancellation_token.cancelled() => {
                    warn!(tool_name = %name, "Tool execution cancelled by user");
                    if let Some(tx) = ctx.progress_tx {
                        let _ = tx.send(AgentEvent::Cancelling { tool_name: name.clone() }).await;
                        // Give UI time to show cancelling status (2 sec cleanup timeout)
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                    return Err(self
                        .cancelled_error(CancelCleanupContext {
                            progress_tx: ctx.progress_tx,
                            todos_arc: Some(ctx.todos_arc),
                        })
                        .await);
                },
                res = timeout(tool_timeout, ctx.registry.execute(&name, &args, Some(&self.session.cancellation_token))) => {
                    match res {
                        Ok(Ok(r)) => r,
                        Ok(Err(e)) => format!("Ошибка выполнения инструмента: {e}"),
                        Err(_) => {
                            warn!(
                                tool_name = %name,
                                timeout_secs = AGENT_TOOL_TIMEOUT_SECS,
                                "Tool execution timed out"
                            );
                            format!(
                                "Инструмент '{name}' превысил лимит времени ({} секунд)",
                                AGENT_TOOL_TIMEOUT_SECS
                            )
                        }
                    }
                },
            }
        };

        if name == "write_todos" {
            self.sync_todos_from_arc(ctx.todos_arc).await;
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
        let tool_msg = AgentMessage::tool(&tool_call.id, &name, &result);
        self.session.memory.add_message(tool_msg);

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
- **execute_command**: выполнить bash-команду в sandbox (доступны: python3, pip, ffmpeg, yt-dlp, curl, wget, date, cat, ls, grep и другие стандартные утилиты)
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

    /// Check if the task has been cancelled
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.session.cancellation_token.is_cancelled()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_tool_call_normal() {
        let (name, args) = AgentExecutor::sanitize_tool_call("write_todos", "{}");
        assert_eq!(name, "write_todos");
        assert_eq!(args, "{}");
    }

    #[test]
    fn test_sanitize_tool_call_json_object_in_name() {
        let malformed_name = r#"{"todos": [{"description": "Task 1", "status": "pending"}]}"#;
        let (name, args) = AgentExecutor::sanitize_tool_call(malformed_name, "{}");

        assert_eq!(name, "write_todos");
        // Should extract the JSON and use it as arguments
        assert!(args.contains("\"todos\""));
        assert!(args.contains("Task 1"));
    }

    #[test]
    fn test_sanitize_tool_call_array_appended_to_todos() {
        let malformed_name = r#"todos [{"description": "Task 1", "status": "in_progress"}]"#;
        let (name, args) = AgentExecutor::sanitize_tool_call(malformed_name, "{}");

        assert_eq!(name, "write_todos");
        // Should wrap array in proper structure
        let parsed = serde_json::from_str::<serde_json::Value>(&args)
            .expect("Failed to parse corrected arguments");
        assert!(parsed.get("todos").is_some());
        assert!(parsed["todos"].is_array());
    }

    #[test]
    fn test_sanitize_tool_call_array_appended_to_write_todos() {
        let malformed_name =
            r#"write_todos [{"description": "Update deps", "status": "completed"}]"#;
        let (name, args) = AgentExecutor::sanitize_tool_call(malformed_name, "{}");

        assert_eq!(name, "write_todos");
        let parsed = serde_json::from_str::<serde_json::Value>(&args)
            .expect("Failed to parse corrected arguments");
        assert!(parsed.get("todos").is_some());
        assert!(parsed["todos"].is_array());
        assert_eq!(parsed["todos"][0]["description"], "Update deps");
    }

    #[test]
    fn test_sanitize_tool_call_complex_array() {
        let malformed_name = r#"todos [
            {"description": "Обновление yt-dlp до последней версии", "status": "in_progress"},
            {"description": "Тестирование новой версии", "status": "pending"},
            {"description": "Документирование изменений", "status": "pending"}
        ]"#;
        let (name, args) = AgentExecutor::sanitize_tool_call(malformed_name, "{}");

        assert_eq!(name, "write_todos");
        let parsed = serde_json::from_str::<serde_json::Value>(&args)
            .expect("Failed to parse corrected arguments");
        assert!(parsed["todos"].is_array());
        let array = parsed["todos"]
            .as_array()
            .expect("todos should be an array");
        assert_eq!(array.len(), 3);
    }

    #[test]
    fn test_sanitize_tool_call_invalid_json() {
        let malformed_name = "todos [invalid json}";
        let (name, args) = AgentExecutor::sanitize_tool_call(malformed_name, "{}");

        // Should fall back to original when JSON is invalid
        assert_eq!(name, "todos [invalid json}");
        assert_eq!(args, "{}");
    }

    #[test]
    fn test_sanitize_tool_call_other_tools_unchanged() {
        let (name, args) =
            AgentExecutor::sanitize_tool_call("execute_command", r#"{"command": "ls"}"#);
        assert_eq!(name, "execute_command");
        assert_eq!(args, r#"{"command": "ls"}"#);
    }

    #[test]
    fn test_extract_first_json_simple() {
        let input = r#"{"key": "value"}"#;
        let result = AgentExecutor::extract_first_json(input);
        assert!(result.is_some());
        if let Some(json) = result {
            assert_eq!(json, r#"{"key": "value"}"#);
        }
    }

    #[test]
    fn test_extract_first_json_with_trailing_text() {
        let input = r#"{"key": "value"} some extra text"#;
        let result = AgentExecutor::extract_first_json(input);
        assert!(result.is_some());
        if let Some(json) = result {
            assert_eq!(json, r#"{"key": "value"}"#);
        }
    }

    #[test]
    fn test_extract_first_json_nested() {
        let input = r#"{"outer": {"inner": "value"}}"#;
        let result = AgentExecutor::extract_first_json(input);
        assert!(result.is_some());
        if let Some(json) = result {
            let parsed = serde_json::from_str::<serde_json::Value>(&json)
                .expect("Failed to parse extracted JSON");
            assert_eq!(parsed["outer"]["inner"], "value");
        }
    }

    #[test]
    fn test_extract_first_json_invalid() {
        let input = "not json at all";
        let result = AgentExecutor::extract_first_json(input);
        assert!(result.is_none());
    }

    // Tests for sanitize_xml_tags function
    #[test]
    fn test_sanitize_xml_tags_basic() {
        let input = "Some text <tool_call>content</tool_call> more text";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "Some text content more text");
    }

    #[test]
    fn test_sanitize_xml_tags_filepath() {
        let input = "read_file<filepath>/workspace/docker-compose.yml</filepath></tool_call>";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "read_file/workspace/docker-compose.yml");
    }

    #[test]
    fn test_sanitize_xml_tags_multiple() {
        let input = "<arg_key>test</arg_key><arg_value>value</arg_value><command>ls</command>";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "testvaluels");
    }

    #[test]
    fn test_sanitize_xml_tags_malformed_tool_call() {
        // Real-world example from bug report
        let input = "todos</arg_key><arg_value>[{\"description\": \"test\"}]";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "todos[{\"description\": \"test\"}]");
        assert!(!result.contains("</arg_key>"));
        assert!(!result.contains("<arg_value>"));
    }

    #[test]
    fn test_sanitize_xml_tags_preserves_content() {
        let input = "Normal text without tags";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_sanitize_xml_tags_preserves_valid_comparison() {
        // Should preserve mathematical comparisons
        let input = "Check if x < 5 and y > 3";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_sanitize_xml_tags_only_lowercase() {
        // Should only match lowercase XML tags
        let input = "Text <ToolCall>content</ToolCall> <COMMAND>ls</COMMAND>";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, input); // Uppercase tags are preserved
    }

    #[test]
    fn test_sanitize_xml_tags_with_underscores() {
        let input = "<tool_name>search</tool_name><arg_key_1>value</arg_key_1>";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "searchvalue");
    }

    #[test]
    fn test_sanitize_xml_tags_with_numbers() {
        let input = "<arg1>first</arg1><arg2>second</arg2>";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "firstsecond");
    }
}
