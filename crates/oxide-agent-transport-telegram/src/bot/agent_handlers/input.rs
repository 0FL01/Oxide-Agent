use super::{
    automatic_agent_control_markup, renew_cancellation_token, run_agent_task_with_text,
    run_user_input_resume, use_inline_flow_controls, use_inline_topic_controls,
    ActiveSessionConfig, RunAgentTaskTextContext, RunUserInputResumeContext,
    PENDING_TEXT_INPUT_BATCHES, SESSION_REGISTRY,
};
use crate::bot::agent::{extract_agent_file_input, extract_agent_input};
use crate::bot::context::{current_context_state, ensure_current_agent_flow_id};
use crate::bot::topic_route::{touch_dynamic_binding_activity_if_needed, TopicRouteDecision};
use crate::bot::{OutboundThreadParams, TelegramThreadSpec};
use anyhow::Result;
use oxide_agent_core::agent::{
    preprocessor::Preprocessor, PendingUserInput, SessionId, UserInputKind,
};
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::sandbox::SandboxScope;
use oxide_agent_core::storage::StorageProvider;
use std::sync::Arc;
use std::time::Instant;
use teloxide::prelude::*;
use teloxide::types::{MessageId, ThreadId};
use tokio::time::Duration;
use tracing::{info, warn};

pub(crate) const AGENT_TEXT_INPUT_BATCH_DEBOUNCE_MS: u64 = 900;
pub(crate) const AGENT_TEXT_INPUT_SPLIT_THRESHOLD_CHARS: usize = 3500;

#[derive(Clone)]
pub(crate) struct BatchedTextTaskContext {
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
}

#[derive(Clone)]
pub(crate) struct PendingTextInputPart {
    pub(crate) message_id: MessageId,
    pub(crate) text: String,
}

pub(crate) struct PendingTextInputBatch {
    pub(crate) ctx: BatchedTextTaskContext,
    pub(crate) parts: Vec<PendingTextInputPart>,
    pub(crate) revision: u64,
    pub(crate) updated_at: Instant,
}

pub(crate) struct BatchedTextInputCheck<'a> {
    pub(crate) msg: &'a Message,
    pub(crate) bot: &'a Bot,
    pub(crate) storage: &'a Arc<dyn StorageProvider>,
    pub(crate) route: &'a TopicRouteDecision,
    pub(crate) thread_spec: TelegramThreadSpec,
    pub(crate) outbound_thread: OutboundThreadParams,
    pub(crate) session_id: SessionId,
    pub(crate) user_id: i64,
    pub(crate) chat_id: ChatId,
    pub(crate) context_key: &'a str,
    pub(crate) agent_flow_id: &'a str,
}

#[derive(Clone)]
pub(crate) struct DeferredAgentInputContext {
    pub(crate) dispatch: BatchedTextTaskContext,
    pub(crate) llm: Arc<LlmClient>,
    pub(crate) msg: Message,
    pub(crate) sandbox_scope: SandboxScope,
}

pub(crate) struct RunningAgentMessageContext<'a> {
    pub(crate) msg: &'a Message,
    pub(crate) bot: &'a Bot,
    pub(crate) route: &'a TopicRouteDecision,
    pub(crate) sandbox_scope: &'a SandboxScope,
    pub(crate) dispatch: BatchedTextTaskContext,
    pub(crate) thread_spec: TelegramThreadSpec,
    pub(crate) outbound_thread: OutboundThreadParams,
    pub(crate) llm: &'a Arc<LlmClient>,
}

impl PendingTextInputBatch {
    pub(crate) fn new(
        ctx: BatchedTextTaskContext,
        message_id: MessageId,
        text: String,
        updated_at: Instant,
    ) -> Self {
        Self {
            ctx,
            parts: vec![PendingTextInputPart { message_id, text }],
            revision: 1,
            updated_at,
        }
    }
}

pub(crate) fn build_batched_text_task_context(
    bot: &Bot,
    active_session: &ActiveSessionConfig,
    outbound_thread: OutboundThreadParams,
) -> BatchedTextTaskContext {
    BatchedTextTaskContext {
        bot: bot.clone(),
        chat_id: active_session.chat_id,
        session_id: active_session.session_id,
        user_id: active_session.user_id,
        storage: active_session.storage.clone(),
        context_key: active_session.context_key.clone(),
        agent_flow_id: active_session.agent_flow_id.clone(),
        message_thread_id: outbound_thread.message_thread_id,
        use_inline_progress_controls: use_inline_topic_controls(active_session.thread_spec),
        use_inline_flow_controls: use_inline_flow_controls(active_session.thread_spec),
    }
}

