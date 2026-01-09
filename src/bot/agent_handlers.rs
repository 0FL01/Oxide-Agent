//! Agent mode handlers for Telegram bot
//!
//! Provides handlers for activating agent mode, processing messages,
//! and managing agent sessions.

use crate::agent::{
    executor::AgentExecutor,
    loop_detection::LoopType,
    preprocessor::Preprocessor,
    progress::{AgentEvent, ProgressState},
    AgentSession, TelegramSessionRegistry,
};
use crate::bot::agent::extract_agent_input;
use crate::bot::messaging::send_long_message;
use crate::bot::state::State;
use crate::bot::views::{
    get_agent_keyboard, loop_action_keyboard, loop_type_label, wipe_confirmation_keyboard,
    AgentView, DefaultAgentView, LOOP_CALLBACK_CANCEL, LOOP_CALLBACK_RESET, LOOP_CALLBACK_RETRY,
};
use crate::config::AGENT_MAX_ITERATIONS;
use crate::llm::LlmClient;
use crate::storage::R2Storage;
use anyhow::Result;
use std::sync::Arc;
use std::sync::LazyLock;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, InputFile, MessageId, ParseMode};
use tracing::{debug, error, info, warn};

/// Type alias for dialogue
pub type AgentDialogue = Dialogue<State, InMemStorage<State>>;

/// Context for running an agent task without blocking the update handler
struct AgentTaskContext {
    bot: Bot,
    msg: Message,
    storage: Arc<R2Storage>,
    llm: Arc<LlmClient>,
}

/// Global session registry for agent executors
static SESSION_REGISTRY: LazyLock<TelegramSessionRegistry> =
    LazyLock::new(TelegramSessionRegistry::new);

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
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let chat_id = msg.chat.id.0;

    info!("Activating agent mode for user {user_id}");

    // Create new session
    let mut session = AgentSession::new(user_id, chat_id);

    // Load saved agent memory if exists
    if let Ok(Some(saved_memory)) = storage.load_agent_memory(user_id).await {
        session.memory = saved_memory;
        info!("Loaded agent memory for user {user_id}");
    }

    let executor = AgentExecutor::new(llm.clone(), session);

    // Store session in registry
    SESSION_REGISTRY.insert(user_id, executor).await;

    // Save state to DB
    storage
        .update_user_state(user_id, "agent_mode".to_string())
        .await?;

    // Update dialogue state
    dialogue.update(State::AgentMode).await?;

    // Send welcome message
    bot.send_message(msg.chat.id, DefaultAgentView::welcome_message())
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
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let chat_id = msg.chat.id;

    // Check for control commands
    if let Some(text) = msg.text() {
        match text {
            "❌ Отменить задачу" => {
                return cancel_agent_task(bot, msg, dialogue).await;
            }
            "🗑 Очистить память" => {
                return clear_agent_memory(bot, msg, storage).await;
            }
            "🔄 Пересоздать контейнер" => {
                return confirm_agent_wipe(bot, msg, dialogue).await;
            }
            "⬅️ Выйти из режима агента" => {
                return exit_agent_mode(bot, msg, dialogue, storage).await;
            }
            _ => {}
        }
    }

    // Get or create session
    ensure_session_exists(user_id, chat_id.0, &llm, &storage).await;

    if is_agent_task_running(user_id).await {
        bot.send_message(
            chat_id,
            "⏳ Задача уже выполняется. Нажмите ❌ Отменить задачу, если нужно прекратить.",
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
                .send_message(task_msg.chat.id, format!("❌ Ошибка: {e}"))
                .await;
        }
    });

    Ok(())
}

async fn ensure_session_exists(
    user_id: i64,
    chat_id: i64,
    llm: &Arc<LlmClient>,
    storage: &Arc<R2Storage>,
) {
    if SESSION_REGISTRY.contains(&user_id).await {
        debug!(user_id = user_id, "Session already exists in cache");
        return;
    }

    let mut session = AgentSession::new(user_id, chat_id);

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

    let executor = AgentExecutor::new(llm.clone(), session);
    SESSION_REGISTRY.insert(user_id, executor).await;
}

