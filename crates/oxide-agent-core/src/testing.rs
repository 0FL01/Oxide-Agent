//! Testing helpers and mock utilities.
//!
//! Provides convenient constructors for mocked LLM and storage providers.

use crate::llm::LlmError;
use crate::storage::{
    AgentFlowRecord, AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord,
    CreateReminderJobOptions, ReminderJobRecord, ReminderJobStatus, TopicBindingKind,
    TopicBindingRecord, UpsertAgentProfileOptions, UpsertTopicBindingOptions, UserConfig,
};
use mockall::predicate::*;

/// Create a mock LLM provider that returns a simple text response.
///
/// # Returns
///
/// A `MockLlmProvider` that returns the provided `response_text` for all
/// `chat_completion` calls. Other methods return error by default.
///
/// # Example
///
/// ```rust,ignore
/// use oxide_agent_core::testing::mock_llm_simple;
///
/// let mut mock = mock_llm_simple("Hello, world!");
/// // Use the mock in tests...
/// ```
#[must_use]
pub fn mock_llm_simple(response_text: &'static str) -> crate::llm::MockLlmProvider {
    let mut mock = crate::llm::MockLlmProvider::new();
    mock.expect_chat_completion()
        .with(always(), always(), always(), always(), always())
        .returning(move |_, _, _, _, _| Ok(response_text.to_string()));

    mock.expect_transcribe_audio()
        .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));

    mock.expect_analyze_image()
        .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));

    mock
}

/// Create a mock storage provider that performs no operations (noop).
///
/// All methods return default/empty values without errors, making it suitable
/// for tests that don't require persistent storage.
///
/// # Returns
///
/// A `MockStorageProvider` where:
/// - `get_user_config` returns a default empty `UserConfig`
/// - `update_user_config` / `update_user_prompt` / `update_user_model` / `update_user_state` return `Ok(())`
/// - `get_user_prompt` / `get_user_model` / `get_user_state` return `Ok(None)`
/// - `save_message` returns `Ok(())`
/// - `get_chat_history` returns an empty `Vec<Message>`
/// - `save_message_for_chat` / `get_chat_history_for_chat` / `clear_chat_history_for_chat` return `Ok(())`/empty
/// - `clear_chat_history` / `clear_agent_memory` / `clear_all_context` return `Ok(())`
/// - `save_agent_memory` returns `Ok(())`
/// - `load_agent_memory` returns `Ok(None)`
/// - `check_connection` returns `Ok(())`
/// - control-plane methods return `Ok(None)` / `Ok(())` / empty lists with
///   minimal records for append/upsert calls
///
/// # Example
///
/// ```rust,ignore
/// use oxide_agent_core::testing::mock_storage_noop;
///
/// let storage = mock_storage_noop();
/// // Use the mock in tests that don't need storage...
/// ```
#[must_use]
pub fn mock_storage_noop() -> crate::storage::MockStorageProvider {
    let mut mock = crate::storage::MockStorageProvider::new();

    configure_basic_expectations(&mut mock);
    configure_chat_expectations(&mut mock);
    configure_agent_expectations(&mut mock);
    configure_control_plane_expectations(&mut mock);

    mock
}

fn configure_basic_expectations(mock: &mut crate::storage::MockStorageProvider) {
    mock.expect_get_user_config()
        .returning(|_| Ok(UserConfig::default()));

    mock.expect_update_user_config().returning(|_, _| Ok(()));
    mock.expect_update_user_prompt().returning(|_, _| Ok(()));
    mock.expect_get_user_prompt().returning(|_| Ok(None));
    mock.expect_update_user_model().returning(|_, _| Ok(()));
    mock.expect_get_user_model().returning(|_| Ok(None));
    mock.expect_update_user_state().returning(|_, _| Ok(()));
    mock.expect_get_user_state().returning(|_| Ok(None));
    mock.expect_check_connection().returning(|| Ok(()));
    mock.expect_clear_all_context().returning(|_| Ok(()));
}

fn configure_chat_expectations(mock: &mut crate::storage::MockStorageProvider) {
    mock.expect_save_message().returning(|_, _, _| Ok(()));
    mock.expect_get_chat_history()
        .returning(|_, _| Ok(Vec::new()));
    mock.expect_clear_chat_history().returning(|_| Ok(()));
    mock.expect_save_message_for_chat()
        .returning(|_, _, _, _| Ok(()));
    mock.expect_get_chat_history_for_chat()
        .returning(|_, _, _| Ok(Vec::new()));
    mock.expect_clear_chat_history_for_chat()
        .returning(|_, _| Ok(()));
}

