use crate::bot::state::State;
use crate::config::{Settings, DEFAULT_MODEL, MODELS};
use crate::llm::{LlmClient, Message as LlmMessage};
use crate::storage::R2Storage;
use crate::utils;
use anyhow::{anyhow, Result};
use std::sync::Arc;
use teloxide::{
    net::Download,
    prelude::*,
    types::{KeyboardButton, KeyboardMarkup, ParseMode},
    utils::command::BotCommands,
};
use tracing::{error, info, warn};

// Helper function to get user name from Message
fn get_user_name(msg: &Message) -> String {
    if let Some(ref user) = msg.from {
        if let Some(ref username) = user.username {
            return username.clone();
        }
        // first_name is String, not Option<String>
        if !user.first_name.is_empty() {
            return user.first_name.clone();
        }
    }
    "Unknown".to_string()
}

/// Safely truncates a string to a maximum character length (not bytes).
/// This is UTF-8 safe and will not panic on multi-byte characters.
fn truncate_str(s: impl AsRef<str>, max_chars: usize) -> String {
    let s = s.as_ref();
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    // Find the byte position of the max_chars-th character
    s.char_indices()
        .nth(max_chars)
        .map_or(s.to_string(), |(pos, _)| s[..pos].to_string())
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Поддерживаемые команды:")]
pub enum Command {
    #[command(description = "Начать работу.")]
    Start,
    #[command(description = "Очистить историю чата.")]
    Clear,
    #[command(description = "Проверка работоспособности.")]
    Healthcheck,
}

pub fn get_main_keyboard() -> KeyboardMarkup {
    let keyboard = vec![
        vec![
            KeyboardButton::new("Очистить контекст"),
            KeyboardButton::new("Сменить модель"),
        ],
        vec![
            KeyboardButton::new("🤖 Режим Агента"),
            KeyboardButton::new("Доп функции"),
        ],
    ];
    KeyboardMarkup::new(keyboard).resize_keyboard()
}

pub fn get_extra_functions_keyboard() -> KeyboardMarkup {
    let keyboard = vec![vec![
        KeyboardButton::new("Изменить промпт"),
        KeyboardButton::new("Назад"),
    ]];
    KeyboardMarkup::new(keyboard).resize_keyboard()
}

pub fn get_model_keyboard() -> KeyboardMarkup {
    let mut keyboard = Vec::new();
    for model_name in MODELS.iter().map(|(n, _)| n) {
        keyboard.push(vec![KeyboardButton::new(model_name.to_string())]);
    }
    keyboard.push(vec![KeyboardButton::new("Назад")]);
    KeyboardMarkup::new(keyboard).resize_keyboard()
}

pub async fn start(bot: Bot, msg: Message, storage: Arc<R2Storage>) -> Result<()> {
    let user_id = msg.from.as_ref().unwrap().id.0 as i64;
    let user_name = get_user_name(&msg);

    info!("User {} ({}) initiated /start command.", user_id, user_name);

    let saved_model = storage.get_user_model(user_id).await.unwrap_or(None);
    let model = saved_model.unwrap_or_else(|| DEFAULT_MODEL.to_string());
    info!(
        "User {} ({}) is allowed. Set model to {}",
        user_id, user_name, model
    );

    let text = format!(
        "<b>Привет!</b> Я бот, который может отвечать на вопросы и распознавать речь.\nТекущая модель: <b>{}</b>",
        model
    );

    info!("Sending welcome message to user {}.", user_id);
    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .reply_markup(get_main_keyboard())
        .await?;

    Ok(())
}

pub async fn clear(bot: Bot, msg: Message, storage: Arc<R2Storage>) -> Result<()> {
    let user_id = msg.from.as_ref().unwrap().id.0 as i64;
    let user_name = get_user_name(&msg);

    info!("User {} ({}) initiated context clear.", user_id, user_name);

    match storage.clear_chat_history(user_id).await {
        Ok(_) => {
            info!("Chat history successfully cleared for user {}.", user_id);
            bot.send_message(msg.chat.id, "<b>История чата очищена.</b>")
                .parse_mode(ParseMode::Html)
                .reply_markup(get_main_keyboard())
                .await?;
        }
        Err(e) => {
            error!("Error clearing chat history for user {}: {}", user_id, e);
            bot.send_message(msg.chat.id, "Произошла ошибка при очистке истории чата.")
                .await?;
        }
    }

    Ok(())
}

pub async fn healthcheck(bot: Bot, msg: Message) -> Result<()> {
    let user_id = msg.from.as_ref().map(|u| u.id.0).unwrap_or(0);
    info!("Healthcheck command received from user {}.", user_id);
    bot.send_message(msg.chat.id, "OK").await?;
    info!("Responded 'OK' to healthcheck from user {}.", user_id);
    Ok(())
}

pub async fn handle_text(
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
    llm: Arc<LlmClient>,
    dialogue: Dialogue<State, teloxide::dispatching::dialogue::InMemStorage<State>>,
    settings: Arc<Settings>,
) -> Result<()> {
    let text = msg.text().unwrap_or("");
    let user_id = msg.from.as_ref().unwrap().id.0 as i64;
    let user_name = get_user_name(&msg);

    let photo = msg.photo().is_some();
    info!(
        "Handling message from user {} ({}). Text: '{}{}'. Photo attached: {}",
        user_id,
        user_name,
        truncate_str(text, 100),
        if text.chars().count() > 100 {
            "..."
        } else {
            ""
        },
        photo
    );

    match text {
        "Очистить контекст" => {
            info!("User {} clicked 'Очистить контекст'.", user_id);
            return clear(bot, msg, storage).await;
        }
        "Сменить модель" => {
            info!("User {} clicked 'Сменить модель'.", user_id);
            info!("Showing model selection keyboard to user {}.", user_id);
            bot.send_message(msg.chat.id, "Выберите модель:")
                .reply_markup(get_model_keyboard())
                .await?;
            return Ok(());
        }
        "Доп функции" => {
            info!("User {} clicked 'Доп функции'.", user_id);
            bot.send_message(msg.chat.id, "Выберите действие:")
                .reply_markup(get_extra_functions_keyboard())
                .await?;
            return Ok(());
        }
        "🤖 Режим Агента" => {
            info!(
                "User {} clicked 'Режим Агента', checking permissions.",
                user_id
            );

            // Check authorization
            let agent_allowed = settings.agent_allowed_users();
            if !agent_allowed.contains(&user_id) && !agent_allowed.is_empty() {
                warn!("User {} denied access to Agent Mode.", user_id);
                bot.send_message(
                    msg.chat.id,
                    "⛔️ У вас нет прав для доступа к режиму агента.",
                )
                .await?;
                return Ok(());
            } else if agent_allowed.is_empty() {
                warn!(
                    "Agent Mode access denied for user {} (AGENT_ACCESS_IDS not configured).",
                    user_id
                );
                bot.send_message(
                    msg.chat.id,
                    "⛔️ Режим агента временно недоступен (не настроен доступ).",
                )
                .await?;
                return Ok(());
            }

            info!("User {} authorized for Agent Mode. Activating.", user_id);
            return crate::bot::agent_handlers::activate_agent_mode(bot, msg, dialogue, llm).await;
        }
        "Изменить промпт" => {
            info!(
                "User {} clicked 'Изменить промпт', entering editing mode.",
                user_id
            );
            dialogue
                .update(State::EditingPrompt)
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
            bot.send_message(
                msg.chat.id,
                "Введите новый системный промпт. Для отмены введите 'Назад':",
            )
            .reply_markup(get_extra_functions_keyboard())
            .await?;
            return Ok(());
        }
        "Назад" => {
            info!(
                "User {} clicked 'Назад' from extra functions menu.",
                user_id
            );
            bot.send_message(msg.chat.id, "Выберите действие: (Или начните диалог)")
                .reply_markup(get_main_keyboard())
                .await?;
            return Ok(());
        }
        "⬅️ Выйти из режима агента" => {
            info!(
                "User {} clicked 'Exit Agent Mode' from global handler.",
                user_id
            );
            // Even if we are not in agent mode, we should confirm exit and show main keyboard
            bot.send_message(msg.chat.id, "👋 Вышли из режима агента")
                .reply_markup(get_main_keyboard())
                .await?;
            return Ok(());
        }
        "❌ Отменить задачу" => {
            info!(
                "User {} clicked 'Cancel Task' from global handler.",
                user_id
            );
            bot.send_message(msg.chat.id, "Нет активной задачи для отмены.")
                .reply_markup(get_main_keyboard())
                .await?;
            return Ok(());
        }
        "🗑 Очистить память" => {
            info!(
                "User {} clicked 'Clear Memory' from global handler.",
                user_id
            );
            bot.send_message(msg.chat.id, "Память агента не активна.")
                .reply_markup(get_main_keyboard())
                .await?;
            return Ok(());
        }
        _ => {}
    }

    // Check if it's a model selection
    if MODELS.iter().any(|(name, _)| *name == text) {
        info!("User {} selected model '{}' via text input.", user_id, text);
        storage.update_user_model(user_id, text.to_string()).await?;
        info!("Model changed to '{}' for user {}.", text, user_id);
        bot.send_message(msg.chat.id, format!("Модель изменена на <b>{}</b>", text))
            .parse_mode(ParseMode::Html)
            .reply_markup(get_main_keyboard())
            .await?;
        return Ok(());
    }

    // Process regular message
    info!("Processing regular text message from user {}.", user_id);
    let text_to_process = text.to_string();
    process_llm_request(bot, msg, storage, llm, text_to_process).await
}

pub async fn handle_editing_prompt(
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
    dialogue: Dialogue<State, teloxide::dispatching::dialogue::InMemStorage<State>>,
) -> Result<()> {
    let text = msg.text().unwrap_or("");
    let user_id = msg.from.as_ref().unwrap().id.0 as i64;

    if text == "Назад" {
        info!("User {} cancelled prompt editing.", user_id);
        dialogue.exit().await.map_err(|e| anyhow!(e.to_string()))?;
        bot.send_message(msg.chat.id, "Отмена обновления системного промпта.")
            .reply_markup(get_main_keyboard())
            .await?;
    } else {
        match storage.update_user_prompt(user_id, text.to_string()).await {
            Ok(_) => {
                info!("System prompt updated for user {}.", user_id);
                dialogue.exit().await.map_err(|e| anyhow!(e.to_string()))?;
                bot.send_message(msg.chat.id, "Системный промпт обновлен.")
                    .reply_markup(get_main_keyboard())
                    .await?;
            }
            Err(e) => {
                error!("Error updating system prompt for user {}: {}", user_id, e);
                bot.send_message(
                    msg.chat.id,
                    "Произошла ошибка при обновлении системного промпта.",
                )
                .reply_markup(get_extra_functions_keyboard())
                .await?;
            }
        }
    }
    Ok(())
}

async fn process_llm_request(
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
    llm: Arc<LlmClient>,
    text: String,
) -> Result<()> {
    let user_id = msg.from.as_ref().unwrap().id.0 as i64;
    let user_name = get_user_name(&msg);

    info!(
        "Starting message processing for user {} ({}). Message snippet: '{}{}'",
        user_id,
        user_name,
        truncate_str(&text, 100),
        if text.chars().count() > 100 {
            "..."
        } else {
            ""
        }
    );

    // Get state
    let system_prompt = storage
        .get_user_prompt(user_id)
        .await?
        .unwrap_or_else(|| std::env::var("SYSTEM_MESSAGE").unwrap_or_default());
    let history = storage.get_chat_history(user_id, 10).await?;
    let model = storage
        .get_user_model(user_id)
        .await?
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    info!(
        "Using system message for user {}: '{}' (truncated)",
        user_id,
        truncate_str(&system_prompt, 100)
    );
    info!(
        "Retrieved {} messages from history for user {}.",
        history.len(),
        user_id
    );

    // Get provider from model name
    let provider_info = MODELS
        .iter()
        .find(|(name, _)| name == &model)
        .map(|(_, info)| info);
    let provider_name = provider_info.map(|p| p.provider).unwrap_or("unknown");
    info!(
        "Selected model for user {}: {} (Provider: {})",
        user_id, model, provider_name
    );

    // Pre-save message to history
    info!(
        "Saving user message for user {} ({}): '{}' (truncated)",
        user_id,
        user_name,
        truncate_str(&text, 100)
    );
    storage
        .save_message(user_id, "user".to_string(), text.clone())
        .await?;

    // Show typing
    info!(
        "Sending typing action to chat {} for user {}.",
        msg.chat.id, user_id
    );
    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
        .await?;

    // Prepare messages
    let llm_history: Vec<LlmMessage> = history
        .into_iter()
        .map(|m| LlmMessage {
            role: m.role,
            content: m.content,
            tool_call_id: None,
            name: None,
            tool_calls: None,
        })
        .collect();
    let total_messages = llm_history.len() + 2; // +2 for system and current message
    info!(
        "Prepared {} messages for API call for user {}.",
        total_messages, user_id
    );
    info!(
        "Making API call to {} with model {} for user {}.",
        provider_name, model, user_id
    );

    // Call LLM
    match llm
        .chat_completion(&system_prompt, &llm_history, &text, &model)
        .await
    {
        Ok(response) => {
            info!(
                "Received response from {} for user {}.",
                provider_name, user_id
            );
            storage
                .save_message(user_id, "assistant".to_string(), response.clone())
                .await?;
            info!(
                "Saving assistant response for user {}. Snippet: '{}' (truncated)",
                user_id,
                truncate_str(&response, 100)
            );

            info!("Formatting response for Telegram for user {}.", user_id);
            let formatted = utils::format_text(&response);
            info!(
                "Splitting response into chunks if necessary for user {}.",
                user_id
            );
            let parts = utils::split_long_message(&formatted, 4000);
            info!(
                "Sending response in {} part(s) to user {}.",
                parts.len(),
                user_id
            );

            for (i, part) in parts.iter().enumerate() {
                info!(
                    "Sending part {}/{} to user {}.",
                    i + 1,
                    parts.len(),
                    user_id
                );
                match bot
                    .send_message(msg.chat.id, part)
                    .parse_mode(ParseMode::Html)
                    .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        error!(
                            "Error sending part {}/{} to user {}: {}",
                            i + 1,
                            parts.len(),
                            user_id,
                            e
                        );
                    }
                }
            }
        }
        Err(e) => {
            error!("Error processing message for user {}: {}", user_id, e);
            bot.send_message(msg.chat.id, format!("<b>Ошибка:</b> {}", e))
                .parse_mode(ParseMode::Html)
                .await?;
        }
    }

    Ok(())
}

