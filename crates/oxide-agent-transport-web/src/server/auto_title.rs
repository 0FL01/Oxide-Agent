//! LLM-powered session title generation.
//!
//! On the first task of a new session the web transport generates a short,
//! meaningful title through the same LLM model family that the agent uses.
//! The generated title is persisted only if the user has not manually renamed
//! the session.

use crate::server::types::{AppState, MAX_SESSION_TITLE_CHARS};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use oxide_agent_core::config::ModelInfo;
use oxide_agent_core::llm::Message;
use oxide_agent_web_contracts::WebSessionRecord;
use std::{sync::Arc, time::Duration, time::Instant};
use tracing::{info, warn};

const AUTO_TITLE_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(30);
const AUTO_TITLE_WORKER_POLL_INTERVAL: Duration = Duration::from_secs(60);
const AUTO_TITLE_DUE_BATCH_LIMIT: usize = 16;
const MAX_AUTO_TITLE_ERROR_CHARS: usize = 512;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub(crate) fn prepare_session_auto_title(
    session: &mut WebSessionRecord,
    source_message: String,
    replaceable_title: String,
    now: DateTime<Utc>,
) {
    session.auto_title_source_message = Some(source_message);
    session.auto_title_replaceable_title = Some(replaceable_title);
    session.auto_title_attempts = 0;
    session.auto_title_next_attempt_at = Some(now);
    session.auto_title_last_error = None;
}

pub(crate) fn clear_session_auto_title(session: &mut WebSessionRecord) {
    session.auto_title_source_message = None;
    session.auto_title_replaceable_title = None;
    session.auto_title_attempts = 0;
    session.auto_title_next_attempt_at = None;
    session.auto_title_last_error = None;
}

pub(crate) fn spawn_background_auto_title(state: AppState, user_id: i64, session_id: String) {
    tokio::spawn(async move {
        if !state.auto_title_enabled {
            return;
        }
        if let Err(error) =
            attempt_auto_title_for_session(state.clone(), user_id, &session_id).await
        {
            warn!(session_id = %session_id, error = %error, "auto title generation failed in background");
        }
    });
}

pub(crate) fn spawn_retry_worker(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(AUTO_TITLE_WORKER_POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if !state.auto_title_enabled {
                continue;
            }
            match process_due_auto_titles_once(&state, AUTO_TITLE_DUE_BATCH_LIMIT).await {
                Ok(processed) if processed > 0 => {
                    info!(processed, "auto title retry worker processed due sessions");
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(error = %error, "auto title retry worker failed");
                }
            }
        }
    });
}

pub(crate) async fn process_due_auto_titles_once(
    state: &AppState,
    limit: usize,
) -> Result<usize, String> {
    let sessions = state
        .web_store
        .list_due_auto_title_sessions(Utc::now(), limit)
        .await
        .map_err(|e| e.to_string())?;
    let processed = sessions.len();

    for session in sessions {
        if let Err(error) =
            attempt_auto_title_for_session(state.clone(), session.user_id, &session.session_id)
                .await
        {
            warn!(session_id = %session.session_id, error = %error, "auto title due retry failed");
        }
    }

    Ok(processed)
}

async fn attempt_auto_title_for_session(
    state: AppState,
    user_id: i64,
    session_id: &str,
) -> Result<(), String> {
    // Guard: session must still exist and be renameable.
    let session = state
        .web_store
        .load_session(user_id, session_id)
        .await
        .map_err(|e| e.to_string())?;
    let Some(session) = session else {
        return Ok(());
    };

    let Some(source_message) = session.auto_title_source_message.clone() else {
        return Ok(());
    };
    let Some(replaceable_title) = session.auto_title_replaceable_title.clone() else {
        return Ok(());
    };

    if !session.may_auto_title(&replaceable_title) {
        info!(
            session_id = %session_id,
            current_title_chars = session.title.chars().count(),
            replaceable_title_chars = replaceable_title.chars().count(),
            manually_renamed = session.manually_renamed,
            "auto title stopped because session title is not replaceable"
        );
        clear_pending_auto_title_if_still_present(&state, user_id, session_id).await?;
        return Ok(());
    }

    let model = inherited_title_model(&state);
    info!(
        session_id = %session_id,
        provider = %model.provider,
        model = %model.id,
        attempt = session.auto_title_attempts.saturating_add(1),
        input_chars = source_message.chars().count(),
        replaceable_title_chars = replaceable_title.chars().count(),
        "auto title generation started"
    );

    let raw_title = match tokio::time::timeout(
        AUTO_TITLE_ATTEMPT_TIMEOUT,
        generate_title(
            state.session_manager.llm_client(),
            model,
            &source_message,
            session_id,
        ),
    )
    .await
    {
        Ok(Ok(raw_title)) => raw_title,
        Ok(Err(error)) => {
            schedule_auto_title_retry(&state, user_id, session_id, &error).await?;
            return Ok(());
        }
        Err(_) => {
            let error = format!(
                "auto title generation timed out after {}s",
                AUTO_TITLE_ATTEMPT_TIMEOUT.as_secs()
            );
            schedule_auto_title_retry(&state, user_id, session_id, &error).await?;
            return Ok(());
        }
    };

    let title = sanitize_auto_title(&raw_title);
    if title.is_empty() {
        let error = format!(
            "auto title LLM returned empty title after sanitization; raw_title_chars={}",
            raw_title.chars().count()
        );
        schedule_auto_title_retry(&state, user_id, session_id, &error).await?;
        return Ok(());
    }

    // Double-check: reload session and verify the title is still replaceable.
    // This prevents overwriting a manual rename that happened while the LLM
    // call was in flight.
    let Some(mut session) = state
        .web_store
        .load_session(user_id, session_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };

    if !session.may_auto_title(&replaceable_title) {
        info!(
            session_id = %session_id,
            current_title_chars = session.title.chars().count(),
            generated_title_chars = title.chars().count(),
            replaceable_title_chars = replaceable_title.chars().count(),
            manually_renamed = session.manually_renamed,
            "auto title stopped because session title changed during generation"
        );
        clear_pending_auto_title_if_still_present(&state, user_id, session_id).await?;
        return Ok(());
    }

    let saved_title_chars = title.chars().count();
    session.title = title;
    clear_session_auto_title(&mut session);
    session.updated_at = Utc::now();

    state
        .web_store
        .save_session(session)
        .await
        .map_err(|e| e.to_string())?;

    info!(session_id = %session_id, title_chars = saved_title_chars, "auto title saved");

    Ok(())
}