async fn is_agent_task_running(user_id: i64) -> bool {
    SESSION_REGISTRY.is_running(&user_id).await
}

async fn renew_cancellation_token(user_id: i64) {
    SESSION_REGISTRY.renew_cancellation_token(&user_id).await;
}

async fn save_memory_after_task(user_id: i64, storage: &Arc<R2Storage>) {
    if let Some(executor_arc) = SESSION_REGISTRY.get(&user_id).await {
        let executor = executor_arc.read().await;
        let _ = storage
            .save_agent_memory(user_id, &executor.session().memory)
            .await;
    }
}

/// Spawn task to handle progress updates and file delivery
fn spawn_progress_updater(
    bot: Bot,
    chat_id: ChatId,
    msg_id: MessageId,
    mut rx: tokio::sync::mpsc::Receiver<AgentEvent>,
) -> tokio::task::JoinHandle<String> {
    tokio::spawn(async move {
        let mut state = ProgressState::new(AGENT_MAX_ITERATIONS);
        let mut last_update = std::time::Instant::now();
        let mut needs_update = false;
        let throttle_duration = std::time::Duration::from_millis(1500);

        while let Some(event) = rx.recv().await {
            // Handle file sending separately (side effect)
            match &event {
                AgentEvent::FileToSend {
                    ref file_name,
                    ref content,
                } => {
                    let input_file =
                        InputFile::memory(content.clone()).file_name(file_name.clone());
                    if let Err(e) = bot.send_document(chat_id, input_file).await {
                        tracing::error!("Failed to send file {}: {}", file_name, e);
                    }
                }
                AgentEvent::FileToSendWithConfirmation {
                    file_name: _,
                    content: _,
                    sandbox_path: _,
                    ..
                } => {
                    // Extract the confirmation channel
                    // We need to destructure the event to move confirmation_tx out
                    if let AgentEvent::FileToSendWithConfirmation {
                        file_name: fname,
                        content: fcontent,
                        sandbox_path: spath,
                        confirmation_tx,
                    } = event
                    {
                        // Retry logic with exponential backoff
                        let result = crate::utils::retry_telegram_operation(|| async {
                            let input_file =
                                InputFile::memory(fcontent.clone()).file_name(fname.clone());
                            bot.send_document(chat_id, input_file)
                                .await
                                .map_err(|e| anyhow::anyhow!("Telegram error: {e}"))
                        })
                        .await;

                        match result {
                            Ok(_) => {
                                info!(file_name = %fname, sandbox_path = %spath, "File delivered successfully");
                                let _ = confirmation_tx.send(Ok(()));
                            }
                            Err(e) => {
                                error!(file_name = %fname, error = %e, "Failed to deliver file after retries");
                                let _ = confirmation_tx.send(Err(e.to_string()));
                            }
                        }
                        // Don't update state for this variant, we already handled it
                        needs_update = true;
                        continue;
                    }
                }
                AgentEvent::LoopDetected {
                    loop_type,
                    iteration,
                } => {
                    if let Err(e) =
                        send_loop_detected_message(&bot, chat_id, *loop_type, *iteration).await
                    {
                        warn!("Failed to send loop notification: {e}");
                    }
                }
                _ => {}
            }

            state.update(event);
            needs_update = true;

            if last_update.elapsed() >= throttle_duration {
                let text = state.format_telegram();
                super::resilient::edit_message_safe_resilient(&bot, chat_id, msg_id, &text).await;
                last_update = std::time::Instant::now();
                needs_update = false;
            }
        }

        let final_text = state.format_telegram();
        if needs_update {
            super::resilient::edit_message_safe_resilient(&bot, chat_id, msg_id, &final_text).await;
        }
        final_text
    })
}

