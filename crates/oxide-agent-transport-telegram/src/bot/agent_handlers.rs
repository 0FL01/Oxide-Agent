//! Agent mode handlers for Telegram bot
//!
//! Provides handlers for activating agent mode, processing messages,
//! and managing agent sessions.

use crate::bot::agent::extract_agent_input;
use crate::bot::agent_transport::TelegramAgentTransport;
use crate::bot::messaging::send_long_message_in_thread;
use crate::bot::progress_render::render_progress_html;
use crate::bot::state::{ConfirmationType, State};
use crate::bot::topic_route::resolve_topic_route;
use crate::bot::views::{
    confirmation_keyboard, get_agent_keyboard, AgentView, DefaultAgentView, LOOP_CALLBACK_CANCEL,
    LOOP_CALLBACK_RESET, LOOP_CALLBACK_RETRY,
};
use crate::bot::{build_outbound_thread_params, resolve_thread_spec, OutboundThreadParams};
use crate::config::BotSettings;
use anyhow::{Error, Result};
use oxide_agent_core::agent::{
    executor::AgentExecutor,
    preprocessor::Preprocessor,
    progress::{AgentEvent, ProgressState},
    AgentSession, SessionId,
};
use oxide_agent_core::config::AGENT_MAX_ITERATIONS;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::storage::StorageProvider;
use oxide_agent_runtime::SessionRegistry;
use oxide_agent_runtime::{spawn_progress_runtime, ProgressRuntimeConfig};
use std::sync::Arc;
use std::sync::LazyLock;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, ParseMode, ThreadId};
use tracing::{debug, info, warn};

/// Type alias for dialogue
pub type AgentDialogue = Dialogue<State, InMemStorage<State>>;

/// Context for running an agent task without blocking the update handler
struct AgentTaskContext {
    bot: Bot,
    msg: Message,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    message_thread_id: Option<ThreadId>,
    session_id: SessionId,
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
}

fn parse_agent_control_command(text: Option<&str>) -> Option<AgentControlCommand> {
    match text {
        Some("❌ Cancel Task") => Some(AgentControlCommand::CancelTask),
        Some("🗑 Clear Memory") => Some(AgentControlCommand::ClearMemory),
        Some("🔄 Recreate Container") => Some(AgentControlCommand::RecreateContainer),
        Some("⬅️ Exit Agent Mode") => Some(AgentControlCommand::ExitAgentMode),
        _ => None,
    }
}

/// Global session registry for agent executors
static SESSION_REGISTRY: LazyLock<SessionRegistry> = LazyLock::new(SessionRegistry::new);

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn fnv1a_mix_i64(mut hash: u64, value: i64) -> u64 {
    for byte in value.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    hash
}

fn derive_agent_mode_session_id(
    user_id: i64,
    chat_id: ChatId,
    thread_id: Option<ThreadId>,
) -> SessionId {
    let Some(thread_id) = thread_id else {
        return SessionId::from(user_id);
    };

    let mut hash = FNV_OFFSET_BASIS;
    hash = fnv1a_mix_i64(hash, user_id);
    hash = fnv1a_mix_i64(hash, chat_id.0);
    hash = fnv1a_mix_i64(hash, i64::from(thread_id.0 .0));

    let folded = hash & (i64::MAX as u64);
    let derived = if folded == 0 { -1 } else { -(folded as i64) };
    SessionId::from(derived)
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

async fn send_agent_message_with_keyboard(
    bot: &Bot,
    chat_id: ChatId,
    text: impl Into<String>,
    keyboard: &teloxide::types::KeyboardMarkup,
    outbound_thread: OutboundThreadParams,
) -> Result<()> {
    let mut req = bot.send_message(chat_id, text);
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.reply_markup(keyboard.clone()).await?;
    Ok(())
}

struct ConfirmationSendCtx<'a> {
    bot: &'a Bot,
    chat_id: ChatId,
    keyboard: &'a teloxide::types::KeyboardMarkup,
    outbound_thread: OutboundThreadParams,
}

