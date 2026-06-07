use crate::bot::context::{current_context_state, set_current_context_state};
use crate::bot::state::State;
use crate::bot::topic_route::{resolve_topic_route, touch_dynamic_binding_activity_if_needed};
use crate::bot::views::{agent_control_markup, AgentView, DefaultAgentView};
use crate::bot::UnauthorizedCache;
use crate::bot::{
    build_outbound_thread_params, resolve_thread_spec, OutboundThreadParams, TelegramThreadKind,
    TelegramThreadSpec,
};
use crate::config::BotSettings;
use anyhow::{anyhow, Result};
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::storage::StorageProvider;
use oxide_agent_core::utils::truncate_str;
use std::sync::Arc;
use teloxide::{
    dispatching::dialogue::InMemStorage,
    prelude::*,
    types::{
        CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, KeyboardButton, KeyboardMarkup,
        ParseMode, ReplyMarkup,
    },
    utils::command::BotCommands,
};
use tracing::info;

const MENU_CALLBACK_AGENT_MODE: &str = "menu:agent";
const MENU_CALLBACK_CLEAR_FLOW: &str = "menu:clear";
const MENU_CALLBACK_BACK: &str = "menu:back";

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

/// Safe extraction of user ID from a message.
/// Returns 0 if the user information is missing.
pub fn get_user_id_safe(msg: &Message) -> i64 {
    msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed())
}

fn can_use_agent_mode(settings: &BotSettings, user_id: i64) -> bool {
    let allowed_users = settings.telegram.allowed_users();
    !allowed_users.is_empty() && allowed_users.contains(&user_id)
}

fn should_default_to_agent_mode(
    _is_supergroup: bool,
    settings: &BotSettings,
    user_id: i64,
) -> bool {
    can_use_agent_mode(settings, user_id)
}

async fn current_or_default_context_state(
    storage: &Arc<dyn StorageProvider>,
    settings: &Arc<BotSettings>,
    user_id: i64,
    msg: &Message,
    thread_spec: TelegramThreadSpec,
) -> Result<Option<String>> {
    let state = current_context_state(storage, user_id, msg.chat.id, thread_spec).await?;
    if state.is_some()
        || !should_default_to_agent_mode(msg.chat.is_supergroup(), settings.as_ref(), user_id)
    {
        return Ok(state);
    }

    info!(
        "Defaulting to agent mode for user {user_id} in supergroup {}",
        msg.chat.id.0
    );
    set_current_context_state(
        storage,
        user_id,
        msg.chat.id,
        thread_spec,
        Some("agent_mode"),
    )
    .await?;

    Ok(Some("agent_mode".to_string()))
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
    let thread_spec = resolve_thread_spec(msg);

    if let Some(state_str) =
        current_or_default_context_state(storage, settings, user_id, msg, thread_spec).await?
        && state_str == "agent_mode" {
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
    Ok(false)
}

/// Supported commands for the bot
#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Supported commands:")]
pub enum Command {
    /// Start the bot and show welcome message
    #[command(description = "Start the bot.")]
    Start,
    /// Show help and controls
    #[command(description = "Show help and controls.")]
    Help,
    /// Cancel the current agent task
    #[command(description = "Cancel the current agent task.")]
    Cancel,
    /// Reset the current agent session
    #[command(description = "Reset the current agent session.")]
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
    let keyboard = vec![vec![KeyboardButton::new("❌ Cancel Task")]];
    KeyboardMarkup::new(keyboard).resize_keyboard()
}

#[must_use]
fn get_main_inline_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "❌ Cancel Task",
        crate::bot::views::AGENT_CALLBACK_CANCEL_TASK,
    )]])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuCallbackData {
    AgentMode,
    ClearFlow,
    Back,
}

fn use_inline_topic_controls(thread_spec: TelegramThreadSpec) -> bool {
    matches!(thread_spec.kind, TelegramThreadKind::Forum)
}

