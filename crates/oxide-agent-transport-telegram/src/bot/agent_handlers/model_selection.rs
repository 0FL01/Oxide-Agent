use crate::bot::resilient;
use crate::bot::thread::OutboundThreadParams;
use crate::config::BotSettings;
use anyhow::Result;
use oxide_agent_core::config::{AgentSettings, ModelInfo};
use oxide_agent_core::storage::StorageProvider;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageId};
use tracing::warn;

const MODELS_PER_PAGE: usize = 7;
const MODEL_CALLBACK_PREFIX: &str = "m:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModelCallbackAction {
    Open,
    Page { owner_id: i64, page: usize },
    Select { owner_id: i64, token: String },
    Default { owner_id: i64 },
    Close { owner_id: i64 },
}

pub(crate) struct ModelCallbackContext<'a> {
    pub(crate) bot: &'a Bot,
    pub(crate) chat_id: ChatId,
    pub(crate) message_id: MessageId,
    pub(crate) outbound_thread: OutboundThreadParams,
    pub(crate) user_id: i64,
    pub(crate) context_key: &'a str,
    pub(crate) storage: &'a Arc<dyn StorageProvider>,
    pub(crate) settings: &'a BotSettings,
}

impl ModelCallbackAction {
    pub(crate) const fn owner_id(&self) -> Option<i64> {
        match self {
            Self::Open => None,
            Self::Page { owner_id, .. }
            | Self::Select { owner_id, .. }
            | Self::Default { owner_id }
            | Self::Close { owner_id } => Some(*owner_id),
        }
    }
}

#[must_use]
pub(crate) fn parse_model_callback_action(data: &str) -> Option<ModelCallbackAction> {
    if data == crate::bot::views::AGENT_CALLBACK_MODEL_OPEN {
        return Some(ModelCallbackAction::Open);
    }
    let mut parts = data.strip_prefix(MODEL_CALLBACK_PREFIX)?.split(':');
    let action = parts.next()?;
    let owner_id = parts.next()?.parse().ok()?;
    let parsed = match action {
        "p" => ModelCallbackAction::Page {
            owner_id,
            page: parts.next()?.parse().ok()?,
        },
        "s" => {
            let token = parts.next()?;
            if token.len() != 24 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            ModelCallbackAction::Select {
                owner_id,
                token: token.to_ascii_lowercase(),
            }
        }
        "d" => ModelCallbackAction::Default { owner_id },
        "x" => ModelCallbackAction::Close { owner_id },
        _ => return None,
    };
    parts.next().is_none().then_some(parsed)
}

#[must_use]
pub(crate) fn qualified_model_id(model: &ModelInfo) -> String {
    format!("{}/{}", model.provider, model.id)
}

fn configured_models(settings: &AgentSettings) -> Vec<ModelInfo> {
    let mut seen = HashSet::new();
    settings
        .get_configured_agent_model_routes()
        .into_iter()
        .filter(|model| seen.insert(qualified_model_id(model)))
        .collect()
}

