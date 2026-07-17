//! Prompt composer module
//!
//! Handles construction of system prompts for the agent, including
//! date context and fallback prompts.

use crate::agent::session::AgentSession;
use crate::agent::tool_runtime::CapabilityGroup;
use crate::llm::ToolDefinition;
use std::collections::BTreeSet;

/// Composed system prompt split into a cacheable base and a volatile date suffix.
///
/// The base prompt contains static blocks (fallback, instructions, workflow,
/// structured output) that are byte-for-byte identical across turns. The date suffix
/// contains the current timestamp and changes every request.
///
/// Downstream, the fold pipeline assembles the final prompt as:
/// `base + stable_system_messages + date_suffix + volatile_system_messages`
#[derive(Debug, Clone)]
pub struct ComposedPrompt {
    /// Cacheable system prompt without date/time context.
    pub base: String,
    /// Volatile date/time block appended after stable content.
    pub date_suffix: String,
}

impl ComposedPrompt {
    /// Reconstruct the full system prompt as a single string.
    ///
    /// Equivalent to the pre-split format: `base + "\n\n" + date_suffix`.
    /// Useful for backward-compatible assertions in tests and internal text calls
    /// that don't go through the fold pipeline.
    #[must_use]
    pub fn full_prompt(&self) -> String {
        let date_trimmed = self.date_suffix.trim();
        if date_trimmed.is_empty() {
            self.base.clone()
        } else {
            format!("{}\n\n{}", self.base.trim(), date_trimmed)
        }
    }
}

impl std::fmt::Display for ComposedPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.full_prompt())
    }
}

// ---------------------------------------------------------------------------
// PromptToolContext
// ---------------------------------------------------------------------------

/// Tool context for prompt composition.
///
/// Encapsulates what the prompt composer needs from the tool catalog:
/// - **Catalog specs** — all tool definitions in the catalog, used for
///   workflow hints and date context.  These reflect the full set of
///   compiled tools, not just the currently visible surface.
/// - **Available groups** — capability groups present in the catalog, used
///   for the "Available Tool Groups" block that tells the model what it can
///   retrieve via `retrieve_tools`.
///
/// In the lazy-tool production path, both fields are derived from the
/// `ToolCatalog`.  In tests and non-lazy paths (sub-agents before Phase H),
/// `PromptToolContext::from_tools` provides a context with no retrievable
/// groups — workflow hints still work, but no category list block is emitted.
pub struct PromptToolContext<'a> {
    catalog_specs: &'a [ToolDefinition],
    available_groups: &'a [CapabilityGroup],
}

impl<'a> PromptToolContext<'a> {
    /// Create a tool context from catalog specs and activatable groups.
    #[must_use]
    pub const fn new(
        catalog_specs: &'a [ToolDefinition],
        available_groups: &'a [CapabilityGroup],
    ) -> Self {
        Self {
            catalog_specs,
            available_groups,
        }
    }

    /// Construct from a tool list with no retrievable groups.
    ///
    /// Use this for tests and non-lazy paths where all tools are already
    /// visible and `retrieve_tools` is not applicable.
    #[must_use]
    pub const fn from_tools(catalog_specs: &'a [ToolDefinition]) -> Self {
        Self {
            catalog_specs,
            available_groups: &[],
        }
    }

    /// Full catalog tool definitions.
    #[must_use]
    pub const fn catalog_specs(&self) -> &'a [ToolDefinition] {
        self.catalog_specs
    }

    /// Activatable capability groups in the catalog.
    #[must_use]
    pub const fn available_groups(&self) -> &'a [CapabilityGroup] {
        self.available_groups
    }

    /// Whether any retrievable groups exist.
    #[must_use]
    pub fn has_retrievable_groups(&self) -> bool {
        !self.available_groups.is_empty()
    }
}

/// Build the date context block for the system prompt
fn build_date_context(tools: &[ToolDefinition]) -> String {
    let now = chrono::Local::now();
    let current_date = now.format("%Y-%m-%d %H:%M:%S").to_string();
    let current_day = now.format("%A").to_string();
    let current_offset = now.format("UTC%:z").to_string();
    let tool_names = tool_name_set(tools);
    let search_tools = available_tool_names(&tool_names, &["web_search"]);

    let mut context = format!(
        "### CURRENT DATE AND TIME\nToday: {current_date}, {current_day}\nCurrent local timezone: {current_offset}"
    );

    if search_tools.is_empty() {
        context.push_str("\nIMPORTANT: Always use this date as the current date.");
    } else {
        context.push_str(&format!(
            "\nIMPORTANT: Always use this date as the current date. If search results ({}) contain phrases like 'today', 'tomorrow', or dates contradicting this, consider the search results outdated and interpret them relative to the date above.",
            format_tool_list(&search_tools)
        ));
    }

    context.push_str("\n\n");
    context
}

fn tool_name_set(tools: &[ToolDefinition]) -> BTreeSet<&str> {
    tools.iter().map(|tool| tool.name.as_str()).collect()
}

fn has_tool(tool_names: &BTreeSet<&str>, name: &str) -> bool {
    tool_names.contains(name)
}

fn has_any_tool(tool_names: &BTreeSet<&str>, names: &[&str]) -> bool {
    names.iter().any(|name| has_tool(tool_names, name))
}

fn available_tool_names<'a>(tool_names: &BTreeSet<&str>, names: &'a [&str]) -> Vec<&'a str> {
    names
        .iter()
        .copied()
        .filter(|name| has_tool(tool_names, name))
        .collect()
}

fn format_tool_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => format!("`{one}`"),
        [first, second] => format!("`{first}` or `{second}`"),
        [rest @ .., last] => {
            let prefix = rest
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{prefix}, or `{last}`")
        }
    }
}

#[derive(Default)]
struct WorkflowGuidanceBuilder {
    sections: Vec<String>,
    section_ids: BTreeSet<&'static str>,
}

