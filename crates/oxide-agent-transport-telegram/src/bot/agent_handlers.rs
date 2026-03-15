//! Agent mode handlers for Telegram bot
//!
//! Provides handlers for activating agent mode, processing messages,
//! and managing agent sessions.

use crate::bot::agent::extract_agent_input;
use crate::bot::agent_transport::TelegramAgentTransport;
use crate::bot::context::{
    current_context_state, ensure_current_agent_flow_id, reset_current_agent_flow_id,
    sandbox_scope, set_current_agent_flow_id, set_current_context_state, storage_context_key,
};
use crate::bot::manager_topic_lifecycle::TelegramManagerTopicLifecycle;
use crate::bot::messaging::send_long_message_in_thread_with_final_markup;
use crate::bot::progress_render::render_progress_html;
use crate::bot::state::{ConfirmationType, State};
use crate::bot::topic_route::{
    resolve_topic_route, touch_dynamic_binding_activity_if_needed, TopicRouteDecision,
};
use crate::bot::views::{
    agent_control_markup, agent_flow_inline_keyboard, cancel_task_confirmation_inline_keyboard,
    confirmation_markup, empty_inline_keyboard, get_agent_inline_keyboard_with_exit,
    progress_inline_keyboard, ssh_approval_inline_keyboard, AgentView, DefaultAgentView,
    AGENT_CALLBACK_ATTACH_PREFIX, AGENT_CALLBACK_CANCEL_TASK, AGENT_CALLBACK_CLEAR_MEMORY,
    AGENT_CALLBACK_CONFIRM_CANCEL_NO, AGENT_CALLBACK_CONFIRM_CANCEL_YES,
    AGENT_CALLBACK_CONFIRM_CLEAR_CANCEL, AGENT_CALLBACK_CONFIRM_CLEAR_YES,
    AGENT_CALLBACK_CONFIRM_RECREATE_CANCEL, AGENT_CALLBACK_CONFIRM_RECREATE_YES,
    AGENT_CALLBACK_DETACH, AGENT_CALLBACK_EXIT, AGENT_CALLBACK_RECREATE_CONTAINER,
    AGENT_CALLBACK_SSH_APPROVE_PREFIX, AGENT_CALLBACK_SSH_REJECT_PREFIX, LOOP_CALLBACK_CANCEL,
    LOOP_CALLBACK_RESET, LOOP_CALLBACK_RETRY,
};
use crate::bot::{
    build_outbound_thread_params, general_forum_topic_id, resolve_thread_spec,
    OutboundThreadParams, TelegramThreadKind, TelegramThreadSpec,
};
use crate::config::BotSettings;
use anyhow::{Error, Result};
use oxide_agent_core::agent::{
    executor::AgentExecutor,
    parse_agent_profile,
    preprocessor::Preprocessor,
    progress::{AgentEvent, ProgressState},
    providers::{
        inject_ssh_approval_system_message, inject_topic_infra_preflight_system_message,
        inspect_topic_infra_config,
    },
    AgentExecutionProfile, AgentSession, SessionId,
};
use oxide_agent_core::config::AGENT_MAX_ITERATIONS;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::sandbox::SandboxScope;
use oxide_agent_core::storage::StorageProvider;
use oxide_agent_runtime::SessionRegistry;
use oxide_agent_runtime::{spawn_progress_runtime, ProgressRuntimeConfig};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::prelude::*;
use teloxide::types::{
    CallbackQuery, InlineKeyboardMarkup, MessageId, ParseMode, ReplyMarkup, ThreadId,
};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Type alias for dialogue
pub type AgentDialogue = Dialogue<State, InMemStorage<State>>;

/// Context for running an agent task without blocking the update handler
struct AgentTaskContext {
    bot: Bot,
    msg: Message,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    context_key: String,
    agent_flow_id: String,
    sandbox_scope: SandboxScope,
    message_thread_id: Option<ThreadId>,
    use_inline_progress_controls: bool,
    session_id: SessionId,
}

#[derive(Clone, Copy, Debug)]
struct AgentModeSessionKeys {
    primary: SessionId,
    legacy: SessionId,
}

#[derive(Clone, Copy)]
struct SessionTransportContext {
    manager_default_chat_id: Option<ChatId>,
    thread_spec: TelegramThreadSpec,
}

struct EnsureSessionContext<'a> {
    session_keys: AgentModeSessionKeys,
    context_key: String,
    agent_flow_id: String,
    agent_flow_created: bool,
    sandbox_scope: SandboxScope,
    user_id: i64,
    bot: &'a Bot,
    transport_ctx: SessionTransportContext,
    llm: &'a Arc<LlmClient>,
    storage: &'a Arc<dyn StorageProvider>,
    settings: &'a Arc<BotSettings>,
}

impl AgentModeSessionKeys {
    fn distinct_legacy(self) -> Option<SessionId> {
        if self.primary == self.legacy {
            None
        } else {
            Some(self.legacy)
        }
    }
}

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

/// Global session registry for agent executors
static SESSION_REGISTRY: LazyLock<SessionRegistry> = LazyLock::new(SessionRegistry::new);
static PENDING_CANCEL_MESSAGES: LazyLock<RwLock<HashMap<SessionId, MessageId>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

