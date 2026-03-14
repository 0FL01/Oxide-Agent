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

#[derive(Clone, Copy, Debug)]
struct AgentModeSessionKeys {
    primary: SessionId,
    legacy: SessionId,
}

impl AgentModeSessionKeys {
    fn distinct_legacy(self) -> Option<SessionId> {
        if self.primary == self.legacy {
            None
        } else {
            Some(self.legacy)
        }
    }
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

fn agent_mode_session_keys(
    user_id: i64,
    chat_id: ChatId,
    thread_id: Option<ThreadId>,
) -> AgentModeSessionKeys {
    AgentModeSessionKeys {
        primary: derive_agent_mode_session_id(user_id, chat_id, thread_id),
        legacy: SessionId::from(user_id),
    }
}

fn select_existing_session_id(
    keys: AgentModeSessionKeys,
    primary_exists: bool,
    legacy_exists: bool,
) -> Option<SessionId> {
    if primary_exists {
        Some(keys.primary)
    } else if legacy_exists {
        Some(keys.legacy)
    } else {
        None
    }
}

async fn resolve_existing_session_id(keys: AgentModeSessionKeys) -> Option<SessionId> {
    let primary_exists = SESSION_REGISTRY.contains(&keys.primary).await;
    let legacy_exists = if let Some(legacy) = keys.distinct_legacy() {
        SESSION_REGISTRY.contains(&legacy).await
    } else {
        primary_exists
    };

    select_existing_session_id(keys, primary_exists, legacy_exists)
}

async fn session_manager_control_plane_enabled(session_id: SessionId) -> Option<bool> {
    let executor_arc = SESSION_REGISTRY.get(&session_id).await?;
    let executor = executor_arc.read().await;
    Some(executor.manager_control_plane_enabled())
}

enum ResetSessionOutcome {
    Reset,
    Busy,
    NotFound,
}

async fn reset_sessions_with_compat(keys: AgentModeSessionKeys) -> ResetSessionOutcome {
    let primary_result = SESSION_REGISTRY.reset(&keys.primary).await;
    let legacy_result = if let Some(legacy) = keys.distinct_legacy() {
        Some(SESSION_REGISTRY.reset(&legacy).await)
    } else {
        None
    };

    let primary_reset = matches!(primary_result, Ok(()));
    let legacy_reset = matches!(legacy_result, Some(Ok(())));
    if primary_reset || legacy_reset {
        return ResetSessionOutcome::Reset;
    }

    let primary_busy = matches!(primary_result, Err("Cannot reset while task is running"));
    let legacy_busy = matches!(
        legacy_result,
        Some(Err("Cannot reset while task is running"))
    );
    if primary_busy || legacy_busy {
        return ResetSessionOutcome::Busy;
    }

    ResetSessionOutcome::NotFound
}

async fn cancel_and_clear_with_compat(keys: AgentModeSessionKeys) -> (bool, bool) {
    let cancelled_primary = SESSION_REGISTRY.cancel(&keys.primary).await;
    let cleared_primary = SESSION_REGISTRY.clear_todos(&keys.primary).await;

    if let Some(legacy) = keys.distinct_legacy() {
        let cancelled_legacy = SESSION_REGISTRY.cancel(&legacy).await;
        let cleared_legacy = SESSION_REGISTRY.clear_todos(&legacy).await;
        (
            cancelled_primary || cancelled_legacy,
            cleared_primary || cleared_legacy,
        )
    } else {
        (cancelled_primary, cleared_primary)
    }
}

async fn remove_sessions_with_compat(keys: AgentModeSessionKeys) {
    SESSION_REGISTRY.remove(&keys.primary).await;
    if let Some(legacy) = keys.distinct_legacy() {
        SESSION_REGISTRY.remove(&legacy).await;
    }
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

fn manager_control_plane_enabled(settings: &BotSettings, user_id: i64) -> bool {
    settings.telegram.manager_allowed_users().contains(&user_id)
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
    session_keys: AgentModeSessionKeys,
    storage: &Arc<dyn StorageProvider>,
    send_ctx: &ConfirmationSendCtx<'_>,
) -> Result<()> {
    info!(user_id = user_id, "User confirmed memory clear");
    match reset_sessions_with_compat(session_keys).await {
        ResetSessionOutcome::Reset => {
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
        ResetSessionOutcome::Busy => {
            send_agent_message_with_keyboard(
                send_ctx.bot,
                send_ctx.chat_id,
                DefaultAgentView::clear_blocked_by_task(),
                send_ctx.keyboard,
                send_ctx.outbound_thread,
            )
            .await?;
        }
        ResetSessionOutcome::NotFound => {
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
    session_keys: AgentModeSessionKeys,
    storage: &Arc<dyn StorageProvider>,
    llm: &Arc<LlmClient>,
    settings: &Arc<BotSettings>,
    send_ctx: &ConfirmationSendCtx<'_>,
) -> Result<()> {
    info!(user_id = user_id, "User confirmed container recreation");
    let session_id = ensure_session_exists(session_keys, user_id, llm, storage, settings).await;

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
    let session_keys = agent_mode_session_keys(user_id, msg.chat.id, thread_spec.thread_id);

    info!("Activating agent mode for user {user_id}");

    ensure_session_exists(session_keys, user_id, &llm, &storage, &settings).await;

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
    let session_keys = agent_mode_session_keys(user_id, chat_id, thread_spec.thread_id);

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
    let session_id = ensure_session_exists(session_keys, user_id, &llm, &storage, &settings).await;

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
    use super::{
        agent_mode_session_keys, derive_agent_mode_session_id, ensure_session_exists,
        manager_control_plane_enabled, parse_agent_control_command, remove_sessions_with_compat,
        select_existing_session_id, session_manager_control_plane_enabled, AgentControlCommand,
        SESSION_REGISTRY,
    };
    use crate::config::{BotSettings, TelegramSettings};
    use async_trait::async_trait;
    use oxide_agent_core::agent::AgentSession;
    use oxide_agent_core::config::AgentSettings;
    use oxide_agent_core::llm::LlmClient;
    use oxide_agent_core::storage::{
        AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord, Message, StorageError,
        StorageProvider, TopicBindingRecord, UpsertAgentProfileOptions, UpsertTopicBindingOptions,
        UserConfig,
    };
    use std::sync::Arc;
    use teloxide::types::{ChatId, MessageId, ThreadId};

    struct NoopStorage;

    #[async_trait]
    impl StorageProvider for NoopStorage {
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
            _user_id: i64,
            _memory: &oxide_agent_core::agent::AgentMemory,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn load_agent_memory(
            &self,
            _user_id: i64,
        ) -> Result<Option<oxide_agent_core::agent::AgentMemory>, StorageError> {
            Ok(None)
        }

        async fn clear_agent_memory(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn clear_all_context(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn check_connection(&self) -> Result<(), String> {
            Ok(())
        }

        async fn get_agent_profile(
            &self,
            _user_id: i64,
            _agent_id: String,
        ) -> Result<Option<AgentProfileRecord>, StorageError> {
            Ok(None)
        }

        async fn upsert_agent_profile(
            &self,
            _options: UpsertAgentProfileOptions,
        ) -> Result<AgentProfileRecord, StorageError> {
            Err(StorageError::Config("not needed in tests".to_string()))
        }

        async fn delete_agent_profile(
            &self,
            _user_id: i64,
            _agent_id: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_topic_binding(
            &self,
            _user_id: i64,
            _topic_id: String,
        ) -> Result<Option<TopicBindingRecord>, StorageError> {
            Ok(None)
        }

        async fn upsert_topic_binding(
            &self,
            _options: UpsertTopicBindingOptions,
        ) -> Result<TopicBindingRecord, StorageError> {
            Err(StorageError::Config("not needed in tests".to_string()))
        }

        async fn delete_topic_binding(
            &self,
            _user_id: i64,
            _topic_id: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn append_audit_event(
            &self,
            _options: AppendAuditEventOptions,
        ) -> Result<AuditEventRecord, StorageError> {
            Err(StorageError::Config("not needed in tests".to_string()))
        }

        async fn list_audit_events(
            &self,
            _user_id: i64,
            _limit: usize,
        ) -> Result<Vec<AuditEventRecord>, StorageError> {
            Ok(Vec::new())
        }

        async fn list_audit_events_page(
            &self,
            _user_id: i64,
            _before_version: Option<u64>,
            _limit: usize,
        ) -> Result<Vec<AuditEventRecord>, StorageError> {
            Ok(Vec::new())
        }
    }

    fn test_settings(manager_users: Option<&str>) -> Arc<BotSettings> {
        Arc::new(BotSettings::new(
            AgentSettings::default(),
            TelegramSettings {
                telegram_token: "dummy".to_string(),
                allowed_users_str: None,
                agent_allowed_users_str: Some("77 88".to_string()),
                manager_allowed_users_str: manager_users.map(str::to_string),
                topic_configs: Vec::new(),
            },
        ))
    }

    fn test_llm(settings: &Arc<BotSettings>) -> Arc<LlmClient> {
        Arc::new(LlmClient::new(settings.agent.as_ref()))
    }

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

    #[test]
    fn existing_session_selection_prefers_primary_key() {
        let keys = agent_mode_session_keys(12345, ChatId(-1001), Some(ThreadId(MessageId(42))));
        let selected = select_existing_session_id(keys, true, true);

        assert_eq!(selected, Some(keys.primary));
    }

    #[test]
    fn existing_session_selection_falls_back_to_legacy_key() {
        let keys = agent_mode_session_keys(12345, ChatId(-1001), Some(ThreadId(MessageId(42))));
        let selected = select_existing_session_id(keys, false, true);

        assert_eq!(selected, Some(keys.legacy));
    }

    #[test]
    fn manager_control_plane_access_requires_dedicated_allowlist_entry() {
        let settings = BotSettings::new(
            AgentSettings::default(),
            TelegramSettings {
                telegram_token: "dummy".to_string(),
                allowed_users_str: None,
                agent_allowed_users_str: Some("77 88".to_string()),
                manager_allowed_users_str: Some("88".to_string()),
                topic_configs: Vec::new(),
            },
        );

        assert!(!manager_control_plane_enabled(&settings, 77));
        assert!(manager_control_plane_enabled(&settings, 88));
    }

    #[test]
    fn manager_control_plane_access_disabled_when_allowlist_is_empty() {
        let settings = BotSettings::new(
            AgentSettings::default(),
            TelegramSettings {
                telegram_token: "dummy".to_string(),
                allowed_users_str: None,
                agent_allowed_users_str: Some("77".to_string()),
                manager_allowed_users_str: None,
                topic_configs: Vec::new(),
            },
        );

        assert!(!manager_control_plane_enabled(&settings, 77));
    }

    #[test]
    fn manager_control_plane_gating_respects_forum_thread_agent_sessions() {
        let settings = BotSettings::new(
            AgentSettings::default(),
            TelegramSettings {
                telegram_token: "dummy".to_string(),
                allowed_users_str: None,
                agent_allowed_users_str: Some("77 88".to_string()),
                manager_allowed_users_str: Some("88".to_string()),
                topic_configs: Vec::new(),
            },
        );

        let thread_keys =
            agent_mode_session_keys(77, ChatId(-100_123), Some(ThreadId(MessageId(42))));

        assert_ne!(thread_keys.primary, thread_keys.legacy);
        assert!(!manager_control_plane_enabled(&settings, 77));
        assert!(manager_control_plane_enabled(&settings, 88));
    }

    #[tokio::test]
    async fn threaded_transport_session_enables_manager_tools_only_for_allowlisted_users() {
        let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage);
        let manager_settings = test_settings(Some("88"));
        let llm = test_llm(&manager_settings);

        let allowed_keys =
            agent_mode_session_keys(88, ChatId(-100_123), Some(ThreadId(MessageId(42))));
        let blocked_keys =
            agent_mode_session_keys(77, ChatId(-100_123), Some(ThreadId(MessageId(43))));

        remove_sessions_with_compat(allowed_keys).await;
        remove_sessions_with_compat(blocked_keys).await;

        let allowed_session =
            ensure_session_exists(allowed_keys, 88, &llm, &storage, &manager_settings).await;
        let blocked_session =
            ensure_session_exists(blocked_keys, 77, &llm, &storage, &manager_settings).await;

        assert_eq!(allowed_session, allowed_keys.primary);
        assert_eq!(blocked_session, blocked_keys.primary);
        assert_eq!(
            session_manager_control_plane_enabled(allowed_session).await,
            Some(true)
        );
        assert_eq!(
            session_manager_control_plane_enabled(blocked_session).await,
            Some(false)
        );

        remove_sessions_with_compat(allowed_keys).await;
        remove_sessions_with_compat(blocked_keys).await;
    }

    #[tokio::test]
    async fn threaded_transport_session_recreates_primary_when_manager_rbac_changes() {
        let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage);
        let allowed_settings = test_settings(Some("77"));
        let restricted_settings = test_settings(None);
        let llm = test_llm(&allowed_settings);
        let keys = agent_mode_session_keys(77, ChatId(-100_123), Some(ThreadId(MessageId(61))));

        remove_sessions_with_compat(keys).await;

        let first_session =
            ensure_session_exists(keys, 77, &llm, &storage, &allowed_settings).await;
        assert_eq!(first_session, keys.primary);
        assert_eq!(
            session_manager_control_plane_enabled(first_session).await,
            Some(true)
        );

        let second_session =
            ensure_session_exists(keys, 77, &llm, &storage, &restricted_settings).await;
        assert_eq!(second_session, keys.primary);
        assert_eq!(
            session_manager_control_plane_enabled(second_session).await,
            Some(false)
        );

        remove_sessions_with_compat(keys).await;
    }

    #[tokio::test]
    async fn threaded_transport_session_defers_rbac_refresh_while_running_then_refreshes_after_complete(
    ) {
        let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage);
        let allowed_settings = test_settings(Some("77"));
        let restricted_settings = test_settings(None);
        let llm = test_llm(&allowed_settings);
        let keys = agent_mode_session_keys(77, ChatId(-100_123), Some(ThreadId(MessageId(62))));

        remove_sessions_with_compat(keys).await;

        let first_session =
            ensure_session_exists(keys, 77, &llm, &storage, &allowed_settings).await;
        assert_eq!(first_session, keys.primary);
        assert_eq!(
            session_manager_control_plane_enabled(first_session).await,
            Some(true)
        );

        let marked_running = SESSION_REGISTRY
            .with_executor_mut(&first_session, |executor| {
                Box::pin(async move {
                    executor.session_mut().start_task();
                })
            })
            .await;
        assert!(marked_running.is_ok());

        let second_session =
            ensure_session_exists(keys, 77, &llm, &storage, &restricted_settings).await;
        assert_eq!(second_session, first_session);
        assert_eq!(
            session_manager_control_plane_enabled(second_session).await,
            Some(true)
        );

        let marked_completed = SESSION_REGISTRY
            .with_executor_mut(&first_session, |executor| {
                Box::pin(async move {
                    executor.session_mut().complete();
                })
            })
            .await;
        assert!(marked_completed.is_ok());

        let third_session =
            ensure_session_exists(keys, 77, &llm, &storage, &restricted_settings).await;
        assert_eq!(third_session, first_session);
        assert_eq!(
            session_manager_control_plane_enabled(third_session).await,
            Some(false)
        );

        remove_sessions_with_compat(keys).await;
    }

    #[tokio::test]
    async fn threaded_transport_session_does_not_bypass_rbac_via_legacy_fallback() {
        let storage: Arc<dyn StorageProvider> = Arc::new(NoopStorage);
        let legacy_manager_settings = test_settings(Some("77"));
        let llm = test_llm(&legacy_manager_settings);
        let keys = agent_mode_session_keys(77, ChatId(-100_123), Some(ThreadId(MessageId(52))));

        remove_sessions_with_compat(keys).await;

        let legacy_executor = oxide_agent_core::agent::AgentExecutor::new(
            llm.clone(),
            AgentSession::new(keys.legacy),
            legacy_manager_settings.agent.clone(),
        )
        .with_manager_control_plane(storage.clone(), 77);
        SESSION_REGISTRY.insert(keys.legacy, legacy_executor).await;

        let restricted_settings = test_settings(None);
        let resolved_session =
            ensure_session_exists(keys, 77, &llm, &storage, &restricted_settings).await;

        assert_eq!(resolved_session, keys.primary);
        assert_eq!(
            session_manager_control_plane_enabled(resolved_session).await,
            Some(false)
        );

        remove_sessions_with_compat(keys).await;
    }
}

async fn ensure_session_exists(
    session_keys: AgentModeSessionKeys,
    user_id: i64,
    llm: &Arc<LlmClient>,
    storage: &Arc<dyn StorageProvider>,
    settings: &Arc<BotSettings>,
) -> SessionId {
    let manager_enabled = manager_control_plane_enabled(settings, user_id);

    if let Some(existing_session_id) = resolve_existing_session_id(session_keys).await {
        if let Some(existing_manager_enabled) =
            session_manager_control_plane_enabled(existing_session_id).await
        {
            if existing_manager_enabled == manager_enabled {
                debug!(session_id = %existing_session_id, "Session already exists in cache");
                return existing_session_id;
            }

            let removed = SESSION_REGISTRY.remove_if_idle(&existing_session_id).await;
            if removed {
                debug!(
                    session_id = %existing_session_id,
                    previous_manager_enabled = existing_manager_enabled,
                    current_manager_enabled = manager_enabled,
                    "Session manager RBAC changed; recreating session"
                );
            } else if SESSION_REGISTRY.contains(&existing_session_id).await {
                debug!(
                    session_id = %existing_session_id,
                    previous_manager_enabled = existing_manager_enabled,
                    current_manager_enabled = manager_enabled,
                    "Session manager RBAC changed while task is running; deferring refresh"
                );
                return existing_session_id;
            }
        } else {
            let removed = SESSION_REGISTRY.remove_if_idle(&existing_session_id).await;
            if !removed && SESSION_REGISTRY.contains(&existing_session_id).await {
                debug!(
                    session_id = %existing_session_id,
                    "Session state unavailable while task is running; deferring refresh"
                );
                return existing_session_id;
            }

            debug!(
                session_id = %existing_session_id,
                "Session state unavailable; recreating session"
            );
        }
    }

    let session_id = session_keys.primary;
    if SESSION_REGISTRY.contains(&session_id).await {
        debug!(session_id = %session_id, "Session already exists in cache");
        return session_id;
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

    let mut executor = AgentExecutor::new(llm.clone(), session, settings.agent.clone());
    if manager_enabled {
        executor = executor.with_manager_control_plane(storage.clone(), user_id);
    }
    SESSION_REGISTRY.insert(session_id, executor).await;
    session_id
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

struct RunAgentTaskTextContext {
    bot: Bot,
    chat_id: ChatId,
    session_id: SessionId,
    user_id: i64,
    task_text: String,
    storage: Arc<dyn StorageProvider>,
    message_thread_id: Option<ThreadId>,
}

async fn run_agent_task_with_text(ctx: RunAgentTaskTextContext) -> Result<()> {
    let progress_msg = super::resilient::send_message_resilient_with_thread(
        &ctx.bot,
        ctx.chat_id,
        "⏳ Processing task...",
        Some(ParseMode::Html),
        ctx.message_thread_id,
    )
    .await?;

    let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(100);
    let transport = TelegramAgentTransport::new(
        ctx.bot.clone(),
        ctx.chat_id,
        progress_msg.id,
        ctx.message_thread_id,
    );
    let cfg = ProgressRuntimeConfig::new(AGENT_MAX_ITERATIONS);
    let progress_handle = spawn_progress_runtime(transport, rx, cfg);

    let result = execute_agent_task(ctx.session_id, &ctx.task_text, Some(tx)).await;
    let state = match progress_handle.await {
        Ok(state) => state,
        Err(err) => {
            warn!(error = %err, "Progress runtime task failed");
            ProgressState::new(AGENT_MAX_ITERATIONS)
        }
    };
    let progress_text = render_progress_html(&state);

    save_memory_after_task(ctx.session_id, ctx.user_id, &ctx.storage).await;

    match result {
        Ok(response) => {
            super::resilient::edit_message_safe_resilient(
                &ctx.bot,
                ctx.chat_id,
                progress_msg.id,
                &progress_text,
            )
            .await;
            // Use send_long_message to properly split response if it exceeds Telegram limit
            send_long_message_in_thread(&ctx.bot, ctx.chat_id, &response, ctx.message_thread_id)
                .await?;
        }
        Err(e) => {
            // Sanitize error text to prevent Telegram HTML parse errors
            let sanitized_error = oxide_agent_core::utils::sanitize_html_error(&e.to_string());
            let error_text = format!("{progress_text}\n\n❌ <b>Error:</b>\n\n{sanitized_error}");
            super::resilient::edit_message_safe_resilient(
                &ctx.bot,
                ctx.chat_id,
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
    session_keys: AgentModeSessionKeys,
    outbound_thread: OutboundThreadParams,
}

async fn handle_loop_retry(
    ctx: &LoopCallbackContext,
    storage: Arc<dyn StorageProvider>,
    llm: Arc<LlmClient>,
    settings: Arc<BotSettings>,
) -> Result<()> {
    let session_id =
        ensure_session_exists(ctx.session_keys, ctx.user_id, &llm, &storage, &settings).await;
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
            message_thread_id: retry_ctx.outbound_thread.message_thread_id,
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
    // Cancel any running task first to release the executor lock.
    let _ = cancel_and_clear_with_compat(ctx.session_keys).await;

    // Brief yield to allow the run loop to observe cancellation and release locks.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    match reset_sessions_with_compat(ctx.session_keys).await {
        ResetSessionOutcome::Reset => {
            send_agent_message_with_keyboard(
                &ctx.bot,
                ctx.chat_id,
                DefaultAgentView::task_reset(),
                &get_agent_keyboard(),
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
    let session_keys = agent_mode_session_keys(user_id, chat_id, thread_id);
    let ctx = LoopCallbackContext {
        bot,
        chat_id,
        user_id,
        session_keys,
        outbound_thread: outbound_thread_from_callback(&q),
    };

    match data {
        LOOP_CALLBACK_RETRY => handle_loop_retry(&ctx, storage, llm, settings).await?,
        LOOP_CALLBACK_RESET => handle_loop_reset(&ctx).await?,
        LOOP_CALLBACK_CANCEL => {
            cancel_agent_task_by_id(
                ctx.bot.clone(),
                ctx.session_keys,
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
    let session_keys = agent_mode_session_keys(user_id, msg.chat.id, thread_spec.thread_id);

    let (cancelled, cleared_todos) = cancel_and_clear_with_compat(session_keys).await;

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
    session_keys: AgentModeSessionKeys,
    chat_id: ChatId,
    message_thread_id: Option<ThreadId>,
) -> Result<()> {
    let (cancelled, cleared_todos) = cancel_and_clear_with_compat(session_keys).await;
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
    let session_keys = agent_mode_session_keys(user_id, msg.chat.id, thread_spec.thread_id);

    let session_id = resolve_existing_session_id(session_keys)
        .await
        .unwrap_or(session_keys.primary);
    save_memory_after_task(session_id, user_id, &storage).await;
    remove_sessions_with_compat(session_keys).await;

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
    let session_keys = agent_mode_session_keys(user_id, msg.chat.id, thread_spec.thread_id);
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
                handle_clear_memory_confirmation(user_id, session_keys, &storage, &send_ctx)
                    .await?;
            }
            ConfirmationType::RecreateContainer => {
                handle_recreate_container_confirmation(
                    user_id,
                    session_keys,
                    &storage,
                    &llm,
                    &settings,
                    &send_ctx,
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