fn configure_agent_expectations(mock: &mut crate::storage::MockStorageProvider) {
    mock.expect_save_agent_memory().returning(|_, _| Ok(()));
    mock.expect_save_agent_memory_for_context()
        .returning(|_, _, _| Ok(()));
    mock.expect_load_agent_memory().returning(|_| Ok(None));
    mock.expect_load_agent_memory_for_context()
        .returning(|_, _| Ok(None));
    mock.expect_clear_agent_memory().returning(|_| Ok(()));
    mock.expect_clear_agent_memory_for_context()
        .returning(|_, _| Ok(()));
    mock.expect_save_agent_memory_for_flow()
        .returning(|_, _, _, _| Ok(()));
    mock.expect_load_agent_memory_for_flow()
        .returning(|_, _, _| Ok(None));
    mock.expect_clear_agent_memory_for_flow()
        .returning(|_, _, _| Ok(()));
    mock.expect_get_agent_flow_record()
        .returning(|_, _, _| Ok(None));
    mock.expect_upsert_agent_flow_record()
        .returning(|user_id, context_key, flow_id| {
            Ok(AgentFlowRecord {
                schema_version: 1,
                user_id,
                context_key,
                flow_id,
                created_at: 0,
                updated_at: 0,
            })
        });
}

fn configure_control_plane_expectations(mock: &mut crate::storage::MockStorageProvider) {
    mock.expect_get_agent_profile().returning(|_, _| Ok(None));
    mock.expect_upsert_agent_profile()
        .returning(|options: UpsertAgentProfileOptions| {
            Ok(AgentProfileRecord {
                schema_version: 1,
                version: 1,
                user_id: options.user_id,
                agent_id: options.agent_id,
                profile: options.profile,
                created_at: 0,
                updated_at: 0,
            })
        });
    mock.expect_delete_agent_profile().returning(|_, _| Ok(()));
    mock.expect_get_topic_binding().returning(|_, _| Ok(None));
    mock.expect_upsert_topic_binding()
        .returning(|options: UpsertTopicBindingOptions| {
            Ok(TopicBindingRecord {
                schema_version: 1,
                version: 1,
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                chat_id: options.chat_id.for_new_record(),
                thread_id: options.thread_id.for_new_record(),
                expires_at: options.expires_at.for_new_record(),
                last_activity_at: options.last_activity_at,
                created_at: 0,
                updated_at: 0,
            })
        });
    mock.expect_delete_topic_binding().returning(|_, _| Ok(()));
    mock.expect_append_audit_event()
        .returning(|options: AppendAuditEventOptions| {
            Ok(AuditEventRecord {
                schema_version: 1,
                version: 1,
                event_id: "mock-audit-event".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 0,
            })
        });
    mock.expect_list_audit_events()
        .returning(|_, _| Ok(Vec::new()));
    mock.expect_list_audit_events_page()
        .returning(|_, _, _| Ok(Vec::new()));
    mock.expect_create_reminder_job()
        .returning(|options: CreateReminderJobOptions| {
            Ok(ReminderJobRecord {
                schema_version: 1,
                version: 1,
                reminder_id: "mock-reminder".to_string(),
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
                lease_until: None,
                last_run_at: None,
                last_error: None,
                run_count: 0,
                created_at: 0,
                updated_at: 0,
            })
        });
    mock.expect_get_reminder_job().returning(|_, _| Ok(None));
    mock.expect_list_reminder_jobs()
        .returning(|_, _, _, _| Ok(Vec::new()));
    mock.expect_list_due_reminder_jobs()
        .returning(|_, _, _| Ok(Vec::new()));
    mock.expect_claim_reminder_job()
        .returning(|_, _, _, _| Ok(None));
    mock.expect_reschedule_reminder_job()
        .returning(|_, _, _, _, _, _| Ok(None));
    mock.expect_complete_reminder_job()
        .returning(|_, _, _| Ok(None));
    mock.expect_fail_reminder_job()
        .returning(|_, _, _, _| Ok(None));
    mock.expect_cancel_reminder_job()
        .returning(|_, _, _| Ok(None));
    mock.expect_pause_reminder_job()
        .returning(|_, _, _| Ok(None));
    mock.expect_resume_reminder_job()
        .returning(|_, _, _, _| Ok(None));
    mock.expect_retry_reminder_job()
        .returning(|_, _, _, _| Ok(None));
}
