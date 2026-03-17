//! Agent mode handlers for Telegram bot
//!
//! Provides handlers for activating agent mode, processing messages,
//! and managing agent sessions.

use crate::bot::context::{
    ensure_current_agent_flow_id, reset_current_agent_flow_id, sandbox_scope,
    set_current_agent_flow_id, set_current_context_state, storage_context_key,
};
use crate::bot::state::{ConfirmationType, State};
use crate::bot::topic_route::{
    resolve_topic_route, touch_dynamic_binding_activity_if_needed, TopicRouteDecision,
};
use crate::bot::views::{
    agent_control_markup, agent_flow_inline_keyboard, cancel_task_confirmation_inline_keyboard,
    confirmation_markup, empty_inline_keyboard, get_agent_inline_keyboard_with_exit, AgentView,
    DefaultAgentView, AGENT_CALLBACK_ATTACH_PREFIX, AGENT_CALLBACK_CANCEL_TASK,
    AGENT_CALLBACK_CLEAR_MEMORY, AGENT_CALLBACK_CONFIRM_CANCEL_NO,
    AGENT_CALLBACK_CONFIRM_CANCEL_YES, AGENT_CALLBACK_CONFIRM_CLEAR_CANCEL,
    AGENT_CALLBACK_CONFIRM_CLEAR_YES, AGENT_CALLBACK_CONFIRM_RECREATE_CANCEL,
    AGENT_CALLBACK_CONFIRM_RECREATE_YES, AGENT_CALLBACK_DETACH, AGENT_CALLBACK_EXIT,
    AGENT_CALLBACK_RECREATE_CONTAINER, AGENT_CALLBACK_SSH_APPROVE_PREFIX,
    AGENT_CALLBACK_SSH_REJECT_PREFIX, LOOP_CALLBACK_CANCEL, LOOP_CALLBACK_RESET,
    LOOP_CALLBACK_RETRY,
};
use crate::bot::{
    build_outbound_thread_params, general_forum_topic_id, resolve_thread_spec,
    OutboundThreadParams, TelegramThreadKind, TelegramThreadSpec,
};
use crate::config::BotSettings;
use anyhow::{Error, Result};
use oxide_agent_core::agent::SessionId;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::sandbox::SandboxScope;
use oxide_agent_core::storage::{
    compute_next_reminder_run_at, resolve_active_topic_binding, ReminderJobRecord,
    ReminderThreadKind, StorageProvider,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::prelude::*;
use teloxide::types::{
    CallbackQuery, InlineKeyboardMarkup, MessageId, ParseMode, ReplyMarkup, ThreadId,
};
use tokio::sync::{Mutex, RwLock};
use tokio::time::{Duration, MissedTickBehavior};
use tracing::{info, warn};

mod execution_config;
mod input;
mod session;
mod task_runner;

pub(crate) use execution_config::*;
pub(crate) use input::*;
pub(crate) use session::*;
pub(crate) use task_runner::*;

/// Type alias for dialogue
pub type AgentDialogue = Dialogue<State, InMemStorage<State>>;

enum AgentWipeError {
    Recreate(Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AgentControlCommand {
    CancelTask,
    ClearMemory,
    RecreateContainer,
    ExitAgentMode,
    ShowControls,
}

fn parse_agent_control_command(text: Option<&str>) -> Option<AgentControlCommand> {
    match text {
        Some("❌ Cancel Task") => Some(AgentControlCommand::CancelTask),
        Some("🗑 Clear Memory") => Some(AgentControlCommand::ClearMemory),
        Some("🔄 Recreate Container") => Some(AgentControlCommand::RecreateContainer),
        Some("⬅️ Exit Agent Mode") => Some(AgentControlCommand::ExitAgentMode),
        Some("/c") => Some(AgentControlCommand::ShowControls),
        _ => None,
    }
}

fn manager_default_chat_id(chat_id: ChatId, thread_spec: TelegramThreadSpec) -> Option<ChatId> {
    matches!(thread_spec.kind, TelegramThreadKind::Forum).then_some(chat_id)
}

fn reminder_thread_kind(thread_spec: TelegramThreadSpec) -> ReminderThreadKind {
    match thread_spec.kind {
        TelegramThreadKind::Dm => ReminderThreadKind::Dm,
        TelegramThreadKind::Forum => ReminderThreadKind::Forum,
        TelegramThreadKind::None => ReminderThreadKind::None,
    }
}

fn telegram_thread_kind(kind: ReminderThreadKind) -> TelegramThreadKind {
    match kind {
        ReminderThreadKind::Dm => TelegramThreadKind::Dm,
        ReminderThreadKind::Forum => TelegramThreadKind::Forum,
        ReminderThreadKind::None => TelegramThreadKind::None,
    }
}

fn thread_spec_from_reminder(record: &ReminderJobRecord) -> TelegramThreadSpec {
    TelegramThreadSpec::new(
        telegram_thread_kind(record.thread_kind),
        record
            .thread_id
            .and_then(|thread_id| i32::try_from(thread_id).ok())
            .map(|thread_id| ThreadId(MessageId(thread_id))),
    )
}

/// Global session registry for agent executors
static PENDING_CANCEL_MESSAGES: LazyLock<RwLock<HashMap<SessionId, MessageId>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
static PENDING_CANCEL_CONFIRMATIONS: LazyLock<RwLock<HashMap<SessionId, MessageId>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
static PENDING_TEXT_INPUT_BATCHES: LazyLock<Mutex<HashMap<SessionId, PendingTextInputBatch>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const TASK_CANCELLED_BY_USER: &str = "Task cancelled by user";
const REMINDER_POLL_INTERVAL_SECS: u64 = 5;
const REMINDER_BATCH_LIMIT: usize = 16;
const REMINDER_LEASE_SECS: i64 = 300;
const REMINDER_BUSY_BACKOFF_SECS: i64 = 30;

fn outbound_thread_from_message(msg: &Message) -> OutboundThreadParams {
    build_outbound_thread_params(resolve_thread_spec(msg))
}

fn outbound_thread_from_callback(q: &CallbackQuery) -> OutboundThreadParams {
    q.message
        .as_ref()
        .and_then(|message| message.regular_message())
        .map_or(
            OutboundThreadParams {
                message_thread_id: None,
            },
            outbound_thread_from_message,
        )
}

fn is_created_forum_topic(thread_spec: TelegramThreadSpec) -> bool {
    matches!(thread_spec.kind, TelegramThreadKind::Forum)
        && thread_spec.thread_id != Some(general_forum_topic_id())
}

fn manager_control_plane_enabled(
    settings: &BotSettings,
    user_id: i64,
    thread_spec: TelegramThreadSpec,
) -> bool {
    settings.telegram.manager_allowed_users().contains(&user_id)
        && !is_created_forum_topic(thread_spec)
}

async fn send_agent_message(
    bot: &Bot,
    chat_id: ChatId,
    text: impl Into<String>,
    outbound_thread: OutboundThreadParams,
) -> Result<()> {
    let mut req = bot.send_message(chat_id, text);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.await?;
    Ok(())
}

async fn send_agent_message_and_return(
    bot: &Bot,
    chat_id: ChatId,
    text: impl Into<String>,
    reply_markup: Option<ReplyMarkup>,
    outbound_thread: OutboundThreadParams,
) -> Result<Message> {
    super::resilient::send_message_resilient_with_thread_and_markup(
        bot,
        chat_id,
        text,
        None,
        outbound_thread.message_thread_id,
        reply_markup,
    )
    .await
}

async fn send_agent_message_with_keyboard(
    bot: &Bot,
    chat_id: ChatId,
    text: impl Into<String>,
    reply_markup: &ReplyMarkup,
    outbound_thread: OutboundThreadParams,
) -> Result<()> {
    let mut req = bot.send_message(chat_id, text);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.reply_markup(reply_markup.clone()).await?;
    Ok(())
}

async fn send_agent_message_with_optional_keyboard(
    bot: &Bot,
    chat_id: ChatId,
    text: impl Into<String>,
    reply_markup: Option<&ReplyMarkup>,
    outbound_thread: OutboundThreadParams,
) -> Result<()> {
    if let Some(reply_markup) = reply_markup {
        send_agent_message_with_keyboard(bot, chat_id, text, reply_markup, outbound_thread).await
    } else {
        send_agent_message(bot, chat_id, text, outbound_thread).await
    }
}

struct ConfirmationSendCtx<'a> {
    bot: &'a Bot,
    chat_id: ChatId,
    context_key: &'a str,
    agent_flow_id: &'a str,
    reply_markup: Option<ReplyMarkup>,
    manager_default_chat_id: Option<ChatId>,
    outbound_thread: OutboundThreadParams,
}

fn use_inline_topic_controls(thread_spec: TelegramThreadSpec) -> bool {
    matches!(thread_spec.kind, TelegramThreadKind::Forum)
}

fn automatic_agent_control_markup(thread_spec: TelegramThreadSpec) -> Option<ReplyMarkup> {
    (!use_inline_topic_controls(thread_spec)).then(|| agent_control_markup(false))
}

fn cancel_status_reply_markup(thread_spec: TelegramThreadSpec, agent_flow_id: &str) -> ReplyMarkup {
    if use_inline_topic_controls(thread_spec) {
        agent_flow_inline_keyboard(agent_flow_id).into()
    } else {
        agent_control_markup(false)
    }
}

fn cancel_status_inline_markup(
    use_inline_controls: bool,
    agent_flow_id: &str,
) -> Option<InlineKeyboardMarkup> {
    use_inline_controls.then(|| agent_flow_inline_keyboard(agent_flow_id))
}

fn is_task_cancelled_error(error: &anyhow::Error) -> bool {
    error.to_string() == TASK_CANCELLED_BY_USER
}

async fn pending_cancel_message(session_id: SessionId) -> Option<MessageId> {
    let pending = PENDING_CANCEL_MESSAGES.read().await;
    pending.get(&session_id).copied()
}

async fn pending_cancel_confirmation(session_id: SessionId) -> Option<MessageId> {
    let pending = PENDING_CANCEL_CONFIRMATIONS.read().await;
    pending.get(&session_id).copied()
}

async fn remember_pending_cancel_message(session_id: SessionId, message_id: MessageId) {
    let mut pending = PENDING_CANCEL_MESSAGES.write().await;
    pending.insert(session_id, message_id);
}

async fn clear_pending_cancel_message(session_id: SessionId) {
    let mut pending = PENDING_CANCEL_MESSAGES.write().await;
    pending.remove(&session_id);
}

async fn remember_pending_cancel_confirmation(session_id: SessionId, message_id: MessageId) {
    let mut pending = PENDING_CANCEL_CONFIRMATIONS.write().await;
    pending.insert(session_id, message_id);
}

async fn clear_pending_cancel_confirmation(session_id: SessionId) {
    let mut pending = PENDING_CANCEL_CONFIRMATIONS.write().await;
    pending.remove(&session_id);
}

async fn take_pending_cancel_message(session_id: SessionId) -> Option<MessageId> {
    let mut pending = PENDING_CANCEL_MESSAGES.write().await;
    pending.remove(&session_id)
}

async fn take_pending_cancel_confirmation(session_id: SessionId) -> Option<MessageId> {
    let mut pending = PENDING_CANCEL_CONFIRMATIONS.write().await;
    pending.remove(&session_id)
}

async fn send_or_update_pending_cancel_message(
    bot: &Bot,
    session_id: SessionId,
    chat_id: ChatId,
    text: &str,
    reply_markup: ReplyMarkup,
    inline_reply_markup: Option<InlineKeyboardMarkup>,
    outbound_thread: OutboundThreadParams,
) -> Result<()> {
    if let Some(message_id) = pending_cancel_message(session_id).await {
        if super::resilient::edit_message_safe_resilient_with_markup(
            bot,
            chat_id,
            message_id,
            text,
            inline_reply_markup.clone(),
        )
        .await
        {
            return Ok(());
        }

        clear_pending_cancel_message(session_id).await;
    }

    let message =
        send_agent_message_and_return(bot, chat_id, text, Some(reply_markup), outbound_thread)
            .await?;
    remember_pending_cancel_message(session_id, message.id).await;
    Ok(())
}

async fn finalize_pending_cancel_message(
    bot: &Bot,
    session_id: SessionId,
    chat_id: ChatId,
    text: &str,
    inline_reply_markup: Option<InlineKeyboardMarkup>,
) {
    let Some(message_id) = take_pending_cancel_message(session_id).await else {
        return;
    };

    let _ = super::resilient::edit_message_safe_resilient_with_markup(
        bot,
        chat_id,
        message_id,
        text,
        inline_reply_markup,
    )
    .await;
}

async fn send_or_update_cancel_confirmation(
    bot: &Bot,
    session_id: SessionId,
    chat_id: ChatId,
    outbound_thread: OutboundThreadParams,
) -> Result<()> {
    let inline_reply_markup = cancel_task_confirmation_inline_keyboard();

    if let Some(message_id) = pending_cancel_confirmation(session_id).await {
        if super::resilient::edit_message_safe_resilient_with_markup(
            bot,
            chat_id,
            message_id,
            DefaultAgentView::task_cancel_confirmation(),
            Some(inline_reply_markup.clone()),
        )
        .await
        {
            return Ok(());
        }

        clear_pending_cancel_confirmation(session_id).await;
    }

    let message = send_agent_message_and_return(
        bot,
        chat_id,
        DefaultAgentView::task_cancel_confirmation(),
        Some(inline_reply_markup.into()),
        outbound_thread,
    )
    .await?;
    remember_pending_cancel_confirmation(session_id, message.id).await;
    Ok(())
}

async fn clear_cancel_confirmation_message(bot: &Bot, session_id: SessionId, chat_id: ChatId) {
    let Some(message_id) = take_pending_cancel_confirmation(session_id).await else {
        return;
    };

    let _ = super::resilient::edit_message_safe_resilient_with_markup(
        bot,
        chat_id,
        message_id,
        DefaultAgentView::task_cancel_confirmation(),
        Some(empty_inline_keyboard()),
    )
    .await;
}

async fn finalize_cancel_status_if_needed(
    bot: &Bot,
    session_id: SessionId,
    chat_id: ChatId,
    cancelled: bool,
    inline_reply_markup: Option<InlineKeyboardMarkup>,
) {
    clear_cancel_confirmation_message(bot, session_id, chat_id).await;

    if cancelled {
        finalize_pending_cancel_message(
            bot,
            session_id,
            chat_id,
            DefaultAgentView::task_cancelled(),
            inline_reply_markup,
        )
        .await;
    } else {
        clear_pending_cancel_message(session_id).await;
    }
}

async fn show_agent_controls(
    bot: Bot,
    msg: Message,
    storage: Arc<dyn StorageProvider>,
) -> Result<()> {
    let thread_spec = resolve_thread_spec(&msg);
    let outbound_thread = build_outbound_thread_params(thread_spec);
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let reply_markup = if use_inline_topic_controls(thread_spec) {
        let (agent_flow_id, _) =
            ensure_current_agent_flow_id(&storage, user_id, msg.chat.id, thread_spec).await?;
        get_agent_inline_keyboard_with_exit(false, Some(&agent_flow_id)).into()
    } else {
        agent_control_markup(false)
    };

    send_agent_message_with_keyboard(
        &bot,
        msg.chat.id,
        DefaultAgentView::ready_to_work(),
        &reply_markup,
        outbound_thread,
    )
    .await
}

async fn handle_clear_memory_confirmation(
    user_id: i64,
    session_keys: AgentModeSessionKeys,
    storage: &Arc<dyn StorageProvider>,
    thread_spec: TelegramThreadSpec,
    send_ctx: &ConfirmationSendCtx<'_>,
) -> Result<()> {
    info!(user_id = user_id, "User confirmed memory clear");
    if is_agent_task_running(session_keys.primary).await {
        send_agent_message_with_optional_keyboard(
            send_ctx.bot,
            send_ctx.chat_id,
            DefaultAgentView::clear_blocked_by_task(),
            send_ctx.reply_markup.as_ref(),
            send_ctx.outbound_thread,
        )
        .await?;
        return Ok(());
    }

    save_memory_after_task(
        session_keys.primary,
        user_id,
        send_ctx.context_key,
        send_ctx.agent_flow_id,
        storage,
    )
    .await;
    let _ = SESSION_REGISTRY.remove_if_idle(&session_keys.primary).await;
    let _ = reset_current_agent_flow_id(storage, user_id, send_ctx.chat_id, thread_spec).await?;
    send_agent_message_with_optional_keyboard(
        send_ctx.bot,
        send_ctx.chat_id,
        DefaultAgentView::memory_cleared(),
        send_ctx.reply_markup.as_ref(),
        send_ctx.outbound_thread,
    )
    .await?;

    Ok(())
}

async fn handle_recreate_container_confirmation(
    user_id: i64,
    session_keys: AgentModeSessionKeys,
    storage: &Arc<dyn StorageProvider>,
    llm: &Arc<LlmClient>,
    settings: &Arc<BotSettings>,
    send_ctx: &ConfirmationSendCtx<'_>,
) -> Result<()> {
    info!(user_id = user_id, "User confirmed container recreation");
    let session_id = ensure_session_exists(EnsureSessionContext {
        session_keys,
        context_key: send_ctx.context_key.to_string(),
        agent_flow_id: send_ctx.agent_flow_id.to_string(),
        agent_flow_created: false,
        sandbox_scope: SandboxScope::new(user_id, send_ctx.context_key.to_string()),
        user_id,
        bot: send_ctx.bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: send_ctx.manager_default_chat_id,
            thread_spec: TelegramThreadSpec::new(
                if send_ctx.manager_default_chat_id.is_some() {
                    TelegramThreadKind::Forum
                } else {
                    TelegramThreadKind::None
                },
                send_ctx.outbound_thread.message_thread_id.or_else(|| {
                    send_ctx
                        .manager_default_chat_id
                        .map(|_| general_forum_topic_id())
                }),
            ),
        },
        llm,
        storage,
        settings,
    })
    .await;

    match SESSION_REGISTRY
        .with_executor_mut(&session_id, |executor| {
            Box::pin(async move {
                executor
                    .session_mut()
                    .force_recreate_sandbox()
                    .await
                    .map_err(AgentWipeError::Recreate)?;
                Ok(())
            })
        })
        .await
    {
        Ok(Ok(())) => {
            send_agent_message_with_optional_keyboard(
                send_ctx.bot,
                send_ctx.chat_id,
                DefaultAgentView::container_recreated(),
                send_ctx.reply_markup.as_ref(),
                send_ctx.outbound_thread,
            )
            .await?;
        }
        Ok(Err(AgentWipeError::Recreate(e))) => {
            warn!(error = %e, "Container recreation failed");
            send_agent_message_with_optional_keyboard(
                send_ctx.bot,
                send_ctx.chat_id,
                DefaultAgentView::container_error(&format!("{e:#}")),
                send_ctx.reply_markup.as_ref(),
                send_ctx.outbound_thread,
            )
            .await?;
        }
        Err("Cannot reset while task is running") => {
            send_agent_message_with_optional_keyboard(
                send_ctx.bot,
                send_ctx.chat_id,
                DefaultAgentView::container_recreate_blocked_by_task(),
                send_ctx.reply_markup.as_ref(),
                send_ctx.outbound_thread,
            )
            .await?;
        }
        Err(_) => {
            send_agent_message_with_optional_keyboard(
                send_ctx.bot,
                send_ctx.chat_id,
                DefaultAgentView::sandbox_access_error(),
                send_ctx.reply_markup.as_ref(),
                send_ctx.outbound_thread,
            )
            .await?;
        }
    }

    Ok(())
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
    storage: Arc<dyn StorageProvider>,
    settings: Arc<BotSettings>,
    user_id: i64,
) -> Result<()> {
    let thread_spec = resolve_thread_spec(&msg);
    let outbound_thread = build_outbound_thread_params(thread_spec);
    let context_key = storage_context_key(msg.chat.id, thread_spec);
    let (agent_flow_id, agent_flow_created) =
        ensure_current_agent_flow_id(&storage, user_id, msg.chat.id, thread_spec).await?;
    let sandbox_scope = sandbox_scope(user_id, msg.chat.id, thread_spec);
    let session_keys =
        agent_mode_session_keys(user_id, msg.chat.id, thread_spec.thread_id, &agent_flow_id);

    info!("Activating agent mode for user {user_id}");

    ensure_session_exists(EnsureSessionContext {
        session_keys,
        context_key,
        agent_flow_id,
        agent_flow_created,
        sandbox_scope,
        user_id,
        bot: &bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: manager_default_chat_id(msg.chat.id, thread_spec),
            thread_spec,
        },
        llm: &llm,
        storage: &storage,
        settings: &settings,
    })
    .await;

    set_current_context_state(
        &storage,
        user_id,
        msg.chat.id,
        thread_spec,
        Some("agent_mode"),
    )
    .await?;

    // Update dialogue state
    dialogue.update(State::AgentMode).await?;

    // Send welcome message
    let (model_id, _, _) = settings.agent.get_configured_agent_model();
    let mut req = bot
        .send_message(msg.chat.id, DefaultAgentView::welcome_message(&model_id))
        .parse_mode(ParseMode::Html);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    if let Some(reply_markup) = automatic_agent_control_markup(thread_spec) {
        req.reply_markup(reply_markup).await?;
    } else {
        req.await?;
    }

    Ok(())
}

