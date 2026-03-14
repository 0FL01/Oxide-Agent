use crate::bot::state::State;
use crate::bot::topic_route::{resolve_topic_route, touch_dynamic_binding_activity_if_needed};
use crate::bot::UnauthorizedCache;
use crate::bot::{build_outbound_thread_params, resolve_thread_spec, OutboundThreadParams};
use crate::config::BotSettings;
use anyhow::{anyhow, Result};
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
    storage: &Arc<dyn StorageProvider>,
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
        } else if state_str == "chat_mode" {
            info!("Restoring chat mode for user {user_id} based on persisted state.");
            dialogue
                .update(State::ChatMode)
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
            return Ok(false);
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

/// Start handler
///
/// # Errors
///
/// Returns an error if the welcome message cannot be sent.
pub async fn start(
    bot: Bot,
    msg: Message,
    storage: Arc<dyn StorageProvider>,
    settings: Arc<BotSettings>,
    dialogue: Dialogue<State, InMemStorage<State>>,
) -> Result<()> {
    let outbound_thread = outbound_thread_from_message(&msg);
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
    let mut req = bot
        .send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.reply_markup(get_main_keyboard()).await?;

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
    let outbound_thread = outbound_thread_from_message(&msg);
    let user_id = get_user_id_safe(&msg);
    let user_name = get_user_name(&msg);

    info!("User {user_id} ({user_name}) initiated flow clear.");

    let mut config = storage.get_user_config(user_id).await?;
    let new_chat_uuid = generate_chat_uuid();
    config.current_chat_uuid = Some(new_chat_uuid.clone());
    storage.update_user_config(user_id, config).await?;

    info!("Started new chat flow for user {user_id}: {new_chat_uuid}");
    let mut req = bot
        .send_message(msg.chat.id, "<b>Flow cleared.</b>")
        .parse_mode(ParseMode::Html);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.reply_markup(get_chat_keyboard()).await?;

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

async fn send_chat_flow_controls_in_thread(
    bot: &Bot,
    chat_id: ChatId,
    chat_uuid: &str,
    outbound_thread: OutboundThreadParams,
) -> Result<()> {
    let mut req = bot.send_message(chat_id, "Flow controls:");
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.reply_markup(chat_flow_controls_keyboard(chat_uuid))
        .await?;
    Ok(())
}

fn outbound_thread_from_message(msg: &Message) -> OutboundThreadParams {
    build_outbound_thread_params(resolve_thread_spec(msg))
}

struct ChatRequestOptions {
    text: String,
    outbound_thread: OutboundThreadParams,
    topic_system_prompt_override: Option<String>,
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
    let outbound_thread = outbound_thread_from_message(&msg);
    let user_id = get_user_id_safe(&msg);
    info!("Healthcheck command received from user {user_id}.");
    let mut req = bot.send_message(msg.chat.id, "OK");
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.await?;
    info!("Responded 'OK' to healthcheck from user {user_id}.");
    Ok(())
}

/// Stats handler - shows bot statistics including unauthorized cache metrics
///
/// # Errors
///
/// Returns an error if the stats response cannot be sent.
pub async fn stats(bot: Bot, msg: Message, cache: Arc<UnauthorizedCache>) -> Result<()> {
    let outbound_thread = outbound_thread_from_message(&msg);
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

    let mut req = bot
        .send_message(msg.chat.id, stats_text)
        .parse_mode(ParseMode::Html);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.await?;

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
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
    settings: Arc<BotSettings>,
) -> Result<()> {
    let text = msg.text().unwrap_or("").to_string();
    let outbound_thread = outbound_thread_from_message(&msg);
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

    let route = resolve_topic_route(&bot, storage.as_ref(), user_id, &settings, &msg).await;

    if !route.allows_processing() {
        info!(
            "Skipping text message in topic route for user {user_id}. enabled={}, require_mention={}, mention_satisfied={}",
            route.enabled, route.require_mention, route.mention_satisfied
        );
        return Ok(());
    }

    if handle_menu_commands(&bot, &msg, &storage, &llm, &dialogue, &settings, &text).await? {
        touch_dynamic_binding_activity_if_needed(storage.as_ref(), user_id, &route).await;
        return Ok(());
    }

    let state = dialogue.get().await?.unwrap_or(State::Start);
    if matches!(state, State::Start) {
        let mut req = bot.send_message(msg.chat.id, "Please select a mode:");
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.reply_markup(get_main_keyboard()).await?;
        touch_dynamic_binding_activity_if_needed(storage.as_ref(), user_id, &route).await;
        return Ok(());
    }

    if settings.agent.get_model_info_by_name(&text).is_some() {
        info!("User {user_id} selected model '{text}' via text input.");
        storage.update_user_model(user_id, text.clone()).await?;
        let mut req = bot
            .send_message(msg.chat.id, format!("Model changed to <b>{text}</b>"))
            .parse_mode(ParseMode::Html);
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.reply_markup(get_chat_keyboard()).await?;
        touch_dynamic_binding_activity_if_needed(storage.as_ref(), user_id, &route).await;
        return Ok(());
    }

    let result = process_llm_request(
        bot,
        msg,
        storage.clone(),
        llm,
        settings,
        ChatRequestOptions {
            text,
            outbound_thread,
            topic_system_prompt_override: route.system_prompt_override.clone(),
        },
    )
    .await;
    if result.is_ok() {
        touch_dynamic_binding_activity_if_needed(storage.as_ref(), user_id, &route).await;
    }
    result
}

async fn handle_menu_commands(
    bot: &Bot,
    msg: &Message,
    storage: &Arc<dyn StorageProvider>,
    llm: &Arc<LlmClient>,
    dialogue: &Dialogue<State, InMemStorage<State>>,
    settings: &Arc<BotSettings>,
    text: &str,
) -> Result<bool> {
    let outbound_thread = outbound_thread_from_message(msg);
    let user_id = get_user_id_safe(msg);

    match text {
        "💬 Chat Mode" => {
            activate_chat_mode(bot, msg, storage, dialogue, settings, user_id).await
        }
        "Clear Flow" => {
            clear(bot.clone(), msg.clone(), storage.clone()).await?;
            Ok(true)
        }
        "Change Model" => {
            send_menu_keyboard(
                bot,
                msg.chat.id,
                "Select a model:",
                get_model_keyboard(settings),
                outbound_thread,
            )
            .await?;
            Ok(true)
        }
        "Extra Functions" => {
            send_menu_keyboard(
                bot,
                msg.chat.id,
                "Select an action:",
                get_extra_functions_keyboard(),
                outbound_thread,
            )
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
        "Edit Prompt" => begin_prompt_editing(bot, msg.chat.id, dialogue, outbound_thread).await,
        "Back" => handle_back_command(bot, msg.chat.id, dialogue, outbound_thread).await,
        "⬅️ Exit Agent Mode" | "❌ Cancel Task" | "🗑 Clear Memory" => {
            let response = if text == "⬅️ Exit Agent Mode" {
                "👋 Exited agent mode"
            } else if text == "❌ Cancel Task" {
                "No active task to cancel."
            } else {
                "Agent memory is not active."
            };
            send_menu_keyboard(
                bot,
                msg.chat.id,
                response,
                get_main_keyboard(),
                outbound_thread,
            )
            .await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn activate_chat_mode(
    bot: &Bot,
    msg: &Message,
    storage: &Arc<dyn StorageProvider>,
    dialogue: &Dialogue<State, InMemStorage<State>>,
    settings: &Arc<BotSettings>,
    user_id: i64,
) -> Result<bool> {
    let outbound_thread = outbound_thread_from_message(msg);
    let _chat_uuid = ensure_current_chat_uuid(storage, user_id).await?;
    let _ = storage
        .update_user_state(user_id, "chat_mode".to_string())
        .await;
    dialogue
        .update(State::ChatMode)
        .await
        .map_err(|e| anyhow!(e.to_string()))?;
    let saved_model = storage.get_user_model(user_id).await?;
    let model = resolve_chat_model(settings, saved_model);

    let mut req = bot
        .send_message(
            msg.chat.id,
            format!("<b>Chat mode activated.</b>\nCurrent model: <b>{model}</b>"),
        )
        .parse_mode(ParseMode::Html);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.reply_markup(get_chat_keyboard()).await?;
    Ok(true)
}

async fn send_menu_keyboard(
    bot: &Bot,
    chat_id: ChatId,
    text: impl Into<String>,
    keyboard: KeyboardMarkup,
    outbound_thread: OutboundThreadParams,
) -> Result<()> {
    let mut req = bot.send_message(chat_id, text);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.reply_markup(keyboard).await?;
    Ok(())
}

async fn send_multimodal_unavailable_message(
    bot: &Bot,
    msg: &Message,
    outbound_thread: OutboundThreadParams,
) -> Result<()> {
    let mut req = bot.send_message(
        msg.chat.id,
        "🚫 Feature unavailable.\nMedia processing is disabled because the Gemini or OpenRouter provider is not configured.",
    );
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.await?;
    Ok(())
}

async fn begin_prompt_editing(
    bot: &Bot,
    chat_id: ChatId,
    dialogue: &Dialogue<State, InMemStorage<State>>,
    outbound_thread: OutboundThreadParams,
) -> Result<bool> {
    dialogue
        .update(State::EditingPrompt)
        .await
        .map_err(|e| anyhow!(e.to_string()))?;
    send_menu_keyboard(
        bot,
        chat_id,
        "Enter a new system prompt. To cancel, type 'Back':",
        get_extra_functions_keyboard(),
        outbound_thread,
    )
    .await?;
    Ok(true)
}

async fn handle_back_command(
    bot: &Bot,
    chat_id: ChatId,
    dialogue: &Dialogue<State, InMemStorage<State>>,
    outbound_thread: OutboundThreadParams,
) -> Result<bool> {
    let state = dialogue.get().await?.unwrap_or(State::Start);
    if matches!(state, State::ChatMode) || matches!(state, State::EditingPrompt) {
        dialogue
            .update(State::Start)
            .await
            .map_err(|e| anyhow!(e.to_string()))?;
    }

    send_menu_keyboard(
        bot,
        chat_id,
        "Please select a mode:",
        get_main_keyboard(),
        outbound_thread,
    )
    .await?;
    Ok(true)
}

async fn check_agent_access(
    bot: &Bot,
    msg: &Message,
    settings: &Arc<BotSettings>,
    user_id: i64,
) -> Result<bool> {
    let outbound_thread = outbound_thread_from_message(msg);
    let agent_allowed = settings.telegram.agent_allowed_users();
    if !agent_allowed.contains(&user_id) && !agent_allowed.is_empty() {
        let mut req = bot.send_message(
            msg.chat.id,
            "⛔️ You do not have permission to access agent mode.",
        );
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.await?;
        return Ok(false);
    } else if agent_allowed.is_empty() {
        let mut req = bot.send_message(
            msg.chat.id,
            "⛔️ Agent mode is temporarily unavailable (access not configured).",
        );
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.await?;
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
    let outbound_thread = outbound_thread_from_message(&msg);
    let text = msg.text().unwrap_or("");
    let user_id = get_user_id_safe(&msg);

    if text == "Back" {
        dialogue
            .update(State::ChatMode)
            .await
            .map_err(|e| anyhow!(e.to_string()))?;
        let mut req = bot.send_message(msg.chat.id, "System prompt update canceled.");
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.reply_markup(get_chat_keyboard()).await?;
    } else {
        storage
            .update_user_prompt(user_id, text.to_string())
            .await?;
        dialogue
            .update(State::ChatMode)
            .await
            .map_err(|e| anyhow!(e.to_string()))?;
        let mut req = bot.send_message(msg.chat.id, "System prompt updated.");
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.reply_markup(get_chat_keyboard()).await?;
    }
    Ok(())
}

async fn process_llm_request(
    bot: Bot,
    msg: Message,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    settings: Arc<BotSettings>,
    options: ChatRequestOptions,
) -> Result<()> {
    let user_id = get_user_id_safe(&msg);
    let system_prompt = resolve_system_prompt(
        &storage,
        user_id,
        options.topic_system_prompt_override.as_deref(),
    )
    .await?;
    let chat_uuid = get_current_chat_uuid(&storage, user_id).await?;
    let history = storage
        .get_chat_history_for_chat(user_id, chat_uuid.clone(), 10)
        .await?;
    let saved_model = storage.get_user_model(user_id).await?;
    let model = resolve_chat_model(&settings, saved_model);

    storage
        .save_message_for_chat(
            user_id,
            chat_uuid.clone(),
            "user".to_string(),
            options.text.clone(),
        )
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
        .chat_completion(&system_prompt, &llm_history, &options.text, &model)
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
            send_long_message_in_thread(
                &bot,
                msg.chat.id,
                &response,
                options.outbound_thread.message_thread_id,
            )
            .await?;
            send_chat_flow_controls_in_thread(
                &bot,
                msg.chat.id,
                &chat_uuid,
                options.outbound_thread,
            )
            .await?;
        }
        Err(e) => {
            let mut req = bot
                .send_message(msg.chat.id, format!("<b>Error:</b> {e}"))
                .parse_mode(ParseMode::Html);
            if let Some(thread_id) = options.outbound_thread.message_thread_id {
                req = req.message_thread_id(thread_id);
            }

            req.await?;
        }
    }
    Ok(())
}

/// Re-export the shared long message sender for convenience.
use super::messaging::send_long_message_in_thread;

/// Voice message handler
///
/// # Errors
///
/// Returns an error if the voice message cannot be processed.
pub async fn handle_voice(
    bot: Bot,
    msg: Message,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
    settings: Arc<BotSettings>,
) -> Result<()> {
    let outbound_thread = outbound_thread_from_message(&msg);
    let user_id = get_user_id_safe(&msg);
    let route = resolve_topic_route(&bot, storage.as_ref(), user_id, &settings, &msg).await;
    if !route.allows_processing() {
        info!(
            "Skipping voice message in topic route for user {user_id}. enabled={}, require_mention={}, mention_satisfied={}",
            route.enabled, route.require_mention, route.mention_satisfied
        );
        return Ok(());
    }

    if Box::pin(check_state_and_redirect(
        &bot, &msg, &storage, &llm, &dialogue, &settings,
    ))
    .await?
    {
        return Ok(());
    }

    let state = dialogue.get().await?.unwrap_or(State::Start);
    if matches!(state, State::Start) {
        let mut req = bot.send_message(msg.chat.id, "Please select a mode:");
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.reply_markup(get_main_keyboard()).await?;
        return Ok(());
    }

    if !llm.is_multimodal_available() {
        send_multimodal_unavailable_message(&bot, &msg, outbound_thread).await?;
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
                let mut req = bot.send_message(msg.chat.id, "Failed to recognize speech.");
                if let Some(thread_id) = outbound_thread.message_thread_id {
                    req = req.message_thread_id(thread_id);
                }

                req.await?;
            } else {
                let mut req = bot.send_message(
                    msg.chat.id,
                    format!("Recognized: \"{text}\"\n\nProcessing request..."),
                );
                if let Some(thread_id) = outbound_thread.message_thread_id {
                    req = req.message_thread_id(thread_id);
                }

                req.await?;
                process_llm_request(
                    bot,
                    msg,
                    storage.clone(),
                    llm,
                    settings,
                    ChatRequestOptions {
                        text,
                        outbound_thread,
                        topic_system_prompt_override: route.system_prompt_override.clone(),
                    },
                )
                .await?;
                touch_dynamic_binding_activity_if_needed(storage.as_ref(), user_id, &route).await;
            }
        }
        Err(e) => {
            let mut req = bot.send_message(msg.chat.id, format!("Recognition error: {e}"));
            if let Some(thread_id) = outbound_thread.message_thread_id {
                req = req.message_thread_id(thread_id);
            }

            req.await?;
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
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
    settings: Arc<BotSettings>,
) -> Result<()> {
    let outbound_thread = outbound_thread_from_message(&msg);
    let user_id = get_user_id_safe(&msg);
    let route = resolve_topic_route(&bot, storage.as_ref(), user_id, &settings, &msg).await;

    if !route.allows_processing() {
        info!(
            "Skipping photo message in topic route for user {user_id}. enabled={}, require_mention={}, mention_satisfied={}",
            route.enabled, route.require_mention, route.mention_satisfied
        );
        return Ok(());
    }

    if Box::pin(check_state_and_redirect(
        &bot, &msg, &storage, &llm, &dialogue, &settings,
    ))
    .await?
    {
        return Ok(());
    }

    let state = dialogue.get().await?.unwrap_or(State::Start);
    if matches!(state, State::Start) {
        let mut req = bot.send_message(msg.chat.id, "Please select a mode:");
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.reply_markup(get_main_keyboard()).await?;
        return Ok(());
    }

    if !llm.is_multimodal_available() {
        send_multimodal_unavailable_message(&bot, &msg, outbound_thread).await?;
        return Ok(());
    }

    let photo = msg
        .photo()
        .and_then(|p| p.last())
        .ok_or_else(|| anyhow!("No photo found"))?;
    let caption = msg.caption().unwrap_or("Describe this image.");
    let saved_model = storage.get_user_model(user_id).await?;
    let model = resolve_chat_model(&settings, saved_model);
    let system_prompt =
        resolve_system_prompt(&storage, user_id, route.system_prompt_override.as_deref()).await?;

    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::UploadPhoto)
        .await?;

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
            let chat_uuid = get_current_chat_uuid(&storage, user_id).await?;
            storage
                .save_message_for_chat(
                    user_id,
                    chat_uuid.clone(),
                    "user".to_string(),
                    format!("[Image] {caption}"),
                )
                .await?;
            storage
                .save_message_for_chat(
                    user_id,
                    chat_uuid.clone(),
                    "assistant".to_string(),
                    response.clone(),
                )
                .await?;
            send_long_message_in_thread(
                &bot,
                msg.chat.id,
                &response,
                outbound_thread.message_thread_id,
            )
            .await?;
            send_chat_flow_controls_in_thread(&bot, msg.chat.id, &chat_uuid, outbound_thread)
                .await?;
            touch_dynamic_binding_activity_if_needed(storage.as_ref(), user_id, &route).await;
        }
        Err(e) => {
            let mut req = bot.send_message(msg.chat.id, format!("Image analysis error: {e}"));
            if let Some(thread_id) = outbound_thread.message_thread_id {
                req = req.message_thread_id(thread_id);
            }

            req.await?;
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
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    settings: Arc<BotSettings>,
) -> Result<()> {
    let outbound_thread = outbound_thread_from_message(&msg);
    let user_id = get_user_id_safe(&msg);
    let route = resolve_topic_route(&bot, storage.as_ref(), user_id, &settings, &msg).await;

    if !route.allows_processing() {
        return Ok(());
    }

    let state = dialogue.get().await?.unwrap_or(State::Start);

    if matches!(state, State::AgentMode) {
        let result = Box::pin(super::agent_handlers::handle_agent_message(
            bot,
            msg,
            storage.clone(),
            llm,
            dialogue,
            settings,
        ))
        .await;
        if result.is_ok() {
            touch_dynamic_binding_activity_if_needed(storage.as_ref(), user_id, &route).await;
        }
        result
    } else {
        let mut req = bot.send_message(
            msg.chat.id,
            "📁 File upload is available only in Agent Mode.\n\n\
             Use /agent to activate.",
        );
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.await?;
        Ok(())
    }
}

async fn resolve_system_prompt(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    topic_override: Option<&str>,
) -> Result<String> {
    let user_prompt = storage.get_user_prompt(user_id).await?;
    let env_prompt = std::env::var("SYSTEM_MESSAGE").ok();
    Ok(pick_system_prompt(topic_override, user_prompt, env_prompt))
}

fn pick_system_prompt(
    topic_override: Option<&str>,
    user_prompt: Option<String>,
    env_prompt: Option<String>,
) -> String {
    if let Some(topic_prompt) = topic_override {
        return topic_prompt.to_string();
    }

    if let Some(prompt) = user_prompt {
        return prompt;
    }

    env_prompt.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        is_valid_chat_uuid, parse_chat_flow_callback_data, pick_system_prompt,
        ChatFlowCallbackData, CHAT_ATTACH_PREFIX, CHAT_DETACH_CALLBACK,
    };

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

    #[test]
    fn pick_system_prompt_prefers_topic_override() {
        let selected = pick_system_prompt(
            Some("topic prompt"),
            Some("user prompt".to_string()),
            Some("env prompt".to_string()),
        );

        assert_eq!(selected, "topic prompt");
    }

    #[test]
    fn pick_system_prompt_falls_back_to_user_then_env() {
        let user_selected = pick_system_prompt(
            None,
            Some("user prompt".to_string()),
            Some("env prompt".to_string()),
        );
        let env_selected = pick_system_prompt(None, None, Some("env prompt".to_string()));

        assert_eq!(user_selected, "user prompt");
        assert_eq!(env_selected, "env prompt");
    }
}
