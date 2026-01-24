use crate::bot::state::State;
use crate::bot::UnauthorizedCache;
use crate::config::BotSettings;
use anyhow::{anyhow, Result};
use oxide_agent_core::llm::{LlmClient, Message as LlmMessage};
use oxide_agent_core::storage::R2Storage;
use oxide_agent_core::utils::truncate_str;
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

fn resolve_chat_model(settings: &BotSettings, stored_model: Option<String>) -> String {
    if let Some(name) = stored_model {
        if settings.agent.get_model_info_by_name(&name).is_some() {
            return name;
        }
    }
    settings.agent.get_default_chat_model_name()
}

/// Safe extraction of user ID from a message.
/// Returns 0 if the user information is missing.
pub fn get_user_id_safe(msg: &Message) -> i64 {
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
    settings: &Arc<BotSettings>,
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
                settings.clone(),
            ))
            .await?;

            return Ok(true);
        }
    }
    Ok(false)
}

/// Supported commands for the bot
#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Supported commands:")]
pub enum Command {
    /// Start the bot and show welcome message
    #[command(description = "Start the bot.")]
    Start,
    /// Clear chat history
    #[command(description = "Clear chat history.")]
    Clear,
    /// Check bot health
    #[command(description = "Check bot health.")]
    Healthcheck,
    /// Show bot statistics
    #[command(description = "Show bot statistics.")]
    Stats,
}

/// Create the main menu keyboard
///
/// # Examples
///
/// ```
/// use oxide_agent_transport_telegram::bot::handlers::get_main_keyboard;
/// let keyboard = get_main_keyboard();
/// assert!(!keyboard.keyboard.is_empty());
/// ```
#[must_use]
pub fn get_main_keyboard() -> KeyboardMarkup {
    let keyboard = vec![vec![
        KeyboardButton::new("🤖 Agent Mode"),
        KeyboardButton::new("💬 Chat Mode"),
    ]];
    KeyboardMarkup::new(keyboard).resize_keyboard()
}

/// Create the chat menu keyboard
#[must_use]
pub fn get_chat_keyboard() -> KeyboardMarkup {
    let keyboard = vec![
        vec![
            KeyboardButton::new("Clear Context"),
            KeyboardButton::new("Change Model"),
        ],
        vec![
            KeyboardButton::new("Extra Functions"),
            KeyboardButton::new("Back"),
        ],
    ];
    KeyboardMarkup::new(keyboard).resize_keyboard()
}

/// Create the extra functions keyboard
///
/// # Examples
///
/// ```
/// use oxide_agent_transport_telegram::bot::handlers::get_extra_functions_keyboard;
/// let keyboard = get_extra_functions_keyboard();
/// assert!(!keyboard.keyboard.is_empty());
/// ```
#[must_use]
pub fn get_extra_functions_keyboard() -> KeyboardMarkup {
    let keyboard = vec![vec![
        KeyboardButton::new("Edit Prompt"),
        KeyboardButton::new("Back"),
    ]];
    KeyboardMarkup::new(keyboard).resize_keyboard()
}

/// Create the model selection keyboard
///
/// # Examples
///
/// ```
/// use oxide_agent_transport_telegram::bot::handlers::get_model_keyboard;
/// use oxide_agent_transport_telegram::config::BotSettings;
/// // This example might need a mock settings or be run in a context where settings are available
/// ```
#[must_use]
pub fn get_model_keyboard(settings: &BotSettings) -> KeyboardMarkup {
    let mut keyboard = Vec::new();
    for model_name in settings.agent.get_chat_models().iter().map(|(n, _)| n) {
        keyboard.push(vec![KeyboardButton::new(model_name.to_string())]);
    }
    keyboard.push(vec![KeyboardButton::new("Back")]);
    KeyboardMarkup::new(keyboard).resize_keyboard()
}