async fn clear_pending_auto_title_if_still_present(
    state: &AppState,
    user_id: i64,
    session_id: &str,
) -> Result<(), String> {
    let Some(mut session) = state
        .web_store
        .load_session(user_id, session_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    if session.auto_title_source_message.is_none() {
        return Ok(());
    }
    clear_session_auto_title(&mut session);
    session.updated_at = Utc::now();
    state
        .web_store
        .save_session(session)
        .await
        .map_err(|e| e.to_string())
}

async fn schedule_auto_title_retry(
    state: &AppState,
    user_id: i64,
    session_id: &str,
    error: &str,
) -> Result<(), String> {
    let Some(mut session) = state
        .web_store
        .load_session(user_id, session_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let Some(replaceable_title) = session.auto_title_replaceable_title.clone() else {
        return Ok(());
    };
    if session.auto_title_source_message.is_none() || !session.may_auto_title(&replaceable_title) {
        return Ok(());
    }

    let now = Utc::now();
    session.auto_title_attempts = session.auto_title_attempts.saturating_add(1);
    let delay = retry_delay(session.auto_title_attempts);
    let next_attempt_at = now + delay;
    session.auto_title_next_attempt_at = Some(next_attempt_at);
    session.auto_title_last_error = Some(truncate_error(error));
    session.updated_at = now;
    let attempts = session.auto_title_attempts;
    state
        .web_store
        .save_session(session)
        .await
        .map_err(|e| e.to_string())?;
    warn!(
        session_id = %session_id,
        attempts,
        next_attempt_at = %next_attempt_at,
        error = %error,
        "auto title retry scheduled"
    );
    Ok(())
}

fn retry_delay(attempts: u32) -> ChronoDuration {
    let seconds = match attempts {
        0 | 1 => 60,
        2 => 5 * 60,
        3 => 15 * 60,
        4 => 60 * 60,
        _ => 6 * 60 * 60,
    };
    ChronoDuration::seconds(seconds)
}

fn truncate_error(error: &str) -> String {
    error.chars().take(MAX_AUTO_TITLE_ERROR_CHARS).collect()
}

/// Derive the model to use for title generation from the primary agent route.
fn inherited_title_model(state: &AppState) -> ModelInfo {
    let settings = state.session_manager.agent_settings();
    settings
        .get_configured_agent_model_routes()
        .into_iter()
        .next()
        .unwrap_or_else(|| settings.get_configured_agent_model())
}

fn title_reasoning_effort(model: &ModelInfo) -> Option<&'static str> {
    if model.provider.contains("opencode-go") || model.provider.contains("opencode-zen") {
        Some("none")
    } else {
        Some("low")
    }
}

async fn generate_title(
    llm: Arc<oxide_agent_core::llm::LlmClient>,
    mut model: ModelInfo,
    first_user_message: &str,
    session_id: &str,
) -> Result<String, String> {
    // Reasoning-capable routes can spend the first ~64 tokens entirely on
    // reasoning and return `finish_reason=length` with empty content. Keep the
    // title call small, but leave enough room for both reasoning and answer.
    model.max_output_tokens = model.max_output_tokens.clamp(512, 1024);

    let system = "\
You generate short chat titles from the user's first message.
Summarize the topic; do not copy the first words of the question.
Return only the title.
No quotes.
No markdown.
No trailing period.
Use the same language as the user.
Prefer a noun phrase, 2-5 words.
For URLs, omit the raw URL and use the site or product name when obvious.

Examples:
Авторизация для сервисов
Запуск модели на Fedora
Effort в веб-версии GPT
Политика данных CrofAI";

    let user = format!("Create a concise title for this new chat:\n\n{first_user_message}");

    let provider = model.provider.clone();
    let model_id = model.id.clone();
    let started_at = Instant::now();
    let response = llm
        .chat_with_tools_single_attempt_for_model_info(
            system,
            "",
            &[Message::user(&user)],
            &[],
            &model,
            None,
            false,
            title_reasoning_effort(&model),
        )
        .await
        .map_err(|e| e.to_string())?;

    let content = response.content.unwrap_or_default();
    let reasoning_chars = response
        .reasoning_content
        .as_deref()
        .map(str::chars)
        .map(Iterator::count)
        .unwrap_or(0);
    let tool_names = response
        .tool_calls
        .iter()
        .map(|tool_call| tool_call.function.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let usage_total_tokens = response.usage.as_ref().map(|usage| usage.total_tokens);
    info!(
        session_id = %session_id,
        provider = %provider,
        model = %model_id,
        elapsed_ms = started_at.elapsed().as_millis(),
        finish_reason = %response.finish_reason,
        content_chars = content.chars().count(),
        reasoning_chars,
        tool_calls = response.tool_calls.len(),
        tool_names = %tool_names,
        usage_total_tokens,
        "auto title LLM response received"
    );

    if response.finish_reason.eq_ignore_ascii_case("length") {
        return Err("auto title LLM response stopped because output limit was reached".to_string());
    }

    Ok(content)
}

fn sanitize_auto_title(raw: &str) -> String {
    let first_line = raw.lines().next().unwrap_or("").trim();

    let without_quotes = first_line.trim_matches([
        '"', '\'', '`', '\u{201c}', '\u{201d}', '\u{00ab}', '\u{00bb}',
    ]);

    let without_prefix = without_quotes
        .strip_prefix("Title:")
        .or_else(|| without_quotes.strip_prefix("title:"))
        .unwrap_or(without_quotes)
        .trim();

    let normalized = without_prefix
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    normalized.chars().take(MAX_SESSION_TITLE_CHARS).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_quotes_and_prefixes() {
        assert_eq!(
            sanitize_auto_title("\"Rust async patterns\""),
            "Rust async patterns"
        );
        assert_eq!(
            sanitize_auto_title("Title: Docker networking"),
            "Docker networking"
        );
        assert_eq!(sanitize_auto_title("title: kube setup"), "kube setup");
        assert_eq!(sanitize_auto_title("`my chat`"), "my chat");
        assert_eq!(
            sanitize_auto_title("\u{201c}hello world\u{201d}"),
            "hello world"
        );
    }

    #[test]
    fn sanitize_collapses_whitespace() {
        assert_eq!(
            sanitize_auto_title("  lots   of   spaces  "),
            "lots of spaces"
        );
    }

    #[test]
    fn sanitize_truncates_long_titles() {
        let long = "a".repeat(200);
        assert!(sanitize_auto_title(&long).len() <= MAX_SESSION_TITLE_CHARS);
    }

    #[test]
    fn sanitize_empty_input_returns_empty() {
        assert_eq!(sanitize_auto_title(""), "");
        assert_eq!(sanitize_auto_title("   "), "");
    }

    #[test]
    fn sanitize_handles_multiline_llm_response() {
        assert_eq!(
            sanitize_auto_title("My title\nThis is extra noise"),
            "My title"
        );
    }

    #[test]
    fn opencode_title_calls_disable_reasoning() {
        let model = ModelInfo {
            id: "deepseek-v4-flash".to_string(),
            max_output_tokens: 1000,
            context_window_tokens: 0,
            provider: "llm-provider/opencode-go".to_string(),
            weight: 1,
        };
        assert_eq!(title_reasoning_effort(&model), Some("none"));
    }

    #[test]
    fn non_opencode_title_calls_keep_low_reasoning() {
        let model = ModelInfo {
            id: "mistral-small-2603".to_string(),
            max_output_tokens: 1000,
            context_window_tokens: 0,
            provider: "mistral".to_string(),
            weight: 1,
        };
        assert_eq!(title_reasoning_effort(&model), Some("low"));
    }

    #[test]
    fn retry_delay_increases_after_repeated_failures() {
        assert_eq!(retry_delay(1), ChronoDuration::seconds(60));
        assert_eq!(retry_delay(2), ChronoDuration::seconds(5 * 60));
        assert_eq!(retry_delay(5), ChronoDuration::seconds(6 * 60 * 60));
    }
}