impl WorkflowGuidanceBuilder {
    fn push_section(&mut self, id: &'static str, heading: &'static str, lines: Vec<String>) {
        if lines.is_empty() || !self.section_ids.insert(id) {
            return;
        }

        let mut section = format!("### {heading}");
        for line in lines {
            section.push_str("\n- ");
            section.push_str(&line);
        }
        self.sections.push(section);
    }

    fn finish(self) -> Option<String> {
        if self.sections.is_empty() {
            None
        } else {
            Some(format!(
                "## Workflow Hints\n\n{}",
                self.sections.join("\n\n")
            ))
        }
    }
}

fn build_workflow_guidance(tools: &[ToolDefinition]) -> Option<String> {
    let tool_names = tool_name_set(tools);
    let mut builder = WorkflowGuidanceBuilder::default();

    if has_tool(&tool_names, "write_todos") {
        builder.push_section(
            "task_tracking",
            "Task Tracking",
            vec![
                "For complex or multi-step work, call `write_todos` before starting.".to_string(),
                "Keep exactly one task `in_progress`; keep the rest `pending`, `completed`, `cancelled`, or `blocked_on_user`.".to_string(),
                "Update tasks as work changes; mark `completed` only after the step is actually done and verified when applicable.".to_string(),
                "When the final `write_todos` update completes all work, return the complete final answer in the next assistant message; do not return only a summary or addendum.".to_string(),
                "If blocked waiting for the user, mark the relevant task `blocked_on_user` before asking for input.".to_string(),
            ],
        );
    }

    if has_any_tool(
        &tool_names,
        &["list_files", "read_file", "apply_file_edit", "write_file"],
    ) {
        let mut lines = Vec::new();
        if has_tool(&tool_names, "list_files") {
            lines.push(
                "Use `list_files` to discover paths when the exact workspace path is unclear."
                    .to_string(),
            );
        }
        if has_tool(&tool_names, "read_file") && has_tool(&tool_names, "apply_file_edit") {
            lines.push("Use `read_file` before `apply_file_edit` on non-empty files.".to_string());
        }
        if has_tool(&tool_names, "apply_file_edit") {
            lines.push("Prefer `apply_file_edit` for targeted exact replacements.".to_string());
        }
        if has_tool(&tool_names, "write_file") {
            lines.push("Use `write_file` for new files or full rewrites; prefer editing existing files when possible.".to_string());
        }
        builder.push_section("sandbox_file", "Sandbox File Workflow", lines);
    }

    if has_tool(&tool_names, "execute_command") {
        let mut lines = vec![
            "Use `execute_command` for builds, tests, diagnostics, and shell-based transformations inside the sandbox.".to_string(),
            "Verify code or file changes with relevant commands when practical.".to_string(),
            "To study external resources, download them into the sandbox first: use `git clone` for repositories, `curl` or `wget` for files and archives. Then unpack archives (unzip, tar -xf) and explore cloned repos with `list_files` and `read_file` to understand their structure and contents.".to_string(),
        ];
        if has_any_tool(&tool_names, &["read_file", "write_file", "apply_file_edit"]) {
            lines.push("Prefer dedicated file tools over shell `cat`, redirection, or ad-hoc text replacement for file operations.".to_string());
        }
        builder.push_section("sandbox_command", "Sandbox Command Workflow", lines);
    }

    if has_tool(&tool_names, "recreate_sandbox") {
        builder.push_section(
            "sandbox_lifecycle",
            "Sandbox Lifecycle",
            vec![
                "Use `recreate_sandbox` only when the user asks for a clean workspace or the current sandbox is irrecoverably broken.".to_string(),
                "Remember that `recreate_sandbox` wipes previous workspace contents.".to_string(),
            ],
        );
    }

    if has_any_tool(&tool_names, &["web_search", "web_crawler", "web_markdown"]) {
        let mut lines = Vec::new();
        if has_tool(&tool_names, "web_search") {
            lines.push("Use `web_search` for current web search, news, facts, documentation, or real-time data you cannot know locally.".to_string());
        }
        if has_tool(&tool_names, "web_crawler") {
            lines.push(
                "Use `web_crawler` after search to read selected result URLs as Markdown; its default HTTP mode falls back once to Lightpanda for anti-bot, HTTP 403, or HTTP 429 failures. Select `lightpanda` or `playwright` directly for known JavaScript-heavy pages."
                    .to_string(),
            );
        } else if has_tool(&tool_names, "web_markdown") {
            lines.push(
                "Use `web_markdown` after search when you need to read a specific result URL as Markdown."
                    .to_string(),
            );
        }
        if has_tool(&tool_names, "web_search") {
            lines.push(
                "Do not fetch every search result automatically; fetch only selected URLs."
                    .to_string(),
            );
        }
        lines.push("Do not claim current external facts from memory when the answer depends on freshness and a web tool is available.".to_string());
        builder.push_section("web_research", "Web Research", lines);
    }

    if has_any_tool(
        &tool_names,
        &["spawn_sub_agents", "wait_sub_agents", "cancel_sub_agents"],
    ) {
        let mut lines = Vec::new();
        if has_tool(&tool_names, "spawn_sub_agents") {
            lines.push("Use `spawn_sub_agents` for independent research branches, not for sequential edits or shared mutable state.".to_string());
        }
        if has_tool(&tool_names, "wait_sub_agents") {
            lines.push("Use `wait_sub_agents` before relying on delegated results.".to_string());
        }
        if has_tool(&tool_names, "cancel_sub_agents") {
            lines.push(
                "Use `cancel_sub_agents` to stop irrelevant or obsolete delegated work."
                    .to_string(),
            );
        }
        builder.push_section("delegation", "Delegation", lines);
    }

    if has_any_tool(&tool_names, &["agents_md_get", "agents_md_update"]) {
        let mut lines = Vec::new();
        if has_tool(&tool_names, "agents_md_get") {
            lines.push("Use `agents_md_get` to inspect the active topic AGENTS.md when maintaining topic instructions.".to_string());
        }
        if has_tool(&tool_names, "agents_md_update") {
            lines.push("Use `agents_md_update` only for explicit topic instruction changes or when the user asks to update agent guidance.".to_string());
        }
        builder.push_section("topic_agents_md", "Topic AGENTS.md", lines);
    }

    if has_tool(&tool_names, "reminder_schedule") {
        builder.push_section(
            "reminder_scheduling",
            "Reminder Scheduling",
            vec![
                "The current date/time block above is the source of truth for local time.".to_string(),
                "Do not compute unix timestamps by hand for reminders.".to_string(),
                "For a one-time reminder, use `kind=once` with `date` + `time` and optional `timezone`.".to_string(),
                "For repeat-every-N-minutes or repeat-every-N-hours, use `kind=interval` with `every_minutes` or `every_hours`.".to_string(),
                "For wall-clock schedules like every day at 09:00 or weekdays at 18:30, use `kind=cron` with `time`, optional `weekdays`, and optional `timezone`.".to_string(),
                "Do not use `kind=interval` for calendar schedules like every day at 09:00; use `kind=cron` to preserve local wall-clock time across calendar/DST changes.".to_string(),
                "When `timezone` is omitted, reminder scheduling uses the current local timezone shown above.".to_string(),
            ],
        );
    }

    let media_tools = available_tool_names(
        &tool_names,
        &[
            "transcribe_audio_file",
            "describe_image_file",
            "describe_video_file",
        ],
    );
    let media_url_tools =
        available_tool_names(&tool_names, &["describe_image_file", "describe_video_file"]);
    let tts_file_tools = available_tool_names(
        &tool_names,
        &["text_to_speech_en_file", "text_to_speech_ru_file"],
    );

    if !media_tools.is_empty() || !tts_file_tools.is_empty() {
        let mut lines = vec![
            "Uploaded files provided for file workflows are preserved in the sandbox and remain directly manipulable.".to_string(),
            "When the user wants editing, transcoding, muxing, translation dubbing, or other file transformations, operate on the sandbox file instead of summarizing it.".to_string(),
        ];
        if !media_tools.is_empty() {
            lines.push(format!(
                "Use {} only when you need multimodal understanding before acting on a file.",
                format_tool_list(&media_tools)
            ));
        }
        if !media_url_tools.is_empty() {
            lines.push(format!(
                "{} can accept sandbox paths or direct `http(s)` URLs; remote media is downloaded into the sandbox automatically and cleaned up after successful analysis.",
                format_tool_list(&media_url_tools)
            ));
        }
        if !tts_file_tools.is_empty() {
            lines.push(format!(
                "Use {} when another tool such as `ffmpeg` needs an audio file path instead of an immediate voice message.",
                format_tool_list(&tts_file_tools)
            ));
        }
        builder.push_section("file_workflows", "File Workflows", lines);
    }

    if has_any_tool(&tool_names, &["send_file_to_user", "upload_file"]) {
        let mut lines = Vec::new();
        if has_tool(&tool_names, "send_file_to_user") {
            lines.push("Use `send_file_to_user` to return finished sandbox files through the chat transport.".to_string());
            lines.push("If `send_file_to_user` returns `download_url`, include that exact URL in `final_answer` as a markdown link so the user can download the file directly from the main chat response.".to_string());
        }
        if has_tool(&tool_names, "upload_file") {
            lines.push("Use `upload_file` for files too large for chat delivery or when an external file link is needed.".to_string());
        }
        builder.push_section("file_delivery", "File Delivery", lines);
    }

    if has_tool(&tool_names, "compress") {
        builder.push_section(
            "context_management",
            "Context Management",
            vec!["Use `compress` when the current task must continue and hot context is becoming too large.".to_string()],
        );
    }

    if has_any_tool(
        &tool_names,
        &[
            "browser_start",
            "browser_observe",
            "browser_execute",
            "browser_extract",
            "browser_debug",
            "browser_close",
        ],
    ) {
        let mut lines = Vec::new();
        if has_tool(&tool_names, "browser_start") {
            lines.push("Use `browser_start` to open a new browser session for a task.".to_string());
        }
        if has_tool(&tool_names, "browser_observe") {
            lines.push("Use `browser_observe` to capture the current page state and a screenshot; the screenshot is attached as a native image to the tool result.".to_string());
        }
        if has_tool(&tool_names, "browser_execute") {
            lines.push("Use `browser_execute` to perform one concrete browser action at a time (click, fill, navigate, script, etc.) based on the screenshot.".to_string());
            lines.push("If a JavaScript or wait action returns a result, use that value before relying on the screenshot.".to_string());
        }
        if has_tool(&tool_names, "browser_extract") {
            lines.push("Use `browser_extract` to pull structured data: network response bodies, single DOM values, or DOM table rows via `selector` + `fields` instead of custom JavaScript.".to_string());
        }
        if has_tool(&tool_names, "browser_debug") {
            lines.push("Use `browser_debug` for network or console summaries when observation summaries are insufficient.".to_string());
        }
        if has_tool(&tool_names, "browser_close") {
            lines.push(
                "Use `browser_close` when the browser session is no longer needed.".to_string(),
            );
        }
        builder.push_section("browser_direct_control", "Browser Direct Control", lines);
    }

    if has_any_tool(
        &tool_names,
        &[
            "ssh_exec",
            "ssh_sudo_exec",
            "ssh_read_file",
            "ssh_apply_file_edit",
            "ssh_check_process",
            "ssh_send_file_to_user",
        ],
    ) {
        let mut lines = Vec::new();
        if has_tool(&tool_names, "ssh_exec") {
            lines.push("Use `ssh_exec` for remote diagnostics and non-privileged commands on configured topic infrastructure.".to_string());
        }
        if has_tool(&tool_names, "ssh_sudo_exec") {
            lines.push("Use `ssh_sudo_exec` only when privileged remote access is necessary for the requested task.".to_string());
        }
        if has_tool(&tool_names, "ssh_read_file") && has_tool(&tool_names, "ssh_apply_file_edit") {
            lines.push(
                "Use `ssh_read_file` before `ssh_apply_file_edit` on non-empty remote files."
                    .to_string(),
            );
        }
        if has_tool(&tool_names, "ssh_check_process") {
            lines.push(
                "Use `ssh_check_process` to verify remote long-running processes without guessing."
                    .to_string(),
            );
        }
        if has_tool(&tool_names, "ssh_send_file_to_user") {
            lines.push(
                "Use `ssh_send_file_to_user` to return remote files through the chat transport."
                    .to_string(),
            );
            lines.push("If `ssh_send_file_to_user` returns `download_url`, include that exact URL in `final_answer` as a markdown link so the user can download the file directly from the main chat response.".to_string());
        }
        builder.push_section("ssh_workflow", "SSH Workflow", lines);
    }

    if has_any_tool(
        &tool_names,
        &["stack_logs_list_sources", "stack_logs_fetch"],
    ) {
        builder.push_section(
            "stack_logs",
            "Stack Logs",
            vec![
                "Use `stack_logs_list_sources` before `stack_logs_fetch` when the log source name is unclear.".to_string(),
                "Use `stack_logs_fetch` for compose-stack diagnostics instead of guessing from memory.".to_string(),
            ],
        );
    }

    builder.finish()
}

