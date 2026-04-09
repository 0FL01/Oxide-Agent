use super::{
    cancel_status_inline_markup, finalize_cancel_status_if_needed, is_task_cancelled_error,
    save_memory_after_task, send_agent_message, should_preserve_pending_file_input,
    SESSION_REGISTRY,
};
use crate::bot::agent_handlers::{
    preprocess_agent_message_input, send_multimodal_unavailable_message,
};
use crate::bot::agent_transport::TelegramAgentTransport;
use crate::bot::messaging::send_long_message_in_thread_with_final_markup;
use crate::bot::progress_render::render_progress_html;
use crate::bot::views::{AgentView, DefaultAgentView};
use anyhow::{anyhow, Result};
use oxide_agent_core::agent::{
    progress::AgentEvent, AgentExecutionOutcome, CompactionOutcome, SessionId,
};
use oxide_agent_core::config::get_agent_max_iterations;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::sandbox::SandboxScope;
use oxide_agent_core::storage::StorageProvider;
use oxide_agent_runtime::{spawn_progress_runtime, ProgressRuntimeConfig};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::LazyLock;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, MessageId, ParseMode, ThreadId};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

#[derive(Clone)]
pub(crate) struct AgentTaskContext {
    pub(crate) bot: Bot,
    pub(crate) msg: Message,
    pub(crate) storage: Arc<dyn StorageProvider>,
    pub(crate) llm: Arc<LlmClient>,
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
    pub(crate) context_key: String,
    pub(crate) agent_flow_id: String,
    pub(crate) message_thread_id: Option<ThreadId>,
    pub(crate) use_inline_progress_controls: bool,
    pub(crate) use_inline_flow_controls: bool,
    pub(crate) attach_detach_enabled: bool,
}

#[derive(Clone)]
pub(crate) struct RunApprovedSshResumeContext {
    pub(crate) bot: Bot,
    pub(crate) chat_id: ChatId,
    pub(crate) session_id: SessionId,
    pub(crate) user_id: i64,
    pub(crate) request_id: String,
    pub(crate) storage: Arc<dyn StorageProvider>,
    pub(crate) context_key: String,
    pub(crate) agent_flow_id: String,
    pub(crate) message_thread_id: Option<ThreadId>,
    pub(crate) use_inline_progress_controls: bool,
    pub(crate) use_inline_flow_controls: bool,
    pub(crate) attach_detach_enabled: bool,
}

#[derive(Clone)]
pub(crate) struct RunUserInputResumeContext {
    pub(crate) bot: Bot,
    pub(crate) chat_id: ChatId,
    pub(crate) session_id: SessionId,
    pub(crate) user_id: i64,
    pub(crate) user_input: String,
    pub(crate) storage: Arc<dyn StorageProvider>,
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
}

struct TaskProgressRuntime {
    progress_message_id: MessageId,
    progress_reply_markup: Option<InlineKeyboardMarkup>,
    progress_handle: tokio::task::JoinHandle<oxide_agent_core::agent::progress::ProgressState>,
    max_iterations: usize,
    tx: tokio::sync::mpsc::Sender<AgentEvent>,
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
        }
    }
}

