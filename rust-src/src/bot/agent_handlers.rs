//! Agent mode handlers for Telegram bot
//!
//! Provides handlers for activating agent mode, processing messages,
//! and managing agent sessions.

use crate::agent::{
    executor::AgentExecutor,
    preprocessor::{AgentInput, Preprocessor},
    progress::{AgentEvent, ProgressState},
    AgentSession,
};
use crate::bot::state::State;
use crate::config::AGENT_MAX_ITERATIONS;
use crate::llm::LlmClient;
use crate::storage::R2Storage;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::net::Download;
use teloxide::prelude::*;
use teloxide::types::{KeyboardButton, KeyboardMarkup, MessageId, ParseMode};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Type alias for dialogue
pub type AgentDialogue = Dialogue<State, InMemStorage<State>>;

/// Global agent sessions storage (`user_id` -> session)
static AGENT_SESSIONS: LazyLock<RwLock<HashMap<i64, AgentExecutor>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Get the agent mode keyboard
#[must_use]
pub fn get_agent_keyboard() -> KeyboardMarkup {
    KeyboardMarkup::new(vec![
        vec![KeyboardButton::new("❌ Отменить задачу")],
        vec![KeyboardButton::new("🗑 Очистить задачи")],
        vec![KeyboardButton::new("🗑 Очистить память")],
        vec![KeyboardButton::new("🔄 Пересоздать контейнер")],
        vec![KeyboardButton::new("⬅️ Выйти из режима агента")],
    ])
    .resize_keyboard()
}

/// Activate agent mode for a user
///
/// # Errors
///
/// Returns an error if the user state cannot be updated or the welcome message cannot be sent.
pub async fn activate_agent_mode(
    bot: Bot,
    msg: Message,
    dialogue: AgentDialogue,
    llm: Arc<LlmClient>,
    storage: Arc<R2Storage>,
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let chat_id = msg.chat.id.0;

    info!("Activating agent mode for user {user_id}");

    // Create new session
    let mut session = AgentSession::new(user_id, chat_id);

    // Load saved agent memory if exists
    if let Ok(Some(saved_memory)) = storage.load_agent_memory(user_id).await {
        session.memory = saved_memory;
        info!("Loaded agent memory for user {user_id}");
    }

    let executor = AgentExecutor::new(llm.clone(), session);

    // Store session
    {
        let mut sessions = AGENT_SESSIONS.write().await;
        sessions.insert(user_id, executor);
    }

    // Save state to DB
    storage
        .update_user_state(user_id, "agent_mode".to_string())
        .await?;

    // Update dialogue state
    dialogue.update(State::AgentMode).await?;

    // Send welcome message
    let welcome = r"🤖 <b>Режим Агента активирован</b>

Я готов помочь с решением сложных задач. Отправьте мне:
• 📝 Текстовое описание задачи
• 🎤 Голосовое сообщение
• 🖼 Изображение с описанием

Я буду анализировать задачу, декомпозировать её и выполнять пошагово, показывая прогресс.

<i>Лимит времени: 30 минут на задачу</i>";

    bot.send_message(msg.chat.id, welcome)
        .parse_mode(ParseMode::Html)
        .reply_markup(get_agent_keyboard())
        .await?;

    Ok(())
}