/// Start handler
///
/// # Errors
///
/// Returns an error if the welcome message cannot be sent.
pub async fn start(
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
    settings: Arc<BotSettings>,
    dialogue: Dialogue<State, InMemStorage<State>>,
) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    let user_name = get_user_name(&msg);

    info!("User {user_id} ({user_name}) initiated /start command.");

    // Reset dialogue state to Start (exit agent mode if active)
    dialogue
        .update(State::Start)
        .await
        .map_err(|e| anyhow!(e.to_string()))?;

    // Reset persisted state in storage to chat_mode
    let _ = storage
        .update_user_state(user_id, "chat_mode".to_string())
        .await;

    let saved_model = storage.get_user_model(user_id).await.unwrap_or(None);
    let model = resolve_chat_model(&settings, saved_model);
    info!("User {user_id} ({user_name}) is allowed. Set model to {model}");

    let text = "👋 <b>I am Oxide Agent.</b>\n\n\
         I am here to automate your routine. Switch me to <b>Agent Mode</b>, and I can:\n\n\
         • Write and run code\n\
         • Download and process video/files\n\
         • Google information for you\n\n\
         I don't just answer questions — I solve tasks.\n\n\
         <i>Also available: <b>Chat Mode</b> for simple questions.</i>\n\n\
         👇 <b>Enable full power:</b>"
        .to_string();

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
            bot.send_message(msg.chat.id, "<b>Chat history cleared.</b>")
                .parse_mode(ParseMode::Html)
                .reply_markup(get_chat_keyboard())
                .await?;
        }
        Err(e) => {
            error!("Error clearing chat history for user {user_id}: {e}");
            bot.send_message(
                msg.chat.id,
                "An error occurred while clearing chat history.",
            )
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

/// Stats handler - shows bot statistics including unauthorized cache metrics
///
/// # Errors
///
/// Returns an error if the stats response cannot be sent.
pub async fn stats(bot: Bot, msg: Message, cache: Arc<UnauthorizedCache>) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    info!("Stats command received from user {user_id}.");

    let cooldown_secs = cache.cooldown().as_secs();
    let cooldown_mins = cooldown_secs / 60;

    let stats_text = format!(
        "<b>📊 Bot Statistics</b>\n\n\
        <b>Anti-spam protection (Access Denied):</b>\n\
        • Cooldown period: {} min.\n\
        • Cache entries: {}\n\
        • Blocked notifications: {}\n\n\
        <i>Bot responds with \"Access Denied\" no more than once every {} minutes per user to avoid being banned by Telegram.</i>",
        cooldown_mins,
        cache.entry_count(),
        cache.silenced_count(),
        cooldown_mins
    );

    bot.send_message(msg.chat.id, stats_text)
        .parse_mode(ParseMode::Html)
        .await?;

    info!("Responded to stats from user {user_id}.");
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
    settings: Arc<BotSettings>,
) -> Result<()> {
    let text = msg.text().unwrap_or("").to_string();
    let user_id = get_user_id_safe(&msg);
    let user_name = get_user_name(&msg);

    info!(
        "Handling message from user {user_id} ({user_name}). Text: '{}'",
        truncate_str(&text, 100)
    );

    if Box::pin(check_state_and_redirect(
        &bot, &msg, &storage, &llm, &dialogue, &settings,
    ))
    .await?
    {
        return Ok(());
    }

    if handle_menu_commands(&bot, &msg, &storage, &llm, &dialogue, &settings, &text).await? {
        return Ok(());
    }

    let state = dialogue.get().await?.unwrap_or(State::Start);
    if matches!(state, State::Start) {
        bot.send_message(msg.chat.id, "Please select a mode:")
            .reply_markup(get_main_keyboard())
            .await?;
        return Ok(());
    }

    if settings.agent.get_model_info_by_name(&text).is_some() {
        info!("User {user_id} selected model '{text}' via text input.");
        storage.update_user_model(user_id, text.clone()).await?;
        bot.send_message(msg.chat.id, format!("Model changed to <b>{text}</b>"))
            .parse_mode(ParseMode::Html)
            .reply_markup(get_chat_keyboard())
            .await?;
        return Ok(());
    }

    process_llm_request(bot, msg, storage, llm, settings, text).await
}

