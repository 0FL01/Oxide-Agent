use crate::bot::state::State;
use crate::config::{Settings, DEFAULT_MODEL, MODELS};
use crate::llm::{LlmClient, Message as LlmMessage};
use crate::storage::R2Storage;
use crate::utils::{self, truncate_str};
use anyhow::{anyhow, Result};
use std::sync::Arc;
use teloxide::{
    dispatching::dialogue::InMemStorage,
    net::Download,
    prelude::*,
    types::{KeyboardButton, KeyboardMarkup, ParseMode},
    utils::command::BotCommands,
};
use tracing::{error, info};

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

fn get_user_id_safe(msg: &Message) -> i64 {
    msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed())
}

/// Checks if the user has a persisted state and redirects if necessary.
/// Returns true if redirected (handled), false otherwise.
///
/// # Errors
///
/// Returns an error if dialogue update or agent message handling fails.
async fn check_state_and_redirect(
    bot: &Bot,
    msg: &Message,
    storage: &Arc<R2Storage>,
    llm: &Arc<LlmClient>,
    dialogue: &Dialogue<State, InMemStorage<State>>,
) -> Result<bool> {
    let user_id = get_user_id_safe(msg);

    if let Ok(Some(state_str)) = storage.get_user_state(user_id).await {
        if state_str == "agent_mode" {
            info!("Restoring agent mode for user {user_id} based on persisted state.");
            dialogue
                .update(State::AgentMode)
                .await
                .map_err(|e| anyhow!(e.to_string()))?;

            Box::pin(crate::bot::agent_handlers::handle_agent_message(
                bot.clone(),
                msg.clone(),
                storage.clone(),
                llm.clone(),
                dialogue.clone(),
            ))
            .await?;

            return Ok(true);
        }
    }
    Ok(false)
}

/// Supported commands for the bot
#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Поддерживаемые команды:")]
pub enum Command {
    /// Start the bot and show welcome message
    #[command(description = "Начать работу.")]
    Start,
    /// Clear chat history
    #[command(description = "Очистить историю чата.")]
    Clear,
    /// Check bot health
    #[command(description = "Проверка работоспособности.")]
    Healthcheck,
}

/// Create the main menu keyboard
///
/// # Examples
///
/// ```
/// use another_chat_rs::bot::handlers::get_main_keyboard;
/// let keyboard = get_main_keyboard();
/// assert!(!keyboard.keyboard.is_empty());
/// ```
#[must_use]
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
        vec![KeyboardButton::new("🗑 Очистить всё")],
    ];
    KeyboardMarkup::new(keyboard).resize_keyboard()
}

/// Create the extra functions keyboard
///
/// # Examples
///
/// ```
/// use another_chat_rs::bot::handlers::get_extra_functions_keyboard;
/// let keyboard = get_extra_functions_keyboard();
/// assert!(!keyboard.keyboard.is_empty());
/// ```
#[must_use]
pub fn get_extra_functions_keyboard() -> KeyboardMarkup {
    let keyboard = vec![vec![
        KeyboardButton::new("Изменить промпт"),
        KeyboardButton::new("Назад"),
    ]];
    KeyboardMarkup::new(keyboard).resize_keyboard()
}

/// Create the model selection keyboard
///
/// # Examples
///
/// ```
/// use another_chat_rs::bot::handlers::get_model_keyboard;
/// let keyboard = get_model_keyboard();
/// assert!(!keyboard.keyboard.is_empty());
/// ```
#[must_use]
pub fn get_model_keyboard() -> KeyboardMarkup {
    let mut keyboard = Vec::new();
    for model_name in MODELS.iter().map(|(n, _)| n) {
        keyboard.push(vec![KeyboardButton::new(model_name.to_string())]);
    }
    keyboard.push(vec![KeyboardButton::new("Назад")]);
    KeyboardMarkup::new(keyboard).resize_keyboard()
}