fn model_token(qualified_id: &str) -> String {
    let digest = Sha256::digest(qualified_id.as_bytes());
    digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn compact_button_label(qualified_id: &str, current: bool) -> String {
    const MAX_CHARS: usize = 54;
    let prefix = if current { "✅ " } else { "" };
    let available = MAX_CHARS.saturating_sub(prefix.chars().count());
    let mut value = qualified_id.chars().take(available).collect::<String>();
    if qualified_id.chars().count() > available {
        value.pop();
        value.push('…');
    }
    format!("{prefix}{value}")
}

fn selector_view(
    settings: &AgentSettings,
    selected_id: Option<&str>,
    owner_id: i64,
    requested_page: usize,
) -> (String, InlineKeyboardMarkup, usize) {
    let models = configured_models(settings);
    let page_count = models.len().div_ceil(MODELS_PER_PAGE).max(1);
    let page = requested_page.min(page_count - 1);
    let selection_available = selected_id.is_some_and(|selected| {
        models
            .iter()
            .any(|model| qualified_model_id(model) == selected)
    });
    let current_label = match selected_id {
        Some(selected) if selection_available => selected.to_string(),
        Some(selected) => format!("{selected} (unavailable; using Default)"),
        None => "Default route pool".to_string(),
    };
    let mut rows = models
        .iter()
        .skip(page * MODELS_PER_PAGE)
        .take(MODELS_PER_PAGE)
        .map(|model| {
            let qualified_id = qualified_model_id(model);
            vec![InlineKeyboardButton::callback(
                compact_button_label(
                    &qualified_id,
                    selected_id.is_some_and(|selected| selected == qualified_id),
                ),
                format!("m:s:{owner_id}:{}", model_token(&qualified_id)),
            )]
        })
        .collect::<Vec<_>>();

    if page_count > 1 {
        let mut pagination = Vec::new();
        if page > 0 {
            pagination.push(InlineKeyboardButton::callback(
                "⬅️",
                format!("m:p:{owner_id}:{}", page - 1),
            ));
        }
        pagination.push(InlineKeyboardButton::callback(
            format!("{}/{page_count}", page + 1),
            format!("m:p:{owner_id}:{page}"),
        ));
        if page + 1 < page_count {
            pagination.push(InlineKeyboardButton::callback(
                "➡️",
                format!("m:p:{owner_id}:{}", page + 1),
            ));
        }
        rows.push(pagination);
    }

    rows.push(vec![
        InlineKeyboardButton::callback(
            if selected_id.is_none() || !selection_available {
                "✅ Default"
            } else {
                "Default"
            },
            format!("m:d:{owner_id}"),
        ),
        InlineKeyboardButton::callback("Close", format!("m:x:{owner_id}")),
    ]);

    (
        format!(
            "🧠 Agent model\nCurrent: {current_label}\n\nChanges apply to the next Agent execution."
        ),
        InlineKeyboardMarkup::new(rows),
        page,
    )
}

async fn selected_id(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: &str,
) -> Result<Option<String>> {
    Ok(storage
        .get_context_agent_model_selection(user_id, context_key)
        .await?)
}

pub(crate) async fn current_model_label(
    storage: &Arc<dyn StorageProvider>,
    settings: &AgentSettings,
    user_id: i64,
    context_key: &str,
) -> String {
    match selected_id(storage, user_id, context_key).await {
        Ok(Some(selected)) => {
            if configured_models(settings)
                .iter()
                .any(|model| qualified_model_id(model) == selected)
            {
                selected
            } else {
                format!("Default (saved {selected} unavailable)")
            }
        }
        Ok(None) => format!(
            "Default ({})",
            qualified_model_id(&settings.get_configured_agent_model())
        ),
        Err(error) => {
            warn!(%error, "Failed to load Telegram model selection");
            format!(
                "Default ({})",
                qualified_model_id(&settings.get_configured_agent_model())
            )
        }
    }
}

pub(crate) async fn selected_model(
    storage: &Arc<dyn StorageProvider>,
    settings: &AgentSettings,
    user_id: i64,
    context_key: &str,
) -> Option<ModelInfo> {
    let selected = match selected_id(storage, user_id, context_key).await {
        Ok(selected) => selected,
        Err(error) => {
            warn!(%error, "Failed to load Telegram model selection; using default model");
            return None;
        }
    };
    let selected = selected?;
    configured_models(settings)
        .into_iter()
        .find(|model| qualified_model_id(model) == selected)
}

pub(crate) async fn show_model_selector(
    bot: &Bot,
    chat_id: ChatId,
    outbound_thread: OutboundThreadParams,
    user_id: i64,
    context_key: &str,
    storage: &Arc<dyn StorageProvider>,
    settings: &BotSettings,
) -> Result<()> {
    let selected = selected_id(storage, user_id, context_key).await?;
    let (text, markup, _) = selector_view(&settings.agent, selected.as_deref(), user_id, 0);
    let mut request = bot.send_message(chat_id, text).reply_markup(markup);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        request = request.message_thread_id(thread_id);
    }
    request.await?;
    Ok(())
}