pub(crate) fn main_menu_markup(thread_spec: TelegramThreadSpec) -> ReplyMarkup {
    if use_inline_topic_controls(thread_spec) {
        get_main_inline_keyboard().into()
    } else {
        get_main_keyboard().into()
    }
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
    let thread_spec = resolve_thread_spec(&msg);
    let outbound_thread = build_outbound_thread_params(thread_spec);
    let user_id = get_user_id_safe(&msg);
    let user_name = get_user_name(&msg);

    info!("User {user_id} ({user_name}) initiated /start command.");

    if !can_use_agent_mode(settings.as_ref(), user_id) {
        let text = if settings.telegram.allowed_users().is_empty() {
            "⛔️ Bot access is not configured. Set TELEGRAM_ALLOWED_USERS and restart the bot."
        } else {
            "⛔️ You do not have permission to use this bot."
        };
        let mut req = bot.send_message(msg.chat.id, text);
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.await?;
        return Ok(());
    }

    set_current_context_state(
        &storage,
        user_id,
        msg.chat.id,
        thread_spec,
        Some("agent_mode"),
    )
    .await?;
    dialogue
        .update(State::AgentMode)
        .await
        .map_err(|e| anyhow!(e.to_string()))?;

    info!("User {user_id} ({user_name}) is allowed. Activated agent mode.");
    let model_id = settings.agent.get_configured_agent_model().id;
    let mut req = bot
        .send_message(msg.chat.id, DefaultAgentView::welcome_message(&model_id))
        .parse_mode(ParseMode::Html);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    if use_inline_topic_controls(thread_spec) {
        req.await?;
    } else {
        req.reply_markup(agent_control_markup(false)).await?;
    }

    Ok(())
}

/// Help handler
///
/// # Errors
///
/// Returns an error if the help message cannot be sent.
pub async fn help(
    bot: Bot,
    msg: Message,
    storage: Arc<dyn StorageProvider>,
    settings: Arc<BotSettings>,
    dialogue: Dialogue<State, InMemStorage<State>>,
) -> Result<()> {
    let thread_spec = resolve_thread_spec(&msg);
    let outbound_thread = build_outbound_thread_params(thread_spec);
    let user_id = get_user_id_safe(&msg);
    let user_name = get_user_name(&msg);

    info!("User {user_id} ({user_name}) initiated /help command.");

    if !can_use_agent_mode(settings.as_ref(), user_id) {
        let text = if settings.telegram.allowed_users().is_empty() {
            "⛔️ Bot access is not configured. Set TELEGRAM_ALLOWED_USERS and restart the bot."
        } else {
            "⛔️ You do not have permission to use this bot."
        };
        let mut req = bot.send_message(msg.chat.id, text);
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.await?;
        return Ok(());
    }

    set_current_context_state(
        &storage,
        user_id,
        msg.chat.id,
        thread_spec,
        Some("agent_mode"),
    )
    .await?;
    dialogue
        .update(State::AgentMode)
        .await
        .map_err(|e| anyhow!(e.to_string()))?;

    let model_id = settings.agent.get_configured_agent_model().id;
    let mut req = bot
        .send_message(msg.chat.id, DefaultAgentView::welcome_message(&model_id))
        .parse_mode(ParseMode::Html);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    if use_inline_topic_controls(thread_spec) {
        req.await?;
    } else {
        req.reply_markup(agent_control_markup(false)).await?;
    }

    Ok(())
}

/// Clear flow handler
///
/// # Errors
///
/// Returns an error if user config cannot be updated or message cannot be sent.
pub async fn clear(bot: Bot, msg: Message, storage: Arc<dyn StorageProvider>) -> Result<()> {
    let thread_spec = resolve_thread_spec(&msg);
    let outbound_thread = build_outbound_thread_params(thread_spec);
    let user_id = get_user_id_safe(&msg);
    let user_name = get_user_name(&msg);

    info!("User {user_id} ({user_name}) initiated agent session reset.");

    set_current_context_state(
        &storage,
        user_id,
        msg.chat.id,
        thread_spec,
        Some("agent_mode"),
    )
    .await?;
    let mut req = bot
        .send_message(msg.chat.id, "<b>Agent Mode is ready. Send a task.</b>")
        .parse_mode(ParseMode::Html);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.reply_markup(main_menu_markup(thread_spec)).await?;

    Ok(())
}