/// Build the "Available Tool Groups" block for the lazy tool protocol.
///
/// Tells the model which capability groups it can activate via
/// `retrieve_tools`.  This block is static for a run (the catalog does not
/// change mid-run) and lives in the cacheable prompt prefix.
///
/// Returns `None` when no retrievable groups exist (test path or all tools
/// are always-visible).
fn build_category_list_block(groups: &[CapabilityGroup]) -> Option<String> {
    if groups.is_empty() {
        return None;
    }

    let group_lines: Vec<String> = groups
        .iter()
        .map(|&g| format!("- `{}` — {}", g.as_str(), capability_group_description(g)))
        .collect();

    Some(format!(
        "## Available Tool Groups\n\
        You can activate additional tools by calling `retrieve_tools` with one or more of the \
        following capability names:\n\
        {groups}\n\
        Call `retrieve_tools` early when you know which capabilities you need. \
        Activated tools appear in the tools list for subsequent turns.",
        groups = group_lines.join("\n"),
    ))
}

/// Human-readable description for a capability group.
const fn capability_group_description(group: CapabilityGroup) -> &'static str {
    match group {
        CapabilityGroup::Files => "file operations (read, write, edit, list)",
        CapabilityGroup::Shell => "command execution and sandbox lifecycle",
        CapabilityGroup::Web => "web search and page fetch",
        CapabilityGroup::Browser => "autonomous browser control",
        CapabilityGroup::Media => "media analysis (audio, image, video)",
        CapabilityGroup::Ytdlp => "YouTube metadata, transcript, download",
        CapabilityGroup::Tts => "text-to-speech",
        CapabilityGroup::Delegation => "sub-agent delegation",
        CapabilityGroup::AgentsMd => "AGENTS.md self-editing",
        CapabilityGroup::Manager => "manager control-plane (topics, infra, profiles)",
        CapabilityGroup::Ssh => "SSH remote execution and file editing",
        CapabilityGroup::StackLogs => "Docker Compose stack logs",
        CapabilityGroup::Reminders => "reminder scheduling",
        CapabilityGroup::Jira => "Jira MCP integration",
        CapabilityGroup::Mattermost => "Mattermost MCP integration",
    }
}

