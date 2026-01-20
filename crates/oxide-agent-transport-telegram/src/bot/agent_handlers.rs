//! Agent mode handlers for Telegram bot
//!
//! Provides handlers for activating agent mode, processing messages,
//! and managing agent sessions.

use oxide_agent_core::agent::{
    executor::AgentExecutor,
    preprocessor::Preprocessor,
    progress::{AgentEvent, ProgressState},
    AgentSession, SessionId,
};
use oxide_agent_runtime::SessionRegistry;
use crate::bot::agent::extract_agent_input;
use crate::bot::agent_transport::TelegramAgentTransport;
use crate::bot::messaging::send_long_message;
use crate::bot::progress_render::render_progress_html;
use crate::bot::state::{ConfirmationType, State};
use crate::bot::views::{
    confirmation_keyboard, get_agent_keyboard, AgentView, DefaultAgentView, LOOP_CALLBACK_CANCEL,
    LOOP_CALLBACK_RESET, LOOP_CALLBACK_RETRY,
};
use crate::config::BotSettings;
use oxide_agent_core::config::AGENT_MAX_ITERATIONS;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::storage::R2Storage;
use oxide_agent_runtime::{spawn_progress_runtime, ProgressRuntimeConfig};
use anyhow::{Error, Result};
use std::sync::Arc;
use std::sync::LazyLock;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, ParseMode};
use tracing::{debug, info, warn};

/// Type alias for dialogue
pub type AgentDialogue = Dialogue<State, InMemStorage<State>>;

/// Context for running an agent task without blocking the update handler
struct AgentTaskContext {
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
    llm: Arc<LlmClient>,
}

enum AgentWipeError {
    SandboxAccess(Error),
    Recreate(Error),
}

/// Global session registry for agent executors
static SESSION_REGISTRY: LazyLock<SessionRegistry> = LazyLock::new(SessionRegistry::new);

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
    storage: Arc<R2Storage>,
    settings: Arc<BotSettings>,
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let session_id = SessionId::from(user_id);

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
    bot.send_message(msg.chat.id, DefaultAgentView::welcome_message(&model_id))
        .parse_mode(ParseMode::Html)
        .reply_markup(get_agent_keyboard())
        .await?;

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
    storage: Arc<R2Storage>,
    llm: Arc<LlmClient>,
    dialogue: AgentDialogue,
    settings: Arc<BotSettings>,
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let chat_id = msg.chat.id;

    // Check for control commands
    if let Some(text) = msg.text() {
        match text {
            "❌ Cancel Task" => {
                return cancel_agent_task(bot, msg, dialogue).await;
            }
            "🗑 Clear Memory" => {
                return confirm_destructive_action(
                    ConfirmationType::ClearMemory,
                    bot,
                    msg,
                    dialogue,
                )
                .await;
            }
            "🔄 Recreate Container" => {
                return confirm_destructive_action(
                    ConfirmationType::RecreateContainer,
                    bot,
                    msg,
                    dialogue,
                )
                .await;
            }
            "⬅️ Exit Agent Mode" => {
                return exit_agent_mode(bot, msg, dialogue, storage).await;
            }
            _ => {}
        }
    }

    // Get or create session
    ensure_session_exists(user_id, &llm, &storage, &settings).await;

    if is_agent_task_running(user_id).await {
        bot.send_message(
            chat_id,
            "⏳ A task is already running. Press ❌ Cancel Task to stop it.",
        )
        .reply_markup(get_agent_keyboard())
        .await?;
        return Ok(());
    }

    renew_cancellation_token(user_id).await;

    let task_bot = bot.clone();
    let task_msg = msg.clone();
    let task_storage = storage.clone();
    let task_llm = llm.clone();

    tokio::spawn(async move {
        let ctx = AgentTaskContext {
            bot: task_bot.clone(),
            msg: task_msg.clone(),
            storage: task_storage,
            llm: task_llm,
        };

        if let Err(e) = run_agent_task(ctx).await {
            let _ = task_bot
                .send_message(task_msg.chat.id, format!("❌ Error: {e}"))
                .await;
        }
    });

    Ok(())
}