fn outbound_thread_from_message(msg: &Message) -> OutboundThreadParams {
    build_outbound_thread_params(resolve_thread_spec(msg))
}

fn parse_menu_callback_data(data: &str) -> Option<MenuCallbackData> {
    match data {
        MENU_CALLBACK_AGENT_MODE => Some(MenuCallbackData::AgentMode),
        MENU_CALLBACK_CLEAR_FLOW => Some(MenuCallbackData::ClearFlow),
        MENU_CALLBACK_BACK => Some(MenuCallbackData::Back),
        _ => None,
    }
}

/// Handle topic-friendly menu callbacks.
///
/// Returns true when callback belongs to topic menu controls.
pub async fn handle_menu_callback(
    bot: &Bot,
    q: &CallbackQuery,
    storage: &Arc<dyn StorageProvider>,
    llm: &Arc<LlmClient>,
    settings: &Arc<BotSettings>,
    dialogue: &Dialogue<State, InMemStorage<State>>,
) -> Result<bool> {
    let Some(data) = q.data.as_deref() else {
        return Ok(false);
    };

    let Some(callback_data) = parse_menu_callback_data(data) else {
        return Ok(false);
    };

    let Some(msg) = q
        .message
        .as_ref()
        .and_then(|message| message.regular_message())
    else {
        bot.answer_callback_query(q.id.clone())
            .text("Message context unavailable")
            .await?;
        return Ok(true);
    };

    let thread_spec = resolve_thread_spec(msg);
    let user_id = q.from.id.0.cast_signed();

    match callback_data {
        MenuCallbackData::AgentMode => {
            if check_agent_access(bot, msg, settings, user_id).await? {
                crate::bot::agent_handlers::activate_agent_mode(
                    bot.clone(),
                    msg.clone(),
                    dialogue.clone(),
                    llm.clone(),
                    storage.clone(),
                    settings.clone(),
                    user_id,
                )
                .await?;
            }
        }
        MenuCallbackData::ClearFlow => {
            clear(bot.clone(), msg.clone(), storage.clone()).await?;
        }
        MenuCallbackData::Back => {
            let outbound_thread = build_outbound_thread_params(thread_spec);
            handle_back_command(bot, msg.chat.id, dialogue, thread_spec, outbound_thread).await?;
        }
    }

    bot.answer_callback_query(q.id.clone()).await?;
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
    let thread_spec = resolve_thread_spec(&msg);
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

    if !check_agent_access(&bot, &msg, &settings, user_id).await? {
        touch_dynamic_binding_activity_if_needed(storage.as_ref(), user_id, &route).await;
        return Ok(());
    }

    set_current_context_state(
        &storage,
        user_id,
        msg.chat.id,
        thread_spec,
        Some("agent_mode"),
    )
    .await?;
    dialogue
        .update(State::AgentMode)
        .await
        .map_err(|e| anyhow!(e.to_string()))?;

    let result = Box::pin(crate::bot::agent_handlers::handle_agent_message(
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
}

#[allow(clippy::too_many_arguments)]
async fn handle_menu_commands(
    bot: &Bot,
    msg: &Message,
    storage: &Arc<dyn StorageProvider>,
    llm: &Arc<LlmClient>,
    dialogue: &Dialogue<State, InMemStorage<State>>,
    settings: &Arc<BotSettings>,
    text: &str,
) -> Result<bool> {
    let thread_spec = resolve_thread_spec(msg);
    let outbound_thread = build_outbound_thread_params(thread_spec);
    let user_id = get_user_id_safe(msg);

    match text {
        "Clear Flow" | "Reset Agent Session" => {
            clear(bot.clone(), msg.clone(), storage.clone()).await?;
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
                    user_id,
                )
                .await?;
            }
            Ok(true)
        }
        "Back" => {
            handle_back_command(bot, msg.chat.id, dialogue, thread_spec, outbound_thread).await
        }
        "Change Model" | "Extra Functions" | "Edit Prompt" => {
            send_menu_markup(
                bot,
                msg.chat.id,
                "This control is no longer supported. Agent Mode is ready; send a task.",
                main_menu_markup(thread_spec),
                outbound_thread,
            )
            .await?;
            Ok(true)
        }
        "⬅️ Exit Agent Mode" | "❌ Cancel Task" | "🗑 Clear Memory" => {
            let response = if text == "⬅️ Exit Agent Mode" {
                "Agent Mode is ready. Send a new task."
            } else if text == "❌ Cancel Task" {
                "No active task to cancel."
            } else {
                "Agent memory is not active."
            };
            send_menu_markup(
                bot,
                msg.chat.id,
                response,
                main_menu_markup(thread_spec),
                outbound_thread,
            )
            .await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn send_menu_markup(
    bot: &Bot,
    chat_id: ChatId,
    text: impl Into<String>,
    reply_markup: ReplyMarkup,
    outbound_thread: OutboundThreadParams,
) -> Result<()> {
    let mut req = bot.send_message(chat_id, text);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.reply_markup(reply_markup).await?;
    Ok(())
}

async fn handle_back_command(
    bot: &Bot,
    chat_id: ChatId,
    dialogue: &Dialogue<State, InMemStorage<State>>,
    thread_spec: TelegramThreadSpec,
    outbound_thread: OutboundThreadParams,
) -> Result<bool> {
    dialogue
        .update(State::AgentMode)
        .await
        .map_err(|e| anyhow!(e.to_string()))?;

    send_menu_markup(
        bot,
        chat_id,
        "Agent Mode is ready. Send a task.",
        main_menu_markup(thread_spec),
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
    let allowed_users = settings.telegram.allowed_users();
    if !allowed_users.is_empty() && !can_use_agent_mode(settings.as_ref(), user_id) {
        let mut req = bot.send_message(
            msg.chat.id,
            "⛔️ You do not have permission to use this bot.",
        );
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.await?;
        return Ok(false);
    } else if allowed_users.is_empty() {
        let mut req = bot.send_message(
            msg.chat.id,
            "⛔️ Bot access is not configured. Set TELEGRAM_ALLOWED_USERS and restart the bot.",
        );
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.await?;
        return Ok(false);
    }
    Ok(true)
}

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
    let thread_spec = resolve_thread_spec(&msg);
    let user_id = get_user_id_safe(&msg);
    let route = resolve_topic_route(&bot, storage.as_ref(), user_id, &settings, &msg).await;
    if !route.allows_processing() {
        info!(
            "Skipping voice message in topic route for user {user_id}. enabled={}, require_mention={}, mention_satisfied={}",
            route.enabled, route.require_mention, route.mention_satisfied
        );
        return Ok(());
    }

    set_current_context_state(
        &storage,
        user_id,
        msg.chat.id,
        thread_spec,
        Some("agent_mode"),
    )
    .await?;
    dialogue
        .update(State::AgentMode)
        .await
        .map_err(|e| anyhow!(e.to_string()))?;

    let result = Box::pin(crate::bot::agent_handlers::handle_agent_message(
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
    let thread_spec = resolve_thread_spec(&msg);
    let user_id = get_user_id_safe(&msg);
    let route = resolve_topic_route(&bot, storage.as_ref(), user_id, &settings, &msg).await;

    if !route.allows_processing() {
        info!(
            "Skipping photo message in topic route for user {user_id}. enabled={}, require_mention={}, mention_satisfied={}",
            route.enabled, route.require_mention, route.mention_satisfied
        );
        return Ok(());
    }

    set_current_context_state(
        &storage,
        user_id,
        msg.chat.id,
        thread_spec,
        Some("agent_mode"),
    )
    .await?;
    dialogue
        .update(State::AgentMode)
        .await
        .map_err(|e| anyhow!(e.to_string()))?;

    let result = Box::pin(crate::bot::agent_handlers::handle_agent_message(
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
}

/// Video message handler
///
/// # Errors
///
/// Returns an error if the video cannot be processed.
pub async fn handle_video(
    bot: Bot,
    msg: Message,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
    settings: Arc<BotSettings>,
) -> Result<()> {
    let thread_spec = resolve_thread_spec(&msg);
    let user_id = get_user_id_safe(&msg);
    let route = resolve_topic_route(&bot, storage.as_ref(), user_id, &settings, &msg).await;

    if !route.allows_processing() {
        info!(
            "Skipping video message in topic route for user {user_id}. enabled={}, require_mention={}, mention_satisfied={}",
            route.enabled, route.require_mention, route.mention_satisfied
        );
        return Ok(());
    }

    set_current_context_state(
        &storage,
        user_id,
        msg.chat.id,
        thread_spec,
        Some("agent_mode"),
    )
    .await?;
    dialogue
        .update(State::AgentMode)
        .await
        .map_err(|e| anyhow!(e.to_string()))?;

    let result = Box::pin(crate::bot::agent_handlers::handle_agent_message(
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
}

/// Handle document messages through Agent Mode.
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

    let thread_spec = resolve_thread_spec(&msg);
    let state =
        current_or_default_context_state(&storage, &settings, user_id, &msg, thread_spec).await?;

    if state.as_deref() == Some("agent_mode") {
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
            "📁 File upload requires bot access. Add your Telegram ID to TELEGRAM_ALLOWED_USERS.",
        );
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_menu_callback_data, should_default_to_agent_mode, MenuCallbackData,
        MENU_CALLBACK_AGENT_MODE, MENU_CALLBACK_BACK,
    };
    use crate::config::{BotSettings, TelegramSettings};
    use oxide_agent_core::config::AgentSettings;

    fn test_settings(allowed_users: Option<&str>) -> BotSettings {
        BotSettings::new(
            AgentSettings::default(),
            TelegramSettings {
                telegram_token: "dummy".to_string(),
                telegram_allowed_users_str: allowed_users.map(str::to_string),
                telegram_manager_allowed_users_str: None,
                manager_home_chat_id: None,
                manager_home_thread_id: None,
                attach_detach_enabled: true,
                reminder_agent_progress_enabled: true,
                reminder_silent_no_change_enabled: false,
                manager_home_agent_id: None,
                topic_configs: Vec::new(),
            },
        )
    }

    #[test]
    fn parse_menu_callback_data_parses_simple_actions() {
        assert_eq!(
            parse_menu_callback_data(MENU_CALLBACK_AGENT_MODE),
            Some(MenuCallbackData::AgentMode)
        );
        assert_eq!(
            parse_menu_callback_data(MENU_CALLBACK_BACK),
            Some(MenuCallbackData::Back)
        );
    }

    #[test]
    fn parse_menu_callback_data_rejects_removed_chat_controls() {
        assert_eq!(parse_menu_callback_data("menu:chat"), None);
        assert_eq!(parse_menu_callback_data("menu:model:3"), None);
        assert_eq!(parse_menu_callback_data("chat_attach:anything"), None);
    }

    #[test]
    fn defaults_to_agent_mode_for_allowed_supergroup_user() {
        let settings = test_settings(Some("77 88"));
        assert!(should_default_to_agent_mode(true, &settings, 77));
    }

    #[test]
    fn defaults_to_agent_mode_for_allowed_private_user() {
        let settings = test_settings(Some("77 88"));
        assert!(should_default_to_agent_mode(false, &settings, 77));
    }

    #[test]
    fn does_not_default_to_agent_mode_without_telegram_access() {
        let settings = test_settings(Some("88"));
        assert!(!should_default_to_agent_mode(true, &settings, 77));

        let unconfigured = test_settings(None);
        assert!(!should_default_to_agent_mode(true, &unconfigured, 77));
    }
}
