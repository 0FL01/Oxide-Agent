use crate::bot::agent_handlers::{
    manager_control_plane_enabled, reminder_thread_kind, resolve_execution_profile,
    resolve_topic_infra_config,
};
use crate::bot::context::sandbox_scope;
use crate::bot::thread::{TelegramThreadKind, TelegramThreadSpec};
use crate::bot::topic_route::TopicRouteDecision;
use crate::config::BotSettings;
use anyhow::Result;
use oxide_agent_core::agent::executor::AgentExecutor;
use oxide_agent_core::agent::providers::{inspect_topic_infra_config, ReminderContext};
use oxide_agent_core::agent::recovery::{prune_tool_history_by_availability, HistoryRepairOutcome};
use oxide_agent_core::agent::{AgentSession, SessionId};
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::storage::{
    resolve_active_topic_binding, PersistedAgentMemoryRef, R2Storage, StorageProvider,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use teloxide::types::{ChatId, MessageId, ThreadId};
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

const STARTUP_TOOL_DRIFT_PRUNE_ENABLED: &str = "STARTUP_TOOL_DRIFT_PRUNE_ENABLED";
const STARTUP_TOOL_DRIFT_PRUNE_DRY_RUN: &str = "STARTUP_TOOL_DRIFT_PRUNE_DRY_RUN";
const STARTUP_TOOL_DRIFT_PRUNE_TIMEOUT_SECS: &str = "STARTUP_TOOL_DRIFT_PRUNE_TIMEOUT_SECS";
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const MAINTENANCE_FLOW_ID: &str = "startup-tool-drift-prune";

#[derive(Debug, Clone, Copy)]
struct StartupToolDriftPruneConfig {
    enabled: bool,
    dry_run: bool,
    timeout_secs: u64,
}

#[derive(Debug, Default, Clone, Copy)]
/// Aggregate results of the startup tool-drift cleanup pass.
pub(crate) struct StartupToolDriftPruneStats {
    /// Number of persisted memory records inspected.
    pub(crate) scanned_records: usize,
    /// Number of memory records that required rewriting.
    pub(crate) changed_records: usize,
    /// Number of tool result messages removed.
    pub(crate) dropped_tool_results: usize,
    /// Number of tool calls trimmed from assistant batches.
    pub(crate) trimmed_tool_calls: usize,
    /// Number of assistant tool-call messages converted back to plain assistant text.
    pub(crate) converted_tool_call_messages: usize,
    /// Number of assistant tool-call messages dropped entirely.
    pub(crate) dropped_tool_call_messages: usize,
}

/// Run the cold-start tool drift cleanup pass for persisted Telegram agent memory.
pub(crate) async fn run_startup_tool_drift_prune(
    storage: Arc<R2Storage>,
    llm_client: Arc<LlmClient>,
    settings: Arc<BotSettings>,
) -> Result<Option<StartupToolDriftPruneStats>> {
    let config = StartupToolDriftPruneConfig::from_env();
    if !config.enabled {
        return Ok(None);
    }

    let storage_for_run = Arc::clone(&storage);
    let llm_for_run = Arc::clone(&llm_client);
    let settings_for_run = Arc::clone(&settings);
    let stats = timeout(
        Duration::from_secs(config.timeout_secs),
        run_startup_tool_drift_prune_inner(storage_for_run, llm_for_run, settings_for_run, config),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "startup tool drift prune timed out after {}s",
            config.timeout_secs
        )
    })??;

    info!(
        scanned_records = stats.scanned_records,
        changed_records = stats.changed_records,
        dropped_tool_results = stats.dropped_tool_results,
        trimmed_tool_calls = stats.trimmed_tool_calls,
        converted_tool_call_messages = stats.converted_tool_call_messages,
        dropped_tool_call_messages = stats.dropped_tool_call_messages,
        dry_run = config.dry_run,
        "Startup tool drift prune completed"
    );

    Ok(Some(stats))
}

