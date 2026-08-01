use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde_json::json;
use sha2::Digest;
use sqlx_core::query::query;
use sqlx_core::raw_sql::raw_sql;
use sqlx_postgres::Postgres;

use super::{SqlxStorage, SqlxStorageConfig};
use crate::agent::memory::AgentMemory;
use crate::storage::{
    AppendAuditEventOptions, BrowserArtifactRecord, CreateReminderJobOptions, ForumTopicContext,
    OptionalMetadataPatch, ReminderJobStatus, ReminderScheduleKind, ReminderThreadKind,
    StorageError, StorageProvider, TopicBindingKind, TopicInfraAuthMode, TopicInfraToolMode,
    UpsertAgentProfileOptions, UpsertTopicAgentsMdOptions, UpsertTopicBindingOptions,
    UpsertTopicContextOptions, UpsertTopicInfraConfigOptions,
};

static USER_COUNTER: AtomicI64 = AtomicI64::new(1);

#[tokio::test]
async fn sqlx_storage_connects_and_runs_migrations_when_test_url_is_set() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };

    storage
        .check_database_connection()
        .await
        .expect("SQLx storage health query should pass after migrations");
}

#[tokio::test]
async fn sqlx_context_model_selection_is_atomic_and_context_scoped() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };
    let user_id = unique_user_id();
    let first_context = "telegram:100:200";
    let second_context = "telegram:100:201";

    storage
        .set_context_agent_model_selection(
            user_id,
            first_context,
            Some("provider/model-a".to_string()),
        )
        .await
        .expect("first model selection should be stored");
    storage
        .set_context_agent_model_selection(
            user_id,
            second_context,
            Some("provider/model-b".to_string()),
        )
        .await
        .expect("second model selection should be stored");

    assert_eq!(
        storage
            .get_context_agent_model_selection(user_id, first_context)
            .await
            .expect("first selection should load")
            .as_deref(),
        Some("provider/model-a")
    );
    assert_eq!(
        storage
            .get_context_agent_model_selection(user_id, second_context)
            .await
            .expect("second selection should load")
            .as_deref(),
        Some("provider/model-b")
    );

    storage
        .set_context_agent_model_selection(user_id, first_context, None)
        .await
        .expect("first selection should clear");
    assert_eq!(
        storage
            .get_context_agent_model_selection(user_id, first_context)
            .await
            .expect("cleared selection should load"),
        None
    );
    assert_eq!(
        storage
            .get_context_agent_model_selection(user_id, second_context)
            .await
            .expect("second selection should remain")
            .as_deref(),
        Some("provider/model-b")
    );
}

#[tokio::test]
async fn sqlx_context_fields_are_updated_independently() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };
    let user_id = unique_user_id();
    let context_key = "telegram:-100:200";

    storage
        .set_context_agent_model_selection(
            user_id,
            context_key,
            Some("provider/model-a".to_string()),
        )
        .await
        .expect("model selection should be stored");
    storage
        .set_context_state(
            user_id,
            context_key,
            Some("agent".to_string()),
            -100,
            Some(200),
            false,
        )
        .await
        .expect("context state should be stored");
    storage
        .set_context_agent_flow(user_id, context_key, "flow-a".to_string(), -100, Some(200))
        .await
        .expect("context flow should be stored");
    storage
        .upsert_forum_topic_context(
            user_id,
            context_key,
            ForumTopicContext {
                chat_id: -100,
                thread_id: 200,
                name: Some("Ops".to_string()),
                icon_color: Some(0x6FB9F0),
                icon_custom_emoji_id: Some("emoji".to_string()),
                closed: true,
            },
        )
        .await
        .expect("forum topic metadata should be stored");

    let context = storage
        .get_user_context(user_id, context_key)
        .await
        .expect("context should load")
        .expect("context should exist");
    assert_eq!(context.state.as_deref(), Some("agent"));
    assert_eq!(context.current_agent_flow_id.as_deref(), Some("flow-a"));
    assert_eq!(context.chat_id, Some(-100));
    assert_eq!(context.thread_id, Some(200));
    assert_eq!(context.forum_topic_name.as_deref(), Some("Ops"));
    assert_eq!(context.forum_topic_icon_color, Some(0x6FB9F0));
    assert_eq!(
        context.forum_topic_icon_custom_emoji_id.as_deref(),
        Some("emoji")
    );
    assert!(context.forum_topic_closed);
    assert_eq!(
        storage
            .get_context_agent_model_selection(user_id, context_key)
            .await
            .expect("model selection should load")
            .as_deref(),
        Some("provider/model-a")
    );
}

#[tokio::test]
async fn sqlx_context_updates_do_not_clobber_other_rows() {
    let Some(storage) = sqlx_test_storage_with_connections(4).await else {
        return;
    };
    let user_id = unique_user_id();

    let first = storage.set_context_state(
        user_id,
        "telegram:-100:200",
        Some("first".to_string()),
        -100,
        Some(200),
        false,
    );
    let second = storage.set_context_state(
        user_id,
        "telegram:-100:201",
        Some("second".to_string()),
        -100,
        Some(201),
        false,
    );
    let (first, second) = tokio::join!(first, second);
    first.expect("first context update should succeed");
    second.expect("second context update should succeed");

    let contexts = storage
        .list_user_contexts(user_id)
        .await
        .expect("contexts should load");
    assert_eq!(contexts.len(), 2);
    assert_eq!(contexts[0].1.state.as_deref(), Some("first"));
    assert_eq!(contexts[1].1.state.as_deref(), Some("second"));
}

