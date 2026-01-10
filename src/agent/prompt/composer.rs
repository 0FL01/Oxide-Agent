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

    format!(
        "### ТЕКУЩАЯ ДАТА И ВРЕМЯ\nСегодня: {current_date}, {current_day_ru}\nВАЖНО: Всегда используй эту дату как текущую. Если результаты поиска (web_search) содержат фразы 'сегодня', 'завтра' или даты, которые противоречат этой, считай результаты поиска устаревшими и интерпретируй их относительно указанной выше даты.\n\n"
    )
}

/// Get the fallback prompt when AGENT.md is missing
fn get_fallback_prompt() -> String {
    r"Ты - AI-агент с доступом к изолированной среде выполнения (sandbox) и веб-поиску.
## Доступные инструменты (основные примеры):
- **execute_command**: выполнить bash-команду в sandbox (доступны: python3, pip, ffmpeg, yt-dlp, curl, wget, date, cat, ls, grep и другие стандартные утилиты)
- **write_file**: записать содержимое в файл
- **read_file**: прочитать содержимое файла
- **web_search**: поиск информации в интернете
- **web_extract**: извлечение текста из веб-страниц
- **write_todos**: создать или обновить список задач
## Важные правила:
- Если нужны реальные данные - ИСПОЛЬЗУЙ ИНСТРУМЕНТЫ
- Для вычислений используй Python
- После получения результата инструмента - проанализируй его и продолжай работу
- Для СЛОЖНЫХ запросов ОБЯЗАТЕЛЬНО используй write_todos для создания плана"
        .to_string()
}

fn build_structured_output_instructions(tools: &[ToolDefinition]) -> String {
    let tools_json = serde_json::to_string_pretty(&tools).unwrap_or_else(|_| "[]".to_string());

    format!(
        r#"## СТРУКТУРИРОВАННЫЙ ВЫВОД (ОБЯЗАТЕЛЬНО)
Ты ДОЛЖЕН отвечать ТОЛЬКО валидным JSON-объектом строго по схеме:
{{
  "thought": "Краткое описание решения и шага",
  "tool_call": {{
    "name": "tool_name",
    "arguments": {{}}
  }},
  "final_answer": "Финальный ответ пользователю"
}}

Правила:
- РОВНО одно из полей `tool_call` или `final_answer` должно быть заполнено (второе = null)
- Если нужен инструмент: `tool_call` = объект, `final_answer` = null
- Если ответ готов: `tool_call` = null, `final_answer` = строка
- `tool_call.arguments` всегда JSON-объект
- Никаких дополнительных ключей, markdown, XML, пояснений или текста вне JSON
- Результаты инструментов приходят в сообщениях с ролью `tool`

## Доступные инструменты (JSON schema)
{tools_json}"#
    )
}

/// Create the system prompt for the agent
///
/// This function builds the complete system prompt by:
/// 1. Adding date/time context
/// 2. Either loading skill-based prompts or falling back to AGENT.md
pub async fn create_agent_system_prompt(
    task: &str,
    tools: &[ToolDefinition],
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

    let structured_output = build_structured_output_instructions(tools);
    format!("{date_context}{base_prompt}\n\n{structured_output}")
}

/// Create a minimal system prompt for sub-agent execution.
#[must_use]
pub fn create_sub_agent_system_prompt(
    task: &str,
    tools: &[ToolDefinition],
    extra_context: Option<&str>,
) -> String {
    let date_context = build_date_context();
    let mut base_prompt = format!(
        "Ты - легковесный суб-агент для черновой работы.\n\
Ты НЕ общаешься с пользователем напрямую и возвращаешь результат только оркестратору.\n\
Твоя задача: {task}.\n\
Используй только доступные инструменты, если это необходимо.\n\
Не вызывай delegate_to_sub_agent и не отправляй файлы пользователю."
    );

    if let Some(extra) = extra_context {
        if !extra.trim().is_empty() {
            base_prompt.push_str("\n\nДополнительный контекст:\n");
            base_prompt.push_str(extra.trim());
        }
    }

    let structured_output = build_structured_output_instructions(tools);
    format!("{date_context}{base_prompt}\n\n{structured_output}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_date_context_contains_date() {
        let context = build_date_context();
        assert!(context.contains("ТЕКУЩАЯ ДАТА И ВРЕМЯ"));
        assert!(context.contains("Сегодня:"));
    }

    #[test]
    fn test_fallback_prompt_contains_tools() {
        let prompt = get_fallback_prompt();
        assert!(prompt.contains("execute_command"));
        assert!(prompt.contains("write_file"));
        assert!(prompt.contains("read_file"));
    }
}