async fn delegate_non_agent_context_message(
    bot: Bot,
    msg: Message,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    dialogue: AgentDialogue,
    settings: Arc<BotSettings>,
) -> Result<()> {
    if msg.text().is_some() {
        return crate::bot::handlers::handle_text(bot, msg, storage, llm, dialogue, settings).await;
    }
    if msg.voice().is_some() {
        return crate::bot::handlers::handle_voice(bot, msg, storage, llm, dialogue, settings)
            .await;
    }
    if msg.photo().is_some() {
        return crate::bot::handlers::handle_photo(bot, msg, storage, llm, dialogue, settings)
            .await;
    }
    if msg.document().is_some() {
        return crate::bot::handlers::handle_document(bot, msg, dialogue, storage, llm, settings)
            .await;
    }

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
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    dialogue: AgentDialogue,
    settings: Arc<BotSettings>,
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let chat_id = msg.chat.id;
    let thread_spec = resolve_thread_spec(&msg);
    let outbound_thread = build_outbound_thread_params(thread_spec);
    let context_key = storage_context_key(chat_id, thread_spec);
    let sandbox_scope = sandbox_scope(user_id, chat_id, thread_spec);

    if !is_agent_mode_context(&storage, user_id, chat_id, thread_spec).await? {
        return delegate_non_agent_context_message(bot, msg, storage, llm, dialogue, settings)
            .await;
    }

    let (agent_flow_id, agent_flow_created, session_keys) =
        ensure_agent_flow_session_keys(&storage, user_id, chat_id, thread_spec).await?;

    if let Some(command) = parse_agent_control_command(msg.text()) {
        return handle_agent_control_command(command, bot, msg, dialogue, storage).await;
    }

    let route = resolve_topic_route(&bot, storage.as_ref(), user_id, &settings, &msg).await;
    if !route_allows_agent_processing(&route, user_id) {
        return Ok(());
    }

    let manager_enabled = manager_control_plane_enabled(&settings, user_id, thread_spec);

    let session_id = ensure_session_exists(EnsureSessionContext {
        session_keys,
        context_key: context_key.clone(),
        agent_flow_id: agent_flow_id.clone(),
        agent_flow_created,
        sandbox_scope: sandbox_scope.clone(),
        user_id,
        bot: &bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: manager_default_chat_id(chat_id, thread_spec),
            thread_spec,
        },
        llm: &llm,
        storage: &storage,
        settings: &settings,
    })
    .await;

    let execution_profile =
        resolve_execution_profile(&storage, user_id, &context_key, &route, manager_enabled).await;
    let topic_infra_config = resolve_topic_infra_config(&storage, user_id, &context_key).await;

    let active_session = ActiveSessionConfig {
        session_id,
        storage: storage.clone(),
        user_id,
        context_key: context_key.clone(),
        agent_flow_id: agent_flow_id.clone(),
        chat_id,
        thread_spec,
    };

    configure_active_session(&active_session, execution_profile, topic_infra_config).await;

    let dispatch_ctx = build_batched_text_task_context(&bot, &active_session, outbound_thread);

    if handle_batched_text_input_if_needed(BatchedTextInputCheck {
        msg: &msg,
        bot: &bot,
        storage: &storage,
        route: &route,
        thread_spec,
        outbound_thread,
        session_id,
        user_id,
        chat_id,
        context_key: &context_key,
        agent_flow_id: &agent_flow_id,
    })
    .await?
    {
        return Ok(());
    }

    if handle_running_agent_message_if_needed(RunningAgentMessageContext {
        msg: &msg,
        bot: &bot,
        route: &route,
        sandbox_scope: &sandbox_scope,
        dispatch: dispatch_ctx.clone(),
        thread_spec,
        outbound_thread,
        llm: &llm,
    })
    .await?
    {
        return Ok(());
    }

    renew_cancellation_token(session_id).await;

    spawn_agent_task(AgentTaskContext {
        bot: bot.clone(),
        msg: msg.clone(),
        storage: storage.clone(),
        llm: llm.clone(),
        context_key,
        agent_flow_id,
        sandbox_scope,
        message_thread_id: outbound_thread.message_thread_id,
        use_inline_progress_controls: use_inline_topic_controls(thread_spec),
        session_id,
    });

    touch_dynamic_binding_activity_if_needed(storage.as_ref(), user_id, &route).await;
    Ok(())
}