#[tokio::test]
async fn sqlx_context_delete_removes_only_the_requested_row() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };
    let user_id = unique_user_id();

    for context_key in ["telegram:-100:200", "telegram:-100:201"] {
        storage
            .set_context_agent_model_selection(
                user_id,
                context_key,
                Some(format!("provider/{context_key}")),
            )
            .await
            .expect("model selection should create a context row");
    }

    assert!(
        storage
            .delete_user_context(user_id, "telegram:-100:200")
            .await
            .expect("context delete should succeed")
    );
    assert!(
        storage
            .get_user_context(user_id, "telegram:-100:200")
            .await
            .expect("deleted context should load")
            .is_none()
    );
    assert_eq!(
        storage
            .get_context_agent_model_selection(user_id, "telegram:-100:200")
            .await
            .expect("deleted model selection should load"),
        None
    );
    assert!(
        storage
            .get_user_context(user_id, "telegram:-100:201")
            .await
            .expect("other context should load")
            .is_some()
    );
    assert!(
        !storage
            .delete_user_context(user_id, "telegram:-100:200")
            .await
            .expect("repeated context delete should succeed")
    );
}

#[tokio::test]
async fn sqlx_context_flow_initialization_is_atomic() {
    let Some(storage) = sqlx_test_storage_with_connections(4).await else {
        return;
    };
    let user_id = unique_user_id();
    let context_key = "telegram:-100:201";

    let first = storage.ensure_context_agent_flow(
        user_id,
        context_key,
        "flow-a".to_string(),
        -100,
        Some(201),
    );
    let second = storage.ensure_context_agent_flow(
        user_id,
        context_key,
        "flow-b".to_string(),
        -100,
        Some(201),
    );
    let (first, second) = tokio::join!(first, second);
    let first = first.expect("first flow initialization should succeed");
    let second = second.expect("second flow initialization should succeed");

    assert_eq!(first.0, second.0);
    assert_ne!(first.1, second.1);
    assert!(first.1 || second.1);
}

#[tokio::test]
async fn sqlx_context_state_mirrors_dm_global_state_atomically() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };
    let user_id = unique_user_id();
    let context_key = "telegram:42:0";

    storage
        .set_context_state(
            user_id,
            context_key,
            Some("agent".to_string()),
            42,
            None,
            true,
        )
        .await
        .expect("DM state should be stored");

    assert_eq!(
        storage
            .get_user_context(user_id, context_key)
            .await
            .expect("DM context should load")
            .and_then(|context| context.state)
            .as_deref(),
        Some("agent")
    );
    assert_eq!(
        storage
            .get_user_state(user_id)
            .await
            .expect("global DM state should load")
            .as_deref(),
        Some("agent")
    );
}

#[tokio::test]
async fn sqlx_model_selection_migration_canonicalizes_legacy_values() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };
    let user_id = unique_user_id();
    let cases = [
        (
            "legacy-bootstrap",
            Some("llm-provider/opencode-go/opencode-go/mimo-v2.5"),
            Some("opencode-go/mimo-v2.5"),
        ),
        (
            "legacy-openrouter",
            Some("llm-provider/openrouter/google/gemini/model"),
            Some("openrouter/google/gemini/model"),
        ),
        (
            "canonical",
            Some("openai-base:zai/vendor/model"),
            Some("openai-base:zai/vendor/model"),
        ),
        ("default", None, None),
    ];

    for (context_key, stored, _) in cases {
        storage
            .set_context_agent_model_selection(
                user_id,
                context_key,
                stored.map(ToString::to_string),
            )
            .await
            .expect("migration fixture should be stored");
    }

    raw_sql(include_str!(
        "../../../../../migrations/0014_canonicalize_user_context_agent_model.sql"
    ))
    .execute(storage.pool())
    .await
    .expect("model selection migration should succeed");

    for (context_key, _, expected) in cases {
        assert_eq!(
            storage
                .get_context_agent_model_selection(user_id, context_key)
                .await
                .expect("migrated selection should load")
                .as_deref(),
            expected
        );
    }

    let malformed_user_id = unique_user_id();
    storage
        .set_context_agent_model_selection(
            malformed_user_id,
            "legacy",
            Some("llm-provider/opencode-go/opencode-go/mimo-v2.5".to_string()),
        )
        .await
        .expect("legacy fixture should be stored");
    storage
        .set_context_agent_model_selection(
            malformed_user_id,
            "malformed",
            Some("broken".to_string()),
        )
        .await
        .expect("malformed fixture should be stored");

    let mut tx = storage
        .pool()
        .begin()
        .await
        .expect("transaction should begin");
    let error = raw_sql(include_str!(
        "../../../../../migrations/0014_canonicalize_user_context_agent_model.sql"
    ))
    .execute(&mut *tx)
    .await
    .expect_err("malformed selection must abort migration");
    assert!(error.to_string().contains("Cannot canonicalize malformed"));
    tx.rollback()
        .await
        .expect("failed migration should roll back");

    assert_eq!(
        storage
            .get_context_agent_model_selection(malformed_user_id, "legacy")
            .await
            .expect("legacy selection should load")
            .as_deref(),
        Some("llm-provider/opencode-go/opencode-go/mimo-v2.5")
    );
    storage
        .set_context_agent_model_selection(malformed_user_id, "malformed", None)
        .await
        .expect("malformed fixture should be cleared");
}