async fn send_loop_detected_message(
    bot: &Bot,
    chat_id: ChatId,
    loop_type: LoopType,
    iteration: usize,
) -> Result<()> {
    let text = format!(
        "🔁 <b>Обнаружена петля в выполнении задачи</b>\nТип: {}\nИтерация: {}\n\nВыберите действие:",
        loop_type_label(loop_type),
        iteration
    );

    bot.send_message(chat_id, text)
        .parse_mode(ParseMode::Html)
        .reply_markup(loop_action_keyboard())
        .await?;

    Ok(())
}

async fn run_agent_task(ctx: AgentTaskContext) -> Result<()> {
    let user_id = ctx.msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let chat_id = ctx.msg.chat.id;

    // Preprocess input
    let preprocessor = Preprocessor::new(ctx.llm.clone(), user_id);
    let input = extract_agent_input(&ctx.bot, &ctx.msg).await?;
    let task_text = preprocessor.preprocess_input(input).await?;
    info!(
        user_id = user_id,
        chat_id = chat_id.0,
        "Input preprocessed, task text extracted"
    );

    // Send initial progress message with retry on network failures
    let progress_msg = super::resilient::send_message_resilient(
        &ctx.bot,
        chat_id,
        "⏳ Обработка задачи...",
        Some(ParseMode::Html),
    )
    .await?;

    // Create progress tracking channel
    let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(100);

    // Spawn progress updater task
    let progress_handle = spawn_progress_updater(ctx.bot.clone(), chat_id, progress_msg.id, rx);

    // Execute the task
    let result = execute_agent_task(user_id, &task_text, Some(tx)).await;
    let progress_text = progress_handle.await.unwrap_or_default();

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
            let sanitized_error = crate::utils::sanitize_html_error(&e.to_string());
            let error_text = format!("{progress_text}\n\n❌ <b>Ошибка:</b>\n\n{sanitized_error}");
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
        "⏳ Обработка задачи...",
        Some(ParseMode::Html),
    )
    .await?;

    let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(100);
    let progress_handle = spawn_progress_updater(bot.clone(), chat_id, progress_msg.id, rx);

    let result = execute_agent_task(user_id, &task_text, Some(tx)).await;
    let progress_text = progress_handle.await.unwrap_or_default();

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
            let sanitized_error = crate::utils::sanitize_html_error(&e.to_string());
            let error_text = format!("{progress_text}\n\n❌ <b>Ошибка:</b>\n\n{sanitized_error}");
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
    // Get executor from registry
    let executor_arc = SESSION_REGISTRY
        .get(&user_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("No agent session found"))?;

    // Get the cancellation token for this task
    let cancellation_token = SESSION_REGISTRY
        .get_cancellation_token(&user_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("No cancellation token found"))?;

    // Acquire write lock on the executor
    let mut executor = executor_arc.write().await;

    debug!(
        user_id = user_id,
        memory_messages = executor.session().memory.get_messages().len(),
        "Executor accessed for task execution"
    );

    // Check timeout
    if executor.is_timed_out() {
        executor.reset();
        return Err(anyhow::anyhow!(
            "Предыдущая сессия истекла по таймауту. Начинаю новую сессию."
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

            ensure_session_exists(user_id, chat_id.0, &llm, &storage).await;
            renew_cancellation_token(user_id).await;

            let executor_arc = SESSION_REGISTRY.get(&user_id).await;

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
        LOOP_CALLBACK_RESET => match SESSION_REGISTRY.reset(&user_id).await {
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
        },
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
    let cancelled = SESSION_REGISTRY.cancel(&user_id).await;

    // Best-effort: clear todos without waiting for executor locks.
    let cleared_todos = SESSION_REGISTRY.clear_todos(&user_id).await;

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
    let cancelled = SESSION_REGISTRY.cancel(&user_id).await;
    let cleared_todos = SESSION_REGISTRY.clear_todos(&user_id).await;

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

/// Clear agent memory
///
/// # Errors
///
/// Returns an error if the confirmation message cannot be sent.
pub async fn clear_agent_memory(bot: Bot, msg: Message, storage: Arc<R2Storage>) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    info!(user_id = user_id, "User requested memory clear via button");

    match SESSION_REGISTRY.reset(&user_id).await {
        Ok(()) => {
            let _ = storage.clear_agent_memory(user_id).await;
            bot.send_message(msg.chat.id, DefaultAgentView::memory_cleared())
                .reply_markup(get_agent_keyboard())
                .await?;
        }
        Err("Cannot reset while task is running") => {
            bot.send_message(msg.chat.id, DefaultAgentView::clear_blocked_by_task())
                .reply_markup(get_agent_keyboard())
                .await?;
        }
        Err(_) => {
            // No session — just clear storage
            let _ = storage.clear_agent_memory(user_id).await;
            bot.send_message(msg.chat.id, DefaultAgentView::memory_cleared())
                .reply_markup(get_agent_keyboard())
                .await?;
        }
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
    SESSION_REGISTRY.remove(&user_id).await;

    let _ = storage
        .update_user_state(user_id, "chat_mode".to_string())
        .await;
    dialogue.update(State::Start).await?;

    let keyboard = crate::bot::handlers::get_main_keyboard();
    bot.send_message(
        msg.chat.id,
        "👋 Вышли из режима агента. Выберите режим работы:",
    )
    .reply_markup(keyboard)
    .await?;
    Ok(())
}

/// Ask for confirmation to recreate container
///
/// # Errors
///
/// Returns an error if the confirmation message cannot be sent.
pub async fn confirm_agent_wipe(bot: Bot, msg: Message, dialogue: AgentDialogue) -> Result<()> {
    dialogue.update(State::AgentWipeConfirmation).await?;
    bot.send_message(msg.chat.id, DefaultAgentView::wipe_confirmation())
        .parse_mode(ParseMode::Html)
        .reply_markup(wipe_confirmation_keyboard())
        .await?;
    Ok(())
}

/// Handle confirmation for wiping agent container
///
/// # Errors
///
/// Returns an error if the container cannot be recreated or message cannot be sent.
pub async fn handle_agent_wipe_confirmation(
    bot: Bot,
    msg: Message,
    dialogue: AgentDialogue,
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let text = msg.text().unwrap_or("");
    let chat_id = msg.chat.id;

    if text != "✅ Да" && text != "❌ Отмена" {
        bot.send_message(chat_id, DefaultAgentView::select_keyboard_option())
            .await?;
        return Ok(());
    }

    dialogue.update(State::AgentMode).await?;
    let keyboard = get_agent_keyboard();

    match text {
        "✅ Да" => {
            if let Some(executor_arc) = SESSION_REGISTRY.get(&user_id).await {
                let mut executor = executor_arc.write().await;
                match executor.session_mut().ensure_sandbox().await {
                    Ok(sandbox) => {
                        if let Err(e) = sandbox.recreate().await {
                            bot.send_message(chat_id, format!("Ошибка при пересоздании: {e}"))
                                .reply_markup(keyboard)
                                .await?;
                        } else {
                            bot.send_message(chat_id, DefaultAgentView::container_recreated())
                                .reply_markup(keyboard)
                                .await?;
                        }
                    }
                    Err(_) => {
                        bot.send_message(chat_id, DefaultAgentView::sandbox_access_error())
                            .reply_markup(keyboard)
                            .await?;
                    }
                }
            } else {
                // If for some reason session is gone, we just show ready to work
                // or session not found. Behaving safely by just showing the keyboard with a generic message
                // or just the ready message if we really want to recover.
                // But the user specifically wanted to remove "Ready to work" AFTER success.
                // Here we are in a weird state. Let's just say session not found to be safe/correct.
                bot.send_message(chat_id, DefaultAgentView::session_not_found())
                    .reply_markup(keyboard)
                    .await?;
            }
        }
        "❌ Отмена" => {
            bot.send_message(chat_id, DefaultAgentView::operation_cancelled())
                .reply_markup(keyboard)
                .await?;
        }
        _ => unreachable!(),
    }

    Ok(())
}
