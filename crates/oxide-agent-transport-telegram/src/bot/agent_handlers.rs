//! Agent mode handlers for Telegram bot
//!
//! Provides handlers for activating agent mode, processing messages,
//! and managing agent sessions.

use crate::bot::agent::extract_agent_input;
use crate::bot::agent_transport::TelegramAgentTransport;
use crate::bot::context::TelegramHandlerContext;
use crate::bot::messaging::send_long_message;
use crate::bot::progress_render::render_progress_html;
use crate::bot::state::{ConfirmationType, State};
use crate::bot::views::{
    confirmation_keyboard, get_agent_keyboard, AgentView, DefaultAgentView, LOOP_CALLBACK_CANCEL,
    LOOP_CALLBACK_RESET, LOOP_CALLBACK_RETRY,
};
use crate::config::BotSettings;
use anyhow::{Error, Result};
use async_trait::async_trait;
use oxide_agent_core::agent::{
    executor::{AgentExecutionOutcome, AgentExecutor},
    preprocessor::Preprocessor,
    progress::{AgentEvent, ProgressState},
    AgentMemory, AgentSession, PendingChoiceInput, PendingInputKind, PendingTextInput, SessionId,
    TaskId, TaskMetadata, TaskSnapshot, TaskState,
};
use oxide_agent_core::config::AGENT_MAX_ITERATIONS;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::storage::{PendingInputPoll, StorageError, StorageProvider};
use oxide_agent_runtime::{
    spawn_progress_runtime, CancellationToken, DetachedTaskSubmission, ProgressRuntimeConfig,
    SessionRegistry, TaskExecutionBackend, TaskExecutionOutcome, TaskExecutionRequest,
    TaskExecutor, TaskExecutorError, TaskExecutorOptions, TaskRecord, TaskRegistry, WorkerManager,
};
use std::future::Future;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, InputPollOption, MessageId, ParseMode, PollAnswer};
use tokio::task::yield_now;
use tracing::{debug, info, warn};

const TELEGRAM_POLL_MIN_OPTIONS: usize = 2;
const TELEGRAM_POLL_MAX_OPTIONS: usize = 10;

/// Type alias for dialogue
pub type AgentDialogue = Dialogue<State, InMemStorage<State>>;

enum AgentWipeError {
    Recreate(Error),
}

/// Shared runtime-owned task path for Telegram agent handlers.
#[derive(Clone)]
pub struct AgentTaskRuntime {
    task_registry: Arc<TaskRegistry>,
    task_executor: Arc<TaskExecutor>,
}

enum SessionResetOutcome {
    Reset,
    SessionNotFound,
    Busy,
}

pub(crate) enum StartResetOutcome<T> {
    Reset(T),
    BlockedByTask,
}

enum ExitSessionOutcome {
    Exited,
    BlockedByTask,
}

enum ClearMemoryOutcome {
    Cleared,
    BlockedByTask,
}

enum ClearTodosOutcome {
    Cleared,
    NotCleared,
}

enum RetryTaskOutcome {
    Submitted,
    NoSavedTask,
    SessionNotFound,
}

enum AgentModeActivationOutcome {
    Activated,
    LiveTaskStillRunning,
}

enum RecreateContainerOutcome {
    Recreated,
    RecreateFailed(Error),
    BlockedByTask,
    SessionAccessError,
}

enum AgentControlCommand {
    CancelTask,
    ClearMemory,
    RecreateContainer,
    ExitAgentMode,
}

/// Inputs required to switch a Telegram user into agent mode.
pub(crate) struct ActivateAgentModeParams {
    /// Telegram bot handle used for reply delivery.
    pub bot: Bot,
    /// Incoming Telegram message that triggered the mode switch.
    pub msg: Message,
    /// Dialogue storage handle for state transitions.
    pub dialogue: AgentDialogue,
    /// Shared runtime and dependency bundle for Telegram handlers.
    pub context: Arc<TelegramHandlerContext>,
}

fn parse_agent_control_command(text: &str) -> Option<AgentControlCommand> {
    match text {
        "❌ Cancel Task" => Some(AgentControlCommand::CancelTask),
        "🗑 Clear Memory" => Some(AgentControlCommand::ClearMemory),
        "🔄 Recreate Container" => Some(AgentControlCommand::RecreateContainer),
        "⬅️ Exit Agent Mode" => Some(AgentControlCommand::ExitAgentMode),
        _ => None,
    }
}

async fn handle_agent_control_command(
    command: AgentControlCommand,
    bot: Bot,
    msg: Message,
    dialogue: AgentDialogue,
    context: Arc<TelegramHandlerContext>,
) -> Result<()> {
    match command {
        AgentControlCommand::CancelTask => {
            cancel_agent_task(bot, msg, dialogue, Arc::clone(&context.task_runtime)).await
        }
        AgentControlCommand::ClearMemory => {
            confirm_destructive_action(
                ConfirmationType::ClearMemory,
                bot,
                msg,
                dialogue,
                Arc::clone(&context.task_runtime),
            )
            .await
        }
        AgentControlCommand::RecreateContainer => {
            confirm_destructive_action(
                ConfirmationType::RecreateContainer,
                bot,
                msg,
                dialogue,
                Arc::clone(&context.task_runtime),
            )
            .await
        }
        AgentControlCommand::ExitAgentMode => {
            exit_agent_mode(
                bot,
                msg,
                dialogue,
                Arc::clone(&context.storage),
                Arc::clone(&context.task_runtime),
            )
            .await
        }
    }
}

impl AgentTaskRuntime {
    /// Build the live Telegram task runtime on top of the shared runtime registry.
    #[must_use]
    pub fn new(
        storage: Arc<dyn StorageProvider>,
        task_registry: Arc<TaskRegistry>,
        max_workers: usize,
    ) -> Self {
        let worker_manager = Arc::new(WorkerManager::new(max_workers));
        let task_executor = Arc::new(TaskExecutor::new(TaskExecutorOptions {
            task_registry: Arc::clone(&task_registry),
            worker_manager: Arc::clone(&worker_manager),
            storage,
        }));

        Self {
            task_registry,
            task_executor,
        }
    }

    /// Return the latest live runtime task for a session, if present.
    pub async fn active_task_for_session(&self, session_id: SessionId) -> Option<TaskRecord> {
        self.task_registry
            .latest_non_terminal_by_session(&session_id)
            .await
    }

    pub(crate) async fn has_active_task_for_session(&self, session_id: SessionId) -> bool {
        self.active_task_for_session(session_id).await.is_some()
    }

    pub(crate) async fn submit_task<B>(
        &self,
        session_id: SessionId,
        task: String,
        backend: Arc<B>,
    ) -> Result<TaskRecord, TaskExecutorError>
    where
        B: TaskExecutionBackend,
    {
        self.task_executor
            .submit(DetachedTaskSubmission { session_id, task }, backend)
            .await
    }

    pub(crate) async fn resume_task<B>(
        &self,
        task_id: &TaskId,
        input: String,
        backend: Arc<B>,
    ) -> Result<TaskRecord, TaskExecutorError>
    where
        B: TaskExecutionBackend,
    {
        let Some(record) = self.task_registry.get(task_id).await else {
            return Err(TaskExecutorError::TaskRegistry(
                oxide_agent_runtime::TaskRegistryError::TaskNotFound(*task_id),
            ));
        };

        self.with_session_gate(record.session_id, || async move {
            self.task_executor
                .resume_task_with_session_gate_held(task_id, input, backend)
                .await
        })
        .await
    }

    async fn ensure_session_exists_inner(
        &self,
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

    #[cfg(test)]
    pub(crate) async fn blocks_start_reset(&self, session_id: SessionId) -> bool {
        self.with_session_gate(session_id, || async move {
            self.has_active_task_for_session(session_id).await
        })
        .await
    }

    pub(crate) async fn reset_start_if_idle<F, Fut, T, E>(
        &self,
        session_id: SessionId,
        reset: F,
    ) -> Result<StartResetOutcome<T>, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        self.with_session_gate(session_id, || async move {
            if self.has_active_task_for_session(session_id).await {
                Ok(StartResetOutcome::BlockedByTask)
            } else {
                reset().await.map(StartResetOutcome::Reset)
            }
        })
        .await
    }

    async fn activate_agent_mode_session(
        &self,
        session_id: SessionId,
        user_id: i64,
        llm: &Arc<LlmClient>,
        storage: &Arc<dyn StorageProvider>,
        settings: &Arc<BotSettings>,
    ) -> AgentModeActivationOutcome {
        self.with_session_gate(session_id, || async move {
            if self.has_active_task_for_session(session_id).await {
                return AgentModeActivationOutcome::LiveTaskStillRunning;
            }

            let mut session = AgentSession::new(session_id);

            if let Ok(Some(saved_memory)) = storage.load_agent_memory(user_id).await {
                session.memory = saved_memory;
                info!("Loaded agent memory for user {user_id}");
            }

            let executor = AgentExecutor::new(Arc::clone(llm), session, settings.agent.clone());
            SESSION_REGISTRY.insert(session_id, executor).await;

            AgentModeActivationOutcome::Activated
        })
        .await
    }

    async fn ensure_session_exists(
        &self,
        user_id: i64,
        llm: &Arc<LlmClient>,
        storage: &Arc<dyn StorageProvider>,
        settings: &Arc<BotSettings>,
    ) {
        let session_id = SessionId::from(user_id);

        self.with_session_gate(session_id, || async move {
            self.ensure_session_exists_inner(session_id, user_id, llm, storage, settings)
                .await;
        })
        .await;
    }

    async fn save_memory_after_task_inner(
        &self,
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

    async fn save_memory_after_task(
        &self,
        session_id: SessionId,
        user_id: i64,
        storage: &Arc<dyn StorageProvider>,
    ) {
        self.with_session_gate(session_id, || async move {
            self.save_memory_after_task_inner(session_id, user_id, storage)
                .await;
        })
        .await;
    }

    async fn retry_task_without_loop_detection<B>(
        &self,
        user_id: i64,
        llm: &Arc<LlmClient>,
        storage: &Arc<dyn StorageProvider>,
        settings: &Arc<BotSettings>,
        backend: Arc<B>,
    ) -> Result<RetryTaskOutcome, TaskExecutorError>
    where
        B: TaskExecutionBackend,
    {
        let session_id = SessionId::from(user_id);

        self.with_session_gate(session_id, || async move {
            self.ensure_session_exists_inner(session_id, user_id, llm, storage, settings)
                .await;

            let task_text = match SESSION_REGISTRY
                .with_executor_mut(&session_id, |executor| {
                    Box::pin(async move {
                        let task_text = executor.last_task().map(str::to_string);
                        if task_text.is_some() {
                            executor.disable_loop_detection_next_run();
                        }
                        task_text
                    })
                })
                .await
            {
                Ok(task_text) => task_text,
                Err(_) => return Ok(RetryTaskOutcome::SessionNotFound),
            };

            let Some(task_text) = task_text else {
                return Ok(RetryTaskOutcome::NoSavedTask);
            };

            self.task_executor
                .submit_with_session_gate_held(
                    DetachedTaskSubmission {
                        session_id,
                        task: task_text,
                    },
                    backend,
                )
                .await?;

            Ok(RetryTaskOutcome::Submitted)
        })
        .await
    }

    async fn with_session_gate<F, Fut, T>(&self, session_id: SessionId, action: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        self.task_executor
            .with_session_gate(session_id, action)
            .await
    }

    pub(crate) async fn cancel_task_for_session(
        &self,
        session_id: SessionId,
    ) -> Result<Option<TaskRecord>, TaskExecutorError> {
        self.with_session_gate(session_id, || async move {
            self.cancel_task_for_session_inner(session_id).await
        })
        .await
    }

    async fn cancel_task_for_session_inner(
        &self,
        session_id: SessionId,
    ) -> Result<Option<TaskRecord>, TaskExecutorError> {
        let Some(record) = self
            .task_registry
            .latest_non_terminal_by_session(&session_id)
            .await
        else {
            return Ok(None);
        };

        self.task_executor
            .cancel_task(&record.metadata.id)
            .await
            .map(Some)
    }

    async fn cancel_and_reset_session(
        &self,
        session_id: SessionId,
    ) -> Result<SessionResetOutcome, TaskExecutorError> {
        self.with_session_gate(session_id, || async {
            let _ = self.cancel_task_for_session_inner(session_id).await?;
            let deadline = Instant::now() + Duration::from_secs(1);

            loop {
                if self.has_active_task_for_session(session_id).await {
                    if Instant::now() >= deadline {
                        return Ok(SessionResetOutcome::Busy);
                    }
                    yield_now().await;
                    continue;
                }

                match SESSION_REGISTRY.reset(&session_id).await {
                    Ok(()) => return Ok(SessionResetOutcome::Reset),
                    Err("Session not found") => return Ok(SessionResetOutcome::SessionNotFound),
                    Err("Cannot reset while task is running") => {
                        if Instant::now() >= deadline {
                            return Ok(SessionResetOutcome::Busy);
                        }
                        yield_now().await;
                    }
                    Err(_) => return Ok(SessionResetOutcome::Busy),
                }
            }
        })
        .await
    }

    async fn exit_session(
        &self,
        session_id: SessionId,
        user_id: i64,
        storage: &Arc<dyn StorageProvider>,
    ) -> ExitSessionOutcome {
        self.with_session_gate(session_id, || async {
            if self.has_active_task_for_session(session_id).await {
                return ExitSessionOutcome::BlockedByTask;
            }

            self.save_memory_after_task_inner(session_id, user_id, storage)
                .await;
            SESSION_REGISTRY.remove(&session_id).await;
            ExitSessionOutcome::Exited
        })
        .await
    }

    async fn clear_todos(&self, session_id: SessionId) -> ClearTodosOutcome {
        self.with_session_gate(session_id, || async move {
            if SESSION_REGISTRY.clear_todos(&session_id).await {
                ClearTodosOutcome::Cleared
            } else {
                ClearTodosOutcome::NotCleared
            }
        })
        .await
    }

    async fn clear_memory(
        &self,
        session_id: SessionId,
        user_id: i64,
        storage: &Arc<dyn StorageProvider>,
    ) -> ClearMemoryOutcome {
        self.with_session_gate(session_id, || async {
            if self.has_active_task_for_session(session_id).await {
                return ClearMemoryOutcome::BlockedByTask;
            }

            let _ = SESSION_REGISTRY.reset(&session_id).await;
            let _ = storage.clear_agent_memory(user_id).await;
            ClearMemoryOutcome::Cleared
        })
        .await
    }

    async fn recreate_container(
        &self,
        session_id: SessionId,
        user_id: i64,
        context: &TelegramHandlerContext,
    ) -> RecreateContainerOutcome {
        self.ensure_session_exists(user_id, &context.llm, &context.storage, &context.settings)
            .await;

        self.with_session_gate(session_id, || async {
            if self.has_active_task_for_session(session_id).await {
                return RecreateContainerOutcome::BlockedByTask;
            }

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
                Ok(Ok(())) => RecreateContainerOutcome::Recreated,
                Ok(Err(AgentWipeError::Recreate(error))) => {
                    RecreateContainerOutcome::RecreateFailed(error)
                }
                Err("Cannot reset while task is running") => {
                    RecreateContainerOutcome::BlockedByTask
                }
                Err(_) => RecreateContainerOutcome::SessionAccessError,
            }
        })
        .await
    }
}

