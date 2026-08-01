use super::{
    SESSION_REGISTRY, cancel_status_inline_markup, finalize_cancel_status_if_needed,
    is_task_cancelled_error, save_memory_after_task, selected_model, send_agent_message,
    should_preserve_pending_file_input,
};
use crate::bot::agent_handlers::{
    media_route_unavailable_detail, preprocess_agent_message_input,
    send_multimodal_unavailable_message,
};
use crate::bot::agent_transport::TelegramAgentTransport;
use crate::bot::messaging::{
    replace_message_with_long_text, send_long_message_in_thread_with_final_markup,
};
use crate::bot::views::DefaultAgentView;
use anyhow::{Result, anyhow};
use oxide_agent_core::agent::{AgentExecutionOutcome, SessionId, progress::AgentEvent};
use oxide_agent_core::config::{AgentSettings, ModelInfo, get_agent_max_iterations};
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::sandbox::SandboxScope;
use oxide_agent_core::storage::StorageProvider;
use oxide_agent_runtime::{ProgressRuntimeConfig, spawn_progress_runtime};
use std::collections::HashMap;
use std::future::Future;
use std::sync::LazyLock;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use teloxide::prelude::*;
use teloxide::types::{MessageId, ParseMode, ThreadId};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

#[derive(Clone)]
pub(crate) struct AgentTaskContext {
    pub(crate) bot: Bot,
    pub(crate) msg: Message,
    pub(crate) storage: Arc<dyn StorageProvider>,
    pub(crate) llm: Arc<LlmClient>,
    pub(crate) agent_settings: Arc<AgentSettings>,
    pub(crate) context_key: String,
    pub(crate) agent_flow_id: String,
    pub(crate) sandbox_scope: SandboxScope,
    pub(crate) message_thread_id: Option<ThreadId>,
    pub(crate) use_inline_progress_controls: bool,
    pub(crate) use_inline_flow_controls: bool,
    pub(crate) attach_detach_enabled: bool,
    pub(crate) session_id: SessionId,
}

#[derive(Clone)]
pub(crate) struct RunAgentTaskTextContext {
    pub(crate) bot: Bot,
    pub(crate) chat_id: ChatId,
    pub(crate) session_id: SessionId,
    pub(crate) user_id: i64,
    pub(crate) task_text: String,
    pub(crate) storage: Arc<dyn StorageProvider>,
    pub(crate) agent_settings: Arc<AgentSettings>,
    pub(crate) context_key: String,
    pub(crate) agent_flow_id: String,
    pub(crate) message_thread_id: Option<ThreadId>,
    pub(crate) use_inline_progress_controls: bool,
    pub(crate) use_inline_flow_controls: bool,
    pub(crate) attach_detach_enabled: bool,
    pub(crate) progress_enabled: bool,
    pub(crate) silent_no_change_enabled: bool,
}

#[derive(Clone)]
pub(crate) struct RunUserInputResumeContext {
    pub(crate) bot: Bot,
    pub(crate) chat_id: ChatId,
    pub(crate) session_id: SessionId,
    pub(crate) user_id: i64,
    pub(crate) user_input: String,
    pub(crate) storage: Arc<dyn StorageProvider>,
    pub(crate) agent_settings: Arc<AgentSettings>,
    pub(crate) context_key: String,
    pub(crate) agent_flow_id: String,
    pub(crate) message_thread_id: Option<ThreadId>,
    pub(crate) use_inline_progress_controls: bool,
    pub(crate) use_inline_flow_controls: bool,
    pub(crate) attach_detach_enabled: bool,
}

#[derive(Clone)]
pub(crate) struct RunManualCompactionContext {
    pub(crate) bot: Bot,
    pub(crate) chat_id: ChatId,
    pub(crate) session_id: SessionId,
    pub(crate) user_id: i64,
    pub(crate) storage: Arc<dyn StorageProvider>,
    pub(crate) context_key: String,
    pub(crate) agent_flow_id: String,
    pub(crate) message_thread_id: Option<ThreadId>,
    pub(crate) use_inline_progress_controls: bool,
    pub(crate) use_inline_flow_controls: bool,
    pub(crate) attach_detach_enabled: bool,
}