pub(crate) async fn is_agent_mode_context(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> Result<bool> {
    Ok(
        current_context_state(storage, user_id, chat_id, thread_spec)
            .await?
            .as_deref()
            == Some("agent_mode"),
    )
}

pub(crate) async fn ensure_agent_flow_session_keys(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> Result<(String, bool, super::AgentModeSessionKeys)> {
    let (agent_flow_id, agent_flow_created) =
        ensure_current_agent_flow_id(storage, user_id, chat_id, thread_spec).await?;
    let session_keys =
        super::agent_mode_session_keys(user_id, chat_id, thread_spec.thread_id, &agent_flow_id);
    Ok((agent_flow_id, agent_flow_created, session_keys))
}

pub(crate) async fn handle_batched_text_input_if_needed(
    ctx: BatchedTextInputCheck<'_>,
) -> Result<bool> {
    let Some(text) = extract_batched_text_candidate(ctx.msg) else {
        return Ok(false);
    };

    buffer_agent_text_input(
        BatchedTextTaskContext {
            bot: ctx.bot.clone(),
            chat_id: ctx.chat_id,
            session_id: ctx.session_id,
            user_id: ctx.user_id,
            storage: ctx.storage.clone(),
            context_key: ctx.context_key.to_string(),
            agent_flow_id: ctx.agent_flow_id.to_string(),
            message_thread_id: ctx.outbound_thread.message_thread_id,
            use_inline_progress_controls: use_inline_topic_controls(ctx.thread_spec),
            use_inline_flow_controls: use_inline_flow_controls(ctx.thread_spec),
        },
        ctx.msg.id,
        text,
    )
    .await;

    touch_dynamic_binding_activity_if_needed(ctx.storage.as_ref(), ctx.user_id, ctx.route).await;
    Ok(true)
}

pub(crate) async fn handle_running_agent_message_if_needed(
    ctx: RunningAgentMessageContext<'_>,
) -> Result<bool> {
    if !SESSION_REGISTRY.is_running(&ctx.dispatch.session_id).await {
        return Ok(false);
    }

    if has_deferred_agent_input_candidate(ctx.msg) {
        let storage = ctx.dispatch.storage.clone();
        let user_id = ctx.dispatch.user_id;
        spawn_deferred_agent_input(DeferredAgentInputContext {
            dispatch: ctx.dispatch,
            llm: ctx.llm.clone(),
            msg: ctx.msg.clone(),
            sandbox_scope: ctx.sandbox_scope.clone(),
        });
        touch_dynamic_binding_activity_if_needed(storage.as_ref(), user_id, ctx.route).await;
        return Ok(true);
    }

    notify_running_agent_task(
        ctx.bot,
        ctx.dispatch.chat_id,
        ctx.thread_spec,
        ctx.outbound_thread,
    )
    .await?;
    touch_dynamic_binding_activity_if_needed(
        ctx.dispatch.storage.as_ref(),
        ctx.dispatch.user_id,
        ctx.route,
    )
    .await;
    Ok(true)
}

pub(crate) fn route_allows_agent_processing(route: &TopicRouteDecision, user_id: i64) -> bool {
    if route.allows_processing() {
        return true;
    }

    info!(
        "Skipping agent message in topic route for user {user_id}. enabled={}, require_mention={}, mention_satisfied={}",
        route.enabled, route.require_mention, route.mention_satisfied
    );
    false
}

fn has_deferred_agent_input_candidate(msg: &Message) -> bool {
    msg.voice().is_some()
        || msg.photo().is_some()
        || msg.video().is_some()
        || msg.document().is_some()
}

fn spawn_deferred_agent_input(ctx: DeferredAgentInputContext) {
    tokio::spawn(async move {
        let chat_id = ctx.dispatch.chat_id;
        let thread_id = ctx.dispatch.message_thread_id;
        let preserve_binary_uploads =
            should_preserve_pending_file_input(&ctx.dispatch.session_id).await;

        match preprocess_agent_message_input(
            &ctx.dispatch.bot,
            &ctx.msg,
            &ctx.llm,
            &ctx.sandbox_scope,
            preserve_binary_uploads,
        )
        .await
        {
            Ok(task_text) => {
                if let Err(error) =
                    dispatch_preprocessed_agent_text(ctx.dispatch.clone(), task_text).await
                {
                    warn!(error = %error, "Failed to dispatch deferred agent input");
                    let sanitized_error =
                        oxide_agent_core::utils::sanitize_html_error(&error.to_string());
                    let _ = crate::bot::resilient::send_message_resilient_with_thread(
                        &ctx.dispatch.bot,
                        chat_id,
                        &format!("❌ Failed to process additional context:\n\n{sanitized_error}"),
                        None,
                        thread_id,
                    )
                    .await;
                }
            }
            Err(error) if error.to_string() == "MULTIMODAL_DISABLED" => {
                let _ = send_multimodal_unavailable_message(&ctx.dispatch.bot, chat_id, thread_id)
                    .await;
            }
            Err(error) => {
                warn!(error = %error, "Failed to preprocess deferred agent input");
                let sanitized_error =
                    oxide_agent_core::utils::sanitize_html_error(&error.to_string());
                let _ = crate::bot::resilient::send_message_resilient_with_thread(
                    &ctx.dispatch.bot,
                    chat_id,
                    &format!("❌ Failed to process additional context:\n\n{sanitized_error}"),
                    None,
                    thread_id,
                )
                .await;
            }
        }
    });
}

pub(crate) async fn preprocess_agent_message_input(
    bot: &Bot,
    msg: &Message,
    llm: &Arc<LlmClient>,
    sandbox_scope: &SandboxScope,
    preserve_binary_uploads: bool,
) -> Result<String> {
    let preprocessor = Preprocessor::new(llm.clone(), sandbox_scope.clone());
    let input = if preserve_binary_uploads {
        extract_agent_file_input(bot, msg).await?
    } else {
        extract_agent_input(bot, msg).await?
    };
    preprocessor.preprocess_input(input).await
}

pub(crate) fn pending_user_input_requires_file(request: Option<&PendingUserInput>) -> bool {
    matches!(
        request.map(|request| &request.kind),
        Some(UserInputKind::File | UserInputKind::UrlOrFile)
    )
}

pub(crate) async fn should_preserve_pending_file_input(session_id: &SessionId) -> bool {
    let Some(executor_arc) = SESSION_REGISTRY.get(session_id).await else {
        return false;
    };

    let executor = executor_arc.read().await;
    pending_user_input_requires_file(executor.session().pending_user_input())
}

#[cfg(test)]
mod tests {
    use super::pending_user_input_requires_file;
    use oxide_agent_core::agent::{PendingUserInput, UserInputKind};

    #[test]
    fn pending_file_request_requires_preserving_attachments() {
        let request = PendingUserInput {
            kind: UserInputKind::File,
            prompt: "upload file".to_string(),
        };

        assert!(pending_user_input_requires_file(Some(&request)));
    }

    #[test]
    fn pending_url_or_file_request_requires_preserving_attachments() {
        let request = PendingUserInput {
            kind: UserInputKind::UrlOrFile,
            prompt: "upload or paste".to_string(),
        };

        assert!(pending_user_input_requires_file(Some(&request)));
    }

    #[test]
    fn text_and_url_requests_keep_default_media_preprocessing() {
        let text = PendingUserInput {
            kind: UserInputKind::Text,
            prompt: "reply".to_string(),
        };
        let url = PendingUserInput {
            kind: UserInputKind::Url,
            prompt: "paste url".to_string(),
        };

        assert!(!pending_user_input_requires_file(Some(&text)));
        assert!(!pending_user_input_requires_file(Some(&url)));
        assert!(!pending_user_input_requires_file(None));
    }
}

pub(crate) async fn send_multimodal_unavailable_message(
    bot: &Bot,
    chat_id: ChatId,
    thread_id: Option<ThreadId>,
) -> Result<()> {
    crate::bot::resilient::send_message_resilient_with_thread(
        bot,
        chat_id,
        "🚫 Agent cannot process this input right now.\nGemini/OpenRouter connection required for image, audio, and video capabilities.",
        None,
        thread_id,
    )
    .await?;

    Ok(())
}

pub(crate) async fn dispatch_preprocessed_agent_text(
    ctx: BatchedTextTaskContext,
    task_text: String,
) -> Result<()> {
    if task_text.trim().is_empty() {
        return Ok(());
    }

    if SESSION_REGISTRY.is_running(&ctx.session_id).await {
        let queued = SESSION_REGISTRY
            .enqueue_runtime_context(&ctx.session_id, task_text.clone())
            .await;
        if queued {
            crate::bot::resilient::send_message_resilient_with_thread(
                &ctx.bot,
                ctx.chat_id,
                "📥 Additional context received. I will apply it on the next iteration.",
                None,
                ctx.message_thread_id,
            )
            .await?;
            return Ok(());
        }
    }

    if !SESSION_REGISTRY.contains(&ctx.session_id).await {
        warn!(session_id = %ctx.session_id, "Session expired before preprocessed input could be processed");
        crate::bot::resilient::send_message_resilient_with_thread(
            &ctx.bot,
            ctx.chat_id,
            "⚠️ Agent session expired before this input was processed. Send it again.",
            None,
            ctx.message_thread_id,
        )
        .await?;
        return Ok(());
    }

    let should_resume_pending_input = match SESSION_REGISTRY.get(&ctx.session_id).await {
        Some(executor_arc) => {
            let executor = executor_arc.read().await;
            executor.session().pending_user_input().is_some() && executor.last_task().is_some()
        }
        None => false,
    };

    renew_cancellation_token(ctx.session_id).await;

    if should_resume_pending_input {
        return run_user_input_resume(RunUserInputResumeContext {
            bot: ctx.bot,
            chat_id: ctx.chat_id,
            session_id: ctx.session_id,
            user_id: ctx.user_id,
            user_input: task_text,
            storage: ctx.storage,
            context_key: ctx.context_key,
            agent_flow_id: ctx.agent_flow_id,
            message_thread_id: ctx.message_thread_id,
            use_inline_progress_controls: ctx.use_inline_progress_controls,
            use_inline_flow_controls: ctx.use_inline_flow_controls,
        })
        .await;
    }

    run_agent_task_with_text(RunAgentTaskTextContext {
        bot: ctx.bot,
        chat_id: ctx.chat_id,
        session_id: ctx.session_id,
        user_id: ctx.user_id,
        task_text,
        storage: ctx.storage,
        context_key: ctx.context_key,
        agent_flow_id: ctx.agent_flow_id,
        message_thread_id: ctx.message_thread_id,
        use_inline_progress_controls: ctx.use_inline_progress_controls,
        use_inline_flow_controls: ctx.use_inline_flow_controls,
    })
    .await
}

pub(crate) async fn notify_running_agent_task(
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

fn extract_batched_text_candidate(msg: &Message) -> Option<String> {
    msg.text().map(str::to_string)
}

pub(crate) fn should_merge_text_batch(
    batch: &PendingTextInputBatch,
    message_id: MessageId,
    text: &str,
    received_at: Instant,
) -> bool {
    let Some(last_part) = batch.parts.last() else {
        return false;
    };

    if received_at.duration_since(batch.updated_at)
        > Duration::from_millis(AGENT_TEXT_INPUT_BATCH_DEBOUNCE_MS)
    {
        return false;
    }

    if message_id.0 != last_part.message_id.0.saturating_add(1) {
        return false;
    }

    if batch.parts.len() > 1 {
        return true;
    }

    last_part.text.chars().count() >= AGENT_TEXT_INPUT_SPLIT_THRESHOLD_CHARS
        || text.chars().count() >= AGENT_TEXT_INPUT_SPLIT_THRESHOLD_CHARS
}

pub(crate) fn assemble_text_batch(parts: &[PendingTextInputPart]) -> String {
    let mut combined = String::new();
    for part in parts {
        combined.push_str(&part.text);
    }
    combined
}

async fn buffer_agent_text_input(ctx: BatchedTextTaskContext, message_id: MessageId, text: String) {
    let received_at = Instant::now();
    let mut finalize_now = None;
    let schedule_revision;

    {
        let mut batches = PENDING_TEXT_INPUT_BATCHES.lock().await;
        if let Some(batch) = batches.get_mut(&ctx.session_id) {
            if should_merge_text_batch(batch, message_id, &text, received_at) {
                batch.parts.push(PendingTextInputPart { message_id, text });
                batch.updated_at = received_at;
                batch.revision = batch.revision.saturating_add(1);
                schedule_revision = Some(batch.revision);
            } else {
                let new_batch =
                    PendingTextInputBatch::new(ctx.clone(), message_id, text, received_at);
                let old_batch = std::mem::replace(batch, new_batch);
                finalize_now = Some(old_batch);
                schedule_revision = Some(1);
            }
        } else {
            batches.insert(
                ctx.session_id,
                PendingTextInputBatch::new(ctx.clone(), message_id, text, received_at),
            );
            schedule_revision = Some(1);
        }
    }

    if let Some(batch) = finalize_now {
        tokio::spawn(async move {
            let session_id = batch.ctx.session_id;
            if let Err(error) = finalize_text_input_batch(batch).await {
                warn!(error = %error, session_id = %session_id, "Failed to finalize immediate text batch");
            }
        });
    }

    if let Some(revision) = schedule_revision {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(AGENT_TEXT_INPUT_BATCH_DEBOUNCE_MS)).await;
            if let Err(error) = finalize_text_input_batch_if_current(ctx.session_id, revision).await
            {
                warn!(error = %error, session_id = %ctx.session_id, "Failed to finalize buffered text batch");
            }
        });
    }
}

async fn finalize_text_input_batch_if_current(session_id: SessionId, revision: u64) -> Result<()> {
    let batch = {
        let mut batches = PENDING_TEXT_INPUT_BATCHES.lock().await;
        let Some(batch) = batches.get(&session_id) else {
            return Ok(());
        };
        if batch.revision != revision {
            return Ok(());
        }
        batches.remove(&session_id)
    };

    if let Some(batch) = batch {
        finalize_text_input_batch(batch).await?;
    }

    Ok(())
}

async fn finalize_text_input_batch(batch: PendingTextInputBatch) -> Result<()> {
    dispatch_preprocessed_agent_text(batch.ctx, assemble_text_batch(&batch.parts)).await
}