async fn handle_agent_control_command(
    command: AgentControlCommand,
    bot: Bot,
    msg: Message,
    dialogue: AgentDialogue,
    storage: Arc<dyn StorageProvider>,
) -> Result<()> {
    match command {
        AgentControlCommand::CancelTask => cancel_agent_task(bot, msg, dialogue, storage).await,
        AgentControlCommand::ClearMemory => {
            confirm_destructive_action(ConfirmationType::ClearMemory, bot, msg, dialogue).await
        }
        AgentControlCommand::RecreateContainer => {
            confirm_destructive_action(ConfirmationType::RecreateContainer, bot, msg, dialogue)
                .await
        }
        AgentControlCommand::ExitAgentMode => exit_agent_mode(bot, msg, dialogue, storage).await,
        AgentControlCommand::ShowControls => show_agent_controls(bot, msg, storage).await,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        agent_mode_session_keys, assemble_text_batch, cancel_status_reply_markup,
        cleanup_abandoned_empty_flow, clear_pending_cancel_confirmation,
        clear_pending_cancel_message, derive_agent_mode_session_id, ensure_session_exists,
        manager_control_plane_enabled, manager_default_chat_id, merge_prompt_instructions,
        parse_agent_callback_action, parse_agent_control_command, pending_cancel_confirmation,
        pending_cancel_message, remember_pending_cancel_confirmation,
        remember_pending_cancel_message, remove_sessions_with_compat, resolve_execution_profile,
        select_existing_session_id, session_manager_control_plane_enabled,
        should_create_fresh_flow_on_detach, should_merge_text_batch,
        take_pending_cancel_confirmation, take_pending_cancel_message, AgentCallbackAction,
        AgentControlCommand, BatchedTextTaskContext, EnsureSessionContext, PendingTextInputBatch,
        PendingTextInputPart, SessionTransportContext, AGENT_TEXT_INPUT_SPLIT_THRESHOLD_CHARS,
        SESSION_REGISTRY,
    };
    use crate::bot::views::{
        AGENT_CALLBACK_CANCEL_TASK, AGENT_CALLBACK_CONFIRM_CANCEL_NO,
        AGENT_CALLBACK_CONFIRM_CANCEL_YES,
    };
    use crate::bot::{general_forum_topic_id, resolve_thread_spec_from_context};
    use crate::config::{BotSettings, TelegramSettings};
    use async_trait::async_trait;
    use oxide_agent_core::agent::SessionId;
    use oxide_agent_core::agent::{AgentMemory, AgentSession};
    use oxide_agent_core::config::AgentSettings;
    use oxide_agent_core::llm::LlmClient;
    use oxide_agent_core::sandbox::SandboxScope;
    use oxide_agent_core::storage::{
        AgentFlowRecord, AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord, Message,
        StorageError, StorageProvider, TopicAgentsMdRecord, TopicBindingRecord,
        UpsertAgentProfileOptions, UpsertTopicBindingOptions, UserConfig,
    };
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use teloxide::types::{ChatId, MessageId, ReplyMarkup, ThreadId};
    use teloxide::Bot;

    fn test_batch() -> PendingTextInputBatch {
        PendingTextInputBatch::new(
            BatchedTextTaskContext {
                bot: Bot::new("TOKEN"),
                chat_id: ChatId(1),
                session_id: SessionId::from(7_i64),
                user_id: 1,
                storage: Arc::new(NoopStorage::default()),
                context_key: "ctx".to_string(),
                agent_flow_id: "flow".to_string(),
                message_thread_id: None,
                use_inline_progress_controls: false,
            },
            MessageId(10),
            "x".repeat(AGENT_TEXT_INPUT_SPLIT_THRESHOLD_CHARS),
            Instant::now(),
        )
    }

    #[test]
    fn merges_sequential_large_text_parts() {
        let batch = test_batch();

        assert!(should_merge_text_batch(
            &batch,
            MessageId(11),
            "tail",
            batch.updated_at + Duration::from_millis(200),
        ));
    }

    #[test]
    fn does_not_merge_short_independent_messages() {
        let batch = PendingTextInputBatch {
            ctx: test_batch().ctx,
            parts: vec![PendingTextInputPart {
                message_id: MessageId(10),
                text: "short note".to_string(),
            }],
            revision: 1,
            updated_at: Instant::now(),
        };

        assert!(!should_merge_text_batch(
            &batch,
            MessageId(11),
            "another short note",
            batch.updated_at + Duration::from_millis(200),
        ));
    }

    #[test]
    fn assembles_text_batch_in_order_without_extra_separators() {
        let parts = vec![
            PendingTextInputPart {
                message_id: MessageId(10),
                text: "first".to_string(),
            },
            PendingTextInputPart {
                message_id: MessageId(11),
                text: "second".to_string(),
            },
        ];

        assert_eq!(assemble_text_batch(&parts), "firstsecond");
    }

    #[derive(Default)]
    struct NoopStorage {
        flow_memory: Option<AgentMemory>,
        agent_profile: Option<serde_json::Value>,
        topic_context: Option<String>,
        topic_agents_md: Option<String>,
        fail_flow_memory_lookup: bool,
        cleared_flows: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl NoopStorage {
        fn with_flow_memory(flow_memory: Option<AgentMemory>) -> Self {
            Self {
                flow_memory,
                agent_profile: None,
                topic_context: None,
                topic_agents_md: None,
                fail_flow_memory_lookup: false,
                cleared_flows: Arc::default(),
            }
        }

        fn with_agent_profile_and_topic_context(
            agent_profile: serde_json::Value,
            topic_context: &str,
        ) -> Self {
            Self {
                flow_memory: None,
                agent_profile: Some(agent_profile),
                topic_context: Some(topic_context.to_string()),
                topic_agents_md: None,
                fail_flow_memory_lookup: false,
                cleared_flows: Arc::default(),
            }
        }

        fn with_topic_agents_md(topic_agents_md: &str) -> Self {
            Self {
                flow_memory: None,
                agent_profile: None,
                topic_context: None,
                topic_agents_md: Some(topic_agents_md.to_string()),
                fail_flow_memory_lookup: false,
                cleared_flows: Arc::default(),
            }
        }

        fn with_failed_flow_memory_lookup() -> Self {
            Self {
                flow_memory: None,
                agent_profile: None,
                topic_context: None,
                topic_agents_md: None,
                fail_flow_memory_lookup: true,
                cleared_flows: Arc::default(),
            }
        }
    }

    #[async_trait]
    impl StorageProvider for NoopStorage {
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

        async fn update_user_state(
            &self,
            _user_id: i64,
            _state: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_state(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
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
            _memory: &oxide_agent_core::agent::AgentMemory,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn load_agent_memory(
            &self,
            _user_id: i64,
        ) -> Result<Option<oxide_agent_core::agent::AgentMemory>, StorageError> {
            Ok(None)
        }

        async fn clear_agent_memory(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn clear_agent_memory_for_flow(
            &self,
            _user_id: i64,
            context_key: String,
            flow_id: String,
        ) -> Result<(), StorageError> {
            self.cleared_flows
                .lock()
                .expect("cleared_flows mutex poisoned")
                .push((context_key, flow_id));
            Ok(())
        }

        async fn load_agent_memory_for_flow(
            &self,
            _user_id: i64,
            _context_key: String,
            _flow_id: String,
        ) -> Result<Option<AgentMemory>, StorageError> {
            if self.fail_flow_memory_lookup {
                return Err(StorageError::Config(
                    "flow memory lookup failed".to_string(),
                ));
            }

            Ok(self.flow_memory.clone())
        }

        async fn get_agent_flow_record(
            &self,
            _user_id: i64,
            _context_key: String,
            _flow_id: String,
        ) -> Result<Option<AgentFlowRecord>, StorageError> {
            Ok(None)
        }

        async fn upsert_agent_flow_record(
            &self,
            user_id: i64,
            context_key: String,
            flow_id: String,
        ) -> Result<AgentFlowRecord, StorageError> {
            Ok(AgentFlowRecord {
                schema_version: 1,
                user_id,
                context_key,
                flow_id,
                created_at: 0,
                updated_at: 0,
            })
        }

        async fn clear_all_context(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn check_connection(&self) -> Result<(), String> {
            Ok(())
        }

        async fn get_agent_profile(
            &self,
            user_id: i64,
            agent_id: String,
        ) -> Result<Option<AgentProfileRecord>, StorageError> {
            Ok(self
                .agent_profile
                .clone()
                .map(|profile| AgentProfileRecord {
                    schema_version: 1,
                    version: 1,
                    user_id,
                    agent_id,
                    profile,
                    created_at: 0,
                    updated_at: 0,
                }))
        }

        async fn upsert_agent_profile(
            &self,
            _options: UpsertAgentProfileOptions,
        ) -> Result<AgentProfileRecord, StorageError> {
            Err(StorageError::Config("not needed in tests".to_string()))
        }

        async fn delete_agent_profile(
            &self,
            _user_id: i64,
            _agent_id: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_topic_context(
            &self,
            user_id: i64,
            topic_id: String,
        ) -> Result<Option<oxide_agent_core::storage::TopicContextRecord>, StorageError> {
            Ok(self.topic_context.clone().map(|context| {
                oxide_agent_core::storage::TopicContextRecord {
                    schema_version: 1,
                    version: 1,
                    user_id,
                    topic_id,
                    context,
                    created_at: 0,
                    updated_at: 0,
                }
            }))
        }

        async fn upsert_topic_context(
            &self,
            _options: oxide_agent_core::storage::UpsertTopicContextOptions,
        ) -> Result<oxide_agent_core::storage::TopicContextRecord, StorageError> {
            Err(StorageError::Config("not needed in tests".to_string()))
        }

        async fn delete_topic_context(
            &self,
            _user_id: i64,
            _topic_id: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_topic_agents_md(
            &self,
            user_id: i64,
            topic_id: String,
        ) -> Result<Option<TopicAgentsMdRecord>, StorageError> {
            Ok(self
                .topic_agents_md
                .clone()
                .map(|agents_md| TopicAgentsMdRecord {
                    schema_version: 1,
                    version: 1,
                    user_id,
                    topic_id,
                    agents_md,
                    created_at: 0,
                    updated_at: 0,
                }))
        }

        async fn get_topic_binding(
            &self,
            _user_id: i64,
            _topic_id: String,
        ) -> Result<Option<TopicBindingRecord>, StorageError> {
            Ok(None)
        }

        async fn upsert_topic_binding(
            &self,
            _options: UpsertTopicBindingOptions,
        ) -> Result<TopicBindingRecord, StorageError> {
            Err(StorageError::Config("not needed in tests".to_string()))
        }

        async fn delete_topic_binding(
            &self,
            _user_id: i64,
            _topic_id: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn append_audit_event(
            &self,
            _options: AppendAuditEventOptions,
        ) -> Result<AuditEventRecord, StorageError> {
            Err(StorageError::Config("not needed in tests".to_string()))
        }

        async fn list_audit_events(
            &self,
            _user_id: i64,
            _limit: usize,
        ) -> Result<Vec<AuditEventRecord>, StorageError> {
            Ok(Vec::new())
        }

        async fn list_audit_events_page(
            &self,
            _user_id: i64,
            _before_version: Option<u64>,
            _limit: usize,
        ) -> Result<Vec<AuditEventRecord>, StorageError> {
            Ok(Vec::new())
        }
    }

    fn test_settings(manager_users: Option<&str>) -> Arc<BotSettings> {
        Arc::new(BotSettings::new(
            AgentSettings::default(),
            TelegramSettings {
                telegram_token: "dummy".to_string(),
                allowed_users_str: None,
                agent_allowed_users_str: Some("77 88".to_string()),
                manager_allowed_users_str: manager_users.map(str::to_string),
                topic_configs: Vec::new(),
            },
        ))
    }

    fn test_llm(settings: &Arc<BotSettings>) -> Arc<LlmClient> {
        Arc::new(LlmClient::new(settings.agent.as_ref()))
    }

    fn test_sandbox_scope(user_id: i64, context_key: &str) -> SandboxScope {
        SandboxScope::new(user_id, context_key.to_string())
    }

    #[test]
    fn control_commands_are_recognized_for_topic_gate_bypass() {
        assert_eq!(
            parse_agent_control_command(Some("❌ Cancel Task")),
            Some(AgentControlCommand::CancelTask)
        );
        assert_eq!(
            parse_agent_control_command(Some("🗑 Clear Memory")),
            Some(AgentControlCommand::ClearMemory)
        );
        assert_eq!(
            parse_agent_control_command(Some("🔄 Recreate Container")),
            Some(AgentControlCommand::RecreateContainer)
        );
        assert_eq!(
            parse_agent_control_command(Some("⬅️ Exit Agent Mode")),
            Some(AgentControlCommand::ExitAgentMode)
        );
        assert_eq!(
            parse_agent_control_command(Some("/c")),
            Some(AgentControlCommand::ShowControls)
        );
    }

    #[test]
    fn non_control_messages_do_not_bypass_topic_gate() {
        assert_eq!(parse_agent_control_command(Some("please help")), None);
        assert_eq!(parse_agent_control_command(Some("user@example.com")), None);
        assert_eq!(parse_agent_control_command(None), None);
    }

    #[test]
    fn inline_cancel_callbacks_are_recognized() {
        assert_eq!(
            parse_agent_callback_action(AGENT_CALLBACK_CANCEL_TASK),
            Some(AgentCallbackAction::StartCancelTaskConfirmation)
        );
        assert_eq!(
            parse_agent_callback_action(AGENT_CALLBACK_CONFIRM_CANCEL_YES),
            Some(AgentCallbackAction::ResolveCancelTaskConfirmation(true))
        );
        assert_eq!(
            parse_agent_callback_action(AGENT_CALLBACK_CONFIRM_CANCEL_NO),
            Some(AgentCallbackAction::ResolveCancelTaskConfirmation(false))
        );
    }

    #[test]
    fn ssh_approval_callbacks_are_recognized() {
        assert_eq!(
            parse_agent_callback_action("agent:ssh:approve:req-1"),
            Some(AgentCallbackAction::ApproveSsh("req-1".to_string()))
        );
        assert_eq!(
            parse_agent_callback_action("agent:ssh:reject:req-1"),
            Some(AgentCallbackAction::RejectSsh("req-1".to_string()))
        );
    }

    #[test]
    fn session_id_derivation_is_stable_without_thread() {
        let user_id = 12345;
        let first = derive_agent_mode_session_id(user_id, ChatId(-1001), None, "flow-a");
        let second = derive_agent_mode_session_id(user_id, ChatId(-1001), None, "flow-a");

        assert_eq!(first, second);
    }

    #[test]
    fn session_id_derivation_is_stable_for_same_thread() {
        let user_id = 12345;
        let thread_id = Some(ThreadId(MessageId(42)));
        let first = derive_agent_mode_session_id(user_id, ChatId(-1001), thread_id, "flow-a");
        let second = derive_agent_mode_session_id(user_id, ChatId(-1001), thread_id, "flow-a");

        assert_eq!(first, second);
    }

    #[test]
    fn session_id_derivation_differs_for_different_threads() {
        let user_id = 12345;
        let first = derive_agent_mode_session_id(
            user_id,
            ChatId(-1001),
            Some(ThreadId(MessageId(42))),
            "flow-a",
        );
        let second = derive_agent_mode_session_id(
            user_id,
            ChatId(-1001),
            Some(ThreadId(MessageId(43))),
            "flow-a",
        );

        assert_ne!(first, second);
    }

    #[test]
    fn session_id_derivation_differs_for_different_flows() {
        let user_id = 12345;
        let thread_id = Some(ThreadId(MessageId(42)));
        let first = derive_agent_mode_session_id(user_id, ChatId(-1001), thread_id, "flow-a");
        let second = derive_agent_mode_session_id(user_id, ChatId(-1001), thread_id, "flow-b");

        assert_ne!(first, second);
    }

    #[test]
    fn existing_session_selection_prefers_primary_key() {
        let keys = agent_mode_session_keys(
            12345,
            ChatId(-1001),
            Some(ThreadId(MessageId(42))),
            "flow-a",
        );
        let selected = select_existing_session_id(keys, true, true);

        assert_eq!(selected, Some(keys.primary));
    }

    #[test]
    fn existing_session_selection_falls_back_to_legacy_key() {
        let keys = agent_mode_session_keys(
            12345,
            ChatId(-1001),
            Some(ThreadId(MessageId(42))),
            "flow-a",
        );
        let selected = select_existing_session_id(keys, false, true);

        assert_eq!(selected, Some(keys.legacy));
    }

    #[test]
    fn cancel_status_reply_markup_uses_flow_controls_in_topics() {
        let thread_id = ThreadId(MessageId(42));
        let markup = cancel_status_reply_markup(
            resolve_thread_spec_from_context(true, true, Some(thread_id)),
            "flow-a",
        );

        let ReplyMarkup::InlineKeyboard(keyboard) = markup else {
            panic!("topic cancel status should use inline keyboard");
        };

        assert_eq!(keyboard.inline_keyboard.len(), 1);
        assert_eq!(keyboard.inline_keyboard[0].len(), 2);
    }

    #[tokio::test]
    async fn pending_cancel_message_round_trip_clears_entry() {
        let session_id = SessionId::from(777_i64);

        clear_pending_cancel_message(session_id).await;
        remember_pending_cancel_message(session_id, MessageId(55)).await;

        assert_eq!(
            pending_cancel_message(session_id).await,
            Some(MessageId(55))
        );
        assert_eq!(
            take_pending_cancel_message(session_id).await,
            Some(MessageId(55))
        );
        assert_eq!(pending_cancel_message(session_id).await, None);
    }

    #[tokio::test]
    async fn pending_cancel_confirmation_round_trip_clears_entry() {
        let session_id = SessionId::from(778_i64);

        clear_pending_cancel_confirmation(session_id).await;
        remember_pending_cancel_confirmation(session_id, MessageId(56)).await;

        assert_eq!(
            pending_cancel_confirmation(session_id).await,
            Some(MessageId(56))
        );
        assert_eq!(
            take_pending_cancel_confirmation(session_id).await,
            Some(MessageId(56))
        );
        assert_eq!(pending_cancel_confirmation(session_id).await, None);
    }

    #[test]
    fn manager_default_chat_id_is_available_in_general_forum_topic() {
        let spec = resolve_thread_spec_from_context(true, true, None);
        assert_eq!(
            manager_default_chat_id(ChatId(-100_123), spec),
            Some(ChatId(-100_123))
        );
    }

    #[test]
    fn manager_default_chat_id_is_not_available_outside_forum_context() {
        let spec = resolve_thread_spec_from_context(true, false, None);
        assert_eq!(manager_default_chat_id(ChatId(-100_123), spec), None);
    }

    #[test]
    fn manager_control_plane_access_requires_dedicated_allowlist_entry() {
        let settings = BotSettings::new(
            AgentSettings::default(),
            TelegramSettings {
                telegram_token: "dummy".to_string(),
                allowed_users_str: None,
                agent_allowed_users_str: Some("77 88".to_string()),
                manager_allowed_users_str: Some("88".to_string()),
                topic_configs: Vec::new(),
            },
        );

        let general_spec = resolve_thread_spec_from_context(true, true, None);

        assert!(!manager_control_plane_enabled(&settings, 77, general_spec));
        assert!(manager_control_plane_enabled(&settings, 88, general_spec));
    }

    #[test]
    fn manager_control_plane_access_disabled_when_allowlist_is_empty() {
        let settings = BotSettings::new(
            AgentSettings::default(),
            TelegramSettings {
                telegram_token: "dummy".to_string(),
                allowed_users_str: None,
                agent_allowed_users_str: Some("77".to_string()),
                manager_allowed_users_str: None,
                topic_configs: Vec::new(),
            },
        );

        let general_spec = resolve_thread_spec_from_context(true, true, None);

        assert!(!manager_control_plane_enabled(&settings, 77, general_spec));
    }

    #[test]
    fn manager_control_plane_gating_disables_tools_inside_created_topics() {
        let settings = BotSettings::new(
            AgentSettings::default(),
            TelegramSettings {
                telegram_token: "dummy".to_string(),
                allowed_users_str: None,
                agent_allowed_users_str: Some("77 88".to_string()),
                manager_allowed_users_str: Some("88".to_string()),
                topic_configs: Vec::new(),
            },
        );

        let thread_keys = agent_mode_session_keys(
            77,
            ChatId(-100_123),
            Some(ThreadId(MessageId(42))),
            "flow-a",
        );
        let general_spec = resolve_thread_spec_from_context(true, true, None);
        let created_topic_spec =
            resolve_thread_spec_from_context(true, true, Some(ThreadId(MessageId(42))));

        assert_ne!(thread_keys.primary, thread_keys.legacy);
        assert!(manager_control_plane_enabled(&settings, 88, general_spec));
        assert!(!manager_control_plane_enabled(
            &settings,
            88,
            created_topic_spec
        ));
        assert!(!manager_control_plane_enabled(&settings, 77, general_spec));
    }

    #[tokio::test]
    async fn threaded_transport_session_disables_manager_tools_inside_created_topics() {
        let bot = Bot::new("token");
        let chat_id = ChatId(-100_123);
        let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::default());
        let manager_settings = test_settings(Some("88"));
        let llm = test_llm(&manager_settings);

        let general_thread = general_forum_topic_id();
        let blocked_thread = ThreadId(MessageId(43));
        let allowed_keys = agent_mode_session_keys(88, chat_id, Some(general_thread), "flow-a");
        let blocked_keys = agent_mode_session_keys(77, chat_id, Some(blocked_thread), "flow-a");

        remove_sessions_with_compat(allowed_keys).await;
        remove_sessions_with_compat(blocked_keys).await;

        let allowed_session = ensure_session_exists(EnsureSessionContext {
            session_keys: allowed_keys,
            context_key: "allowed".to_string(),
            agent_flow_id: "flow-a".to_string(),
            agent_flow_created: false,
            sandbox_scope: test_sandbox_scope(88, "allowed"),
            user_id: 88,
            bot: &bot,
            transport_ctx: SessionTransportContext {
                manager_default_chat_id: Some(chat_id),
                thread_spec: resolve_thread_spec_from_context(true, true, Some(general_thread)),
            },
            llm: &llm,
            storage: &storage,
            settings: &manager_settings,
        })
        .await;
        let blocked_session = ensure_session_exists(EnsureSessionContext {
            session_keys: blocked_keys,
            context_key: "blocked".to_string(),
            agent_flow_id: "flow-a".to_string(),
            agent_flow_created: false,
            sandbox_scope: test_sandbox_scope(77, "blocked"),
            user_id: 77,
            bot: &bot,
            transport_ctx: SessionTransportContext {
                manager_default_chat_id: Some(chat_id),
                thread_spec: resolve_thread_spec_from_context(true, true, Some(blocked_thread)),
            },
            llm: &llm,
            storage: &storage,
            settings: &manager_settings,
        })
        .await;

        assert_eq!(allowed_session, allowed_keys.primary);
        assert_eq!(blocked_session, blocked_keys.primary);
        assert_eq!(
            session_manager_control_plane_enabled(allowed_session).await,
            Some(true)
        );
        assert_eq!(
            session_manager_control_plane_enabled(blocked_session).await,
            Some(false)
        );

        remove_sessions_with_compat(allowed_keys).await;
        remove_sessions_with_compat(blocked_keys).await;
    }

    #[tokio::test]
    async fn threaded_transport_session_keeps_manager_tools_disabled_for_allowlisted_created_topic()
    {
        let bot = Bot::new("token");
        let chat_id = ChatId(-100_123);
        let thread_id = ThreadId(MessageId(42));
        let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::default());
        let manager_settings = test_settings(Some("88"));
        let llm = test_llm(&manager_settings);
        let keys = agent_mode_session_keys(88, chat_id, Some(thread_id), "flow-a");

        remove_sessions_with_compat(keys).await;

        let session_id = ensure_session_exists(EnsureSessionContext {
            session_keys: keys,
            context_key: "topic-a".to_string(),
            agent_flow_id: "flow-a".to_string(),
            agent_flow_created: false,
            sandbox_scope: test_sandbox_scope(88, "topic-a"),
            user_id: 88,
            bot: &bot,
            transport_ctx: SessionTransportContext {
                manager_default_chat_id: Some(chat_id),
                thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
            },
            llm: &llm,
            storage: &storage,
            settings: &manager_settings,
        })
        .await;

        assert_eq!(
            session_manager_control_plane_enabled(session_id).await,
            Some(false)
        );

        remove_sessions_with_compat(keys).await;
    }

    #[tokio::test]
    async fn new_flow_injects_topic_agents_md_once() {
        let bot = Bot::new("token");
        let chat_id = ChatId(-100_123);
        let thread_id = general_forum_topic_id();
        let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::with_topic_agents_md(
            "# Topic AGENTS\nUse deploy checklist.",
        ));
        let settings = test_settings(None);
        let llm = test_llm(&settings);
        let keys = agent_mode_session_keys(77, chat_id, Some(thread_id), "flow-agents");

        remove_sessions_with_compat(keys).await;

        let session_id = ensure_session_exists(EnsureSessionContext {
            session_keys: keys,
            context_key: "topic-a".to_string(),
            agent_flow_id: "flow-agents".to_string(),
            agent_flow_created: true,
            sandbox_scope: test_sandbox_scope(77, "topic-a"),
            user_id: 77,
            bot: &bot,
            transport_ctx: SessionTransportContext {
                manager_default_chat_id: Some(chat_id),
                thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
            },
            llm: &llm,
            storage: &storage,
            settings: &settings,
        })
        .await;

        let executor = SESSION_REGISTRY
            .get(&session_id)
            .await
            .expect("session must exist");
        let executor = executor.read().await;
        let memory = &executor.session().memory;
        let pinned_count = memory
            .get_messages()
            .iter()
            .filter(|message| message.content.starts_with("[TOPIC_AGENTS_MD]\n"))
            .count();

        assert!(memory.has_topic_agents_md());
        assert_eq!(pinned_count, 1);

        remove_sessions_with_compat(keys).await;
    }

    #[tokio::test]
    async fn restored_flow_does_not_duplicate_topic_agents_md() {
        let bot = Bot::new("token");
        let chat_id = ChatId(-100_123);
        let thread_id = general_forum_topic_id();
        let mut flow_memory = AgentMemory::new(10_000);
        flow_memory.add_message(
            oxide_agent_core::agent::memory::AgentMessage::topic_agents_md(
                "# Topic AGENTS\nUse deploy checklist.",
            ),
        );
        let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage {
            flow_memory: Some(flow_memory),
            agent_profile: None,
            topic_context: None,
            topic_agents_md: Some("# Topic AGENTS\nUse deploy checklist.".to_string()),
            fail_flow_memory_lookup: false,
            cleared_flows: Arc::default(),
        });
        let settings = test_settings(None);
        let llm = test_llm(&settings);
        let keys = agent_mode_session_keys(77, chat_id, Some(thread_id), "flow-existing");

        remove_sessions_with_compat(keys).await;

        let session_id = ensure_session_exists(EnsureSessionContext {
            session_keys: keys,
            context_key: "topic-a".to_string(),
            agent_flow_id: "flow-existing".to_string(),
            agent_flow_created: false,
            sandbox_scope: test_sandbox_scope(77, "topic-a"),
            user_id: 77,
            bot: &bot,
            transport_ctx: SessionTransportContext {
                manager_default_chat_id: Some(chat_id),
                thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
            },
            llm: &llm,
            storage: &storage,
            settings: &settings,
        })
        .await;

        let executor = SESSION_REGISTRY
            .get(&session_id)
            .await
            .expect("session must exist");
        let executor = executor.read().await;
        let pinned_count = executor
            .session()
            .memory
            .get_messages()
            .iter()
            .filter(|message| message.content.starts_with("[TOPIC_AGENTS_MD]\n"))
            .count();

        assert_eq!(pinned_count, 1);

        remove_sessions_with_compat(keys).await;
    }

    #[tokio::test]
    async fn threaded_transport_session_recreates_primary_when_manager_rbac_changes() {
        let flow_id = "flow-rbac-refresh";
        let bot = Bot::new("token");
        let chat_id = ChatId(-100_123);
        let thread_id = general_forum_topic_id();
        let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::default());
        let allowed_settings = test_settings(Some("77"));
        let restricted_settings = test_settings(None);
        let llm = test_llm(&allowed_settings);
        let keys = agent_mode_session_keys(77, chat_id, Some(thread_id), flow_id);

        remove_sessions_with_compat(keys).await;

        let first_session = ensure_session_exists(EnsureSessionContext {
            session_keys: keys,
            context_key: "topic-a".to_string(),
            agent_flow_id: flow_id.to_string(),
            agent_flow_created: false,
            sandbox_scope: test_sandbox_scope(77, "topic-a"),
            user_id: 77,
            bot: &bot,
            transport_ctx: SessionTransportContext {
                manager_default_chat_id: Some(chat_id),
                thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
            },
            llm: &llm,
            storage: &storage,
            settings: &allowed_settings,
        })
        .await;
        assert_eq!(first_session, keys.primary);
        assert_eq!(
            session_manager_control_plane_enabled(first_session).await,
            Some(true)
        );

        let second_session = ensure_session_exists(EnsureSessionContext {
            session_keys: keys,
            context_key: "topic-a".to_string(),
            agent_flow_id: flow_id.to_string(),
            agent_flow_created: false,
            sandbox_scope: test_sandbox_scope(77, "topic-a"),
            user_id: 77,
            bot: &bot,
            transport_ctx: SessionTransportContext {
                manager_default_chat_id: Some(chat_id),
                thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
            },
            llm: &llm,
            storage: &storage,
            settings: &restricted_settings,
        })
        .await;
        assert_eq!(second_session, keys.primary);
        assert_eq!(
            session_manager_control_plane_enabled(second_session).await,
            Some(false)
        );

        remove_sessions_with_compat(keys).await;
    }

    #[tokio::test]
    async fn threaded_transport_session_defers_rbac_refresh_while_running_then_refreshes_after_complete(
    ) {
        let flow_id = "flow-rbac-running";
        let bot = Bot::new("token");
        let chat_id = ChatId(-100_123);
        let thread_id = general_forum_topic_id();
        let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::default());
        let allowed_settings = test_settings(Some("77"));
        let restricted_settings = test_settings(None);
        let llm = test_llm(&allowed_settings);
        let keys = agent_mode_session_keys(77, chat_id, Some(thread_id), flow_id);

        remove_sessions_with_compat(keys).await;

        let first_session = ensure_session_exists(EnsureSessionContext {
            session_keys: keys,
            context_key: "topic-a".to_string(),
            agent_flow_id: flow_id.to_string(),
            agent_flow_created: false,
            sandbox_scope: test_sandbox_scope(77, "topic-a"),
            user_id: 77,
            bot: &bot,
            transport_ctx: SessionTransportContext {
                manager_default_chat_id: Some(chat_id),
                thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
            },
            llm: &llm,
            storage: &storage,
            settings: &allowed_settings,
        })
        .await;
        assert_eq!(first_session, keys.primary);
        assert_eq!(
            session_manager_control_plane_enabled(first_session).await,
            Some(true)
        );

        let marked_running = SESSION_REGISTRY
            .with_executor_mut(&first_session, |executor| {
                Box::pin(async move {
                    executor.session_mut().start_task();
                })
            })
            .await;
        assert!(marked_running.is_ok());

        let second_session = ensure_session_exists(EnsureSessionContext {
            session_keys: keys,
            context_key: "topic-a".to_string(),
            agent_flow_id: flow_id.to_string(),
            agent_flow_created: false,
            sandbox_scope: test_sandbox_scope(77, "topic-a"),
            user_id: 77,
            bot: &bot,
            transport_ctx: SessionTransportContext {
                manager_default_chat_id: Some(chat_id),
                thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
            },
            llm: &llm,
            storage: &storage,
            settings: &restricted_settings,
        })
        .await;
        assert_eq!(second_session, first_session);
        assert_eq!(
            session_manager_control_plane_enabled(second_session).await,
            Some(true)
        );

        let marked_completed = SESSION_REGISTRY
            .with_executor_mut(&first_session, |executor| {
                Box::pin(async move {
                    executor.session_mut().complete();
                })
            })
            .await;
        assert!(marked_completed.is_ok());

        let third_session = ensure_session_exists(EnsureSessionContext {
            session_keys: keys,
            context_key: "topic-a".to_string(),
            agent_flow_id: flow_id.to_string(),
            agent_flow_created: false,
            sandbox_scope: test_sandbox_scope(77, "topic-a"),
            user_id: 77,
            bot: &bot,
            transport_ctx: SessionTransportContext {
                manager_default_chat_id: Some(chat_id),
                thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
            },
            llm: &llm,
            storage: &storage,
            settings: &restricted_settings,
        })
        .await;
        assert_eq!(third_session, first_session);
        assert_eq!(
            session_manager_control_plane_enabled(third_session).await,
            Some(false)
        );

        remove_sessions_with_compat(keys).await;
    }

    #[tokio::test]
    async fn threaded_transport_session_does_not_bypass_rbac_via_legacy_fallback() {
        let bot = Bot::new("token");
        let chat_id = ChatId(-100_123);
        let thread_id = ThreadId(MessageId(52));
        let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::default());
        let legacy_manager_settings = test_settings(Some("77"));
        let llm = test_llm(&legacy_manager_settings);
        let keys = agent_mode_session_keys(77, chat_id, Some(thread_id), "flow-a");

        remove_sessions_with_compat(keys).await;

        let legacy_executor = oxide_agent_core::agent::AgentExecutor::new(
            llm.clone(),
            AgentSession::new(keys.legacy),
            legacy_manager_settings.agent.clone(),
        )
        .with_manager_control_plane(storage.clone(), 77);
        SESSION_REGISTRY.insert(keys.legacy, legacy_executor).await;

        let restricted_settings = test_settings(None);
        let resolved_session = ensure_session_exists(EnsureSessionContext {
            session_keys: keys,
            context_key: "topic-a".to_string(),
            agent_flow_id: "flow-a".to_string(),
            agent_flow_created: false,
            sandbox_scope: test_sandbox_scope(77, "topic-a"),
            user_id: 77,
            bot: &bot,
            transport_ctx: SessionTransportContext {
                manager_default_chat_id: Some(chat_id),
                thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
            },
            llm: &llm,
            storage: &storage,
            settings: &restricted_settings,
        })
        .await;

        assert_eq!(resolved_session, keys.primary);
        assert_eq!(
            session_manager_control_plane_enabled(resolved_session).await,
            Some(false)
        );

        remove_sessions_with_compat(keys).await;
    }

    #[tokio::test]
    async fn threaded_transport_session_migrates_idle_legacy_session_to_primary_scope() {
        let bot = Bot::new("token");
        let chat_id = ChatId(-100_123);
        let thread_id = ThreadId(MessageId(53));
        let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::default());
        let settings = test_settings(None);
        let llm = test_llm(&settings);
        let keys = agent_mode_session_keys(77, chat_id, Some(thread_id), "flow-a");

        remove_sessions_with_compat(keys).await;

        let legacy_executor = oxide_agent_core::agent::AgentExecutor::new(
            llm.clone(),
            AgentSession::new(keys.legacy),
            settings.agent.clone(),
        );
        SESSION_REGISTRY.insert(keys.legacy, legacy_executor).await;

        let resolved_session = ensure_session_exists(EnsureSessionContext {
            session_keys: keys,
            context_key: "topic-a".to_string(),
            agent_flow_id: "flow-a".to_string(),
            agent_flow_created: false,
            sandbox_scope: test_sandbox_scope(77, "topic-a"),
            user_id: 77,
            bot: &bot,
            transport_ctx: SessionTransportContext {
                manager_default_chat_id: Some(chat_id),
                thread_spec: resolve_thread_spec_from_context(true, true, Some(thread_id)),
            },
            llm: &llm,
            storage: &storage,
            settings: &settings,
        })
        .await;

        assert_eq!(resolved_session, keys.primary);
        assert!(SESSION_REGISTRY.contains(&keys.primary).await);
        assert!(!SESSION_REGISTRY.contains(&keys.legacy).await);

        remove_sessions_with_compat(keys).await;
    }

    #[tokio::test]
    async fn detach_creates_new_flow_only_when_current_flow_has_saved_memory() {
        let storage: Arc<dyn StorageProvider> =
            Arc::new(NoopStorage::with_flow_memory(Some(AgentMemory::new(1024))));

        assert!(should_create_fresh_flow_on_detach(&storage, 77, "-100123:42", "flow-a").await);
    }

    #[tokio::test]
    async fn detach_reuses_current_flow_when_current_flow_has_no_saved_memory() {
        let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::with_flow_memory(None));

        assert!(!should_create_fresh_flow_on_detach(&storage, 77, "-100123:42", "flow-a").await);
    }

    #[tokio::test]
    async fn detach_falls_back_to_reset_when_memory_lookup_fails() {
        let storage: Arc<dyn StorageProvider> =
            Arc::new(NoopStorage::with_failed_flow_memory_lookup());

        assert!(should_create_fresh_flow_on_detach(&storage, 77, "-100123:42", "flow-a").await);
    }

    #[tokio::test]
    async fn attach_cleanup_deletes_abandoned_empty_flow_record() {
        let storage = NoopStorage::with_flow_memory(None);
        let cleared_flows = storage.cleared_flows.clone();
        let storage: Arc<dyn StorageProvider> = Arc::new(storage);

        cleanup_abandoned_empty_flow(&storage, 77, "-100123:42", "flow-empty").await;

        let cleared_flows = cleared_flows
            .lock()
            .expect("cleared_flows mutex poisoned")
            .clone();
        assert_eq!(
            cleared_flows,
            vec![("-100123:42".to_string(), "flow-empty".to_string())]
        );
    }

    #[tokio::test]
    async fn attach_cleanup_keeps_non_empty_flow_record() {
        let storage = NoopStorage::with_flow_memory(Some(AgentMemory::new(1024)));
        let cleared_flows = storage.cleared_flows.clone();
        let storage: Arc<dyn StorageProvider> = Arc::new(storage);

        cleanup_abandoned_empty_flow(&storage, 77, "-100123:42", "flow-a").await;

        assert!(cleared_flows
            .lock()
            .expect("cleared_flows mutex poisoned")
            .is_empty());
    }

    #[tokio::test]
    async fn attach_cleanup_skips_delete_when_memory_lookup_fails() {
        let storage = NoopStorage::with_failed_flow_memory_lookup();
        let cleared_flows = storage.cleared_flows.clone();
        let storage: Arc<dyn StorageProvider> = Arc::new(storage);

        cleanup_abandoned_empty_flow(&storage, 77, "-100123:42", "flow-a").await;

        assert!(cleared_flows
            .lock()
            .expect("cleared_flows mutex poisoned")
            .is_empty());
    }

    #[test]
    fn merge_prompt_instructions_deduplicates_identical_sections() {
        let merged = merge_prompt_instructions(Some("infra rules"), Some("infra rules"));

        assert_eq!(merged.as_deref(), Some("infra rules"));
    }

    #[tokio::test]
    async fn resolve_execution_profile_loads_profile_policy_for_static_topic_agent() {
        let storage: Arc<dyn StorageProvider> =
            Arc::new(NoopStorage::with_agent_profile_and_topic_context(
                serde_json::json!({
                    "systemPrompt": "act as infra agent",
                    "allowedTools": ["todos_write", "execute_command"],
                    "blockedTools": ["delegate_to_sub_agent"],
                    "disabledHooks": ["search_budget"]
                }),
                "persistent topic runbook",
            ));
        let route = crate::bot::topic_route::TopicRouteDecision {
            enabled: true,
            require_mention: false,
            mention_satisfied: true,
            system_prompt_override: Some("topic-specific note".to_string()),
            agent_id: Some("infra-agent".to_string()),
            dynamic_binding_topic_id: None,
        };

        let profile = resolve_execution_profile(&storage, 77, "topic-a", &route, false).await;

        assert_eq!(profile.agent_id(), Some("infra-agent"));
        let prompt = profile
            .prompt_instructions()
            .expect("profile prompt must be resolved");
        assert!(prompt.contains("Profile instructions:"));
        assert!(prompt.contains("Topic instructions:"));
        assert!(prompt.contains("Persistent topic context:"));
        assert!(prompt.contains("persistent topic runbook"));
        assert!(profile.tool_policy().allows("todos_write"));
        assert!(profile.tool_policy().allows("reminder_schedule"));
        assert!(!profile.tool_policy().allows("delegate_to_sub_agent"));
        assert!(!profile.tool_policy().allows("file_write"));
        assert!(!profile.hook_policy().allows("search_budget"));
    }

    #[tokio::test]
    async fn resolve_execution_profile_applies_manager_default_blocklist() {
        let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage::default());
        let route = crate::bot::topic_route::TopicRouteDecision {
            enabled: true,
            require_mention: false,
            mention_satisfied: true,
            system_prompt_override: None,
            agent_id: None,
            dynamic_binding_topic_id: None,
        };

        let profile = resolve_execution_profile(&storage, 77, "manager-topic", &route, true).await;

        assert!(profile.tool_policy().allows("execute_command"));
        assert!(!profile.tool_policy().allows("delegate_to_sub_agent"));
        assert!(!profile.tool_policy().allows("ytdlp_get_video_metadata"));
        assert!(!profile.tool_policy().allows("ytdlp_download_video"));
    }

    #[tokio::test]
    async fn resolve_execution_profile_keeps_manager_tools_available_with_profile_allowlist() {
        let storage: Arc<dyn StorageProvider> =
            Arc::new(NoopStorage::with_agent_profile_and_topic_context(
                serde_json::json!({
                    "systemPrompt": "act as control plane agent",
                    "allowedTools": ["execute_command"],
                }),
                "manager topic context",
            ));
        let route = crate::bot::topic_route::TopicRouteDecision {
            enabled: true,
            require_mention: false,
            mention_satisfied: true,
            system_prompt_override: None,
            agent_id: Some("control-plane".to_string()),
            dynamic_binding_topic_id: None,
        };

        let profile = resolve_execution_profile(&storage, 77, "manager-topic", &route, true).await;

        assert!(profile.tool_policy().allows("execute_command"));
        assert!(profile.tool_policy().allows("topic_agents_md_upsert"));
        assert!(profile.tool_policy().allows("topic_agents_md_get"));
        assert!(profile.tool_policy().allows("forum_topic_list"));
        assert!(!profile.tool_policy().allows("delegate_to_sub_agent"));
    }
}

