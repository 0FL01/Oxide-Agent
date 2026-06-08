//! Row-to-domain mapper functions for SQLx result rows.

use sqlx_postgres::PgRow;

use super::{
    AgentFlowRecord, AgentProfileRecord, AuditEventRecord, ReminderJobRecord, ReminderJobStatus,
    ReminderScheduleKind, ReminderThreadKind, StorageError, TopicAgentsMdRecord, TopicBindingKind,
    TopicBindingRecord, TopicContextRecord, TopicInfraAuthMode, TopicInfraConfigRecord,
    TopicInfraToolMode, UserContextConfig,
};
use super::helpers::{
    enum_from_sql, enum_vec_from_sql, i32_to_u16, i32_to_u32, i64_to_u32, i64_to_u64, row_value,
};

pub(super) fn row_to_user_context(row: &PgRow) -> Result<UserContextConfig, StorageError> {
    let forum_topic_icon_color = row_value::<Option<i64>>(row, "forum_topic_icon_color")?
        .map(|value| i64_to_u32(value, "forum_topic_icon_color"))
        .transpose()?;

    Ok(UserContextConfig {
        state: row_value(row, "state")?,
        current_agent_flow_id: row_value(row, "current_agent_flow_id")?,
        chat_id: row_value(row, "chat_id")?,
        thread_id: row_value(row, "thread_id")?,
        forum_topic_name: row_value(row, "forum_topic_name")?,
        forum_topic_icon_color,
        forum_topic_icon_custom_emoji_id: row_value(row, "forum_topic_icon_custom_emoji_id")?,
        forum_topic_closed: row_value(row, "forum_topic_closed")?,
    })
}

pub(super) fn row_to_agent_flow(row: &PgRow) -> Result<AgentFlowRecord, StorageError> {
    Ok(AgentFlowRecord {
        schema_version: i32_to_u32(
            row_value(row, "schema_version")?,
            "agent flow schema_version",
        )?,
        user_id: row_value(row, "user_id")?,
        context_key: row_value(row, "context_key")?,
        flow_id: row_value(row, "flow_id")?,
        created_at: row_value(row, "created_at")?,
        updated_at: row_value(row, "updated_at")?,
    })
}

pub(super) fn row_to_agent_profile(row: &PgRow) -> Result<AgentProfileRecord, StorageError> {
    Ok(AgentProfileRecord {
        schema_version: i32_to_u32(
            row_value(row, "schema_version")?,
            "agent profile schema_version",
        )?,
        version: i64_to_u64(row_value(row, "version")?, "agent profile version")?,
        user_id: row_value(row, "user_id")?,
        agent_id: row_value(row, "agent_id")?,
        profile: row_value(row, "profile")?,
        created_at: row_value(row, "created_at")?,
        updated_at: row_value(row, "updated_at")?,
    })
}

pub(super) fn row_to_topic_context(row: &PgRow) -> Result<TopicContextRecord, StorageError> {
    Ok(TopicContextRecord {
        schema_version: i32_to_u32(
            row_value(row, "schema_version")?,
            "topic context schema_version",
        )?,
        version: i64_to_u64(row_value(row, "version")?, "topic context version")?,
        user_id: row_value(row, "user_id")?,
        topic_id: row_value(row, "topic_id")?,
        context: row_value(row, "context")?,
        created_at: row_value(row, "created_at")?,
        updated_at: row_value(row, "updated_at")?,
    })
}

pub(super) fn row_to_topic_agents_md(row: &PgRow) -> Result<TopicAgentsMdRecord, StorageError> {
    Ok(TopicAgentsMdRecord {
        schema_version: i32_to_u32(
            row_value(row, "schema_version")?,
            "topic AGENTS.md schema_version",
        )?,
        version: i64_to_u64(row_value(row, "version")?, "topic AGENTS.md version")?,
        user_id: row_value(row, "user_id")?,
        topic_id: row_value(row, "topic_id")?,
        agents_md: row_value(row, "agents_md")?,
        created_at: row_value(row, "created_at")?,
        updated_at: row_value(row, "updated_at")?,
    })
}