async fn run_startup_tool_drift_prune_inner(
    storage: Arc<R2Storage>,
    llm_client: Arc<LlmClient>,
    settings: Arc<BotSettings>,
    config: StartupToolDriftPruneConfig,
) -> Result<StartupToolDriftPruneStats> {
    let references = storage.list_persisted_agent_memories().await?;
    let mut stats = StartupToolDriftPruneStats::default();

    for reference in references {
        stats.scanned_records = stats.scanned_records.saturating_add(1);
        if let Err(error) = prune_single_memory_record(
            Arc::clone(&storage),
            Arc::clone(&llm_client),
            Arc::clone(&settings),
            &reference,
            config.dry_run,
            &mut stats,
        )
        .await
        {
            warn!(
                error = %error,
                user_id = reference.user_id,
                context_key = %reference.context_key,
                flow_id = reference.flow_id.as_deref().unwrap_or("<context>"),
                "Startup tool drift prune skipped a memory record"
            );
        }
    }

    Ok(stats)
}

async fn prune_single_memory_record(
    storage: Arc<R2Storage>,
    llm_client: Arc<LlmClient>,
    settings: Arc<BotSettings>,
    reference: &PersistedAgentMemoryRef,
    dry_run: bool,
    stats: &mut StartupToolDriftPruneStats,
) -> Result<()> {
    let storage_dyn: Arc<dyn StorageProvider> = storage.clone();
    let Some((chat_id, thread_spec)) = parse_storage_context_key(&reference.context_key) else {
        warn!(
            user_id = reference.user_id,
            context_key = %reference.context_key,
            "Skipping startup tool drift prune for unparsable context key"
        );
        return Ok(());
    };

    let memory = match &reference.flow_id {
        Some(flow_id) => {
            storage_dyn
                .load_agent_memory_for_flow(
                    reference.user_id,
                    reference.context_key.clone(),
                    flow_id.clone(),
                )
                .await?
        }
        None => {
            storage_dyn
                .load_agent_memory_for_context(reference.user_id, reference.context_key.clone())
                .await?
        }
    };
    let Some(mut memory) = memory else {
        return Ok(());
    };

    let available_tools = resolve_available_tools_for_memory(
        Arc::clone(&storage_dyn),
        llm_client,
        settings,
        reference,
        chat_id,
        thread_spec,
    )
    .await?;
    let (rewritten_messages, outcome) =
        prune_tool_history_by_availability(memory.get_messages(), &available_tools);

    if !outcome.applied {
        return Ok(());
    }

    stats.changed_records = stats.changed_records.saturating_add(1);
    merge_repair_outcome(stats, &outcome);

    if dry_run {
        info!(
            user_id = reference.user_id,
            context_key = %reference.context_key,
            flow_id = reference.flow_id.as_deref().unwrap_or("<context>"),
            dropped_tool_results = outcome.dropped_tool_results,
            trimmed_tool_calls = outcome.trimmed_tool_calls,
            converted_tool_call_messages = outcome.converted_tool_call_messages,
            dropped_tool_call_messages = outcome.dropped_tool_call_messages,
            "Startup tool drift prune would rewrite memory record"
        );
        return Ok(());
    }

    memory.replace_messages(rewritten_messages);

    match &reference.flow_id {
        Some(flow_id) => {
            storage_dyn
                .save_agent_memory_for_flow(
                    reference.user_id,
                    reference.context_key.clone(),
                    flow_id.clone(),
                    &memory,
                )
                .await?
        }
        None => {
            storage_dyn
                .save_agent_memory_for_context(
                    reference.user_id,
                    reference.context_key.clone(),
                    &memory,
                )
                .await?
        }
    }

    info!(
        user_id = reference.user_id,
        context_key = %reference.context_key,
        flow_id = reference.flow_id.as_deref().unwrap_or("<context>"),
        dropped_tool_results = outcome.dropped_tool_results,
        trimmed_tool_calls = outcome.trimmed_tool_calls,
        converted_tool_call_messages = outcome.converted_tool_call_messages,
        dropped_tool_call_messages = outcome.dropped_tool_call_messages,
        "Startup tool drift prune rewrote memory record"
    );

    Ok(())
}

