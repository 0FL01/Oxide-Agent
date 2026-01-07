//! Agent executor module
//!
//! Handles the iterative execution of tasks using LLM with tool calling.

use super::hooks::{CompletionCheckHook, HookContext, HookEvent, HookRegistry, HookResult};
use super::loop_detection::{LoopDetectionConfig, LoopDetectionService, LoopType};
use super::memory::AgentMessage;
use super::session::AgentSession;
use super::skills::SkillRegistry;
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
/// This function is public to allow reuse in integration tests, progress tracking,
/// todo descriptions, and other agent components that need protection from XML leaks.
pub fn sanitize_xml_tags(text: &str) -> String {
    use lazy_regex::regex;

    // Pattern to match opening and closing XML tags: <tag_name> or </tag_name>
    // Matches lowercase letters, digits, underscores in tag names
    let xml_tag_pattern = regex!(r"</?[a-z_][a-z0-9_]*>");

    xml_tag_pattern.replace_all(text, " ").trim().to_string()
}

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

    /// Sanitize tool call by detecting malformed LLM responses where JSON arguments are placed in tool name
    /// Returns (`corrected_name`, `corrected_arguments`)
    fn sanitize_tool_call(name: &str, arguments: &str) -> (String, String) {
        let xml_sanitized_name = sanitize_xml_tags(name);
        let trimmed_name = xml_sanitized_name.trim();

        // PATTERN 1: Check if name looks like it contains JSON object (starts with { and has "todos" key)
        // Example: `{"todos": [{"description": "...", "status": "..."}]}`
        if trimmed_name.starts_with('{') && trimmed_name.contains("\"todos\"") {
            warn!(
                tool_name = %name,
                sanitized_name = %xml_sanitized_name,
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
                        sanitized_name = %xml_sanitized_name,
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
        if xml_sanitized_name != name {
            let normalized_name = Self::normalize_tool_name(trimmed_name, name);
            return (normalized_name, arguments.to_string());
        }

        (name.to_string(), arguments.to_string())
    }

    fn normalize_tool_name(sanitized_name: &str, original_name: &str) -> String {
        let mut tokens = sanitized_name.split_whitespace();
        let Some(first) = tokens.next() else {
            warn!(
                tool_name = %original_name,
                sanitized_name = %sanitized_name,
                "Sanitized tool name is empty"
            );
            return String::new();
        };

        if tokens.next().is_some() {
            warn!(
                tool_name = %original_name,
                sanitized_name = %sanitized_name,
                normalized_name = %first,
                "Sanitized tool name contained extra tokens; using first token"
            );
        }

        first.to_string()
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
    ///
    /// BUGFIX AGENT-2026-001: Extended to support ytdlp tools
    fn try_parse_malformed_tool_call(content: &str) -> Option<ToolCall> {
        const TOOL_NAMES: [&str; 11] = [
            "read_file",
            "write_file",
            "execute_command",
            "list_files",
            "send_file_to_user",
            "upload_file",
            // BUGFIX AGENT-2026-001: Add ytdlp tools to malformed call recovery
            "ytdlp_get_video_metadata",
            "ytdlp_download_transcript",
            "ytdlp_search_videos",
            "ytdlp_download_video",
            "ytdlp_download_audio",
        ];

        for tool_name in TOOL_NAMES {
            if !content.contains(tool_name) {
                continue;
            }

            let Some(arguments) = Self::extract_malformed_tool_arguments(tool_name, content) else {
                continue;
            };

            return Self::build_recovered_tool_call(tool_name, arguments);
        }

        None
    }

    fn extract_malformed_tool_arguments(tool_name: &str, content: &str) -> Option<Value> {
        match tool_name {
            "read_file" => Self::extract_read_file_arguments(content),
            "write_file" => Self::extract_write_file_arguments(content),
            "execute_command" => Self::extract_execute_command_arguments(content),
            "list_files" => Self::extract_list_files_arguments(content),
            "send_file_to_user" => Self::extract_send_file_to_user_arguments(content),
            "upload_file" => Self::extract_upload_file_arguments(content),
            "ytdlp_get_video_metadata" => Self::extract_ytdlp_url_arguments(content, tool_name),
            "ytdlp_download_transcript" => Self::extract_ytdlp_url_arguments(content, tool_name),
            "ytdlp_search_videos" => Self::extract_ytdlp_search_arguments(content),
            "ytdlp_download_video" => Self::extract_ytdlp_url_arguments(content, tool_name),
            "ytdlp_download_audio" => Self::extract_ytdlp_url_arguments(content, tool_name),
            _ => None,
        }
    }

    fn build_recovered_tool_call(tool_name: &str, arguments: Value) -> Option<ToolCall> {
        use uuid::Uuid;

        let arguments_str = serde_json::to_string(&arguments).ok()?;

        warn!(
            tool_name = tool_name,
            arguments = %arguments_str,
            "Recovered malformed tool call from content"
        );

        Some(ToolCall {
            id: format!("recovered_{}", Uuid::new_v4()),
            function: ToolCallFunction {
                name: tool_name.to_string(),
                arguments: arguments_str,
            },
        })
    }

    fn extract_tag_value<'a>(content: &'a str, tag: &str) -> Option<&'a str> {
        let open = format!("<{tag}>");
        let start = content.find(&open)? + open.len();
        let after_open = &content[start..];
        let end = after_open.find("</").unwrap_or(after_open.len());
        let value = after_open[..end].trim();
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    }

    fn extract_token_after_tool_name<'a>(
        content: &'a str,
        tool_name: &str,
        optional_prefix: Option<&str>,
    ) -> Option<&'a str> {
        let idx = content.find(tool_name)?;
        let mut after = &content[idx + tool_name.len()..];
        after = after.trim_start();
        if let Some(prefix) = optional_prefix {
            after = after.strip_prefix(prefix).unwrap_or(after).trim_start();
        }

        let end = after
            .char_indices()
            .find(|(_, ch)| ch.is_whitespace() || *ch == '<')
            .map_or(after.len(), |(i, _)| i);
        let token = after[..end].trim();
        if token.is_empty() {
            None
        } else {
            Some(token)
        }
    }

    fn extract_read_file_arguments(content: &str) -> Option<Value> {
        if let Some(path) = Self::extract_tag_value(content, "filepath") {
            return Some(serde_json::json!({ "path": path }));
        }

        Self::extract_token_after_tool_name(content, "read_file", Some("path"))
            .map(|path| serde_json::json!({ "path": path }))
    }

    fn extract_write_file_arguments(content: &str) -> Option<Value> {
        let path = Self::extract_tag_value(content, "filepath")?;
        let file_content = Self::extract_tag_value(content, "content").unwrap_or("");
        Some(serde_json::json!({ "path": path, "content": file_content }))
    }

    fn extract_execute_command_arguments(content: &str) -> Option<Value> {
        if let Some(command) = Self::extract_tag_value(content, "command") {
            return Some(serde_json::json!({ "command": command }));
        }

        Self::extract_token_after_tool_name(content, "execute_command", Some("command"))
            .map(|command| serde_json::json!({ "command": command }))
    }

    fn extract_list_files_arguments(content: &str) -> Option<Value> {
        let path = Self::extract_tag_value(content, "directory").unwrap_or("");
        Some(serde_json::json!({ "path": path }))
    }

    fn extract_send_file_to_user_arguments(content: &str) -> Option<Value> {
        if let Some(path) = Self::extract_tag_value(content, "filepath")
            .or_else(|| Self::extract_tag_value(content, "path"))
        {
            return Some(serde_json::json!({ "path": path }));
        }

        None
    }

    fn extract_upload_file_arguments(content: &str) -> Option<Value> {
        if let Some(path) = Self::extract_tag_value(content, "filepath")
            .or_else(|| Self::extract_tag_value(content, "path"))
        {
            return Some(serde_json::json!({ "path": path }));
        }

        None
    }

    fn extract_ytdlp_url_arguments(content: &str, tool_name: &str) -> Option<Value> {
        if let Some(url) = Self::extract_tag_value(content, "url") {
            return Some(serde_json::json!({ "url": url }));
        }

        Self::extract_token_after_tool_name(content, tool_name, Some("url"))
            .map(|url| serde_json::json!({ "url": url }))
    }

    fn extract_ytdlp_search_arguments(content: &str) -> Option<Value> {
        if let Some(query) = Self::extract_tag_value(content, "query") {
            return Some(serde_json::json!({ "query": query }));
        }

        Self::extract_token_after_tool_name(content, "ytdlp_search_videos", Some("query"))
            .map(|query| serde_json::json!({ "query": query }))
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
        let system_prompt = self.create_agent_system_prompt(task).await;
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

            let tool_calls = Self::sanitize_tool_calls(response.tool_calls);

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

            self.execute_tool_calls(tool_calls, ctx).await?;
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

    async fn loop_detected_error(
        &self,
        loop_type: LoopType,
        iteration: usize,
        ctx: &AgentLoopContext<'_>,
    ) -> anyhow::Error {
        use super::progress::AgentEvent;

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

    /// Check if text looks like a malformed tool call attempt
    ///
    /// This detects patterns that indicate the LLM tried to call a tool but failed to use
    /// proper JSON format. Examples:
    /// - "[Вызов инструментов: ytdlp_get_video_metadataurl...]"
    /// - "[Tool calls: read_file]read_filepath..."
    /// - "ytdlp_download_videourl..."
    fn looks_like_tool_call_text(text: &str) -> bool {
        // Pattern 1: Explicit tool call markers in Russian or English
        if text.contains("[Tool call") || text.contains("Tool calls:") {
            return true;
        }

        // Check for Russian markers
        if text.contains("Вызов инструмент") {
            return true;
        }

        // Pattern 2: Known tool names (simple contains check for malformed cases)
        let tool_names = [
            "ytdlp_get_video_metadata",
            "ytdlp_download_transcript",
            "ytdlp_search_videos",
            "ytdlp_download_video",
            "ytdlp_download_audio",
            "write_file",
            "read_file",
            "execute_command",
            "web_search",
            "web_extract",
            "list_files",
            "send_file_to_user",
            "upload_file",
            "write_todos",
        ];

        for tool_name in &tool_names {
            if text.contains(tool_name) {
                return true;
            }
        }

        false
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
            if Self::looks_like_tool_call_text(&final_response) {
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

    fn sanitize_tool_calls(tool_calls: Vec<ToolCall>) -> Vec<ToolCall> {
        tool_calls
            .into_iter()
            .map(|call| {
                let (name, arguments) =
                    Self::sanitize_tool_call(&call.function.name, &call.function.arguments);
                ToolCall {
                    id: call.id,
                    function: ToolCallFunction { name, arguments },
                }
            })
            .collect()
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

        self.load_skill_context_for_tool(&name, ctx).await?;

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

    /// Create the system prompt for the agent
    async fn create_agent_system_prompt(&mut self, task: &str) -> String {
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

        if let Some(registry) = self.skill_registry.as_mut() {
            match registry.build_prompt(task).await {
                Ok(skill_prompt) if !skill_prompt.content.is_empty() => {
                    self.session.set_loaded_skills(&skill_prompt.skills);
                    info!(
                        skills = ?skill_prompt.skills,
                        total_tokens = skill_prompt.token_count,
                        skipped = ?skill_prompt.skipped,
                        "Skills loaded for request"
                    );
                    return format!("{date_context}{}", skill_prompt.content);
                }
                Ok(_) => {
                    warn!("Skills prompt empty, falling back to AGENT.md");
                }
                Err(err) => {
                    warn!(error = %err, "Failed to build skills prompt, falling back to AGENT.md");
                }
            }
        }

        let empty_skills: [crate::agent::skills::SkillContext; 0] = [];
        self.session.set_loaded_skills(&empty_skills);

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
    fn test_sanitize_tool_call_strips_xml_from_name() {
        let (name, args) =
            AgentExecutor::sanitize_tool_call("command</arg_key><arg_value>cd", "{}");
        assert_eq!(name, "command");
        assert_eq!(args, "{}");
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
        assert_eq!(result, "Some text  content  more text");
    }

    #[test]
    fn test_sanitize_xml_tags_filepath() {
        let input = "read_file<filepath>/workspace/docker-compose.yml</filepath></tool_call>";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "read_file /workspace/docker-compose.yml");
    }

    #[test]
    fn test_sanitize_xml_tags_multiple() {
        let input = "<arg_key>test</arg_key><arg_value>value</arg_value><command>ls</command>";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "test  value  ls");
    }

    #[test]
    fn test_sanitize_xml_tags_malformed_tool_call() {
        // Real-world example from bug report
        let input = "todos</arg_key><arg_value>[{\"description\": \"test\"}]";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "todos  [{\"description\": \"test\"}]");
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
        assert_eq!(result, "search  value");
    }

    #[test]
    fn test_sanitize_xml_tags_with_numbers() {
        let input = "<arg1>first</arg1><arg2>second</arg2>";
        let result = sanitize_xml_tags(input);
        assert_eq!(result, "first  second");
    }

    // Tests for BUGFIX AGENT-2026-001: looks_like_tool_call_text
    #[test]
    fn test_looks_like_tool_call_text_with_russian_marker() {
        let input = "[Вызов инструментов: ytdlp_get_video_metadataurl...]";
        assert!(AgentExecutor::looks_like_tool_call_text(input));
    }

    #[test]
    fn test_looks_like_tool_call_text_with_english_marker() {
        let input = "[Tool calls: read_file]read_filepath...";
        assert!(AgentExecutor::looks_like_tool_call_text(input));
    }

    #[test]
    fn test_looks_like_tool_call_text_with_ytdlp_tool_name() {
        let input = "ytdlp_get_video_metadataurl...";
        assert!(AgentExecutor::looks_like_tool_call_text(input));
    }

    #[test]
    fn test_looks_like_tool_call_text_with_other_tool_names() {
        assert!(AgentExecutor::looks_like_tool_call_text(
            "execute_command ls"
        ));
        assert!(AgentExecutor::looks_like_tool_call_text(
            "read_file /path/to/file"
        ));
        assert!(AgentExecutor::looks_like_tool_call_text(
            "write_todos [...]"
        ));
    }

    #[test]
    fn test_looks_like_tool_call_text_normal_text() {
        let input = "This is a normal response with some information about the task.";
        assert!(!AgentExecutor::looks_like_tool_call_text(input));
    }

    #[test]
    fn test_looks_like_tool_call_text_normal_russian_text() {
        let input = "Вот результат выполнения задачи без вызова инструментов.";
        assert!(!AgentExecutor::looks_like_tool_call_text(input));
    }

    // Tests for BUGFIX AGENT-2026-001: try_parse_malformed_tool_call with ytdlp
    #[test]
    fn test_try_parse_malformed_ytdlp_get_video_metadata() {
        let input = "ytdlp_get_video_metadata<url>https://youtube.com/watch?v=xxx</url>";
        let result = AgentExecutor::try_parse_malformed_tool_call(input);

        assert!(result.is_some());
        let tool_call = result.expect("tool_call should be Some");
        assert_eq!(tool_call.function.name, "ytdlp_get_video_metadata");

        let args: serde_json::Value = serde_json::from_str(&tool_call.function.arguments)
            .expect("arguments should be valid JSON");
        assert_eq!(args["url"], "https://youtube.com/watch?v=xxx");
    }

    #[test]
    fn test_try_parse_malformed_ytdlp_without_tags() {
        let input = "ytdlp_get_video_metadataurl https://youtube.com/watch?v=xxx";
        let result = AgentExecutor::try_parse_malformed_tool_call(input);

        assert!(result.is_some());
        let tool_call = result.expect("tool_call should be Some");
        assert_eq!(tool_call.function.name, "ytdlp_get_video_metadata");
    }

    #[test]
    fn test_try_parse_malformed_ytdlp_download_transcript() {
        let input = "ytdlp_download_transcripturlhttps://youtube.com/watch?v=yyy";
        let result = AgentExecutor::try_parse_malformed_tool_call(input);

        assert!(result.is_some());
        let tool_call = result.expect("tool_call should be Some");
        assert_eq!(tool_call.function.name, "ytdlp_download_transcript");
    }
}