pub(crate) fn spawn_reminder_scheduler(
    bot: Bot,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    settings: Arc<BotSettings>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(REMINDER_POLL_INTERVAL_SECS));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            if let Err(error) = process_due_reminders(&bot, &storage, &llm, &settings).await {
                warn!(error = %error, "Reminder scheduler poll failed");
            }
        }
    });
}

async fn process_due_reminders(
    bot: &Bot,
    storage: &Arc<dyn StorageProvider>,
    llm: &Arc<LlmClient>,
    settings: &Arc<BotSettings>,
) -> Result<()> {
    for user_id in settings.telegram.agent_allowed_users() {
        let now = current_timestamp_unix_secs();
        let reminders = match storage
            .list_due_reminder_jobs(user_id, now, REMINDER_BATCH_LIMIT)
            .await
        {
            Ok(reminders) => reminders,
            Err(error) => {
                warn!(user_id, error = %error, "Failed to list due reminders");
                continue;
            }
        };

        for reminder in reminders {
            if let Err(error) = process_due_reminder(bot, storage, llm, settings, reminder).await {
                warn!(error = %error, "Failed to execute due reminder");
            }
        }
    }

    Ok(())
}

async fn process_due_reminder(
    bot: &Bot,
    storage: &Arc<dyn StorageProvider>,
    llm: &Arc<LlmClient>,
    settings: &Arc<BotSettings>,
    reminder: ReminderJobRecord,
) -> Result<()> {
    let now = current_timestamp_unix_secs();
    let Some(reminder) = storage
        .claim_reminder_job(
            reminder.user_id,
            reminder.reminder_id.clone(),
            now.saturating_add(REMINDER_LEASE_SECS),
            now,
        )
        .await?
    else {
        return Ok(());
    };

    let chat_id = ChatId(reminder.chat_id);
    let thread_spec = thread_spec_from_reminder(&reminder);
    let session_keys = agent_mode_session_keys(
        reminder.user_id,
        chat_id,
        thread_spec.thread_id,
        &reminder.flow_id,
    );
    let manager_enabled = manager_control_plane_enabled(settings, reminder.user_id, thread_spec);
    let session_id = ensure_session_exists(EnsureSessionContext {
        session_keys,
        context_key: reminder.context_key.clone(),
        agent_flow_id: reminder.flow_id.clone(),
        agent_flow_created: false,
        sandbox_scope: sandbox_scope(reminder.user_id, chat_id, thread_spec),
        user_id: reminder.user_id,
        bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: manager_default_chat_id(chat_id, thread_spec),
            thread_spec,
        },
        llm,
        storage,
        settings,
    })
    .await;

    if is_agent_task_running(session_id).await {
        defer_busy_reminder(storage, &reminder).await;
        return Ok(());
    }

    let route = resolve_scheduled_topic_route(
        storage,
        reminder.user_id,
        settings,
        &reminder.context_key,
        chat_id,
        thread_spec,
    )
    .await;
    let execution_profile = resolve_execution_profile(
        storage,
        reminder.user_id,
        &reminder.context_key,
        &route,
        manager_enabled,
    )
    .await;
    let topic_infra_config =
        resolve_topic_infra_config(storage, reminder.user_id, &reminder.context_key).await;

    apply_execution_profile(session_id, execution_profile).await;
    apply_topic_infra_config(
        session_id,
        storage.clone(),
        reminder.user_id,
        reminder.context_key.clone(),
        topic_infra_config,
    )
    .await;
    apply_reminder_context(
        session_id,
        storage.clone(),
        reminder.user_id,
        reminder.context_key.clone(),
        reminder.flow_id.clone(),
        chat_id,
        thread_spec,
    )
    .await;
    renew_cancellation_token(session_id).await;

    let result = run_agent_task_with_text(RunAgentTaskTextContext {
        bot: bot.clone(),
        chat_id,
        session_id,
        user_id: reminder.user_id,
        task_text: scheduled_reminder_task_text(&reminder),
        storage: storage.clone(),
        context_key: reminder.context_key.clone(),
        agent_flow_id: reminder.flow_id.clone(),
        message_thread_id: build_outbound_thread_params(thread_spec).message_thread_id,
        use_inline_progress_controls: use_inline_topic_controls(thread_spec),
    })
    .await;

    finalize_reminder_execution(storage, &reminder, result.as_ref()).await;
    touch_dynamic_binding_activity_if_needed(storage.as_ref(), reminder.user_id, &route).await;
    result
}