pub async fn handle_voice(
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
    llm: Arc<LlmClient>,
) -> Result<()> {
    let user_id = msg.from.as_ref().unwrap().id.0 as i64;
    let user_name = get_user_name(&msg);

    info!(
        "Received voice message from user {} ({}).",
        user_id, user_name
    );

    let voice = msg.voice().ok_or_else(|| anyhow!("No voice found"))?;

    // Determine provider
    let model = storage
        .get_user_model(user_id)
        .await?
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let provider_info = MODELS
        .iter()
        .find(|(name, _)| name == &model)
        .map(|(_, info)| info);
    let provider_name = provider_info.map(|p| p.provider).unwrap_or("unknown");
    info!(
        "Using provider '{}' for voice processing (model: {})",
        provider_name, model
    );

    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
        .await?;

    let file = bot.get_file(voice.file.id.clone()).await?;
    let mut buffer = Vec::new();
    bot.download_file(&file.path, &mut buffer).await?;
    info!("Voice message downloaded. Size: {} bytes.", buffer.len());

    let model_id = provider_info.map(|p| p.id).unwrap_or("unknown");
    match llm
        .transcribe_audio_with_fallback(provider_name, buffer, "audio/wav", model_id)
        .await
    {
        Ok(text) => {
            if text.starts_with("(Gemini):") || text.starts_with("(OpenRouter):") {
                warn!(
                    "Transcription service returned a notice for user {}: {}",
                    user_id, text
                );
                bot.send_message(msg.chat.id, format!("Не удалось распознать речь: {}", text))
                    .await?;
            } else if text.is_empty() {
                warn!("Transcription result is empty for user {}.", user_id);
                bot.send_message(
                    msg.chat.id,
                    "Не удалось распознать речь (пустой результат).",
                )
                .await?;
            } else {
                info!(
                    "Voice message from user {} ({}) transcribed: '{}'",
                    user_id, user_name, text
                );
                info!("Processing transcribed text for user {}.", user_id);
                bot.send_message(
                    msg.chat.id,
                    format!("Распознано: \"{}\"\n\nОбрабатываю запрос...", text),
                )
                .await?;
                process_llm_request(bot, msg, storage, llm, text).await?;
            }
        }
        Err(e) => {
            error!("Error transcribing audio for user {}: {}", user_id, e);
            bot.send_message(msg.chat.id, format!("Ошибка распознавания: {}", e))
                .await?;
        }
    }

    Ok(())
}