#[tokio::test]
async fn sqlx_agent_memory_scopes_are_independent() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };
    let user_id = unique_user_id();
    let global_memory = AgentMemory::new(1024);
    let context_memory = AgentMemory::new(2048);
    let flow_memory = AgentMemory::new(4096);

    storage
        .save_agent_memory(user_id, &global_memory)
        .await
        .expect("global memory should save");
    storage
        .save_agent_memory_for_context(user_id, "ctx-a".to_string(), &context_memory)
        .await
        .expect("context memory should save");
    storage
        .save_agent_memory_for_flow(
            user_id,
            "ctx-a".to_string(),
            "flow-a".to_string(),
            &flow_memory,
        )
        .await
        .expect("flow memory should save");

    assert_memory_eq(
        &global_memory,
        &storage
            .load_agent_memory(user_id)
            .await
            .expect("global memory should load")
            .expect("global memory should exist"),
    );
    assert_memory_eq(
        &context_memory,
        &storage
            .load_agent_memory_for_context(user_id, "ctx-a".to_string())
            .await
            .expect("context memory should load")
            .expect("context memory should exist"),
    );
    assert_memory_eq(
        &flow_memory,
        &storage
            .load_agent_memory_for_flow(user_id, "ctx-a".to_string(), "flow-a".to_string())
            .await
            .expect("flow memory should load")
            .expect("flow memory should exist"),
    );

    storage
        .clear_agent_memory_for_flow(user_id, "ctx-a".to_string(), "flow-a".to_string())
        .await
        .expect("flow clear should delete memory");
    assert!(
        storage
            .load_agent_memory_for_flow(user_id, "ctx-a".to_string(), "flow-a".to_string())
            .await
            .expect("flow memory lookup should succeed")
            .is_none()
    );
    storage
        .clear_agent_memory_for_context(user_id, "ctx-a".to_string())
        .await
        .expect("context clear should delete context memory");
    assert!(
        storage
            .load_agent_memory_for_context(user_id, "ctx-a".to_string())
            .await
            .expect("context memory lookup should succeed")
            .is_none()
    );
    assert!(
        storage
            .load_agent_memory(user_id)
            .await
            .expect("global memory lookup should succeed")
            .is_some()
    );
}

#[tokio::test]
async fn sqlx_control_plane_records_and_secrets_roundtrip() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };
    let user_id = unique_user_id();

    let profile = storage
        .upsert_agent_profile(UpsertAgentProfileOptions {
            user_id,
            agent_id: "ops".to_string(),
            profile: json!({"model": "test-a"}),
        })
        .await
        .expect("agent profile should upsert");
    let updated_profile = storage
        .upsert_agent_profile(UpsertAgentProfileOptions {
            user_id,
            agent_id: "ops".to_string(),
            profile: json!({"model": "test-b"}),
        })
        .await
        .expect("agent profile should update");
    assert_eq!(profile.version + 1, updated_profile.version);
    assert_eq!(updated_profile.profile, json!({"model": "test-b"}));
    assert_eq!(
        storage
            .list_agent_profiles(user_id)
            .await
            .expect("agent profiles should list")
            .len(),
        1
    );

    let context = storage
        .upsert_topic_context(UpsertTopicContextOptions {
            user_id,
            topic_id: "topic-a".to_string(),
            context: "short operational note".to_string(),
        })
        .await
        .expect("topic context should upsert");
    assert_eq!(context.context, "short operational note");

    let duplicate = storage
        .upsert_topic_agents_md(UpsertTopicAgentsMdOptions {
            user_id,
            topic_id: "topic-a".to_string(),
            agents_md: "short operational note".to_string(),
        })
        .await
        .expect_err("duplicate prompt content across stores should be rejected");
    assert!(matches!(
        duplicate,
        StorageError::DuplicateTopicPromptContent { .. }
    ));

    let agents_md = storage
        .upsert_topic_agents_md(UpsertTopicAgentsMdOptions {
            user_id,
            topic_id: "topic-a".to_string(),
            agents_md: "# Topic AGENTS\nKeep it short.".to_string(),
        })
        .await
        .expect("topic AGENTS.md should upsert");
    assert!(agents_md.agents_md.starts_with("# Topic AGENTS"));

    let infra = storage
        .upsert_topic_infra_config(UpsertTopicInfraConfigOptions {
            user_id,
            topic_id: "topic-a".to_string(),
            target_name: "host-a".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            remote_user: "oxide".to_string(),
            auth_mode: TopicInfraAuthMode::PrivateKey,
            secret_ref: Some("storage:ssh/key".to_string()),
            sudo_secret_ref: Some("storage:ssh/sudo".to_string()),
            environment: Some("test".to_string()),
            tags: vec!["local".to_string()],
            allowed_tool_modes: vec![TopicInfraToolMode::Exec, TopicInfraToolMode::ReadFile],
        })
        .await
        .expect("topic infra should upsert");
    let loaded_infra = storage
        .get_topic_infra_config(user_id, "topic-a".to_string())
        .await
        .expect("topic infra should load")
        .expect("topic infra should exist");
    assert_eq!(loaded_infra.version, infra.version);
    assert_eq!(loaded_infra.auth_mode, TopicInfraAuthMode::PrivateKey);
    assert_eq!(loaded_infra.allowed_tool_modes, infra.allowed_tool_modes);

    storage
        .put_secret_value(user_id, "storage:ssh/key".to_string(), "secret".to_string())
        .await
        .expect("secret should save");
    assert_eq!(
        storage
            .get_secret_value(user_id, "storage:ssh/key".to_string())
            .await
            .expect("secret should load")
            .as_deref(),
        Some("secret")
    );
    storage
        .delete_secret_value(user_id, "storage:ssh/key".to_string())
        .await
        .expect("secret should delete");
    assert!(
        storage
            .get_secret_value(user_id, "storage:ssh/key".to_string())
            .await
            .expect("secret lookup should succeed")
            .is_none()
    );

    let binding = storage
        .upsert_topic_binding(UpsertTopicBindingOptions {
            user_id,
            topic_id: "topic-a".to_string(),
            agent_id: "ops".to_string(),
            binding_kind: Some(TopicBindingKind::Runtime),
            chat_id: OptionalMetadataPatch::Set(10),
            thread_id: OptionalMetadataPatch::Set(20),
            expires_at: OptionalMetadataPatch::Set(123_456),
            last_activity_at: Some(123_000),
        })
        .await
        .expect("topic binding should upsert");
    assert_eq!(binding.binding_kind, TopicBindingKind::Runtime);
    assert_eq!(binding.chat_id, Some(10));
    assert_eq!(binding.thread_id, Some(20));
    assert_eq!(binding.expires_at, Some(123_456));
}