/// Start handler
///
/// # Errors
///
/// Returns an error if the welcome message cannot be sent.
pub async fn start(bot: Bot, msg: Message, storage: Arc<R2Storage>) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    let user_name = get_user_name(&msg);

    info!("User {user_id} ({user_name}) initiated /start command.");

    let saved_model = storage.get_user_model(user_id).await.unwrap_or(None);
    let model = saved_model.unwrap_or_else(|| DEFAULT_MODEL.to_string());
    info!("User {user_id} ({user_name}) is allowed. Set model to {model}");

    let text = format!(
        "<b>Привет!</b> Я бот, который может отвечать на вопросы и распознавать речь.\nТекущая модель: <b>{model}</b>"
    );

    info!("Sending welcome message to user {user_id}.");
    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .reply_markup(get_main_keyboard())
        .await?;

    Ok(())
}

/// Clear context handler
///
/// # Errors
///
/// Returns an error if chat history cannot be cleared or message cannot be sent.
pub async fn clear(bot: Bot, msg: Message, storage: Arc<R2Storage>) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    let user_name = get_user_name(&msg);

    info!("User {user_id} ({user_name}) initiated context clear.");

    match storage.clear_chat_history(user_id).await {
        Ok(()) => {
            info!("Chat history successfully cleared for user {user_id}.");
            bot.send_message(msg.chat.id, "<b>История чата очищена.</b>")
                .parse_mode(ParseMode::Html)
                .reply_markup(get_main_keyboard())
                .await?;
        }
        Err(e) => {
            error!("Error clearing chat history for user {user_id}: {e}");
            bot.send_message(msg.chat.id, "Произошла ошибка при очистке истории чата.")
                .await?;
        }
    }

    Ok(())
}

/// Healthcheck handler
///
/// # Errors
///
/// Returns an error if the healthcheck response cannot be sent.
pub async fn healthcheck(bot: Bot, msg: Message) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    info!("Healthcheck command received from user {user_id}.");
    bot.send_message(msg.chat.id, "OK").await?;
    info!("Responded 'OK' to healthcheck from user {user_id}.");
    Ok(())
}

/// Text message handler
///
/// # Errors
///
/// Returns an error if the message cannot be processed.
pub async fn handle_text(
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
    llm: Arc<LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
    settings: Arc<Settings>,
) -> Result<()> {
    let text = msg.text().unwrap_or("").to_string();
    let user_id = get_user_id_safe(&msg);
    let user_name = get_user_name(&msg);

    info!(
        "Handling message from user {user_id} ({user_name}). Text: '{}'",
        truncate_str(&text, 100)
    );

    if Box::pin(check_state_and_redirect(
        &bot, &msg, &storage, &llm, &dialogue,
    ))
    .await?
    {
        return Ok(());
    }

    if handle_menu_commands(&bot, &msg, &storage, &llm, &dialogue, &settings, &text).await? {
        return Ok(());
    }

    if MODELS.iter().any(|(name, _)| *name == text) {
        info!("User {user_id} selected model '{text}' via text input.");
        storage.update_user_model(user_id, text.clone()).await?;
        bot.send_message(msg.chat.id, format!("Модель изменена на <b>{text}</b>"))
            .parse_mode(ParseMode::Html)
            .reply_markup(get_main_keyboard())
            .await?;
        return Ok(());
    }

    process_llm_request(bot, msg, storage, llm, text).await
}