#[derive(Clone)]
struct TaskDeliveryContext {
    bot: Bot,
    chat_id: ChatId,
    session_id: SessionId,
    user_id: i64,
    storage: Arc<dyn StorageProvider>,
    context_key: String,
    agent_flow_id: String,
    message_thread_id: Option<ThreadId>,
    use_inline_progress_controls: bool,
    use_inline_flow_controls: bool,
    attach_detach_enabled: bool,
    progress_enabled: bool,
    silent_no_change_enabled: bool,
}

struct TaskProgressRuntime {
    progress_message_id: Option<MessageId>,
    progress_handle: tokio::task::JoinHandle<oxide_agent_core::agent::progress::ProgressState>,
    loop_notification_delivered: Arc<AtomicBool>,
    tx: tokio::sync::mpsc::Sender<AgentEvent>,
}

enum ModelOverrideUpdate {
    Keep,
    Set(Option<ModelInfo>),
}

struct TaskProgressDelivery {
    message_id: Option<MessageId>,
    loop_notification_delivered: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CompletedResponseDeliveryPhase {
    #[default]
    Executing,
    Finalizing,
    TerminalSendStarted,
}

#[derive(Debug, Default)]
struct CompletedResponseDeliveryState {
    phase: CompletedResponseDeliveryPhase,
    pending_followups: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CompletedResponseDeliveryAction {
    Deliver,
    Restart(Vec<String>),
}

static COMPLETED_RESPONSE_DELIVERY: LazyLock<
    Mutex<HashMap<SessionId, CompletedResponseDeliveryState>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) const NO_USER_VISIBLE_CHANGE_SENTINEL: &str = "<OXIDE_NO_USER_VISIBLE_CHANGE>";

pub(crate) fn is_no_user_visible_change_response(response: &str) -> bool {
    response.trim() == NO_USER_VISIBLE_CHANGE_SENTINEL
}

impl From<&RunAgentTaskTextContext> for TaskDeliveryContext {
    fn from(value: &RunAgentTaskTextContext) -> Self {
        Self {
            bot: value.bot.clone(),
            chat_id: value.chat_id,
            session_id: value.session_id,
            user_id: value.user_id,
            storage: value.storage.clone(),
            context_key: value.context_key.clone(),
            agent_flow_id: value.agent_flow_id.clone(),
            message_thread_id: value.message_thread_id,
            use_inline_progress_controls: value.use_inline_progress_controls,
            use_inline_flow_controls: value.use_inline_flow_controls,
            attach_detach_enabled: value.attach_detach_enabled,
            progress_enabled: value.progress_enabled,
            silent_no_change_enabled: value.silent_no_change_enabled,
        }
    }
}

impl From<&RunUserInputResumeContext> for TaskDeliveryContext {
    fn from(value: &RunUserInputResumeContext) -> Self {
        Self {
            bot: value.bot.clone(),
            chat_id: value.chat_id,
            session_id: value.session_id,
            user_id: value.user_id,
            storage: value.storage.clone(),
            context_key: value.context_key.clone(),
            agent_flow_id: value.agent_flow_id.clone(),
            message_thread_id: value.message_thread_id,
            use_inline_progress_controls: value.use_inline_progress_controls,
            use_inline_flow_controls: value.use_inline_flow_controls,
            attach_detach_enabled: value.attach_detach_enabled,
            progress_enabled: true,
            silent_no_change_enabled: false,
        }
    }
}

impl From<&RunManualCompactionContext> for TaskDeliveryContext {
    fn from(value: &RunManualCompactionContext) -> Self {
        Self {
            bot: value.bot.clone(),
            chat_id: value.chat_id,
            session_id: value.session_id,
            user_id: value.user_id,
            storage: value.storage.clone(),
            context_key: value.context_key.clone(),
            agent_flow_id: value.agent_flow_id.clone(),
            message_thread_id: value.message_thread_id,
            use_inline_progress_controls: value.use_inline_progress_controls,
            use_inline_flow_controls: value.use_inline_flow_controls,
            attach_detach_enabled: value.attach_detach_enabled,
            progress_enabled: true,
            silent_no_change_enabled: false,
        }
    }
}

pub(crate) fn spawn_agent_task(ctx: AgentTaskContext) {
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

pub(crate) async fn run_agent_task(ctx: AgentTaskContext) -> Result<()> {
    let user_id = ctx.msg.from.as_ref().map_or(0, |u| u.id.0.cast_signed());
    let chat_id = ctx.msg.chat.id;
    let preserve_binary_uploads = should_preserve_pending_file_input(&ctx.session_id).await;
    let task_text = match preprocess_agent_message_input(
        &ctx.bot,
        &ctx.msg,
        &ctx.llm,
        &ctx.agent_settings,
        &ctx.sandbox_scope,
        preserve_binary_uploads,
    )
    .await
    {
        Ok(text) => text,
        Err(err) => {
            if let Some(detail) = media_route_unavailable_detail(&err) {
                send_multimodal_unavailable_message(
                    &ctx.bot,
                    chat_id,
                    ctx.message_thread_id,
                    Some(&detail),
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

    run_agent_task_with_text(RunAgentTaskTextContext {
        bot: ctx.bot,
        chat_id,
        session_id: ctx.session_id,
        user_id,
        task_text,
        storage: ctx.storage,
        agent_settings: ctx.agent_settings,
        context_key: ctx.context_key,
        agent_flow_id: ctx.agent_flow_id,
        message_thread_id: ctx.message_thread_id,
        use_inline_progress_controls: ctx.use_inline_progress_controls,
        use_inline_flow_controls: ctx.use_inline_flow_controls,
        attach_detach_enabled: ctx.attach_detach_enabled,
        progress_enabled: true,
        silent_no_change_enabled: false,
    })
    .await
}

pub(crate) async fn run_agent_task_with_text(ctx: RunAgentTaskTextContext) -> Result<()> {
    let model = selected_model(
        &ctx.storage,
        &ctx.agent_settings,
        ctx.user_id,
        &ctx.context_key,
    )
    .await?;
    let delivery_ctx = TaskDeliveryContext::from(&ctx);
    let session_id = ctx.session_id;
    let task_text = ctx.task_text;
    run_task_execution(delivery_ctx, move |progress_tx| async move {
        execute_agent_task(session_id, &task_text, model, progress_tx).await
    })
    .await
}

pub(crate) async fn run_agent_task_continuation_with_text(
    ctx: RunAgentTaskTextContext,
) -> Result<()> {
    let model = selected_model(
        &ctx.storage,
        &ctx.agent_settings,
        ctx.user_id,
        &ctx.context_key,
    )
    .await?;
    let delivery_ctx = TaskDeliveryContext::from(&ctx);
    let session_id = ctx.session_id;
    let user_context = ctx.task_text;
    run_task_execution(delivery_ctx, move |progress_tx| async move {
        execute_agent_task_continuation(
            session_id,
            vec![user_context],
            ModelOverrideUpdate::Set(model),
            progress_tx,
        )
        .await
    })
    .await
}

pub(crate) async fn run_user_input_resume(ctx: RunUserInputResumeContext) -> Result<()> {
    let model = selected_model(
        &ctx.storage,
        &ctx.agent_settings,
        ctx.user_id,
        &ctx.context_key,
    )
    .await?;
    let delivery_ctx = TaskDeliveryContext::from(&ctx);
    let session_id = ctx.session_id;
    let user_input = ctx.user_input;
    run_task_execution(delivery_ctx, move |progress_tx| async move {
        execute_user_input_resume(session_id, user_input, model, progress_tx).await
    })
    .await
}

pub(crate) fn spawn_manual_compaction_task(ctx: RunManualCompactionContext) {
    tokio::spawn(async move {
        let error_bot = ctx.bot.clone();
        let chat_id = ctx.chat_id;
        let message_thread_id = ctx.message_thread_id;
        if let Err(error) = run_manual_compaction(ctx).await {
            let mut req = error_bot
                .send_message(chat_id, DefaultAgentView::error_message(&error.to_string()));
            if let Some(thread_id) = message_thread_id {
                req = req.message_thread_id(thread_id);
            }
            let _ = req.await;
        }
    });
}

pub(crate) async fn run_manual_compaction(ctx: RunManualCompactionContext) -> Result<()> {
    let delivery_ctx = TaskDeliveryContext::from(&ctx);
    let runtime = start_task_progress_runtime_with_text(
        &delivery_ctx,
        DefaultAgentView::context_compacting(),
    )
    .await?;
    let TaskProgressRuntime {
        progress_message_id,
        progress_handle,
        loop_notification_delivered: _,
        tx,
    } = runtime;
    let progress_message_id = progress_message_id.expect("progress message id must exist");
    let result = execute_manual_compaction(ctx.session_id, Some(tx)).await;
    finish_task_progress_runtime(progress_handle).await;

    save_memory_after_task(
        ctx.session_id,
        ctx.user_id,
        &ctx.context_key,
        &ctx.agent_flow_id,
        &ctx.storage,
    )
    .await;

    deliver_manual_compaction_result(&delivery_ctx, result, progress_message_id).await
}

async fn run_task_execution<Exec, Fut>(ctx: TaskDeliveryContext, execute: Exec) -> Result<()>
where
    Exec: FnOnce(Option<tokio::sync::mpsc::Sender<AgentEvent>>) -> Fut,
    Fut: Future<Output = Result<AgentExecutionOutcome>>,
{
    let runtime = if ctx.progress_enabled {
        start_task_progress_runtime(&ctx).await?
    } else {
        start_silent_task_progress_runtime(&ctx)
    };
    let mut progress_message_id = runtime.progress_message_id;
    let mut progress_handle = Some(runtime.progress_handle);
    let mut loop_notification_delivered = runtime.loop_notification_delivered;
    let progress_tx = Some(runtime.tx);
    mark_completed_response_execution_started(ctx.session_id).await;
    // Keep sender ownership with the active execution pass so the progress runtime
    // can observe channel closure and terminate once the pass finishes.
    let mut result = execute(progress_tx).await;

    loop {
        let completed = matches!(result, Ok(AgentExecutionOutcome::Completed(_)));
        if completed {
            begin_completed_response_finalization(ctx.session_id).await;
        }

        let progress = {
            let handle = progress_handle
                .take()
                .expect("progress runtime handle must exist");
            finish_task_progress_runtime(handle).await;
            TaskProgressDelivery {
                message_id: progress_message_id,
                loop_notification_delivered: loop_notification_delivered.load(Ordering::Acquire),
            }
        };

        if completed {
            match prepare_completed_response_delivery(&ctx.session_id).await {
                CompletedResponseDeliveryAction::Restart(followups) => {
                    info!(
                        session_id = %ctx.session_id,
                        followup_count = followups.len(),
                        "Restarting task before completed response delivery"
                    );
                    let next_progress_tx = if ctx.progress_enabled {
                        let runtime = match restart_task_progress_runtime(
                            &ctx,
                            progress_message_id.expect("progress message id must exist"),
                        )
                        .await
                        {
                            Ok(runtime) => runtime,
                            Err(error) => {
                                clear_completed_response_delivery_state(&ctx.session_id).await;
                                return Err(error);
                            }
                        };
                        progress_message_id = runtime.progress_message_id;
                        progress_handle = Some(runtime.progress_handle);
                        loop_notification_delivered = runtime.loop_notification_delivered;
                        Some(runtime.tx)
                    } else {
                        let runtime = start_silent_task_progress_runtime(&ctx);
                        progress_message_id = runtime.progress_message_id;
                        progress_handle = Some(runtime.progress_handle);
                        loop_notification_delivered = runtime.loop_notification_delivered;
                        Some(runtime.tx)
                    };
                    result = execute_agent_task_continuation(
                        ctx.session_id,
                        followups,
                        ModelOverrideUpdate::Keep,
                        next_progress_tx,
                    )
                    .await;
                    continue;
                }
                CompletedResponseDeliveryAction::Deliver => {}
            }
        }

        save_memory_after_task(
            ctx.session_id,
            ctx.user_id,
            &ctx.context_key,
            &ctx.agent_flow_id,
            &ctx.storage,
        )
        .await;

        let replaces_progress_anchor = progress.message_id.is_some();
        let delivery_result = deliver_task_result(&ctx, result, progress).await;
        clear_completed_response_delivery_state(&ctx.session_id).await;
        if replaces_progress_anchor && let Err(error) = &delivery_result {
            warn!(error = %error, "Terminal Telegram progress replacement failed");
            return Ok(());
        }
        return delivery_result;
    }
}

async fn start_task_progress_runtime(ctx: &TaskDeliveryContext) -> Result<TaskProgressRuntime> {
    start_task_progress_runtime_with_text(ctx, DefaultAgentView::task_processing()).await
}

async fn restart_task_progress_runtime(
    ctx: &TaskDeliveryContext,
    progress_message_id: MessageId,
) -> Result<TaskProgressRuntime> {
    let progress_reply_markup = ctx
        .use_inline_progress_controls
        .then_some(crate::bot::views::progress_inline_keyboard());
    crate::bot::resilient::edit_message_resilient_with_markup(
        &ctx.bot,
        ctx.chat_id,
        progress_message_id,
        DefaultAgentView::task_processing(),
        Some(ParseMode::Html),
        progress_reply_markup.clone(),
    )
    .await?;

    Ok(bind_task_progress_runtime(ctx, progress_message_id))
}

async fn start_task_progress_runtime_with_text(
    ctx: &TaskDeliveryContext,
    initial_text: &str,
) -> Result<TaskProgressRuntime> {
    let progress_reply_markup = ctx
        .use_inline_progress_controls
        .then_some(crate::bot::views::progress_inline_keyboard());
    let progress_msg = crate::bot::resilient::send_message_resilient_with_thread_and_markup(
        &ctx.bot,
        ctx.chat_id,
        initial_text,
        Some(ParseMode::Html),
        ctx.message_thread_id,
        progress_reply_markup.clone().map(Into::into),
    )
    .await?;

    Ok(bind_task_progress_runtime(ctx, progress_msg.id))
}

fn bind_task_progress_runtime(
    ctx: &TaskDeliveryContext,
    progress_message_id: MessageId,
) -> TaskProgressRuntime {
    let max_iterations = get_agent_max_iterations();
    let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(100);
    let loop_notification_delivered = Arc::new(AtomicBool::new(false));
    let transport = TelegramAgentTransport::new(
        ctx.bot.clone(),
        ctx.chat_id,
        progress_message_id,
        ctx.message_thread_id,
        ctx.use_inline_progress_controls,
        Arc::clone(&loop_notification_delivered),
    );
    let cfg = ProgressRuntimeConfig::new(max_iterations);
    let progress_handle = spawn_progress_runtime(transport, rx, cfg);

    TaskProgressRuntime {
        progress_message_id: Some(progress_message_id),
        progress_handle,
        loop_notification_delivered,
        tx,
    }
}

fn start_silent_task_progress_runtime(ctx: &TaskDeliveryContext) -> TaskProgressRuntime {
    let max_iterations = get_agent_max_iterations();
    let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(100);
    let loop_notification_delivered = Arc::new(AtomicBool::new(false));
    let transport = TelegramAgentTransport::silent(
        ctx.bot.clone(),
        ctx.chat_id,
        ctx.message_thread_id,
        Arc::clone(&loop_notification_delivered),
    );
    let cfg = ProgressRuntimeConfig::new(max_iterations);
    let progress_handle = spawn_progress_runtime(transport, rx, cfg);

    TaskProgressRuntime {
        progress_message_id: None,
        progress_handle,
        loop_notification_delivered,
        tx,
    }
}

pub(crate) async fn mark_completed_response_execution_started(session_id: SessionId) {
    let mut states = COMPLETED_RESPONSE_DELIVERY.lock().await;
    states.insert(
        session_id,
        CompletedResponseDeliveryState {
            phase: CompletedResponseDeliveryPhase::Executing,
            pending_followups: Vec::new(),
        },
    );
}

pub(crate) async fn begin_completed_response_finalization(session_id: SessionId) {
    let mut states = COMPLETED_RESPONSE_DELIVERY.lock().await;
    let state = states.entry(session_id).or_default();
    state.phase = CompletedResponseDeliveryPhase::Finalizing;
}

pub(crate) async fn queue_followup_during_completed_response_delivery(
    session_id: &SessionId,
    content: String,
) -> bool {
    let mut states = COMPLETED_RESPONSE_DELIVERY.lock().await;
    let Some(state) = states.get_mut(session_id) else {
        return false;
    };

    if state.phase != CompletedResponseDeliveryPhase::Finalizing {
        return false;
    }

    state.pending_followups.push(content);
    true
}

pub(crate) async fn prepare_completed_response_delivery(
    session_id: &SessionId,
) -> CompletedResponseDeliveryAction {
    let mut states = COMPLETED_RESPONSE_DELIVERY.lock().await;
    let Some(state) = states.get_mut(session_id) else {
        return CompletedResponseDeliveryAction::Deliver;
    };

    if state.phase != CompletedResponseDeliveryPhase::Finalizing {
        return CompletedResponseDeliveryAction::Deliver;
    }

    if state.pending_followups.is_empty() {
        state.phase = CompletedResponseDeliveryPhase::TerminalSendStarted;
        return CompletedResponseDeliveryAction::Deliver;
    }

    state.phase = CompletedResponseDeliveryPhase::Executing;
    CompletedResponseDeliveryAction::Restart(std::mem::take(&mut state.pending_followups))
}

pub(crate) async fn clear_completed_response_delivery_state(session_id: &SessionId) {
    let mut states = COMPLETED_RESPONSE_DELIVERY.lock().await;
    states.remove(session_id);
}

async fn deliver_manual_compaction_result(
    ctx: &TaskDeliveryContext,
    result: Result<()>,
    progress_message_id: MessageId,
) -> Result<()> {
    let text = match result {
        Ok(()) => DefaultAgentView::context_compacted(true).to_string(),
        Err(error) => DefaultAgentView::error_message(&error.to_string()),
    };
    if let Err(error) = replace_message_with_long_text(
        &ctx.bot,
        ctx.chat_id,
        progress_message_id,
        &text,
        ctx.message_thread_id,
        None,
    )
    .await
    {
        warn!(error = %error, "Manual compaction progress replacement failed");
    }
    Ok(())
}

async fn finish_task_progress_runtime(
    progress_handle: tokio::task::JoinHandle<oxide_agent_core::agent::progress::ProgressState>,
) {
    match progress_handle.await {
        Ok(_) => {}
        Err(err) => {
            warn!(error = %err, "Progress runtime task failed");
        }
    }
}

async fn deliver_task_result(
    ctx: &TaskDeliveryContext,
    result: Result<AgentExecutionOutcome>,
    progress: TaskProgressDelivery,
) -> Result<()> {
    let cancelled = result.as_ref().err().is_some_and(is_task_cancelled_error);

    match result {
        Ok(AgentExecutionOutcome::Completed(response)) => {
            if should_suppress_completed_response(ctx, &response) {
                if let Some(message_id) = progress.message_id {
                    delete_progress_anchor(ctx, message_id).await;
                }
            } else {
                let final_markup = ctx
                    .use_inline_flow_controls
                    .then(|| {
                        crate::bot::views::agent_flow_inline_keyboard_with_options(
                            &ctx.agent_flow_id,
                            ctx.attach_detach_enabled,
                            ctx.use_inline_progress_controls,
                        )
                    })
                    .filter(|markup| !markup.inline_keyboard.is_empty());
                if let Some(message_id) = progress.message_id {
                    replace_message_with_long_text(
                        &ctx.bot,
                        ctx.chat_id,
                        message_id,
                        &response,
                        ctx.message_thread_id,
                        final_markup,
                    )
                    .await?;
                } else {
                    send_long_message_in_thread_with_final_markup(
                        &ctx.bot,
                        ctx.chat_id,
                        &response,
                        ctx.message_thread_id,
                        final_markup,
                    )
                    .await?;
                }
            }
        }
        Ok(AgentExecutionOutcome::WaitingForUserInput(request)) => {
            if let Some(message_id) = progress.message_id {
                replace_message_with_long_text(
                    &ctx.bot,
                    ctx.chat_id,
                    message_id,
                    &request.prompt,
                    ctx.message_thread_id,
                    None,
                )
                .await?;
            } else {
                send_long_message_in_thread_with_final_markup(
                    &ctx.bot,
                    ctx.chat_id,
                    &request.prompt,
                    ctx.message_thread_id,
                    None,
                )
                .await?;
            }
        }
        Err(error) => {
            if progress.loop_notification_delivered {
                return Ok(());
            }
            let error_text = DefaultAgentView::error_message(&error.to_string());
            if let Some(message_id) = progress.message_id {
                replace_message_with_long_text(
                    &ctx.bot,
                    ctx.chat_id,
                    message_id,
                    &error_text,
                    ctx.message_thread_id,
                    None,
                )
                .await?;
            } else {
                send_agent_message(
                    &ctx.bot,
                    ctx.chat_id,
                    DefaultAgentView::error_message(&error.to_string()),
                    crate::bot::OutboundThreadParams {
                        message_thread_id: ctx.message_thread_id,
                    },
                )
                .await?;
            }
        }
    }

    finalize_cancel_status_if_needed(
        &ctx.bot,
        ctx.session_id,
        ctx.chat_id,
        cancelled,
        cancel_status_inline_markup(
            ctx.use_inline_flow_controls,
            &ctx.agent_flow_id,
            ctx.attach_detach_enabled,
        ),
    )
    .await;

    Ok(())
}

async fn delete_progress_anchor(ctx: &TaskDeliveryContext, message_id: MessageId) {
    if let Err(error) = oxide_agent_core::utils::retry_transport_operation(|| async {
        ctx.bot
            .delete_message(ctx.chat_id, message_id)
            .await
            .map(|_| ())
            .map_err(|error| anyhow!("Telegram delete error: {error}"))
    })
    .await
    {
        warn!(error = %error, "Failed to delete silent Telegram progress anchor");
    }
}

fn should_suppress_completed_response(ctx: &TaskDeliveryContext, response: &str) -> bool {
    ctx.silent_no_change_enabled && is_no_user_visible_change_response(response)
}

async fn execute_agent_task(
    session_id: SessionId,
    task: &str,
    model: Option<ModelInfo>,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> Result<AgentExecutionOutcome> {
    let (executor_arc, cancellation_token) = SESSION_REGISTRY
        .execution_handles(&session_id)
        .await
        .ok_or_else(|| anyhow!("No agent session found"))?;

    let mut executor = executor_arc.write().await;
    debug!(
        session_id = %session_id,
        memory_messages = executor.session().memory.get_messages().len(),
        "Executor accessed for task execution"
    );

    if executor.is_timed_out() {
        executor.reset();
        return Err(anyhow!(
            "Previous session timed out. Starting a new session."
        ));
    }

    executor.set_model_override(model);
    executor.session_mut().cancellation_token = (*cancellation_token).clone();
    executor.execute(task, progress_tx).await
}

async fn execute_agent_task_continuation(
    session_id: SessionId,
    followups: Vec<String>,
    model_override: ModelOverrideUpdate,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> Result<AgentExecutionOutcome> {
    let (executor_arc, cancellation_token) = SESSION_REGISTRY
        .execution_handles(&session_id)
        .await
        .ok_or_else(|| anyhow!("No agent session found"))?;

    let mut executor = executor_arc.write().await;
    if executor.is_timed_out() {
        executor.reset();
        return Err(anyhow!(
            "Previous session timed out. Starting a new session."
        ));
    }

    for followup in followups {
        executor.enqueue_runtime_context(followup);
    }

    if let ModelOverrideUpdate::Set(model) = model_override {
        executor.set_model_override(model);
    }

    executor.session_mut().cancellation_token = (*cancellation_token).clone();
    executor.continue_after_runtime_context(progress_tx).await
}

pub(crate) async fn execute_user_input_resume(
    session_id: SessionId,
    user_input: String,
    model: Option<ModelInfo>,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> Result<AgentExecutionOutcome> {
    let (executor_arc, cancellation_token) = SESSION_REGISTRY
        .execution_handles(&session_id)
        .await
        .ok_or_else(|| anyhow!("No agent session found"))?;

    let mut executor = executor_arc.write().await;
    if executor.is_timed_out() {
        executor.reset();
        return Err(anyhow!(
            "Previous session timed out. Starting a new session."
        ));
    }

    executor.set_model_override(model);
    executor.session_mut().cancellation_token = (*cancellation_token).clone();
    executor
        .resume_after_user_input(user_input, progress_tx)
        .await
}

pub(crate) async fn execute_manual_compaction(
    session_id: SessionId,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> Result<()> {
    let (executor_arc, cancellation_token) = SESSION_REGISTRY
        .execution_handles(&session_id)
        .await
        .ok_or_else(|| anyhow!("No agent session found"))?;

    let mut executor = executor_arc.write().await;
    if executor.is_timed_out() {
        executor.reset();
        return Err(anyhow!(
            "Previous session timed out. Starting a new session."
        ));
    }

    executor.session_mut().cancellation_token = (*cancellation_token).clone();
    executor.compact_current_context(progress_tx).await
}