/// Get the built-in fallback prompt for the main agent.
#[must_use]
pub fn get_fallback_prompt() -> String {
    r"You are an AI agent operating inside Oxide Agent.
## Core Rules:
- Follow the active topic AGENTS.md instructions when they are present in memory
- Use tools whenever you need real data, file contents, system state, or external information
- After each tool result, analyze it and continue until the task is complete
- Keep answers concise, accurate, and directly useful to the user
- Prefer verifying your changes with relevant tests or checks when possible"
        .to_string()
}

/// Build instructions for mandatory structured output (JSON).
///
/// Tool schemas are NOT included here — the model receives them via the
/// native `tools[]` API payload.  The structured output instructions contain
/// only the JSON schema, rules, and examples.
///
/// When `has_retrievable_groups` is true, an additional rule tells the model
/// to call `retrieve_tools` to activate tools that are not yet in the
/// tools list.
#[must_use]
pub fn build_structured_output_instructions(
    catalog_tools: &[ToolDefinition],
    has_retrievable_groups: bool,
) -> String {
    let tool_names = tool_name_set(catalog_tools);
    let todo_blocked_rule = if has_tool(&tool_names, "write_todos") {
        "\n- If you maintain a todo list and the remaining work is blocked on the user, mark the relevant todo as `blocked_on_user` before returning `awaiting_user_input`"
    } else {
        ""
    };
    let retrieve_rule = if has_retrievable_groups {
        "\n- Only call tools that are currently in the tools list; if you need a tool that is not listed, call `retrieve_tools` to activate it first"
    } else {
        ""
    };

    format!(
        r#"## STRUCTURED OUTPUT (MANDATORY)
You MUST respond ONLY with a valid JSON object strictly following the schema:
{{
  "thought": "Brief description of the solution and step",
  "tool_call": {{
    "name": "tool_name",
    "arguments": {{}}
  }},
  "final_answer": "Final answer to the user",
  "awaiting_user_input": {{
    "kind": "text|url|file|url_or_file",
    "prompt": "Question or request for the user"
  }}
}}

Rules:
- EXACTLY one of `tool_call`, `final_answer`, or `awaiting_user_input` must be filled (the others = null)
- If a tool is needed: `tool_call` = object, `final_answer` = null, `awaiting_user_input` = null
- If answer is ready: `tool_call` = null, `final_answer` = string, `awaiting_user_input` = null
- If the task is blocked on the user: `tool_call` = null, `final_answer` = null, `awaiting_user_input` = object
- Use `awaiting_user_input` when you need the user to provide missing text, a link, a file, or either a link/file before the task can continue{todo_blocked_rule}
- `awaiting_user_input.kind` must be exactly one of: `text`, `url`, `file`, `url_or_file`
- `awaiting_user_input.prompt` must be a short, direct request telling the user what to send next
- `tool_call.arguments` is always a JSON object
- No extra keys, markdown, XML, explanations, or text outside JSON
- Tool results arrive in messages with role `tool`{retrieve_rule}
- In `final_answer`, ALWAYS use markdown code blocks (```language) for code, logs, terminal outputs, and file contents
- Use backticks (`) for inline code, such as file paths, variables, and short commands

### Example Tool Call
{{"thought":"Need to call an available tool","tool_call":{{"name":"tool_name","arguments":{{}}}},"final_answer":null,"awaiting_user_input":null}}

### Example Final Answer
{{"thought":"File read, answer ready","tool_call":null,"final_answer":"Here is the content of `file.txt`:\n\n```rust\nfn main() {{\n    println!(\"Hello world\");\n}}\n```","awaiting_user_input":null}}

### Example Awaiting User Input
{{"thought":"Need the APK source before continuing","tool_call":null,"final_answer":null,"awaiting_user_input":{{"kind":"url_or_file","prompt":"Send a direct download link for the APK or upload the APK file so I can continue."}}}}"#,
        todo_blocked_rule = todo_blocked_rule,
        retrieve_rule = retrieve_rule,
    )
}

