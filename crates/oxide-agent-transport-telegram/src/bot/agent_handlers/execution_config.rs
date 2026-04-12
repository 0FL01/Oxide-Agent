use super::{current_reminder_schedule_notifier, reminder_thread_kind, SESSION_REGISTRY};
use crate::bot::topic_route::TopicRouteDecision;
use crate::bot::TelegramThreadKind;
use crate::bot::TelegramThreadSpec;
use oxide_agent_core::agent::{
    dm_tool_policy, manager_default_blocked_tools, parse_agent_profile,
    providers::{
        agents_md_tool_names, inject_topic_infra_preflight_system_message,
        inspect_topic_infra_config, manager_control_plane_tool_names, reminder_tool_names,
    },
    AgentExecutionProfile, SessionId,
};
use oxide_agent_core::storage::{StorageProvider, TopicInfraConfigRecord};
use std::sync::Arc;
use teloxide::types::ChatId;
use tracing::warn;

#[derive(Clone)]
pub(crate) struct ActiveSessionConfig {
    pub(crate) session_id: SessionId,
    pub(crate) storage: Arc<dyn StorageProvider>,
    pub(crate) user_id: i64,
    pub(crate) context_key: String,
    pub(crate) agent_flow_id: String,
    pub(crate) chat_id: ChatId,
    pub(crate) thread_spec: TelegramThreadSpec,
}

pub(crate) async fn configure_active_session(
    ctx: &ActiveSessionConfig,
    execution_profile: AgentExecutionProfile,
    topic_infra_config: Option<TopicInfraConfigRecord>,
) {
    apply_execution_profile(ctx.session_id, execution_profile).await;
    apply_topic_infra_config(
        ctx.session_id,
        ctx.storage.clone(),
        ctx.user_id,
        ctx.context_key.clone(),
        topic_infra_config,
    )
    .await;
    apply_reminder_context(
        ctx.session_id,
        ctx.storage.clone(),
        ctx.user_id,
        ctx.context_key.clone(),
        ctx.agent_flow_id.clone(),
        ctx.chat_id,
        ctx.thread_spec,
    )
    .await;
}

pub(crate) async fn resolve_execution_profile(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: &str,
    route: &TopicRouteDecision,
    manager_enabled: bool,
    thread_spec: TelegramThreadSpec,
) -> AgentExecutionProfile {
    let route_prompt = normalize_prompt_section(route.system_prompt_override.as_deref());
    let topic_context_prompt = match storage
        .get_topic_context(user_id, topic_id.to_string())
        .await
    {
        Ok(record) => {
            record.and_then(|record| normalize_prompt_section(Some(record.context.as_str())))
        }
        Err(error) => {
            warn!(
                error = %error,
                user_id,
                topic_id,
                "Failed to load topic context for executor configuration"
            );
            None
        }
    };

    // Check if this is a DM context (direct/private chat)
    let is_dm = thread_spec.kind == TelegramThreadKind::Dm;

    let Some(agent_id) = route.agent_id.clone() else {
        let mut tool_policy = if manager_enabled {
            oxide_agent_core::agent::ToolAccessPolicy::default()
                .with_additional_blocked_tools(manager_default_blocked_tools())
        } else {
            oxide_agent_core::agent::ToolAccessPolicy::default()
        };

        // Apply DM tool policy if in DM context
        if is_dm {
            tool_policy = tool_policy
                .with_additional_blocked_tools(dm_tool_policy().blocked_tools().iter().cloned());
        }

        return AgentExecutionProfile::new(
            None,
            compose_execution_prompt_instructions(
                None,
                route_prompt.as_deref(),
                topic_context_prompt.as_deref(),
            ),
            tool_policy,
        )
        .with_hook_policy(Default::default());
    };

    let mut parsed_profile = match storage.get_agent_profile(user_id, agent_id.clone()).await {
        Ok(Some(record)) => parse_agent_profile(&record.profile),
        Ok(None) => Default::default(),
        Err(error) => {
            warn!(
                error = %error,
                user_id,
                agent_id = %agent_id,
                "Failed to load agent profile for executor configuration"
            );
            Default::default()
        }
    };
    if manager_enabled {
        parsed_profile.tool_policy = parsed_profile
            .tool_policy
            .with_additional_allowed_tools(manager_control_plane_tool_names())
            .with_additional_blocked_tools(manager_default_blocked_tools());
    }
    parsed_profile.tool_policy = parsed_profile
        .tool_policy
        .with_additional_allowed_tools(agents_md_tool_names())
        .with_additional_allowed_tools(reminder_tool_names());

    // Apply DM tool policy if in DM context
    if is_dm {
        parsed_profile.tool_policy = parsed_profile
            .tool_policy
            .with_additional_blocked_tools(dm_tool_policy().blocked_tools().iter().cloned());
    }

    let prompt_instructions = compose_execution_prompt_instructions(
        parsed_profile.prompt_instructions.as_deref(),
        route_prompt.as_deref(),
        topic_context_prompt.as_deref(),
    );

    AgentExecutionProfile::new(
        Some(agent_id),
        prompt_instructions,
        parsed_profile.tool_policy,
    )
    .with_hook_policy(parsed_profile.hook_policy)
}