async fn handle_menu_commands(
    bot: &Bot,
    msg: &Message,
    storage: &Arc<R2Storage>,
    llm: &Arc<LlmClient>,
    dialogue: &Dialogue<State, InMemStorage<State>>,
    settings: &Arc<Settings>,
    text: &str,
) -> Result<bool> {
    let user_id = get_user_id_safe(msg);
    match text {
        "Очистить контекст" => {
            clear(bot.clone(), msg.clone(), storage.clone()).await?;
            Ok(true)
        }
        "Сменить модель" => {
            bot.send_message(msg.chat.id, "Выберите модель:")
                .reply_markup(get_model_keyboard())
                .await?;
            Ok(true)
        }
        "Доп функции" => {
            bot.send_message(msg.chat.id, "Выберите действие:")
                .reply_markup(get_extra_functions_keyboard())
                .await?;
            Ok(true)
        }
        "🤖 Режим Агента" => {
            if check_agent_access(bot, msg, settings, user_id).await? {
                crate::bot::agent_handlers::activate_agent_mode(
                    bot.clone(),
                    msg.clone(),
                    dialogue.clone(),
                    llm.clone(),
                    storage.clone(),
                )
                .await?;
            }
            Ok(true)
        }
        "Изменить промпт" => {
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
            Ok(true)
        }
        "Назад" => {
            bot.send_message(msg.chat.id, "Выберите действие: (Или начните диалог)")
                .reply_markup(get_main_keyboard())
                .await?;
            Ok(true)
        }
        "⬅️ Выйти из режима агента" | "❌ Отменить задачу" | "🗑 Очистить память" =>
        {
            let response = match text {
                "⬅️ Выйти из режима агента" => "👋 Вышли из режима агента",
                "❌ Отменить задачу" => "Нет активной задачи для отмены.",
                _ => "Память агента не активна.",
            };
            bot.send_message(msg.chat.id, response)
                .reply_markup(get_main_keyboard())
                .await?;
            Ok(true)
        }
        "🗑 Очистить всё" => {
            storage.clear_all_context(user_id).await?;
            bot.send_message(msg.chat.id, "<b>🗑 Весь контекст очищен</b>")
                .parse_mode(ParseMode::Html)
                .reply_markup(get_main_keyboard())
                .await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn check_agent_access(
    bot: &Bot,
    msg: &Message,
    settings: &Arc<Settings>,
    user_id: i64,
) -> Result<bool> {
    let agent_allowed = settings.agent_allowed_users();
    if !agent_allowed.contains(&user_id) && !agent_allowed.is_empty() {
        bot.send_message(
            msg.chat.id,
            "⛔️ У вас нет прав для доступа к режиму агента.",
        )
        .await?;
        return Ok(false);
    } else if agent_allowed.is_empty() {
        bot.send_message(
            msg.chat.id,
            "⛔️ Режим агента временно недоступен (не настроен доступ).",
        )
        .await?;
        return Ok(false);
    }
    Ok(true)
}

/// Prompt editing handler
///
/// # Errors
///
/// Returns an error if the prompt cannot be updated.
pub async fn handle_editing_prompt(
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
    dialogue: Dialogue<State, InMemStorage<State>>,
) -> Result<()> {
    let text = msg.text().unwrap_or("");
    let user_id = get_user_id_safe(&msg);

    if text == "Назад" {
        dialogue.exit().await.map_err(|e| anyhow!(e.to_string()))?;
        bot.send_message(msg.chat.id, "Отмена обновления системного промпта.")
            .reply_markup(get_main_keyboard())
            .await?;
    } else {
        storage
            .update_user_prompt(user_id, text.to_string())
            .await?;
        dialogue.exit().await.map_err(|e| anyhow!(e.to_string()))?;
        bot.send_message(msg.chat.id, "Системный промпт обновлен.")
            .reply_markup(get_main_keyboard())
            .await?;
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
    let user_id = get_user_id_safe(&msg);
    let system_prompt = storage
        .get_user_prompt(user_id)
        .await?
        .unwrap_or_else(|| std::env::var("SYSTEM_MESSAGE").unwrap_or_default());
    let history = storage.get_chat_history(user_id, 10).await?;
    let model = storage
        .get_user_model(user_id)
        .await?
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    storage
        .save_message(user_id, "user".to_string(), text.clone())
        .await?;
    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
        .await?;

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

    match llm
        .chat_completion(&system_prompt, &llm_history, &text, &model)
        .await
    {
        Ok(response) => {
            storage
                .save_message(user_id, "assistant".to_string(), response.clone())
                .await?;
            send_long_message(&bot, msg.chat.id, &response).await?;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("<b>Ошибка:</b> {e}"))
                .parse_mode(ParseMode::Html)
                .await?;
        }
    }
    Ok(())
}

async fn send_long_message(bot: &Bot, chat_id: ChatId, text: &str) -> Result<()> {
    let formatted = utils::format_text(text);
    let parts = utils::split_long_message(&formatted, 4000);
    for part in parts {
        bot.send_message(chat_id, part)
            .parse_mode(ParseMode::Html)
            .await?;
    }
    Ok(())
}

/// Voice message handler
///
/// # Errors
///
/// Returns an error if the voice message cannot be processed.
pub async fn handle_voice(
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
    llm: Arc<LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    if Box::pin(check_state_and_redirect(
        &bot, &msg, &storage, &llm, &dialogue,
    ))
    .await?
    {
        return Ok(());
    }

    let voice = msg.voice().ok_or_else(|| anyhow!("No voice found"))?;
    let model = storage
        .get_user_model(user_id)
        .await?
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let provider_info = MODELS
        .iter()
        .find(|(name, _)| name == &model)
        .map(|(_, info)| info);
    let provider_name = provider_info.map_or("unknown", |p| p.provider);

    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
        .await?;
    let file = bot.get_file(voice.file.id.clone()).await?;
    let mut buffer = Vec::new();
    bot.download_file(&file.path, &mut buffer).await?;

    let model_id = provider_info.map_or("unknown", |p| p.id);
    match llm
        .transcribe_audio_with_fallback(provider_name, buffer, "audio/wav", model_id)
        .await
    {
        Ok(text) => {
            if text.starts_with("(Gemini):") || text.starts_with("(OpenRouter):") || text.is_empty()
            {
                bot.send_message(msg.chat.id, "Не удалось распознать речь.")
                    .await?;
            } else {
                bot.send_message(
                    msg.chat.id,
                    format!("Распознано: \"{text}\"\n\nОбрабатываю запрос..."),
                )
                .await?;
                process_llm_request(bot, msg, storage, llm, text).await?;
            }
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Ошибка распознавания: {e}"))
                .await?;
        }
    }
    Ok(())
}

/// Photo message handler
///
/// # Errors
///
/// Returns an error if the photo cannot be processed.
pub async fn handle_photo(
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
    llm: Arc<LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    if Box::pin(check_state_and_redirect(
        &bot, &msg, &storage, &llm, &dialogue,
    ))
    .await?
    {
        return Ok(());
    }

    let photo = msg
        .photo()
        .and_then(|p| p.last())
        .ok_or_else(|| anyhow!("No photo found"))?;
    let caption = msg.caption().unwrap_or("Опиши это изображение.");
    let model = storage
        .get_user_model(user_id)
        .await?
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let system_prompt = storage
        .get_user_prompt(user_id)
        .await?
        .unwrap_or_else(|| std::env::var("SYSTEM_MESSAGE").unwrap_or_default());

    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::UploadPhoto)
        .await?;
    let file = bot.get_file(photo.file.id.clone()).await?;
    let mut buffer = Vec::new();
    bot.download_file(&file.path, &mut buffer).await?;

    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
        .await?;
    match llm
        .analyze_image(buffer, caption, &system_prompt, &model)
        .await
    {
        Ok(response) => {
            storage
                .save_message(
                    user_id,
                    "user".to_string(),
                    format!("[Изображение] {caption}"),
                )
                .await?;
            storage
                .save_message(user_id, "assistant".to_string(), response.clone())
                .await?;
            send_long_message(&bot, msg.chat.id, &response).await?;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Ошибка анализа изображения: {e}"))
                .await?;
        }
    }
    Ok(())
}

/// Handle document messages
/// Routes to agent mode if active, otherwise informs user
///
/// # Errors
///
/// Returns an error if document handling fails.
pub async fn handle_document(
    bot: Bot,
    msg: Message,
    dialogue: Dialogue<State, InMemStorage<State>>,
    storage: Arc<R2Storage>,
    llm: Arc<LlmClient>,
) -> Result<()> {
    let state = dialogue.get().await?.unwrap_or(State::Start);

    if let State::AgentMode = state {
        Box::pin(super::agent_handlers::handle_agent_message(
            bot, msg, storage, llm, dialogue,
        ))
        .await
    } else {
        bot.send_message(
            msg.chat.id,
            "📁 Загрузка файлов доступна только в режиме Агента.\n\n\
             Используйте /agent для активации.",
        )
        .await?;
        Ok(())
    }
}