#[tokio::test]
async fn sqlx_reminder_jobs_claim_and_status_roundtrip() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };
    let user_id = unique_user_id();
    let reminder = storage
        .create_reminder_job(CreateReminderJobOptions {
            user_id,
            context_key: "ctx-reminders".to_string(),
            flow_id: "flow-reminders".to_string(),
            chat_id: 10,
            thread_id: Some(20),
            thread_kind: ReminderThreadKind::Forum,
            task_prompt: "Ping me".to_string(),
            schedule_kind: ReminderScheduleKind::Interval,
            next_run_at: 100,
            interval_secs: Some(60),
            cron_expression: None,
            timezone: None,
        })
        .await
        .expect("reminder should be created");
    assert_eq!(reminder.version, 1);
    assert_eq!(reminder.status, ReminderJobStatus::Scheduled);

    let loaded = storage
        .get_reminder_job(user_id, reminder.reminder_id.clone())
        .await
        .expect("reminder lookup should succeed")
        .expect("reminder should exist");
    assert_eq!(loaded.reminder_id, reminder.reminder_id);
    assert_eq!(loaded.thread_kind, ReminderThreadKind::Forum);
    assert_eq!(loaded.interval_secs, Some(60));

    let listed = storage
        .list_reminder_jobs(
            user_id,
            Some("ctx-reminders".to_string()),
            Some(vec![ReminderJobStatus::Scheduled]),
            10,
        )
        .await
        .expect("reminder list should load");
    assert_eq!(listed.len(), 1);
    let due = storage
        .list_due_reminder_jobs(user_id, 100, 10)
        .await
        .expect("due reminders should load");
    assert_eq!(due.len(), 1);

    let claimed = storage
        .claim_reminder_job(user_id, reminder.reminder_id.clone(), 200, 100)
        .await
        .expect("claim should execute")
        .expect("due reminder should be claimed");
    assert_eq!(claimed.version, reminder.version + 1);
    assert_eq!(claimed.lease_until, Some(200));
    assert!(
        storage
            .claim_reminder_job(user_id, reminder.reminder_id.clone(), 250, 150)
            .await
            .expect("second claim should execute")
            .is_none()
    );

    let reclaimed = storage
        .claim_reminder_job(user_id, reminder.reminder_id.clone(), 300, 200)
        .await
        .expect("expired lease claim should execute")
        .expect("expired lease should allow reclaim");
    assert_eq!(reclaimed.lease_until, Some(300));

    let rescheduled = storage
        .reschedule_reminder_job(
            user_id,
            reminder.reminder_id.clone(),
            400,
            Some(200),
            Some("late".to_string()),
            true,
        )
        .await
        .expect("reschedule should execute")
        .expect("scheduled reminder should reschedule");
    assert_eq!(rescheduled.status, ReminderJobStatus::Scheduled);
    assert_eq!(rescheduled.next_run_at, 400);
    assert_eq!(rescheduled.lease_until, None);
    assert_eq!(rescheduled.run_count, 1);
    assert_eq!(rescheduled.last_error.as_deref(), Some("late"));

    let paused = storage
        .pause_reminder_job(user_id, reminder.reminder_id.clone(), 401)
        .await
        .expect("pause should execute")
        .expect("scheduled reminder should pause");
    assert_eq!(paused.status, ReminderJobStatus::Paused);
    let resumed = storage
        .resume_reminder_job(user_id, reminder.reminder_id.clone(), 500, 402)
        .await
        .expect("resume should execute")
        .expect("paused reminder should resume");
    assert_eq!(resumed.status, ReminderJobStatus::Scheduled);
    assert_eq!(resumed.next_run_at, 500);

    let failed = storage
        .fail_reminder_job(
            user_id,
            reminder.reminder_id.clone(),
            501,
            "boom".to_string(),
        )
        .await
        .expect("fail should execute")
        .expect("scheduled reminder should fail");
    assert_eq!(failed.status, ReminderJobStatus::Failed);
    assert_eq!(failed.last_error.as_deref(), Some("boom"));
    let retried = storage
        .retry_reminder_job(user_id, reminder.reminder_id.clone(), 600, 502)
        .await
        .expect("retry should execute")
        .expect("failed reminder should retry");
    assert_eq!(retried.status, ReminderJobStatus::Scheduled);
    assert_eq!(retried.last_error, None);

    let cancelled = storage
        .cancel_reminder_job(user_id, reminder.reminder_id.clone(), 601)
        .await
        .expect("cancel should execute")
        .expect("scheduled reminder should cancel");
    assert_eq!(cancelled.status, ReminderJobStatus::Cancelled);
    storage
        .delete_reminder_job(user_id, reminder.reminder_id.clone())
        .await
        .expect("delete should execute");
    assert!(
        storage
            .get_reminder_job(user_id, reminder.reminder_id)
            .await
            .expect("lookup after delete should execute")
            .is_none()
    );
}