#[derive(Clone)]
struct TelegramTaskExecutionBackend {
    bot: Bot,
    chat_id: ChatId,
    storage: Arc<dyn StorageProvider>,
    task_runtime: Arc<AgentTaskRuntime>,
}

struct AgentExecutionInput {
    task_text: String,
    resume_input: Option<String>,
}

struct AgentExecutionResult {
    outcome: AgentExecutionOutcome,
    memory: AgentMemory,
}

struct RunAgentTaskRequest {
    bot: Bot,
    chat_id: ChatId,
    user_id: i64,
    execution: AgentExecutionInput,
    storage: Arc<dyn StorageProvider>,
    task_runtime: Arc<AgentTaskRuntime>,
    cancellation_token: Arc<CancellationToken>,
}

#[async_trait]
impl TaskExecutionBackend for TelegramTaskExecutionBackend {
    async fn execute(&self, request: TaskExecutionRequest) -> Result<TaskExecutionOutcome> {
        let TaskExecutionRequest {
            session_id,
            task,
            resume_input,
            cancellation_token,
            ..
        } = request;

        run_agent_task_with_text(RunAgentTaskRequest {
            bot: self.bot.clone(),
            chat_id: self.chat_id,
            user_id: session_id.as_i64(),
            execution: AgentExecutionInput {
                task_text: task,
                resume_input,
            },
            storage: Arc::clone(&self.storage),
            task_runtime: Arc::clone(&self.task_runtime),
            cancellation_token,
        })
        .await
    }
}

/// Global session registry for agent executors
static SESSION_REGISTRY: LazyLock<SessionRegistry> = LazyLock::new(SessionRegistry::new);