fn strip_structured_output_requirement(prompt: &str) -> String {
    prompt
        .lines()
        .filter(|line| !line.contains("Respond ONLY with valid JSON"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Create the system prompt for the agent
///
/// This function builds the complete system prompt by:
/// 1. Adding built-in operational instructions
/// 2. Separating date/time context into `date_suffix` for cache-friendly assembly
///
/// The `tool_ctx` provides the full catalog specs (for workflow hints and date
/// context) and the activatable capability groups (for the "Available Tool
/// Groups" block in the lazy tool protocol).
pub async fn create_agent_system_prompt(
    _task: &str,
    tool_ctx: PromptToolContext<'_>,
    structured_output: bool,
    _session: &mut AgentSession,
    prompt_instructions: Option<&str>,
) -> ComposedPrompt {
    let catalog_specs = tool_ctx.catalog_specs();

    // Build date_context separately — it will be inserted between stable
    // and volatile system messages by the fold pipeline.
    let date_context = build_date_context(catalog_specs);

    let base_prompt = get_fallback_prompt();

    let base_prompt = if let Some(instructions) = normalize_prompt_instructions(prompt_instructions)
    {
        format!("{base_prompt}\n\nAdditional agent role instructions:\n{instructions}")
    } else {
        base_prompt
    };

    let base_prompt = if structured_output {
        base_prompt
    } else {
        strip_structured_output_requirement(&base_prompt)
    };

    // Workflow guidance is stable for a given tool-set.
    let base_prompt = if let Some(workflow_guidance) = build_workflow_guidance(catalog_specs) {
        format!("{base_prompt}\n\n{workflow_guidance}")
    } else {
        base_prompt
    };

    // Category list block: tells the model which capability groups it can
    // retrieve.  Static for a run — placed in the cacheable prefix.
    let base_prompt =
        if let Some(category_block) = build_category_list_block(tool_ctx.available_groups()) {
            format!("{base_prompt}\n\n{category_block}")
        } else {
            base_prompt
        };

    let base = if structured_output {
        let structured_output =
            build_structured_output_instructions(catalog_specs, tool_ctx.has_retrievable_groups());
        format!("{base_prompt}\n\n{structured_output}")
    } else {
        base_prompt
    };

    ComposedPrompt {
        base,
        date_suffix: date_context,
    }
}

fn normalize_prompt_instructions(prompt_instructions: Option<&str>) -> Option<&str> {
    prompt_instructions.and_then(|instructions| {
        let trimmed = instructions.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    })
}

/// Create a minimal system prompt for sub-agent execution.
#[must_use]
pub fn create_sub_agent_system_prompt(
    _task: &str,
    tool_ctx: PromptToolContext<'_>,
    structured_output: bool,
    extra_context: Option<&str>,
) -> ComposedPrompt {
    let catalog_specs = tool_ctx.catalog_specs();

    // Build date_context separately — it will be inserted between stable
    // and volatile system messages by the fold pipeline.
    let date_context = build_date_context(catalog_specs);

    // Task is intentionally excluded from the system prompt to keep the prefix
    // cache-stable across different sub-agent invocations.  The task reaches the
    // model exclusively via the first user message (AgentMessage::user_task).
    let mut base_prompt = "You are a lightweight sub-agent for draft work.\n\
You do NOT communicate with the user directly and return the result only to the orchestrator.\n\
Use only available tools if necessary.\n\
Do not spawn, wait for, or cancel sub-agents and do not send files to the user."
        .to_string();

    if let Some(extra) = extra_context
        && !extra.trim().is_empty()
    {
        base_prompt.push_str("\n\nAdditional context:\n");
        base_prompt.push_str(extra.trim());
    }

    let base_prompt = if structured_output {
        base_prompt
    } else {
        strip_structured_output_requirement(&base_prompt)
    };

    let base_prompt = if let Some(workflow_guidance) = build_workflow_guidance(catalog_specs) {
        format!("{base_prompt}\n\n{workflow_guidance}")
    } else {
        base_prompt
    };

    // Category list block for sub-agents with retrievable groups.
    let base_prompt =
        if let Some(category_block) = build_category_list_block(tool_ctx.available_groups()) {
            format!("{base_prompt}\n\n{category_block}")
        } else {
            base_prompt
        };

    let base = if structured_output {
        let structured_output =
            build_structured_output_instructions(catalog_specs, tool_ctx.has_retrievable_groups());
        format!("{base_prompt}\n\n{structured_output}")
    } else {
        base_prompt
    };

    ComposedPrompt {
        base,
        date_suffix: date_context,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_date_context_contains_date() {
        let context = build_date_context(&[]);
        assert!(context.contains("CURRENT DATE AND TIME"));
        assert!(context.contains("Today:"));
        assert!(context.contains("Current local timezone:"));
    }

    #[test]
    fn test_fallback_prompt_omits_tool_specific_guidance() {
        let prompt = get_fallback_prompt();
        assert!(prompt.contains("Oxide Agent"));
        assert!(prompt.contains("Follow the active topic AGENTS.md instructions"));
        assert!(!prompt.contains("create and maintain a todo list"));
        assert!(!prompt.contains("sandbox and file tools"));
        assert!(!prompt.contains("web tools"));
    }

    #[tokio::test]
    async fn test_create_agent_system_prompt_appends_role_instructions() {
        let tools = [ToolDefinition {
            name: "demo_tool".to_string(),
            description: "demo".to_string(),
            parameters: serde_json::json!({ "type": "object" }),
        }];
        let mut session = AgentSession::new(1_i64.into());

        let prompt = create_agent_system_prompt(
            "demo task",
            PromptToolContext::from_tools(&tools),
            true,
            &mut session,
            Some("Stay within the infra role."),
        )
        .await;
        let prompt = prompt.full_prompt();

        assert!(prompt.contains("Additional agent role instructions:"));
        assert!(prompt.contains("Stay within the infra role."));
    }

    #[tokio::test]
    async fn test_create_agent_system_prompt_adds_reminder_guidance() {
        let tools = [ToolDefinition {
            name: "reminder_schedule".to_string(),
            description: "demo".to_string(),
            parameters: serde_json::json!({ "type": "object" }),
        }];
        let mut session = AgentSession::new(1_i64.into());

        let prompt = create_agent_system_prompt(
            "demo task",
            PromptToolContext::from_tools(&tools),
            true,
            &mut session,
            None,
        )
        .await;
        let prompt = prompt.full_prompt();

        assert!(prompt.contains("## Reminder Scheduling"));
        assert!(prompt.contains("Do not compute unix timestamps by hand for reminders"));
    }

    #[tokio::test]
    async fn test_create_agent_system_prompt_adds_task_tracking_only_with_todos() {
        let mut session = AgentSession::new(1_i64.into());
        let prompt = create_agent_system_prompt(
            "demo task",
            PromptToolContext::from_tools(&[]),
            true,
            &mut session,
            None,
        )
        .await;
        let prompt = prompt.full_prompt();
        assert!(!prompt.contains("## Workflow Hints"));
        assert!(!prompt.contains("write_todos"));

        let tools = [ToolDefinition {
            name: "write_todos".to_string(),
            description: "demo".to_string(),
            parameters: serde_json::json!({ "type": "object" }),
        }];
        let mut session = AgentSession::new(1_i64.into());
        let prompt = create_agent_system_prompt(
            "demo task",
            PromptToolContext::from_tools(&tools),
            true,
            &mut session,
            None,
        )
        .await;
        let prompt = prompt.full_prompt();

        assert!(prompt.contains("## Workflow Hints"));
        assert!(prompt.contains("### Task Tracking"));
        assert!(prompt.contains("call `write_todos` before starting"));
    }

    #[tokio::test]
    async fn test_create_agent_system_prompt_adds_file_workflow_guidance() {
        let tools = [
            ToolDefinition {
                name: "describe_video_file".to_string(),
                description: "demo".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
            },
            ToolDefinition {
                name: "text_to_speech_en_file".to_string(),
                description: "demo".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
            },
        ];
        let mut session = AgentSession::new(1_i64.into());

        let prompt = create_agent_system_prompt(
            "demo task",
            PromptToolContext::from_tools(&tools),
            true,
            &mut session,
            None,
        )
        .await;
        let prompt = prompt.full_prompt();

        assert!(prompt.contains("## File Workflows"));
        assert!(prompt.contains("operate on the sandbox file instead of summarizing it"));
        assert!(prompt.contains("Use `describe_video_file` only when"));
        assert!(prompt.contains("Use `text_to_speech_en_file` when"));
        assert!(!prompt.contains("describe_image_file"));
        assert!(!prompt.contains("transcribe_audio_file"));
        assert!(!prompt.contains("text_to_speech_ru_file"));
    }

    #[tokio::test]
    async fn test_create_agent_system_prompt_requires_download_url_in_final_answer() {
        let tools = [ToolDefinition {
            name: "send_file_to_user".to_string(),
            description: "demo".to_string(),
            parameters: serde_json::json!({ "type": "object" }),
        }];
        let mut session = AgentSession::new(1_i64.into());

        let prompt = create_agent_system_prompt(
            "demo task",
            PromptToolContext::from_tools(&tools),
            true,
            &mut session,
            None,
        )
        .await;
        let prompt = prompt.full_prompt();

        assert!(prompt.contains("If `send_file_to_user` returns `download_url`"));
        assert!(prompt.contains("main chat response"));
    }

    #[tokio::test]
    async fn test_create_agent_system_prompt_adds_browser_direct_control_guidance() {
        let tools = [
            ToolDefinition {
                name: "browser_start".to_string(),
                description: "demo".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
            },
            ToolDefinition {
                name: "browser_observe".to_string(),
                description: "demo".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
            },
            ToolDefinition {
                name: "browser_execute".to_string(),
                description: "demo".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
            },
            ToolDefinition {
                name: "browser_extract".to_string(),
                description: "demo".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
            },
            ToolDefinition {
                name: "browser_close".to_string(),
                description: "demo".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
            },
        ];
        let mut session = AgentSession::new(1_i64.into());

        let prompt = create_agent_system_prompt(
            "demo task",
            PromptToolContext::from_tools(&tools),
            true,
            &mut session,
            None,
        )
        .await;
        let prompt = prompt.full_prompt();

        assert!(prompt.contains("## Browser Direct Control"));
        assert!(prompt.contains("Use `browser_observe` to capture the current page state"));
        assert!(prompt.contains("Use `browser_execute` to perform one concrete browser action"));
        assert!(prompt.contains("Use `browser_extract` to pull structured data"));
        assert!(prompt.contains("`selector` + `fields` instead of custom JavaScript"));
        assert!(
            prompt.contains("Use `browser_close` when the browser session is no longer needed")
        );
    }

    #[tokio::test]
    async fn test_web_guidance_mentions_only_available_web_tools() {
        let tools = [ToolDefinition {
            name: "web_markdown".to_string(),
            description: "demo".to_string(),
            parameters: serde_json::json!({ "type": "object" }),
        }];
        let mut session = AgentSession::new(1_i64.into());

        let prompt = create_agent_system_prompt(
            "demo task",
            PromptToolContext::from_tools(&tools),
            true,
            &mut session,
            None,
        )
        .await;
        let prompt = prompt.full_prompt();

        assert!(prompt.contains("### Web Research"));
        assert!(prompt.contains("Use `web_markdown`"));
        assert!(!prompt.contains("web_search"));
        assert!(!prompt.contains("web_extract"));
    }

    #[tokio::test]
    async fn test_workflow_guidance_deduplicates_sections() {
        let tools = [
            ToolDefinition {
                name: "web_search".to_string(),
                description: "demo".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
            },
            ToolDefinition {
                name: "web_markdown".to_string(),
                description: "demo".to_string(),
                parameters: serde_json::json!({ "type": "object" }),
            },
        ];
        let mut session = AgentSession::new(1_i64.into());

        let prompt = create_agent_system_prompt(
            "demo task",
            PromptToolContext::from_tools(&tools),
            true,
            &mut session,
            None,
        )
        .await;
        let prompt = prompt.full_prompt();

        assert_eq!(prompt.matches("### Web Research").count(), 1);
    }

    #[test]
    fn test_structured_output_instructions_include_awaiting_user_input() {
        let prompt = build_structured_output_instructions(&[], false);

        assert!(prompt.contains("awaiting_user_input"));
        assert!(prompt.contains("url_or_file"));
        assert!(!prompt.contains("blocked_on_user"));
        assert!(!prompt.contains("read_file"));
        assert!(
            prompt.contains("EXACTLY one of `tool_call`, `final_answer`, or `awaiting_user_input`")
        );
    }

    #[test]
    fn test_structured_output_instructions_include_todo_blocked_rule_only_with_todos() {
        let tools = [ToolDefinition {
            name: "write_todos".to_string(),
            description: "demo".to_string(),
            parameters: serde_json::json!({ "type": "object" }),
        }];

        let prompt = build_structured_output_instructions(&tools, false);

        assert!(prompt.contains("blocked_on_user"));
    }

    /// Date/time block must be at the end of the system prompt, not the beginning,
    /// to preserve prompt cache hit across requests (stable prefix + dynamic suffix).
    #[tokio::test]
    async fn test_date_context_at_end_of_main_agent_prompt() {
        let tools = [ToolDefinition {
            name: "demo_tool".to_string(),
            description: "demo".to_string(),
            parameters: serde_json::json!({ "type": "object" }),
        }];
        let mut session = AgentSession::new(1_i64.into());

        let prompt = create_agent_system_prompt(
            "demo task",
            PromptToolContext::from_tools(&tools),
            true,
            &mut session,
            None,
        )
        .await;

        let full = prompt.full_prompt();
        let date_pos = full
            .find("### CURRENT DATE AND TIME")
            .expect("date context must be present");
        let structured_pos = full
            .find("## STRUCTURED OUTPUT")
            .expect("structured output must be present");

        assert!(
            date_pos > structured_pos,
            "date context must come AFTER structured output for cache hit, \
             but date is at {date_pos} and structured output at {structured_pos}"
        );
    }

    /// Task must NOT appear in the sub-agent system prompt — it is delivered
    /// exclusively via the first user message to keep the prefix cache-stable.
    #[test]
    fn test_sub_agent_prompt_excludes_task() {
        let tools = [ToolDefinition {
            name: "demo_tool".to_string(),
            description: "demo".to_string(),
            parameters: serde_json::json!({ "type": "object" }),
        }];

        let unique_task = "XRAY_UNIQUE_TASK_MARKER_7f3a";
        let prompt = create_sub_agent_system_prompt(
            unique_task,
            PromptToolContext::from_tools(&tools),
            true,
            None,
        );
        let prompt = prompt.full_prompt();

        assert!(
            !prompt.contains(unique_task),
            "sub-agent system prompt must not contain the task string; \
             task is delivered via the user message for cache stability"
        );
        assert!(
            prompt.contains("lightweight sub-agent"),
            "sub-agent system prompt must still contain identity instructions"
        );
    }

    /// Date/time block must be at the end of the sub-agent system prompt too.
    #[test]
    fn test_date_context_at_end_of_sub_agent_prompt() {
        let tools = [ToolDefinition {
            name: "demo_tool".to_string(),
            description: "demo".to_string(),
            parameters: serde_json::json!({ "type": "object" }),
        }];

        let prompt = create_sub_agent_system_prompt(
            "demo task",
            PromptToolContext::from_tools(&tools),
            true,
            None,
        );

        let full = prompt.full_prompt();
        let date_pos = full
            .find("### CURRENT DATE AND TIME")
            .expect("date context must be present");
        let structured_pos = full
            .find("## STRUCTURED OUTPUT")
            .expect("structured output must be present");

        assert!(
            date_pos > structured_pos,
            "sub-agent date context must come AFTER structured output for cache hit, \
             but date is at {date_pos} and structured output at {structured_pos}"
        );
    }

    // --- Tool schema duplication cache-miss tests ---

    /// Helper: build a realistic set of tool definitions for size measurement.
    fn realistic_tools() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "execute_command".to_string(),
                description: "Execute a shell command in the sandbox".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command to run" },
                        "timeout": { "type": "integer", "description": "Timeout in seconds" }
                    },
                    "required": ["command"]
                }),
            },
            ToolDefinition {
                name: "read_file".to_string(),
                description: "Read a file from the sandbox".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path" },
                        "encoding": { "type": "string", "description": "File encoding" }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: "write_file".to_string(),
                description: "Write a file to the sandbox".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path" },
                        "content": { "type": "string", "description": "File content" }
                    },
                    "required": ["path", "content"]
                }),
            },
            ToolDefinition {
                name: "web_search".to_string(),
                description: "Search the web".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query" },
                        "max_results": { "type": "integer", "description": "Max results" }
                    },
                    "required": ["query"]
                }),
            },
            ToolDefinition {
                name: "write_todos".to_string(),
                description: "Update the task todo list".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "todos": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "content": { "type": "string" },
                                    "status": { "type": "string", "enum": ["pending","in_progress","completed","cancelled"] },
                                    "priority": { "type": "string", "enum": ["high","medium","low"] }
                                },
                                "required": ["content","status","priority"]
                            }
                        }
                    },
                    "required": ["todos"]
                }),
            },
        ]
    }

    /// Verifies that build_structured_output_instructions() does NOT embed full tool
    /// JSON schemas or tool descriptions in the system prompt text.  Tool schemas are
    /// delivered exclusively via the native `tools[]` API payload.  The structured
    /// output instructions contain only the JSON schema, rules, and examples.
    #[test]
    fn test_structured_output_uses_compact_tool_names_not_schemas() {
        let tools = realistic_tools();

        let instructions = build_structured_output_instructions(&tools, false);

        // 1. The prompt must NOT contain full JSON schemas.
        let tools_json = serde_json::to_string_pretty(&tools)
            .expect("serializing tool definitions must succeed");
        assert!(
            !instructions.contains(&tools_json),
            "prompt must NOT embed full pretty-printed tools_json — schemas belong in native tools[] only"
        );

        // 2. The prompt must NOT contain any tool descriptions or parameter schemas.
        for tool in &tools {
            assert!(
                !instructions.contains(&tool.description),
                "tool description for '{}' must NOT appear in prompt — it is in native tools[]",
                tool.name
            );
        }

        // 3. No "## Available Tools" section — tool discovery is via tools[].
        assert!(
            !instructions.contains("## Available Tools"),
            "structured output instructions must NOT contain an Available Tools section"
        );

        // 4. The instructions are much smaller than the full tools JSON.
        let instructions_bytes = instructions.len();
        let tools_json_bytes = tools_json.len();
        eprintln!(
            "Structured output instructions: {instructions_bytes} bytes\n\
             Full tools JSON: {tools_json_bytes} bytes\n\
             Reduction: {:.1}x",
            tools_json_bytes as f64 / instructions_bytes.max(1) as f64
        );
        assert!(
            instructions_bytes < tools_json_bytes,
            "structured output instructions ({instructions_bytes} bytes) must be smaller than \
             tools JSON ({tools_json_bytes} bytes)"
        );
    }

    /// Verifies cache-stability: same tool set → byte-identical base prompt,
    /// and adding a tool preserves the stable prefix (fallback + workflow hints).
    #[tokio::test]
    async fn test_tool_addition_preserves_stable_prefix() {
        let tools_small: Vec<ToolDefinition> = realistic_tools();
        let tools_large = {
            let mut t = tools_small.clone();
            t.push(ToolDefinition {
                name: "send_file_to_user".to_string(),
                description: "Send a sandbox file to the user".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Sandbox file path" }
                    },
                    "required": ["path"]
                }),
            });
            t
        };

        let mut session1 = AgentSession::new(1_i64.into());
        let mut session2 = AgentSession::new(1_i64.into());

        let prompt_small = create_agent_system_prompt(
            "task",
            PromptToolContext::from_tools(&tools_small),
            true,
            &mut session1,
            None,
        )
        .await;
        let prompt_large = create_agent_system_prompt(
            "task",
            PromptToolContext::from_tools(&tools_large),
            true,
            &mut session2,
            None,
        )
        .await;

        // Property 1: same tool set → identical base prompt.
        let mut session3 = AgentSession::new(1_i64.into());
        let prompt_same = create_agent_system_prompt(
            "task",
            PromptToolContext::from_tools(&tools_small),
            true,
            &mut session3,
            None,
        )
        .await;

        assert_eq!(
            prompt_small.base, prompt_same.base,
            "same tool set must produce byte-identical base prompt"
        );

        // Property 2: different tool sets → stable prefix preserved.
        // The shared prefix includes fallback + workflow hints.  The suffix
        // changes because workflow hints differ (`send_file_to_user` adds
        // file delivery guidance).
        let shared_prefix_len = prompt_small
            .base
            .chars()
            .zip(prompt_large.base.chars())
            .take_while(|(a, b)| a == b)
            .count();

        eprintln!(
            "Prefix stability analysis:\n\
             - Shared prefix length: {shared_prefix_len} chars\n\
             - prompt_small base: {} chars\n\
             - prompt_large base: {} chars",
            prompt_small.base.len(),
            prompt_large.base.len(),
        );

        // The stable prefix (fallback + shared workflow hints) must be substantial.
        assert!(shared_prefix_len > 40, "stable prefix must be substantial");
    }

    /// Verifies that the prompt does NOT duplicate tool schemas or descriptions
    /// from the native `tools[]` payload.  The prompt contains only instructions
    /// and rules — tool discovery is via `tools[]`.
    #[test]
    fn test_prompt_and_native_payload_are_complementary() {
        let tools = realistic_tools();

        // Structured output instructions (no tool schemas).
        let instructions = build_structured_output_instructions(&tools, false);

        // Native OpenAI-format tools[] (full schemas).
        let native_tools: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect();
        let native_tools_json =
            serde_json::to_string(&native_tools).expect("serializing native tools must succeed");

        // No description or parameter content from native tools should leak into prompt.
        for tool in &tools {
            assert!(
                !instructions.contains(&tool.description),
                "tool description for '{}' must NOT appear in prompt",
                tool.name
            );
            let params_str = serde_json::to_string(&tool.parameters)
                .expect("serializing parameters must succeed");
            assert!(
                !instructions.contains(&params_str),
                "tool parameters for '{}' must NOT appear in prompt",
                tool.name
            );
        }

        eprintln!(
            "Wire metrics (no duplication):\n\
             - Prompt structured output instructions: {} bytes\n\
             - Native tools[] payload: {} bytes (full schemas)",
            instructions.len(),
            native_tools_json.len(),
        );
    }
}