const TASK_CANCELLED_BY_USER: &str = "Task cancelled by user";

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn fnv1a_mix_i64(mut hash: u64, value: i64) -> u64 {
    for byte in value.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    hash
}

fn fnv1a_mix_str(mut hash: u64, value: &str) -> u64 {
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    hash
}

fn derive_agent_mode_session_id(
    user_id: i64,
    chat_id: ChatId,
    thread_id: Option<ThreadId>,
    agent_flow_id: &str,
) -> SessionId {
    let mut hash = FNV_OFFSET_BASIS;
    hash = fnv1a_mix_i64(hash, user_id);
    hash = fnv1a_mix_i64(hash, chat_id.0);
    hash = fnv1a_mix_i64(
        hash,
        thread_id.map_or(0, |thread_id| i64::from(thread_id.0 .0)),
    );
    hash = fnv1a_mix_str(hash, agent_flow_id);

    let folded = hash & (i64::MAX as u64);
    let derived = if folded == 0 { -1 } else { -(folded as i64) };
    SessionId::from(derived)
}

fn agent_mode_session_keys(
    user_id: i64,
    chat_id: ChatId,
    thread_id: Option<ThreadId>,
    agent_flow_id: &str,
) -> AgentModeSessionKeys {
    AgentModeSessionKeys {
        primary: derive_agent_mode_session_id(user_id, chat_id, thread_id, agent_flow_id),
        legacy: SessionId::from(user_id),
    }
}

fn select_existing_session_id(
    keys: AgentModeSessionKeys,
    primary_exists: bool,
    legacy_exists: bool,
) -> Option<SessionId> {
    if primary_exists {
        Some(keys.primary)
    } else if legacy_exists {
        Some(keys.legacy)
    } else {
        None
    }
}

async fn resolve_existing_session_id(keys: AgentModeSessionKeys) -> Option<SessionId> {
    let primary_exists = SESSION_REGISTRY.contains(&keys.primary).await;
    let legacy_exists = if let Some(legacy) = keys.distinct_legacy() {
        SESSION_REGISTRY.contains(&legacy).await
    } else {
        primary_exists
    };

    select_existing_session_id(keys, primary_exists, legacy_exists)
}

async fn session_manager_control_plane_enabled(session_id: SessionId) -> Option<bool> {
    let executor_arc = SESSION_REGISTRY.get(&session_id).await?;
    let executor = executor_arc.read().await;
    Some(executor.manager_control_plane_enabled())
}

enum ResetSessionOutcome {
    Reset,
    Busy,
    NotFound,
}

async fn reset_sessions_with_compat(keys: AgentModeSessionKeys) -> ResetSessionOutcome {
    let primary_result = SESSION_REGISTRY.reset(&keys.primary).await;
    let legacy_result = if let Some(legacy) = keys.distinct_legacy() {
        Some(SESSION_REGISTRY.reset(&legacy).await)
    } else {
        None
    };

    let primary_reset = matches!(primary_result, Ok(()));
    let legacy_reset = matches!(legacy_result, Some(Ok(())));
    if primary_reset || legacy_reset {
        return ResetSessionOutcome::Reset;
    }

    let primary_busy = matches!(primary_result, Err("Cannot reset while task is running"));
    let legacy_busy = matches!(
        legacy_result,
        Some(Err("Cannot reset while task is running"))
    );
    if primary_busy || legacy_busy {
        return ResetSessionOutcome::Busy;
    }

    ResetSessionOutcome::NotFound
}

async fn cancel_and_clear_with_compat(keys: AgentModeSessionKeys) -> (bool, bool) {
    let cancelled_primary = SESSION_REGISTRY.cancel(&keys.primary).await;
    let cleared_primary = SESSION_REGISTRY.clear_todos(&keys.primary).await;

    if let Some(legacy) = keys.distinct_legacy() {
        let cancelled_legacy = SESSION_REGISTRY.cancel(&legacy).await;
        let cleared_legacy = SESSION_REGISTRY.clear_todos(&legacy).await;
        (
            cancelled_primary || cancelled_legacy,
            cleared_primary || cleared_legacy,
        )
    } else {
        (cancelled_primary, cleared_primary)
    }
}

async fn remove_sessions_with_compat(keys: AgentModeSessionKeys) {
    SESSION_REGISTRY.remove(&keys.primary).await;
    clear_pending_cancel_message(keys.primary).await;
    if let Some(legacy) = keys.distinct_legacy() {
        SESSION_REGISTRY.remove(&legacy).await;
        clear_pending_cancel_message(legacy).await;
    }
}

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

async fn remember_pending_cancel_message(session_id: SessionId, message_id: MessageId) {
    let mut pending = PENDING_CANCEL_MESSAGES.write().await;
    pending.insert(session_id, message_id);
}

async fn clear_pending_cancel_message(session_id: SessionId) {
    let mut pending = PENDING_CANCEL_MESSAGES.write().await;
    pending.remove(&session_id);
}

