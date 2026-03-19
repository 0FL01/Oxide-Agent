use super::{
    schema::{
        AGENT_FLOW_SCHEMA_VERSION, AGENT_PROFILE_SCHEMA_VERSION, AUDIT_EVENT_SCHEMA_VERSION,
        REMINDER_JOB_SCHEMA_VERSION, TOPIC_AGENTS_MD_SCHEMA_VERSION, TOPIC_BINDING_SCHEMA_VERSION,
        TOPIC_CONTEXT_SCHEMA_VERSION, TOPIC_INFRA_CONFIG_SCHEMA_VERSION,
    },
    AgentFlowRecord, AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord,
    CreateReminderJobOptions, ReminderJobRecord, ReminderJobStatus, TopicAgentsMdRecord,
    TopicBindingRecord, TopicContextRecord, TopicInfraConfigRecord, UpsertAgentProfileOptions,
    UpsertTopicAgentsMdOptions, UpsertTopicBindingOptions, UpsertTopicContextOptions,
    UpsertTopicInfraConfigOptions,
};

#[must_use]
pub(crate) fn build_agent_profile_record(
    options: UpsertAgentProfileOptions,
    existing: Option<AgentProfileRecord>,
    now: i64,
) -> AgentProfileRecord {
    match existing {
        Some(existing_record) => AgentProfileRecord {
            schema_version: AGENT_PROFILE_SCHEMA_VERSION,
            version: next_record_version(Some(existing_record.version)),
            user_id: options.user_id,
            agent_id: options.agent_id,
            profile: options.profile,
            created_at: existing_record.created_at,
            updated_at: now,
        },
        None => AgentProfileRecord {
            schema_version: AGENT_PROFILE_SCHEMA_VERSION,
            version: next_record_version(None),
            user_id: options.user_id,
            agent_id: options.agent_id,
            profile: options.profile,
            created_at: now,
            updated_at: now,
        },
    }
}

#[must_use]
pub(crate) fn build_topic_context_record(
    options: UpsertTopicContextOptions,
    existing: Option<TopicContextRecord>,
    now: i64,
) -> TopicContextRecord {
    match existing {
        Some(existing_record) => TopicContextRecord {
            schema_version: TOPIC_CONTEXT_SCHEMA_VERSION,
            version: next_record_version(Some(existing_record.version)),
            user_id: options.user_id,
            topic_id: options.topic_id,
            context: options.context,
            created_at: existing_record.created_at,
            updated_at: now,
        },
        None => TopicContextRecord {
            schema_version: TOPIC_CONTEXT_SCHEMA_VERSION,
            version: next_record_version(None),
            user_id: options.user_id,
            topic_id: options.topic_id,
            context: options.context,
            created_at: now,
            updated_at: now,
        },
    }
}

#[must_use]
pub(crate) fn build_topic_agents_md_record(
    options: UpsertTopicAgentsMdOptions,
    existing: Option<TopicAgentsMdRecord>,
    now: i64,
) -> TopicAgentsMdRecord {
    match existing {
        Some(existing_record) => TopicAgentsMdRecord {
            schema_version: TOPIC_AGENTS_MD_SCHEMA_VERSION,
            version: next_record_version(Some(existing_record.version)),
            user_id: options.user_id,
            topic_id: options.topic_id,
            agents_md: options.agents_md,
            created_at: existing_record.created_at,
            updated_at: now,
        },
        None => TopicAgentsMdRecord {
            schema_version: TOPIC_AGENTS_MD_SCHEMA_VERSION,
            version: next_record_version(None),
            user_id: options.user_id,
            topic_id: options.topic_id,
            agents_md: options.agents_md,
            created_at: now,
            updated_at: now,
        },
    }
}

#[must_use]
pub(crate) fn build_topic_infra_config_record(
    options: UpsertTopicInfraConfigOptions,
    existing: Option<TopicInfraConfigRecord>,
    now: i64,
) -> TopicInfraConfigRecord {
    match existing {
        Some(existing_record) => TopicInfraConfigRecord {
            schema_version: TOPIC_INFRA_CONFIG_SCHEMA_VERSION,
            version: next_record_version(Some(existing_record.version)),
            user_id: options.user_id,
            topic_id: options.topic_id,
            target_name: options.target_name,
            host: options.host,
            port: options.port,
            remote_user: options.remote_user,
            auth_mode: options.auth_mode,
            secret_ref: options.secret_ref,
            sudo_secret_ref: options.sudo_secret_ref,
            environment: options.environment,
            tags: options.tags,
            allowed_tool_modes: options.allowed_tool_modes,
            approval_required_modes: options.approval_required_modes,
            created_at: existing_record.created_at,
            updated_at: now,
        },
        None => TopicInfraConfigRecord {
            schema_version: TOPIC_INFRA_CONFIG_SCHEMA_VERSION,
            version: next_record_version(None),
            user_id: options.user_id,
            topic_id: options.topic_id,
            target_name: options.target_name,
            host: options.host,
            port: options.port,
            remote_user: options.remote_user,
            auth_mode: options.auth_mode,
            secret_ref: options.secret_ref,
            sudo_secret_ref: options.sudo_secret_ref,
            environment: options.environment,
            tags: options.tags,
            allowed_tool_modes: options.allowed_tool_modes,
            approval_required_modes: options.approval_required_modes,
            created_at: now,
            updated_at: now,
        },
    }
}