impl From<&RunApprovedSshResumeContext> for TaskDeliveryContext {
    fn from(value: &RunApprovedSshResumeContext) -> Self {
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
        &ctx.sandbox_scope,
        preserve_binary_uploads,
    )
    .await
    {
        Ok(text) => text,
        Err(err) => {
            if err.to_string() == "MULTIMODAL_DISABLED" {
                send_multimodal_unavailable_message(&ctx.bot, chat_id, ctx.message_thread_id)
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
        context_key: ctx.context_key,
        agent_flow_id: ctx.agent_flow_id,
        message_thread_id: ctx.message_thread_id,
        use_inline_progress_controls: ctx.use_inline_progress_controls,
        use_inline_flow_controls: ctx.use_inline_flow_controls,
        attach_detach_enabled: ctx.attach_detach_enabled,
    })
    .await
}

pub(crate) async fn run_agent_task_with_text(ctx: RunAgentTaskTextContext) -> Result<()> {
    let delivery_ctx = TaskDeliveryContext::from(&ctx);
    let session_id = ctx.session_id;
    let task_text = ctx.task_text;
    run_task_execution(delivery_ctx, move |progress_tx| async move {
        execute_agent_task(session_id, &task_text, Some(progress_tx)).await
    })
    .await
}

pub(crate) async fn run_approved_ssh_resume(ctx: RunApprovedSshResumeContext) -> Result<()> {
    let delivery_ctx = TaskDeliveryContext::from(&ctx);
    let session_id = ctx.session_id;
    let request_id = ctx.request_id;
    run_task_execution(delivery_ctx, move |progress_tx| async move {
        execute_ssh_approval_resume(session_id, &request_id, Some(progress_tx)).await
    })
    .await
}

pub(crate) async fn run_user_input_resume(ctx: RunUserInputResumeContext) -> Result<()> {
    let delivery_ctx = TaskDeliveryContext::from(&ctx);
    let session_id = ctx.session_id;
    let user_input = ctx.user_input;
    run_task_execution(delivery_ctx, move |progress_tx| async move {
        execute_user_input_resume(session_id, user_input, Some(progress_tx)).await
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
        progress_reply_markup,
        progress_handle,
        max_iterations,
        tx,
    } = runtime;
    let result = execute_manual_compaction(ctx.session_id, Some(tx)).await;
    let progress_text = finish_task_progress_runtime(progress_handle, max_iterations).await;

    save_memory_after_task(
        ctx.session_id,
        ctx.user_id,
        &ctx.context_key,
        &ctx.agent_flow_id,
        &ctx.storage,
    )
    .await;

    deliver_manual_compaction_result(
        &delivery_ctx,
        result,
        &progress_text,
        progress_message_id,
        progress_reply_markup,
    )
    .await
}

async fn run_task_execution<Exec, Fut>(ctx: TaskDeliveryContext, execute: Exec) -> Result<()>
where
    Exec: FnOnce(tokio::sync::mpsc::Sender<AgentEvent>) -> Fut,
    Fut: Future<Output = Result<AgentExecutionOutcome>>,
{
    let mut runtime = start_task_progress_runtime(&ctx).await?;
    mark_completed_response_execution_started(ctx.session_id).await;
    let mut result = execute(runtime.tx.clone()).await;

    loop {
        let completed = matches!(result, Ok(AgentExecutionOutcome::Completed(_)));
        if completed {
            begin_completed_response_finalization(ctx.session_id).await;
        }

        let progress_text =
            finish_task_progress_runtime(runtime.progress_handle, runtime.max_iterations).await;

        if completed {
            match prepare_completed_response_delivery(&ctx.session_id).await {
                CompletedResponseDeliveryAction::Restart(followups) => {
                    info!(
                        session_id = %ctx.session_id,
                        followup_count = followups.len(),
                        "Restarting task before completed response delivery"
                    );
                    runtime = match restart_task_progress_runtime(&ctx, runtime.progress_message_id)
                        .await
                    {
                        Ok(runtime) => runtime,
                        Err(error) => {
                            clear_completed_response_delivery_state(&ctx.session_id).await;
                            return Err(error);
                        }
                    };
                    result = execute_agent_task_continuation(
                        ctx.session_id,
                        followups,
                        Some(runtime.tx.clone()),
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

        let delivery_result = deliver_task_result(
            &ctx,
            result,
            &progress_text,
            runtime.progress_message_id,
            runtime.progress_reply_markup,
        )
        .await;
        clear_completed_response_delivery_state(&ctx.session_id).await;
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
    crate::bot::resilient::edit_message_safe_resilient_with_markup(
        &ctx.bot,
        ctx.chat_id,
        progress_message_id,
        DefaultAgentView::task_processing(),
        progress_reply_markup.clone(),
    )
    .await;

    Ok(bind_task_progress_runtime(
        ctx,
        progress_message_id,
        progress_reply_markup,
    ))
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

    Ok(bind_task_progress_runtime(
        ctx,
        progress_msg.id,
        progress_reply_markup,
    ))
}

fn bind_task_progress_runtime(
    ctx: &TaskDeliveryContext,
    progress_message_id: MessageId,
    progress_reply_markup: Option<InlineKeyboardMarkup>,
) -> TaskProgressRuntime {
    let max_iterations = get_agent_max_iterations();
    let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(100);
    let transport = TelegramAgentTransport::new(
        ctx.bot.clone(),
        ctx.chat_id,
        progress_message_id,
        ctx.message_thread_id,
        ctx.use_inline_progress_controls,
    );
    let cfg = ProgressRuntimeConfig::new(max_iterations);
    let progress_handle = spawn_progress_runtime(transport, rx, cfg);

    TaskProgressRuntime {
        progress_message_id,
        progress_reply_markup,
        progress_handle,
        max_iterations,
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
    result: Result<CompactionOutcome>,
    progress_text: &str,
    progress_message_id: MessageId,
    progress_reply_markup: Option<InlineKeyboardMarkup>,
) -> Result<()> {
    let terminal_progress_reply_markup = progress_reply_markup
        .as_ref()
        .map(|_| crate::bot::views::empty_inline_keyboard());
    crate::bot::resilient::edit_message_safe_resilient_with_markup(
        &ctx.bot,
        ctx.chat_id,
        progress_message_id,
        progress_text,
        terminal_progress_reply_markup,
    )
    .await;

    match result {
        Ok(outcome) => {
            send_agent_message(
                &ctx.bot,
                ctx.chat_id,
                DefaultAgentView::context_compacted(outcome.applied),
                crate::bot::OutboundThreadParams {
                    message_thread_id: ctx.message_thread_id,
                },
            )
            .await
        }
        Err(error) => {
            send_agent_message(
                &ctx.bot,
                ctx.chat_id,
                DefaultAgentView::error_message(&error.to_string()),
                crate::bot::OutboundThreadParams {
                    message_thread_id: ctx.message_thread_id,
                },
            )
            .await
        }
    }
}

async fn finish_task_progress_runtime(
    progress_handle: tokio::task::JoinHandle<oxide_agent_core::agent::progress::ProgressState>,
    max_iterations: usize,
) -> String {
    let state = match progress_handle.await {
        Ok(state) => state,
        Err(err) => {
            warn!(error = %err, "Progress runtime task failed");
            oxide_agent_core::agent::progress::ProgressState::new(max_iterations)
        }
    };
    render_progress_html(&state)
}

async fn deliver_task_result(
    ctx: &TaskDeliveryContext,
    result: Result<AgentExecutionOutcome>,
    progress_text: &str,
    progress_message_id: MessageId,
    progress_reply_markup: Option<InlineKeyboardMarkup>,
) -> Result<()> {
    let terminal_progress_reply_markup = progress_reply_markup
        .as_ref()
        .map(|_| crate::bot::views::empty_inline_keyboard());
    let cancelled = result.as_ref().err().is_some_and(is_task_cancelled_error);
    let pending_ssh_approvals = take_pending_ssh_approvals(ctx.session_id).await;

    match result {
        Ok(AgentExecutionOutcome::Completed(response)) => {
            crate::bot::resilient::edit_message_safe_resilient_with_markup(
                &ctx.bot,
                ctx.chat_id,
                progress_message_id,
                progress_text,
                terminal_progress_reply_markup.clone(),
            )
            .await;
            let final_markup = ctx
                .use_inline_flow_controls
                .then(|| {
                    crate::bot::views::agent_flow_inline_keyboard_with_toggle(
                        &ctx.agent_flow_id,
                        ctx.attach_detach_enabled,
                    )
                })
                .filter(|markup| !markup.inline_keyboard.is_empty());
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
        Ok(AgentExecutionOutcome::WaitingForApproval) => {
            crate::bot::resilient::edit_message_safe_resilient_with_markup(
                &ctx.bot,
                ctx.chat_id,
                progress_message_id,
                progress_text,
                terminal_progress_reply_markup,
            )
            .await;
            send_pending_ssh_approval_messages(
                &ctx.bot,
                ctx.chat_id,
                ctx.message_thread_id,
                &pending_ssh_approvals,
            )
            .await?;
        }
        Ok(AgentExecutionOutcome::WaitingForUserInput(request)) => {
            crate::bot::resilient::edit_message_safe_resilient_with_markup(
                &ctx.bot,
                ctx.chat_id,
                progress_message_id,
                progress_text,
                terminal_progress_reply_markup,
            )
            .await;
            send_long_message_in_thread_with_final_markup(
                &ctx.bot,
                ctx.chat_id,
                &request.prompt,
                ctx.message_thread_id,
                None,
            )
            .await?;
        }
        Err(error) => {
            let sanitized_error = oxide_agent_core::utils::sanitize_html_error(&error.to_string());
            let error_text = format!("{progress_text}\n\n❌ <b>Error:</b>\n\n{sanitized_error}");
            crate::bot::resilient::edit_message_safe_resilient_with_markup(
                &ctx.bot,
                ctx.chat_id,
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

async fn execute_agent_task(
    session_id: SessionId,
    task: &str,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> Result<AgentExecutionOutcome> {
    let executor_arc = SESSION_REGISTRY
        .get(&session_id)
        .await
        .ok_or_else(|| anyhow!("No agent session found"))?;
    let cancellation_token = SESSION_REGISTRY
        .get_cancellation_token(&session_id)
        .await
        .ok_or_else(|| anyhow!("No cancellation token found"))?;

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

    executor.session_mut().cancellation_token = (*cancellation_token).clone();
    executor.execute(task, progress_tx).await
}

async fn execute_agent_task_continuation(
    session_id: SessionId,
    followups: Vec<String>,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> Result<AgentExecutionOutcome> {
    let executor_arc = SESSION_REGISTRY
        .get(&session_id)
        .await
        .ok_or_else(|| anyhow!("No agent session found"))?;
    let cancellation_token = SESSION_REGISTRY
        .get_cancellation_token(&session_id)
        .await
        .ok_or_else(|| anyhow!("No cancellation token found"))?;

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

    executor.session_mut().cancellation_token = (*cancellation_token).clone();
    executor.continue_after_runtime_context(progress_tx).await
}

pub(crate) async fn execute_ssh_approval_resume(
    session_id: SessionId,
    request_id: &str,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> Result<AgentExecutionOutcome> {
    let executor_arc = SESSION_REGISTRY
        .get(&session_id)
        .await
        .ok_or_else(|| anyhow!("No agent session found"))?;
    let cancellation_token = SESSION_REGISTRY
        .get_cancellation_token(&session_id)
        .await
        .ok_or_else(|| anyhow!("No cancellation token found"))?;

    let mut executor = executor_arc.write().await;
    if executor.is_timed_out() {
        executor.reset();
        return Err(anyhow!(
            "Previous session timed out. Starting a new session."
        ));
    }

    executor.session_mut().cancellation_token = (*cancellation_token).clone();
    executor.resume_ssh_approval(request_id, progress_tx).await
}

pub(crate) async fn execute_user_input_resume(
    session_id: SessionId,
    user_input: String,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> Result<AgentExecutionOutcome> {
    let executor_arc = SESSION_REGISTRY
        .get(&session_id)
        .await
        .ok_or_else(|| anyhow!("No agent session found"))?;
    let cancellation_token = SESSION_REGISTRY
        .get_cancellation_token(&session_id)
        .await
        .ok_or_else(|| anyhow!("No cancellation token found"))?;

    let mut executor = executor_arc.write().await;
    if executor.is_timed_out() {
        executor.reset();
        return Err(anyhow!(
            "Previous session timed out. Starting a new session."
        ));
    }

    executor.session_mut().cancellation_token = (*cancellation_token).clone();
    executor
        .resume_after_user_input(user_input, progress_tx)
        .await
}

pub(crate) async fn execute_manual_compaction(
    session_id: SessionId,
    progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
) -> Result<CompactionOutcome> {
    let executor_arc = SESSION_REGISTRY
        .get(&session_id)
        .await
        .ok_or_else(|| anyhow!("No agent session found"))?;
    let cancellation_token = SESSION_REGISTRY
        .get_cancellation_token(&session_id)
        .await
        .ok_or_else(|| anyhow!("No cancellation token found"))?;

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

pub(crate) async fn take_pending_ssh_approvals(
    session_id: SessionId,
) -> Vec<oxide_agent_core::agent::SshApprovalRequestView> {
    let Some(executor_arc) = SESSION_REGISTRY.get(&session_id).await else {
        return Vec::new();
    };
    let executor = executor_arc.read().await;
    executor.take_pending_ssh_approvals().await
}

pub(crate) async fn send_pending_ssh_approval_messages(
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
        req.reply_markup(crate::bot::views::ssh_approval_inline_keyboard(
            &request.request_id,
        ))
        .await?;
    }

    Ok(())
}
