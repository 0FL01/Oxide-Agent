use crate::bot::resilient;
use crate::bot::thread::OutboundThreadParams;
use crate::config::BotSettings;
use anyhow::{Result, anyhow};
use oxide_agent_core::config::{AgentSettings, ModelInfo};
use oxide_agent_core::llm::{
    DiscoveredLlmModel, LlmClient, canonical_model_id, resolve_model_selection,
};
use oxide_agent_core::storage::StorageProvider;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageId};
use tokio::time::timeout;
use tracing::warn;

const MODELS_PER_PAGE: usize = 7;
const MODEL_CALLBACK_PREFIX: &str = "m:";
const MODEL_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

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
    pub(crate) llm: &'a Arc<LlmClient>,
    pub(crate) settings: &'a BotSettings,
}

pub(crate) struct ShowModelSelectorContext<'a> {
    pub(crate) bot: &'a Bot,
    pub(crate) chat_id: ChatId,
    pub(crate) outbound_thread: OutboundThreadParams,
    pub(crate) user_id: i64,
    pub(crate) context_key: &'a str,
    pub(crate) storage: &'a Arc<dyn StorageProvider>,
    pub(crate) llm: &'a Arc<LlmClient>,
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

fn configured_models(settings: &AgentSettings) -> Vec<ModelInfo> {
    let mut seen = HashSet::new();
    settings
        .get_configured_agent_model_routes()
        .into_iter()
        .filter(|model| {
            canonical_model_id(model).is_some_and(|qualified_id| seen.insert(qualified_id))
        })
        .collect()
}

fn merge_selectable_model_ids(
    configured: &[ModelInfo],
    discovered: Vec<DiscoveredLlmModel>,
    provider_available: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut model_ids = configured
        .iter()
        .filter_map(canonical_model_id)
        .filter(|qualified_id| seen.insert(qualified_id.clone()))
        .collect::<Vec<_>>();

    for model in discovered {
        if !provider_available(&model.provider_id)
            || !matches!(
                model.protocol.as_str(),
                "openai_chat_completions" | "anthropic_messages"
            )
            || resolve_model_selection(&model.qualified_id, configured).is_none()
            || !seen.insert(model.qualified_id.clone())
        {
            continue;
        }
        model_ids.push(model.qualified_id);
    }

    model_ids
}

