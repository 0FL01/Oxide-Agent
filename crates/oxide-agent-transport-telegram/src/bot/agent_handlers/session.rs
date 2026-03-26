use super::{
    clear_pending_cancel_confirmation, clear_pending_cancel_message, manager_control_plane_enabled,
    PENDING_TEXT_INPUT_BATCHES,
};
use crate::bot::manager_topic_lifecycle::TelegramManagerTopicLifecycle;
use crate::config::BotSettings;
use anyhow::Result;
use async_trait::async_trait;
use oxide_agent_core::agent::{
    executor::AgentExecutor, AgentMemory, AgentMemoryCheckpoint, AgentSession, SessionId,
};
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::sandbox::SandboxScope;
use oxide_agent_core::storage::StorageProvider;
use oxide_agent_runtime::SessionRegistry;
use std::sync::Arc;
use std::sync::LazyLock;
use teloxide::prelude::*;
use tracing::{debug, info, warn};

pub(crate) static SESSION_REGISTRY: LazyLock<SessionRegistry> = LazyLock::new(SessionRegistry::new);

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

#[derive(Clone, Copy, Debug)]
pub(crate) struct AgentModeSessionKeys {
    pub(crate) primary: SessionId,
    pub(crate) legacy: SessionId,
}

#[derive(Clone, Copy)]
pub(crate) struct SessionTransportContext {
    pub(crate) chat_id: ChatId,
    pub(crate) manager_default_chat_id: Option<ChatId>,
    pub(crate) thread_spec: crate::bot::TelegramThreadSpec,
}

pub(crate) struct EnsureSessionContext<'a> {
    pub(crate) session_keys: AgentModeSessionKeys,
    pub(crate) context_key: String,
    pub(crate) agent_flow_id: String,
    pub(crate) agent_flow_created: bool,
    pub(crate) sandbox_scope: SandboxScope,
    pub(crate) user_id: i64,
    pub(crate) bot: &'a Bot,
    pub(crate) transport_ctx: SessionTransportContext,
    pub(crate) llm: &'a Arc<LlmClient>,
    pub(crate) storage: &'a Arc<dyn StorageProvider>,
    pub(crate) settings: &'a Arc<BotSettings>,
}

#[derive(Clone)]
struct FlowMemoryCheckpoint {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: String,
    agent_flow_id: String,
}

#[async_trait]
impl AgentMemoryCheckpoint for FlowMemoryCheckpoint {
    async fn persist(&self, memory: &AgentMemory) -> Result<()> {
        self.storage
            .save_agent_memory_for_flow(
                self.user_id,
                self.context_key.clone(),
                self.agent_flow_id.clone(),
                memory,
            )
            .await?;
        Ok(())
    }
}

impl AgentModeSessionKeys {
    pub(crate) fn distinct_legacy(self) -> Option<SessionId> {
        if self.primary == self.legacy {
            None
        } else {
            Some(self.legacy)
        }
    }
}

pub(crate) enum ResetSessionOutcome {
    Reset,
    Busy,
    NotFound,
}

fn fnv1a_mix_i64(mut hash: u64, value: i64) -> u64 {
    for byte in value.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    hash
}

fn fnv1a_mix_str(mut hash: u64, value: &str) -> u64 {
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    hash
}

pub(crate) fn derive_agent_mode_session_id(
    user_id: i64,
    chat_id: ChatId,
    thread_id: Option<teloxide::types::ThreadId>,
    agent_flow_id: &str,
) -> SessionId {
    let mut hash = FNV_OFFSET_BASIS;
    hash = fnv1a_mix_i64(hash, user_id);
    hash = fnv1a_mix_i64(hash, chat_id.0);
    hash = fnv1a_mix_i64(
        hash,
        thread_id.map_or(0, |thread_id| i64::from(thread_id.0 .0)),
    );
    hash = fnv1a_mix_str(hash, agent_flow_id);

    let folded = hash & (i64::MAX as u64);
    let derived = if folded == 0 { -1 } else { -(folded as i64) };
    SessionId::from(derived)
}