#[tokio::test]
async fn sqlx_reminder_claim_is_single_winner() {
    let Some(storage) = sqlx_test_storage_with_connections(4).await else {
        return;
    };
    let user_id = unique_user_id();
    let reminder = storage
        .create_reminder_job(CreateReminderJobOptions {
            user_id,
            context_key: "ctx-concurrent".to_string(),
            flow_id: "flow-concurrent".to_string(),
            chat_id: 10,
            thread_id: None,
            thread_kind: ReminderThreadKind::Dm,
            task_prompt: "Ping once".to_string(),
            schedule_kind: ReminderScheduleKind::Once,
            next_run_at: 100,
            interval_secs: None,
            cron_expression: None,
            timezone: None,
        })
        .await
        .expect("reminder should be created");

    let first_storage = storage.clone();
    let second_storage = storage.clone();
    let first_id = reminder.reminder_id.clone();
    let second_id = reminder.reminder_id.clone();
    let (first, second) = tokio::join!(
        first_storage.claim_reminder_job(user_id, first_id, 200, 100),
        second_storage.claim_reminder_job(user_id, second_id, 200, 100),
    );
    let claims = [first, second]
        .into_iter()
        .map(|result| result.expect("claim should execute"))
        .filter(Option::is_some)
        .count();
    assert_eq!(claims, 1);
    assert!(
        storage
            .list_due_reminder_jobs(user_id, 150, 10)
            .await
            .expect("due list should execute")
            .is_empty()
    );
}

#[tokio::test]
async fn sqlx_audit_events_append_and_page_by_version() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };
    let user_id = unique_user_id();

    let first = storage
        .append_audit_event(AppendAuditEventOptions {
            user_id,
            topic_id: Some("topic-a".to_string()),
            agent_id: Some("agent-a".to_string()),
            action: "first".to_string(),
            payload: json!({"n": 1}),
        })
        .await
        .expect("first audit event should append");
    let second = storage
        .append_audit_event(AppendAuditEventOptions {
            user_id,
            topic_id: Some("topic-a".to_string()),
            agent_id: None,
            action: "second".to_string(),
            payload: json!({"n": 2}),
        })
        .await
        .expect("second audit event should append");
    let third = storage
        .append_audit_event(AppendAuditEventOptions {
            user_id,
            topic_id: None,
            agent_id: None,
            action: "third".to_string(),
            payload: json!({"n": 3}),
        })
        .await
        .expect("third audit event should append");
    assert_eq!([first.version, second.version, third.version], [1, 2, 3]);

    let recent_versions: Vec<u64> = storage
        .list_audit_events(user_id, 2)
        .await
        .expect("recent audit events should load")
        .iter()
        .map(|event| event.version)
        .collect();
    assert_eq!(recent_versions, vec![2, 3]);

    let first_page_versions: Vec<u64> = storage
        .list_audit_events_page(user_id, None, 2)
        .await
        .expect("audit page should load")
        .iter()
        .map(|event| event.version)
        .collect();
    let second_page_versions: Vec<u64> = storage
        .list_audit_events_page(user_id, Some(2), 2)
        .await
        .expect("audit cursor page should load")
        .iter()
        .map(|event| event.version)
        .collect();
    assert_eq!(first_page_versions, vec![3, 2]);
    assert_eq!(second_page_versions, vec![1]);

    let other_user = unique_user_id();
    let other = storage
        .append_audit_event(AppendAuditEventOptions {
            user_id: other_user,
            topic_id: None,
            agent_id: None,
            action: "other".to_string(),
            payload: json!({}),
        })
        .await
        .expect("other user audit stream should append");
    assert_eq!(other.version, 1);
}