pub(crate) async fn resolve_topic_infra_config(
    storage: &Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: &str,
) -> Option<TopicInfraConfigRecord> {
    match storage
        .get_topic_infra_config(user_id, topic_id.to_string())
        .await
    {
        Ok(record) => record,
        Err(error) => {
            warn!(
                error = %error,
                user_id,
                topic_id,
                "Failed to load topic infra config for executor configuration"
            );
            None
        }
    }
}

pub(crate) async fn apply_execution_profile(session_id: SessionId, profile: AgentExecutionProfile) {
    let Some(executor_arc) = SESSION_REGISTRY.get(&session_id).await else {
        warn!(session_id = %session_id, "Cannot apply execution profile: session not found");
        return;
    };

    let mut executor = executor_arc.write().await;
    executor.set_execution_profile(profile);
}

pub(crate) async fn apply_topic_infra_config(
    session_id: SessionId,
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: String,
    config: Option<TopicInfraConfigRecord>,
) {
    let preflight = match config.as_ref() {
        Some(config) => {
            Some(inspect_topic_infra_config(&storage, user_id, &topic_id, config).await)
        }
        None => None,
    };
    let provider_config = match preflight.as_ref() {
        Some(report) if report.provider_enabled => config.clone(),
        Some(_) => None,
        None => None,
    };
    let preflight_message = preflight
        .as_ref()
        .map(inject_topic_infra_preflight_system_message)
        .map(|message| message.content);

    let Some(executor_arc) = SESSION_REGISTRY.get(&session_id).await else {
        warn!(session_id = %session_id, "Cannot apply topic infra config: session not found");
        return;
    };

    let mut executor = executor_arc.write().await;
    executor.set_topic_infra(storage, user_id, topic_id, provider_config);
    executor.set_topic_infra_preflight_status(preflight.as_ref(), preflight_message);
}

pub(crate) async fn apply_reminder_context(
    session_id: SessionId,
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: String,
    agent_flow_id: String,
    chat_id: ChatId,
    thread_spec: TelegramThreadSpec,
) {
    let Some(executor_arc) = SESSION_REGISTRY.get(&session_id).await else {
        warn!(session_id = %session_id, "Cannot apply reminder context: session not found");
        return;
    };

    let mut executor = executor_arc.write().await;
    executor.set_reminder_context(oxide_agent_core::agent::providers::ReminderContext {
        storage,
        user_id,
        context_key,
        flow_id: agent_flow_id,
        chat_id: chat_id.0,
        thread_id: thread_spec
            .thread_id
            .map(|thread_id| i64::from(thread_id.0 .0)),
        thread_kind: reminder_thread_kind(thread_spec),
        notifier: current_reminder_schedule_notifier().await,
    });
}

#[cfg(test)]
pub(crate) fn merge_prompt_instructions(
    profile_prompt: Option<&str>,
    route_prompt: Option<&str>,
) -> Option<String> {
    match (
        normalize_prompt_section(profile_prompt),
        normalize_prompt_section(route_prompt),
    ) {
        (Some(profile_prompt), Some(route_prompt)) if profile_prompt == route_prompt => {
            Some(profile_prompt)
        }
        (Some(profile_prompt), Some(route_prompt)) => Some(format!(
            "Profile instructions:\n{profile_prompt}\n\nTopic instructions:\n{route_prompt}"
        )),
        (Some(profile_prompt), None) => Some(profile_prompt),
        (None, Some(route_prompt)) => Some(route_prompt),
        (None, None) => None,
    }
}

pub(crate) fn compose_execution_prompt_instructions(
    profile_prompt: Option<&str>,
    route_prompt: Option<&str>,
    topic_context_prompt: Option<&str>,
) -> Option<String> {
    let mut sections = Vec::new();

    if let Some(profile_prompt) = normalize_prompt_section(profile_prompt) {
        sections.push(("Profile instructions", profile_prompt));
    }
    if let Some(route_prompt) = normalize_prompt_section(route_prompt) {
        sections.push(("Topic instructions", route_prompt));
    }
    if let Some(topic_context_prompt) = normalize_prompt_section(topic_context_prompt) {
        sections.push(("Persistent topic context", topic_context_prompt));
    }

    if sections.is_empty() {
        return None;
    }

    Some(
        sections
            .into_iter()
            .map(|(label, content)| format!("{label}:\n{content}"))
            .collect::<Vec<_>>()
            .join("\n\n"),
    )
}

pub(crate) fn normalize_prompt_section(prompt: Option<&str>) -> Option<String> {
    prompt
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        .map(str::to_string)
}