pub async fn handle_photo(
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
    llm: Arc<LlmClient>,
) -> Result<()> {
    let user_id = msg.from.as_ref().unwrap().id.0 as i64;
    let user_name = get_user_name(&msg);

    info!("Processing photo from user {} ({}).", user_id, user_name);

    let photo = msg
        .photo()
        .and_then(|p| p.last())
        .ok_or_else(|| anyhow!("No photo found"))?;
    if let Some(photo_sizes) = msg.photo() {
        info!("Photo details: {} sizes available.", photo_sizes.len());
        if let Some(largest) = photo_sizes.last() {
            info!("Largest: {}x{}", largest.width, largest.height);
        }
    }

    let caption = msg.caption().unwrap_or("Опиши это изображение.");
    info!("Photo caption: '{}'", caption);

    // Determine provider
    let model = storage
        .get_user_model(user_id)
        .await?
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let provider_info = MODELS
        .iter()
        .find(|(name, _)| name == &model)
        .map(|(_, info)| info);
    let provider_name = provider_info.map(|p| p.provider).unwrap_or("unknown");
    info!(
        "Using provider '{}' for photo analysis (model: {})",
        provider_name, model
    );

    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::UploadPhoto)
        .await?;

    let file = bot.get_file(photo.file.id.clone()).await?;
    let mut buffer = Vec::new();
    bot.download_file(&file.path, &mut buffer).await?;
    info!(
        "Photo downloaded from user {}. Size: {} bytes.",
        user_id,
        buffer.len()
    );

    let system_prompt = storage
        .get_user_prompt(user_id)
        .await?
        .unwrap_or_else(|| std::env::var("SYSTEM_MESSAGE").unwrap_or_default());
    info!(
        "Using system message for user {}: '{}' (truncated)",
        user_id,
        truncate_str(&system_prompt, 100)
    );
    info!(
        "Using text prompt for image analysis: '{}' (truncated)",
        truncate_str(caption, 100)
    );

    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
        .await?;

    info!(
        "Sending image and prompt to {} for user {}.",
        provider_name, user_id
    );
    match llm
        .analyze_image(buffer, caption, &system_prompt, &model)
        .await
    {
        Ok(response) => {
            info!(
                "Received response from {} for image analysis for user {}. Snippet: '{}' (truncated)",
                provider_name,
                user_id,
                truncate_str(&response, 100)
            );

            storage
                .save_message(
                    user_id,
                    "user".to_string(),
                    format!("[Изображение] {}", caption),
                )
                .await?;
            storage
                .save_message(user_id, "assistant".to_string(), response.clone())
                .await?;

            let formatted = utils::format_text(&response);
            let parts = utils::split_long_message(&formatted, 4000);

            for (i, part) in parts.iter().enumerate() {
                info!(
                    "Sending response part {}/{} to user {}.",
                    i + 1,
                    parts.len(),
                    user_id
                );
                match bot
                    .send_message(msg.chat.id, part)
                    .parse_mode(ParseMode::Html)
                    .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        error!(
                            "Error sending response part {}/{} to user {}: {}",
                            i + 1,
                            parts.len(),
                            user_id,
                            e
                        );
                    }
                }
            }
        }
        Err(e) => {
            error!("Error processing photo for user {}: {}", user_id, e);
            bot.send_message(msg.chat.id, format!("Ошибка анализа изображения: {}", e))
                .await?;
        }
    }

    Ok(())
}