async fn sqlx_test_storage() -> Option<SqlxStorage> {
    sqlx_test_storage_with_connections(1).await
}

async fn sqlx_test_storage_with_connections(max_connections: u32) -> Option<SqlxStorage> {
    let Ok(database_url) = std::env::var("OXIDE_DATABASE_TEST_URL") else {
        eprintln!("OXIDE_DATABASE_TEST_URL not set; skipping SQLx/Postgres test");
        return None;
    };

    let migrations_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("migrations");
    let config = SqlxStorageConfig {
        database_url,
        max_connections,
        connect_timeout: Duration::from_secs(5),
        migrate_on_startup: true,
        migrations_dir,
    };

    Some(
        SqlxStorage::connect(config)
            .await
            .expect("SQLx storage should connect and run migrations"),
    )
}

fn unique_user_id() -> i64 {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_micros() as i64;
    1_000_000_000_000 + (micros % 1_000_000_000_000) + USER_COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn assert_memory_eq(expected: &AgentMemory, actual: &AgentMemory) {
    assert_eq!(
        serde_json::to_value(expected).expect("expected memory should serialize"),
        serde_json::to_value(actual).expect("actual memory should serialize")
    );
}

// ---------------------------------------------------------------------------
// Browser artifact storage tests
// ---------------------------------------------------------------------------

/// Create prerequisite `users` row. No FK chain needed — `browser_artifacts`
/// has no FK after migration 0009. Returns `(user_id, context_key)`.
async fn setup_browser_artifact_scope(storage: &SqlxStorage) -> (i64, String) {
    let user_id = unique_user_id();
    let context_key = format!("web-session-{user_id}");

    query::<Postgres>("INSERT INTO users (user_id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(user_id)
        .execute(storage.pool())
        .await
        .expect("insert users");

    (user_id, context_key)
}

/// Clean up the prerequisite user row.
async fn cleanup_browser_artifact_scope(storage: &SqlxStorage, user_id: i64) {
    let _ = query::<Postgres>("DELETE FROM users WHERE user_id = $1")
        .bind(user_id)
        .execute(storage.pool())
        .await;
}

#[tokio::test]
async fn sqlx_browser_artifact_save_load_round_trip() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };
    let (user_id, context_key) = setup_browser_artifact_scope(&storage).await;

    let session_id = format!("br-{user_id}");
    let task_id = format!("task-{user_id}");
    let artifact_uri = format!("artifact://browser/{task_id}/{session_id}/step-0001-milestone.jpg");
    let test_data = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46]; // JPEG SOI + APP0
    let sha256 = format!("{:x}", sha2::Digest::finalize(sha2::Sha256::new()));

    let record = BrowserArtifactRecord {
        artifact_uri: artifact_uri.clone(),
        user_id,
        context_key: context_key.clone(),
        session_id: session_id.clone(),
        task_id: task_id.clone(),
        mime_type: "image/jpeg".to_string(),
        data: test_data.clone(),
        bytes: test_data.len() as i64,
        sha256: Some(sha256),
    };

    storage
        .save_browser_artifact(record)
        .await
        .expect("save_browser_artifact should succeed");

    let loaded = storage
        .load_browser_artifact(user_id, &artifact_uri)
        .await
        .expect("load_browser_artifact should succeed")
        .expect("artifact should exist after save");

    assert_eq!(loaded.mime_type, "image/jpeg");
    assert_eq!(loaded.data, test_data);
    assert_eq!(loaded.bytes, test_data.len() as i64);

    // Upsert: save again with different data.
    let updated_data = vec![0xff, 0xd8, 0xff, 0xe1, 0x00, 0x10];
    let record2 = BrowserArtifactRecord {
        artifact_uri: artifact_uri.clone(),
        user_id,
        context_key: context_key.clone(),
        session_id: session_id.clone(),
        task_id: task_id.clone(),
        mime_type: "image/jpeg".to_string(),
        data: updated_data.clone(),
        bytes: updated_data.len() as i64,
        sha256: None,
    };
    storage
        .save_browser_artifact(record2)
        .await
        .expect("upsert should succeed");

    let loaded2 = storage
        .load_browser_artifact(user_id, &artifact_uri)
        .await
        .expect("load after upsert")
        .expect("artifact should exist after upsert");
    assert_eq!(loaded2.data, updated_data);

    // Load non-existent URI.
    let missing = storage
        .load_browser_artifact(user_id, "artifact://browser/nonexistent/test.jpg")
        .await
        .expect("load non-existent should not error");
    assert!(missing.is_none());

    cleanup_browser_artifact_scope(&storage, user_id).await;
}

