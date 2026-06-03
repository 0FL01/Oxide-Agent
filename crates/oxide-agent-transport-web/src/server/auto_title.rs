//! Async LLM-powered session title generation.
//!
//! On the first task of a new session a background worker is spawned to
//! generate a short, meaningful title through the same LLM model that the
//! agent uses.  The HTTP response returns immediately with a
//! `markdown_preview()` fallback; the generated title is persisted only
//! if the user has not manually renamed the session.

use crate::server::types::{AppState, MAX_SESSION_TITLE_CHARS};
use chrono::Utc;
use oxide_agent_core::config::ModelInfo;
use oxide_agent_core::llm::Message;
use std::sync::Arc;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct AutoTitleRequest {
    pub(crate) user_id: i64,
    pub(crate) session_id: String,
    pub(crate) first_user_message: String,
    pub(crate) fallback_preview: String,
}

/// Fire-and-forget: spawn an async LLM title generation task.
pub(crate) fn spawn_auto_title(state: AppState, request: AutoTitleRequest) {
    tokio::spawn(async move {
        let session_id = request.session_id.clone();
        info!(session_id = %session_id, "auto title generation started");
        if let Err(error) = generate_and_save_auto_title(state, request).await {
            warn!(session_id = %session_id, error = %error, "auto title generation failed");
        } else {
            info!(session_id = %session_id, "auto title generation finished");
        }
    });
}

// ---------------------------------------------------------------------------
// Internal
// ---------------------------------------------------------------------------

async fn generate_and_save_auto_title(
    state: AppState,
    request: AutoTitleRequest,
) -> Result<(), String> {
    // Guard: session must still exist and be renameable.
    let session = state
        .web_store
        .load_session(request.user_id, &request.session_id)
        .await
        .map_err(|e| e.to_string())?;
    let Some(session) = session else {
        return Ok(());
    };

    if !session.may_auto_title(&request.fallback_preview) {
        info!(session_id = %request.session_id, "auto title skipped because session title is not replaceable");
        return Ok(());
    }

    let model = inherited_title_model(&state);
    let raw_title = generate_title(
        state.session_manager.llm_client(),
        model,
        &request.first_user_message,
    )
    .await?;

    let title = sanitize_auto_title(&raw_title);
    if title.is_empty() {
        info!(session_id = %request.session_id, "auto title skipped because generated title is empty");
        return Ok(());
    }

    // Double-check: reload session and verify the title is still replaceable.
    // This prevents overwriting a manual rename that happened while the LLM
    // call was in flight.
    let Some(mut session) = state
        .web_store
        .load_session(request.user_id, &request.session_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };

    if !session.may_auto_title(&request.fallback_preview) {
        info!(session_id = %request.session_id, "auto title skipped because session title changed during generation");
        return Ok(());
    }

    session.title = title;
    session.updated_at = Utc::now();

    state
        .web_store
        .save_session(session)
        .await
        .map_err(|e| e.to_string())?;

    info!(session_id = %request.session_id, "auto title saved");

    Ok(())
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

async fn generate_title(
    llm: Arc<oxide_agent_core::llm::LlmClient>,
    mut model: ModelInfo,
    first_user_message: &str,
) -> Result<String, String> {
    model.max_output_tokens = model.max_output_tokens.clamp(16, 64);

    let system = "\
You generate short chat titles.
Return only the title.
No quotes.
No markdown.
Use the same language as the user.
Keep it under 6 words.";

    let user = format!("Create a concise title for this new chat:\n\n{first_user_message}");

    let response = llm
        .chat_with_tools_single_attempt_for_model_info(
            system,
            "",
            &[Message::user(&user)],
            &[],
            &model,
            None,
            false,
            None,
        )
        .await
        .map_err(|e| e.to_string())?;

    Ok(response.content.unwrap_or_default())
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
}