async fn handle_clear_memory_confirmation(
    user_id: i64,
    session_id: SessionId,
    storage: &Arc<dyn StorageProvider>,
    send_ctx: &ConfirmationSendCtx<'_>,
) -> Result<()> {
    info!(user_id = user_id, "User confirmed memory clear");
    match SESSION_REGISTRY.reset(&session_id).await {
        Ok(()) => {
            let _ = storage.clear_agent_memory(user_id).await;
            send_agent_message_with_keyboard(
                send_ctx.bot,
                send_ctx.chat_id,
                DefaultAgentView::memory_cleared(),
                send_ctx.keyboard,
                send_ctx.outbound_thread,
            )
            .await?;
        }
        Err("Cannot reset while task is running") => {
            send_agent_message_with_keyboard(
                send_ctx.bot,
                send_ctx.chat_id,
                DefaultAgentView::clear_blocked_by_task(),
                send_ctx.keyboard,
                send_ctx.outbound_thread,
            )
            .await?;
        }
        Err(_) => {
            let _ = storage.clear_agent_memory(user_id).await;
            send_agent_message_with_keyboard(
                send_ctx.bot,
                send_ctx.chat_id,
                DefaultAgentView::memory_cleared(),
                send_ctx.keyboard,
                send_ctx.outbound_thread,
            )
            .await?;
        }
    }

    Ok(())
}