pub(crate) async fn handle_model_callback(
    action: ModelCallbackAction,
    ctx: ModelCallbackContext<'_>,
) -> Result<()> {
    let ModelCallbackContext {
        bot,
        chat_id,
        message_id,
        outbound_thread,
        user_id,
        context_key,
        storage,
        settings,
    } = ctx;
    if matches!(action, ModelCallbackAction::Open) {
        return show_model_selector(
            bot,
            chat_id,
            outbound_thread,
            user_id,
            context_key,
            storage,
            settings,
        )
        .await;
    }

    if matches!(action, ModelCallbackAction::Close { .. }) {
        resilient::edit_message_resilient_with_markup(
            bot,
            chat_id,
            message_id,
            "🧠 Model selector closed.",
            None,
            Some(crate::bot::views::empty_inline_keyboard()),
        )
        .await?;
        return Ok(());
    }

    let mut page = match action {
        ModelCallbackAction::Page { page, .. } => page,
        _ => 0,
    };
    match action {
        ModelCallbackAction::Select { token, .. } => {
            let matches = configured_models(&settings.agent)
                .into_iter()
                .enumerate()
                .filter(|(_, model)| model_token(&qualified_model_id(model)) == token)
                .collect::<Vec<_>>();
            if let [(index, model)] = matches.as_slice() {
                let qualified_id = qualified_model_id(model);
                storage
                    .set_context_agent_model_selection(user_id, context_key, Some(qualified_id))
                    .await?;
                page = index / MODELS_PER_PAGE;
            } else {
                anyhow::bail!("Selected model is no longer available");
            }
        }
        ModelCallbackAction::Default { .. } => {
            storage
                .set_context_agent_model_selection(user_id, context_key, None)
                .await?;
        }
        ModelCallbackAction::Page { .. }
        | ModelCallbackAction::Open
        | ModelCallbackAction::Close { .. } => {}
    }

    let selected = selected_id(storage, user_id, context_key).await?;
    let (text, markup, _) = selector_view(&settings.agent, selected.as_deref(), user_id, page);
    resilient::edit_message_resilient_with_markup(
        bot,
        chat_id,
        message_id,
        text,
        None,
        Some(markup),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with_routes(count: usize) -> AgentSettings {
        AgentSettings {
            agent_model_routes: Some(
                (0..count)
                    .map(|index| ModelInfo {
                        id: format!("model-{index}"),
                        provider: "test".to_string(),
                        max_output_tokens: 1_000,
                        context_window_tokens: 8_000,
                    })
                    .collect(),
            ),
            ..AgentSettings::default()
        }
    }

    #[test]
    fn selector_paginates_and_keeps_callbacks_compact() {
        let settings = settings_with_routes(8);
        let (_, first, _) = selector_view(&settings, Some("test/model-7"), 42, 0);
        let (_, second, _) = selector_view(&settings, Some("test/model-7"), 42, 1);
        assert_eq!(
            first
                .inline_keyboard
                .iter()
                .flatten()
                .filter(|button| button.text.contains("model-"))
                .count(),
            7
        );
        assert_eq!(
            second
                .inline_keyboard
                .iter()
                .flatten()
                .filter(|button| button.text.contains("model-"))
                .count(),
            1
        );
        for button in first
            .inline_keyboard
            .iter()
            .chain(&second.inline_keyboard)
            .flatten()
        {
            if let teloxide::types::InlineKeyboardButtonKind::CallbackData(data) = &button.kind {
                assert!(data.len() <= 64);
            }
        }
    }

    #[test]
    fn model_callbacks_are_typed_and_owner_bound() {
        assert_eq!(
            parse_model_callback_action("m:s:42:0123456789abcdef01234567"),
            Some(ModelCallbackAction::Select {
                owner_id: 42,
                token: "0123456789abcdef01234567".to_string(),
            })
        );
        assert_eq!(parse_model_callback_action("m:s:42:abc"), None);
        assert_eq!(parse_model_callback_action("m:p:42:nope"), None);
    }
}
