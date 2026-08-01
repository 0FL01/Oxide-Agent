use super::{
    ActiveSessionConfig, AgentControlCommand, AgentDialogue, AgentTaskContext,
    BatchedTextInputCheck, EnsureSessionContext, RunningAgentMessageContext,
    SessionTransportContext, ShowModelSelectorContext, build_batched_text_task_context,
    cancel_agent_task, configure_active_session, confirm_destructive_action,
    ensure_agent_flow_session, ensure_session_exists, exit_agent_mode,
    handle_batched_text_input_if_needed, handle_running_agent_message_if_needed,
    manager_control_plane_enabled, manager_default_chat_id, parse_agent_control_command,
    resolve_execution_profile, resolve_topic_infra_config, route_allows_agent_processing,
    show_agent_controls, show_model_selector, spawn_agent_task, use_inline_flow_controls,
    use_inline_topic_controls,
};
use crate::bot::context::{
    current_context_state, sandbox_scope, set_current_context_state, storage_context_key,
};
use crate::bot::state::ConfirmationType;
use crate::bot::thread::OutboundThreadParams;
use crate::bot::topic_route::{
    TopicRouteDecision, resolve_topic_route, touch_dynamic_binding_activity_if_needed,
};
use crate::bot::{build_outbound_thread_params, resolve_thread_spec};
use crate::config::BotSettings;
use anyhow::Result;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::sandbox::SandboxScope;
use oxide_agent_core::storage::StorageProvider;
use std::sync::Arc;
use teloxide::prelude::*;

struct PreSpawnAgentMessageContext<'a> {
    msg: &'a Message,
    bot: &'a Bot,
    storage: &'a Arc<dyn StorageProvider>,
    llm: &'a Arc<LlmClient>,
    route: &'a TopicRouteDecision,
    sandbox_scope: &'a SandboxScope,
    active_session: &'a ActiveSessionConfig,
    outbound_thread: OutboundThreadParams,
    attach_detach_enabled: bool,
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

    if let Some(command) = parse_agent_control_command(msg.text()) {
        return handle_agent_control_command(command, bot, msg, dialogue, storage, llm, settings)
            .await;
    }

    if !is_supported_agent_input(&msg) {
        return Ok(());
    }

    let route = resolve_topic_route(&bot, storage.as_ref(), user_id, &settings, &msg).await;
    if !route_allows_agent_processing(&route, user_id) {
        return Ok(());
    }

    if current_context_state(&storage, user_id, chat_id, thread_spec)
        .await?
        .as_deref()
        != Some("agent_mode")
    {
        set_current_context_state(&storage, user_id, chat_id, thread_spec, Some("agent_mode"))
            .await?;
    }

    let (agent_flow_id, agent_flow_created, session_id) =
        ensure_agent_flow_session(&storage, user_id, chat_id, thread_spec).await?;

    let manager_enabled = manager_control_plane_enabled(&settings, user_id, chat_id, thread_spec);
    let session_id = ensure_session_exists(EnsureSessionContext {
        session_id,
        context_key: context_key.clone(),
        agent_flow_id: agent_flow_id.clone(),
        agent_flow_created,
        sandbox_scope: sandbox_scope.clone(),
        user_id,
        bot: &bot,
        transport_ctx: SessionTransportContext {
            chat_id,
            manager_default_chat_id: manager_default_chat_id(&settings, chat_id, thread_spec),
            thread_spec,
        },
        llm: &llm,
        storage: &storage,
        settings: &settings,
    })
    .await;

    let active_session = ActiveSessionConfig {
        session_id,
        storage: storage.clone(),
        llm: llm.clone(),
        agent_settings: settings.agent.clone(),
        user_id,
        context_key: context_key.clone(),
        agent_flow_id: agent_flow_id.clone(),
        chat_id,
        thread_spec,
    };

    configure_message_active_session(
        &storage,
        user_id,
        &context_key,
        &route,
        manager_enabled,
        &active_session,
    )
    .await;

    if handle_pre_spawn_agent_message(PreSpawnAgentMessageContext {
        msg: &msg,
        bot: &bot,
        storage: &storage,
        llm: &llm,
        route: &route,
        sandbox_scope: &sandbox_scope,
        active_session: &active_session,
        outbound_thread,
        attach_detach_enabled: settings.telegram.attach_detach_enabled,
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
        agent_settings: settings.agent.clone(),
        context_key,
        agent_flow_id,
        sandbox_scope,
        message_thread_id: outbound_thread.message_thread_id,
        use_inline_progress_controls: use_inline_topic_controls(thread_spec),
        use_inline_flow_controls: use_inline_flow_controls(thread_spec),
        attach_detach_enabled: settings.telegram.attach_detach_enabled,
        session_id,
    });

    touch_dynamic_binding_activity_if_needed(storage.as_ref(), user_id, &route).await;
    Ok(())
}