async fn ensure_session_exists(
    user_id: i64,
    llm: &Arc<LlmClient>,
    storage: &Arc<R2Storage>,
    settings: &Arc<BotSettings>,
) {
    let session_id = SessionId::from(user_id);
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

async fn is_agent_task_running(user_id: i64) -> bool {
    let session_id = SessionId::from(user_id);
    SESSION_REGISTRY.is_running(&session_id).await
}

async fn renew_cancellation_token(user_id: i64) {
    let session_id = SessionId::from(user_id);
    SESSION_REGISTRY.renew_cancellation_token(&session_id).await;
}

async fn save_memory_after_task(user_id: i64, storage: &Arc<R2Storage>) {
    let session_id = SessionId::from(user_id);
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
                super::resilient::send_message_resilient(
                    &ctx.bot,
                    chat_id,
                    "🚫 Agent cannot process this file.\nGemini/OpenRouter connection required for vision and audio capabilities.",
                    None,
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
    let progress_msg = super::resilient::send_message_resilient(
        &ctx.bot,
        chat_id,
        "⏳ Processing task...",
        Some(ParseMode::Html),
    )
    .await?;

    // Create progress tracking channel
    let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(100);
    let transport = TelegramAgentTransport::new(ctx.bot.clone(), chat_id, progress_msg.id);
    let cfg = ProgressRuntimeConfig::new(AGENT_MAX_ITERATIONS);
    let progress_handle = spawn_progress_runtime(transport, rx, cfg);

    // Execute the task
    let result = execute_agent_task(user_id, &task_text, Some(tx)).await;
    let state = match progress_handle.await {
        Ok(state) => state,
        Err(err) => {
            warn!(error = %err, "Progress runtime task failed");
            ProgressState::new(AGENT_MAX_ITERATIONS)
        }
    };
    let progress_text = render_progress_html(&state);

    // Save agent memory after task execution
    save_memory_after_task(user_id, &ctx.storage).await;

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
            send_long_message(&ctx.bot, chat_id, &response).await?;
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
    user_id: i64,
    task_text: String,
    storage: Arc<R2Storage>,
) -> Result<()> {
    let progress_msg = super::resilient::send_message_resilient(
        &bot,
        chat_id,
        "⏳ Processing task...",
        Some(ParseMode::Html),
    )
    .await?;

    let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(100);
    let transport = TelegramAgentTransport::new(bot.clone(), chat_id, progress_msg.id);
    let cfg = ProgressRuntimeConfig::new(AGENT_MAX_ITERATIONS);
    let progress_handle = spawn_progress_runtime(transport, rx, cfg);

    let result = execute_agent_task(user_id, &task_text, Some(tx)).await;
    let state = match progress_handle.await {
        Ok(state) => state,
        Err(err) => {
            warn!(error = %err, "Progress runtime task failed");
            ProgressState::new(AGENT_MAX_ITERATIONS)
        }
    };
    let progress_text = render_progress_html(&state);

    save_memory_after_task(user_id, &storage).await;

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
            send_long_message(&bot, chat_id, &response).await?;
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
    user_id: i64,
    task: &str,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> Result<String> {
    let session_id = SessionId::from(user_id);
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

/// Handle loop-detection inline keyboard callbacks.
///
/// # Errors
///
/// Returns an error if Telegram API calls fail.
pub async fn handle_loop_callback(
    bot: Bot,
    q: CallbackQuery,
    storage: Arc<R2Storage>,
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

    match data {
        LOOP_CALLBACK_RETRY => {
            if is_agent_task_running(user_id).await {
                bot.send_message(chat_id, DefaultAgentView::task_already_running())
                    .await?;
                return Ok(());
            }

            ensure_session_exists(user_id, &llm, &storage, &settings).await;
            renew_cancellation_token(user_id).await;

            let executor_arc = SESSION_REGISTRY.get(&SessionId::from(user_id)).await;

            let Some(executor_arc) = executor_arc else {
                bot.send_message(chat_id, DefaultAgentView::session_not_found())
                    .await?;
                return Ok(());
            };

            let task_text = {
                let executor = executor_arc.read().await;
                executor.last_task().map(str::to_string)
            };

            let Some(task_text) = task_text else {
                bot.send_message(chat_id, DefaultAgentView::no_saved_task())
                    .await?;
                return Ok(());
            };

            {
                let mut executor = executor_arc.write().await;
                executor.disable_loop_detection_next_run();
            }

            let task_bot = bot.clone();
            let task_storage = storage.clone();
            tokio::spawn(async move {
                let error_bot = task_bot.clone();
                if let Err(e) =
                    run_agent_task_with_text(task_bot, chat_id, user_id, task_text, task_storage)
                        .await
                {
                    let _ = error_bot
                        .send_message(chat_id, DefaultAgentView::error_message(&e.to_string()))
                        .await;
                }
            });
        }
        LOOP_CALLBACK_RESET => {
            // Cancel any running task first to release the executor lock.
            SESSION_REGISTRY.cancel(&SessionId::from(user_id)).await;

            // Brief yield to allow the run loop to observe cancellation and release locks.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            match SESSION_REGISTRY.reset(&SessionId::from(user_id)).await {
                Ok(()) => {
                    bot.send_message(chat_id, DefaultAgentView::task_reset())
                        .reply_markup(get_agent_keyboard())
                        .await?;
                }
                Err("Session not found") => {
                    bot.send_message(chat_id, DefaultAgentView::session_not_found())
                        .await?;
                }
                Err(_) => {
                    bot.send_message(chat_id, DefaultAgentView::reset_blocked_by_task())
                        .await?;
                }
            }
        }
        LOOP_CALLBACK_CANCEL => {
            cancel_agent_task_by_id(bot.clone(), user_id, chat_id).await?;
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
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());

    // Access the cancellation token from registry (lock-free)
    let cancelled = SESSION_REGISTRY.cancel(&SessionId::from(user_id)).await;

    // Best-effort: clear todos without waiting for executor locks.
    let cleared_todos = SESSION_REGISTRY.clear_todos(&SessionId::from(user_id)).await;

    let text = DefaultAgentView::task_cancelled(cleared_todos);
    if !cancelled && !cleared_todos {
        bot.send_message(msg.chat.id, DefaultAgentView::no_active_task())
            .reply_markup(get_agent_keyboard())
            .await?;
    } else {
        bot.send_message(msg.chat.id, text)
            .reply_markup(get_agent_keyboard())
            .await?;
    }
    Ok(())
}

async fn cancel_agent_task_by_id(bot: Bot, user_id: i64, chat_id: ChatId) -> Result<()> {
    let session_id = SessionId::from(user_id);
    let cancelled = SESSION_REGISTRY.cancel(&session_id).await;
    let cleared_todos = SESSION_REGISTRY.clear_todos(&session_id).await;

    let text = DefaultAgentView::task_cancelled(cleared_todos);
    if !cancelled && !cleared_todos {
        bot.send_message(chat_id, DefaultAgentView::no_active_task())
            .reply_markup(get_agent_keyboard())
            .await?;
    } else {
        bot.send_message(chat_id, text)
            .reply_markup(get_agent_keyboard())
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
    storage: Arc<R2Storage>,
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());

    save_memory_after_task(user_id, &storage).await;
    SESSION_REGISTRY.remove(&SessionId::from(user_id)).await;

    let _ = storage
        .update_user_state(user_id, "chat_mode".to_string())
        .await;
    dialogue.update(State::Start).await?;

    let keyboard = crate::bot::handlers::get_main_keyboard();
    bot.send_message(msg.chat.id, "👋 Exited agent mode. Select a working mode:")
        .reply_markup(keyboard)
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
    dialogue
        .update(State::AgentConfirmation(action.clone()))
        .await?;

    let message_text = match action {
        ConfirmationType::ClearMemory => DefaultAgentView::memory_clear_confirmation(),
        ConfirmationType::RecreateContainer => DefaultAgentView::container_wipe_confirmation(),
    };

    bot.send_message(msg.chat.id, message_text)
        .parse_mode(ParseMode::Html)
        .reply_markup(confirmation_keyboard())
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
    storage: Arc<R2Storage>,
    llm: Arc<LlmClient>,
    settings: Arc<BotSettings>,
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let text = msg.text().unwrap_or("");
    let chat_id = msg.chat.id;

    if text != "✅ Yes" && text != "❌ Cancel" {
        bot.send_message(chat_id, DefaultAgentView::select_keyboard_option())
            .await?;
        return Ok(());
    }

    dialogue.update(State::AgentMode).await?;
    let keyboard = get_agent_keyboard();

    match text {
        "✅ Yes" => match action {
            ConfirmationType::ClearMemory => {
                info!(user_id = user_id, "User confirmed memory clear");
                match SESSION_REGISTRY.reset(&SessionId::from(user_id)).await {
                    Ok(()) => {
                        let _ = storage.clear_agent_memory(user_id).await;
                        bot.send_message(chat_id, DefaultAgentView::memory_cleared())
                            .reply_markup(keyboard)
                            .await?;
                    }
                    Err("Cannot reset while task is running") => {
                        bot.send_message(chat_id, DefaultAgentView::clear_blocked_by_task())
                            .reply_markup(keyboard)
                            .await?;
                    }
                    Err(_) => {
                        // No session — just clear storage
                        let _ = storage.clear_agent_memory(user_id).await;
                        bot.send_message(chat_id, DefaultAgentView::memory_cleared())
                            .reply_markup(keyboard)
                            .await?;
                    }
                }
            }
            ConfirmationType::RecreateContainer => {
                info!(user_id = user_id, "User confirmed container recreation");
                // Ensure session exists (restores from DB if needs be, or creates new)
                ensure_session_exists(user_id, &llm, &storage, &settings).await;
                match SESSION_REGISTRY
                    .with_executor_mut(&SessionId::from(user_id), |executor| {
                        Box::pin(async move {
                            let sandbox = executor
                                .session_mut()
                                .ensure_sandbox()
                                .await
                                .map_err(AgentWipeError::SandboxAccess)?;
                            sandbox.recreate().await.map_err(AgentWipeError::Recreate)?;
                            Ok(())
                        })
                    })
                    .await
                {
                    Ok(Ok(())) => {
                        bot.send_message(chat_id, DefaultAgentView::container_recreated())
                            .reply_markup(keyboard)
                            .await?;
                    }
                    Ok(Err(AgentWipeError::SandboxAccess(e))) => {
                        warn!(error = %e, "Sandbox access failed during container recreate");
                        bot.send_message(chat_id, DefaultAgentView::sandbox_access_error())
                            .reply_markup(keyboard)
                            .await?;
                    }
                    Ok(Err(AgentWipeError::Recreate(e))) => {
                        warn!(error = %e, "Container recreation failed");
                        bot.send_message(
                            chat_id,
                            DefaultAgentView::container_error(&e.to_string()),
                        )
                        .reply_markup(keyboard)
                        .await?;
                    }
                    Err("Cannot reset while task is running") => {
                        bot.send_message(
                            chat_id,
                            DefaultAgentView::container_recreate_blocked_by_task(),
                        )
                        .reply_markup(keyboard)
                        .await?;
                    }
                    Err(_) => {
                        bot.send_message(chat_id, DefaultAgentView::sandbox_access_error())
                            .reply_markup(keyboard)
                            .await?;
                    }
                }
            }
        },
        "❌ Cancel" => {
            info!(user_id = user_id, action = ?action, "User cancelled destructive action");
            bot.send_message(chat_id, DefaultAgentView::operation_cancelled())
                .reply_markup(keyboard)
                .await?;
        }
        _ => unreachable!(),
    }

    Ok(())
}