/// Activate agent mode for a user
///
/// # Errors
///
/// Returns an error if the user state cannot be updated or the welcome message cannot be sent.
pub(crate) async fn activate_agent_mode(params: ActivateAgentModeParams) -> Result<()> {
    let ActivateAgentModeParams {
        bot,
        msg,
        dialogue,
        context,
    } = params;
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let session_id = SessionId::from(user_id);

    info!("Activating agent mode for user {user_id}");

    let activation_outcome = context
        .task_runtime
        .activate_agent_mode_session(
            session_id,
            user_id,
            &context.llm,
            &context.storage,
            &context.settings,
        )
        .await;

    // Save state to DB
    context
        .storage
        .update_user_state(user_id, "agent_mode".to_string())
        .await?;

    // Update dialogue state
    dialogue.update(State::AgentMode).await?;

    match activation_outcome {
        AgentModeActivationOutcome::Activated => {
            let (model_id, _, _) = context.settings.agent.get_configured_agent_model();
            bot.send_message(msg.chat.id, DefaultAgentView::welcome_message(&model_id))
                .parse_mode(ParseMode::Html)
                .reply_markup(get_agent_keyboard())
                .await?;
        }
        AgentModeActivationOutcome::LiveTaskStillRunning => {
            if let Some(record) = context
                .task_runtime
                .active_task_for_session(session_id)
                .await
            {
                let delivered_poll = deliver_waiting_choice_poll_if_needed(
                    &bot,
                    msg.chat.id,
                    user_id,
                    context.as_ref(),
                    &record,
                )
                .await?;

                if delivered_poll {
                    bot.send_message(
                        msg.chat.id,
                        "⏳ Task is waiting for your response. Answer the active poll to continue.",
                    )
                    .reply_markup(get_agent_keyboard())
                    .await?;
                    return Ok(());
                }
            }

            bot.send_message(msg.chat.id, DefaultAgentView::task_already_running())
                .reply_markup(get_agent_keyboard())
                .await?;
        }
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
    dialogue: AgentDialogue,
    context: Arc<TelegramHandlerContext>,
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let chat_id = msg.chat.id;
    let session_id = SessionId::from(user_id);

    if let Some(command) = msg.text().and_then(parse_agent_control_command) {
        return handle_agent_control_command(command, bot, msg, dialogue, context).await;
    }

    // Get or create session
    ensure_session_exists(
        &context.task_runtime,
        user_id,
        &context.llm,
        &context.storage,
        &context.settings,
    )
    .await;

    if let Some(record) = context
        .task_runtime
        .active_task_for_session(session_id)
        .await
    {
        handle_active_task_message(&bot, &msg, context.as_ref(), user_id, chat_id, &record).await?;
        return Ok(());
    }

    let preprocessor = Preprocessor::new(Arc::clone(&context.llm), user_id);
    let input = extract_agent_input(&bot, &msg).await?;
    let task_text = match preprocessor.preprocess_input(input).await {
        Ok(text) => text,
        Err(err) => {
            if err.to_string() == "MULTIMODAL_DISABLED" {
                super::resilient::send_message_resilient(
                    &bot,
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

    submit_agent_task(
        &bot,
        chat_id,
        Arc::clone(&context.storage),
        Arc::clone(&context.task_runtime),
        session_id,
        task_text,
    )
    .await
}

async fn handle_active_task_message(
    bot: &Bot,
    msg: &Message,
    context: &TelegramHandlerContext,
    user_id: i64,
    chat_id: ChatId,
    record: &TaskRecord,
) -> Result<()> {
    if record.metadata.state == TaskState::WaitingInput {
        if let Some(pending_input) = record.pending_input.as_ref() {
            if let PendingInputKind::Text(pending_text) = &pending_input.kind {
                let Some(input_text) = msg.text() else {
                    bot.send_message(
                        chat_id,
                        "⏳ Task is waiting for text input. Send your response as a text message to continue.",
                    )
                    .reply_markup(get_agent_keyboard())
                    .await?;
                    return Ok(());
                };

                if let Some(validation_error) =
                    validate_pending_text_resume_input(input_text, pending_text)
                {
                    bot.send_message(chat_id, validation_error)
                        .reply_markup(get_agent_keyboard())
                        .await?;
                    return Ok(());
                }

                let resumed = resume_waiting_task_input(
                    bot,
                    context,
                    ResumeTaskInput {
                        user_id,
                        chat_id,
                        task_id: &record.metadata.id,
                        input: input_text.to_string(),
                    },
                )
                .await?;

                if resumed {
                    bot.send_message(chat_id, "✅ Input accepted. Continuing task...")
                        .reply_markup(get_agent_keyboard())
                        .await?;
                } else {
                    bot.send_message(chat_id, DefaultAgentView::task_already_running())
                        .reply_markup(get_agent_keyboard())
                        .await?;
                }

                return Ok(());
            }
        }
    }

    let delivered_poll =
        deliver_waiting_choice_poll_if_needed(bot, chat_id, user_id, context, record).await?;

    if delivered_poll {
        bot.send_message(
            chat_id,
            "⏳ Task is waiting for your response. Answer the active poll to continue.",
        )
        .reply_markup(get_agent_keyboard())
        .await?;
    } else {
        bot.send_message(
            chat_id,
            "⏳ A task is already running. Press ❌ Cancel Task to stop it.",
        )
        .reply_markup(get_agent_keyboard())
        .await?;
    }

    Ok(())
}

async fn ensure_session_exists(
    task_runtime: &AgentTaskRuntime,
    user_id: i64,
    llm: &Arc<LlmClient>,
    storage: &Arc<dyn StorageProvider>,
    settings: &Arc<BotSettings>,
) {
    task_runtime
        .ensure_session_exists(user_id, llm, storage, settings)
        .await;
}

struct ResumeTaskInput<'a> {
    user_id: i64,
    chat_id: ChatId,
    task_id: &'a TaskId,
    input: String,
}

#[cfg(test)]
async fn exit_block_message(
    task_runtime: &AgentTaskRuntime,
    session_id: SessionId,
) -> Option<&'static str> {
    if task_runtime.has_active_task_for_session(session_id).await {
        return Some(DefaultAgentView::exit_blocked_by_task());
    }

    None
}

async fn destructive_action_block_message(
    task_runtime: &AgentTaskRuntime,
    session_id: SessionId,
    action: &ConfirmationType,
) -> Option<&'static str> {
    if !task_runtime.has_active_task_for_session(session_id).await {
        return None;
    }

    Some(match action {
        ConfirmationType::ClearMemory => DefaultAgentView::clear_blocked_by_task(),
        ConfirmationType::RecreateContainer => {
            DefaultAgentView::container_recreate_blocked_by_task()
        }
    })
}

fn is_valid_poll_answer(option_ids: &[u8], choice: &PendingChoiceInput) -> bool {
    let selection_count = option_ids.len();
    if selection_count == 0 {
        return false;
    }

    if selection_count < usize::from(choice.min_choices)
        || selection_count > usize::from(choice.max_choices)
    {
        return false;
    }

    if !choice.allow_multiple && selection_count != 1 {
        return false;
    }

    let mut seen = vec![false; choice.options.len()];
    for option_id in option_ids {
        let index = usize::from(*option_id);
        if index >= choice.options.len() || seen[index] {
            return false;
        }
        seen[index] = true;
    }

    true
}

fn encode_poll_resume_input(option_ids: &[u8]) -> String {
    option_ids
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn validate_pending_text_resume_input(
    input: &str,
    pending_text: &PendingTextInput,
) -> Option<String> {
    if !pending_text.multiline && (input.contains('\n') || input.contains('\r')) {
        return Some("⚠️ This response must be a single line.".to_string());
    }

    let input_len = input.len();
    if let Some(min_length) = pending_text.min_length {
        if input_len < usize::from(min_length) {
            return Some(format!(
                "⚠️ Response is too short (minimum {min_length} bytes)."
            ));
        }
    }

    if let Some(max_length) = pending_text.max_length {
        if input_len > usize::from(max_length) {
            return Some(format!(
                "⚠️ Response is too long (maximum {max_length} bytes)."
            ));
        }
    }

    None
}

async fn resume_waiting_task_input(
    bot: &Bot,
    context: &TelegramHandlerContext,
    resume: ResumeTaskInput<'_>,
) -> Result<bool> {
    let backend = Arc::new(TelegramTaskExecutionBackend {
        bot: bot.clone(),
        chat_id: resume.chat_id,
        storage: Arc::clone(&context.storage),
        task_runtime: Arc::clone(&context.task_runtime),
    });

    resume_waiting_task_input_with_backend(context, resume, backend).await
}

async fn resume_waiting_task_input_with_backend<B>(
    context: &TelegramHandlerContext,
    resume: ResumeTaskInput<'_>,
    backend: Arc<B>,
) -> Result<bool>
where
    B: TaskExecutionBackend,
{
    ensure_session_exists(
        &context.task_runtime,
        resume.user_id,
        &context.llm,
        &context.storage,
        &context.settings,
    )
    .await;

    if let Err(error) = restore_waiting_task_memory(context, resume.user_id, resume.task_id).await {
        warn!(task_id = %resume.task_id, error = %error, "Refusing resume: waiting snapshot pause context is invalid");
        return Ok(false);
    }

    match context
        .task_runtime
        .resume_task(resume.task_id, resume.input, backend)
        .await
    {
        Ok(_) => Ok(true),
        Err(error) => {
            warn!(task_id = %resume.task_id, error = %error, "Failed to resume waiting task input");
            Ok(false)
        }
    }
}

async fn restore_waiting_task_memory(
    context: &TelegramHandlerContext,
    user_id: i64,
    task_id: &TaskId,
) -> Result<()> {
    let snapshot = context
        .storage
        .load_task_snapshot(*task_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("missing waiting task snapshot for resume"))?;

    if snapshot.metadata.state != TaskState::WaitingInput {
        return Ok(());
    }

    if snapshot.pending_input.is_none() {
        return Err(anyhow::anyhow!(
            "waiting task snapshot missing pending input payload"
        ));
    }

    let Some(memory) = snapshot.parse_agent_memory()? else {
        return Err(anyhow::anyhow!(
            "waiting task snapshot missing pause memory payload"
        ));
    };

    let session_id = SessionId::from(user_id);
    let apply_result = SESSION_REGISTRY
        .with_executor_mut(&session_id, move |executor| {
            Box::pin(async move {
                executor.session_mut().memory = memory;
            })
        })
        .await;

    apply_result.map_err(|error| {
        anyhow::anyhow!("failed to restore waiting-task memory into session {session_id}: {error}")
    })?;

    Ok(())
}

async fn resume_task_from_consumed_poll_answer(
    bot: &Bot,
    context: &TelegramHandlerContext,
    pending_poll: &PendingInputPoll,
    option_ids: &[u8],
) -> Result<ConsumedPollResumeOutcome> {
    let resumed = resume_waiting_task_input(
        bot,
        context,
        ResumeTaskInput {
            user_id: pending_poll.owner_user_id,
            chat_id: ChatId(pending_poll.chat_id),
            task_id: &pending_poll.task_id,
            input: encode_poll_resume_input(option_ids),
        },
    )
    .await?;

    if resumed {
        match context
            .storage
            .delete_pending_input_poll(pending_poll.task_id, &pending_poll.poll_id)
            .await
        {
            Ok(()) => Ok(ConsumedPollResumeOutcome::Resumed),
            Err(StorageError::Unsupported(_)) => Ok(ConsumedPollResumeOutcome::Resumed),
            Err(error) => Err(error.into()),
        }
    } else {
        Ok(ConsumedPollResumeOutcome::Deferred)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsumedPollResumeOutcome {
    Resumed,
    Deferred,
}

async fn deliver_waiting_choice_poll_if_needed(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    context: &TelegramHandlerContext,
    record: &TaskRecord,
) -> Result<bool> {
    if record.metadata.state != TaskState::WaitingInput {
        return Ok(false);
    }

    let Some(pending_input) = record.pending_input.as_ref() else {
        return Ok(false);
    };

    let PendingInputKind::Choice(choice) = &pending_input.kind else {
        return Ok(false);
    };

    if let Some(existing_poll) = context
        .storage
        .load_pending_input_poll_by_task(record.metadata.id)
        .await?
    {
        if existing_poll.request_id == pending_input.request_id {
            if existing_poll.answered {
                if existing_poll.selected_option_ids.is_empty() {
                    return Ok(false);
                }

                resume_task_from_consumed_poll_answer(
                    bot,
                    context,
                    &existing_poll,
                    &existing_poll.selected_option_ids,
                )
                .await?;
            }

            return Ok(true);
        }
    }

    if !(TELEGRAM_POLL_MIN_OPTIONS..=TELEGRAM_POLL_MAX_OPTIONS).contains(&choice.options.len()) {
        warn!(
            task_id = %record.metadata.id,
            options = choice.options.len(),
            "Pending choice input cannot be delivered as Telegram poll"
        );
        return Ok(false);
    }

    let poll_message = bot
        .send_poll(
            chat_id,
            pending_input.prompt.clone(),
            choice
                .options
                .iter()
                .cloned()
                .map(InputPollOption::from)
                .collect::<Vec<_>>(),
        )
        .is_anonymous(false)
        .allows_multiple_answers(choice.allow_multiple)
        .await?;

    let poll = poll_message
        .poll()
        .ok_or_else(|| anyhow::anyhow!("Telegram poll response missing poll payload"))?;

    context
        .storage
        .save_pending_input_poll(&PendingInputPoll {
            task_id: record.metadata.id,
            request_id: pending_input.request_id.clone(),
            owner_user_id: user_id,
            poll_id: poll.id.to_string(),
            chat_id: chat_id.0,
            message_id: poll_message.id.0,
            answered: false,
            selected_option_ids: Vec::new(),
        })
        .await?;

    Ok(true)
}

async fn mark_pending_poll_answered(
    storage: &Arc<dyn StorageProvider>,
    pending_poll: &mut PendingInputPoll,
) -> Result<()> {
    pending_poll.answered = true;
    storage.save_pending_input_poll(pending_poll).await?;
    Ok(())
}

async fn run_agent_task_with_text(request: RunAgentTaskRequest) -> Result<TaskExecutionOutcome> {
    let RunAgentTaskRequest {
        bot,
        chat_id,
        user_id,
        execution,
        storage,
        task_runtime,
        cancellation_token,
    } = request;

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

    let result = execute_agent_task(
        user_id,
        &execution.task_text,
        execution.resume_input.as_deref(),
        Some(tx),
        cancellation_token,
    )
    .await;
    let state = match progress_handle.await {
        Ok(state) => state,
        Err(err) => {
            warn!(error = %err, "Progress runtime task failed");
            ProgressState::new(AGENT_MAX_ITERATIONS)
        }
    };
    let progress_text = render_progress_html(&state);

    let session_id = SessionId::from(user_id);
    task_runtime
        .save_memory_after_task(session_id, user_id, &storage)
        .await;

    match result {
        Ok(AgentExecutionResult {
            outcome: AgentExecutionOutcome::Completed(response),
            ..
        }) => {
            super::resilient::edit_message_safe_resilient(
                &bot,
                chat_id,
                progress_msg.id,
                &progress_text,
            )
            .await;
            // Use send_long_message to properly split response if it exceeds Telegram limit
            send_long_message(&bot, chat_id, &response).await?;
            Ok(TaskExecutionOutcome::Completed)
        }
        Ok(AgentExecutionResult {
            outcome: AgentExecutionOutcome::WaitingInput(pending_input),
            memory,
        }) => {
            let waiting_text =
                format_waiting_input_progress_text(&progress_text, &pending_input.prompt);
            super::resilient::edit_message_safe_resilient(
                &bot,
                chat_id,
                progress_msg.id,
                &waiting_text,
            )
            .await;

            let mut snapshot = TaskSnapshot::new(
                TaskMetadata::new(),
                SessionId::from(user_id),
                "serialize waiting memory".to_string(),
                0,
            );
            snapshot.set_agent_memory(&memory)?;
            let agent_memory = snapshot
                .agent_memory
                .ok_or_else(|| anyhow::anyhow!("missing serialized waiting memory payload"))?;
            Ok(TaskExecutionOutcome::WaitingInput {
                pending_input,
                agent_memory,
            })
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
            Err(e)
        }
    }
}

fn format_waiting_input_progress_text(progress_text: &str, prompt: &str) -> String {
    let escaped_prompt = html_escape::encode_text(prompt);
    format!(
        "{progress_text}\n\n⏸️ <b>Waiting for input:</b>\n\n{}",
        escaped_prompt
    )
}

/// Execute an agent task and return the result
async fn execute_agent_task(
    user_id: i64,
    task: &str,
    resume_input: Option<&str>,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    cancellation_token: Arc<CancellationToken>,
) -> Result<AgentExecutionResult> {
    let session_id = SessionId::from(user_id);
    // Get executor from registry
    let executor_arc = SESSION_REGISTRY
        .get(&session_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("No agent session found"))?;

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
    let outcome = executor
        .execute_with_outcome(task, resume_input, progress_tx)
        .await?;
    let memory = executor.session().memory.clone();

    Ok(AgentExecutionResult { outcome, memory })
}

async fn submit_agent_task(
    bot: &Bot,
    chat_id: ChatId,
    storage: Arc<dyn StorageProvider>,
    task_runtime: Arc<AgentTaskRuntime>,
    session_id: SessionId,
    task_text: String,
) -> Result<()> {
    info!(
        session_id = %session_id,
        chat_id = chat_id.0,
        "Submitting agent task through runtime task executor"
    );

    let backend = Arc::new(TelegramTaskExecutionBackend {
        bot: bot.clone(),
        chat_id,
        storage,
        task_runtime: Arc::clone(&task_runtime),
    });

    if let Err(error) = task_runtime
        .submit_task(session_id, task_text, backend)
        .await
    {
        warn!(session_id = %session_id, error = %error, "Failed to submit agent task");
        bot.send_message(chat_id, DefaultAgentView::error_message(&error.to_string()))
            .await?;
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
    context: Arc<TelegramHandlerContext>,
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
            let backend = Arc::new(TelegramTaskExecutionBackend {
                bot: bot.clone(),
                chat_id,
                storage: Arc::clone(&context.storage),
                task_runtime: Arc::clone(&context.task_runtime),
            });

            match context
                .task_runtime
                .retry_task_without_loop_detection(
                    user_id,
                    &context.llm,
                    &context.storage,
                    &context.settings,
                    backend,
                )
                .await?
            {
                RetryTaskOutcome::Submitted => {}
                RetryTaskOutcome::NoSavedTask => {
                    bot.send_message(chat_id, DefaultAgentView::no_saved_task())
                        .await?;
                }
                RetryTaskOutcome::SessionNotFound => {
                    bot.send_message(chat_id, DefaultAgentView::session_not_found())
                        .await?;
                }
            }
        }
        LOOP_CALLBACK_RESET => {
            match context
                .task_runtime
                .cancel_and_reset_session(SessionId::from(user_id))
                .await?
            {
                SessionResetOutcome::Reset => {
                    bot.send_message(chat_id, DefaultAgentView::task_reset())
                        .reply_markup(get_agent_keyboard())
                        .await?;
                }
                SessionResetOutcome::SessionNotFound => {
                    bot.send_message(chat_id, DefaultAgentView::session_not_found())
                        .await?;
                }
                SessionResetOutcome::Busy => {
                    bot.send_message(chat_id, DefaultAgentView::reset_blocked_by_task())
                        .await?;
                }
            }
        }
        LOOP_CALLBACK_CANCEL => {
            cancel_agent_task_by_id(
                bot.clone(),
                user_id,
                chat_id,
                Arc::clone(&context.task_runtime),
            )
            .await?;
        }
        _ => {}
    }

    Ok(())
}

/// Handle Telegram poll answers for waiting choice input routing.
///
/// # Errors
///
/// Returns an error only for storage or Telegram API failures.
pub async fn handle_pending_input_poll_answer(
    bot: Bot,
    answer: PollAnswer,
    context: Arc<TelegramHandlerContext>,
) -> Result<()> {
    let Some(user) = answer.voter.user() else {
        return Ok(());
    };

    let answering_user_id = user.id.0.cast_signed();
    let Some(mut pending_poll) = context
        .storage
        .load_pending_input_poll_by_id(&answer.poll_id.0)
        .await?
    else {
        return Ok(());
    };

    if pending_poll.poll_id != answer.poll_id.0 {
        warn!(
            expected_poll_id = %pending_poll.poll_id,
            actual_poll_id = %answer.poll_id,
            task_id = %pending_poll.task_id,
            "Rejected mismatched poll answer mapping"
        );
        return Ok(());
    }

    if pending_poll.owner_user_id != answering_user_id {
        warn!(
            poll_id = %answer.poll_id,
            expected_owner = pending_poll.owner_user_id,
            actual_user = answering_user_id,
            "Rejected foreign poll answer"
        );
        return Ok(());
    }

    if pending_poll.answered {
        return Ok(());
    }

    let Some(record) = context
        .task_runtime
        .task_registry
        .get(&pending_poll.task_id)
        .await
    else {
        mark_pending_poll_answered(&context.storage, &mut pending_poll).await?;
        return Ok(());
    };

    let Some(pending_input) = record.pending_input else {
        mark_pending_poll_answered(&context.storage, &mut pending_poll).await?;
        return Ok(());
    };

    if record.metadata.state != TaskState::WaitingInput
        || pending_input.request_id != pending_poll.request_id
    {
        mark_pending_poll_answered(&context.storage, &mut pending_poll).await?;
        return Ok(());
    }

    let PendingInputKind::Choice(choice) = pending_input.kind else {
        mark_pending_poll_answered(&context.storage, &mut pending_poll).await?;
        return Ok(());
    };

    if !is_valid_poll_answer(&answer.option_ids, &choice) {
        warn!(
            poll_id = %answer.poll_id,
            task_id = %pending_poll.task_id,
            "Rejected invalid poll answer payload"
        );
        return Ok(());
    }

    pending_poll.answered = true;
    pending_poll.selected_option_ids = answer.option_ids.clone();
    context
        .storage
        .save_pending_input_poll(&pending_poll)
        .await?;

    let resume_outcome = resume_task_from_consumed_poll_answer(
        &bot,
        context.as_ref(),
        &pending_poll,
        &answer.option_ids,
    )
    .await?;

    if resume_outcome == ConsumedPollResumeOutcome::Resumed {
        if let Err(error) = bot
            .stop_poll(
                ChatId(pending_poll.chat_id),
                MessageId(pending_poll.message_id),
            )
            .await
        {
            warn!(
                task_id = %pending_poll.task_id,
                poll_id = %pending_poll.poll_id,
                error = %error,
                "Failed to close consumed Telegram poll"
            );
        }
    }

    Ok(())
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
    task_runtime: Arc<AgentTaskRuntime>,
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let session_id = SessionId::from(user_id);

    let cancelled = task_runtime
        .cancel_task_for_session(session_id)
        .await?
        .is_some();

    // Best-effort: clear todos without waiting for executor locks.
    let cleared_todos = matches!(
        task_runtime.clear_todos(session_id).await,
        ClearTodosOutcome::Cleared
    );

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

async fn cancel_agent_task_by_id(
    bot: Bot,
    user_id: i64,
    chat_id: ChatId,
    task_runtime: Arc<AgentTaskRuntime>,
) -> Result<()> {
    let session_id = SessionId::from(user_id);
    let cancelled = task_runtime
        .cancel_task_for_session(session_id)
        .await?
        .is_some();
    let cleared_todos = matches!(
        task_runtime.clear_todos(session_id).await,
        ClearTodosOutcome::Cleared
    );

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
    storage: Arc<dyn StorageProvider>,
    task_runtime: Arc<AgentTaskRuntime>,
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let session_id = SessionId::from(user_id);

    match task_runtime
        .exit_session(session_id, user_id, &storage)
        .await
    {
        ExitSessionOutcome::BlockedByTask => {
            bot.send_message(msg.chat.id, DefaultAgentView::exit_blocked_by_task())
                .reply_markup(get_agent_keyboard())
                .await?;
            return Ok(());
        }
        ExitSessionOutcome::Exited => {}
    }

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
    task_runtime: Arc<AgentTaskRuntime>,
) -> Result<()> {
    let user_id = msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    if let Some(message_text) =
        destructive_action_block_message(&task_runtime, SessionId::from(user_id), &action).await
    {
        bot.send_message(msg.chat.id, message_text)
            .reply_markup(get_agent_keyboard())
            .await?;
        return Ok(());
    }

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

async fn handle_clear_memory_confirmation(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    session_id: SessionId,
    _action: &ConfirmationType,
    context: &TelegramHandlerContext,
) -> Result<()> {
    info!(user_id = user_id, "User confirmed memory clear");
    let keyboard = get_agent_keyboard();

    match context
        .task_runtime
        .clear_memory(session_id, user_id, &context.storage)
        .await
    {
        ClearMemoryOutcome::BlockedByTask => {
            bot.send_message(chat_id, DefaultAgentView::clear_blocked_by_task())
                .reply_markup(keyboard)
                .await?;
        }
        ClearMemoryOutcome::Cleared => {
            bot.send_message(chat_id, DefaultAgentView::memory_cleared())
                .reply_markup(keyboard)
                .await?;
        }
    }

    Ok(())
}

async fn handle_recreate_container_confirmation(
    bot: &Bot,
    chat_id: ChatId,
    user_id: i64,
    session_id: SessionId,
    _action: &ConfirmationType,
    context: &TelegramHandlerContext,
) -> Result<()> {
    info!(user_id = user_id, "User confirmed container recreation");
    let keyboard = get_agent_keyboard();

    match context
        .task_runtime
        .recreate_container(session_id, user_id, context)
        .await
    {
        RecreateContainerOutcome::Recreated => {
            bot.send_message(chat_id, DefaultAgentView::container_recreated())
                .reply_markup(keyboard)
                .await?;
        }
        RecreateContainerOutcome::RecreateFailed(error) => {
            warn!(error = %error, "Container recreation failed");
            bot.send_message(
                chat_id,
                DefaultAgentView::container_error(&format!("{error:#}")),
            )
            .reply_markup(keyboard)
            .await?;
        }
        RecreateContainerOutcome::BlockedByTask => {
            bot.send_message(
                chat_id,
                DefaultAgentView::container_recreate_blocked_by_task(),
            )
            .reply_markup(keyboard)
            .await?;
        }
        RecreateContainerOutcome::SessionAccessError => {
            bot.send_message(chat_id, DefaultAgentView::sandbox_access_error())
                .reply_markup(keyboard)
                .await?;
        }
    }

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
    context: Arc<TelegramHandlerContext>,
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
    let session_id = SessionId::from(user_id);

    match text {
        "✅ Yes" => match action {
            ConfirmationType::ClearMemory => {
                handle_clear_memory_confirmation(
                    &bot, chat_id, user_id, session_id, &action, &context,
                )
                .await?;
            }
            ConfirmationType::RecreateContainer => {
                handle_recreate_container_confirmation(
                    &bot, chat_id, user_id, session_id, &action, &context,
                )
                .await?;
            }
        },
        "❌ Cancel" => {
            info!(user_id = user_id, action = ?action, "User cancelled destructive action");
            let keyboard = get_agent_keyboard();
            bot.send_message(chat_id, DefaultAgentView::operation_cancelled())
                .reply_markup(keyboard)
                .await?;
        }
        _ => unreachable!(),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        destructive_action_block_message, exit_block_message, AgentModeActivationOutcome,
        AgentTaskRuntime, ClearMemoryOutcome, ExitSessionOutcome, RecreateContainerOutcome,
        RetryTaskOutcome, SessionResetOutcome, SESSION_REGISTRY,
    };
    use crate::bot::context::TelegramHandlerContext;
    use crate::bot::views::{AgentView, DefaultAgentView};
    use crate::config::{BotSettings, TelegramSettings};
    use anyhow::{anyhow, Result as AnyResult};
    use async_trait::async_trait;
    use oxide_agent_core::agent::{
        AgentExecutionOutcome, AgentExecutor, AgentMemory, AgentSession, PendingChoiceInput,
        PendingInput, PendingInputKind, PendingTextInput, SessionId, TaskEvent, TaskId,
        TaskMetadata, TaskSnapshot, TaskState, TodoItem, TodoStatus,
    };
    use oxide_agent_core::config::AgentSettings;
    use oxide_agent_core::llm::{
        ChatResponse, LlmClient, LlmError, LlmProvider, Message as LlmMessage, ToolCall,
        ToolCallFunction, ToolDefinition,
    };
    use oxide_agent_core::storage::{Message, StorageError, StorageProvider, UserConfig};
    use oxide_agent_runtime::{
        CancellationToken, TaskExecutionBackend, TaskExecutionOutcome, TaskExecutionRequest,
        TaskExecutorError, TaskRegistry,
    };
    use std::collections::HashMap;
    use std::sync::Arc;
    use teloxide::types::{ChatId, MaybeAnonymousUser, PollAnswer, PollId, User, UserId};
    use teloxide::Bot;
    use tokio::sync::{Barrier, Mutex, Notify};
    use tokio::task::{JoinError, JoinHandle};
    use tokio::time::{timeout, Duration};

    struct TestStorage {
        snapshots: Mutex<HashMap<TaskId, TaskSnapshot>>,
        pending_polls_by_task: Mutex<HashMap<TaskId, oxide_agent_core::storage::PendingInputPoll>>,
        pending_poll_task_by_id: Mutex<HashMap<String, TaskId>>,
        saved_memory_users: Mutex<Vec<i64>>,
        cleared_memory_users: Mutex<Vec<i64>>,
        block_first_task_snapshot_save: Mutex<bool>,
        snapshot_save_started: Option<Arc<Notify>>,
        release_snapshot_save: Option<Arc<Notify>>,
    }

    impl Default for TestStorage {
        fn default() -> Self {
            Self {
                snapshots: Mutex::new(HashMap::new()),
                pending_polls_by_task: Mutex::new(HashMap::new()),
                pending_poll_task_by_id: Mutex::new(HashMap::new()),
                saved_memory_users: Mutex::new(Vec::new()),
                cleared_memory_users: Mutex::new(Vec::new()),
                block_first_task_snapshot_save: Mutex::new(false),
                snapshot_save_started: None,
                release_snapshot_save: None,
            }
        }
    }

    impl TestStorage {
        fn with_blocked_first_task_snapshot_save(
            snapshot_save_started: Arc<Notify>,
            release_snapshot_save: Arc<Notify>,
        ) -> Self {
            Self {
                block_first_task_snapshot_save: Mutex::new(true),
                snapshot_save_started: Some(snapshot_save_started),
                release_snapshot_save: Some(release_snapshot_save),
                ..Self::default()
            }
        }
    }

    struct LockingBackend {
        started: Arc<Notify>,
        released: Arc<Notify>,
    }

    struct CancelledButLiveBackend {
        started: Arc<Notify>,
        cancelled: Arc<Notify>,
        release: Arc<Notify>,
        stopped: Arc<Notify>,
    }

    struct DeferredLockBackend {
        started: Arc<Notify>,
        allow_executor_lock: Arc<Notify>,
        entered_executor: Arc<Notify>,
        released: Arc<Notify>,
    }

    struct CompletedBackend;

    struct WaitingInputLlmProvider;

    #[derive(Clone, Default)]
    struct RecordingResumeBackend {
        captured: Arc<Mutex<Vec<TaskExecutionRequest>>>,
        started: Arc<Notify>,
    }

    #[derive(Clone, Default)]
    struct RecordingSessionMemoryBackend {
        captured_first_message: Arc<Mutex<Option<String>>>,
        started: Arc<Notify>,
    }

    #[async_trait]
    impl TaskExecutionBackend for LockingBackend {
        async fn execute(&self, request: TaskExecutionRequest) -> AnyResult<TaskExecutionOutcome> {
            let executor_arc = SESSION_REGISTRY
                .get(&request.session_id)
                .await
                .ok_or_else(|| anyhow!("session missing for test backend"))?;
            let mut executor = executor_arc.write().await;
            executor.session_mut().cancellation_token = (*request.cancellation_token).clone();
            self.started.notify_one();
            request.cancellation_token.cancelled().await;
            drop(executor);
            self.released.notify_one();
            Ok(TaskExecutionOutcome::Completed)
        }
    }

    #[async_trait]
    impl TaskExecutionBackend for CancelledButLiveBackend {
        async fn execute(&self, request: TaskExecutionRequest) -> AnyResult<TaskExecutionOutcome> {
            let executor_arc = SESSION_REGISTRY
                .get(&request.session_id)
                .await
                .ok_or_else(|| anyhow!("session missing for test backend"))?;
            let mut executor = executor_arc.write().await;
            executor.session_mut().cancellation_token = (*request.cancellation_token).clone();
            self.started.notify_one();
            request.cancellation_token.cancelled().await;
            self.cancelled.notify_one();
            self.release.notified().await;
            drop(executor);
            self.stopped.notify_one();
            Ok(TaskExecutionOutcome::Completed)
        }
    }

    #[async_trait]
    impl TaskExecutionBackend for DeferredLockBackend {
        async fn execute(&self, request: TaskExecutionRequest) -> AnyResult<TaskExecutionOutcome> {
            self.started.notify_one();
            self.allow_executor_lock.notified().await;

            let executor_arc = SESSION_REGISTRY
                .get(&request.session_id)
                .await
                .ok_or_else(|| anyhow!("session missing for test backend"))?;
            let mut executor = executor_arc.write().await;
            executor.session_mut().cancellation_token = (*request.cancellation_token).clone();
            self.entered_executor.notify_one();
            request.cancellation_token.cancelled().await;
            drop(executor);
            self.released.notify_one();
            Ok(TaskExecutionOutcome::Completed)
        }
    }

    #[async_trait]
    impl TaskExecutionBackend for CompletedBackend {
        async fn execute(&self, _request: TaskExecutionRequest) -> AnyResult<TaskExecutionOutcome> {
            Ok(TaskExecutionOutcome::Completed)
        }
    }

    #[async_trait]
    impl LlmProvider for WaitingInputLlmProvider {
        async fn chat_completion(
            &self,
            _system_prompt: &str,
            _history: &[LlmMessage],
            _user_message: &str,
            _model_id: &str,
            _max_tokens: u32,
        ) -> Result<String, LlmError> {
            Err(LlmError::Unknown(
                "chat_completion is not used in this test".to_string(),
            ))
        }

        async fn transcribe_audio(
            &self,
            _audio_bytes: Vec<u8>,
            _mime_type: &str,
            _model_id: &str,
        ) -> Result<String, LlmError> {
            Err(LlmError::Unknown(
                "transcribe_audio is not used in this test".to_string(),
            ))
        }

        async fn analyze_image(
            &self,
            _image_bytes: Vec<u8>,
            _text_prompt: &str,
            _system_prompt: &str,
            _model_id: &str,
        ) -> Result<String, LlmError> {
            Err(LlmError::Unknown(
                "analyze_image is not used in this test".to_string(),
            ))
        }

        async fn chat_with_tools(
            &self,
            _system_prompt: &str,
            _messages: &[LlmMessage],
            _tools: &[ToolDefinition],
            _model_id: &str,
            _max_tokens: u32,
            _json_mode: bool,
        ) -> Result<ChatResponse, LlmError> {
            Ok(ChatResponse {
                content: Some("tool_call".to_string()),
                tool_calls: vec![ToolCall {
                    id: "call_waiting_input_telegram".to_string(),
                    function: ToolCallFunction {
                        name: "request_user_input".to_string(),
                        arguments: r#"{"prompt":"Provide release approval","kind":"text","text":{"min_length":1,"max_length":32,"multiline":false}}"#.to_string(),
                    },
                    is_recovered: false,
                }],
                finish_reason: "tool_calls".to_string(),
                reasoning_content: None,
                usage: None,
            })
        }
    }

    #[async_trait]
    impl TaskExecutionBackend for RecordingResumeBackend {
        async fn execute(&self, request: TaskExecutionRequest) -> AnyResult<TaskExecutionOutcome> {
            self.captured.lock().await.push(request);
            self.started.notify_waiters();
            Ok(TaskExecutionOutcome::Completed)
        }
    }

    #[async_trait]
    impl TaskExecutionBackend for RecordingSessionMemoryBackend {
        async fn execute(&self, request: TaskExecutionRequest) -> AnyResult<TaskExecutionOutcome> {
            let executor_arc = SESSION_REGISTRY
                .get(&request.session_id)
                .await
                .ok_or_else(|| anyhow!("session missing for test backend"))?;
            let executor = executor_arc.read().await;
            let first_message = executor
                .session()
                .memory
                .get_messages()
                .first()
                .map(|message| message.content.clone());
            *self.captured_first_message.lock().await = first_message;
            self.started.notify_waiters();
            Ok(TaskExecutionOutcome::Completed)
        }
    }

    #[async_trait]
    impl StorageProvider for TestStorage {
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
            user_id: i64,
            _memory: &AgentMemory,
        ) -> Result<(), StorageError> {
            self.saved_memory_users.lock().await.push(user_id);
            Ok(())
        }

        async fn load_agent_memory(
            &self,
            _user_id: i64,
        ) -> Result<Option<AgentMemory>, StorageError> {
            Ok(None)
        }

        async fn clear_agent_memory(&self, user_id: i64) -> Result<(), StorageError> {
            self.cleared_memory_users.lock().await.push(user_id);
            Ok(())
        }

        async fn clear_all_context(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_task_snapshot(&self, snapshot: &TaskSnapshot) -> Result<(), StorageError> {
            let should_block = {
                let mut block_first_save = self.block_first_task_snapshot_save.lock().await;
                if *block_first_save {
                    *block_first_save = false;
                    true
                } else {
                    false
                }
            };

            if should_block {
                if let Some(notify) = &self.snapshot_save_started {
                    notify.notify_one();
                }
                if let Some(release) = &self.release_snapshot_save {
                    release.notified().await;
                }
            }

            self.snapshots
                .lock()
                .await
                .insert(snapshot.metadata.id, snapshot.clone());
            Ok(())
        }

        async fn load_task_snapshot(
            &self,
            task_id: TaskId,
        ) -> Result<Option<TaskSnapshot>, StorageError> {
            Ok(self.snapshots.lock().await.get(&task_id).cloned())
        }

        async fn append_task_event(
            &self,
            _task_id: TaskId,
            _event: TaskEvent,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn load_task_events(&self, _task_id: TaskId) -> Result<Vec<TaskEvent>, StorageError> {
            Ok(Vec::new())
        }

        async fn save_pending_input_poll(
            &self,
            poll: &oxide_agent_core::storage::PendingInputPoll,
        ) -> Result<(), StorageError> {
            self.pending_poll_task_by_id
                .lock()
                .await
                .insert(poll.poll_id.clone(), poll.task_id);
            self.pending_polls_by_task
                .lock()
                .await
                .insert(poll.task_id, poll.clone());
            Ok(())
        }

        async fn load_pending_input_poll_by_task(
            &self,
            task_id: TaskId,
        ) -> Result<Option<oxide_agent_core::storage::PendingInputPoll>, StorageError> {
            Ok(self
                .pending_polls_by_task
                .lock()
                .await
                .get(&task_id)
                .cloned())
        }

        async fn load_pending_input_poll_by_id(
            &self,
            poll_id: &str,
        ) -> Result<Option<oxide_agent_core::storage::PendingInputPoll>, StorageError> {
            let task_id = self
                .pending_poll_task_by_id
                .lock()
                .await
                .get(poll_id)
                .copied();
            let Some(task_id) = task_id else {
                return Ok(None);
            };
            Ok(self
                .pending_polls_by_task
                .lock()
                .await
                .get(&task_id)
                .cloned())
        }

        async fn delete_pending_input_poll(
            &self,
            task_id: TaskId,
            poll_id: &str,
        ) -> Result<(), StorageError> {
            self.pending_polls_by_task.lock().await.remove(&task_id);
            self.pending_poll_task_by_id.lock().await.remove(poll_id);
            Ok(())
        }

        async fn check_connection(&self) -> Result<(), String> {
            Ok(())
        }
    }

    fn settings_without_llm_providers() -> AgentSettings {
        AgentSettings {
            openrouter_site_name: "Oxide Agent Bot".to_string(),
            ..AgentSettings::default()
        }
    }

    fn settings_with_waiting_input_model() -> AgentSettings {
        AgentSettings {
            openrouter_site_name: "Oxide Agent Bot".to_string(),
            agent_model_id: Some("test-model".to_string()),
            agent_model_provider: Some("openrouter".to_string()),
            agent_model_max_tokens: Some(8_192),
            ..AgentSettings::default()
        }
    }

    async fn insert_test_session(session_id: SessionId) {
        let settings = Arc::new(settings_without_llm_providers());
        let llm = Arc::new(LlmClient::new(&settings));
        let mut session = AgentSession::new(session_id);
        session.last_task = Some("stale task".to_string());
        session.memory.todos.items.push(TodoItem {
            description: "todo".to_string(),
            status: TodoStatus::Pending,
        });
        let executor = AgentExecutor::new(llm, session, settings);
        SESSION_REGISTRY.insert(session_id, executor).await;
    }

    fn make_test_context(
        storage: Arc<dyn StorageProvider>,
        task_runtime: Arc<AgentTaskRuntime>,
    ) -> TelegramHandlerContext {
        let agent_settings = settings_without_llm_providers();
        let llm_settings = Arc::new(agent_settings.clone());
        let llm = Arc::new(LlmClient::new(&llm_settings));

        TelegramHandlerContext {
            storage,
            llm,
            settings: Arc::new(BotSettings::new(
                agent_settings,
                TelegramSettings::default(),
            )),
            task_runtime,
        }
    }

    fn unwrap_join_result<T>(result: Result<T, JoinError>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("unexpected join error: {error}"),
        }
    }

    fn retry_runtime_client() -> (Arc<LlmClient>, Arc<BotSettings>) {
        let agent_settings = settings_without_llm_providers();
        let llm_settings = Arc::new(agent_settings.clone());
        let llm = Arc::new(LlmClient::new(&llm_settings));
        let settings = Arc::new(BotSettings::new(
            agent_settings,
            TelegramSettings::default(),
        ));

        (llm, settings)
    }

    fn waiting_pending_input() -> PendingInput {
        PendingInput {
            request_id: "waiting-request".to_string(),
            prompt: "Provide approval".to_string(),
            kind: PendingInputKind::Text(PendingTextInput {
                min_length: Some(1),
                max_length: Some(120),
                multiline: false,
            }),
        }
    }

    fn waiting_choice_pending_input() -> PendingInput {
        PendingInput {
            request_id: "choice-request".to_string(),
            prompt: "Pick deployment strategy".to_string(),
            kind: PendingInputKind::Choice(PendingChoiceInput {
                options: vec!["blue-green".to_string(), "rolling".to_string()],
                allow_multiple: false,
                min_choices: 1,
                max_choices: 1,
            }),
        }
    }

    fn attach_waiting_snapshot_memory(snapshot: &mut TaskSnapshot) {
        let mut memory = AgentMemory::new(4_096);
        memory.add_message(oxide_agent_core::agent::memory::AgentMessage::assistant(
            "paused for user input",
        ));
        assert!(snapshot.set_agent_memory(&memory).is_ok());
    }

    fn build_poll_answer(poll_id: &str, user_id: i64, option_ids: &[u8]) -> PollAnswer {
        PollAnswer {
            poll_id: PollId(poll_id.to_string()),
            voter: MaybeAnonymousUser::User(User {
                id: UserId(u64::try_from(user_id).unwrap_or_default()),
                is_bot: false,
                first_name: "tester".to_string(),
                last_name: None,
                username: None,
                language_code: None,
                is_premium: false,
                added_to_attachment_menu: false,
            }),
            option_ids: option_ids.to_vec(),
        }
    }

    async fn wait_for_resume_request(backend: &RecordingResumeBackend) -> TaskExecutionRequest {
        let waited = timeout(Duration::from_secs(2), backend.started.notified()).await;
        assert!(waited.is_ok());
        let requests = backend.captured.lock().await;
        assert_eq!(requests.len(), 1);
        requests[0].clone()
    }

    async fn assert_no_resume_request(backend: &RecordingResumeBackend) {
        let waited = timeout(Duration::from_millis(200), backend.started.notified()).await;
        assert!(waited.is_err());
        assert!(backend.captured.lock().await.is_empty());
    }

    async fn wait_for_first_session_message(
        backend: &RecordingSessionMemoryBackend,
    ) -> Option<String> {
        let waited = timeout(Duration::from_secs(2), backend.started.notified()).await;
        assert!(waited.is_ok());
        backend.captured_first_message.lock().await.clone()
    }

    async fn assert_waiting_task_blocks_controls(
        task_runtime: &AgentTaskRuntime,
        storage: Arc<dyn StorageProvider>,
        session_id: SessionId,
    ) {
        let user_id = session_id.as_i64();
        let context = make_test_context(Arc::clone(&storage), Arc::new(task_runtime.clone()));

        let active = task_runtime.active_task_for_session(session_id).await;
        assert!(matches!(
            active,
            Some(record) if record.metadata.state == TaskState::WaitingInput
        ));

        let submitted = task_runtime
            .submit_task(
                session_id,
                "should be blocked by waiting task".to_string(),
                Arc::new(CompletedBackend),
            )
            .await;
        assert!(matches!(
            submitted,
            Err(TaskExecutorError::SessionTaskAlreadyRunning(rejected_session_id))
                if rejected_session_id == session_id
        ));

        assert!(task_runtime.blocks_start_reset(session_id).await);

        let exit_outcome = task_runtime
            .exit_session(session_id, user_id, &storage)
            .await;
        assert!(matches!(exit_outcome, ExitSessionOutcome::BlockedByTask));

        let clear_outcome = task_runtime
            .clear_memory(session_id, user_id, &storage)
            .await;
        assert!(matches!(clear_outcome, ClearMemoryOutcome::BlockedByTask));

        let recreate_outcome = task_runtime
            .recreate_container(session_id, user_id, &context)
            .await;
        assert!(matches!(
            recreate_outcome,
            RecreateContainerOutcome::BlockedByTask
        ));
    }

    fn spawn_retry_without_loop_detection<B>(
        task_runtime: Arc<AgentTaskRuntime>,
        user_id: i64,
        llm: Arc<LlmClient>,
        storage: Arc<dyn StorageProvider>,
        settings: Arc<BotSettings>,
        backend: Arc<B>,
    ) -> JoinHandle<Result<RetryTaskOutcome, TaskExecutorError>>
    where
        B: TaskExecutionBackend,
    {
        tokio::spawn(async move {
            task_runtime
                .retry_task_without_loop_detection(user_id, &llm, &storage, &settings, backend)
                .await
        })
    }

    async fn wait_for_active_runtime_task(task_runtime: &AgentTaskRuntime, session_id: SessionId) {
        let active_result = timeout(Duration::from_secs(1), async {
            loop {
                if task_runtime
                    .active_task_for_session(session_id)
                    .await
                    .is_some()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(active_result.is_ok());
    }

    async fn wait_for_runtime_task_completion(
        task_runtime: &AgentTaskRuntime,
        session_id: SessionId,
    ) {
        let waited = timeout(Duration::from_secs(1), async {
            loop {
                if task_runtime
                    .active_task_for_session(session_id)
                    .await
                    .is_none()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(waited.is_ok());
    }

    #[test]
    fn waiting_input_progress_text_escapes_html_prompt() {
        let text = super::format_waiting_input_progress_text(
            "progress",
            "Need <b>approval</b> & \"quote\"",
        );

        assert!(text.contains("&lt;b&gt;approval&lt;/b&gt;"));
        assert!(text.contains("&amp;"));
        assert!(!text.contains("<b>approval</b>"));
    }

    #[tokio::test]
    async fn execute_agent_task_returns_waiting_input_for_real_executor_path() {
        let session_id = SessionId::from(4_001);
        let settings = Arc::new(settings_with_waiting_input_model());
        let mut llm_client = LlmClient::new(settings.as_ref());
        llm_client.register_provider("openrouter".to_string(), Arc::new(WaitingInputLlmProvider));
        let llm = Arc::new(llm_client);
        let executor = AgentExecutor::new(llm, AgentSession::new(session_id), settings);
        SESSION_REGISTRY.insert(session_id, executor).await;

        let result = super::execute_agent_task(
            session_id.as_i64(),
            "Need operator approval",
            None,
            None,
            Arc::new(CancellationToken::new()),
        )
        .await;

        let outcome = match result {
            Ok(value) => value,
            Err(error) => panic!("expected waiting-input outcome, got error: {error}"),
        };

        match outcome.outcome {
            AgentExecutionOutcome::WaitingInput(pending_input) => {
                assert_eq!(pending_input.prompt, "Provide release approval");
                match pending_input.kind {
                    PendingInputKind::Text(text) => {
                        assert_eq!(text.min_length, Some(1));
                        assert_eq!(text.max_length, Some(32));
                        assert!(!text.multiline);
                    }
                    PendingInputKind::Choice(_) => {
                        panic!("expected text pending input kind");
                    }
                }
            }
            AgentExecutionOutcome::Completed(answer) => {
                panic!("expected waiting-input outcome, got completion: {answer}");
            }
        }

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_cancel_for_session_marks_runtime_task_cancelled() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        );
        let session_id = SessionId::from(41);
        insert_test_session(session_id).await;

        let started = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let backend = Arc::new(LockingBackend {
            started: Arc::clone(&started),
            released: Arc::clone(&released),
        });

        let submit_result = task_runtime
            .submit_task(session_id, "cancel me".to_string(), backend)
            .await;
        assert!(submit_result.is_ok());

        let started_result = timeout(Duration::from_secs(1), started.notified()).await;
        assert!(started_result.is_ok());

        let cancelled = task_runtime.cancel_task_for_session(session_id).await;
        assert!(matches!(
            cancelled,
            Ok(Some(ref record)) if record.metadata.state.is_terminal()
        ));

        let released_result = timeout(Duration::from_secs(1), released.notified()).await;
        assert!(released_result.is_ok());
        assert!(task_runtime
            .active_task_for_session(session_id)
            .await
            .is_none());

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_reset_uses_runtime_cancellation_before_session_reset() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        );
        let session_id = SessionId::from(42);
        insert_test_session(session_id).await;

        let started = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let backend = Arc::new(LockingBackend {
            started: Arc::clone(&started),
            released: Arc::clone(&released),
        });

        let submit_result = task_runtime
            .submit_task(session_id, "reset me".to_string(), backend)
            .await;
        assert!(submit_result.is_ok());

        let started_result = timeout(Duration::from_secs(1), started.notified()).await;
        assert!(started_result.is_ok());

        let reset_result = task_runtime.cancel_and_reset_session(session_id).await;
        assert!(matches!(reset_result, Ok(SessionResetOutcome::Reset)));

        let released_result = timeout(Duration::from_secs(1), released.notified()).await;
        assert!(released_result.is_ok());

        let executor_arc = SESSION_REGISTRY.get(&session_id).await;
        assert!(executor_arc.is_some());
        let executor_arc = executor_arc.unwrap_or_else(|| unreachable!());
        let executor = executor_arc.read().await;
        assert!(executor.last_task().is_none());
        assert!(executor.session().memory.todos.items.is_empty());

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_exit_guard_blocks_when_runtime_task_is_active() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        );
        let session_id = SessionId::from(43);
        insert_test_session(session_id).await;

        let started = Arc::new(Notify::new());
        let cancelled_notify = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let stopped = Arc::new(Notify::new());
        let backend = Arc::new(CancelledButLiveBackend {
            started: Arc::clone(&started),
            cancelled: Arc::clone(&cancelled_notify),
            release: Arc::clone(&release),
            stopped: Arc::clone(&stopped),
        });

        let submit_result = task_runtime
            .submit_task(session_id, "exit guard".to_string(), backend)
            .await;
        assert!(submit_result.is_ok());

        let started_result = timeout(Duration::from_secs(1), started.notified()).await;
        assert!(started_result.is_ok());
        let block_message = exit_block_message(&task_runtime, session_id).await;
        assert_eq!(
            block_message,
            Some(DefaultAgentView::exit_blocked_by_task())
        );

        let cancelled = task_runtime.cancel_task_for_session(session_id).await;
        assert!(cancelled.is_ok());

        let cancelled_result = timeout(Duration::from_secs(1), cancelled_notify.notified()).await;
        assert!(cancelled_result.is_ok());

        let block_message = exit_block_message(&task_runtime, session_id).await;
        assert!(block_message.is_none());

        release.notify_one();

        let released_result = timeout(Duration::from_secs(1), stopped.notified()).await;
        assert!(released_result.is_ok());

        assert!(exit_block_message(&task_runtime, session_id)
            .await
            .is_none());

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_destructive_actions_guard_blocks_when_runtime_task_is_active() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        );
        let session_id = SessionId::from(44);
        insert_test_session(session_id).await;

        let started = Arc::new(Notify::new());
        let cancelled_notify = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let stopped = Arc::new(Notify::new());
        let backend = Arc::new(CancelledButLiveBackend {
            started: Arc::clone(&started),
            cancelled: Arc::clone(&cancelled_notify),
            release: Arc::clone(&release),
            stopped: Arc::clone(&stopped),
        });

        let submit_result = task_runtime
            .submit_task(session_id, "destructive guard".to_string(), backend)
            .await;
        assert!(submit_result.is_ok());

        let started_result = timeout(Duration::from_secs(1), started.notified()).await;
        assert!(started_result.is_ok());
        let clear_block = destructive_action_block_message(
            &task_runtime,
            session_id,
            &crate::bot::state::ConfirmationType::ClearMemory,
        )
        .await;
        assert_eq!(clear_block, Some(DefaultAgentView::clear_blocked_by_task()));

        let recreate_block = destructive_action_block_message(
            &task_runtime,
            session_id,
            &crate::bot::state::ConfirmationType::RecreateContainer,
        )
        .await;
        assert_eq!(
            recreate_block,
            Some(DefaultAgentView::container_recreate_blocked_by_task())
        );

        let cancelled = task_runtime.cancel_task_for_session(session_id).await;
        assert!(cancelled.is_ok());

        let cancelled_result = timeout(Duration::from_secs(1), cancelled_notify.notified()).await;
        assert!(cancelled_result.is_ok());

        let clear_block = destructive_action_block_message(
            &task_runtime,
            session_id,
            &crate::bot::state::ConfirmationType::ClearMemory,
        )
        .await;
        assert!(clear_block.is_none());

        let recreate_block = destructive_action_block_message(
            &task_runtime,
            session_id,
            &crate::bot::state::ConfirmationType::RecreateContainer,
        )
        .await;
        assert!(recreate_block.is_none());

        release.notify_one();

        let released_result = timeout(Duration::from_secs(1), stopped.notified()).await;
        assert!(released_result.is_ok());

        assert!(destructive_action_block_message(
            &task_runtime,
            session_id,
            &crate::bot::state::ConfirmationType::ClearMemory,
        )
        .await
        .is_none());
        assert!(destructive_action_block_message(
            &task_runtime,
            session_id,
            &crate::bot::state::ConfirmationType::RecreateContainer,
        )
        .await
        .is_none());

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_start_reset_guard_blocks_when_runtime_task_is_active() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        );
        let session_id = SessionId::from(56);
        insert_test_session(session_id).await;

        let started = Arc::new(Notify::new());
        let cancelled_notify = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let stopped = Arc::new(Notify::new());
        let backend = Arc::new(CancelledButLiveBackend {
            started: Arc::clone(&started),
            cancelled: Arc::clone(&cancelled_notify),
            release: Arc::clone(&release),
            stopped: Arc::clone(&stopped),
        });

        let submit_result = task_runtime
            .submit_task(session_id, "start guard".to_string(), backend)
            .await;
        assert!(submit_result.is_ok());
        assert!(timeout(Duration::from_secs(1), started.notified())
            .await
            .is_ok());

        assert!(task_runtime.blocks_start_reset(session_id).await);

        let cancelled = task_runtime.cancel_task_for_session(session_id).await;
        assert!(cancelled.is_ok());
        assert!(timeout(Duration::from_secs(1), cancelled_notify.notified())
            .await
            .is_ok());

        assert!(!task_runtime.blocks_start_reset(session_id).await);

        release.notify_one();
        assert!(timeout(Duration::from_secs(1), stopped.notified())
            .await
            .is_ok());

        assert!(!task_runtime.blocks_start_reset(session_id).await);

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_agent_mode_reentry_keeps_live_session_executor() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        );
        let session_id = SessionId::from(57);
        let user_id = session_id.as_i64();
        insert_test_session(session_id).await;
        let original_executor = SESSION_REGISTRY
            .get(&session_id)
            .await
            .unwrap_or_else(|| unreachable!());

        let started = Arc::new(Notify::new());
        let cancelled_notify = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let stopped = Arc::new(Notify::new());
        let backend = Arc::new(CancelledButLiveBackend {
            started: Arc::clone(&started),
            cancelled: Arc::clone(&cancelled_notify),
            release: Arc::clone(&release),
            stopped: Arc::clone(&stopped),
        });
        let (llm, settings) = retry_runtime_client();

        let submit_result = task_runtime
            .submit_task(session_id, "agent reentry guard".to_string(), backend)
            .await;
        assert!(submit_result.is_ok());
        assert!(timeout(Duration::from_secs(1), started.notified())
            .await
            .is_ok());

        let activation_outcome = task_runtime
            .activate_agent_mode_session(
                session_id,
                user_id,
                &llm,
                &(Arc::clone(&storage) as Arc<dyn StorageProvider>),
                &settings,
            )
            .await;
        assert!(matches!(
            activation_outcome,
            AgentModeActivationOutcome::LiveTaskStillRunning
        ));

        let current_executor = SESSION_REGISTRY
            .get(&session_id)
            .await
            .unwrap_or_else(|| unreachable!());
        assert!(Arc::ptr_eq(&original_executor, &current_executor));

        let cancelled = task_runtime.cancel_task_for_session(session_id).await;
        assert!(cancelled.is_ok());
        assert!(timeout(Duration::from_secs(1), cancelled_notify.notified())
            .await
            .is_ok());

        release.notify_one();
        assert!(timeout(Duration::from_secs(1), stopped.notified())
            .await
            .is_ok());

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_rejects_concurrent_submit_for_same_session() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            2,
        ));
        let session_id = SessionId::from(45);
        insert_test_session(session_id).await;

        let started = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let backend = Arc::new(LockingBackend {
            started: Arc::clone(&started),
            released: Arc::clone(&released),
        });
        let start_barrier = Arc::new(Barrier::new(3));

        let first_submit = {
            let task_runtime = Arc::clone(&task_runtime);
            let backend = Arc::clone(&backend);
            let start_barrier = Arc::clone(&start_barrier);
            tokio::spawn(async move {
                start_barrier.wait().await;
                task_runtime
                    .submit_task(session_id, "first".to_string(), backend)
                    .await
            })
        };
        let second_submit = {
            let task_runtime = Arc::clone(&task_runtime);
            let backend = Arc::clone(&backend);
            let start_barrier = Arc::clone(&start_barrier);
            tokio::spawn(async move {
                start_barrier.wait().await;
                task_runtime
                    .submit_task(session_id, "second".to_string(), backend)
                    .await
            })
        };

        start_barrier.wait().await;

        let first_result = first_submit.await;
        assert!(first_result.is_ok(), "first submit task failed to join");
        let second_result = second_submit.await;
        assert!(second_result.is_ok(), "second submit task failed to join");

        let first_result = unwrap_join_result(first_result);
        let second_result = unwrap_join_result(second_result);

        let successful_record = match (first_result, second_result) {
            (
                Ok(record),
                Err(TaskExecutorError::SessionTaskAlreadyRunning(rejected_session_id)),
            ) => {
                assert_eq!(rejected_session_id, session_id);
                record
            }
            (
                Err(TaskExecutorError::SessionTaskAlreadyRunning(rejected_session_id)),
                Ok(record),
            ) => {
                assert_eq!(rejected_session_id, session_id);
                record
            }
            (left, right) => panic!("unexpected concurrent submit results: {left:?} {right:?}"),
        };

        wait_for_active_runtime_task(&task_runtime, session_id).await;

        let session_records = task_registry.list_by_session(&session_id).await;
        assert_eq!(session_records.len(), 1);
        assert_eq!(
            session_records[0].metadata.id,
            successful_record.metadata.id
        );
        assert!(task_runtime
            .active_task_for_session(session_id)
            .await
            .is_some());

        let cancelled = task_runtime.cancel_task_for_session(session_id).await;
        assert!(matches!(
            cancelled,
            Ok(Some(ref record)) if record.metadata.id == successful_record.metadata.id
        ));

        let released_result = timeout(Duration::from_secs(1), released.notified()).await;
        assert!(released_result.is_ok());
        assert!(task_runtime
            .active_task_for_session(session_id)
            .await
            .is_none());

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_submit_vs_cancel_race_waits_for_admission_and_cancels_created_task() {
        let snapshot_save_started = Arc::new(Notify::new());
        let release_snapshot_save = Arc::new(Notify::new());
        let storage = Arc::new(TestStorage::with_blocked_first_task_snapshot_save(
            Arc::clone(&snapshot_save_started),
            Arc::clone(&release_snapshot_save),
        ));
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let session_id = SessionId::from(49);
        insert_test_session(session_id).await;

        let backend = Arc::new(LockingBackend {
            started: Arc::new(Notify::new()),
            released: Arc::new(Notify::new()),
        });

        let submit_task = {
            let task_runtime = Arc::clone(&task_runtime);
            let backend = Arc::clone(&backend);
            tokio::spawn(async move {
                task_runtime
                    .submit_task(session_id, "cancel admission race".to_string(), backend)
                    .await
            })
        };

        assert!(
            timeout(Duration::from_secs(1), snapshot_save_started.notified())
                .await
                .is_ok()
        );
        assert!(task_registry
            .latest_non_terminal_by_session(&session_id)
            .await
            .is_some());
        assert!(task_runtime
            .active_task_for_session(session_id)
            .await
            .is_some());

        let mut cancel_task = {
            let task_runtime = Arc::clone(&task_runtime);
            tokio::spawn(async move { task_runtime.cancel_task_for_session(session_id).await })
        };

        assert!(timeout(Duration::from_millis(50), &mut cancel_task)
            .await
            .is_err());

        release_snapshot_save.notify_one();

        let submit_result = unwrap_join_result(submit_task.await);
        assert!(submit_result.is_ok());
        let submitted_record = match submit_result {
            Ok(record) => record,
            Err(error) => panic!("unexpected submit error: {error}"),
        };

        let cancel_result = unwrap_join_result(cancel_task.await);
        assert!(matches!(
            cancel_result,
            Ok(Some(ref record))
                if record.metadata.id == submitted_record.metadata.id
                    && record.metadata.state == TaskState::Cancelled
        ));

        let stored_record = task_registry.get(&submitted_record.metadata.id).await;
        assert!(matches!(
            stored_record,
            Some(ref record) if record.metadata.state == TaskState::Cancelled
        ));
        wait_for_runtime_task_completion(&task_runtime, session_id).await;

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_controls_remain_blocked_for_paused_waiting_task() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        );
        let session_id = SessionId::from(113);

        let created = task_registry.create(session_id).await;
        let running = task_registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await;
        assert!(running.is_ok());

        let waiting = task_registry
            .enter_waiting_input(&created.metadata.id, waiting_pending_input())
            .await;
        assert!(waiting.is_ok());

        assert_waiting_task_blocks_controls(
            &task_runtime,
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            session_id,
        )
        .await;

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_controls_remain_blocked_for_recovered_waiting_task() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        );
        let session_id = SessionId::from(114);

        let mut metadata = TaskMetadata::new();
        metadata.state = TaskState::WaitingInput;
        let pending_input = waiting_pending_input();
        let restored = task_registry
            .restore(metadata, session_id, 2, Some(pending_input.clone()))
            .await;
        assert_eq!(restored.metadata.state, TaskState::WaitingInput);
        assert_eq!(restored.pending_input, Some(pending_input));

        assert_waiting_task_blocks_controls(
            &task_runtime,
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            session_id,
        )
        .await;

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn poll_answer_handler_rejects_foreign_owner() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let owner_user_id = 600;
        let session_id = SessionId::from(owner_user_id);

        let created = task_registry.create(session_id).await;
        assert!(task_registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await
            .is_ok());
        let pending_input = waiting_choice_pending_input();
        assert!(task_registry
            .enter_waiting_input(&created.metadata.id, pending_input.clone())
            .await
            .is_ok());

        let poll_id = "poll-foreign";
        assert!(storage
            .save_pending_input_poll(&oxide_agent_core::storage::PendingInputPoll {
                task_id: created.metadata.id,
                request_id: pending_input.request_id,
                owner_user_id,
                poll_id: poll_id.to_string(),
                chat_id: owner_user_id,
                message_id: 10,
                answered: false,
                selected_option_ids: Vec::new(),
            })
            .await
            .is_ok());

        let context = Arc::new(make_test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        ));
        let answer = build_poll_answer(poll_id, 601, &[0]);

        let handled =
            super::handle_pending_input_poll_answer(Bot::new("test-token"), answer, context).await;
        assert!(handled.is_ok());

        let poll = storage.load_pending_input_poll_by_id(poll_id).await;
        assert!(poll.is_ok());
        assert!(matches!(poll.ok().flatten(), Some(poll) if !poll.answered));
    }

    #[tokio::test]
    async fn poll_answer_handler_marks_late_answers_as_consumed() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let owner_user_id = 700;
        let session_id = SessionId::from(owner_user_id);

        let created = task_registry.create(session_id).await;
        let pending_input = waiting_choice_pending_input();
        assert!(storage
            .save_pending_input_poll(&oxide_agent_core::storage::PendingInputPoll {
                task_id: created.metadata.id,
                request_id: pending_input.request_id,
                owner_user_id,
                poll_id: "poll-late".to_string(),
                chat_id: owner_user_id,
                message_id: 11,
                answered: false,
                selected_option_ids: Vec::new(),
            })
            .await
            .is_ok());

        let context = Arc::new(make_test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        ));
        let answer = build_poll_answer("poll-late", owner_user_id, &[0]);

        let handled =
            super::handle_pending_input_poll_answer(Bot::new("test-token"), answer, context).await;
        assert!(handled.is_ok());

        let poll = storage.load_pending_input_poll_by_id("poll-late").await;
        assert!(poll.is_ok());
        assert!(matches!(poll.ok().flatten(), Some(poll) if poll.answered));
    }

    #[tokio::test]
    async fn text_resume_valid_input_resumes_waiting_task() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let owner_user_id = 740;
        let session_id = SessionId::from(owner_user_id);
        insert_test_session(session_id).await;

        let created = task_registry.create(session_id).await;
        assert!(task_registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await
            .is_ok());
        let pending_input = waiting_pending_input();
        let waiting = task_registry
            .enter_waiting_input(&created.metadata.id, pending_input)
            .await;
        assert!(waiting.is_ok());
        let waiting = waiting.unwrap_or_else(|_| unreachable!());

        let mut snapshot = TaskSnapshot::new(
            waiting.record.metadata.clone(),
            session_id,
            "resume text task".to_string(),
            waiting.event_sequence,
        );
        snapshot.pending_input = waiting.record.pending_input.clone();
        attach_waiting_snapshot_memory(&mut snapshot);
        assert!(storage.save_task_snapshot(&snapshot).await.is_ok());

        let context = Arc::new(make_test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        ));

        SESSION_REGISTRY.remove(&session_id).await;
        assert!(!SESSION_REGISTRY.contains(&session_id).await);

        let resumed = super::resume_waiting_task_input(
            &Bot::new("test-token"),
            context.as_ref(),
            super::ResumeTaskInput {
                user_id: owner_user_id,
                chat_id: ChatId(owner_user_id),
                task_id: &created.metadata.id,
                input: "approved".to_string(),
            },
        )
        .await;
        assert!(matches!(resumed, Ok(true)));

        let waited = timeout(Duration::from_secs(2), async {
            loop {
                if let Some(record) = task_registry.get(&created.metadata.id).await {
                    if record.metadata.state != TaskState::WaitingInput {
                        break;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(
            waited.is_ok(),
            "task did not leave waiting state after text resume"
        );
        assert!(SESSION_REGISTRY.contains(&session_id).await);

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn text_resume_cold_restart_preserves_original_task_for_backend_request() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let owner_user_id = 742;
        let session_id = SessionId::from(owner_user_id);

        let created = task_registry.create(session_id).await;
        assert!(task_registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await
            .is_ok());
        let pending_input = waiting_pending_input();
        let waiting = task_registry
            .enter_waiting_input(&created.metadata.id, pending_input)
            .await;
        assert!(waiting.is_ok());
        let waiting = waiting.unwrap_or_else(|_| unreachable!());

        let mut snapshot = TaskSnapshot::new(
            waiting.record.metadata.clone(),
            session_id,
            "resume text task original".to_string(),
            waiting.event_sequence,
        );
        snapshot.pending_input = waiting.record.pending_input.clone();
        attach_waiting_snapshot_memory(&mut snapshot);
        assert!(storage.save_task_snapshot(&snapshot).await.is_ok());

        let context = make_test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        );

        SESSION_REGISTRY.remove(&session_id).await;
        assert!(!SESSION_REGISTRY.contains(&session_id).await);

        let backend = Arc::new(RecordingResumeBackend::default());
        let resumed = super::resume_waiting_task_input_with_backend(
            &context,
            super::ResumeTaskInput {
                user_id: owner_user_id,
                chat_id: ChatId(owner_user_id),
                task_id: &created.metadata.id,
                input: "approved".to_string(),
            },
            Arc::clone(&backend),
        )
        .await;
        assert!(matches!(resumed, Ok(true)));

        let request = wait_for_resume_request(backend.as_ref()).await;
        assert_eq!(request.task, "resume text task original");
        assert_eq!(request.resume_input.as_deref(), Some("approved"));
        assert!(SESSION_REGISTRY.contains(&session_id).await);

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn text_resume_cold_restart_restores_pause_memory_before_resume() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let owner_user_id = 743;
        let session_id = SessionId::from(owner_user_id);

        let created = task_registry.create(session_id).await;
        assert!(task_registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await
            .is_ok());
        let pending_input = waiting_pending_input();
        let waiting = task_registry
            .enter_waiting_input(&created.metadata.id, pending_input)
            .await;
        assert!(waiting.is_ok());
        let waiting = waiting.unwrap_or_else(|_| unreachable!());

        let mut snapshot = TaskSnapshot::new(
            waiting.record.metadata.clone(),
            session_id,
            "resume text with memory".to_string(),
            waiting.event_sequence,
        );
        snapshot.pending_input = waiting.record.pending_input.clone();
        attach_waiting_snapshot_memory(&mut snapshot);
        assert!(storage.save_task_snapshot(&snapshot).await.is_ok());

        let context = make_test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        );

        SESSION_REGISTRY.remove(&session_id).await;
        assert!(!SESSION_REGISTRY.contains(&session_id).await);

        let backend = Arc::new(RecordingSessionMemoryBackend::default());
        let resumed = super::resume_waiting_task_input_with_backend(
            &context,
            super::ResumeTaskInput {
                user_id: owner_user_id,
                chat_id: ChatId(owner_user_id),
                task_id: &created.metadata.id,
                input: "approved".to_string(),
            },
            Arc::clone(&backend),
        )
        .await;
        assert!(matches!(resumed, Ok(true)));

        let first_message = wait_for_first_session_message(backend.as_ref()).await;
        assert_eq!(first_message.as_deref(), Some("paused for user input"));

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn text_resume_aborts_when_pause_memory_restore_cannot_lock_session() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let owner_user_id = 746;
        let session_id = SessionId::from(owner_user_id);

        let created = task_registry.create(session_id).await;
        assert!(task_registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await
            .is_ok());
        let pending_input = waiting_pending_input();
        let waiting = task_registry
            .enter_waiting_input(&created.metadata.id, pending_input)
            .await;
        assert!(waiting.is_ok());
        let waiting = waiting.unwrap_or_else(|_| unreachable!());

        let mut snapshot = TaskSnapshot::new(
            waiting.record.metadata.clone(),
            session_id,
            "resume text lock failure".to_string(),
            waiting.event_sequence,
        );
        snapshot.pending_input = waiting.record.pending_input.clone();
        attach_waiting_snapshot_memory(&mut snapshot);
        assert!(storage.save_task_snapshot(&snapshot).await.is_ok());

        let context = make_test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        );

        insert_test_session(session_id).await;

        let executor_arc = SESSION_REGISTRY.get(&session_id).await;
        assert!(executor_arc.is_some());
        let executor_arc = executor_arc.unwrap_or_else(|| unreachable!());
        let guard = executor_arc.write().await;

        let backend = Arc::new(RecordingResumeBackend::default());
        let resumed = super::resume_waiting_task_input_with_backend(
            &context,
            super::ResumeTaskInput {
                user_id: owner_user_id,
                chat_id: ChatId(owner_user_id),
                task_id: &created.metadata.id,
                input: "approved".to_string(),
            },
            Arc::clone(&backend),
        )
        .await;
        assert!(matches!(resumed, Ok(false)));
        assert_no_resume_request(backend.as_ref()).await;

        drop(guard);
        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn text_resume_rejects_legacy_waiting_snapshot_without_pause_memory() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let owner_user_id = 744;
        let session_id = SessionId::from(owner_user_id);

        let created = task_registry.create(session_id).await;
        assert!(task_registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await
            .is_ok());
        let pending_input = waiting_pending_input();
        let waiting = task_registry
            .enter_waiting_input(&created.metadata.id, pending_input)
            .await;
        assert!(waiting.is_ok());
        let waiting = waiting.unwrap_or_else(|_| unreachable!());

        let mut snapshot = TaskSnapshot::new(
            waiting.record.metadata.clone(),
            session_id,
            "legacy waiting without memory".to_string(),
            waiting.event_sequence,
        );
        snapshot.pending_input = waiting.record.pending_input.clone();
        assert!(storage.save_task_snapshot(&snapshot).await.is_ok());

        let context = make_test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        );

        SESSION_REGISTRY.remove(&session_id).await;
        assert!(!SESSION_REGISTRY.contains(&session_id).await);

        let backend = Arc::new(RecordingResumeBackend::default());
        let resumed = super::resume_waiting_task_input_with_backend(
            &context,
            super::ResumeTaskInput {
                user_id: owner_user_id,
                chat_id: ChatId(owner_user_id),
                task_id: &created.metadata.id,
                input: "approved".to_string(),
            },
            Arc::clone(&backend),
        )
        .await;
        assert!(matches!(resumed, Ok(false)));
        assert_no_resume_request(backend.as_ref()).await;

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn text_resume_rejects_waiting_snapshot_with_corrupted_pause_memory() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let owner_user_id = 745;
        let session_id = SessionId::from(owner_user_id);

        let created = task_registry.create(session_id).await;
        assert!(task_registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await
            .is_ok());
        let pending_input = waiting_pending_input();
        let waiting = task_registry
            .enter_waiting_input(&created.metadata.id, pending_input)
            .await;
        assert!(waiting.is_ok());
        let waiting = waiting.unwrap_or_else(|_| unreachable!());

        let mut snapshot = TaskSnapshot::new(
            waiting.record.metadata.clone(),
            session_id,
            "waiting with corrupted memory".to_string(),
            waiting.event_sequence,
        );
        snapshot.pending_input = waiting.record.pending_input.clone();
        snapshot.agent_memory = Some("{broken-json".to_string());
        assert!(storage.save_task_snapshot(&snapshot).await.is_ok());

        let context = make_test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        );

        SESSION_REGISTRY.remove(&session_id).await;
        assert!(!SESSION_REGISTRY.contains(&session_id).await);

        let backend = Arc::new(RecordingResumeBackend::default());
        let resumed = super::resume_waiting_task_input_with_backend(
            &context,
            super::ResumeTaskInput {
                user_id: owner_user_id,
                chat_id: ChatId(owner_user_id),
                task_id: &created.metadata.id,
                input: "approved".to_string(),
            },
            Arc::clone(&backend),
        )
        .await;
        assert!(matches!(resumed, Ok(false)));
        assert_no_resume_request(backend.as_ref()).await;

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn text_resume_invalid_and_late_inputs_are_rejected_safely() {
        let pending_text = PendingTextInput {
            min_length: Some(2),
            max_length: Some(4),
            multiline: false,
        };
        assert!(super::validate_pending_text_resume_input("x", &pending_text).is_some());
        assert!(super::validate_pending_text_resume_input("x\ny", &pending_text).is_some());
        assert!(super::validate_pending_text_resume_input("good", &pending_text).is_none());

        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let owner_user_id = 741;
        let session_id = SessionId::from(owner_user_id);
        insert_test_session(session_id).await;

        let created = task_registry.create(session_id).await;
        assert!(task_registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await
            .is_ok());
        let waiting = task_registry
            .enter_waiting_input(&created.metadata.id, waiting_pending_input())
            .await;
        assert!(waiting.is_ok());
        let waiting = waiting.unwrap_or_else(|_| unreachable!());

        let mut snapshot = TaskSnapshot::new(
            waiting.record.metadata.clone(),
            session_id,
            "resume text duplicate".to_string(),
            waiting.event_sequence,
        );
        snapshot.pending_input = waiting.record.pending_input.clone();
        attach_waiting_snapshot_memory(&mut snapshot);
        assert!(storage.save_task_snapshot(&snapshot).await.is_ok());

        let context = Arc::new(make_test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        ));

        let first_resume = super::resume_waiting_task_input(
            &Bot::new("test-token"),
            context.as_ref(),
            super::ResumeTaskInput {
                user_id: owner_user_id,
                chat_id: ChatId(owner_user_id),
                task_id: &created.metadata.id,
                input: "done".to_string(),
            },
        )
        .await;
        assert!(matches!(first_resume, Ok(true)));

        let waited = timeout(Duration::from_secs(2), async {
            loop {
                if let Some(record) = task_registry.get(&created.metadata.id).await {
                    if record.metadata.state != TaskState::WaitingInput {
                        break;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(
            waited.is_ok(),
            "task did not leave waiting state after first text resume"
        );

        let late_resume = super::resume_waiting_task_input(
            &Bot::new("test-token"),
            context.as_ref(),
            super::ResumeTaskInput {
                user_id: owner_user_id,
                chat_id: ChatId(owner_user_id),
                task_id: &created.metadata.id,
                input: "redo".to_string(),
            },
        )
        .await;
        assert!(matches!(late_resume, Ok(false)));

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn poll_resume_valid_answer_resumes_task_and_cleans_poll_mapping() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let owner_user_id = 750;
        let session_id = SessionId::from(owner_user_id);
        insert_test_session(session_id).await;

        let created = task_registry.create(session_id).await;
        assert!(task_registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await
            .is_ok());
        let pending_input = waiting_choice_pending_input();
        let waiting = task_registry
            .enter_waiting_input(&created.metadata.id, pending_input.clone())
            .await;
        assert!(waiting.is_ok());
        let waiting = waiting.unwrap_or_else(|_| unreachable!());

        let mut snapshot = TaskSnapshot::new(
            waiting.record.metadata.clone(),
            session_id,
            "resume poll task".to_string(),
            waiting.event_sequence,
        );
        snapshot.pending_input = waiting.record.pending_input.clone();
        attach_waiting_snapshot_memory(&mut snapshot);
        assert!(storage.save_task_snapshot(&snapshot).await.is_ok());

        assert!(storage
            .save_pending_input_poll(&oxide_agent_core::storage::PendingInputPoll {
                task_id: created.metadata.id,
                request_id: pending_input.request_id,
                owner_user_id,
                poll_id: "poll-resume".to_string(),
                chat_id: owner_user_id,
                message_id: 14,
                answered: false,
                selected_option_ids: Vec::new(),
            })
            .await
            .is_ok());

        let context = Arc::new(make_test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        ));
        SESSION_REGISTRY.remove(&session_id).await;
        assert!(!SESSION_REGISTRY.contains(&session_id).await);

        let answer = build_poll_answer("poll-resume", owner_user_id, &[0]);

        let handled =
            super::handle_pending_input_poll_answer(Bot::new("test-token"), answer, context).await;
        assert!(handled.is_ok());

        let waited = timeout(Duration::from_secs(2), async {
            loop {
                if let Some(record) = task_registry.get(&created.metadata.id).await {
                    if record.metadata.state != TaskState::WaitingInput {
                        assert!(record.pending_input.is_none());
                        break;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(
            waited.is_ok(),
            "task did not leave waiting state after poll resume"
        );
        let poll_by_id = storage.load_pending_input_poll_by_id("poll-resume").await;
        let poll_by_task = storage
            .load_pending_input_poll_by_task(created.metadata.id)
            .await;
        assert!(poll_by_id.is_ok());
        assert!(poll_by_task.is_ok());
        assert!(poll_by_id.ok().flatten().is_none());
        assert!(poll_by_task.ok().flatten().is_none());
        assert!(SESSION_REGISTRY.contains(&session_id).await);

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn poll_resume_cold_restart_preserves_original_task_for_backend_request() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let owner_user_id = 751;
        let session_id = SessionId::from(owner_user_id);

        let created = task_registry.create(session_id).await;
        assert!(task_registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await
            .is_ok());
        let pending_input = waiting_choice_pending_input();
        let waiting = task_registry
            .enter_waiting_input(&created.metadata.id, pending_input)
            .await;
        assert!(waiting.is_ok());
        let waiting = waiting.unwrap_or_else(|_| unreachable!());

        let mut snapshot = TaskSnapshot::new(
            waiting.record.metadata.clone(),
            session_id,
            "resume poll task original".to_string(),
            waiting.event_sequence,
        );
        snapshot.pending_input = waiting.record.pending_input.clone();
        attach_waiting_snapshot_memory(&mut snapshot);
        assert!(storage.save_task_snapshot(&snapshot).await.is_ok());

        let context = make_test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        );

        SESSION_REGISTRY.remove(&session_id).await;
        assert!(!SESSION_REGISTRY.contains(&session_id).await);

        let resume_input = super::encode_poll_resume_input(&[0]);
        let backend = Arc::new(RecordingResumeBackend::default());
        let resumed = super::resume_waiting_task_input_with_backend(
            &context,
            super::ResumeTaskInput {
                user_id: owner_user_id,
                chat_id: ChatId(owner_user_id),
                task_id: &created.metadata.id,
                input: resume_input.clone(),
            },
            Arc::clone(&backend),
        )
        .await;
        assert!(matches!(resumed, Ok(true)));

        let request = wait_for_resume_request(backend.as_ref()).await;
        assert_eq!(request.task, "resume poll task original");
        assert_eq!(request.resume_input.as_deref(), Some(resume_input.as_str()));
        assert!(SESSION_REGISTRY.contains(&session_id).await);

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn poll_resume_failure_keeps_captured_mapping_and_answer() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let owner_user_id = 760;
        let orphaned_task_id = TaskMetadata::new().id;

        let pending_poll = oxide_agent_core::storage::PendingInputPoll {
            task_id: orphaned_task_id,
            request_id: "choice-request".to_string(),
            owner_user_id,
            poll_id: "poll-resume-failure".to_string(),
            chat_id: owner_user_id,
            message_id: 15,
            answered: true,
            selected_option_ids: vec![1],
        };
        assert!(storage.save_pending_input_poll(&pending_poll).await.is_ok());

        let context = Arc::new(make_test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            task_runtime,
        ));

        let resumed = super::resume_task_from_consumed_poll_answer(
            &Bot::new("test-token"),
            context.as_ref(),
            &pending_poll,
            &[1],
        )
        .await;
        assert!(matches!(
            resumed,
            Ok(super::ConsumedPollResumeOutcome::Deferred)
        ));

        let stored = storage
            .load_pending_input_poll_by_id("poll-resume-failure")
            .await;
        assert!(stored.is_ok());
        assert!(matches!(
            stored.ok().flatten(),
            Some(poll) if poll.answered && poll.selected_option_ids == vec![1]
        ));
    }

    #[tokio::test]
    async fn waiting_choice_poll_delivery_retries_captured_answer_until_resume_succeeds() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let owner_user_id = 800;
        let session_id = SessionId::from(owner_user_id);
        let context = Arc::new(make_test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        ));
        let created = task_registry.create(session_id).await;
        assert!(task_registry
            .update_state(&created.metadata.id, TaskState::Running)
            .await
            .is_ok());
        let pending_input = waiting_choice_pending_input();
        let waiting = task_registry
            .enter_waiting_input(&created.metadata.id, pending_input.clone())
            .await;
        assert!(waiting.is_ok());
        let waiting = waiting.unwrap_or_else(|_| unreachable!());

        assert!(storage
            .save_pending_input_poll(&oxide_agent_core::storage::PendingInputPoll {
                task_id: created.metadata.id,
                request_id: pending_input.request_id,
                owner_user_id,
                poll_id: "poll-answered".to_string(),
                chat_id: owner_user_id,
                message_id: 15,
                answered: true,
                selected_option_ids: vec![0],
            })
            .await
            .is_ok());

        let delivered = super::deliver_waiting_choice_poll_if_needed(
            &Bot::new("test-token"),
            ChatId(owner_user_id),
            owner_user_id,
            context.as_ref(),
            &waiting.record,
        )
        .await;

        assert!(matches!(delivered, Ok(true)));

        let still_waiting = task_registry.get(&created.metadata.id).await;
        assert!(matches!(
            still_waiting,
            Some(record) if record.metadata.state == TaskState::WaitingInput
        ));

        let mut snapshot = TaskSnapshot::new(
            waiting.record.metadata.clone(),
            session_id,
            "resume delivered answer".to_string(),
            waiting.event_sequence,
        );
        snapshot.pending_input = waiting.record.pending_input.clone();
        attach_waiting_snapshot_memory(&mut snapshot);
        assert!(storage.save_task_snapshot(&snapshot).await.is_ok());

        insert_test_session(session_id).await;

        let delivered_retry = super::deliver_waiting_choice_poll_if_needed(
            &Bot::new("test-token"),
            ChatId(owner_user_id),
            owner_user_id,
            context.as_ref(),
            &waiting.record,
        )
        .await;

        assert!(matches!(delivered_retry, Ok(true)));

        let resumed = timeout(Duration::from_secs(2), async {
            loop {
                if let Some(record) = task_registry.get(&created.metadata.id).await {
                    if record.metadata.state != TaskState::WaitingInput {
                        assert!(record.pending_input.is_none());
                        break;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(
            resumed.is_ok(),
            "task did not leave waiting state after replaying captured poll answer"
        );

        let poll_by_id = storage.load_pending_input_poll_by_id("poll-answered").await;
        let poll_by_task = storage
            .load_pending_input_poll_by_task(created.metadata.id)
            .await;
        assert!(poll_by_id.is_ok());
        assert!(poll_by_task.is_ok());
        assert!(poll_by_id.ok().flatten().is_none());
        assert!(poll_by_task.ok().flatten().is_none());

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_submit_vs_exit_race_blocks_exit_before_executor_lock() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let session_id = SessionId::from(46);
        let user_id = session_id.as_i64();
        insert_test_session(session_id).await;

        let started = Arc::new(Notify::new());
        let allow_executor_lock = Arc::new(Notify::new());
        let entered_executor = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let backend = Arc::new(DeferredLockBackend {
            started: Arc::clone(&started),
            allow_executor_lock: Arc::clone(&allow_executor_lock),
            entered_executor: Arc::clone(&entered_executor),
            released: Arc::clone(&released),
        });

        let submit_result = task_runtime
            .submit_task(session_id, "exit race".to_string(), backend)
            .await;
        assert!(submit_result.is_ok());
        assert!(timeout(Duration::from_secs(1), started.notified())
            .await
            .is_ok());
        wait_for_active_runtime_task(&task_runtime, session_id).await;

        let storage_provider = Arc::clone(&storage) as Arc<dyn StorageProvider>;
        let exit_outcome = task_runtime
            .exit_session(session_id, user_id, &storage_provider)
            .await;
        assert!(matches!(exit_outcome, ExitSessionOutcome::BlockedByTask));
        assert!(SESSION_REGISTRY.contains(&session_id).await);
        assert!(storage.saved_memory_users.lock().await.is_empty());
        assert!(
            timeout(Duration::from_millis(50), entered_executor.notified())
                .await
                .is_err()
        );

        let cancelled = task_runtime.cancel_task_for_session(session_id).await;
        assert!(cancelled.is_ok());
        allow_executor_lock.notify_one();
        assert!(timeout(Duration::from_secs(1), released.notified())
            .await
            .is_ok());
        wait_for_runtime_task_completion(&task_runtime, session_id).await;

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_submit_vs_reset_race_waits_for_runtime_task_before_reset() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let session_id = SessionId::from(47);
        insert_test_session(session_id).await;

        let started = Arc::new(Notify::new());
        let allow_executor_lock = Arc::new(Notify::new());
        let entered_executor = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let backend = Arc::new(DeferredLockBackend {
            started: Arc::clone(&started),
            allow_executor_lock: Arc::clone(&allow_executor_lock),
            entered_executor: Arc::clone(&entered_executor),
            released: Arc::clone(&released),
        });

        let submit_result = task_runtime
            .submit_task(session_id, "reset race".to_string(), backend)
            .await;
        assert!(submit_result.is_ok());
        assert!(timeout(Duration::from_secs(1), started.notified())
            .await
            .is_ok());
        wait_for_active_runtime_task(&task_runtime, session_id).await;

        let mut reset_task = {
            let task_runtime = Arc::clone(&task_runtime);
            tokio::spawn(async move { task_runtime.cancel_and_reset_session(session_id).await })
        };

        assert!(
            timeout(Duration::from_millis(50), entered_executor.notified())
                .await
                .is_err()
        );
        let early_reset_result = match timeout(Duration::from_millis(50), &mut reset_task).await {
            Ok(result) => Some(unwrap_join_result(result)),
            Err(_) => None,
        };

        allow_executor_lock.notify_one();
        assert!(timeout(Duration::from_secs(1), released.notified())
            .await
            .is_ok());

        let reset_result = if let Some(result) = early_reset_result {
            result
        } else {
            unwrap_join_result(reset_task.await)
        };
        assert!(matches!(reset_result, Ok(SessionResetOutcome::Reset)));

        let executor_arc = SESSION_REGISTRY.get(&session_id).await;
        assert!(executor_arc.is_some());
        let executor_arc = executor_arc.unwrap_or_else(|| unreachable!());
        let executor = executor_arc.read().await;
        assert!(executor.last_task().is_none());
        assert!(executor.session().memory.todos.items.is_empty());

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_submit_vs_destructive_actions_race_blocks_before_executor_lock() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let context = make_test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        );
        let session_id = SessionId::from(48);
        let user_id = session_id.as_i64();
        insert_test_session(session_id).await;

        let started = Arc::new(Notify::new());
        let allow_executor_lock = Arc::new(Notify::new());
        let entered_executor = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let backend = Arc::new(DeferredLockBackend {
            started: Arc::clone(&started),
            allow_executor_lock: Arc::clone(&allow_executor_lock),
            entered_executor: Arc::clone(&entered_executor),
            released: Arc::clone(&released),
        });

        let submit_result = task_runtime
            .submit_task(session_id, "destructive race".to_string(), backend)
            .await;
        assert!(submit_result.is_ok());
        assert!(timeout(Duration::from_secs(1), started.notified())
            .await
            .is_ok());
        wait_for_active_runtime_task(&task_runtime, session_id).await;

        let storage_provider = Arc::clone(&storage) as Arc<dyn StorageProvider>;
        let clear_outcome = task_runtime
            .clear_memory(session_id, user_id, &storage_provider)
            .await;
        assert!(matches!(clear_outcome, ClearMemoryOutcome::BlockedByTask));
        assert!(storage.cleared_memory_users.lock().await.is_empty());

        let recreate_outcome = task_runtime
            .recreate_container(session_id, user_id, &context)
            .await;
        assert!(matches!(
            recreate_outcome,
            RecreateContainerOutcome::BlockedByTask
        ));
        assert!(SESSION_REGISTRY.contains(&session_id).await);
        assert!(
            timeout(Duration::from_millis(50), entered_executor.notified())
                .await
                .is_err()
        );

        let cancelled = task_runtime.cancel_task_for_session(session_id).await;
        assert!(cancelled.is_ok());
        allow_executor_lock.notify_one();
        assert!(timeout(Duration::from_secs(1), released.notified())
            .await
            .is_ok());
        wait_for_runtime_task_completion(&task_runtime, session_id).await;

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_retry_without_loop_detection_submits_saved_task() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        );
        let session_id = SessionId::from(50);
        let user_id = session_id.as_i64();
        insert_test_session(session_id).await;

        let (llm, settings) = retry_runtime_client();

        let started = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let backend = Arc::new(LockingBackend {
            started: Arc::clone(&started),
            released: Arc::clone(&released),
        });

        let retry_result = task_runtime
            .retry_task_without_loop_detection(
                user_id,
                &llm,
                &(Arc::clone(&storage) as Arc<dyn StorageProvider>),
                &settings,
                backend,
            )
            .await;
        assert!(matches!(retry_result, Ok(RetryTaskOutcome::Submitted)));
        assert!(timeout(Duration::from_secs(1), started.notified())
            .await
            .is_ok());
        wait_for_active_runtime_task(&task_runtime, session_id).await;

        let cancelled = task_runtime.cancel_task_for_session(session_id).await;
        assert!(cancelled.is_ok());
        assert!(timeout(Duration::from_secs(1), released.notified())
            .await
            .is_ok());
        wait_for_runtime_task_completion(&task_runtime, session_id).await;

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_retry_vs_reset_race_waits_for_admission_and_resets_created_task() {
        let snapshot_save_started = Arc::new(Notify::new());
        let release_snapshot_save = Arc::new(Notify::new());
        let storage = Arc::new(TestStorage::with_blocked_first_task_snapshot_save(
            Arc::clone(&snapshot_save_started),
            Arc::clone(&release_snapshot_save),
        ));
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let session_id = SessionId::from(52);
        let user_id = session_id.as_i64();
        insert_test_session(session_id).await;

        let agent_settings = settings_without_llm_providers();
        let llm_settings = Arc::new(agent_settings.clone());
        let llm = Arc::new(LlmClient::new(&llm_settings));
        let settings = Arc::new(BotSettings::new(
            agent_settings,
            TelegramSettings::default(),
        ));

        let released = Arc::new(Notify::new());
        let backend = Arc::new(LockingBackend {
            started: Arc::new(Notify::new()),
            released: Arc::clone(&released),
        });

        let retry_task = spawn_retry_without_loop_detection(
            Arc::clone(&task_runtime),
            user_id,
            Arc::clone(&llm),
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&settings),
            Arc::clone(&backend),
        );

        assert!(
            timeout(Duration::from_secs(1), snapshot_save_started.notified())
                .await
                .is_ok()
        );
        assert!(task_registry
            .latest_non_terminal_by_session(&session_id)
            .await
            .is_some());
        assert!(task_runtime
            .active_task_for_session(session_id)
            .await
            .is_some());

        let mut reset_task = {
            let task_runtime = Arc::clone(&task_runtime);
            tokio::spawn(async move { task_runtime.cancel_and_reset_session(session_id).await })
        };

        assert!(timeout(Duration::from_millis(50), &mut reset_task)
            .await
            .is_err());

        release_snapshot_save.notify_one();

        let retry_result = unwrap_join_result(retry_task.await);
        assert!(matches!(retry_result, Ok(RetryTaskOutcome::Submitted)));

        let reset_result = unwrap_join_result(reset_task.await);
        assert!(matches!(reset_result, Ok(SessionResetOutcome::Reset)));
        wait_for_runtime_task_completion(&task_runtime, session_id).await;

        let session_records = task_registry.list_by_session(&session_id).await;
        assert_eq!(session_records.len(), 1);
        assert_eq!(session_records[0].metadata.state, TaskState::Cancelled);

        let executor_arc = SESSION_REGISTRY.get(&session_id).await;
        assert!(executor_arc.is_some());
        let executor_arc = executor_arc.unwrap_or_else(|| unreachable!());
        let executor = executor_arc.read().await;
        assert!(executor.last_task().is_none());
        assert!(executor.session().memory.todos.items.is_empty());

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_retry_vs_exit_race_blocks_exit_before_executor_lock() {
        let snapshot_save_started = Arc::new(Notify::new());
        let release_snapshot_save = Arc::new(Notify::new());
        let storage = Arc::new(TestStorage::with_blocked_first_task_snapshot_save(
            Arc::clone(&snapshot_save_started),
            Arc::clone(&release_snapshot_save),
        ));
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let session_id = SessionId::from(53);
        let user_id = session_id.as_i64();
        insert_test_session(session_id).await;

        let (llm, settings) = retry_runtime_client();

        let started = Arc::new(Notify::new());
        let allow_executor_lock = Arc::new(Notify::new());
        let entered_executor = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let backend = Arc::new(DeferredLockBackend {
            started: Arc::clone(&started),
            allow_executor_lock: Arc::clone(&allow_executor_lock),
            entered_executor: Arc::clone(&entered_executor),
            released: Arc::clone(&released),
        });

        let retry_task = spawn_retry_without_loop_detection(
            Arc::clone(&task_runtime),
            user_id,
            Arc::clone(&llm),
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&settings),
            Arc::clone(&backend),
        );

        assert!(
            timeout(Duration::from_secs(1), snapshot_save_started.notified())
                .await
                .is_ok()
        );

        let mut exit_task = {
            let task_runtime = Arc::clone(&task_runtime);
            let storage_provider = Arc::clone(&storage) as Arc<dyn StorageProvider>;
            tokio::spawn(async move {
                task_runtime
                    .exit_session(session_id, user_id, &storage_provider)
                    .await
            })
        };

        assert!(timeout(Duration::from_millis(50), &mut exit_task)
            .await
            .is_err());

        release_snapshot_save.notify_one();

        let retry_result = unwrap_join_result(retry_task.await);
        assert!(matches!(retry_result, Ok(RetryTaskOutcome::Submitted)));
        assert!(timeout(Duration::from_secs(1), started.notified())
            .await
            .is_ok());
        wait_for_active_runtime_task(&task_runtime, session_id).await;

        let exit_result = unwrap_join_result(exit_task.await);
        assert!(matches!(exit_result, ExitSessionOutcome::BlockedByTask));
        assert!(SESSION_REGISTRY.contains(&session_id).await);
        assert!(storage.saved_memory_users.lock().await.is_empty());
        assert!(
            timeout(Duration::from_millis(50), entered_executor.notified())
                .await
                .is_err()
        );

        let cancelled = task_runtime.cancel_task_for_session(session_id).await;
        assert!(cancelled.is_ok());
        allow_executor_lock.notify_one();
        assert!(timeout(Duration::from_secs(1), released.notified())
            .await
            .is_ok());
        wait_for_runtime_task_completion(&task_runtime, session_id).await;

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_retry_vs_destructive_actions_race_blocks_before_executor_lock() {
        let snapshot_save_started = Arc::new(Notify::new());
        let release_snapshot_save = Arc::new(Notify::new());
        let storage = Arc::new(TestStorage::with_blocked_first_task_snapshot_save(
            Arc::clone(&snapshot_save_started),
            Arc::clone(&release_snapshot_save),
        ));
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = Arc::new(AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        ));
        let context = make_test_context(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_runtime),
        );
        let session_id = SessionId::from(54);
        let user_id = session_id.as_i64();
        insert_test_session(session_id).await;

        let (llm, settings) = retry_runtime_client();

        let started = Arc::new(Notify::new());
        let allow_executor_lock = Arc::new(Notify::new());
        let entered_executor = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let backend = Arc::new(DeferredLockBackend {
            started: Arc::clone(&started),
            allow_executor_lock: Arc::clone(&allow_executor_lock),
            entered_executor: Arc::clone(&entered_executor),
            released: Arc::clone(&released),
        });

        let retry_task = spawn_retry_without_loop_detection(
            Arc::clone(&task_runtime),
            user_id,
            Arc::clone(&llm),
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&settings),
            Arc::clone(&backend),
        );

        assert!(
            timeout(Duration::from_secs(1), snapshot_save_started.notified())
                .await
                .is_ok()
        );

        let mut clear_task = {
            let task_runtime = Arc::clone(&task_runtime);
            let storage_provider = Arc::clone(&storage) as Arc<dyn StorageProvider>;
            tokio::spawn(async move {
                task_runtime
                    .clear_memory(session_id, user_id, &storage_provider)
                    .await
            })
        };
        let mut recreate_task = {
            let task_runtime = Arc::clone(&task_runtime);
            let context = context.clone();
            tokio::spawn(async move {
                task_runtime
                    .recreate_container(session_id, user_id, &context)
                    .await
            })
        };

        assert!(timeout(Duration::from_millis(50), &mut clear_task)
            .await
            .is_err());
        assert!(timeout(Duration::from_millis(50), &mut recreate_task)
            .await
            .is_err());

        release_snapshot_save.notify_one();

        let retry_result = unwrap_join_result(retry_task.await);
        assert!(matches!(retry_result, Ok(RetryTaskOutcome::Submitted)));
        assert!(timeout(Duration::from_secs(1), started.notified())
            .await
            .is_ok());
        wait_for_active_runtime_task(&task_runtime, session_id).await;

        let clear_result = unwrap_join_result(clear_task.await);
        assert!(matches!(clear_result, ClearMemoryOutcome::BlockedByTask));
        assert!(storage.cleared_memory_users.lock().await.is_empty());

        let recreate_result = unwrap_join_result(recreate_task.await);
        assert!(matches!(
            recreate_result,
            RecreateContainerOutcome::BlockedByTask
        ));
        assert!(SESSION_REGISTRY.contains(&session_id).await);
        assert!(
            timeout(Duration::from_millis(50), entered_executor.notified())
                .await
                .is_err()
        );

        let cancelled = task_runtime.cancel_task_for_session(session_id).await;
        assert!(cancelled.is_ok());
        allow_executor_lock.notify_one();
        assert!(timeout(Duration::from_secs(1), released.notified())
            .await
            .is_ok());
        wait_for_runtime_task_completion(&task_runtime, session_id).await;

        SESSION_REGISTRY.remove(&session_id).await;
    }

    #[tokio::test]
    async fn task_runtime_save_memory_after_task_persists_session_memory() {
        let storage = Arc::new(TestStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());
        let task_runtime = AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        );
        let session_id = SessionId::from(51);
        let user_id = session_id.as_i64();
        insert_test_session(session_id).await;

        task_runtime
            .save_memory_after_task(
                session_id,
                user_id,
                &(Arc::clone(&storage) as Arc<dyn StorageProvider>),
            )
            .await;

        assert_eq!(
            storage.saved_memory_users.lock().await.as_slice(),
            &[user_id]
        );

        SESSION_REGISTRY.remove(&session_id).await;
    }
}