/// Handle a message in agent mode
///
/// # Errors
///
/// Returns an error if the input cannot be preprocessed or the task cannot be executed.
pub async fn handle_agent_message(
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
    llm: Arc<LlmClient>,
    dialogue: AgentDialogue,
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let chat_id = msg.chat.id;

    // Check for control commands
    if let Some(text) = msg.text() {
        match text {
            "❌ Отменить задачу" => {
                return cancel_agent_task(bot, msg, dialogue).await;
            }
            "/cleartodos" | "🗑 Очистить задачи" => {
                return clear_agent_todos(bot, msg).await;
            }
            "🗑 Очистить память" => {
                return clear_agent_memory(bot, msg, storage).await;
            }
            "🔄 Пересоздать контейнер" => {
                return confirm_agent_wipe(bot, msg, dialogue).await;
            }
            "⬅️ Выйти из режима агента" => {
                return exit_agent_mode(bot, msg, dialogue, storage).await;
            }
            _ => {}
        }
    }

    // Get or create session
    ensure_session_exists(user_id, chat_id.0, &llm).await;

    // Preprocess input
    let preprocessor = Preprocessor::new(llm.clone(), user_id);
    let input = extract_agent_input(&bot, &msg).await?;
    let task_text = preprocessor.preprocess_input(input).await?;
    info!(
        user_id = user_id,
        chat_id = chat_id.0,
        "Input preprocessed, task text extracted"
    );

    // Send initial progress message
    let progress_msg = bot
        .send_message(chat_id, "⏳ Обработка задачи...")
        .parse_mode(ParseMode::Html)
        .await?;

    // Create progress tracking channel
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(100);

    // Spawn progress updater task
    let bot_clone = bot.clone();
    let chat_id_clone = chat_id;
    let msg_id = progress_msg.id;

    let progress_handle = tokio::spawn(async move {
        let mut state = ProgressState::new(AGENT_MAX_ITERATIONS);
        let mut last_update = std::time::Instant::now();
        let mut needs_update = false;
        let throttle_duration = std::time::Duration::from_millis(1500);

        while let Some(event) = rx.recv().await {
            state.update(event);
            needs_update = true;

            if last_update.elapsed() >= throttle_duration {
                let text = state.format_telegram();
                edit_message_safe(&bot_clone, chat_id_clone, msg_id, &text).await;
                last_update = std::time::Instant::now();
                needs_update = false;
            }
        }

        let final_text = state.format_telegram();
        if needs_update {
            edit_message_safe(&bot_clone, chat_id_clone, msg_id, &final_text).await;
        }
        final_text
    });

    // Execute the task
    let result = execute_agent_task(user_id, &task_text, Some(tx)).await;
    let progress_text = progress_handle.await.unwrap_or_default();

    // Save agent memory after task execution
    save_memory_after_task(user_id, &storage).await;

    // Update the message with the result
    match result {
        Ok(response) => {
            edit_message_safe(&bot, chat_id, progress_msg.id, &progress_text).await;
            let formatted_response = crate::utils::format_text(&response);
            bot.send_message(chat_id, formatted_response)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        Err(e) => {
            let error_text = format!("{progress_text}\n\n❌ <b>Ошибка:</b>\n\n{e}");
            edit_message_safe(&bot, chat_id, progress_msg.id, &error_text).await;
        }
    }

    Ok(())
}

async fn ensure_session_exists(user_id: i64, chat_id: i64, llm: &Arc<LlmClient>) {
    let has_session = {
        let sessions = AGENT_SESSIONS.read().await;
        sessions.contains_key(&user_id)
    };

    if !has_session {
        let session = AgentSession::new(user_id, chat_id);
        let executor = AgentExecutor::new(llm.clone(), session);
        let mut sessions = AGENT_SESSIONS.write().await;
        sessions.insert(user_id, executor);
    }
}

async fn save_memory_after_task(user_id: i64, storage: &Arc<R2Storage>) {
    let sessions = AGENT_SESSIONS.read().await;
    if let Some(executor) = sessions.get(&user_id) {
        let _ = storage
            .save_agent_memory(user_id, &executor.session().memory)
            .await;
    }
}

/// Execute an agent task and return the result
/// NOTE: Takes the executor out of the map during execution to avoid holding lock
async fn execute_agent_task(
    user_id: i64,
    task: &str,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> Result<String> {
    // Take the executor out of the map to avoid holding lock during execution
    let mut executor = {
        let mut sessions = AGENT_SESSIONS.write().await;
        sessions
            .remove(&user_id)
            .ok_or_else(|| anyhow::anyhow!("No agent session found"))?
    };

    // Check timeout
    if executor.is_timed_out() {
        executor.reset();
        AGENT_SESSIONS.write().await.insert(user_id, executor);
        return Err(anyhow::anyhow!(
            "Предыдущая сессия истекла по таймауту. Начинаю новую сессию."
        ));
    }

    // Execute the task without holding the lock
    let result = executor.execute(task, progress_tx).await;

    // Put the executor back
    {
        let mut sessions = AGENT_SESSIONS.write().await;
        sessions.insert(user_id, executor);
    }

    result
}

/// Extract input from a message
async fn extract_agent_input(bot: &Bot, msg: &Message) -> Result<AgentInput> {
    if let Some(voice) = msg.voice() {
        let file = bot.get_file(voice.file.id.clone()).await?;
        let mut buffer = Vec::new();
        bot.download_file(&file.path, &mut buffer).await?;
        let mime_type = voice
            .mime_type
            .as_ref()
            .map_or_else(|| "audio/ogg".to_string(), ToString::to_string);
        return Ok(AgentInput::Voice {
            bytes: buffer,
            mime_type,
        });
    }

    if let Some(photos) = msg.photo() {
        if let Some(photo) = photos.last() {
            let file = bot.get_file(photo.file.id.clone()).await?;
            let mut buffer = Vec::new();
            bot.download_file(&file.path, &mut buffer).await?;
            let caption = msg.caption().map(ToString::to_string);
            return Ok(AgentInput::Image {
                bytes: buffer,
                context: caption,
            });
        }
    }

    // Document
    if let Some(doc) = msg.document() {
        const MAX_FILE_SIZE: u32 = 20 * 1024 * 1024; // 20 MB

        if doc.file.size > MAX_FILE_SIZE {
            anyhow::bail!(
                "Файл слишком большой: {:.1} MB (максимум 20 MB)",
                f64::from(doc.file.size) / 1024.0 / 1024.0
            );
        }

        let file = bot.get_file(doc.file.id.clone()).await?;
        let mut buffer = Vec::new();
        bot.download_file(&file.path, &mut buffer).await?;

        info!(
            file_name = ?doc.file_name,
            mime_type = ?doc.mime_type,
            size = buffer.len(),
            "Downloaded document from Telegram"
        );

        return Ok(AgentInput::Document {
            bytes: buffer,
            file_name: doc.file_name.clone().unwrap_or_else(|| "file".to_string()),
            mime_type: doc.mime_type.as_ref().map(|m| m.to_string()),
            caption: msg.caption().map(String::from),
        });
    }

    let text = msg
        .text()
        .or_else(|| msg.caption())
        .unwrap_or("")
        .to_string();
    Ok(AgentInput::Text(text))
}

/// Edit a message safely (ignore errors)
async fn edit_message_safe(bot: &Bot, chat_id: ChatId, msg_id: MessageId, text: &str) {
    const ERROR_NOT_MODIFIED: &str = "message is not modified";
    const ERROR_NOT_FOUND: &str = "message to edit not found";

    let truncated = if text.chars().count() > 4000 {
        let truncated_text = crate::utils::truncate_str(text, 4000);
        format!("{truncated_text}...\n\n<i>(сообщение обрезано)</i>")
    } else {
        text.to_string()
    };

    if let Err(e) = bot
        .edit_message_text(chat_id, msg_id, truncated)
        .parse_mode(ParseMode::Html)
        .await
    {
        let err_msg = e.to_string();
        if !err_msg.contains(ERROR_NOT_MODIFIED) && !err_msg.contains(ERROR_NOT_FOUND) {
            warn!("Failed to edit message: {e}");
        } else {
            debug!("Message update skipped or not found: {err_msg}");
        }
    }
}

/// Cancel the current agent task
///
/// # Errors
///
/// Returns an error if the cancellation message cannot be sent.
pub async fn cancel_agent_task(bot: Bot, msg: Message, _dialogue: AgentDialogue) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());

    {
        let mut sessions = AGENT_SESSIONS.write().await;
        if let Some(executor) = sessions.get_mut(&user_id) {
            executor.cancel();
        }
    }

    bot.send_message(msg.chat.id, "❌ Задача отменена")
        .reply_markup(get_agent_keyboard())
        .await?;
    Ok(())
}

