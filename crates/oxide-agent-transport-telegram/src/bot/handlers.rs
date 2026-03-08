use crate::bot::context::TelegramHandlerContext;
use crate::bot::state::State;
use crate::bot::views::AgentView;
use crate::bot::UnauthorizedCache;
use crate::config::BotSettings;
use anyhow::{anyhow, Result};
use oxide_agent_core::agent::SessionId;
use oxide_agent_core::llm::{LlmClient, Message as LlmMessage};
use oxide_agent_core::storage::{generate_chat_uuid, StorageProvider};
use oxide_agent_core::utils::truncate_str;
use std::sync::Arc;
use teloxide::{
    dispatching::dialogue::InMemStorage,
    net::Download,
    prelude::*,
    types::{
        CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, KeyboardButton, KeyboardMarkup,
        ParseMode,
    },
    utils::command::BotCommands,
};
use tracing::info;

use super::agent_handlers::StartResetOutcome;

const CHAT_ATTACH_PREFIX: &str = "chat_attach:";
const CHAT_DETACH_CALLBACK: &str = "chat_detach";

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
    dialogue: &Dialogue<State, InMemStorage<State>>,
    context: &TelegramHandlerContext,
) -> Result<bool> {
    let user_id = get_user_id_safe(msg);

    if let Some(state) = restore_persisted_dialogue_state(user_id, dialogue, context).await? {
        if matches!(state, State::AgentMode) {
            Box::pin(crate::bot::agent_handlers::handle_agent_message(
                bot.clone(),
                msg.clone(),
                dialogue.clone(),
                Arc::new(context.clone()),
            ))
            .await?;

            return Ok(true);
        }
    }
    Ok(false)
}

async fn restore_persisted_dialogue_state(
    user_id: i64,
    dialogue: &Dialogue<State, InMemStorage<State>>,
    context: &TelegramHandlerContext,
) -> Result<Option<State>> {
    if let Ok(Some(state_str)) = context.storage.get_user_state(user_id).await {
        match state_str.as_str() {
            "agent_mode" => {
                let agent_allowed = context.settings.telegram.agent_allowed_users();
                if !agent_allowed.contains(&user_id) || agent_allowed.is_empty() {
                    info!(
                        "Skipping persisted agent mode restore for user {user_id}: access revoked."
                    );
                    context
                        .storage
                        .update_user_state(user_id, "chat_mode".to_string())
                        .await
                        .map_err(anyhow::Error::from)?;
                    dialogue
                        .update(State::ChatMode)
                        .await
                        .map_err(|e| anyhow!(e.to_string()))?;
                    return Ok(Some(State::ChatMode));
                }
                info!("Restoring agent mode for user {user_id} based on persisted state.");
                dialogue
                    .update(State::AgentMode)
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?;
                return Ok(Some(State::AgentMode));
            }
            "chat_mode" => {
                info!("Restoring chat mode for user {user_id} based on persisted state.");
                dialogue
                    .update(State::ChatMode)
                    .await
                    .map_err(|e| anyhow!(e.to_string()))?;
                return Ok(Some(State::ChatMode));
            }
            _ => {}
        }
    }

    Ok(None)
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
            KeyboardButton::new("Clear Flow"),
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

/// Prepare `/start` state transitions under the runtime session gate.
async fn prepare_start_state(
    user_id: i64,
    session_id: SessionId,
    dialogue: &Dialogue<State, InMemStorage<State>>,
    context: &TelegramHandlerContext,
) -> Result<StartResetOutcome<()>> {
    let dialogue = dialogue.clone();
    let storage = Arc::clone(&context.storage);

    context
        .task_runtime
        .reset_start_if_idle(session_id, move || async move {
            storage
                .update_user_state(user_id, "chat_mode".to_string())
                .await
                .map_err(anyhow::Error::from)?;
            dialogue
                .update(State::Start)
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
            Ok(())
        })
        .await
}

/// Start handler
///
/// # Errors
///
/// Returns an error if the welcome message cannot be sent.
pub async fn start(
    bot: Bot,
    msg: Message,
    dialogue: Dialogue<State, InMemStorage<State>>,
    context: Arc<TelegramHandlerContext>,
) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    let user_name = get_user_name(&msg);
    let session_id = SessionId::from(user_id);

    info!("User {user_id} ({user_name}) initiated /start command.");

    match prepare_start_state(user_id, session_id, &dialogue, &context).await? {
        StartResetOutcome::BlockedByTask => {
            let _ = context
                .storage
                .update_user_state(user_id, "agent_mode".to_string())
                .await;
            dialogue
                .update(State::AgentMode)
                .await
                .map_err(|e| anyhow!(e.to_string()))?;

            bot.send_message(
                msg.chat.id,
                crate::bot::views::DefaultAgentView::task_already_running(),
            )
            .reply_markup(crate::bot::views::get_agent_keyboard())
            .await?;

            return Ok(());
        }
        StartResetOutcome::Reset(()) => {}
    }

    let saved_model = context
        .storage
        .get_user_model(user_id)
        .await
        .unwrap_or(None);
    let model = resolve_chat_model(&context.settings, saved_model);
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

async fn ensure_current_chat_uuid(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
) -> Result<String> {
    let mut config = storage.get_user_config(user_id).await?;

    if let Some(chat_uuid) = config.current_chat_uuid {
        return Ok(chat_uuid);
    }

    let chat_uuid = generate_chat_uuid();
    config.current_chat_uuid = Some(chat_uuid.clone());
    storage.update_user_config(user_id, config).await?;

    Ok(chat_uuid)
}

/// Clear flow handler
///
/// # Errors
///
/// Returns an error if user config cannot be updated or message cannot be sent.
pub async fn clear(bot: Bot, msg: Message, storage: Arc<dyn StorageProvider>) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    let user_name = get_user_name(&msg);

    info!("User {user_id} ({user_name}) initiated flow clear.");

    let mut config = storage.get_user_config(user_id).await?;
    let new_chat_uuid = generate_chat_uuid();
    config.current_chat_uuid = Some(new_chat_uuid.clone());
    storage.update_user_config(user_id, config).await?;

    info!("Started new chat flow for user {user_id}: {new_chat_uuid}");
    bot.send_message(msg.chat.id, "<b>Flow cleared.</b>")
        .parse_mode(ParseMode::Html)
        .reply_markup(get_chat_keyboard())
        .await?;

    Ok(())
}

