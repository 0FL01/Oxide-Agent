//! Prompt composer module
//!
//! Handles construction of system prompts for the agent, including skill-based
//! prompts, date context, and fallback prompts.

use crate::agent::session::AgentSession;
use crate::agent::skills::{SkillContext, SkillRegistry};
use crate::llm::ToolDefinition;
use tracing::{error, info, warn};

/// Build the date context block for the system prompt
fn build_date_context() -> String {
    let now = chrono::Local::now();
    let current_date = now.format("%Y-%m-%d %H:%M:%S").to_string();
    let current_day = now.format("%A").to_string();

    format!(
        "### CURRENT DATE AND TIME\nToday: {current_date}, {current_day}\nIMPORTANT: Always use this date as the current date. If search results (web_search) contain phrases like 'today', 'tomorrow', or dates contradicting this, consider the search results outdated and interpret them relative to the date above.\n\n"
    )
}

/// Get the fallback prompt when AGENT.md is missing
#[must_use]
pub fn get_fallback_prompt() -> String {
    r"You are an AI agent with access to a sandbox environment and web search.
## Available Tools (Basic Examples):
- **execute_command**: execute bash command in sandbox (available: python3, pip, ffmpeg, yt-dlp, curl, wget, date, cat, ls, grep and other standard utilities)
- **write_file**: write content to file
- **read_file**: read file content
- **web_search**: search information on the web
- **web_extract**: extract text from web pages
- **write_todos**: create or update todo list
## Important Rules:
- If real data is needed - USE TOOLS
- Use Python for calculations
- After receiving tool result - analyze it and continue working
- For COMPLEX requests, YOU MUST use write_todos to create a plan"
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
  "final_answer": "Final answer to the user"
}}

Rules:
- EXACTLY one of `tool_call` or `final_answer` must be filled (the other = null)
- If a tool is needed: `tool_call` = object, `final_answer` = null
- If answer is ready: `tool_call` = null, `final_answer` = string
- `tool_call.arguments` is always a JSON object
- No extra keys, markdown, XML, explanations, or text outside JSON
- Tool results arrive in messages with role `tool`
- In `final_answer`, ALWAYS use markdown code blocks (```language) for code, logs, terminal outputs, and file contents
- Use backticks (`) for inline code, such as file paths, variables, and short commands

### Example Tool Call
{{"thought":"Need to read a file","tool_call":{{"name":"read_file","arguments":{{"filePath":"/abs/path/to/file.txt"}}}},"final_answer":null}}

### Example Final Answer
{{"thought":"File read, answer ready","tool_call":null,"final_answer":"Here is the content of `file.txt`:\n\n```rust\nfn main() {{\n    println!(\"Hello world\");\n}}\n```"}}

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
/// 2. Either loading skill-based prompts or falling back to AGENT.md
pub async fn create_agent_system_prompt(
    task: &str,
    tools: &[ToolDefinition],
    structured_output: bool,
    skill_registry: Option<&mut SkillRegistry>,
    session: &mut AgentSession,
) -> String {
    let date_context = build_date_context();

    let base_prompt = if let Some(registry) = skill_registry {
        match registry.build_prompt(task).await {
            Ok(skill_prompt) if !skill_prompt.content.is_empty() => {
                session.set_loaded_skills(&skill_prompt.skills);
                info!(
                    skills = ?skill_prompt.skills,
                    total_tokens = skill_prompt.token_count,
                    skipped = ?skill_prompt.skipped,
                    "Skills loaded for request"
                );
                skill_prompt.content
            }
            Ok(_) => {
                warn!("Skills prompt empty, falling back to AGENT.md");
                String::new()
            }
            Err(err) => {
                warn!(error = %err, "Failed to build skills prompt, falling back to AGENT.md");
                String::new()
            }
        }
    } else {
        String::new()
    };

    let base_prompt = if !base_prompt.is_empty() {
        base_prompt
    } else {
        let empty_skills: [SkillContext; 0] = [];
        session.set_loaded_skills(&empty_skills);

        match std::fs::read_to_string("AGENT.md") {
            Ok(prompt) => prompt,
            Err(e) => {
                error!("Failed to load AGENT.md: {e}. Using default fallback prompt.");
                get_fallback_prompt()
            }
        }
    };

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
    }

    #[test]
    fn test_fallback_prompt_contains_tools() {
        let prompt = get_fallback_prompt();
        assert!(prompt.contains("execute_command"));
        assert!(prompt.contains("write_file"));
        assert!(prompt.contains("read_file"));
    }
}