async fn resolve_scheduled_topic_route(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    settings: &Arc<BotSettings>,
    context_key: &str,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> TopicRouteDecision {
    let now = current_timestamp_unix_secs();
    let binding = match storage
        .get_topic_binding(user_id, context_key.to_string())
        .await
    {
        Ok(record) => resolve_active_topic_binding(record, now),
        Err(error) => {
            warn!(error = %error, user_id, topic_id = %context_key, "Failed to resolve binding for scheduled reminder");
            None
        }
    };

    if let Some(binding) = binding {
        return TopicRouteDecision {
            enabled: true,
            require_mention: false,
            mention_satisfied: true,
            system_prompt_override: None,
            agent_id: Some(binding.agent_id),
            dynamic_binding_topic_id: Some(binding.topic_id),
        };
    }

    let thread_id = thread_spec.thread_id.map(|thread_id| thread_id.0 .0);
    match settings.telegram.resolve_topic_config(chat_id.0, thread_id) {
        Some(topic) => TopicRouteDecision {
            enabled: topic.enabled,
            require_mention: topic.require_mention,
            mention_satisfied: true,
            system_prompt_override: topic.system_prompt.clone(),
            agent_id: topic.agent_id.clone(),
            dynamic_binding_topic_id: None,
        },
        None => TopicRouteDecision {
            enabled: true,
            require_mention: false,
            mention_satisfied: true,
            system_prompt_override: None,
            agent_id: None,
            dynamic_binding_topic_id: None,
        },
    }
}

async fn defer_busy_reminder(storage: &Arc<dyn StorageProvider>, reminder: &ReminderJobRecord) {
    let next_run_at = current_timestamp_unix_secs().saturating_add(REMINDER_BUSY_BACKOFF_SECS);
    let _ = storage
        .reschedule_reminder_job(
            reminder.user_id,
            reminder.reminder_id.clone(),
            next_run_at,
            None,
            Some("Agent session is busy; reminder deferred.".to_string()),
            false,
        )
        .await;
}

async fn finalize_reminder_execution(
    storage: &Arc<dyn StorageProvider>,
    reminder: &ReminderJobRecord,
    result: std::result::Result<&(), &Error>,
) {
    let now = current_timestamp_unix_secs();

    match result {
        Ok(()) if reminder.is_recurring() => {
            finalize_recurring_reminder_success(storage, reminder, now).await;
        }
        Ok(()) => finalize_one_shot_reminder_success(storage, reminder, now).await,
        Err(error) if reminder.is_recurring() => {
            finalize_recurring_reminder_failure(storage, reminder, now, &error.to_string()).await;
        }
        Err(error) => {
            finalize_one_shot_reminder_failure(storage, reminder, now, &error.to_string()).await;
        }
    }
}

async fn finalize_recurring_reminder_success(
    storage: &Arc<dyn StorageProvider>,
    reminder: &ReminderJobRecord,
    now: i64,
) {
    let Some(next_run_at) = resolve_recurring_next_run(storage, reminder, now, None).await else {
        return;
    };
    let _ = storage
        .reschedule_reminder_job(
            reminder.user_id,
            reminder.reminder_id.clone(),
            next_run_at,
            Some(now),
            None,
            true,
        )
        .await;
    let _ = append_reminder_audit_event(
        storage,
        reminder,
        "reminder_job_completed",
        serde_json::json!({
            "next_run_at": next_run_at,
            "recurring": true,
        }),
    )
    .await;
}

async fn finalize_one_shot_reminder_success(
    storage: &Arc<dyn StorageProvider>,
    reminder: &ReminderJobRecord,
    now: i64,
) {
    let _ = storage
        .complete_reminder_job(reminder.user_id, reminder.reminder_id.clone(), now)
        .await;
    let _ = append_reminder_audit_event(
        storage,
        reminder,
        "reminder_job_completed",
        serde_json::json!({
            "completed_at": now,
            "recurring": false,
        }),
    )
    .await;
    let _ = storage
        .delete_reminder_job(reminder.user_id, reminder.reminder_id.clone())
        .await;
}

async fn finalize_recurring_reminder_failure(
    storage: &Arc<dyn StorageProvider>,
    reminder: &ReminderJobRecord,
    now: i64,
    error_text: &str,
) {
    let Some(next_run_at) =
        resolve_recurring_next_run(storage, reminder, now, Some(error_text.to_string())).await
    else {
        return;
    };
    let _ = storage
        .reschedule_reminder_job(
            reminder.user_id,
            reminder.reminder_id.clone(),
            next_run_at,
            Some(now),
            Some(error_text.to_string()),
            false,
        )
        .await;
    let _ = append_reminder_audit_event(
        storage,
        reminder,
        "reminder_job_failed",
        serde_json::json!({
            "error": error_text,
            "next_run_at": next_run_at,
            "recurring": true,
        }),
    )
    .await;
}

async fn finalize_one_shot_reminder_failure(
    storage: &Arc<dyn StorageProvider>,
    reminder: &ReminderJobRecord,
    now: i64,
    error_text: &str,
) {
    let _ = storage
        .fail_reminder_job(
            reminder.user_id,
            reminder.reminder_id.clone(),
            now,
            error_text.to_string(),
        )
        .await;
    let _ = append_reminder_audit_event(
        storage,
        reminder,
        "reminder_job_failed",
        serde_json::json!({
            "error": error_text,
            "recurring": false,
        }),
    )
    .await;
}

async fn resolve_recurring_next_run(
    storage: &Arc<dyn StorageProvider>,
    reminder: &ReminderJobRecord,
    now: i64,
    error_text: Option<String>,
) -> Option<i64> {
    match compute_next_reminder_run_at(reminder, now) {
        Ok(Some(next_run_at)) => Some(next_run_at),
        Ok(None) => {
            let _ = storage
                .complete_reminder_job(reminder.user_id, reminder.reminder_id.clone(), now)
                .await;
            None
        }
        Err(schedule_error) => {
            let combined_error = match error_text {
                Some(error_text) => format!("{error_text}; reschedule failed: {schedule_error}"),
                None => schedule_error.to_string(),
            };
            let _ = storage
                .fail_reminder_job(
                    reminder.user_id,
                    reminder.reminder_id.clone(),
                    now,
                    combined_error.clone(),
                )
                .await;
            let _ = append_reminder_audit_event(
                storage,
                reminder,
                "reminder_job_failed",
                serde_json::json!({
                    "error": combined_error,
                    "recurring": true,
                }),
            )
            .await;
            None
        }
    }
}

async fn append_reminder_audit_event(
    storage: &Arc<dyn StorageProvider>,
    reminder: &ReminderJobRecord,
    action: &str,
    payload: serde_json::Value,
) -> Result<()> {
    storage
        .append_audit_event(oxide_agent_core::storage::AppendAuditEventOptions {
            user_id: reminder.user_id,
            topic_id: Some(reminder.context_key.clone()),
            agent_id: None,
            action: action.to_string(),
            payload: serde_json::json!({
                "reminder_id": reminder.reminder_id.clone(),
                "flow_id": reminder.flow_id.clone(),
                "payload": payload,
            }),
        })
        .await?;
    Ok(())
}

fn scheduled_reminder_task_text(reminder: &ReminderJobRecord) -> String {
    format!(
        "Scheduled wake-up reminder.\nReminder ID: {}\nSchedule: {:?}\nCurrent time (unix): {}\n\nTask:\n{}\n\nExecute the task now and send the user a concise report.",
        reminder.reminder_id,
        reminder.schedule_kind,
        current_timestamp_unix_secs(),
        reminder.task_prompt,
    )
}

fn current_timestamp_unix_secs() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

#[derive(Clone)]
struct LoopCallbackContext {
    bot: Bot,
    chat_id: ChatId,
    context_key: String,
    agent_flow_id: String,
    user_id: i64,
    session_keys: AgentModeSessionKeys,
    manager_default_chat_id: Option<ChatId>,
    thread_spec: TelegramThreadSpec,
    outbound_thread: OutboundThreadParams,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AgentCallbackAction {
    LoopRetry,
    LoopReset,
    LoopCancel,
    Attach(String),
    Detach,
    ApproveSsh(String),
    RejectSsh(String),
    StartCancelTaskConfirmation,
    ResolveCancelTaskConfirmation(bool),
    StartConfirmation(ConfirmationType),
    Exit,
    ResolveConfirmation(ConfirmationType, bool),
}

struct AgentCallbackContext {
    callback_id: teloxide::types::CallbackQueryId,
    loop_ctx: LoopCallbackContext,
    msg: Message,
    dialogue: AgentDialogue,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    settings: Arc<BotSettings>,
}

fn parse_agent_callback_action(data: &str) -> Option<AgentCallbackAction> {
    if data == AGENT_CALLBACK_DETACH {
        return Some(AgentCallbackAction::Detach);
    }
    if let Some(flow_id) = data.strip_prefix(AGENT_CALLBACK_ATTACH_PREFIX) {
        return Some(AgentCallbackAction::Attach(flow_id.to_string()));
    }
    if let Some(request_id) = data.strip_prefix(AGENT_CALLBACK_SSH_APPROVE_PREFIX) {
        return Some(AgentCallbackAction::ApproveSsh(request_id.to_string()));
    }
    if let Some(request_id) = data.strip_prefix(AGENT_CALLBACK_SSH_REJECT_PREFIX) {
        return Some(AgentCallbackAction::RejectSsh(request_id.to_string()));
    }

    match data {
        LOOP_CALLBACK_RETRY => Some(AgentCallbackAction::LoopRetry),
        LOOP_CALLBACK_RESET => Some(AgentCallbackAction::LoopReset),
        LOOP_CALLBACK_CANCEL => Some(AgentCallbackAction::LoopCancel),
        AGENT_CALLBACK_CANCEL_TASK => Some(AgentCallbackAction::StartCancelTaskConfirmation),
        AGENT_CALLBACK_CONFIRM_CANCEL_YES => {
            Some(AgentCallbackAction::ResolveCancelTaskConfirmation(true))
        }
        AGENT_CALLBACK_CONFIRM_CANCEL_NO => {
            Some(AgentCallbackAction::ResolveCancelTaskConfirmation(false))
        }
        AGENT_CALLBACK_CLEAR_MEMORY => Some(AgentCallbackAction::StartConfirmation(
            ConfirmationType::ClearMemory,
        )),
        AGENT_CALLBACK_RECREATE_CONTAINER => Some(AgentCallbackAction::StartConfirmation(
            ConfirmationType::RecreateContainer,
        )),
        AGENT_CALLBACK_EXIT => Some(AgentCallbackAction::Exit),
        AGENT_CALLBACK_CONFIRM_CLEAR_YES => Some(AgentCallbackAction::ResolveConfirmation(
            ConfirmationType::ClearMemory,
            true,
        )),
        AGENT_CALLBACK_CONFIRM_CLEAR_CANCEL => Some(AgentCallbackAction::ResolveConfirmation(
            ConfirmationType::ClearMemory,
            false,
        )),
        AGENT_CALLBACK_CONFIRM_RECREATE_YES => Some(AgentCallbackAction::ResolveConfirmation(
            ConfirmationType::RecreateContainer,
            true,
        )),
        AGENT_CALLBACK_CONFIRM_RECREATE_CANCEL => Some(AgentCallbackAction::ResolveConfirmation(
            ConfirmationType::RecreateContainer,
            false,
        )),
        _ => None,
    }
}

fn is_valid_agent_flow_id(flow_id: &str) -> bool {
    if flow_id.len() != 36 {
        return false;
    }

    for (idx, ch) in flow_id.chars().enumerate() {
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

fn short_flow_id(flow_id: &str) -> String {
    flow_id.chars().take(8).collect()
}

async fn handle_loop_retry(
    ctx: &LoopCallbackContext,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    settings: Arc<BotSettings>,
) -> Result<()> {
    let session_id = ensure_session_exists(EnsureSessionContext {
        session_keys: ctx.session_keys,
        context_key: ctx.context_key.clone(),
        agent_flow_id: ctx.agent_flow_id.clone(),
        agent_flow_created: false,
        sandbox_scope: SandboxScope::new(ctx.user_id, ctx.context_key.clone()),
        user_id: ctx.user_id,
        bot: &ctx.bot,
        transport_ctx: SessionTransportContext {
            manager_default_chat_id: ctx.manager_default_chat_id,
            thread_spec: ctx.thread_spec,
        },
        llm: &llm,
        storage: &storage,
        settings: &settings,
    })
    .await;
    if is_agent_task_running(session_id).await {
        send_agent_message(
            &ctx.bot,
            ctx.chat_id,
            DefaultAgentView::task_already_running(),
            ctx.outbound_thread,
        )
        .await?;
        return Ok(());
    }

    renew_cancellation_token(session_id).await;

    let executor_arc = SESSION_REGISTRY.get(&session_id).await;
    let Some(executor_arc) = executor_arc else {
        send_agent_message(
            &ctx.bot,
            ctx.chat_id,
            DefaultAgentView::session_not_found(),
            ctx.outbound_thread,
        )
        .await?;
        return Ok(());
    };

    let task_text = {
        let executor = executor_arc.read().await;
        executor.last_task().map(str::to_string)
    };

    let Some(task_text) = task_text else {
        send_agent_message(
            &ctx.bot,
            ctx.chat_id,
            DefaultAgentView::no_saved_task(),
            ctx.outbound_thread,
        )
        .await?;
        return Ok(());
    };

    {
        let mut executor = executor_arc.write().await;
        executor.disable_loop_detection_next_run();
    }

    let retry_ctx = ctx.clone();
    tokio::spawn(async move {
        let error_bot = retry_ctx.bot.clone();
        if let Err(e) = run_agent_task_with_text(RunAgentTaskTextContext {
            bot: retry_ctx.bot,
            chat_id: retry_ctx.chat_id,
            session_id,
            user_id: retry_ctx.user_id,
            task_text,
            storage,
            context_key: retry_ctx.context_key,
            agent_flow_id: retry_ctx.agent_flow_id,
            message_thread_id: retry_ctx.outbound_thread.message_thread_id,
            use_inline_progress_controls: use_inline_topic_controls(retry_ctx.thread_spec),
        })
        .await
        {
            let _ = send_agent_message(
                &error_bot,
                retry_ctx.chat_id,
                DefaultAgentView::error_message(&e.to_string()),
                retry_ctx.outbound_thread,
            )
            .await;
        }
    });

    Ok(())
}

async fn handle_loop_reset(ctx: &LoopCallbackContext) -> Result<()> {
    // Cancel any running task first to release the executor lock.
    let _ = cancel_and_clear_with_compat(ctx.session_keys).await;

    // Brief yield to allow the run loop to observe cancellation and release locks.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    match reset_sessions_with_compat(ctx.session_keys).await {
        ResetSessionOutcome::Reset => {
            let reply_markup = automatic_agent_control_markup(ctx.thread_spec);
            send_agent_message_with_optional_keyboard(
                &ctx.bot,
                ctx.chat_id,
                DefaultAgentView::task_reset(),
                reply_markup.as_ref(),
                ctx.outbound_thread,
            )
            .await?;
        }
        ResetSessionOutcome::NotFound => {
            send_agent_message(
                &ctx.bot,
                ctx.chat_id,
                DefaultAgentView::session_not_found(),
                ctx.outbound_thread,
            )
            .await?;
        }
        ResetSessionOutcome::Busy => {
            send_agent_message(
                &ctx.bot,
                ctx.chat_id,
                DefaultAgentView::reset_blocked_by_task(),
                ctx.outbound_thread,
            )
            .await?;
        }
    }

    Ok(())
}

async fn handle_ssh_approval_callback(
    ctx: &AgentCallbackContext,
    request_id: String,
) -> Result<()> {
    let Some(session_id) = resolve_existing_session_id(ctx.loop_ctx.session_keys).await else {
        send_agent_message(
            &ctx.loop_ctx.bot,
            ctx.loop_ctx.chat_id,
            DefaultAgentView::session_not_found(),
            ctx.loop_ctx.outbound_thread,
        )
        .await?;
        return Ok(());
    };

    if is_agent_task_running(session_id).await {
        send_agent_message(
            &ctx.loop_ctx.bot,
            ctx.loop_ctx.chat_id,
            DefaultAgentView::task_already_running(),
            ctx.loop_ctx.outbound_thread,
        )
        .await?;
        return Ok(());
    }

    renew_cancellation_token(session_id).await;

    let Some(executor_arc) = SESSION_REGISTRY.get(&session_id).await else {
        send_agent_message(
            &ctx.loop_ctx.bot,
            ctx.loop_ctx.chat_id,
            DefaultAgentView::session_not_found(),
            ctx.loop_ctx.outbound_thread,
        )
        .await?;
        return Ok(());
    };

    let has_saved_task = {
        let executor = executor_arc.read().await;
        executor.last_task().is_some()
    };

    if !has_saved_task {
        send_agent_message(
            &ctx.loop_ctx.bot,
            ctx.loop_ctx.chat_id,
            DefaultAgentView::no_saved_task(),
            ctx.loop_ctx.outbound_thread,
        )
        .await?;
        return Ok(());
    }

    send_agent_message(
        &ctx.loop_ctx.bot,
        ctx.loop_ctx.chat_id,
        "SSH approval granted. Resuming the task.",
        ctx.loop_ctx.outbound_thread,
    )
    .await?;

    let retry_ctx = ctx.loop_ctx.clone();
    let storage = ctx.storage.clone();
    let request_id_for_resume = request_id.clone();
    tokio::spawn(async move {
        let error_bot = retry_ctx.bot.clone();
        if let Err(e) = run_approved_ssh_resume(RunApprovedSshResumeContext {
            bot: retry_ctx.bot,
            chat_id: retry_ctx.chat_id,
            session_id,
            user_id: retry_ctx.user_id,
            request_id: request_id_for_resume,
            storage,
            context_key: retry_ctx.context_key,
            agent_flow_id: retry_ctx.agent_flow_id,
            message_thread_id: retry_ctx.outbound_thread.message_thread_id,
            use_inline_progress_controls: use_inline_topic_controls(retry_ctx.thread_spec),
        })
        .await
        {
            let _ = send_agent_message(
                &error_bot,
                retry_ctx.chat_id,
                DefaultAgentView::error_message(&e.to_string()),
                retry_ctx.outbound_thread,
            )
            .await;
        }
    });

    Ok(())
}

async fn handle_ssh_reject_callback(ctx: &AgentCallbackContext, request_id: String) -> Result<()> {
    let Some(session_id) = resolve_existing_session_id(ctx.loop_ctx.session_keys).await else {
        send_agent_message(
            &ctx.loop_ctx.bot,
            ctx.loop_ctx.chat_id,
            DefaultAgentView::session_not_found(),
            ctx.loop_ctx.outbound_thread,
        )
        .await?;
        return Ok(());
    };

    let Some(executor_arc) = SESSION_REGISTRY.get(&session_id).await else {
        send_agent_message(
            &ctx.loop_ctx.bot,
            ctx.loop_ctx.chat_id,
            DefaultAgentView::session_not_found(),
            ctx.loop_ctx.outbound_thread,
        )
        .await?;
        return Ok(());
    };

    let rejected = {
        let mut executor = executor_arc.write().await;
        executor.reject_ssh_approval(&request_id).await
    };

    let message = if rejected.is_some() {
        "SSH action rejected. The agent will not replay the command."
    } else {
        "SSH approval request not found or already handled."
    };
    send_agent_message(
        &ctx.loop_ctx.bot,
        ctx.loop_ctx.chat_id,
        message,
        ctx.loop_ctx.outbound_thread,
    )
    .await?;
    Ok(())
}

async fn handle_agent_confirmation_callback(
    ctx: &AgentCallbackContext,
    action: ConfirmationType,
    confirmed: bool,
) -> Result<()> {
    let loop_ctx = &ctx.loop_ctx;
    let outbound_thread = build_outbound_thread_params(loop_ctx.thread_spec);
    let reply_markup = automatic_agent_control_markup(loop_ctx.thread_spec);

    ctx.dialogue.update(State::AgentMode).await?;

    let send_ctx = ConfirmationSendCtx {
        bot: &loop_ctx.bot,
        chat_id: loop_ctx.chat_id,
        context_key: &loop_ctx.context_key,
        agent_flow_id: &loop_ctx.agent_flow_id,
        reply_markup,
        manager_default_chat_id: loop_ctx.manager_default_chat_id,
        outbound_thread,
    };

    if confirmed {
        match action {
            ConfirmationType::ClearMemory => {
                handle_clear_memory_confirmation(
                    loop_ctx.user_id,
                    loop_ctx.session_keys,
                    &ctx.storage,
                    loop_ctx.thread_spec,
                    &send_ctx,
                )
                .await?;
            }
            ConfirmationType::RecreateContainer => {
                handle_recreate_container_confirmation(
                    loop_ctx.user_id,
                    loop_ctx.session_keys,
                    &ctx.storage,
                    &ctx.llm,
                    &ctx.settings,
                    &send_ctx,
                )
                .await?;
            }
        }
    } else {
        info!(user_id = loop_ctx.user_id, action = ?action, "User cancelled destructive action");
        send_agent_message_with_optional_keyboard(
            &loop_ctx.bot,
            loop_ctx.chat_id,
            DefaultAgentView::operation_cancelled(),
            send_ctx.reply_markup.as_ref(),
            outbound_thread,
        )
        .await?;
    }

    Ok(())
}

async fn send_cancel_task_confirmation(ctx: &LoopCallbackContext) -> Result<()> {
    let session_id = resolve_existing_session_id(ctx.session_keys)
        .await
        .unwrap_or(ctx.session_keys.primary);
    send_or_update_cancel_confirmation(&ctx.bot, session_id, ctx.chat_id, ctx.outbound_thread).await
}

async fn handle_cancel_task_confirmation_callback(
    ctx: &AgentCallbackContext,
    confirmed: bool,
) -> Result<()> {
    let session_id = resolve_existing_session_id(ctx.loop_ctx.session_keys)
        .await
        .unwrap_or(ctx.loop_ctx.session_keys.primary);
    clear_cancel_confirmation_message(&ctx.loop_ctx.bot, session_id, ctx.loop_ctx.chat_id).await;

    if confirmed {
        cancel_agent_task_by_id(
            ctx.loop_ctx.bot.clone(),
            ctx.loop_ctx.session_keys,
            ctx.loop_ctx.chat_id,
            ctx.loop_ctx.thread_spec,
            ctx.loop_ctx.outbound_thread.message_thread_id,
            &ctx.loop_ctx.agent_flow_id,
        )
        .await
    } else {
        send_agent_message(
            &ctx.loop_ctx.bot,
            ctx.loop_ctx.chat_id,
            DefaultAgentView::operation_cancelled(),
            ctx.loop_ctx.outbound_thread,
        )
        .await
    }
}

async fn handle_detach_flow_callback(ctx: &AgentCallbackContext) -> Result<()> {
    if is_agent_task_running(ctx.loop_ctx.session_keys.primary).await {
        ctx.loop_ctx
            .bot
            .answer_callback_query(ctx.callback_id.clone())
            .text("Task is running")
            .await?;
        return Ok(());
    }

    if !should_create_fresh_flow_on_detach(
        &ctx.storage,
        ctx.loop_ctx.user_id,
        &ctx.loop_ctx.context_key,
        &ctx.loop_ctx.agent_flow_id,
    )
    .await
    {
        ctx.loop_ctx
            .bot
            .answer_callback_query(ctx.callback_id.clone())
            .text("Already using empty session")
            .await?;
        return Ok(());
    }

    save_memory_after_task(
        ctx.loop_ctx.session_keys.primary,
        ctx.loop_ctx.user_id,
        &ctx.loop_ctx.context_key,
        &ctx.loop_ctx.agent_flow_id,
        &ctx.storage,
    )
    .await;
    let _ = SESSION_REGISTRY
        .remove_if_idle(&ctx.loop_ctx.session_keys.primary)
        .await;
    let new_flow_id = reset_current_agent_flow_id(
        &ctx.storage,
        ctx.loop_ctx.user_id,
        ctx.loop_ctx.chat_id,
        ctx.loop_ctx.thread_spec,
    )
    .await?;

    ctx.loop_ctx
        .bot
        .answer_callback_query(ctx.callback_id.clone())
        .text(format!("Detached: {}", short_flow_id(&new_flow_id)))
        .await?;
    Ok(())
}

async fn handle_attach_flow_callback(
    ctx: &AgentCallbackContext,
    selected_flow_id: String,
) -> Result<()> {
    if !is_valid_agent_flow_id(&selected_flow_id) {
        ctx.loop_ctx
            .bot
            .answer_callback_query(ctx.callback_id.clone())
            .text("Invalid flow ID")
            .await?;
        return Ok(());
    }

    if selected_flow_id == ctx.loop_ctx.agent_flow_id {
        ctx.loop_ctx
            .bot
            .answer_callback_query(ctx.callback_id.clone())
            .text("Already attached")
            .await?;
        return Ok(());
    }

    if is_agent_task_running(ctx.loop_ctx.session_keys.primary).await {
        ctx.loop_ctx
            .bot
            .answer_callback_query(ctx.callback_id.clone())
            .text("Task is running")
            .await?;
        return Ok(());
    }

    save_memory_after_task(
        ctx.loop_ctx.session_keys.primary,
        ctx.loop_ctx.user_id,
        &ctx.loop_ctx.context_key,
        &ctx.loop_ctx.agent_flow_id,
        &ctx.storage,
    )
    .await;
    let _ = SESSION_REGISTRY
        .remove_if_idle(&ctx.loop_ctx.session_keys.primary)
        .await;
    cleanup_abandoned_empty_flow(
        &ctx.storage,
        ctx.loop_ctx.user_id,
        &ctx.loop_ctx.context_key,
        &ctx.loop_ctx.agent_flow_id,
    )
    .await;
    set_current_agent_flow_id(
        &ctx.storage,
        ctx.loop_ctx.user_id,
        ctx.loop_ctx.chat_id,
        ctx.loop_ctx.thread_spec,
        selected_flow_id.clone(),
    )
    .await?;

    ctx.loop_ctx
        .bot
        .answer_callback_query(ctx.callback_id.clone())
        .text(format!("Attached: {}", short_flow_id(&selected_flow_id)))
        .await?;
    Ok(())
}

async fn answer_agent_callback(
    bot: &Bot,
    callback_id: teloxide::types::CallbackQueryId,
    text: Option<&str>,
) {
    let mut req = bot.answer_callback_query(callback_id);
    if let Some(text) = text {
        req = req.text(text);
    }
    let _ = req.await;
}

async fn dispatch_agent_callback(
    action: AgentCallbackAction,
    ctx: AgentCallbackContext,
) -> Result<()> {
    match action {
        AgentCallbackAction::Attach(selected_flow_id) => {
            handle_attach_flow_callback(&ctx, selected_flow_id).await
        }
        AgentCallbackAction::Detach => handle_detach_flow_callback(&ctx).await,
        AgentCallbackAction::ApproveSsh(request_id) => {
            answer_agent_callback(
                &ctx.loop_ctx.bot,
                ctx.callback_id.clone(),
                Some("SSH action approved"),
            )
            .await;
            handle_ssh_approval_callback(&ctx, request_id).await
        }
        AgentCallbackAction::RejectSsh(request_id) => {
            answer_agent_callback(
                &ctx.loop_ctx.bot,
                ctx.callback_id.clone(),
                Some("SSH action rejected"),
            )
            .await;
            handle_ssh_reject_callback(&ctx, request_id).await
        }
        AgentCallbackAction::LoopRetry => {
            answer_agent_callback(&ctx.loop_ctx.bot, ctx.callback_id.clone(), None).await;
            handle_loop_retry(
                &ctx.loop_ctx,
                ctx.storage.clone(),
                ctx.llm.clone(),
                ctx.settings,
            )
            .await
        }
        AgentCallbackAction::LoopReset => {
            answer_agent_callback(&ctx.loop_ctx.bot, ctx.callback_id.clone(), None).await;
            handle_loop_reset(&ctx.loop_ctx).await
        }
        AgentCallbackAction::LoopCancel => {
            answer_agent_callback(&ctx.loop_ctx.bot, ctx.callback_id.clone(), None).await;
            cancel_agent_task_by_id(
                ctx.loop_ctx.bot.clone(),
                ctx.loop_ctx.session_keys,
                ctx.loop_ctx.chat_id,
                ctx.loop_ctx.thread_spec,
                ctx.loop_ctx.outbound_thread.message_thread_id,
                &ctx.loop_ctx.agent_flow_id,
            )
            .await
        }
        AgentCallbackAction::StartCancelTaskConfirmation => {
            answer_agent_callback(&ctx.loop_ctx.bot, ctx.callback_id.clone(), None).await;
            send_cancel_task_confirmation(&ctx.loop_ctx).await
        }
        AgentCallbackAction::ResolveCancelTaskConfirmation(confirmed) => {
            answer_agent_callback(&ctx.loop_ctx.bot, ctx.callback_id.clone(), None).await;
            handle_cancel_task_confirmation_callback(&ctx, confirmed).await
        }
        AgentCallbackAction::StartConfirmation(action) => {
            answer_agent_callback(&ctx.loop_ctx.bot, ctx.callback_id.clone(), None).await;
            confirm_destructive_action(action, ctx.loop_ctx.bot.clone(), ctx.msg, ctx.dialogue)
                .await
        }
        AgentCallbackAction::Exit => {
            answer_agent_callback(&ctx.loop_ctx.bot, ctx.callback_id.clone(), None).await;
            exit_agent_mode(ctx.loop_ctx.bot.clone(), ctx.msg, ctx.dialogue, ctx.storage).await
        }
        AgentCallbackAction::ResolveConfirmation(action, confirmed) => {
            answer_agent_callback(&ctx.loop_ctx.bot, ctx.callback_id.clone(), None).await;
            handle_agent_confirmation_callback(&ctx, action, confirmed).await
        }
    }
}

/// Handle agent inline keyboard callbacks.
///
/// # Errors
///
/// Returns an error if Telegram API calls fail.
pub async fn handle_agent_callback(
    bot: Bot,
    q: CallbackQuery,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    settings: Arc<BotSettings>,
    dialogue: AgentDialogue,
) -> Result<()> {
    let Some(data) = q.data.as_deref() else {
        return Ok(());
    };

    let Some(action) = parse_agent_callback_action(data) else {
        return Ok(());
    };

    let msg = q
        .message
        .as_ref()
        .and_then(|message| message.regular_message())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Callback message missing regular message context"))?;
    let user_id = q.from.id.0.cast_signed();
    let chat_id = msg.chat.id;
    let thread_spec = resolve_thread_spec(&msg);
    let (agent_flow_id, _) =
        ensure_current_agent_flow_id(&storage, user_id, chat_id, thread_spec).await?;
    let session_keys =
        agent_mode_session_keys(user_id, chat_id, thread_spec.thread_id, &agent_flow_id);
    let ctx = AgentCallbackContext {
        callback_id: q.id.clone(),
        loop_ctx: LoopCallbackContext {
            bot,
            chat_id,
            context_key: storage_context_key(chat_id, thread_spec),
            agent_flow_id,
            user_id,
            session_keys,
            manager_default_chat_id: manager_default_chat_id(chat_id, thread_spec),
            thread_spec,
            outbound_thread: outbound_thread_from_callback(&q),
        },
        msg,
        dialogue,
        storage,
        llm,
        settings,
    };

    dispatch_agent_callback(action, ctx).await
}

/// Cancel the current agent task
///
/// # Errors
///
/// Returns an error if the cancellation message cannot be sent.
pub async fn cancel_agent_task(
    bot: Bot,
    msg: Message,
    _dialogue: AgentDialogue,
    storage: Arc<dyn StorageProvider>,
) -> Result<()> {
    let thread_spec = resolve_thread_spec(&msg);
    let outbound_thread = build_outbound_thread_params(thread_spec);
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let (agent_flow_id, _) =
        ensure_current_agent_flow_id(&storage, user_id, msg.chat.id, thread_spec).await?;
    let session_keys =
        agent_mode_session_keys(user_id, msg.chat.id, thread_spec.thread_id, &agent_flow_id);
    let session_id = resolve_existing_session_id(session_keys)
        .await
        .unwrap_or(session_keys.primary);
    let reply_markup = automatic_agent_control_markup(thread_spec);

    let (cancelled, cleared_todos) = cancel_and_clear_with_compat(session_keys).await;

    if !cancelled && !cleared_todos {
        clear_pending_cancel_message(session_id).await;
        clear_cancel_confirmation_message(&bot, session_id, msg.chat.id).await;
        send_agent_message_with_optional_keyboard(
            &bot,
            msg.chat.id,
            DefaultAgentView::no_active_task(),
            reply_markup.as_ref(),
            outbound_thread,
        )
        .await?;
    } else {
        send_or_update_pending_cancel_message(
            &bot,
            session_id,
            msg.chat.id,
            DefaultAgentView::task_cancelling(cleared_todos),
            cancel_status_reply_markup(thread_spec, &agent_flow_id),
            cancel_status_inline_markup(use_inline_topic_controls(thread_spec), &agent_flow_id),
            outbound_thread,
        )
        .await?;
    }
    Ok(())
}

async fn cancel_agent_task_by_id(
    bot: Bot,
    session_keys: AgentModeSessionKeys,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
    message_thread_id: Option<ThreadId>,
    agent_flow_id: &str,
) -> Result<()> {
    let session_id = resolve_existing_session_id(session_keys)
        .await
        .unwrap_or(session_keys.primary);
    let (cancelled, cleared_todos) = cancel_and_clear_with_compat(session_keys).await;
    let outbound_thread = OutboundThreadParams { message_thread_id };
    let reply_markup = automatic_agent_control_markup(thread_spec);

    if !cancelled && !cleared_todos {
        clear_pending_cancel_message(session_id).await;
        clear_cancel_confirmation_message(&bot, session_id, chat_id).await;
        send_agent_message_with_optional_keyboard(
            &bot,
            chat_id,
            DefaultAgentView::no_active_task(),
            reply_markup.as_ref(),
            outbound_thread,
        )
        .await?;
    } else {
        send_or_update_pending_cancel_message(
            &bot,
            session_id,
            chat_id,
            DefaultAgentView::task_cancelling(cleared_todos),
            cancel_status_reply_markup(thread_spec, agent_flow_id),
            cancel_status_inline_markup(use_inline_topic_controls(thread_spec), agent_flow_id),
            outbound_thread,
        )
        .await?;
    }

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
    storage: Arc<dyn StorageProvider>,
) -> Result<()> {
    let thread_spec = resolve_thread_spec(&msg);
    let outbound_thread = build_outbound_thread_params(thread_spec);
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let context_key = storage_context_key(msg.chat.id, thread_spec);
    let (agent_flow_id, _) =
        ensure_current_agent_flow_id(&storage, user_id, msg.chat.id, thread_spec).await?;
    let session_keys =
        agent_mode_session_keys(user_id, msg.chat.id, thread_spec.thread_id, &agent_flow_id);

    let session_id = resolve_existing_session_id(session_keys)
        .await
        .unwrap_or(session_keys.primary);
    save_memory_after_task(session_id, user_id, &context_key, &agent_flow_id, &storage).await;
    remove_sessions_with_compat(session_keys).await;

    let _ = set_current_context_state(
        &storage,
        user_id,
        msg.chat.id,
        thread_spec,
        Some("chat_mode"),
    )
    .await;
    dialogue.update(State::Start).await?;

    let mut req = bot.send_message(msg.chat.id, "👋 Exited agent mode. Select a working mode:");
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.reply_markup(crate::bot::handlers::main_menu_markup(thread_spec))
        .await?;
    Ok(())
}

/// Ask for confirmation for destructive action (clear memory or recreate container)
///
/// # Errors
///
/// Returns an error if the confirmation message cannot be sent.
pub async fn confirm_destructive_action(
    action: ConfirmationType,
    bot: Bot,
    msg: Message,
    dialogue: AgentDialogue,
) -> Result<()> {
    let thread_spec = resolve_thread_spec(&msg);
    let outbound_thread = build_outbound_thread_params(thread_spec);
    dialogue
        .update(State::AgentConfirmation(action.clone()))
        .await?;

    let message_text = match action {
        ConfirmationType::ClearMemory => DefaultAgentView::memory_clear_confirmation(),
        ConfirmationType::RecreateContainer => DefaultAgentView::container_wipe_confirmation(),
    };

    let mut req = bot
        .send_message(msg.chat.id, message_text)
        .parse_mode(ParseMode::Html);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.reply_markup(confirmation_markup(
        use_inline_topic_controls(thread_spec),
        action,
    ))
    .await?;
    Ok(())
}

/// Handle confirmation for destructive agent actions
///
/// # Errors
///
/// Returns an error if the action cannot be performed or message cannot be sent.
pub async fn handle_agent_confirmation(
    bot: Bot,
    msg: Message,
    dialogue: AgentDialogue,
    action: ConfirmationType,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    settings: Arc<BotSettings>,
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let thread_spec = resolve_thread_spec(&msg);
    let (agent_flow_id, _) =
        ensure_current_agent_flow_id(&storage, user_id, msg.chat.id, thread_spec).await?;
    let session_keys =
        agent_mode_session_keys(user_id, msg.chat.id, thread_spec.thread_id, &agent_flow_id);
    let text = msg.text().unwrap_or("");
    let chat_id = msg.chat.id;
    let outbound_thread = build_outbound_thread_params(thread_spec);

    if text != "✅ Yes" && text != "❌ Cancel" {
        send_agent_message(
            &bot,
            chat_id,
            DefaultAgentView::select_keyboard_option(),
            outbound_thread,
        )
        .await?;
        return Ok(());
    }

    dialogue.update(State::AgentMode).await?;
    let reply_markup = automatic_agent_control_markup(thread_spec);
    let context_key = storage_context_key(chat_id, thread_spec);
    let send_ctx = ConfirmationSendCtx {
        bot: &bot,
        chat_id,
        context_key: &context_key,
        agent_flow_id: &agent_flow_id,
        reply_markup,
        manager_default_chat_id: manager_default_chat_id(chat_id, thread_spec),
        outbound_thread,
    };

    match text {
        "✅ Yes" => match action {
            ConfirmationType::ClearMemory => {
                handle_clear_memory_confirmation(
                    user_id,
                    session_keys,
                    &storage,
                    thread_spec,
                    &send_ctx,
                )
                .await?;
            }
            ConfirmationType::RecreateContainer => {
                handle_recreate_container_confirmation(
                    user_id,
                    session_keys,
                    &storage,
                    &llm,
                    &settings,
                    &send_ctx,
                )
                .await?;
            }
        },
        "❌ Cancel" => {
            info!(user_id = user_id, action = ?action, "User cancelled destructive action");
            send_agent_message_with_optional_keyboard(
                &bot,
                chat_id,
                DefaultAgentView::operation_cancelled(),
                send_ctx.reply_markup.as_ref(),
                outbound_thread,
            )
            .await?;
        }
        _ => unreachable!(),
    }

    Ok(())
}