/// Clear agent todos
///
/// # Errors
///
/// Returns an error if the confirmation message cannot be sent.
pub async fn clear_agent_todos(bot: Bot, msg: Message) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());

    {
        let mut sessions = AGENT_SESSIONS.write().await;
        if let Some(executor) = sessions.get_mut(&user_id) {
            executor.session_mut().clear_todos();
        }
    }

    bot.send_message(msg.chat.id, "📋 Список задач очищен")
        .await?;
    Ok(())
}

/// Clear agent memory
///
/// # Errors
///
/// Returns an error if the confirmation message cannot be sent.
pub async fn clear_agent_memory(bot: Bot, msg: Message, storage: Arc<R2Storage>) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());

    {
        let mut sessions = AGENT_SESSIONS.write().await;
        if let Some(executor) = sessions.get_mut(&user_id) {
            executor.reset();
        }
    }

    let _ = storage.clear_agent_memory(user_id).await;
    bot.send_message(msg.chat.id, "🗑 Память агента очищена")
        .reply_markup(get_agent_keyboard())
        .await?;
    Ok(())
}

/// Exit agent mode
///
/// # Errors
///
/// Returns an error if the dialogue state or user state cannot be updated.
pub async fn exit_agent_mode(
    bot: Bot,
    msg: Message,
    dialogue: AgentDialogue,
    storage: Arc<R2Storage>,
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());

    save_memory_after_task(user_id, &storage).await;

    {
        let mut sessions = AGENT_SESSIONS.write().await;
        sessions.remove(&user_id);
    }

    let _ = storage
        .update_user_state(user_id, "chat_mode".to_string())
        .await;
    dialogue.update(State::Start).await?;

    let keyboard = crate::bot::handlers::get_main_keyboard();
    bot.send_message(msg.chat.id, "👋 Вышли из режима агента")
        .reply_markup(keyboard)
        .await?;
    Ok(())
}