async fn get_current_chat_uuid(storage: &Arc<dyn StorageProvider>, user_id: i64) -> Result<String> {
    let config = storage.get_user_config(user_id).await?;
    if let Some(chat_uuid) = config.current_chat_uuid {
        return Ok(chat_uuid);
    }

    ensure_current_chat_uuid(storage, user_id).await
}

fn chat_flow_controls_keyboard(chat_uuid: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback("Attach", format!("{CHAT_ATTACH_PREFIX}{chat_uuid}")),
        InlineKeyboardButton::callback("Detach", CHAT_DETACH_CALLBACK),
    ]])
}

async fn send_chat_flow_controls(bot: &Bot, chat_id: ChatId, chat_uuid: &str) -> Result<()> {
    bot.send_message(chat_id, "Flow controls:")
        .reply_markup(chat_flow_controls_keyboard(chat_uuid))
        .await?;
    Ok(())
}

fn is_valid_chat_uuid(uuid: &str) -> bool {
    if uuid.len() != 36 {
        return false;
    }

    for (idx, ch) in uuid.chars().enumerate() {
        let is_hyphen_pos = matches!(idx, 8 | 13 | 18 | 23);
        if is_hyphen_pos {
            if ch != '-' {
                return false;
            }
            continue;
        }

        if !ch.is_ascii_hexdigit() {
            return false;
        }
    }

    true
}

#[derive(Debug, PartialEq, Eq)]
enum ChatFlowCallbackData<'a> {
    Attach(&'a str),
    Detach,
}

fn parse_chat_flow_callback_data(data: &str) -> Option<ChatFlowCallbackData<'_>> {
    if data == CHAT_DETACH_CALLBACK {
        return Some(ChatFlowCallbackData::Detach);
    }

    data.strip_prefix(CHAT_ATTACH_PREFIX)
        .map(ChatFlowCallbackData::Attach)
}