#[tokio::test]
async fn sqlx_browser_artifact_delete_by_context_key() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };
    let (user_id, context_key) = setup_browser_artifact_scope(&storage).await;

    let session_id = format!("br-{user_id}");
    let task_id = format!("task-{user_id}");

    // Save two artifacts for the same context_key.
    for i in 0..2u8 {
        let uri = format!("artifact://browser/{task_id}/{session_id}/step-{i:04}-live.jpg");
        storage
            .save_browser_artifact(BrowserArtifactRecord {
                artifact_uri: uri,
                user_id,
                context_key: context_key.clone(),
                session_id: session_id.clone(),
                task_id: task_id.clone(),
                mime_type: "image/jpeg".to_string(),
                data: vec![0xff, 0xd8, 0xff, i],
                bytes: 4,
                sha256: None,
            })
            .await
            .expect("save artifact");
    }

    // Delete by context_key.
    let deleted = storage
        .delete_browser_artifacts_by_context_key(user_id, &context_key)
        .await
        .expect("delete by context_key should succeed");
    assert_eq!(deleted, 2);

    // Verify they're gone.
    let uri = format!("artifact://browser/{task_id}/{session_id}/step-0000-live.jpg");
    let loaded = storage
        .load_browser_artifact(user_id, &uri)
        .await
        .expect("load after delete should not error");
    assert!(loaded.is_none());

    cleanup_browser_artifact_scope(&storage, user_id).await;
}

#[tokio::test]
async fn sqlx_browser_artifact_isolation_by_context_key() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };
    let (user_id, context_key_a) = setup_browser_artifact_scope(&storage).await;
    let context_key_b = format!("web-session-other-{user_id}");

    let session_id = format!("br-{user_id}");
    let task_id = format!("task-{user_id}");

    // Save one artifact for context_key_a and one for context_key_b.
    let uri_a = format!("artifact://browser/{task_id}/{session_id}/step-0001-final.jpg");
    storage
        .save_browser_artifact(BrowserArtifactRecord {
            artifact_uri: uri_a.clone(),
            user_id,
            context_key: context_key_a.clone(),
            session_id: session_id.clone(),
            task_id: task_id.clone(),
            mime_type: "image/jpeg".to_string(),
            data: vec![0xff, 0xd8, 0xff, 0xd9],
            bytes: 4,
            sha256: None,
        })
        .await
        .expect("save artifact A");

    let uri_b = format!("artifact://browser/{task_id}/{session_id}/step-0002-final.jpg");
    storage
        .save_browser_artifact(BrowserArtifactRecord {
            artifact_uri: uri_b.clone(),
            user_id,
            context_key: context_key_b.clone(),
            session_id: session_id.clone(),
            task_id: task_id.clone(),
            mime_type: "image/jpeg".to_string(),
            data: vec![0xff, 0xd8, 0xff, 0xd8],
            bytes: 4,
            sha256: None,
        })
        .await
        .expect("save artifact B");

    // Delete only context_key_a — context_key_b must survive.
    let deleted = storage
        .delete_browser_artifacts_by_context_key(user_id, &context_key_a)
        .await
        .expect("delete by context_key_a should succeed");
    assert_eq!(deleted, 1);

    let loaded_a = storage
        .load_browser_artifact(user_id, &uri_a)
        .await
        .expect("load after delete should not error");
    assert!(loaded_a.is_none(), "artifact A should be deleted");

    let loaded_b = storage
        .load_browser_artifact(user_id, &uri_b)
        .await
        .expect("load survivor should not error")
        .expect("artifact B should still exist");
    assert_eq!(loaded_b.data, vec![0xff, 0xd8, 0xff, 0xd8]);

    // Clean up remaining artifact B.
    storage
        .delete_browser_artifacts_by_context_key(user_id, &context_key_b)
        .await
        .expect("cleanup context_key_b");

    cleanup_browser_artifact_scope(&storage, user_id).await;
}

// ---------------------------------------------------------------------------
// Browser artifact retention tests
// ---------------------------------------------------------------------------

/// Helper: insert an artifact, then set its `created_at` to a known value.
async fn insert_artifact_at(
    storage: &SqlxStorage,
    user_id: i64,
    context_key: &str,
    uri: &str,
    data: Vec<u8>,
    created_at: DateTime<Utc>,
) {
    let len = data.len() as i64;
    storage
        .save_browser_artifact(BrowserArtifactRecord {
            artifact_uri: uri.to_string(),
            user_id,
            context_key: context_key.to_string(),
            session_id: "br-test".to_string(),
            task_id: "task-test".to_string(),
            mime_type: "image/jpeg".to_string(),
            data,
            bytes: len,
            sha256: None,
        })
        .await
        .expect("save artifact");

    query::<Postgres>("UPDATE browser_artifacts SET created_at = $1 WHERE artifact_uri = $2")
        .bind(created_at)
        .bind(uri)
        .execute(storage.pool())
        .await
        .expect("update created_at");
}