/// Ask for confirmation to recreate container
///
/// # Errors
///
/// Returns an error if the confirmation message cannot be sent.
pub async fn confirm_agent_wipe(bot: Bot, msg: Message, dialogue: AgentDialogue) -> Result<()> {
    dialogue.update(State::AgentWipeConfirmation).await?;
    let keyboard = KeyboardMarkup::new(vec![vec![
        KeyboardButton::new("✅ Да"),
        KeyboardButton::new("❌ Отмена"),
    ]])
    .resize_keyboard();
    bot.send_message(msg.chat.id, "⚠️ <b>Внимание!</b>\n\nЭто действие удалит текущий контейнер агента и все файлы внутри него. История переписки сохранится.\n\nВы уверены?")
        .parse_mode(ParseMode::Html).reply_markup(keyboard).await?;
    Ok(())
}

/// Handle confirmation for wiping agent container
///
/// # Errors
///
/// Returns an error if the container cannot be recreated or message cannot be sent.
pub async fn handle_agent_wipe_confirmation(
    bot: Bot,
    msg: Message,
    dialogue: AgentDialogue,
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let text = msg.text().unwrap_or("");

    match text {
        "✅ Да" => {
            let mut sessions = AGENT_SESSIONS.write().await;
            if let Some(executor) = sessions.get_mut(&user_id) {
                match executor.session_mut().ensure_sandbox().await {
                    Ok(sandbox) => {
                        if let Err(e) = sandbox.recreate().await {
                            bot.send_message(msg.chat.id, format!("Ошибка при пересоздании: {e}"))
                                .await?;
                        } else {
                            bot.send_message(msg.chat.id, "✅ Контейнер успешно пересоздан.")
                                .await?;
                        }
                    }
                    Err(_) => {
                        bot.send_message(msg.chat.id, "Ошибка доступа к менеджеру песочницы.")
                            .await?;
                    }
                }
            }
        }
        "❌ Отмена" => {
            bot.send_message(msg.chat.id, "Отменено.").await?;
        }
        _ => {
            bot.send_message(msg.chat.id, "Пожалуйста, выберите вариант на клавиатуре.")
                .await?;
            return Ok(());
        }
    }

    dialogue.update(State::AgentMode).await?;
    bot.send_message(msg.chat.id, "Готов к работе.")
        .reply_markup(get_agent_keyboard())
        .await?;
    Ok(())
}
