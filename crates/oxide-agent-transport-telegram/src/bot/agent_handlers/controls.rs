use super::{
    ensure_session_exists, remove_sessions_with_compat, save_memory_after_task, AgentDialogue,
    AgentModeSessionKeys, EnsureSessionContext, SessionTransportContext, SESSION_REGISTRY,
};
use crate::bot::context::{ensure_current_agent_flow_id, reset_current_agent_flow_id};
use crate::bot::resilient;
use crate::bot::state::ConfirmationType;
use crate::bot::views::{
    agent_control_markup, agent_flow_inline_keyboard, cancel_task_confirmation_inline_keyboard,
    empty_inline_keyboard, get_agent_inline_keyboard_with_exit, AgentView, DefaultAgentView,
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
use oxide_agent_core::storage::StorageProvider;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, InlineKeyboardMarkup, MessageId, ReplyMarkup};
use tracing::{info, warn};

const TASK_CANCELLED_BY_USER: &str = "Task cancelled by user";

enum AgentWipeError {
    Recreate(Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentControlCommand {
    CancelTask,
    ClearMemory,
    RecreateContainer,
    ExitAgentMode,
    ShowControls,
}

pub(crate) struct ConfirmationSendCtx<'a> {
    pub(crate) bot: &'a Bot,
    pub(crate) chat_id: ChatId,
    pub(crate) context_key: &'a str,
    pub(crate) agent_flow_id: &'a str,
    pub(crate) reply_markup: Option<ReplyMarkup>,
    pub(crate) manager_default_chat_id: Option<ChatId>,
    pub(crate) outbound_thread: OutboundThreadParams,
}

pub(crate) fn parse_agent_control_command(text: Option<&str>) -> Option<AgentControlCommand> {
    match text {
        Some("❌ Cancel Task") => Some(AgentControlCommand::CancelTask),
        Some("🗑 Clear Memory") => Some(AgentControlCommand::ClearMemory),
        Some("🔄 Recreate Container") => Some(AgentControlCommand::RecreateContainer),
        Some("⬅️ Exit Agent Mode") => Some(AgentControlCommand::ExitAgentMode),
        Some("/c") => Some(AgentControlCommand::ShowControls),
        _ => None,
    }
}

pub(crate) fn outbound_thread_from_message(msg: &Message) -> OutboundThreadParams {
    build_outbound_thread_params(resolve_thread_spec(msg))
}

pub(crate) fn outbound_thread_from_callback(q: &CallbackQuery) -> OutboundThreadParams {
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

pub(crate) async fn send_agent_message(
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

pub(crate) async fn send_agent_message_and_return(
    bot: &Bot,
    chat_id: ChatId,
    text: impl Into<String>,
    reply_markup: Option<ReplyMarkup>,
    outbound_thread: OutboundThreadParams,
) -> Result<Message> {
    resilient::send_message_resilient_with_thread_and_markup(
        bot,
        chat_id,
        text,
        None,
        outbound_thread.message_thread_id,
        reply_markup,
    )
    .await
}

pub(crate) async fn send_agent_message_with_keyboard(
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

pub(crate) async fn send_agent_message_with_optional_keyboard(
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

pub(crate) fn use_inline_topic_controls(thread_spec: TelegramThreadSpec) -> bool {
    matches!(thread_spec.kind, TelegramThreadKind::Forum)
}

pub(crate) fn use_inline_flow_controls(thread_spec: TelegramThreadSpec) -> bool {
    matches!(
        thread_spec.kind,
        TelegramThreadKind::Forum | TelegramThreadKind::Dm
    )
}

pub(crate) fn automatic_agent_control_markup(
    thread_spec: TelegramThreadSpec,
) -> Option<ReplyMarkup> {
    (!use_inline_topic_controls(thread_spec)).then(|| agent_control_markup(false))
}

pub(crate) fn cancel_status_reply_markup(
    thread_spec: TelegramThreadSpec,
    agent_flow_id: &str,
) -> ReplyMarkup {
    if use_inline_flow_controls(thread_spec) {
        agent_flow_inline_keyboard(agent_flow_id).into()
    } else {
        agent_control_markup(false)
    }
}

pub(crate) fn cancel_status_inline_markup(
    use_inline_flow_controls: bool,
    agent_flow_id: &str,
) -> Option<InlineKeyboardMarkup> {
    use_inline_flow_controls.then(|| agent_flow_inline_keyboard(agent_flow_id))
}

pub(crate) async fn send_agent_flow_controls_message(
    bot: &Bot,
    chat_id: ChatId,
    agent_flow_id: &str,
    outbound_thread: OutboundThreadParams,
) -> Result<()> {
    let reply_markup: ReplyMarkup = agent_flow_inline_keyboard(agent_flow_id).into();
    send_agent_message_with_keyboard(
        bot,
        chat_id,
        "Flow controls:",
        &reply_markup,
        outbound_thread,
    )
    .await
}

pub(crate) fn is_task_cancelled_error(error: &anyhow::Error) -> bool {
    error.to_string() == TASK_CANCELLED_BY_USER
}

pub(crate) async fn pending_cancel_message(session_id: SessionId) -> Option<MessageId> {
    let pending = super::PENDING_CANCEL_MESSAGES.read().await;
    pending.get(&session_id).copied()
}

pub(crate) async fn pending_cancel_confirmation(session_id: SessionId) -> Option<MessageId> {
    let pending = super::PENDING_CANCEL_CONFIRMATIONS.read().await;
    pending.get(&session_id).copied()
}

pub(crate) async fn remember_pending_cancel_message(session_id: SessionId, message_id: MessageId) {
    let mut pending = super::PENDING_CANCEL_MESSAGES.write().await;
    pending.insert(session_id, message_id);
}

pub(crate) async fn clear_pending_cancel_message(session_id: SessionId) {
    let mut pending = super::PENDING_CANCEL_MESSAGES.write().await;
    pending.remove(&session_id);
}

pub(crate) async fn remember_pending_cancel_confirmation(
    session_id: SessionId,
    message_id: MessageId,
) {
    let mut pending = super::PENDING_CANCEL_CONFIRMATIONS.write().await;
    pending.insert(session_id, message_id);
}

pub(crate) async fn clear_pending_cancel_confirmation(session_id: SessionId) {
    let mut pending = super::PENDING_CANCEL_CONFIRMATIONS.write().await;
    pending.remove(&session_id);
}

pub(crate) async fn take_pending_cancel_message(session_id: SessionId) -> Option<MessageId> {
    let mut pending = super::PENDING_CANCEL_MESSAGES.write().await;
    pending.remove(&session_id)
}

pub(crate) async fn take_pending_cancel_confirmation(session_id: SessionId) -> Option<MessageId> {
    let mut pending = super::PENDING_CANCEL_CONFIRMATIONS.write().await;
    pending.remove(&session_id)
}

pub(crate) async fn send_or_update_pending_cancel_message(
    bot: &Bot,
    session_id: SessionId,
    chat_id: ChatId,
    text: &str,
    reply_markup: ReplyMarkup,
    inline_reply_markup: Option<InlineKeyboardMarkup>,
    outbound_thread: OutboundThreadParams,
) -> Result<()> {
    if let Some(message_id) = pending_cancel_message(session_id).await {
        if resilient::edit_message_safe_resilient_with_markup(
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

pub(crate) async fn finalize_pending_cancel_message(
    bot: &Bot,
    session_id: SessionId,
    chat_id: ChatId,
    text: &str,
    inline_reply_markup: Option<InlineKeyboardMarkup>,
) {
    let Some(message_id) = take_pending_cancel_message(session_id).await else {
        return;
    };

    let _ = resilient::edit_message_safe_resilient_with_markup(
        bot,
        chat_id,
        message_id,
        text,
        inline_reply_markup,
    )
    .await;
}

pub(crate) async fn send_or_update_cancel_confirmation(
    bot: &Bot,
    session_id: SessionId,
    chat_id: ChatId,
    outbound_thread: OutboundThreadParams,
) -> Result<()> {
    let inline_reply_markup = cancel_task_confirmation_inline_keyboard();

    if let Some(message_id) = pending_cancel_confirmation(session_id).await {
        if resilient::edit_message_safe_resilient_with_markup(
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

pub(crate) async fn clear_cancel_confirmation_message(
    bot: &Bot,
    session_id: SessionId,
    chat_id: ChatId,
) {
    let Some(message_id) = take_pending_cancel_confirmation(session_id).await else {
        return;
    };

    let _ = resilient::edit_message_safe_resilient_with_markup(
        bot,
        chat_id,
        message_id,
        DefaultAgentView::task_cancel_confirmation(),
        Some(empty_inline_keyboard()),
    )
    .await;
}

pub(crate) async fn finalize_cancel_status_if_needed(
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

pub(crate) async fn show_agent_controls(
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
    .await?;

    if matches!(thread_spec.kind, TelegramThreadKind::Dm) {
        let (agent_flow_id, _) =
            ensure_current_agent_flow_id(&storage, user_id, msg.chat.id, thread_spec).await?;
        send_agent_flow_controls_message(&bot, msg.chat.id, &agent_flow_id, outbound_thread)
            .await?;
    }

    Ok(())
}

pub(crate) async fn handle_clear_memory_confirmation(
    user_id: i64,
    session_keys: AgentModeSessionKeys,
    storage: &Arc<dyn StorageProvider>,
    thread_spec: TelegramThreadSpec,
    send_ctx: &ConfirmationSendCtx<'_>,
) -> Result<()> {
    info!(user_id = user_id, "User confirmed memory clear");
    if super::is_agent_task_running(session_keys.primary).await {
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

pub(crate) async fn handle_recreate_container_confirmation(
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

pub(crate) async fn exit_agent_mode(
    bot: Bot,
    msg: Message,
    dialogue: AgentDialogue,
    storage: Arc<dyn StorageProvider>,
) -> Result<()> {
    let thread_spec = resolve_thread_spec(&msg);
    let outbound_thread = build_outbound_thread_params(thread_spec);
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let context_key = crate::bot::context::storage_context_key(msg.chat.id, thread_spec);
    let (agent_flow_id, _) =
        ensure_current_agent_flow_id(&storage, user_id, msg.chat.id, thread_spec).await?;
    let session_keys =
        super::agent_mode_session_keys(user_id, msg.chat.id, thread_spec.thread_id, &agent_flow_id);

    let session_id = super::resolve_existing_session_id(session_keys)
        .await
        .unwrap_or(session_keys.primary);
    save_memory_after_task(session_id, user_id, &context_key, &agent_flow_id, &storage).await;
    remove_sessions_with_compat(session_keys).await;

    let _ = crate::bot::context::set_current_context_state(
        &storage,
        user_id,
        msg.chat.id,
        thread_spec,
        Some("chat_mode"),
    )
    .await;
    dialogue.update(crate::bot::state::State::Start).await?;

    let mut req = bot.send_message(msg.chat.id, "👋 Exited agent mode. Select a working mode:");
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.reply_markup(crate::bot::handlers::main_menu_markup(thread_spec))
        .await?;
    Ok(())
}

pub(crate) async fn confirm_destructive_action(
    action: ConfirmationType,
    bot: Bot,
    msg: Message,
    dialogue: AgentDialogue,
) -> Result<()> {
    let thread_spec = resolve_thread_spec(&msg);
    let outbound_thread = build_outbound_thread_params(thread_spec);
    dialogue
        .update(crate::bot::state::State::AgentConfirmation(action.clone()))
        .await?;

    let message_text = match action {
        ConfirmationType::ClearMemory => DefaultAgentView::memory_clear_confirmation(),
        ConfirmationType::RecreateContainer => DefaultAgentView::container_wipe_confirmation(),
    };

    let mut req = bot
        .send_message(msg.chat.id, message_text)
        .parse_mode(teloxide::types::ParseMode::Html);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.reply_markup(crate::bot::views::confirmation_markup(
        use_inline_topic_controls(thread_spec),
        action,
    ))
    .await?;
    Ok(())
}

pub(crate) async fn handle_agent_confirmation(
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
        super::agent_mode_session_keys(user_id, msg.chat.id, thread_spec.thread_id, &agent_flow_id);
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

    dialogue.update(crate::bot::state::State::AgentMode).await?;
    let reply_markup = automatic_agent_control_markup(thread_spec);
    let context_key = crate::bot::context::storage_context_key(chat_id, thread_spec);
    let send_ctx = ConfirmationSendCtx {
        bot: &bot,
        chat_id,
        context_key: &context_key,
        agent_flow_id: &agent_flow_id,
        reply_markup,
        manager_default_chat_id: super::manager_default_chat_id(chat_id, thread_spec),
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