#[tokio::test]
async fn sqlx_browser_artifact_retention_delete_before() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };
    let (user_id, context_key) = setup_browser_artifact_scope(&storage).await;

    let now = Utc::now();
    let old = now - chrono::Duration::days(10);
    let medium = now - chrono::Duration::days(5);
    let recent = now - chrono::Duration::days(1);

    let uri_old = format!("artifact://browser/task-{user_id}/br/step-0001-old.jpg");
    let uri_med = format!("artifact://browser/task-{user_id}/br/step-0002-med.jpg");
    let uri_new = format!("artifact://browser/task-{user_id}/br/step-0003-new.jpg");

    insert_artifact_at(
        &storage,
        user_id,
        &context_key,
        &uri_old,
        vec![0xff; 50],
        old,
    )
    .await;
    insert_artifact_at(
        &storage,
        user_id,
        &context_key,
        &uri_med,
        vec![0xff; 50],
        medium,
    )
    .await;
    insert_artifact_at(
        &storage,
        user_id,
        &context_key,
        &uri_new,
        vec![0xff; 50],
        recent,
    )
    .await;

    // Cutoff = 7 days ago → only the 10-day-old artifact should be deleted.
    let cutoff = now - chrono::Duration::days(7);
    let deleted = storage
        .delete_browser_artifacts_before(cutoff)
        .await
        .expect("delete_browser_artifacts_before");
    assert_eq!(deleted, 1, "only the oldest artifact should be deleted");

    // Verify medium and recent survive.
    let med = storage
        .load_browser_artifact(user_id, &uri_med)
        .await
        .expect("load med");
    assert!(med.is_some(), "medium-age artifact should survive");

    let new = storage
        .load_browser_artifact(user_id, &uri_new)
        .await
        .expect("load new");
    assert!(new.is_some(), "recent artifact should survive");

    // Verify old is gone.
    let old_loaded = storage
        .load_browser_artifact(user_id, &uri_old)
        .await
        .expect("load old");
    assert!(old_loaded.is_none(), "old artifact should be deleted");

    cleanup_browser_artifact_scope(&storage, user_id).await;
}

#[tokio::test]
async fn sqlx_browser_artifact_retention_soft_cap() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };
    let (user_id, context_key) = setup_browser_artifact_scope(&storage).await;

    let now = Utc::now();
    let base = now - chrono::Duration::days(10);

    // Insert 3 artifacts: A=100B (oldest), B=200B, C=150B (newest).
    // Total = 450B. Cap = 250B.
    // Keep newest first: C=150, C+B=350 → B and A exceed cap → deleted.
    // After: only C remains (150B ≤ 250B).
    let uri_a = format!("artifact://browser/task-{user_id}/br/step-0001-a.jpg");
    let uri_b = format!("artifact://browser/task-{user_id}/br/step-0002-b.jpg");
    let uri_c = format!("artifact://browser/task-{user_id}/br/step-0003-c.jpg");

    insert_artifact_at(
        &storage,
        user_id,
        &context_key,
        &uri_a,
        vec![0xff; 100],
        base,
    )
    .await;
    insert_artifact_at(
        &storage,
        user_id,
        &context_key,
        &uri_b,
        vec![0xff; 200],
        base + chrono::Duration::days(1),
    )
    .await;
    insert_artifact_at(
        &storage,
        user_id,
        &context_key,
        &uri_c,
        vec![0xff; 150],
        base + chrono::Duration::days(2),
    )
    .await;

    let deleted = storage
        .delete_browser_artifacts_oldest_until_cap(250)
        .await
        .expect("delete_browser_artifacts_oldest_until_cap");
    assert_eq!(deleted, 2, "should delete 2 oldest artifacts to fit cap");

    // Verify only C (newest, 150B) survives.
    let a = storage
        .load_browser_artifact(user_id, &uri_a)
        .await
        .expect("load a");
    assert!(a.is_none(), "artifact A (oldest) should be deleted");

    let b = storage
        .load_browser_artifact(user_id, &uri_b)
        .await
        .expect("load b");
    assert!(b.is_none(), "artifact B (middle) should be deleted");

    let c = storage
        .load_browser_artifact(user_id, &uri_c)
        .await
        .expect("load c");
    assert!(c.is_some(), "artifact C (newest) should survive");

    cleanup_browser_artifact_scope(&storage, user_id).await;
}

#[tokio::test]
async fn sqlx_browser_artifact_retention_soft_cap_under_cap_noop() {
    let Some(storage) = sqlx_test_storage().await else {
        return;
    };
    let (user_id, context_key) = setup_browser_artifact_scope(&storage).await;

    let now = Utc::now();

    // Insert 1 artifact: 100B. Cap = 1000B. Total (100B) ≤ cap → no deletion.
    let uri = format!("artifact://browser/task-{user_id}/br/step-0001.jpg");
    insert_artifact_at(&storage, user_id, &context_key, &uri, vec![0xff; 100], now).await;

    let deleted = storage
        .delete_browser_artifacts_oldest_until_cap(1000)
        .await
        .expect("delete under cap");
    assert_eq!(deleted, 0, "should not delete anything when under cap");

    let loaded = storage
        .load_browser_artifact(user_id, &uri)
        .await
        .expect("load");
    assert!(loaded.is_some(), "artifact should survive when under cap");

    cleanup_browser_artifact_scope(&storage, user_id).await;
}