pub(crate) fn agent_mode_session_keys(
    user_id: i64,
    chat_id: ChatId,
    thread_id: Option<teloxide::types::ThreadId>,
    agent_flow_id: &str,
) -> AgentModeSessionKeys {
    AgentModeSessionKeys {
        primary: derive_agent_mode_session_id(user_id, chat_id, thread_id, agent_flow_id),
        legacy: SessionId::from(user_id),
    }
}

pub(crate) fn select_existing_session_id(
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

pub(crate) async fn resolve_existing_session_id(keys: AgentModeSessionKeys) -> Option<SessionId> {
    let primary_exists = SESSION_REGISTRY.contains(&keys.primary).await;
    let legacy_exists = if let Some(legacy) = keys.distinct_legacy() {
        SESSION_REGISTRY.contains(&legacy).await
    } else {
        primary_exists
    };

    select_existing_session_id(keys, primary_exists, legacy_exists)
}

pub(crate) async fn session_manager_control_plane_enabled(session_id: SessionId) -> Option<bool> {
    let executor_arc = SESSION_REGISTRY.get(&session_id).await?;
    let executor = executor_arc.read().await;
    Some(executor.manager_control_plane_enabled())
}

pub(crate) async fn reset_sessions_with_compat(keys: AgentModeSessionKeys) -> ResetSessionOutcome {
    let primary_result = SESSION_REGISTRY.reset(&keys.primary).await;
    let legacy_result = if let Some(legacy) = keys.distinct_legacy() {
        Some(SESSION_REGISTRY.reset(&legacy).await)
    } else {
        None
    };

    let primary_reset = matches!(primary_result, Ok(()));
    let legacy_reset = matches!(legacy_result, Some(Ok(())));
    if primary_reset || legacy_reset {
        clear_pending_text_batch(keys.primary).await;
        if let Some(legacy) = keys.distinct_legacy() {
            clear_pending_text_batch(legacy).await;
        }
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

pub(crate) async fn cancel_and_clear_with_compat(keys: AgentModeSessionKeys) -> (bool, bool) {
    let cancelled_primary = SESSION_REGISTRY.cancel(&keys.primary).await;

    if let Some(legacy) = keys.distinct_legacy() {
        let cancelled_legacy = SESSION_REGISTRY.cancel(&legacy).await;
        (cancelled_primary || cancelled_legacy, false)
    } else {
        (cancelled_primary, false)
    }
}

pub(crate) async fn remove_sessions_with_compat(keys: AgentModeSessionKeys) {
    SESSION_REGISTRY.remove(&keys.primary).await;
    clear_pending_text_batch(keys.primary).await;
    clear_pending_cancel_message(keys.primary).await;
    clear_pending_cancel_confirmation(keys.primary).await;
    if let Some(legacy) = keys.distinct_legacy() {
        SESSION_REGISTRY.remove(&legacy).await;
        clear_pending_text_batch(legacy).await;
        clear_pending_cancel_message(legacy).await;
        clear_pending_cancel_confirmation(legacy).await;
    }
}

pub(crate) async fn clear_pending_text_batch(session_id: SessionId) {
    let mut batches = PENDING_TEXT_INPUT_BATCHES.lock().await;
    batches.remove(&session_id);
}

pub(crate) async fn ensure_session_exists(ctx: EnsureSessionContext<'_>) -> SessionId {
    let manager_enabled = manager_control_plane_enabled(
        ctx.settings,
        ctx.user_id,
        ctx.transport_ctx.chat_id,
        ctx.transport_ctx.thread_spec,
    );
    let requires_primary_session = ctx.session_keys.primary != ctx.session_keys.legacy;

    if let Some(existing_session_id) = resolve_existing_session_id(ctx.session_keys).await {
        let should_migrate_legacy =
            requires_primary_session && existing_session_id != ctx.session_keys.primary;
        if let Some(existing_manager_enabled) =
            session_manager_control_plane_enabled(existing_session_id).await
        {
            if existing_manager_enabled == manager_enabled && !should_migrate_legacy {
                debug!(session_id = %existing_session_id, "Session already exists in cache");
                return existing_session_id;
            }

            let removed = SESSION_REGISTRY.remove_if_idle(&existing_session_id).await;
            if removed {
                debug!(
                    session_id = %existing_session_id,
                    previous_manager_enabled = existing_manager_enabled,
                    current_manager_enabled = manager_enabled,
                    migrate_to_primary = should_migrate_legacy,
                    "Session identity changed; recreating session"
                );
            } else if SESSION_REGISTRY.contains(&existing_session_id).await {
                debug!(
                    session_id = %existing_session_id,
                    previous_manager_enabled = existing_manager_enabled,
                    current_manager_enabled = manager_enabled,
                    migrate_to_primary = should_migrate_legacy,
                    "Session identity changed while task is running; deferring refresh"
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

    let session_id = ctx.session_keys.primary;
    if SESSION_REGISTRY.contains(&session_id).await {
        debug!(session_id = %session_id, "Session already exists in cache");
        return session_id;
    }

    let mut session = AgentSession::new_with_sandbox_scope(session_id, ctx.sandbox_scope.clone());
    session.set_memory_checkpoint(Arc::new(FlowMemoryCheckpoint {
        storage: ctx.storage.clone(),
        user_id: ctx.user_id,
        context_key: ctx.context_key.clone(),
        agent_flow_id: ctx.agent_flow_id.clone(),
    }));
    load_agent_memory_into_session(&ctx, &mut session).await;
    inject_topic_agents_md_for_flow(&ctx, &mut session).await;

    let mut executor = AgentExecutor::new(ctx.llm.clone(), session, ctx.settings.agent.clone());
    executor.set_agents_md_context(ctx.storage.clone(), ctx.user_id, ctx.context_key.clone());
    if manager_enabled {
        let topic_lifecycle = Arc::new(TelegramManagerTopicLifecycle::new(
            ctx.bot.clone(),
            ctx.transport_ctx.manager_default_chat_id,
        ));
        executor = executor
            .with_manager_control_plane(ctx.storage.clone(), ctx.user_id)
            .with_manager_topic_lifecycle(topic_lifecycle);
    }
    SESSION_REGISTRY.insert(session_id, executor).await;
    session_id
}

async fn load_agent_memory_into_session(
    ctx: &EnsureSessionContext<'_>,
    session: &mut AgentSession,
) {
    if let Some(saved_memory) = load_flow_agent_memory(ctx).await {
        session.memory = saved_memory;
        info!(
            user_id = ctx.user_id,
            messages_count = session.memory.get_messages().len(),
            "Loaded agent memory for user in ensure_session_exists"
        );
        return;
    }

    if ctx.agent_flow_created {
        migrate_legacy_agent_memory_into_flow(ctx, session).await;
    } else {
        info!(
            user_id = ctx.user_id,
            "No saved agent memory found, starting fresh"
        );
    }
}

async fn inject_topic_agents_md_for_flow(
    ctx: &EnsureSessionContext<'_>,
    session: &mut AgentSession,
) {
    if session.memory.has_topic_agents_md() {
        return;
    }

    if !ctx.agent_flow_created && !session.memory.get_messages().is_empty() {
        return;
    }

    let topic_agents_md = match ctx
        .storage
        .get_topic_agents_md(ctx.user_id, ctx.context_key.clone())
        .await
    {
        Ok(record) => record.map(|record| record.agents_md),
        Err(error) => {
            warn!(
                error = %error,
                user_id = ctx.user_id,
                topic_id = %ctx.context_key,
                "Failed to load topic AGENTS.md for flow bootstrap"
            );
            None
        }
    };

    let Some(topic_agents_md) = topic_agents_md.map(|content| content.trim().to_string()) else {
        return;
    };
    if topic_agents_md.is_empty() {
        return;
    }

    session.memory.add_message(
        oxide_agent_core::agent::memory::AgentMessage::topic_agents_md(&topic_agents_md),
    );

    if let Err(error) = ctx
        .storage
        .save_agent_memory_for_flow(
            ctx.user_id,
            ctx.context_key.clone(),
            ctx.agent_flow_id.clone(),
            &session.memory,
        )
        .await
    {
        warn!(
            error = %error,
            user_id = ctx.user_id,
            topic_id = %ctx.context_key,
            flow_id = %ctx.agent_flow_id,
            "Failed to persist pinned topic AGENTS.md after bootstrap"
        );
    }
}

async fn load_flow_agent_memory(
    ctx: &EnsureSessionContext<'_>,
) -> Option<oxide_agent_core::agent::AgentMemory> {
    match ctx
        .storage
        .load_agent_memory_for_flow(
            ctx.user_id,
            ctx.context_key.clone(),
            ctx.agent_flow_id.clone(),
        )
        .await
    {
        Ok(memory) => memory,
        Err(error) => {
            warn!(
                error = %error,
                user_id = ctx.user_id,
                topic_id = %ctx.context_key,
                flow_id = %ctx.agent_flow_id,
                "Failed to load flow-scoped agent memory"
            );
            None
        }
    }
}

async fn migrate_legacy_agent_memory_into_flow(
    ctx: &EnsureSessionContext<'_>,
    session: &mut AgentSession,
) {
    if let Ok(Some(saved_memory)) = ctx
        .storage
        .load_agent_memory_for_context(ctx.user_id, ctx.context_key.clone())
        .await
    {
        session.memory = saved_memory;
        if let Err(error) = ctx
            .storage
            .save_agent_memory_for_flow(
                ctx.user_id,
                ctx.context_key.clone(),
                ctx.agent_flow_id.clone(),
                &session.memory,
            )
            .await
        {
            warn!(
                error = %error,
                user_id = ctx.user_id,
                topic_id = %ctx.context_key,
                flow_id = %ctx.agent_flow_id,
                "Failed to migrate legacy agent memory into flow-scoped storage"
            );
        }
        info!(
            user_id = ctx.user_id,
            messages_count = session.memory.get_messages().len(),
            "Migrated legacy agent memory into flow-scoped storage"
        );
    } else {
        info!(
            user_id = ctx.user_id,
            "No saved agent memory found, starting fresh"
        );
    }
}

pub(crate) async fn is_agent_task_running(session_id: SessionId) -> bool {
    SESSION_REGISTRY.is_running(&session_id).await
}

pub(crate) async fn renew_cancellation_token(session_id: SessionId) {
    SESSION_REGISTRY.renew_cancellation_token(&session_id).await;
}

pub(crate) async fn save_memory_after_task(
    session_id: SessionId,
    user_id: i64,
    context_key: &str,
    agent_flow_id: &str,
    _storage: &Arc<dyn StorageProvider>,
) {
    if let Some(executor_arc) = SESSION_REGISTRY.get(&session_id).await {
        let executor = executor_arc.read().await;
        if let Err(error) = executor.session().persist_memory_checkpoint().await {
            warn!(
                error = %error,
                user_id,
                context_key,
                flow_id = agent_flow_id,
                "Failed to flush agent memory checkpoint after task"
            );
        }
    }
}

async fn flow_has_saved_memory(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: &str,
    agent_flow_id: &str,
) -> Result<bool, oxide_agent_core::storage::StorageError> {
    storage
        .load_agent_memory_for_flow(user_id, context_key.to_string(), agent_flow_id.to_string())
        .await
        .map(|memory| memory.is_some())
}

pub(crate) async fn should_create_fresh_flow_on_detach(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: &str,
    agent_flow_id: &str,
) -> bool {
    match flow_has_saved_memory(storage, user_id, context_key, agent_flow_id).await {
        Ok(has_saved_memory) => has_saved_memory,
        Err(err) => {
            warn!(
                user_id,
                context_key,
                agent_flow_id,
                error = %err,
                "Failed to inspect current flow memory before detach; falling back to detach"
            );
            true
        }
    }
}

pub(crate) async fn cleanup_abandoned_empty_flow(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: &str,
    agent_flow_id: &str,
) {
    match flow_has_saved_memory(storage, user_id, context_key, agent_flow_id).await {
        Ok(true) => {}
        Ok(false) => {
            if let Err(err) = storage
                .clear_agent_memory_for_flow(
                    user_id,
                    context_key.to_string(),
                    agent_flow_id.to_string(),
                )
                .await
            {
                warn!(
                    user_id,
                    context_key,
                    agent_flow_id,
                    error = %err,
                    "Failed to delete abandoned empty flow after attach"
                );
            }
        }
        Err(err) => {
            warn!(
                user_id,
                context_key,
                agent_flow_id,
                error = %err,
                "Failed to inspect current flow memory before attach; leaving flow record intact"
            );
        }
    }
}
