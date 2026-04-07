use super::{
    agent_mode_session_keys, automatic_agent_control_markup, cancel_and_clear_with_compat,
    cancel_status_inline_markup, cancel_status_reply_markup, cleanup_abandoned_empty_flow,
    clear_cancel_confirmation_message, clear_pending_cancel_message, confirm_destructive_action,
    ensure_session_exists, exit_agent_mode, handle_clear_memory_confirmation,
    handle_recreate_container_confirmation, is_agent_task_running, manager_default_chat_id,
    outbound_thread_from_callback, renew_cancellation_token, reset_sessions_with_compat,
    resolve_existing_session_id, run_agent_task_with_text, run_approved_ssh_resume,
    save_memory_after_task, send_agent_message, send_agent_message_with_optional_keyboard,
    send_or_update_cancel_confirmation, send_or_update_pending_cancel_message,
    should_create_fresh_flow_on_detach, start_manual_compaction, use_inline_flow_controls,
    use_inline_topic_controls, AgentDialogue, AgentModeSessionKeys, ConfirmationSendCtx,
    EnsureSessionContext, ResetSessionOutcome, RunAgentTaskTextContext,
    RunApprovedSshResumeContext, SessionTransportContext, SESSION_REGISTRY,
};
use crate::bot::context::{
    ensure_current_agent_flow_id, reset_current_agent_flow_id, set_current_agent_flow_id,
    storage_context_key,
};
use crate::bot::state::{ConfirmationType, State};
use crate::bot::views::{
    AgentView, DefaultAgentView, AGENT_CALLBACK_ATTACH_PREFIX, AGENT_CALLBACK_CANCEL_TASK,
    AGENT_CALLBACK_CLEAR_MEMORY, AGENT_CALLBACK_COMPACT_CONTEXT, AGENT_CALLBACK_CONFIRM_CANCEL_NO,
    AGENT_CALLBACK_CONFIRM_CANCEL_YES, AGENT_CALLBACK_CONFIRM_CLEAR_CANCEL,
    AGENT_CALLBACK_CONFIRM_CLEAR_YES, AGENT_CALLBACK_CONFIRM_COMPACT_CANCEL,
    AGENT_CALLBACK_CONFIRM_COMPACT_YES, AGENT_CALLBACK_CONFIRM_RECREATE_CANCEL,
    AGENT_CALLBACK_CONFIRM_RECREATE_YES, AGENT_CALLBACK_DETACH, AGENT_CALLBACK_EXIT,
    AGENT_CALLBACK_RECREATE_CONTAINER, AGENT_CALLBACK_SSH_APPROVE_PREFIX,
    AGENT_CALLBACK_SSH_REJECT_PREFIX, LOOP_CALLBACK_CANCEL, LOOP_CALLBACK_RESET,
    LOOP_CALLBACK_RETRY,
};
use crate::bot::{
    build_outbound_thread_params, resolve_thread_spec, OutboundThreadParams, TelegramThreadSpec,
};
use crate::config::BotSettings;
use anyhow::Result;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::sandbox::SandboxScope;
use oxide_agent_core::storage::StorageProvider;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, ChatId, Message, ThreadId};
use tracing::info;

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
pub(crate) enum AgentCallbackAction {
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
    persistent_memory_store: Arc<dyn oxide_agent_core::agent::PersistentMemoryStore>,
    settings: Arc<BotSettings>,
}

pub(crate) fn parse_agent_callback_action(data: &str) -> Option<AgentCallbackAction> {
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
        AGENT_CALLBACK_COMPACT_CONTEXT => Some(AgentCallbackAction::StartConfirmation(
            ConfirmationType::CompactContext,
        )),
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
        AGENT_CALLBACK_CONFIRM_COMPACT_YES => Some(AgentCallbackAction::ResolveConfirmation(
            ConfirmationType::CompactContext,
            true,
        )),
        AGENT_CALLBACK_CONFIRM_COMPACT_CANCEL => Some(AgentCallbackAction::ResolveConfirmation(
            ConfirmationType::CompactContext,
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
    persistent_memory_store: Arc<dyn oxide_agent_core::agent::PersistentMemoryStore>,
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
            chat_id: ctx.chat_id,
            manager_default_chat_id: ctx.manager_default_chat_id,
            thread_spec: ctx.thread_spec,
        },
        llm: &llm,
        storage: &storage,
        persistent_memory_store: &persistent_memory_store,
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
            use_inline_flow_controls: use_inline_flow_controls(retry_ctx.thread_spec),
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
    let _ = cancel_and_clear_with_compat(ctx.session_keys).await;
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
            use_inline_flow_controls: use_inline_flow_controls(retry_ctx.thread_spec),
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
            ConfirmationType::CompactContext => {
                start_manual_compaction(
                    loop_ctx.bot.clone(),
                    ctx.msg.clone(),
                    ctx.storage.clone(),
                    ctx.llm.clone(),
                    ctx.persistent_memory_store.clone(),
                    ctx.settings.clone(),
                )
                .await?;
            }
            ConfirmationType::RecreateContainer => {
                handle_recreate_container_confirmation(
                    loop_ctx.user_id,
                    loop_ctx.session_keys,
                    &ctx.storage,
                    &ctx.llm,
                    &ctx.persistent_memory_store,
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
                ctx.persistent_memory_store.clone(),
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

pub async fn handle_agent_callback(
    bot: Bot,
    q: CallbackQuery,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    persistent_memory_store: Arc<dyn oxide_agent_core::agent::PersistentMemoryStore>,
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
            manager_default_chat_id: manager_default_chat_id(&settings, chat_id, thread_spec),
            thread_spec,
            outbound_thread: outbound_thread_from_callback(&q),
        },
        msg,
        dialogue,
        storage,
        llm,
        persistent_memory_store,
        settings,
    };

    dispatch_agent_callback(action, ctx).await
}

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
            cancel_status_inline_markup(use_inline_flow_controls(thread_spec), &agent_flow_id),
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
            cancel_status_inline_markup(use_inline_flow_controls(thread_spec), agent_flow_id),
            outbound_thread,
        )
        .await?;
    }

    Ok(())
}