async fn handle_recreate_container_confirmation(
    user_id: i64,
    session_id: SessionId,
    storage: &Arc<dyn StorageProvider>,
    llm: &Arc<LlmClient>,
    settings: &Arc<BotSettings>,
    send_ctx: &ConfirmationSendCtx<'_>,
) -> Result<()> {
    info!(user_id = user_id, "User confirmed container recreation");
    ensure_session_exists(session_id, user_id, llm, storage, settings).await;

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
            send_agent_message_with_keyboard(
                send_ctx.bot,
                send_ctx.chat_id,
                DefaultAgentView::container_recreated(),
                send_ctx.keyboard,
                send_ctx.outbound_thread,
            )
            .await?;
        }
        Ok(Err(AgentWipeError::Recreate(e))) => {
            warn!(error = %e, "Container recreation failed");
            send_agent_message_with_keyboard(
                send_ctx.bot,
                send_ctx.chat_id,
                DefaultAgentView::container_error(&format!("{e:#}")),
                send_ctx.keyboard,
                send_ctx.outbound_thread,
            )
            .await?;
        }
        Err("Cannot reset while task is running") => {
            send_agent_message_with_keyboard(
                send_ctx.bot,
                send_ctx.chat_id,
                DefaultAgentView::container_recreate_blocked_by_task(),
                send_ctx.keyboard,
                send_ctx.outbound_thread,
            )
            .await?;
        }
        Err(_) => {
            send_agent_message_with_keyboard(
                send_ctx.bot,
                send_ctx.chat_id,
                DefaultAgentView::sandbox_access_error(),
                send_ctx.keyboard,
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
) -> Result<()> {
    let thread_spec = resolve_thread_spec(&msg);
    let outbound_thread = build_outbound_thread_params(thread_spec);
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let session_id = derive_agent_mode_session_id(user_id, msg.chat.id, thread_spec.thread_id);

    info!("Activating agent mode for user {user_id}");

    // Create new session
    let mut session = AgentSession::new(session_id);

    // Load saved agent memory if exists
    if let Ok(Some(saved_memory)) = storage.load_agent_memory(user_id).await {
        session.memory = saved_memory;
        info!("Loaded agent memory for user {user_id}");
    }

    let executor = AgentExecutor::new(llm.clone(), session, settings.agent.clone());

    // Store session in registry
    SESSION_REGISTRY.insert(session_id, executor).await;

    // Save state to DB
    storage
        .update_user_state(user_id, "agent_mode".to_string())
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

    req.reply_markup(get_agent_keyboard()).await?;

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
    let session_id = derive_agent_mode_session_id(user_id, chat_id, thread_spec.thread_id);

    if let Some(command) = parse_agent_control_command(msg.text()) {
        return match command {
            AgentControlCommand::CancelTask => cancel_agent_task(bot, msg, dialogue).await,
            AgentControlCommand::ClearMemory => {
                confirm_destructive_action(ConfirmationType::ClearMemory, bot, msg, dialogue).await
            }
            AgentControlCommand::RecreateContainer => {
                confirm_destructive_action(ConfirmationType::RecreateContainer, bot, msg, dialogue)
                    .await
            }
            AgentControlCommand::ExitAgentMode => {
                exit_agent_mode(bot, msg, dialogue, storage).await
            }
        };
    }

    let route = resolve_topic_route(&bot, &settings, &msg).await;

    if !route.allows_processing() {
        info!(
            "Skipping agent message in topic route for user {user_id}. enabled={}, require_mention={}, mention_satisfied={}",
            route.enabled, route.require_mention, route.mention_satisfied
        );
        return Ok(());
    }

    // Get or create session
    ensure_session_exists(session_id, user_id, &llm, &storage, &settings).await;

    if is_agent_task_running(session_id).await {
        let mut req = bot.send_message(
            chat_id,
            "⏳ A task is already running. Press ❌ Cancel Task to stop it.",
        );
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.reply_markup(get_agent_keyboard()).await?;
        return Ok(());
    }

    renew_cancellation_token(session_id).await;

    let task_bot = bot.clone();
    let task_msg = msg.clone();
    let task_storage = storage.clone();
    let task_llm = llm.clone();

    tokio::spawn(async move {
        let message_thread_id = outbound_thread.message_thread_id;
        let ctx = AgentTaskContext {
            bot: task_bot.clone(),
            msg: task_msg.clone(),
            storage: task_storage,
            llm: task_llm,
            message_thread_id,
            session_id,
        };

        if let Err(e) = run_agent_task(ctx).await {
            let mut req = task_bot.send_message(task_msg.chat.id, format!("❌ Error: {e}"));
            if let Some(thread_id) = message_thread_id {
                req = req.message_thread_id(thread_id);
            }

            let _ = req.await;
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{derive_agent_mode_session_id, parse_agent_control_command, AgentControlCommand};
    use teloxide::types::{ChatId, MessageId, ThreadId};

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
    }

    #[test]
    fn non_control_messages_do_not_bypass_topic_gate() {
        assert_eq!(parse_agent_control_command(Some("please help")), None);
        assert_eq!(parse_agent_control_command(Some("user@example.com")), None);
        assert_eq!(parse_agent_control_command(None), None);
    }

    #[test]
    fn session_id_derivation_uses_legacy_without_thread() {
        let user_id = 12345;
        let session_id = derive_agent_mode_session_id(user_id, ChatId(-1001), None);

        assert_eq!(session_id, user_id.into());
    }

    #[test]
    fn session_id_derivation_is_stable_for_same_thread() {
        let user_id = 12345;
        let thread_id = Some(ThreadId(MessageId(42)));
        let first = derive_agent_mode_session_id(user_id, ChatId(-1001), thread_id);
        let second = derive_agent_mode_session_id(user_id, ChatId(-1001), thread_id);

        assert_eq!(first, second);
    }

    #[test]
    fn session_id_derivation_differs_for_different_threads() {
        let user_id = 12345;
        let first =
            derive_agent_mode_session_id(user_id, ChatId(-1001), Some(ThreadId(MessageId(42))));
        let second =
            derive_agent_mode_session_id(user_id, ChatId(-1001), Some(ThreadId(MessageId(43))));

        assert_ne!(first, second);
    }
}

async fn ensure_session_exists(
    session_id: SessionId,
    user_id: i64,
    llm: &Arc<LlmClient>,
    storage: &Arc<dyn StorageProvider>,
    settings: &Arc<BotSettings>,
) {
    if SESSION_REGISTRY.contains(&session_id).await {
        debug!(session_id = %session_id, "Session already exists in cache");
        return;
    }

    let mut session = AgentSession::new(session_id);

    // Load saved agent memory if exists
    if let Ok(Some(saved_memory)) = storage.load_agent_memory(user_id).await {
        session.memory = saved_memory;
        info!(
            user_id = user_id,
            messages_count = session.memory.get_messages().len(),
            "Loaded agent memory for user in ensure_session_exists"
        );
    } else {
        info!(
            user_id = user_id,
            "No saved agent memory found, starting fresh"
        );
    }

    let executor = AgentExecutor::new(llm.clone(), session, settings.agent.clone());
    SESSION_REGISTRY.insert(session_id, executor).await;
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
    storage: &Arc<dyn StorageProvider>,
) {
    if let Some(executor_arc) = SESSION_REGISTRY.get(&session_id).await {
        let executor = executor_arc.read().await;
        let _ = storage
            .save_agent_memory(user_id, &executor.session().memory)
            .await;
    }
}

async fn run_agent_task(ctx: AgentTaskContext) -> Result<()> {
    let user_id = ctx.msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let chat_id = ctx.msg.chat.id;

    // Preprocess input
    let preprocessor = Preprocessor::new(ctx.llm.clone(), user_id);
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
    let progress_msg = super::resilient::send_message_resilient_with_thread(
        &ctx.bot,
        chat_id,
        "⏳ Processing task...",
        Some(ParseMode::Html),
        ctx.message_thread_id,
    )
    .await?;

    // Create progress tracking channel
    let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(100);
    let transport = TelegramAgentTransport::new(
        ctx.bot.clone(),
        chat_id,
        progress_msg.id,
        ctx.message_thread_id,
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
    save_memory_after_task(ctx.session_id, user_id, &ctx.storage).await;

    // Update the message with the result
    match result {
        Ok(response) => {
            super::resilient::edit_message_safe_resilient(
                &ctx.bot,
                chat_id,
                progress_msg.id,
                &progress_text,
            )
            .await;
            // Use send_long_message to properly split response if it exceeds Telegram limit
            send_long_message_in_thread(&ctx.bot, chat_id, &response, ctx.message_thread_id)
                .await?;
        }
        Err(e) => {
            // Sanitize error text to prevent Telegram HTML parse errors
            // (errors from API may contain raw HTML like Nginx error pages)
            let sanitized_error = oxide_agent_core::utils::sanitize_html_error(&e.to_string());
            let error_text = format!("{progress_text}\n\n❌ <b>Error:</b>\n\n{sanitized_error}");
            super::resilient::edit_message_safe_resilient(
                &ctx.bot,
                chat_id,
                progress_msg.id,
                &error_text,
            )
            .await;
        }
    }

    Ok(())
}

async fn run_agent_task_with_text(
    bot: Bot,
    chat_id: ChatId,
    session_id: SessionId,
    user_id: i64,
    task_text: String,
    storage: Arc<dyn StorageProvider>,
    message_thread_id: Option<ThreadId>,
) -> Result<()> {
    let progress_msg = super::resilient::send_message_resilient_with_thread(
        &bot,
        chat_id,
        "⏳ Processing task...",
        Some(ParseMode::Html),
        message_thread_id,
    )
    .await?;

    let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(100);
    let transport =
        TelegramAgentTransport::new(bot.clone(), chat_id, progress_msg.id, message_thread_id);
    let cfg = ProgressRuntimeConfig::new(AGENT_MAX_ITERATIONS);
    let progress_handle = spawn_progress_runtime(transport, rx, cfg);

    let result = execute_agent_task(session_id, &task_text, Some(tx)).await;
    let state = match progress_handle.await {
        Ok(state) => state,
        Err(err) => {
            warn!(error = %err, "Progress runtime task failed");
            ProgressState::new(AGENT_MAX_ITERATIONS)
        }
    };
    let progress_text = render_progress_html(&state);

    save_memory_after_task(session_id, user_id, &storage).await;

    match result {
        Ok(response) => {
            super::resilient::edit_message_safe_resilient(
                &bot,
                chat_id,
                progress_msg.id,
                &progress_text,
            )
            .await;
            // Use send_long_message to properly split response if it exceeds Telegram limit
            send_long_message_in_thread(&bot, chat_id, &response, message_thread_id).await?;
        }
        Err(e) => {
            // Sanitize error text to prevent Telegram HTML parse errors
            let sanitized_error = oxide_agent_core::utils::sanitize_html_error(&e.to_string());
            let error_text = format!("{progress_text}\n\n❌ <b>Error:</b>\n\n{sanitized_error}");
            super::resilient::edit_message_safe_resilient(
                &bot,
                chat_id,
                progress_msg.id,
                &error_text,
            )
            .await;
        }
    }

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

#[derive(Clone)]
struct LoopCallbackContext {
    bot: Bot,
    chat_id: ChatId,
    user_id: i64,
    session_id: SessionId,
    outbound_thread: OutboundThreadParams,
}

async fn handle_loop_retry(
    ctx: &LoopCallbackContext,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    settings: Arc<BotSettings>,
) -> Result<()> {
    if is_agent_task_running(ctx.session_id).await {
        send_agent_message(
            &ctx.bot,
            ctx.chat_id,
            DefaultAgentView::task_already_running(),
            ctx.outbound_thread,
        )
        .await?;
        return Ok(());
    }

    ensure_session_exists(ctx.session_id, ctx.user_id, &llm, &storage, &settings).await;
    renew_cancellation_token(ctx.session_id).await;

    let executor_arc = SESSION_REGISTRY.get(&ctx.session_id).await;
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
        if let Err(e) = run_agent_task_with_text(
            retry_ctx.bot,
            retry_ctx.chat_id,
            retry_ctx.session_id,
            retry_ctx.user_id,
            task_text,
            storage,
            retry_ctx.outbound_thread.message_thread_id,
        )
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
    SESSION_REGISTRY.cancel(&ctx.session_id).await;

    // Brief yield to allow the run loop to observe cancellation and release locks.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    match SESSION_REGISTRY.reset(&ctx.session_id).await {
        Ok(()) => {
            send_agent_message_with_keyboard(
                &ctx.bot,
                ctx.chat_id,
                DefaultAgentView::task_reset(),
                &get_agent_keyboard(),
                ctx.outbound_thread,
            )
            .await?;
        }
        Err("Session not found") => {
            send_agent_message(
                &ctx.bot,
                ctx.chat_id,
                DefaultAgentView::session_not_found(),
                ctx.outbound_thread,
            )
            .await?;
        }
        Err(_) => {
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

/// Handle loop-detection inline keyboard callbacks.
///
/// # Errors
///
/// Returns an error if Telegram API calls fail.
pub async fn handle_loop_callback(
    bot: Bot,
    q: CallbackQuery,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    settings: Arc<BotSettings>,
) -> Result<()> {
    let Some(data) = q.data.as_deref() else {
        return Ok(());
    };

    let _ = bot.answer_callback_query(q.id.clone()).await;

    let user_id = q.from.id.0.cast_signed();
    let chat_id = q
        .message
        .as_ref()
        .map(|msg| msg.chat().id)
        .ok_or_else(|| anyhow::anyhow!("Callback message missing chat id"))?;
    let thread_id = q
        .message
        .as_ref()
        .and_then(|message| message.regular_message())
        .map(resolve_thread_spec)
        .and_then(|spec| spec.thread_id);
    let session_id = derive_agent_mode_session_id(user_id, chat_id, thread_id);
    let ctx = LoopCallbackContext {
        bot,
        chat_id,
        user_id,
        session_id,
        outbound_thread: outbound_thread_from_callback(&q),
    };

    match data {
        LOOP_CALLBACK_RETRY => handle_loop_retry(&ctx, storage, llm, settings).await?,
        LOOP_CALLBACK_RESET => handle_loop_reset(&ctx).await?,
        LOOP_CALLBACK_CANCEL => {
            cancel_agent_task_by_id(
                ctx.bot.clone(),
                ctx.session_id,
                ctx.chat_id,
                ctx.outbound_thread.message_thread_id,
            )
            .await?;
        }
        _ => {}
    }

    Ok(())
}

/// Cancel the current agent task
///
/// # Errors
///
/// Returns an error if the cancellation message cannot be sent.
pub async fn cancel_agent_task(bot: Bot, msg: Message, _dialogue: AgentDialogue) -> Result<()> {
    let thread_spec = resolve_thread_spec(&msg);
    let outbound_thread = build_outbound_thread_params(thread_spec);
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let session_id = derive_agent_mode_session_id(user_id, msg.chat.id, thread_spec.thread_id);

    // Access the cancellation token from registry (lock-free)
    let cancelled = SESSION_REGISTRY.cancel(&session_id).await;

    // Best-effort: clear todos without waiting for executor locks.
    let cleared_todos = SESSION_REGISTRY.clear_todos(&session_id).await;

    let text = DefaultAgentView::task_cancelled(cleared_todos);
    if !cancelled && !cleared_todos {
        let mut req = bot.send_message(msg.chat.id, DefaultAgentView::no_active_task());
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.reply_markup(get_agent_keyboard()).await?;
    } else {
        let mut req = bot.send_message(msg.chat.id, text);
        if let Some(thread_id) = outbound_thread.message_thread_id {
            req = req.message_thread_id(thread_id);
        }

        req.reply_markup(get_agent_keyboard()).await?;
    }
    Ok(())
}

async fn cancel_agent_task_by_id(
    bot: Bot,
    session_id: SessionId,
    chat_id: ChatId,
    message_thread_id: Option<ThreadId>,
) -> Result<()> {
    let cancelled = SESSION_REGISTRY.cancel(&session_id).await;
    let cleared_todos = SESSION_REGISTRY.clear_todos(&session_id).await;
    let outbound_thread = OutboundThreadParams { message_thread_id };

    let text = DefaultAgentView::task_cancelled(cleared_todos);
    if !cancelled && !cleared_todos {
        send_agent_message_with_keyboard(
            &bot,
            chat_id,
            DefaultAgentView::no_active_task(),
            &get_agent_keyboard(),
            outbound_thread,
        )
        .await?;
    } else {
        send_agent_message_with_keyboard(
            &bot,
            chat_id,
            text,
            &get_agent_keyboard(),
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
    let session_id = derive_agent_mode_session_id(user_id, msg.chat.id, thread_spec.thread_id);

    save_memory_after_task(session_id, user_id, &storage).await;
    SESSION_REGISTRY.remove(&session_id).await;

    let _ = storage
        .update_user_state(user_id, "chat_mode".to_string())
        .await;
    dialogue.update(State::Start).await?;

    let keyboard = crate::bot::handlers::get_main_keyboard();
    let mut req = bot.send_message(msg.chat.id, "👋 Exited agent mode. Select a working mode:");
    if let Some(thread_id) = outbound_thread.message_thread_id {
        req = req.message_thread_id(thread_id);
    }

    req.reply_markup(keyboard).await?;
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
    let outbound_thread = outbound_thread_from_message(&msg);
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

    req.reply_markup(confirmation_keyboard()).await?;
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
    let session_id = derive_agent_mode_session_id(user_id, msg.chat.id, thread_spec.thread_id);
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
    let keyboard = get_agent_keyboard();
    let send_ctx = ConfirmationSendCtx {
        bot: &bot,
        chat_id,
        keyboard: &keyboard,
        outbound_thread,
    };

    match text {
        "✅ Yes" => match action {
            ConfirmationType::ClearMemory => {
                handle_clear_memory_confirmation(user_id, session_id, &storage, &send_ctx).await?;
            }
            ConfirmationType::RecreateContainer => {
                handle_recreate_container_confirmation(
                    user_id, session_id, &storage, &llm, &settings, &send_ctx,
                )
                .await?;
            }
        },
        "❌ Cancel" => {
            info!(user_id = user_id, action = ?action, "User cancelled destructive action");
            send_agent_message_with_keyboard(
                &bot,
                chat_id,
                DefaultAgentView::operation_cancelled(),
                &keyboard,
                outbound_thread,
            )
            .await?;
        }
        _ => unreachable!(),
    }

    Ok(())
}