#[must_use]
pub(crate) fn build_topic_binding_record(
    options: UpsertTopicBindingOptions,
    existing: Option<TopicBindingRecord>,
    now: i64,
) -> TopicBindingRecord {
    match existing {
        Some(existing_record) => {
            let binding_kind = options.binding_kind.unwrap_or(existing_record.binding_kind);
            let chat_id = options.chat_id.apply(existing_record.chat_id);
            let thread_id = options.thread_id.apply(existing_record.thread_id);
            let expires_at = options.expires_at.apply(existing_record.expires_at);
            let last_activity_at = Some(options.last_activity_at.unwrap_or(now));

            TopicBindingRecord {
                schema_version: TOPIC_BINDING_SCHEMA_VERSION,
                version: next_record_version(Some(existing_record.version)),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                binding_kind,
                chat_id,
                thread_id,
                expires_at,
                last_activity_at,
                created_at: existing_record.created_at,
                updated_at: now,
            }
        }
        None => TopicBindingRecord {
            schema_version: TOPIC_BINDING_SCHEMA_VERSION,
            version: next_record_version(None),
            user_id: options.user_id,
            topic_id: options.topic_id,
            agent_id: options.agent_id,
            binding_kind: options.binding_kind.unwrap_or_default(),
            chat_id: options.chat_id.for_new_record(),
            thread_id: options.thread_id.for_new_record(),
            expires_at: options.expires_at.for_new_record(),
            last_activity_at: Some(options.last_activity_at.unwrap_or(now)),
            created_at: now,
            updated_at: now,
        },
    }
}

#[must_use]
pub(crate) fn build_agent_flow_record(
    user_id: i64,
    context_key: String,
    flow_id: String,
    existing: Option<AgentFlowRecord>,
    now: i64,
) -> AgentFlowRecord {
    match existing {
        Some(existing_record) => AgentFlowRecord {
            schema_version: AGENT_FLOW_SCHEMA_VERSION,
            user_id,
            context_key,
            flow_id,
            created_at: existing_record.created_at,
            updated_at: now,
        },
        None => AgentFlowRecord {
            schema_version: AGENT_FLOW_SCHEMA_VERSION,
            user_id,
            context_key,
            flow_id,
            created_at: now,
            updated_at: now,
        },
    }
}

#[must_use]
pub(crate) fn build_audit_event_record(
    options: AppendAuditEventOptions,
    current_version: Option<u64>,
    now: i64,
    event_id: String,
) -> AuditEventRecord {
    AuditEventRecord {
        schema_version: AUDIT_EVENT_SCHEMA_VERSION,
        version: next_record_version(current_version),
        event_id,
        user_id: options.user_id,
        topic_id: options.topic_id,
        agent_id: options.agent_id,
        action: options.action,
        payload: options.payload,
        created_at: now,
    }
}

#[must_use]
pub(crate) fn build_reminder_job_record(
    options: CreateReminderJobOptions,
    reminder_id: String,
    now: i64,
) -> ReminderJobRecord {
    ReminderJobRecord {
        schema_version: REMINDER_JOB_SCHEMA_VERSION,
        version: next_record_version(None),
        reminder_id,
        user_id: options.user_id,
        context_key: options.context_key,
        flow_id: options.flow_id,
        chat_id: options.chat_id,
        thread_id: options.thread_id,
        thread_kind: options.thread_kind,
        task_prompt: options.task_prompt,
        schedule_kind: options.schedule_kind,
        status: ReminderJobStatus::Scheduled,
        next_run_at: options.next_run_at,
        interval_secs: options.interval_secs,
        cron_expression: options.cron_expression,
        timezone: options.timezone,
        lease_until: None,
        last_run_at: None,
        last_error: None,
        run_count: 0,
        created_at: now,
        updated_at: now,
    }
}

#[must_use]
pub(crate) fn with_next_reminder_version(record: &ReminderJobRecord) -> u64 {
    next_record_version(Some(record.version))
}

#[must_use]
pub(crate) fn next_record_version(current_version: Option<u64>) -> u64 {
    match current_version {
        Some(version) => version.saturating_add(1),
        None => 1,
    }
}
