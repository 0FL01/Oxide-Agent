use crate::bot::UnauthorizedCache;
use crate::bot::context::set_current_context_state;
use crate::bot::state::State;
use crate::bot::views::{DefaultAgentView, agent_control_markup};
use crate::bot::{
    OutboundThreadParams, TelegramThreadKind, TelegramThreadSpec, build_outbound_thread_params,
    resolve_thread_spec,
};
use crate::config::BotSettings;
use anyhow::{Result, anyhow};
use oxide_agent_core::storage::StorageProvider;
use std::sync::Arc;
use teloxide::{
    dispatching::dialogue::InMemStorage,
    prelude::*,
    types::{InlineKeyboardMarkup, KeyboardMarkup, ParseMode, ReplyMarkup},
    utils::command::BotCommands,
};
use tracing::info;

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

/// Supported commands for the bot
#[derive(BotCommands, Clone, Debug)]
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
    /// Select the Agent model for this chat
    #[command(description = "Select the Agent model for this chat.")]
    Model,
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
    crate::bot::views::get_agent_keyboard()
}

#[must_use]
fn get_main_inline_keyboard() -> InlineKeyboardMarkup {
    crate::bot::views::get_agent_inline_keyboard()
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
    let context_key = crate::bot::context::storage_context_key(msg.chat.id, thread_spec);
    let model_id = crate::bot::agent_handlers::current_model_label(
        &storage,
        &settings.agent,
        user_id,
        &context_key,
    )
    .await;
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

    let context_key = crate::bot::context::storage_context_key(msg.chat.id, thread_spec);
    let model_id = crate::bot::agent_handlers::current_model_label(
        &storage,
        &settings.agent,
        user_id,
        &context_key,
    )
    .await;
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

#[cfg(test)]
mod tests {
    use super::Command;

    #[test]
    fn telegram_commands_do_not_expose_browser_start_or_control() {
        let command_names = [
            Command::Start,
            Command::Help,
            Command::Cancel,
            Command::Clear,
            Command::Healthcheck,
            Command::Stats,
        ]
        .map(|command| format!("{command:?}").to_ascii_lowercase())
        .join(" ");

        assert!(!command_names.contains("browser"));
        assert!(!command_names.contains("chrome"));
        assert!(!command_names.contains("control"));
    }
}
