//! Prompt composer module
//!
//! Handles construction of system prompts for the agent, including
//! date context and fallback prompts.

use crate::agent::session::AgentSession;
use crate::agent::skills::{SkillContext, SkillRegistry};
use crate::llm::ToolDefinition;

/// Build the date context block for the system prompt
fn build_date_context() -> String {
    let now = chrono::Local::now();
    let current_date = now.format("%Y-%m-%d %H:%M:%S").to_string();
    let current_day = now.format("%A").to_string();
    let current_offset = now.format("UTC%:z").to_string();

    format!(
        "### CURRENT DATE AND TIME\nToday: {current_date}, {current_day}\nCurrent local timezone: {current_offset}\nIMPORTANT: Always use this date as the current date. If search results (`web_search` or `searxng_search`) contain phrases like 'today', 'tomorrow', or dates contradicting this, consider the search results outdated and interpret them relative to the date above.\n\n"
    )
}

fn build_reminder_guidance(tools: &[ToolDefinition]) -> Option<&'static str> {
    tools.iter().any(|tool| tool.name == "reminder_schedule").then_some(
        "## Reminder Scheduling\n- The current date/time block above is the source of truth for local time\n- Do not compute unix timestamps by hand for reminders\n- For a one-time reminder, use `kind=once` with `date` + `time` and optional `timezone`\n- For repeat-after-N-minutes or repeat-after-N-hours, use `kind=interval` with `every_minutes` or `every_hours`\n- For wall-clock schedules like every day at 09:00 or weekdays at 18:30, use `kind=cron` with `time`, optional `weekdays`, and optional `timezone`\n- Do not use `kind=interval` for calendar schedules like every day at 09:00 because interval means fixed delay after the previous run\n- When `timezone` is omitted, reminder scheduling uses the current local timezone shown above"
    )
}

/// Get the built-in fallback prompt for the main agent.
#[must_use]
pub fn get_fallback_prompt() -> String {
    r"You are an AI agent operating inside Oxide Agent.
## Core Rules:
- Follow the active topic AGENTS.md instructions when they are present in memory
- Use tools whenever you need real data, file contents, system state, or external information
- After each tool result, analyze it and continue until the task is complete
- For complex work, create and maintain a todo list
- Keep answers concise, accurate, and directly useful to the user
## Tool Usage:
- Use sandbox and file tools for local work
- Use web tools for external information
- Prefer verifying your changes with relevant tests or checks when possible"
        .to_string()
}

/// Build instructions for mandatory structured output (JSON).
#[must_use]
pub fn build_structured_output_instructions(tools: &[ToolDefinition]) -> String {
    let tools_json = serde_json::to_string_pretty(&tools).unwrap_or_else(|_| "[]".to_string());

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
- Use `awaiting_user_input` when you need the user to provide missing text, a link, a file, or either a link/file before the task can continue
- If you maintain a todo list and the remaining work is blocked on the user, mark the relevant todo as `blocked_on_user` before returning `awaiting_user_input`
- `awaiting_user_input.kind` must be exactly one of: `text`, `url`, `file`, `url_or_file`
- `awaiting_user_input.prompt` must be a short, direct request telling the user what to send next
- `tool_call.arguments` is always a JSON object
- No extra keys, markdown, XML, explanations, or text outside JSON
- Tool results arrive in messages with role `tool`
- In `final_answer`, ALWAYS use markdown code blocks (```language) for code, logs, terminal outputs, and file contents
- Use backticks (`) for inline code, such as file paths, variables, and short commands

### Example Tool Call
{{"thought":"Need to read a file","tool_call":{{"name":"read_file","arguments":{{"filePath":"/abs/path/to/file.txt"}}}},"final_answer":null,"awaiting_user_input":null}}

### Example Final Answer
{{"thought":"File read, answer ready","tool_call":null,"final_answer":"Here is the content of `file.txt`:\n\n```rust\nfn main() {{\n    println!(\"Hello world\");\n}}\n```","awaiting_user_input":null}}

### Example Awaiting User Input
{{"thought":"Need the APK source before continuing","tool_call":null,"final_answer":null,"awaiting_user_input":{{"kind":"url_or_file","prompt":"Send a direct download link for the APK or upload the APK file so I can continue."}}}}

## Available Tools (JSON schema)
{tools_json}"#,
        tools_json = tools_json
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
/// 1. Adding date/time context
/// 2. Adding built-in operational instructions
pub async fn create_agent_system_prompt(
    _task: &str,
    tools: &[ToolDefinition],
    structured_output: bool,
    _skill_registry: Option<&mut SkillRegistry>,
    _session: &mut AgentSession,
    prompt_instructions: Option<&str>,
) -> String {
    let date_context = build_date_context();
    let empty_skills: [SkillContext; 0] = [];
    _session.set_loaded_skills(&empty_skills);

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

    let reminder_guidance = build_reminder_guidance(tools).unwrap_or_default();

    if structured_output {
        let structured_output = build_structured_output_instructions(tools);
        format!("{date_context}{base_prompt}\n\n{reminder_guidance}\n\n{structured_output}")
    } else {
        format!("{date_context}{base_prompt}\n\n{reminder_guidance}")
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
    task: &str,
    tools: &[ToolDefinition],
    structured_output: bool,
    extra_context: Option<&str>,
) -> String {
    let date_context = build_date_context();
    let mut base_prompt = format!(
        "You are a lightweight sub-agent for draft work.\n\
You do NOT communicate with the user directly and return the result only to the orchestrator.\n\
Your task: {task}.\n\
Use only available tools if necessary.\n\
Do not call delegate_to_sub_agent and do not send files to the user."
    );

    if let Some(extra) = extra_context {
        if !extra.trim().is_empty() {
            base_prompt.push_str("\n\nAdditional context:\n");
            base_prompt.push_str(extra.trim());
        }
    }

    let base_prompt = if structured_output {
        base_prompt
    } else {
        strip_structured_output_requirement(&base_prompt)
    };

    if structured_output {
        let structured_output = build_structured_output_instructions(tools);
        format!("{date_context}{base_prompt}\n\n{structured_output}")
    } else {
        format!("{date_context}{base_prompt}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_date_context_contains_date() {
        let context = build_date_context();
        assert!(context.contains("CURRENT DATE AND TIME"));
        assert!(context.contains("Today:"));
        assert!(context.contains("Current local timezone:"));
    }

    #[test]
    fn test_fallback_prompt_contains_tools() {
        let prompt = get_fallback_prompt();
        assert!(prompt.contains("Oxide Agent"));
        assert!(prompt.contains("Follow the active topic AGENTS.md instructions"));
        assert!(prompt.contains("create and maintain a todo list"));
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
            &tools,
            true,
            None,
            &mut session,
            Some("Stay within the infra role."),
        )
        .await;

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

        let prompt =
            create_agent_system_prompt("demo task", &tools, true, None, &mut session, None).await;

        assert!(prompt.contains("## Reminder Scheduling"));
        assert!(prompt.contains("Do not compute unix timestamps by hand for reminders"));
    }

    #[test]
    fn test_structured_output_instructions_include_awaiting_user_input() {
        let prompt = build_structured_output_instructions(&[]);

        assert!(prompt.contains("awaiting_user_input"));
        assert!(prompt.contains("blocked_on_user"));
        assert!(prompt.contains("url_or_file"));
        assert!(
            prompt.contains("EXACTLY one of `tool_call`, `final_answer`, or `awaiting_user_input`")
        );
    }
}