fn short_uuid(uuid: &str) -> String {
    uuid.chars().take(8).collect()
}

/// Handle chat flow Attach/Detach inline callbacks.
///
/// Returns true when callback belongs to chat flow controls.
///
/// # Errors
///
/// Returns an error if storage or Telegram API operations fail.
pub async fn handle_chat_flow_callback(
    bot: &Bot,
    q: &CallbackQuery,
    storage: &Arc<dyn StorageProvider>,
) -> Result<bool> {
    let Some(data) = q.data.as_deref() else {
        return Ok(false);
    };

    let Some(callback_data) = parse_chat_flow_callback_data(data) else {
        return Ok(false);
    };

    let user_id = q.from.id.0.cast_signed();
    let user_state = storage.get_user_state(user_id).await?;
    if user_state.as_deref() != Some("chat_mode") {
        bot.answer_callback_query(q.id.clone())
            .text("Chat Mode only")
            .await?;
        return Ok(true);
    }

    match callback_data {
        ChatFlowCallbackData::Detach => {
            let mut config = storage.get_user_config(user_id).await?;
            let new_chat_uuid = generate_chat_uuid();
            config.current_chat_uuid = Some(new_chat_uuid.clone());
            storage.update_user_config(user_id, config).await?;

            bot.answer_callback_query(q.id.clone())
                .text(format!("Detached: {}", short_uuid(&new_chat_uuid)))
                .await?;
        }
        ChatFlowCallbackData::Attach(selected_uuid) => {
            if !is_valid_chat_uuid(selected_uuid) {
                bot.answer_callback_query(q.id.clone())
                    .text("Invalid chat UUID")
                    .await?;
                return Ok(true);
            }

            let mut config = storage.get_user_config(user_id).await?;
            config.current_chat_uuid = Some(selected_uuid.to_string());
            storage.update_user_config(user_id, config).await?;

            bot.answer_callback_query(q.id.clone())
                .text(format!("Attached: {}", short_uuid(selected_uuid)))
                .await?;
        }
    }

    Ok(true)
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
    dialogue: Dialogue<State, InMemStorage<State>>,
    context: Arc<TelegramHandlerContext>,
) -> Result<()> {
    let text = msg.text().unwrap_or("").to_string();
    let user_id = get_user_id_safe(&msg);
    let user_name = get_user_name(&msg);

    info!(
        "Handling message from user {user_id} ({user_name}). Text: '{}'",
        truncate_str(&text, 100)
    );

    if Box::pin(check_state_and_redirect(&bot, &msg, &dialogue, &context)).await? {
        return Ok(());
    }

    if handle_menu_commands(&bot, &msg, &dialogue, &context, &text).await? {
        return Ok(());
    }

    let state = dialogue.get().await?.unwrap_or(State::Start);
    if matches!(state, State::Start) {
        bot.send_message(msg.chat.id, "Please select a mode:")
            .reply_markup(get_main_keyboard())
            .await?;
        return Ok(());
    }

    if context
        .settings
        .agent
        .get_model_info_by_name(&text)
        .is_some()
    {
        info!("User {user_id} selected model '{text}' via text input.");
        context
            .storage
            .update_user_model(user_id, text.clone())
            .await?;
        bot.send_message(msg.chat.id, format!("Model changed to <b>{text}</b>"))
            .parse_mode(ParseMode::Html)
            .reply_markup(get_chat_keyboard())
            .await?;
        return Ok(());
    }

    process_llm_request(
        bot,
        msg,
        Arc::clone(&context.storage),
        Arc::clone(&context.llm),
        Arc::clone(&context.settings),
        text,
    )
    .await
}