fn is_supported_agent_input(msg: &Message) -> bool {
    msg.text().is_some()
        || msg.voice().is_some()
        || msg.photo().is_some()
        || msg.video().is_some()
        || msg.document().is_some()
}

async fn handle_pre_spawn_agent_message(ctx: PreSpawnAgentMessageContext<'_>) -> Result<bool> {
    let dispatch_ctx = build_batched_text_task_context(
        ctx.bot,
        ctx.active_session,
        ctx.outbound_thread,
        ctx.attach_detach_enabled,
    );
    if handle_batched_text_input_if_needed(BatchedTextInputCheck {
        msg: ctx.msg,
        bot: ctx.bot,
        storage: ctx.storage,
        llm: ctx.llm,
        agent_settings: &ctx.active_session.agent_settings,
        route: ctx.route,
        thread_spec: ctx.active_session.thread_spec,
        outbound_thread: ctx.outbound_thread,
        session_id: ctx.active_session.session_id,
        user_id: ctx.active_session.user_id,
        chat_id: ctx.active_session.chat_id,
        context_key: &ctx.active_session.context_key,
        agent_flow_id: &ctx.active_session.agent_flow_id,
        attach_detach_enabled: ctx.attach_detach_enabled,
    })
    .await?
    {
        return Ok(true);
    }

    handle_running_agent_message_if_needed(RunningAgentMessageContext {
        msg: ctx.msg,
        bot: ctx.bot,
        route: ctx.route,
        sandbox_scope: ctx.sandbox_scope,
        dispatch: dispatch_ctx,
        thread_spec: ctx.active_session.thread_spec,
        outbound_thread: ctx.outbound_thread,
        llm: ctx.llm,
    })
    .await
}

async fn configure_message_active_session(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: &str,
    route: &TopicRouteDecision,
    manager_enabled: bool,
    active_session: &ActiveSessionConfig,
) {
    let execution_profile = resolve_execution_profile(
        storage,
        user_id,
        context_key,
        route,
        manager_enabled,
        active_session.thread_spec,
    )
    .await;
    let topic_infra_config = resolve_topic_infra_config(storage, user_id, context_key).await;
    configure_active_session(active_session, execution_profile, topic_infra_config).await;
}

async fn handle_agent_control_command(
    command: AgentControlCommand,
    bot: Bot,
    msg: Message,
    dialogue: AgentDialogue,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    settings: Arc<BotSettings>,
) -> Result<()> {
    match command {
        AgentControlCommand::CancelTask => {
            cancel_agent_task(bot, msg, dialogue, storage, settings).await
        }
        AgentControlCommand::ClearMemory => {
            confirm_destructive_action(ConfirmationType::ClearMemory, bot, msg, dialogue).await
        }
        AgentControlCommand::CompactContext => {
            confirm_destructive_action(ConfirmationType::CompactContext, bot, msg, dialogue).await
        }
        AgentControlCommand::RecreateContainer => {
            confirm_destructive_action(ConfirmationType::RecreateContainer, bot, msg, dialogue)
                .await
        }
        AgentControlCommand::ExitAgentMode => exit_agent_mode(bot, msg, dialogue, storage).await,
        AgentControlCommand::ShowControls => show_agent_controls(bot, msg, storage, settings).await,
        AgentControlCommand::ShowModelSelector => {
            let thread_spec = resolve_thread_spec(&msg);
            let user_id = msg.from.as_ref().map_or(0, |user| user.id.0.cast_signed());
            let context_key = storage_context_key(msg.chat.id, thread_spec);
            show_model_selector(ShowModelSelectorContext {
                bot: &bot,
                chat_id: msg.chat.id,
                outbound_thread: build_outbound_thread_params(thread_spec),
                user_id,
                context_key: &context_key,
                storage: &storage,
                llm: &llm,
                settings: &settings,
            })
            .await
        }
    }
}