async fn take_pending_cancel_message(session_id: SessionId) -> Option<MessageId> {
    let mut pending = PENDING_CANCEL_MESSAGES.write().await;
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

async fn finalize_cancel_status_if_needed(
    bot: &Bot,
    session_id: SessionId,
    chat_id: ChatId,
    cancelled: bool,
    inline_reply_markup: Option<InlineKeyboardMarkup>,
) {
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

    let scoped_state = current_context_state(&storage, user_id, chat_id, thread_spec).await?;
    if scoped_state.as_deref() != Some("agent_mode") {
        return delegate_non_agent_context_message(bot, msg, storage, llm, dialogue, settings)
            .await;
    }

    let (agent_flow_id, agent_flow_created) =
        ensure_current_agent_flow_id(&storage, user_id, chat_id, thread_spec).await?;
    let session_keys =
        agent_mode_session_keys(user_id, chat_id, thread_spec.thread_id, &agent_flow_id);

    if let Some(command) = parse_agent_control_command(msg.text()) {
        return handle_agent_control_command(command, bot, msg, dialogue, storage).await;
    }

    let route = resolve_topic_route(&bot, storage.as_ref(), user_id, &settings, &msg).await;

    if !route.allows_processing() {
        info!(
            "Skipping agent message in topic route for user {user_id}. enabled={}, require_mention={}, mention_satisfied={}",
            route.enabled, route.require_mention, route.mention_satisfied
        );
        return Ok(());
    }

    // Get or create session
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
        resolve_execution_profile(&storage, user_id, &context_key, &route).await;
    let topic_infra_config = resolve_topic_infra_config(&storage, user_id, &context_key).await;

    if is_agent_task_running(session_id).await {
        notify_running_agent_task(&bot, chat_id, thread_spec, outbound_thread).await?;
        touch_dynamic_binding_activity_if_needed(storage.as_ref(), user_id, &route).await;
        return Ok(());
    }

    apply_execution_profile(session_id, execution_profile).await;
    apply_topic_infra_config(
        session_id,
        storage.clone(),
        user_id,
        context_key.clone(),
        topic_infra_config,
    )
    .await;

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

async fn notify_running_agent_task(
    bot: &Bot,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
    outbound_thread: OutboundThreadParams,
) -> Result<()> {
    let mut req = bot.send_message(
        chat_id,
        "⏳ A task is already running. Press ❌ Cancel Task to stop it.",
    );
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

fn spawn_agent_task(ctx: AgentTaskContext) {
    tokio::spawn(async move {
        let task_bot = ctx.bot.clone();
        let task_msg = ctx.msg.clone();
        let message_thread_id = ctx.message_thread_id;

        if let Err(e) = run_agent_task(ctx).await {
            let mut req = task_bot.send_message(task_msg.chat.id, format!("❌ Error: {e}"));
            if let Some(thread_id) = message_thread_id {
                req = req.message_thread_id(thread_id);
            }

            let _ = req.await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        agent_mode_session_keys, cancel_status_reply_markup, cleanup_abandoned_empty_flow,
        clear_pending_cancel_message, derive_agent_mode_session_id, ensure_session_exists,
        manager_control_plane_enabled, manager_default_chat_id, merge_prompt_instructions,
        parse_agent_callback_action, parse_agent_control_command, pending_cancel_message,
        remember_pending_cancel_message, remove_sessions_with_compat, resolve_execution_profile,
        select_existing_session_id, session_manager_control_plane_enabled,
        should_create_fresh_flow_on_detach, take_pending_cancel_message, AgentCallbackAction,
        AgentControlCommand, EnsureSessionContext, SessionTransportContext, SESSION_REGISTRY,
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
        StorageError, StorageProvider, TopicBindingRecord, UpsertAgentProfileOptions,
        UpsertTopicBindingOptions, UserConfig,
    };
    use std::sync::{Arc, Mutex};
    use teloxide::types::{ChatId, MessageId, ReplyMarkup, ThreadId};
    use teloxide::Bot;

    #[derive(Default)]
    struct NoopStorage {
        flow_memory: Option<AgentMemory>,
        agent_profile: Option<serde_json::Value>,
        topic_context: Option<String>,
        fail_flow_memory_lookup: bool,
        cleared_flows: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl NoopStorage {
        fn with_flow_memory(flow_memory: Option<AgentMemory>) -> Self {
            Self {
                flow_memory,
                agent_profile: None,
                topic_context: None,
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
                fail_flow_memory_lookup: false,
                cleared_flows: Arc::default(),
            }
        }

        fn with_failed_flow_memory_lookup() -> Self {
            Self {
                flow_memory: None,
                agent_profile: None,
                topic_context: None,
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
                    "blockedTools": ["delegate_to_sub_agent"]
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

        let profile = resolve_execution_profile(&storage, 77, "topic-a", &route).await;

        assert_eq!(profile.agent_id(), Some("infra-agent"));
        let prompt = profile
            .prompt_instructions()
            .expect("profile prompt must be resolved");
        assert!(prompt.contains("Profile instructions:"));
        assert!(prompt.contains("Topic instructions:"));
        assert!(prompt.contains("Persistent topic context:"));
        assert!(prompt.contains("persistent topic runbook"));
        assert!(profile.tool_policy().allows("todos_write"));
        assert!(!profile.tool_policy().allows("delegate_to_sub_agent"));
        assert!(!profile.tool_policy().allows("file_write"));
    }
}

async fn ensure_session_exists(ctx: EnsureSessionContext<'_>) -> SessionId {
    let manager_enabled =
        manager_control_plane_enabled(ctx.settings, ctx.user_id, ctx.transport_ctx.thread_spec);
    let requires_primary_session = ctx.session_keys.primary != ctx.session_keys.legacy;

    if let Some(existing_session_id) = resolve_existing_session_id(ctx.session_keys).await {
        let should_migrate_legacy =
            requires_primary_session && existing_session_id != ctx.session_keys.primary;
        if let Some(existing_manager_enabled) =
            session_manager_control_plane_enabled(existing_session_id).await
        {
            if existing_manager_enabled == manager_enabled && !should_migrate_legacy {
                debug!(session_id = %existing_session_id, "Session already exists in cache");
                return existing_session_id;
            }

            let removed = SESSION_REGISTRY.remove_if_idle(&existing_session_id).await;
            if removed {
                debug!(
                    session_id = %existing_session_id,
                    previous_manager_enabled = existing_manager_enabled,
                    current_manager_enabled = manager_enabled,
                    migrate_to_primary = should_migrate_legacy,
                    "Session identity changed; recreating session"
                );
            } else if SESSION_REGISTRY.contains(&existing_session_id).await {
                debug!(
                    session_id = %existing_session_id,
                    previous_manager_enabled = existing_manager_enabled,
                    current_manager_enabled = manager_enabled,
                    migrate_to_primary = should_migrate_legacy,
                    "Session identity changed while task is running; deferring refresh"
                );
                return existing_session_id;
            }
        } else {
            let removed = SESSION_REGISTRY.remove_if_idle(&existing_session_id).await;
            if !removed && SESSION_REGISTRY.contains(&existing_session_id).await {
                debug!(
                    session_id = %existing_session_id,
                    "Session state unavailable while task is running; deferring refresh"
                );
                return existing_session_id;
            }

            debug!(
                session_id = %existing_session_id,
                "Session state unavailable; recreating session"
            );
        }
    }

    let session_id = ctx.session_keys.primary;
    if SESSION_REGISTRY.contains(&session_id).await {
        debug!(session_id = %session_id, "Session already exists in cache");
        return session_id;
    }

    let mut session = AgentSession::new_with_sandbox_scope(session_id, ctx.sandbox_scope.clone());
    load_agent_memory_into_session(&ctx, &mut session).await;

    let mut executor = AgentExecutor::new(ctx.llm.clone(), session, ctx.settings.agent.clone());
    if manager_enabled {
        let topic_lifecycle = Arc::new(TelegramManagerTopicLifecycle::new(
            ctx.bot.clone(),
            ctx.transport_ctx.manager_default_chat_id,
        ));
        executor = executor
            .with_manager_control_plane(ctx.storage.clone(), ctx.user_id)
            .with_manager_topic_lifecycle(topic_lifecycle);
    }
    SESSION_REGISTRY.insert(session_id, executor).await;
    session_id
}

async fn resolve_execution_profile(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: &str,
    route: &TopicRouteDecision,
) -> AgentExecutionProfile {
    let route_prompt = normalize_prompt_section(route.system_prompt_override.as_deref());
    let topic_context_prompt = match storage
        .get_topic_context(user_id, topic_id.to_string())
        .await
    {
        Ok(record) => {
            record.and_then(|record| normalize_prompt_section(Some(record.context.as_str())))
        }
        Err(error) => {
            warn!(
                error = %error,
                user_id,
                topic_id,
                "Failed to load topic context for executor configuration"
            );
            None
        }
    };
    let Some(agent_id) = route.agent_id.clone() else {
        return AgentExecutionProfile::new(
            None,
            compose_execution_prompt_instructions(
                None,
                route_prompt.as_deref(),
                topic_context_prompt.as_deref(),
            ),
            Default::default(),
        );
    };

    let parsed_profile = match storage.get_agent_profile(user_id, agent_id.clone()).await {
        Ok(Some(record)) => parse_agent_profile(&record.profile),
        Ok(None) => Default::default(),
        Err(error) => {
            warn!(
                error = %error,
                user_id,
                agent_id = %agent_id,
                "Failed to load agent profile for executor configuration"
            );
            Default::default()
        }
    };

    let prompt_instructions = compose_execution_prompt_instructions(
        parsed_profile.prompt_instructions.as_deref(),
        route_prompt.as_deref(),
        topic_context_prompt.as_deref(),
    );

    AgentExecutionProfile::new(
        Some(agent_id),
        prompt_instructions,
        parsed_profile.tool_policy,
    )
}

async fn resolve_topic_infra_config(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: &str,
) -> Option<oxide_agent_core::storage::TopicInfraConfigRecord> {
    match storage
        .get_topic_infra_config(user_id, topic_id.to_string())
        .await
    {
        Ok(record) => record,
        Err(error) => {
            warn!(
                error = %error,
                user_id,
                topic_id,
                "Failed to load topic infra config for executor configuration"
            );
            None
        }
    }
}

async fn apply_execution_profile(session_id: SessionId, profile: AgentExecutionProfile) {
    let Some(executor_arc) = SESSION_REGISTRY.get(&session_id).await else {
        warn!(session_id = %session_id, "Cannot apply execution profile: session not found");
        return;
    };

    let mut executor = executor_arc.write().await;
    executor.set_execution_profile(profile);
}

async fn apply_topic_infra_config(
    session_id: SessionId,
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: String,
    config: Option<oxide_agent_core::storage::TopicInfraConfigRecord>,
) {
    let preflight = match config.as_ref() {
        Some(config) => {
            Some(inspect_topic_infra_config(&storage, user_id, &topic_id, config).await)
        }
        None => None,
    };
    let provider_config = match preflight.as_ref() {
        Some(report) if report.provider_enabled => config.clone(),
        Some(_) => None,
        None => None,
    };
    let preflight_message = preflight
        .as_ref()
        .map(inject_topic_infra_preflight_system_message)
        .map(|message| message.content);

    let Some(executor_arc) = SESSION_REGISTRY.get(&session_id).await else {
        warn!(session_id = %session_id, "Cannot apply topic infra config: session not found");
        return;
    };

    let mut executor = executor_arc.write().await;
    executor.set_topic_infra(storage, user_id, topic_id, provider_config);
    executor.set_topic_infra_preflight_status(preflight.as_ref(), preflight_message);
}

#[cfg(test)]
fn merge_prompt_instructions(
    profile_prompt: Option<&str>,
    route_prompt: Option<&str>,
) -> Option<String> {
    match (
        normalize_prompt_section(profile_prompt),
        normalize_prompt_section(route_prompt),
    ) {
        (Some(profile_prompt), Some(route_prompt)) if profile_prompt == route_prompt => {
            Some(profile_prompt)
        }
        (Some(profile_prompt), Some(route_prompt)) => Some(format!(
            "Profile instructions:\n{profile_prompt}\n\nTopic instructions:\n{route_prompt}"
        )),
        (Some(profile_prompt), None) => Some(profile_prompt),
        (None, Some(route_prompt)) => Some(route_prompt),
        (None, None) => None,
    }
}

fn compose_execution_prompt_instructions(
    profile_prompt: Option<&str>,
    route_prompt: Option<&str>,
    topic_context_prompt: Option<&str>,
) -> Option<String> {
    let mut sections = Vec::new();

    if let Some(profile_prompt) = normalize_prompt_section(profile_prompt) {
        sections.push(("Profile instructions", profile_prompt));
    }
    if let Some(route_prompt) = normalize_prompt_section(route_prompt) {
        sections.push(("Topic instructions", route_prompt));
    }
    if let Some(topic_context_prompt) = normalize_prompt_section(topic_context_prompt) {
        sections.push(("Persistent topic context", topic_context_prompt));
    }

    if sections.is_empty() {
        return None;
    }

    Some(
        sections
            .into_iter()
            .map(|(label, content)| format!("{label}:\n{content}"))
            .collect::<Vec<_>>()
            .join("\n\n"),
    )
}

fn normalize_prompt_section(prompt: Option<&str>) -> Option<String> {
    prompt
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        .map(str::to_string)
}

async fn load_agent_memory_into_session(
    ctx: &EnsureSessionContext<'_>,
    session: &mut AgentSession,
) {
    if let Some(saved_memory) = load_flow_agent_memory(ctx).await {
        session.memory = saved_memory;
        info!(
            user_id = ctx.user_id,
            messages_count = session.memory.get_messages().len(),
            "Loaded agent memory for user in ensure_session_exists"
        );
        return;
    }

    if ctx.agent_flow_created {
        migrate_legacy_agent_memory_into_flow(ctx, session).await;
    } else {
        info!(
            user_id = ctx.user_id,
            "No saved agent memory found, starting fresh"
        );
    }
}

async fn load_flow_agent_memory(
    ctx: &EnsureSessionContext<'_>,
) -> Option<oxide_agent_core::agent::AgentMemory> {
    ctx.storage
        .load_agent_memory_for_flow(
            ctx.user_id,
            ctx.context_key.clone(),
            ctx.agent_flow_id.clone(),
        )
        .await
        .ok()
        .flatten()
}

async fn migrate_legacy_agent_memory_into_flow(
    ctx: &EnsureSessionContext<'_>,
    session: &mut AgentSession,
) {
    if let Ok(Some(saved_memory)) = ctx
        .storage
        .load_agent_memory_for_context(ctx.user_id, ctx.context_key.clone())
        .await
    {
        session.memory = saved_memory;
        let _ = ctx
            .storage
            .save_agent_memory_for_flow(
                ctx.user_id,
                ctx.context_key.clone(),
                ctx.agent_flow_id.clone(),
                &session.memory,
            )
            .await;
        info!(
            user_id = ctx.user_id,
            messages_count = session.memory.get_messages().len(),
            "Migrated legacy agent memory into flow-scoped storage"
        );
    } else {
        info!(
            user_id = ctx.user_id,
            "No saved agent memory found, starting fresh"
        );
    }
}

async fn is_agent_task_running(session_id: SessionId) -> bool {
    SESSION_REGISTRY.is_running(&session_id).await
}

async fn renew_cancellation_token(session_id: SessionId) {
    SESSION_REGISTRY.renew_cancellation_token(&session_id).await;
}

async fn save_memory_after_task(
    session_id: SessionId,
    user_id: i64,
    context_key: &str,
    agent_flow_id: &str,
    storage: &Arc<dyn StorageProvider>,
) {
    if let Some(executor_arc) = SESSION_REGISTRY.get(&session_id).await {
        let executor = executor_arc.read().await;
        let _ = storage
            .upsert_agent_flow_record(user_id, context_key.to_string(), agent_flow_id.to_string())
            .await;
        let _ = storage
            .save_agent_memory_for_flow(
                user_id,
                context_key.to_string(),
                agent_flow_id.to_string(),
                &executor.session().memory,
            )
            .await;
    }
}

async fn flow_has_saved_memory(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: &str,
    agent_flow_id: &str,
) -> Result<bool, oxide_agent_core::storage::StorageError> {
    storage
        .load_agent_memory_for_flow(user_id, context_key.to_string(), agent_flow_id.to_string())
        .await
        .map(|memory| memory.is_some())
}

async fn should_create_fresh_flow_on_detach(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: &str,
    agent_flow_id: &str,
) -> bool {
    match flow_has_saved_memory(storage, user_id, context_key, agent_flow_id).await {
        Ok(has_saved_memory) => has_saved_memory,
        Err(err) => {
            warn!(
                user_id,
                context_key,
                agent_flow_id,
                error = %err,
                "Failed to inspect current flow memory before detach; falling back to detach"
            );
            true
        }
    }
}

async fn cleanup_abandoned_empty_flow(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: &str,
    agent_flow_id: &str,
) {
    match flow_has_saved_memory(storage, user_id, context_key, agent_flow_id).await {
        Ok(true) => {}
        Ok(false) => {
            if let Err(err) = storage
                .clear_agent_memory_for_flow(
                    user_id,
                    context_key.to_string(),
                    agent_flow_id.to_string(),
                )
                .await
            {
                warn!(
                    user_id,
                    context_key,
                    agent_flow_id,
                    error = %err,
                    "Failed to delete abandoned empty flow after attach"
                );
            }
        }
        Err(err) => {
            warn!(
                user_id,
                context_key,
                agent_flow_id,
                error = %err,
                "Failed to inspect current flow memory before attach; leaving flow record intact"
            );
        }
    }
}

async fn run_agent_task(ctx: AgentTaskContext) -> Result<()> {
    let user_id = ctx.msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let chat_id = ctx.msg.chat.id;
    let progress_reply_markup = ctx
        .use_inline_progress_controls
        .then_some(progress_inline_keyboard());

    // Preprocess input
    let preprocessor = Preprocessor::new(ctx.llm.clone(), ctx.sandbox_scope.clone());
    let input = extract_agent_input(&ctx.bot, &ctx.msg).await?;
    let task_text = match preprocessor.preprocess_input(input).await {
        Ok(text) => text,
        Err(err) => {
            if err.to_string() == "MULTIMODAL_DISABLED" {
                super::resilient::send_message_resilient_with_thread(
                    &ctx.bot,
                    chat_id,
                    "🚫 Agent cannot process this file.\nGemini/OpenRouter connection required for vision and audio capabilities.",
                    None,
                    ctx.message_thread_id,
                )
                .await?;
                return Ok(());
            }
            return Err(err);
        }
    };
    info!(
        user_id = user_id,
        chat_id = chat_id.0,
        "Input preprocessed, task text extracted"
    );

    // Send initial progress message with retry on network failures
    let progress_msg = super::resilient::send_message_resilient_with_thread_and_markup(
        &ctx.bot,
        chat_id,
        "⏳ Processing task...",
        Some(ParseMode::Html),
        ctx.message_thread_id,
        progress_reply_markup.clone().map(Into::into),
    )
    .await?;

    // Create progress tracking channel
    let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(100);
    let transport = TelegramAgentTransport::new(
        ctx.bot.clone(),
        chat_id,
        progress_msg.id,
        ctx.message_thread_id,
        ctx.use_inline_progress_controls,
    );
    let cfg = ProgressRuntimeConfig::new(AGENT_MAX_ITERATIONS);
    let progress_handle = spawn_progress_runtime(transport, rx, cfg);

    // Execute the task
    let result = execute_agent_task(ctx.session_id, &task_text, Some(tx)).await;
    let state = match progress_handle.await {
        Ok(state) => state,
        Err(err) => {
            warn!(error = %err, "Progress runtime task failed");
            ProgressState::new(AGENT_MAX_ITERATIONS)
        }
    };
    let progress_text = render_progress_html(&state);

    // Save agent memory after task execution
    save_memory_after_task(
        ctx.session_id,
        user_id,
        &ctx.context_key,
        &ctx.agent_flow_id,
        &ctx.storage,
    )
    .await;

    deliver_agent_task_result(
        &ctx,
        result,
        &progress_text,
        progress_msg.id,
        progress_reply_markup,
    )
    .await?;

    Ok(())
}

async fn deliver_agent_task_result(
    ctx: &AgentTaskContext,
    result: Result<String>,
    progress_text: &str,
    progress_message_id: teloxide::types::MessageId,
    progress_reply_markup: Option<teloxide::types::InlineKeyboardMarkup>,
) -> Result<()> {
    let terminal_progress_reply_markup = progress_reply_markup
        .as_ref()
        .map(|_| empty_inline_keyboard());
    let cancelled = result.as_ref().err().is_some_and(is_task_cancelled_error);

    match result {
        Ok(response) => {
            super::resilient::edit_message_safe_resilient_with_markup(
                &ctx.bot,
                ctx.msg.chat.id,
                progress_message_id,
                progress_text,
                terminal_progress_reply_markup.clone(),
            )
            .await;
            let final_markup = ctx
                .use_inline_progress_controls
                .then(|| agent_flow_inline_keyboard(&ctx.agent_flow_id));
            send_long_message_in_thread_with_final_markup(
                &ctx.bot,
                ctx.msg.chat.id,
                &response,
                ctx.message_thread_id,
                final_markup,
            )
            .await?;
        }
        Err(e) => {
            let sanitized_error = oxide_agent_core::utils::sanitize_html_error(&e.to_string());
            let error_text = format!("{progress_text}\n\n❌ <b>Error:</b>\n\n{sanitized_error}");
            super::resilient::edit_message_safe_resilient_with_markup(
                &ctx.bot,
                ctx.msg.chat.id,
                progress_message_id,
                &error_text,
                terminal_progress_reply_markup,
            )
            .await;
        }
    }

    finalize_cancel_status_if_needed(
        &ctx.bot,
        ctx.session_id,
        ctx.msg.chat.id,
        cancelled,
        cancel_status_inline_markup(ctx.use_inline_progress_controls, &ctx.agent_flow_id),
    )
    .await;

    Ok(())
}

struct RunAgentTaskTextContext {
    bot: Bot,
    chat_id: ChatId,
    session_id: SessionId,
    user_id: i64,
    task_text: String,
    storage: Arc<dyn StorageProvider>,
    context_key: String,
    agent_flow_id: String,
    message_thread_id: Option<ThreadId>,
    use_inline_progress_controls: bool,
}

async fn run_agent_task_with_text(ctx: RunAgentTaskTextContext) -> Result<()> {
    let progress_reply_markup = ctx
        .use_inline_progress_controls
        .then_some(progress_inline_keyboard());

    let progress_msg = super::resilient::send_message_resilient_with_thread_and_markup(
        &ctx.bot,
        ctx.chat_id,
        "⏳ Processing task...",
        Some(ParseMode::Html),
        ctx.message_thread_id,
        progress_reply_markup.clone().map(Into::into),
    )
    .await?;

    let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(100);
    let transport = TelegramAgentTransport::new(
        ctx.bot.clone(),
        ctx.chat_id,
        progress_msg.id,
        ctx.message_thread_id,
        ctx.use_inline_progress_controls,
    );
    let cfg = ProgressRuntimeConfig::new(AGENT_MAX_ITERATIONS);
    let progress_handle = spawn_progress_runtime(transport, rx, cfg);

    let result = execute_agent_task(ctx.session_id, &ctx.task_text, Some(tx)).await;
    let state = match progress_handle.await {
        Ok(state) => state,
        Err(err) => {
            warn!(error = %err, "Progress runtime task failed");
            ProgressState::new(AGENT_MAX_ITERATIONS)
        }
    };
    let progress_text = render_progress_html(&state);
    let pending_ssh_approvals = take_pending_ssh_approvals(ctx.session_id).await;

    save_memory_after_task(
        ctx.session_id,
        ctx.user_id,
        &ctx.context_key,
        &ctx.agent_flow_id,
        &ctx.storage,
    )
    .await;

    let cancelled = result.as_ref().err().is_some_and(is_task_cancelled_error);

    match result {
        Ok(response) => {
            let terminal_progress_reply_markup = progress_reply_markup
                .as_ref()
                .map(|_| empty_inline_keyboard());
            super::resilient::edit_message_safe_resilient_with_markup(
                &ctx.bot,
                ctx.chat_id,
                progress_msg.id,
                &progress_text,
                terminal_progress_reply_markup.clone(),
            )
            .await;
            // Use send_long_message to properly split response if it exceeds Telegram limit
            let final_markup = ctx
                .use_inline_progress_controls
                .then(|| agent_flow_inline_keyboard(&ctx.agent_flow_id));
            send_long_message_in_thread_with_final_markup(
                &ctx.bot,
                ctx.chat_id,
                &response,
                ctx.message_thread_id,
                final_markup,
            )
            .await?;
            send_pending_ssh_approval_messages(
                &ctx.bot,
                ctx.chat_id,
                ctx.message_thread_id,
                &pending_ssh_approvals,
            )
            .await?;
        }
        Err(e) => {
            // Sanitize error text to prevent Telegram HTML parse errors
            let sanitized_error = oxide_agent_core::utils::sanitize_html_error(&e.to_string());
            let error_text = format!("{progress_text}\n\n❌ <b>Error:</b>\n\n{sanitized_error}");
            let terminal_progress_reply_markup = progress_reply_markup
                .as_ref()
                .map(|_| empty_inline_keyboard());
            super::resilient::edit_message_safe_resilient_with_markup(
                &ctx.bot,
                ctx.chat_id,
                progress_msg.id,
                &error_text,
                terminal_progress_reply_markup,
            )
            .await;
        }
    }

    finalize_cancel_status_if_needed(
        &ctx.bot,
        ctx.session_id,
        ctx.chat_id,
        cancelled,
        cancel_status_inline_markup(ctx.use_inline_progress_controls, &ctx.agent_flow_id),
    )
    .await;

    Ok(())
}

/// Execute an agent task and return the result
async fn execute_agent_task(
    session_id: SessionId,
    task: &str,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> Result<String> {
    // Get executor from registry
    let executor_arc = SESSION_REGISTRY
        .get(&session_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("No agent session found"))?;

    // Get the cancellation token for this task
    let cancellation_token = SESSION_REGISTRY
        .get_cancellation_token(&session_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("No cancellation token found"))?;

    // Acquire write lock on the executor
    let mut executor = executor_arc.write().await;

    debug!(
        session_id = %session_id,
        memory_messages = executor.session().memory.get_messages().len(),
        "Executor accessed for task execution"
    );

    // Check timeout
    if executor.is_timed_out() {
        executor.reset();
        return Err(anyhow::anyhow!(
            "Previous session timed out. Starting a new session."
        ));
    }

    // IMPORTANT: Set the external cancellation token into session
    executor.session_mut().cancellation_token = (*cancellation_token).clone();

    // Execute the task (now uses external token that can be cancelled lock-free)
    executor.execute(task, progress_tx).await
}

async fn take_pending_ssh_approvals(
    session_id: SessionId,
) -> Vec<oxide_agent_core::agent::SshApprovalRequestView> {
    let Some(executor_arc) = SESSION_REGISTRY.get(&session_id).await else {
        return Vec::new();
    };
    let executor = executor_arc.read().await;
    executor.take_pending_ssh_approvals().await
}

async fn send_pending_ssh_approval_messages(
    bot: &Bot,
    chat_id: ChatId,
    message_thread_id: Option<ThreadId>,
    requests: &[oxide_agent_core::agent::SshApprovalRequestView],
) -> Result<()> {
    for request in requests {
        let text = format!(
            "⚠️ <b>SSH approval required</b>\n\nTarget: <b>{}</b>\nTool: <code>{}</code>\n\n{}",
            html_escape::encode_text(&request.target_name),
            html_escape::encode_text(&request.tool_name),
            html_escape::encode_text(&request.summary),
        );
        let mut req = bot.send_message(chat_id, text).parse_mode(ParseMode::Html);
        if let Some(thread_id) = message_thread_id {
            req = req.message_thread_id(thread_id);
        }
        req.reply_markup(ssh_approval_inline_keyboard(&request.request_id))
            .await?;
    }

    Ok(())
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

    let task_text = {
        let mut executor = executor_arc.write().await;
        let Some(grant) = executor.grant_ssh_approval(&request_id).await else {
            send_agent_message(
                &ctx.loop_ctx.bot,
                ctx.loop_ctx.chat_id,
                "SSH approval request not found or already handled.",
                ctx.loop_ctx.outbound_thread,
            )
            .await?;
            return Ok(());
        };
        executor.inject_system_message(inject_ssh_approval_system_message(&grant).content);
        executor.last_task().map(str::to_string)
    };

    let Some(task_text) = task_text else {
        send_agent_message(
            &ctx.loop_ctx.bot,
            ctx.loop_ctx.chat_id,
            DefaultAgentView::no_saved_task(),
            ctx.loop_ctx.outbound_thread,
        )
        .await?;
        return Ok(());
    };

    send_agent_message(
        &ctx.loop_ctx.bot,
        ctx.loop_ctx.chat_id,
        "SSH approval granted. Resuming the task.",
        ctx.loop_ctx.outbound_thread,
    )
    .await?;

    let retry_ctx = ctx.loop_ctx.clone();
    let storage = ctx.storage.clone();
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
        let executor = executor_arc.read().await;
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
    send_agent_message_with_keyboard(
        &ctx.bot,
        ctx.chat_id,
        DefaultAgentView::task_cancel_confirmation(),
        &cancel_task_confirmation_inline_keyboard().into(),
        ctx.outbound_thread,
    )
    .await
}

async fn handle_cancel_task_confirmation_callback(
    ctx: &AgentCallbackContext,
    confirmed: bool,
) -> Result<()> {
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