async fn resolve_available_tools_for_memory(
    storage: Arc<dyn StorageProvider>,
    llm_client: Arc<LlmClient>,
    settings: Arc<BotSettings>,
    reference: &PersistedAgentMemoryRef,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> Result<HashSet<String>> {
    let manager_enabled =
        manager_control_plane_enabled(settings.as_ref(), reference.user_id, chat_id, thread_spec);
    let route = resolve_topic_route_for_context(
        storage.as_ref(),
        settings.as_ref(),
        reference.user_id,
        &reference.context_key,
        chat_id,
        thread_spec,
    )
    .await;
    let execution_profile = resolve_execution_profile(
        &storage,
        reference.user_id,
        &reference.context_key,
        &route,
        manager_enabled,
    )
    .await;
    let topic_infra_config =
        resolve_effective_topic_infra_config(&storage, reference.user_id, &reference.context_key)
            .await;

    let flow_id = reference
        .flow_id
        .clone()
        .unwrap_or_else(|| MAINTENANCE_FLOW_ID.to_string());
    let session_id = maintenance_session_id(reference);
    let session = AgentSession::new_with_sandbox_scope(
        session_id,
        sandbox_scope(reference.user_id, chat_id, thread_spec),
    );
    let mut executor = AgentExecutor::new(llm_client, session, settings.agent.clone());
    executor.set_agents_md_context(
        storage.clone(),
        reference.user_id,
        reference.context_key.clone(),
    );
    if manager_enabled {
        executor = executor.with_manager_control_plane(storage.clone(), reference.user_id);
    }
    if let Some(config) = topic_infra_config {
        executor.set_topic_infra(
            storage.clone(),
            reference.user_id,
            reference.context_key.clone(),
            Some(config),
        );
    }
    executor.set_reminder_context(ReminderContext {
        storage,
        user_id: reference.user_id,
        context_key: reference.context_key.clone(),
        flow_id,
        chat_id: chat_id.0,
        thread_id: thread_spec
            .thread_id
            .map(|thread_id| i64::from(thread_id.0 .0)),
        thread_kind: reminder_thread_kind(thread_spec),
    });
    executor.set_execution_profile(execution_profile);

    Ok(executor
        .current_tool_definitions()
        .into_iter()
        .map(|tool| tool.name)
        .collect())
}

async fn resolve_topic_route_for_context(
    storage: &dyn StorageProvider,
    settings: &BotSettings,
    user_id: i64,
    context_key: &str,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) -> TopicRouteDecision {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    let dynamic_binding = match storage
        .get_topic_binding(user_id, context_key.to_string())
        .await
    {
        Ok(record) => resolve_active_topic_binding(record, now),
        Err(error) => {
            warn!(
                error = %error,
                user_id,
                context_key,
                "Failed to load topic binding during startup tool drift prune"
            );
            None
        }
    };
    if let Some(binding) = dynamic_binding {
        return TopicRouteDecision {
            enabled: true,
            require_mention: false,
            mention_satisfied: true,
            system_prompt_override: None,
            agent_id: Some(binding.agent_id),
            dynamic_binding_topic_id: Some(binding.topic_id),
        };
    }

    let thread_id = thread_spec.thread_id.map(|thread_id| thread_id.0 .0);
    if let Some(topic) = settings.telegram.resolve_topic_config(chat_id.0, thread_id) {
        return TopicRouteDecision {
            enabled: topic.enabled,
            require_mention: false,
            mention_satisfied: true,
            system_prompt_override: topic.system_prompt,
            agent_id: topic.agent_id,
            dynamic_binding_topic_id: None,
        };
    }

    TopicRouteDecision {
        enabled: true,
        require_mention: false,
        mention_satisfied: true,
        system_prompt_override: None,
        agent_id: None,
        dynamic_binding_topic_id: None,
    }
}

async fn resolve_effective_topic_infra_config(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: &str,
) -> Option<oxide_agent_core::storage::TopicInfraConfigRecord> {
    let config = resolve_topic_infra_config(storage, user_id, context_key).await?;
    let report = inspect_topic_infra_config(storage, user_id, context_key, &config).await;
    report.provider_enabled.then_some(config)
}

fn parse_storage_context_key(context_key: &str) -> Option<(ChatId, TelegramThreadSpec)> {
    let (chat_part, thread_part) = context_key.rsplit_once(':')?;
    let chat_id = chat_part.parse::<i64>().ok()?;
    let thread_id = thread_part.parse::<i32>().ok()?;

    let thread_spec = if chat_id > 0 {
        TelegramThreadSpec::new(
            TelegramThreadKind::Dm,
            (thread_id != 0).then_some(ThreadId(MessageId(thread_id))),
        )
    } else if thread_id != 0 {
        TelegramThreadSpec::new(
            TelegramThreadKind::Forum,
            Some(ThreadId(MessageId(thread_id))),
        )
    } else {
        TelegramThreadSpec::new(TelegramThreadKind::None, None)
    };

    Some((ChatId(chat_id), thread_spec))
}

fn maintenance_session_id(reference: &PersistedAgentMemoryRef) -> SessionId {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in reference.user_id.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    for byte in reference.context_key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if let Some(flow_id) = &reference.flow_id {
        for byte in flow_id.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    SessionId::from(hash as i64)
}

fn merge_repair_outcome(stats: &mut StartupToolDriftPruneStats, outcome: &HistoryRepairOutcome) {
    stats.dropped_tool_results = stats
        .dropped_tool_results
        .saturating_add(outcome.dropped_tool_results);
    stats.trimmed_tool_calls = stats
        .trimmed_tool_calls
        .saturating_add(outcome.trimmed_tool_calls);
    stats.converted_tool_call_messages = stats
        .converted_tool_call_messages
        .saturating_add(outcome.converted_tool_call_messages);
    stats.dropped_tool_call_messages = stats
        .dropped_tool_call_messages
        .saturating_add(outcome.dropped_tool_call_messages);
}

impl StartupToolDriftPruneConfig {
    fn from_env() -> Self {
        Self {
            enabled: env_bool(STARTUP_TOOL_DRIFT_PRUNE_ENABLED).unwrap_or(true),
            dry_run: env_bool(STARTUP_TOOL_DRIFT_PRUNE_DRY_RUN).unwrap_or(false),
            timeout_secs: env_u64(STARTUP_TOOL_DRIFT_PRUNE_TIMEOUT_SECS)
                .unwrap_or(DEFAULT_TIMEOUT_SECS),
        }
    }
}

fn env_bool(key: &str) -> Option<bool> {
    std::env::var(key)
        .ok()
        .and_then(|raw| match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
}

fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::parse_storage_context_key;
    use crate::bot::thread::TelegramThreadKind;
    use teloxide::types::ChatId;

    #[test]
    fn parses_forum_context_key() {
        let (chat_id, spec) =
            parse_storage_context_key("-1001:42").expect("context key must parse");

        assert_eq!(chat_id, ChatId(-1001));
        assert_eq!(spec.kind, TelegramThreadKind::Forum);
        assert_eq!(spec.thread_id.expect("thread id must exist").0 .0, 42);
    }

    #[test]
    fn parses_group_context_key_without_thread() {
        let (chat_id, spec) = parse_storage_context_key("-1001:0").expect("context key must parse");

        assert_eq!(chat_id, ChatId(-1001));
        assert_eq!(spec.kind, TelegramThreadKind::None);
        assert!(spec.thread_id.is_none());
    }

    #[test]
    fn parses_dm_context_key() {
        let (chat_id, spec) = parse_storage_context_key("42:0").expect("context key must parse");

        assert_eq!(chat_id, ChatId(42));
        assert_eq!(spec.kind, TelegramThreadKind::Dm);
        assert!(spec.thread_id.is_none());
    }
}