async fn selectable_model_ids(settings: &AgentSettings, llm: &LlmClient) -> Vec<String> {
    let configured = configured_models(settings);
    let discovered = match timeout(MODEL_DISCOVERY_TIMEOUT, llm.discovered_models()).await {
        Ok(models) => models,
        Err(_) => {
            warn!("Telegram model discovery timed out; using configured routes");
            Vec::new()
        }
    };
    merge_selectable_model_ids(&configured, discovered, |provider| {
        llm.is_provider_available(provider)
    })
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
    models: &[String],
    selected_id: Option<&str>,
    owner_id: i64,
    requested_page: usize,
    notice: Option<&str>,
) -> (String, InlineKeyboardMarkup, usize) {
    let page_count = models.len().div_ceil(MODELS_PER_PAGE).max(1);
    let page = requested_page.min(page_count - 1);
    let configured = configured_models(settings);
    let selection_valid = selected_id
        .and_then(|selected| resolve_model_selection(selected, &configured))
        .is_some();
    let current_label = match selected_id {
        Some(selected) if selection_valid => selected.to_string(),
        Some(selected) => format!("{selected} (invalid; execution blocked)"),
        None => configured.first().and_then(canonical_model_id).map_or_else(
            || "Default".to_string(),
            |model| format!("Default ({model})"),
        ),
    };
    let mut rows = models
        .iter()
        .skip(page * MODELS_PER_PAGE)
        .take(MODELS_PER_PAGE)
        .map(|qualified_id| {
            vec![InlineKeyboardButton::callback(
                compact_button_label(
                    qualified_id,
                    selected_id.is_some_and(|selected| selected == qualified_id.as_str()),
                ),
                format!("m:s:{owner_id}:{}", model_token(qualified_id)),
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
            if selected_id.is_none() {
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
            "🧠 Agent model\nCurrent: {current_label}{}\n\nChanges apply to the next Agent execution.",
            notice.map_or_else(String::new, |notice| format!("\n{notice}"))
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
    let configured = configured_models(settings);
    match selected_id(storage, user_id, context_key).await {
        Ok(Some(selected)) if resolve_model_selection(&selected, &configured).is_some() => selected,
        Ok(Some(selected)) => format!("Invalid saved model ({selected})"),
        Ok(None) => canonical_model_id(&settings.get_configured_agent_model()).map_or_else(
            || "Default".to_string(),
            |model| format!("Default ({model})"),
        ),
        Err(error) => {
            warn!(%error, "Failed to load Telegram model selection");
            "Model selection unavailable".to_string()
        }
    }
}

pub(crate) async fn selected_model(
    storage: &Arc<dyn StorageProvider>,
    settings: &AgentSettings,
    user_id: i64,
    context_key: &str,
) -> Result<Option<ModelInfo>> {
    let Some(selected) = selected_id(storage, user_id, context_key).await? else {
        return Ok(None);
    };
    let configured = configured_models(settings);
    resolve_model_selection(&selected, &configured)
        .map(Some)
        .ok_or_else(|| anyhow!("Saved Agent model selection '{selected}' is invalid"))
}

pub(crate) async fn show_model_selector(ctx: ShowModelSelectorContext<'_>) -> Result<()> {
    let ShowModelSelectorContext {
        bot,
        chat_id,
        outbound_thread,
        user_id,
        context_key,
        storage,
        llm,
        settings,
    } = ctx;
    let selected = selected_id(storage, user_id, context_key).await?;
    let models = selectable_model_ids(&settings.agent, llm).await;
    let (text, markup, _) = selector_view(
        &settings.agent,
        &models,
        selected.as_deref(),
        user_id,
        0,
        None,
    );
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
        llm,
        settings,
    } = ctx;
    if matches!(action, ModelCallbackAction::Open) {
        return show_model_selector(ShowModelSelectorContext {
            bot,
            chat_id,
            outbound_thread,
            user_id,
            context_key,
            storage,
            llm,
            settings,
        })
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

    let models = selectable_model_ids(&settings.agent, llm).await;
    let mut notice = None;
    let mut page = match action {
        ModelCallbackAction::Page { page, .. } => page,
        _ => 0,
    };
    match action {
        ModelCallbackAction::Select { token, .. } => {
            let matches = models
                .iter()
                .enumerate()
                .filter(|(_, qualified_id)| model_token(qualified_id) == token)
                .collect::<Vec<_>>();
            if let [(index, qualified_id)] = matches.as_slice() {
                storage
                    .set_context_agent_model_selection(
                        user_id,
                        context_key,
                        Some((*qualified_id).clone()),
                    )
                    .await?;
                page = index / MODELS_PER_PAGE;
            } else {
                notice = Some("The selected model is no longer available.");
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
    let (text, markup, _) = selector_view(
        &settings.agent,
        &models,
        selected.as_deref(),
        user_id,
        page,
        notice,
    );
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
    use chrono::Utc;

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

    fn discovered_model(provider_id: &str, model_id: &str, protocol: &str) -> DiscoveredLlmModel {
        DiscoveredLlmModel {
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
            qualified_id: format!("{provider_id}/{model_id}"),
            display_name: model_id.to_string(),
            protocol: protocol.to_string(),
            supports_image_input: false,
            source: "cache".to_string(),
            fetched_at: Utc::now(),
        }
    }

    #[test]
    fn discovered_models_are_filtered_and_deduplicated_against_configured_routes() {
        let configured = vec![ModelInfo {
            id: "opencode-go/mimo-v2.5".to_string(),
            provider: "llm-provider/opencode-go".to_string(),
            max_output_tokens: 1_000,
            context_window_tokens: 8_000,
        }];
        let discovered = vec![
            discovered_model("opencode-go", "mimo-v2.5", "openai_chat_completions"),
            discovered_model("opencode-go", "kimi-k2.6", "anthropic_messages"),
            discovered_model("opencode-go", "unknown", "unknown"),
            discovered_model("opencode-zen", "free", "openai_chat_completions"),
        ];

        let models = merge_selectable_model_ids(&configured, discovered, |provider| {
            provider == "opencode-go"
        });

        assert_eq!(models, ["opencode-go/mimo-v2.5", "opencode-go/kimi-k2.6"]);
    }

    #[test]
    fn selector_paginates_and_keeps_callbacks_compact() {
        let settings = settings_with_routes(8);
        let models = configured_models(&settings)
            .iter()
            .filter_map(canonical_model_id)
            .collect::<Vec<_>>();
        let (_, first, _) = selector_view(&settings, &models, Some("test/model-7"), 42, 0, None);
        let (_, second, _) = selector_view(&settings, &models, Some("test/model-7"), 42, 1, None);
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