async fn handle_menu_commands(
    bot: &Bot,
    msg: &Message,
    storage: &Arc<R2Storage>,
    llm: &Arc<LlmClient>,
    dialogue: &Dialogue<State, InMemStorage<State>>,
    settings: &Arc<BotSettings>,
    text: &str,
) -> Result<bool> {
    let user_id = get_user_id_safe(msg);
    match text {
        "💬 Chat Mode" => {
            dialogue
                .update(State::ChatMode)
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
            let saved_model = storage.get_user_model(user_id).await?;
            let model = resolve_chat_model(settings, saved_model);
            bot.send_message(
                msg.chat.id,
                format!("<b>Chat mode activated.</b>\nCurrent model: <b>{model}</b>"),
            )
            .parse_mode(ParseMode::Html)
            .reply_markup(get_chat_keyboard())
            .await?;
            Ok(true)
        }
        "Clear Context" => {
            clear(bot.clone(), msg.clone(), storage.clone()).await?;
            Ok(true)
        }
        "Change Model" => {
            bot.send_message(msg.chat.id, "Select a model:")
                .reply_markup(get_model_keyboard(settings))
                .await?;
            Ok(true)
        }
        "Extra Functions" => {
            bot.send_message(msg.chat.id, "Select an action:")
                .reply_markup(get_extra_functions_keyboard())
                .await?;
            Ok(true)
        }
        "🤖 Agent Mode" => {
            if check_agent_access(bot, msg, settings, user_id).await? {
                crate::bot::agent_handlers::activate_agent_mode(
                    bot.clone(),
                    msg.clone(),
                    dialogue.clone(),
                    llm.clone(),
                    storage.clone(),
                    settings.clone(),
                )
                .await?;
            }
            Ok(true)
        }
        "Edit Prompt" => {
            dialogue
                .update(State::EditingPrompt)
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
            bot.send_message(
                msg.chat.id,
                "Enter a new system prompt. To cancel, type 'Back':",
            )
            .reply_markup(get_extra_functions_keyboard())
            .await?;
            Ok(true)
        }
        "Back" => {
            let state = dialogue.get().await?.unwrap_or(State::Start);
            if matches!(state, State::ChatMode) || matches!(state, State::EditingPrompt) {
                dialogue
                    .update(State::Start)
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?;
                bot.send_message(msg.chat.id, "Please select a mode:")
                    .reply_markup(get_main_keyboard())
                    .await?;
            } else {
                bot.send_message(msg.chat.id, "Please select a mode:")
                    .reply_markup(get_main_keyboard())
                    .await?;
            }
            Ok(true)
        }
        "⬅️ Exit Agent Mode" | "❌ Cancel Task" | "🗑 Clear Memory" => {
            let response = match text {
                "⬅️ Exit Agent Mode" => "👋 Exited agent mode",
                "❌ Cancel Task" => "No active task to cancel.",
                _ => "Agent memory is not active.",
            };
            bot.send_message(msg.chat.id, response)
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
    settings: &Arc<BotSettings>,
    user_id: i64,
) -> Result<bool> {
    let agent_allowed = settings.telegram.agent_allowed_users();
    if !agent_allowed.contains(&user_id) && !agent_allowed.is_empty() {
        bot.send_message(
            msg.chat.id,
            "⛔️ You do not have permission to access agent mode.",
        )
        .await?;
        return Ok(false);
    } else if agent_allowed.is_empty() {
        bot.send_message(
            msg.chat.id,
            "⛔️ Agent mode is temporarily unavailable (access not configured).",
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

    if text == "Back" {
        dialogue
            .update(State::ChatMode)
            .await
            .map_err(|e| anyhow!(e.to_string()))?;
        bot.send_message(msg.chat.id, "System prompt update canceled.")
            .reply_markup(get_chat_keyboard())
            .await?;
    } else {
        storage
            .update_user_prompt(user_id, text.to_string())
            .await?;
        dialogue
            .update(State::ChatMode)
            .await
            .map_err(|e| anyhow!(e.to_string()))?;
        bot.send_message(msg.chat.id, "System prompt updated.")
            .reply_markup(get_chat_keyboard())
            .await?;
    }
    Ok(())
}

async fn process_llm_request(
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
    llm: Arc<LlmClient>,
    settings: Arc<BotSettings>,
    text: String,
) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    let system_prompt = storage
        .get_user_prompt(user_id)
        .await?
        .unwrap_or_else(|| std::env::var("SYSTEM_MESSAGE").unwrap_or_default());
    let history = storage.get_chat_history(user_id, 10).await?;
    let saved_model = storage.get_user_model(user_id).await?;
    let model = resolve_chat_model(&settings, saved_model);

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
            bot.send_message(msg.chat.id, format!("<b>Error:</b> {e}"))
                .parse_mode(ParseMode::Html)
                .await?;
        }
    }
    Ok(())
}

/// Re-export the shared send_long_message function for convenience.
/// This function formats text and splits it into multiple messages if needed.
use super::messaging::send_long_message;

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
    settings: Arc<BotSettings>,
) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    if Box::pin(check_state_and_redirect(
        &bot, &msg, &storage, &llm, &dialogue, &settings,
    ))
    .await?
    {
        return Ok(());
    }

    let state = dialogue.get().await?.unwrap_or(State::Start);
    if matches!(state, State::Start) {
        bot.send_message(msg.chat.id, "Please select a mode:")
            .reply_markup(get_main_keyboard())
            .await?;
        return Ok(());
    }

    if !llm.is_multimodal_available() {
        bot.send_message(
            msg.chat.id,
            "🚫 Feature unavailable.\nMedia processing is disabled because the Gemini or OpenRouter provider is not configured.",
        )
        .await?;
        return Ok(());
    }

    let voice = msg.voice().ok_or_else(|| anyhow!("No voice found"))?;
    let saved_model = storage.get_user_model(user_id).await?;
    let model = resolve_chat_model(&settings, saved_model);

    let provider_info = settings.agent.get_model_info_by_name(&model);
    let provider_name = provider_info.as_ref().map_or("unknown", |p| &p.provider);

    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
        .await?;

    // Download voice file with retry logic
    let buffer = oxide_agent_core::utils::retry_transport_operation(|| async {
        let file = bot.get_file(voice.file.id.clone()).await?;
        let mut buf = Vec::new();
        bot.download_file(&file.path, &mut buf).await?;
        Ok(buf)
    })
    .await?;

    let model_id = provider_info.as_ref().map_or("unknown", |p| &p.id);
    match llm
        .transcribe_audio_with_fallback(provider_name, buffer, "audio/wav", model_id)
        .await
    {
        Ok(text) => {
            if text.starts_with("(Gemini):") || text.starts_with("(OpenRouter):") || text.is_empty()
            {
                bot.send_message(msg.chat.id, "Failed to recognize speech.")
                    .await?;
            } else {
                bot.send_message(
                    msg.chat.id,
                    format!("Recognized: \"{text}\"\n\nProcessing request..."),
                )
                .await?;
                process_llm_request(bot, msg, storage, llm, settings, text).await?;
            }
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Recognition error: {e}"))
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
    settings: Arc<BotSettings>,
) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    if Box::pin(check_state_and_redirect(
        &bot, &msg, &storage, &llm, &dialogue, &settings,
    ))
    .await?
    {
        return Ok(());
    }

    let state = dialogue.get().await?.unwrap_or(State::Start);
    if matches!(state, State::Start) {
        bot.send_message(msg.chat.id, "Please select a mode:")
            .reply_markup(get_main_keyboard())
            .await?;
        return Ok(());
    }

    if !llm.is_multimodal_available() {
        bot.send_message(
            msg.chat.id,
            "🚫 Feature unavailable.\nMedia processing is disabled because the Gemini or OpenRouter provider is not configured.",
        )
        .await?;
        return Ok(());
    }

    let photo = msg
        .photo()
        .and_then(|p| p.last())
        .ok_or_else(|| anyhow!("No photo found"))?;
    let caption = msg.caption().unwrap_or("Describe this image.");
    let saved_model = storage.get_user_model(user_id).await?;
    let model = resolve_chat_model(&settings, saved_model);
    let system_prompt = storage
        .get_user_prompt(user_id)
        .await?
        .unwrap_or_else(|| std::env::var("SYSTEM_MESSAGE").unwrap_or_default());

    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::UploadPhoto)
        .await?;

    // Download photo file with retry logic
    let buffer = oxide_agent_core::utils::retry_transport_operation(|| async {
        let file = bot.get_file(photo.file.id.clone()).await?;
        let mut buf = Vec::new();
        bot.download_file(&file.path, &mut buf).await?;
        Ok(buf)
    })
    .await?;

    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
        .await?;
    match llm
        .analyze_image(buffer, caption, &system_prompt, &model)
        .await
    {
        Ok(response) => {
            storage
                .save_message(user_id, "user".to_string(), format!("[Image] {caption}"))
                .await?;
            storage
                .save_message(user_id, "assistant".to_string(), response.clone())
                .await?;
            send_long_message(&bot, msg.chat.id, &response).await?;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("Image analysis error: {e}"))
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
    settings: Arc<BotSettings>,
) -> Result<()> {
    let state = dialogue.get().await?.unwrap_or(State::Start);

    if matches!(state, State::AgentMode) {
        Box::pin(super::agent_handlers::handle_agent_message(
            bot, msg, storage, llm, dialogue, settings,
        ))
        .await
    } else {
        bot.send_message(
            msg.chat.id,
            "📁 File upload is available only in Agent Mode.\n\n\
             Use /agent to activate.",
        )
        .await?;
        Ok(())
    }
}