async fn handle_menu_commands(
    bot: &Bot,
    msg: &Message,
    dialogue: &Dialogue<State, InMemStorage<State>>,
    context: &TelegramHandlerContext,
    text: &str,
) -> Result<bool> {
    let user_id = get_user_id_safe(msg);
    match text {
        "💬 Chat Mode" => {
            let _chat_uuid = ensure_current_chat_uuid(&context.storage, user_id).await?;
            // Save state to DB
            let _ = context
                .storage
                .update_user_state(user_id, "chat_mode".to_string())
                .await;
            dialogue
                .update(State::ChatMode)
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
            let saved_model = context.storage.get_user_model(user_id).await?;
            let model = resolve_chat_model(&context.settings, saved_model);
            bot.send_message(
                msg.chat.id,
                format!("<b>Chat mode activated.</b>\nCurrent model: <b>{model}</b>"),
            )
            .parse_mode(ParseMode::Html)
            .reply_markup(get_chat_keyboard())
            .await?;
            Ok(true)
        }
        "Clear Flow" => {
            clear(bot.clone(), msg.clone(), Arc::clone(&context.storage)).await?;
            Ok(true)
        }
        "Change Model" => {
            bot.send_message(msg.chat.id, "Select a model:")
                .reply_markup(get_model_keyboard(&context.settings))
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
            if check_agent_access(bot, msg, &context.settings, user_id).await? {
                crate::bot::agent_handlers::activate_agent_mode(
                    crate::bot::agent_handlers::ActivateAgentModeParams {
                        bot: bot.clone(),
                        msg: msg.clone(),
                        dialogue: dialogue.clone(),
                        context: Arc::new(context.clone()),
                    },
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
    storage: Arc<dyn StorageProvider>,
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
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    settings: Arc<BotSettings>,
    text: String,
) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    let system_prompt = storage
        .get_user_prompt(user_id)
        .await?
        .unwrap_or_else(|| std::env::var("SYSTEM_MESSAGE").unwrap_or_default());
    let chat_uuid = get_current_chat_uuid(&storage, user_id).await?;
    let history = storage
        .get_chat_history_for_chat(user_id, chat_uuid.clone(), 10)
        .await?;
    let saved_model = storage.get_user_model(user_id).await?;
    let model = resolve_chat_model(&settings, saved_model);

    storage
        .save_message_for_chat(user_id, chat_uuid.clone(), "user".to_string(), text.clone())
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
                .save_message_for_chat(
                    user_id,
                    chat_uuid.clone(),
                    "assistant".to_string(),
                    response.clone(),
                )
                .await?;
            send_long_message(&bot, msg.chat.id, &response).await?;
            send_chat_flow_controls(&bot, msg.chat.id, &chat_uuid).await?;
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
    dialogue: Dialogue<State, InMemStorage<State>>,
    context: Arc<TelegramHandlerContext>,
) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    if Box::pin(check_state_and_redirect(&bot, &msg, &dialogue, &context)).await? {
        return Ok(());
    }

    let state = dialogue.get().await?.unwrap_or(State::Start);
    if matches!(state, State::Start) {
        bot.send_message(msg.chat.id, "Please select a mode:")
            .reply_markup(get_main_keyboard())
            .await?;
        return Ok(());
    }

    if !context.llm.is_multimodal_available() {
        bot.send_message(
            msg.chat.id,
            "🚫 Feature unavailable.\nMedia processing is disabled because the Gemini or OpenRouter provider is not configured.",
        )
        .await?;
        return Ok(());
    }

    let voice = msg.voice().ok_or_else(|| anyhow!("No voice found"))?;
    let saved_model = context.storage.get_user_model(user_id).await?;
    let model = resolve_chat_model(&context.settings, saved_model);

    let provider_info = context.settings.agent.get_model_info_by_name(&model);
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
    match context
        .llm
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
                process_llm_request(
                    bot,
                    msg,
                    Arc::clone(&context.storage),
                    Arc::clone(&context.llm),
                    Arc::clone(&context.settings),
                    text,
                )
                .await?;
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
    dialogue: Dialogue<State, InMemStorage<State>>,
    context: Arc<TelegramHandlerContext>,
) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    if Box::pin(check_state_and_redirect(&bot, &msg, &dialogue, &context)).await? {
        return Ok(());
    }

    let state = dialogue.get().await?.unwrap_or(State::Start);
    if matches!(state, State::Start) {
        bot.send_message(msg.chat.id, "Please select a mode:")
            .reply_markup(get_main_keyboard())
            .await?;
        return Ok(());
    }

    if !context.llm.is_multimodal_available() {
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
    let saved_model = context.storage.get_user_model(user_id).await?;
    let model = resolve_chat_model(&context.settings, saved_model);
    let system_prompt = context
        .storage
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
    match context
        .llm
        .analyze_image(buffer, caption, &system_prompt, &model)
        .await
    {
        Ok(response) => {
            let chat_uuid = get_current_chat_uuid(&context.storage, user_id).await?;
            context
                .storage
                .save_message_for_chat(
                    user_id,
                    chat_uuid.clone(),
                    "user".to_string(),
                    format!("[Image] {caption}"),
                )
                .await?;
            context
                .storage
                .save_message_for_chat(
                    user_id,
                    chat_uuid.clone(),
                    "assistant".to_string(),
                    response.clone(),
                )
                .await?;
            send_long_message(&bot, msg.chat.id, &response).await?;
            send_chat_flow_controls(&bot, msg.chat.id, &chat_uuid).await?;
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
    context: Arc<TelegramHandlerContext>,
) -> Result<()> {
    if should_route_document_to_agent(get_user_id_safe(&msg), &dialogue, &context).await? {
        Box::pin(super::agent_handlers::handle_agent_message(
            bot, msg, dialogue, context,
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

async fn should_route_document_to_agent(
    user_id: i64,
    dialogue: &Dialogue<State, InMemStorage<State>>,
    context: &TelegramHandlerContext,
) -> Result<bool> {
    if matches!(
        restore_persisted_dialogue_state(user_id, dialogue, context).await?,
        Some(State::AgentMode)
    ) {
        return Ok(true);
    }

    Ok(matches!(
        dialogue.get().await?.unwrap_or(State::Start),
        State::AgentMode
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        is_valid_chat_uuid, parse_chat_flow_callback_data, prepare_start_state,
        should_route_document_to_agent, ChatFlowCallbackData, CHAT_ATTACH_PREFIX,
        CHAT_DETACH_CALLBACK,
    };
    use crate::bot::agent_handlers::{AgentTaskRuntime, StartResetOutcome};
    use crate::bot::context::TelegramHandlerContext;
    use crate::bot::state::State;
    use crate::config::{BotSettings, TelegramSettings};
    use anyhow::Result as AnyResult;
    use async_trait::async_trait;
    use oxide_agent_core::agent::{AgentMemory, SessionId, TaskEvent, TaskId, TaskSnapshot};
    use oxide_agent_core::config::AgentSettings;
    use oxide_agent_core::llm::LlmClient;
    use oxide_agent_core::storage::{Message, StorageError, StorageProvider, UserConfig};
    use oxide_agent_runtime::{
        TaskEventBroadcaster, TaskEventBroadcasterOptions, TaskExecutionBackend,
        TaskExecutionOutcome, TaskExecutionRequest, TaskRegistry,
    };
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use teloxide::dispatching::dialogue::{Dialogue, InMemStorage};
    use teloxide::types::ChatId;
    use tokio::sync::{Mutex, Notify};
    use tokio::time::{timeout, Duration};

    #[test]
    fn is_valid_chat_uuid_accepts_canonical_uuid() {
        assert!(is_valid_chat_uuid("123e4567-e89b-12d3-a456-426614174000"));
    }

    #[test]
    fn is_valid_chat_uuid_rejects_invalid_length() {
        assert!(!is_valid_chat_uuid("123e4567-e89b-12d3-a456-42661417400"));
    }

    #[test]
    fn is_valid_chat_uuid_rejects_wrong_hyphen_positions() {
        assert!(!is_valid_chat_uuid("123e4567e-89b-12d3-a456-426614174000"));
    }

    #[test]
    fn is_valid_chat_uuid_rejects_non_hex_characters() {
        assert!(!is_valid_chat_uuid("123e4567-e89b-12d3-a456-42661417400z"));
    }

    #[test]
    fn parse_chat_flow_callback_data_parses_detach() {
        assert_eq!(
            parse_chat_flow_callback_data(CHAT_DETACH_CALLBACK),
            Some(ChatFlowCallbackData::Detach)
        );
    }

    #[test]
    fn parse_chat_flow_callback_data_parses_attach_payload() {
        let callback = "chat_attach:123e4567-e89b-12d3-a456-426614174000";
        assert_eq!(
            parse_chat_flow_callback_data(callback),
            Some(ChatFlowCallbackData::Attach(
                "123e4567-e89b-12d3-a456-426614174000"
            ))
        );
    }

    #[test]
    fn parse_chat_flow_callback_data_treats_empty_attach_payload_as_attach() {
        assert_eq!(
            parse_chat_flow_callback_data(CHAT_ATTACH_PREFIX),
            Some(ChatFlowCallbackData::Attach(""))
        );
    }

    #[test]
    fn parse_chat_flow_callback_data_rejects_unknown_callback() {
        assert_eq!(parse_chat_flow_callback_data("unknown"), None);
    }

    #[derive(Default)]
    struct StartTestStorage {
        user_states: Mutex<HashMap<i64, String>>,
        snapshots: Mutex<HashMap<TaskId, TaskSnapshot>>,
        fail_state_update: Mutex<bool>,
    }

    struct BlockingBackend {
        started: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl TaskExecutionBackend for BlockingBackend {
        async fn execute(&self, request: TaskExecutionRequest) -> AnyResult<TaskExecutionOutcome> {
            self.started.notify_one();
            request.cancellation_token.cancelled().await;
            self.release.notify_one();
            Ok(TaskExecutionOutcome::Completed)
        }
    }

    #[async_trait]
    impl StorageProvider for StartTestStorage {
        async fn get_user_config(&self, _user_id: i64) -> Result<UserConfig, StorageError> {
            Ok(UserConfig::default())
        }

        async fn update_user_config(
            &self,
            _user_id: i64,
            _config: UserConfig,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn update_user_prompt(
            &self,
            _user_id: i64,
            _system_prompt: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_prompt(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn update_user_model(
            &self,
            _user_id: i64,
            _model_name: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_model(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn update_user_state(&self, user_id: i64, state: String) -> Result<(), StorageError> {
            if *self.fail_state_update.lock().await {
                return Err(StorageError::Config("state update failed".to_string()));
            }
            self.user_states.lock().await.insert(user_id, state);
            Ok(())
        }

        async fn get_user_state(&self, user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(self.user_states.lock().await.get(&user_id).cloned())
        }

        async fn save_message(
            &self,
            _user_id: i64,
            _role: String,
            _content: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_chat_history(
            &self,
            _user_id: i64,
            _limit: usize,
        ) -> Result<Vec<Message>, StorageError> {
            Ok(Vec::new())
        }

        async fn clear_chat_history(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_message_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
            _role: String,
            _content: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_chat_history_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
            _limit: usize,
        ) -> Result<Vec<Message>, StorageError> {
            Ok(Vec::new())
        }

        async fn clear_chat_history_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_agent_memory(
            &self,
            _user_id: i64,
            _memory: &AgentMemory,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn load_agent_memory(
            &self,
            _user_id: i64,
        ) -> Result<Option<AgentMemory>, StorageError> {
            Ok(None)
        }

        async fn clear_agent_memory(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn clear_all_context(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_task_snapshot(&self, snapshot: &TaskSnapshot) -> Result<(), StorageError> {
            self.snapshots
                .lock()
                .await
                .insert(snapshot.metadata.id, snapshot.clone());
            Ok(())
        }

        async fn load_task_snapshot(
            &self,
            task_id: TaskId,
        ) -> Result<Option<TaskSnapshot>, StorageError> {
            Ok(self.snapshots.lock().await.get(&task_id).cloned())
        }

        async fn list_task_snapshots(&self) -> Result<Vec<TaskSnapshot>, StorageError> {
            Ok(self.snapshots.lock().await.values().cloned().collect())
        }

        async fn append_task_event(
            &self,
            _task_id: TaskId,
            _event: TaskEvent,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn load_task_events(&self, _task_id: TaskId) -> Result<Vec<TaskEvent>, StorageError> {
            Ok(Vec::new())
        }

        async fn check_connection(&self) -> Result<(), String> {
            Ok(())
        }
    }

    fn test_context(
        storage: Arc<dyn StorageProvider>,
        task_runtime: Arc<AgentTaskRuntime>,
    ) -> TelegramHandlerContext {
        test_context_with_telegram_settings(storage, task_runtime, TelegramSettings::default())
    }

    fn test_context_with_telegram_settings(
        storage: Arc<dyn StorageProvider>,
        task_runtime: Arc<AgentTaskRuntime>,
        telegram_settings: TelegramSettings,
    ) -> TelegramHandlerContext {
        let agent_settings = AgentSettings {
            openrouter_site_name: "Oxide Agent Bot".to_string(),
            ..AgentSettings::default()
        };
        let llm_settings = Arc::new(agent_settings.clone());
        let llm = Arc::new(LlmClient::new(&llm_settings));

        TelegramHandlerContext {
            storage: Arc::clone(&storage),
            llm,
            settings: Arc::new(BotSettings::new(agent_settings, telegram_settings)),
            task_runtime,
            task_events: Arc::new(TaskEventBroadcaster::new(TaskEventBroadcasterOptions::new(
                storage,
            ))),
            observer_access: None,
            web_observer_ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            task_watchers: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
        }
    }

    #[tokio::test]
    async fn task_runtime_start_prepare_blocks_stale_reset_after_submit_admission() {
        let storage = Arc::new(StartTestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let context = test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        );
        let user_id = 71;
        let session_id = SessionId::from(user_id);

        let dialogue_storage = InMemStorage::<State>::new();
        let dialogue = Dialogue::new(Arc::clone(&dialogue_storage), ChatId(user_id));
        let update_result = dialogue.update(State::AgentMode).await;
        assert!(update_result.is_ok());
        let storage_result = storage
            .update_user_state(user_id, "agent_mode".to_string())
            .await;
        assert!(storage_result.is_ok());

        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let backend = Arc::new(BlockingBackend {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        });

        let submit_result = task_runtime
            .submit_task(session_id, "live task".to_string(), backend)
            .await;
        assert!(submit_result.is_ok());
        assert!(timeout(Duration::from_secs(1), started.notified())
            .await
            .is_ok());

        let outcome = prepare_start_state(user_id, session_id, &dialogue, &context).await;
        assert!(matches!(outcome, Ok(StartResetOutcome::BlockedByTask)));

        let state = dialogue.get().await;
        assert!(matches!(state, Ok(Some(State::AgentMode))));
        let persisted_state = storage.get_user_state(user_id).await;
        assert!(matches!(persisted_state, Ok(Some(ref state)) if state == "agent_mode"));

        let cancelled = task_runtime.cancel_task_for_session(session_id).await;
        assert!(cancelled.is_ok());
        assert!(timeout(Duration::from_secs(1), release.notified())
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn task_runtime_start_prepare_keeps_agent_mode_when_chat_state_persist_fails() {
        let storage = Arc::new(StartTestStorage::default());
        *storage.fail_state_update.lock().await = true;

        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            task_registry,
            1,
        ));
        let context = test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        );

        let user_id = 72;
        let session_id = SessionId::from(user_id);
        let dialogue_storage = InMemStorage::<State>::new();
        let dialogue = Dialogue::new(Arc::clone(&dialogue_storage), ChatId(user_id));

        let update_result = dialogue.update(State::AgentMode).await;
        assert!(update_result.is_ok());

        let outcome = prepare_start_state(user_id, session_id, &dialogue, &context).await;
        assert!(outcome.is_err());

        let state = dialogue.get().await;
        assert!(matches!(state, Ok(Some(State::AgentMode))));

        let persisted_state = storage.get_user_state(user_id).await;
        assert!(matches!(persisted_state, Ok(None)));
    }

    #[tokio::test]
    async fn task_runtime_document_restore_reenters_agent_mode_from_start_state() {
        let storage = Arc::new(StartTestStorage::default());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::new(TaskRegistry::new()),
            1,
        ));
        let context = test_context_with_telegram_settings(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            task_runtime,
            TelegramSettings {
                agent_allowed_users_str: Some("73".to_string()),
                ..TelegramSettings::default()
            },
        );

        let user_id = 73;
        let storage_result = storage
            .update_user_state(user_id, "agent_mode".to_string())
            .await;
        assert!(storage_result.is_ok());

        let dialogue_storage = InMemStorage::<State>::new();
        let dialogue = Dialogue::new(Arc::clone(&dialogue_storage), ChatId(user_id));
        let initial_state = dialogue.get().await;
        assert!(matches!(initial_state, Ok(None)));

        let restored = super::restore_persisted_dialogue_state(user_id, &dialogue, &context).await;
        assert!(matches!(restored, Ok(Some(State::AgentMode))));
        let state = dialogue.get().await;
        assert!(matches!(state, Ok(Some(State::AgentMode))));
    }

    #[tokio::test]
    async fn task_runtime_document_route_restores_agent_mode_before_fallback_branch() {
        let storage = Arc::new(StartTestStorage::default());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::new(TaskRegistry::new()),
            1,
        ));
        let context = test_context_with_telegram_settings(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            task_runtime,
            TelegramSettings {
                agent_allowed_users_str: Some("74".to_string()),
                ..TelegramSettings::default()
            },
        );

        let user_id = 74;
        let storage_result = storage
            .update_user_state(user_id, "agent_mode".to_string())
            .await;
        assert!(storage_result.is_ok());

        let dialogue_storage = InMemStorage::<State>::new();
        let dialogue = Dialogue::new(Arc::clone(&dialogue_storage), ChatId(user_id));
        let initial_state = dialogue.get().await;
        assert!(matches!(initial_state, Ok(None)));

        let should_route = should_route_document_to_agent(user_id, &dialogue, &context).await;
        assert!(matches!(should_route, Ok(true)));

        let state = dialogue.get().await;
        assert!(matches!(state, Ok(Some(State::AgentMode))));
    }

    #[tokio::test]
    async fn task_runtime_restore_downgrades_revoked_agent_persisted_state_for_message_paths() {
        let storage = Arc::new(StartTestStorage::default());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::new(TaskRegistry::new()),
            1,
        ));
        let context = test_context_with_telegram_settings(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            task_runtime,
            TelegramSettings {
                agent_allowed_users_str: Some("999".to_string()),
                ..TelegramSettings::default()
            },
        );

        let user_id = 75;
        let storage_result = storage
            .update_user_state(user_id, "agent_mode".to_string())
            .await;
        assert!(storage_result.is_ok());

        let dialogue_storage = InMemStorage::<State>::new();
        let dialogue = Dialogue::new(Arc::clone(&dialogue_storage), ChatId(user_id));
        let restored = super::restore_persisted_dialogue_state(user_id, &dialogue, &context).await;
        assert!(matches!(restored, Ok(Some(State::ChatMode))));

        let state = dialogue.get().await;
        assert!(matches!(state, Ok(Some(State::ChatMode))));

        let persisted_state = storage.get_user_state(user_id).await;
        assert!(matches!(persisted_state, Ok(Some(ref state)) if state == "chat_mode"));
    }

    #[tokio::test]
    async fn task_runtime_document_route_denies_revoked_agent_persisted_state() {
        let storage = Arc::new(StartTestStorage::default());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::new(TaskRegistry::new()),
            1,
        ));
        let context = test_context_with_telegram_settings(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            task_runtime,
            TelegramSettings {
                agent_allowed_users_str: Some("999".to_string()),
                ..TelegramSettings::default()
            },
        );

        let user_id = 76;
        let storage_result = storage
            .update_user_state(user_id, "agent_mode".to_string())
            .await;
        assert!(storage_result.is_ok());

        let dialogue_storage = InMemStorage::<State>::new();
        let dialogue = Dialogue::new(Arc::clone(&dialogue_storage), ChatId(user_id));
        let update_result = dialogue.update(State::AgentMode).await;
        assert!(update_result.is_ok());

        let should_route = should_route_document_to_agent(user_id, &dialogue, &context).await;
        assert!(matches!(should_route, Ok(false)));

        let state = dialogue.get().await;
        assert!(matches!(state, Ok(Some(State::ChatMode))));

        let persisted_state = storage.get_user_state(user_id).await;
        assert!(matches!(persisted_state, Ok(Some(ref state)) if state == "chat_mode"));
    }
}