pub(super) fn row_to_topic_infra_config(row: &PgRow) -> Result<TopicInfraConfigRecord, StorageError> {
    let auth_mode = enum_from_sql::<TopicInfraAuthMode>(
        &row_value::<String>(row, "auth_mode")?,
        "topic infra auth mode",
    )?;
    let allowed_tool_modes = enum_vec_from_sql::<TopicInfraToolMode>(
        row_value(row, "allowed_tool_modes")?,
        "topic infra tool mode",
    )?;
    let approval_required_modes = enum_vec_from_sql::<TopicInfraToolMode>(
        row_value(row, "approval_required_modes")?,
        "topic infra tool mode",
    )?;

    Ok(TopicInfraConfigRecord {
        schema_version: i32_to_u32(
            row_value(row, "schema_version")?,
            "topic infra schema_version",
        )?,
        version: i64_to_u64(row_value(row, "version")?, "topic infra version")?,
        user_id: row_value(row, "user_id")?,
        topic_id: row_value(row, "topic_id")?,
        target_name: row_value(row, "target_name")?,
        host: row_value(row, "host")?,
        port: i32_to_u16(row_value(row, "port")?, "topic infra port")?,
        remote_user: row_value(row, "remote_user")?,
        auth_mode,
        secret_ref: row_value(row, "secret_ref")?,
        sudo_secret_ref: row_value(row, "sudo_secret_ref")?,
        environment: row_value(row, "environment")?,
        tags: row_value(row, "tags")?,
        allowed_tool_modes,
        approval_required_modes,
        created_at: row_value(row, "created_at")?,
        updated_at: row_value(row, "updated_at")?,
    })
}

pub(super) fn row_to_topic_binding(row: &PgRow) -> Result<TopicBindingRecord, StorageError> {
    let binding_kind = enum_from_sql::<TopicBindingKind>(
        &row_value::<String>(row, "binding_kind")?,
        "topic binding kind",
    )?;

    Ok(TopicBindingRecord {
        schema_version: i32_to_u32(
            row_value(row, "schema_version")?,
            "topic binding schema_version",
        )?,
        version: i64_to_u64(row_value(row, "version")?, "topic binding version")?,
        user_id: row_value(row, "user_id")?,
        topic_id: row_value(row, "topic_id")?,
        agent_id: row_value(row, "agent_id")?,
        binding_kind,
        chat_id: row_value(row, "chat_id")?,
        thread_id: row_value(row, "thread_id")?,
        expires_at: row_value(row, "expires_at")?,
        last_activity_at: row_value(row, "last_activity_at")?,
        created_at: row_value(row, "created_at")?,
        updated_at: row_value(row, "updated_at")?,
    })
}

pub(super) fn row_to_audit_event(row: &PgRow) -> Result<AuditEventRecord, StorageError> {
    Ok(AuditEventRecord {
        schema_version: i32_to_u32(row_value(row, "schema_version")?, "audit schema_version")?,
        version: i64_to_u64(row_value(row, "version")?, "audit version")?,
        event_id: row_value(row, "event_id")?,
        user_id: row_value(row, "user_id")?,
        topic_id: row_value(row, "topic_id")?,
        agent_id: row_value(row, "agent_id")?,
        action: row_value(row, "action")?,
        payload: row_value(row, "payload")?,
        created_at: row_value(row, "created_at")?,
    })
}

pub(super) fn row_to_reminder_job(row: &PgRow) -> Result<ReminderJobRecord, StorageError> {
    let thread_kind = enum_from_sql::<ReminderThreadKind>(
        &row_value::<String>(row, "thread_kind")?,
        "reminder thread kind",
    )?;
    let schedule_kind = enum_from_sql::<ReminderScheduleKind>(
        &row_value::<String>(row, "schedule_kind")?,
        "reminder schedule kind",
    )?;
    let status = enum_from_sql::<ReminderJobStatus>(
        &row_value::<String>(row, "status")?,
        "reminder status",
    )?;
    let interval_secs = row_value::<Option<i64>>(row, "interval_secs")?
        .map(|value| i64_to_u64(value, "reminder interval_secs"))
        .transpose()?;

    Ok(ReminderJobRecord {
        schema_version: i32_to_u32(row_value(row, "schema_version")?, "reminder schema_version")?,
        version: i64_to_u64(row_value(row, "version")?, "reminder version")?,
        reminder_id: row_value(row, "reminder_id")?,
        user_id: row_value(row, "user_id")?,
        context_key: row_value(row, "context_key")?,
        flow_id: row_value(row, "flow_id")?,
        chat_id: row_value(row, "chat_id")?,
        thread_id: row_value(row, "thread_id")?,
        thread_kind,
        task_prompt: row_value(row, "task_prompt")?,
        schedule_kind,
        status,
        next_run_at: row_value(row, "next_run_at")?,
        interval_secs,
        cron_expression: row_value(row, "cron_expression")?,
        timezone: row_value(row, "timezone")?,
        lease_until: row_value(row, "lease_until")?,
        last_run_at: row_value(row, "last_run_at")?,
        last_error: row_value(row, "last_error")?,
        run_count: i64_to_u64(row_value(row, "run_count")?, "reminder run_count")?,
        created_at: row_value(row, "created_at")?,
        updated_at: row_value(row, "updated_at")?,
    })
}
