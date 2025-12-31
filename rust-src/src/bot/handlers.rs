use std::sync::Arc;
use teloxide::{
    prelude::*,
    types::{KeyboardButton, KeyboardMarkup, ParseMode, MessageKind, InputFile},
    utils::command::BotCommands,
    net::Download,
};
use crate::storage::R2Storage;
use crate::llm::{LlmClient, Message as LlmMessage};
use crate::config::{Settings, MODELS, DEFAULT_MODEL};
use crate::bot::state::State;
use crate::utils;
use anyhow::{Result, anyhow};

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
        vec![KeyboardButton::new("Очистить контекст"), KeyboardButton::new("Сменить модель")],
        vec![KeyboardButton::new("Доп функции")],
    ];
    KeyboardMarkup::new(keyboard).resize_keyboard()
}

pub fn get_extra_functions_keyboard() -> KeyboardMarkup {
    let keyboard = vec![
        vec![KeyboardButton::new("Изменить промпт"), KeyboardButton::new("Назад")],
    ];
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

pub async fn start(
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
) -> Result<()> {
    let user_id = msg.from.as_ref().unwrap().id.0 as i64;
    let saved_model = storage.get_user_model(user_id).await.unwrap_or(None);
    let model = saved_model.unwrap_or_else(|| DEFAULT_MODEL.to_string());
    
    let text = format!(
        "<b>Привет!</b> Я бот, который может отвечать на вопросы и распознавать речь.\nТекущая модель: <b>{}</b>",
        model
    );
    
    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .reply_markup(get_main_keyboard())
        .await?;
        
    Ok(())
}

pub async fn clear(
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
) -> Result<()> {
    let user_id = msg.from.as_ref().unwrap().id.0 as i64;
    storage.clear_chat_history(user_id).await?;
    
    bot.send_message(msg.chat.id, "<b>История чата очищена.</b>")
        .parse_mode(ParseMode::Html)
        .reply_markup(get_main_keyboard())
        .await?;
        
    Ok(())
}

pub async fn healthcheck(bot: Bot, msg: Message) -> Result<()> {
    bot.send_message(msg.chat.id, "OK").await?;
    Ok(())
}

pub async fn handle_text(
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
    llm: Arc<LlmClient>,
    dialogue: Dialogue<State, teloxide::dispatching::dialogue::InMemStorage<State>>,
) -> Result<()> {
    let text = msg.text().unwrap_or("");
    let user_id = msg.from.as_ref().unwrap().id.0 as i64;

    match text {
        "Очистить контекст" => return clear(bot, msg, storage).await,
        "Сменить модель" => {
            bot.send_message(msg.chat.id, "Выберите модель:")
                .reply_markup(get_model_keyboard())
                .await?;
            return Ok(());
        }
        "Доп функции" => {
            bot.send_message(msg.chat.id, "Выберите действие:")
                .reply_markup(get_extra_functions_keyboard())
                .await?;
            return Ok(());
        }
        "Изменить промпт" => {
            dialogue.update(State::EditingPrompt).await.map_err(|e| anyhow!(e.to_string()))?;
            bot.send_message(msg.chat.id, "Введите новый системный промпт. Для отмены введите 'Назад':")
                .reply_markup(get_extra_functions_keyboard())
                .await?;
            return Ok(());
        }
        "Назад" => {
            bot.send_message(msg.chat.id, "Выберите действие: (Или начните диалог)")
                .reply_markup(get_main_keyboard())
                .await?;
            return Ok(());
        }
        _ => {}
    }

    // Check if it's a model selection
    if let Some(_) = MODELS.iter().find(|(name, _)| *name == text) {
        storage.update_user_model(user_id, text.to_string()).await?;
        bot.send_message(msg.chat.id, format!("Модель изменена на <b>{}</b>", text))
            .parse_mode(ParseMode::Html)
            .reply_markup(get_main_keyboard())
            .await?;
        return Ok(());
    }

    // Process regular message
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
        dialogue.exit().await.map_err(|e| anyhow!(e.to_string()))?;
        bot.send_message(msg.chat.id, "Отмена обновления системного промпта.", )
            .reply_markup(get_main_keyboard())
            .await?;
    } else {
        storage.update_user_prompt(user_id, text.to_string()).await?;
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
    let user_id = msg.from.as_ref().unwrap().id.0 as i64;
    
    // Get state
    let system_prompt = storage.get_user_prompt(user_id).await?.unwrap_or_else(|| std::env::var("SYSTEM_MESSAGE").unwrap_or_default());
    let history = storage.get_chat_history(user_id, 10).await?;
    let model = storage.get_user_model(user_id).await?.unwrap_or_else(|| DEFAULT_MODEL.to_string());
    
    // Pre-save message to history
    storage.save_message(user_id, "user".to_string(), text.clone()).await?;
    
    // Show typing
    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing).await?;
    
    // Call LLM
    let llm_history: Vec<LlmMessage> = history.into_iter().map(|m| LlmMessage { role: m.role, content: m.content }).collect();
    
    match llm.chat_completion(&system_prompt, &llm_history, &text, &model).await {
        Ok(response) => {
            storage.save_message(user_id, "assistant".to_string(), response.clone()).await?;
            
            let formatted = utils::format_text(&response);
            let parts = utils::split_long_message(&formatted, 4000);
            
            for part in parts {
                 bot.send_message(msg.chat.id, part)
                    .parse_mode(ParseMode::Html)
                    .await?;
            }
        }
        Err(e) => {
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
    let voice = msg.voice().ok_or_else(|| anyhow!("No voice found"))?;
    
    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing).await?;
    
    let file = bot.get_file(voice.file.id.clone()).await?;
    // We can't use directly download as bytearray in teloxide without a feature or simple impl
    // Let's assume we use a temporary file or download to buffer
    let mut buffer = Vec::new();
    bot.download_file(&file.path, &mut buffer).await?;
    
    let model = storage.get_user_model(user_id).await?.unwrap_or_else(|| DEFAULT_MODEL.to_string());
    
    match llm.transcribe_audio(buffer, "audio/wav", &model).await {
        Ok(text) => {
            bot.send_message(msg.chat.id, format!("Распознано: \"{}\"\n\nОбрабатываю запрос...", text)).await?;
            process_llm_request(bot, msg, storage, llm, text).await?;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Ошибка распознавания: {}", e)).await?;
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
    let photo = msg.photo().and_then(|p| p.last()).ok_or_else(|| anyhow!("No photo found"))?;
    let caption = msg.caption().unwrap_or("Опиши это изображение.");
    
    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing).await?;
    
    let file = bot.get_file(photo.file.id.clone()).await?;
    let mut buffer = Vec::new();
    bot.download_file(&file.path, &mut buffer).await?;
    
    let system_prompt = storage.get_user_prompt(user_id).await?.unwrap_or_else(|| std::env::var("SYSTEM_MESSAGE").unwrap_or_default());
    let model = storage.get_user_model(user_id).await?.unwrap_or_else(|| DEFAULT_MODEL.to_string());
    
    match llm.analyze_image(buffer, caption, &system_prompt, &model).await {
        Ok(response) => {
             storage.save_message(user_id, "user".to_string(), format!("[Изображение] {}", caption)).await?;
             storage.save_message(user_id, "assistant".to_string(), response.clone()).await?;
             
             let formatted = utils::format_text(&response);
             let parts = utils::split_long_message(&formatted, 4000);
             
             for part in parts {
                  bot.send_message(msg.chat.id, part)
                     .parse_mode(ParseMode::Html)
                     .await?;
             }
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Ошибка анализа изображения: {}", e)).await?;
        }
    }
    
    Ok(())
}
