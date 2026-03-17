use super::{
    agent_mode_session_keys, automatic_agent_control_markup, build_batched_text_task_context,
    cancel_agent_task, configure_active_session, confirm_destructive_action,
    ensure_agent_flow_session_keys, ensure_session_exists, exit_agent_mode,
    handle_batched_text_input_if_needed, handle_running_agent_message_if_needed,
    is_agent_mode_context, manager_control_plane_enabled, manager_default_chat_id,
    parse_agent_control_command, resolve_execution_profile, resolve_topic_infra_config,
    route_allows_agent_processing, show_agent_controls, spawn_agent_task,
    use_inline_topic_controls, ActiveSessionConfig, AgentControlCommand, AgentDialogue,
    AgentTaskContext, BatchedTextInputCheck, EnsureSessionContext, RunningAgentMessageContext,
    SessionTransportContext,
};
use crate::bot::context::{
    ensure_current_agent_flow_id, sandbox_scope, set_current_context_state, storage_context_key,
};
use crate::bot::state::{ConfirmationType, State};
use crate::bot::topic_route::{resolve_topic_route, touch_dynamic_binding_activity_if_needed};
use crate::bot::views::{AgentView, DefaultAgentView};
use crate::bot::{build_outbound_thread_params, resolve_thread_spec};
use crate::config::BotSettings;
use anyhow::Result;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::storage::StorageProvider;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ParseMode;
use tracing::info;

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
    dialogue.update(State::AgentMode).await?;

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

    super::renew_cancellation_token(session_id).await;
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
