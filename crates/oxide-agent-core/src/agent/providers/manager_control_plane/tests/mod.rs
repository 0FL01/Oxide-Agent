// Allow clone_on_ref_ptr in tests due to trait object coercion requirements
#![allow(clippy::clone_on_ref_ptr)]

use super::*;
use crate::agent::registry::ToolRegistry;
use crate::storage::{
    AgentProfileRecord, AppendAuditEventOptions, TopicAgentsMdRecord, TopicBindingRecord,
    TopicContextRecord, TopicInfraAuthMode, TopicInfraConfigRecord, TopicInfraToolMode,
};
use mockall::{predicate::eq, Sequence};

fn binding(user_id: i64, topic_id: &str, agent_id: &str, version: u64) -> TopicBindingRecord {
    TopicBindingRecord {
        schema_version: 1,
        version,
        user_id,
        topic_id: topic_id.to_string(),
        agent_id: agent_id.to_string(),
        binding_kind: TopicBindingKind::Manual,
        chat_id: None,
        thread_id: None,
        expires_at: None,
        last_activity_at: Some(20),
        created_at: 10,
        updated_at: 20,
    }
}

fn topic_infra(user_id: i64, topic_id: &str, version: u64) -> TopicInfraConfigRecord {
    TopicInfraConfigRecord {
        schema_version: 1,
        version,
        user_id,
        topic_id: topic_id.to_string(),
        target_name: "prod-app".to_string(),
        host: "prod.example.com".to_string(),
        port: 22,
        remote_user: "deploy".to_string(),
        auth_mode: TopicInfraAuthMode::PrivateKey,
        secret_ref: Some("storage:ssh/prod-key".to_string()),
        sudo_secret_ref: Some("storage:ssh/prod-sudo".to_string()),
        environment: Some("prod".to_string()),
        tags: vec!["prod".to_string()],
        allowed_tool_modes: vec![TopicInfraToolMode::Exec, TopicInfraToolMode::ReadFile],
        approval_required_modes: vec![TopicInfraToolMode::SudoExec],
        created_at: 10,
        updated_at: 20,
    }
}

fn expect_forum_topic_provision_profile_calls(mock: &mut crate::storage::MockStorageProvider) {
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("n-ru1".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_upsert_agent_profile()
        .withf(|options| {
            options.agent_id == "n-ru1"
                && options
                    .profile
                    .get("allowedTools")
                    .and_then(|value| value.as_array())
                    .is_some()
                && options
                    .profile
                    .get("blockedTools")
                    .and_then(|value| value.as_array())
                    .is_some_and(|tools| {
                        topic_agent_default_blocked_tools().iter().all(|tool| {
                            tools
                                .iter()
                                .any(|value| value.as_str() == Some(tool.as_str()))
                        })
                    })
        })
        .returning(|options| {
            Ok(AgentProfileRecord {
                schema_version: 1,
                version: 1,
                user_id: options.user_id,
                agent_id: options.agent_id,
                profile: options.profile,
                created_at: 10,
                updated_at: 10,
            })
        });
}

fn expect_forum_topic_provision_binding_calls(mock: &mut crate::storage::MockStorageProvider) {
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("-100777:313".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_upsert_topic_binding()
        .withf(|options| {
            options.topic_id == "-100777:313"
                && options.agent_id == "n-ru1"
                && options.chat_id == OptionalMetadataPatch::Set(-100777)
                && options.thread_id == OptionalMetadataPatch::Set(313)
        })
        .returning(|options| {
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
                created_at: 10,
                updated_at: 10,
            })
        });
}

fn expect_forum_topic_provision_infra_calls(mock: &mut crate::storage::MockStorageProvider) {
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("-100777:313".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_upsert_topic_infra_config()
        .withf(|options| {
            options.topic_id == "-100777:313"
                && options.target_name == "n-ru1"
                && options.auth_mode == TopicInfraAuthMode::None
        })
        .returning(|options| {
            Ok(TopicInfraConfigRecord {
                schema_version: 1,
                version: 1,
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
                created_at: 10,
                updated_at: 10,
            })
        });
}

fn mock_storage_for_forum_topic_provision() -> crate::storage::MockStorageProvider {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config()
        .times(1)
        .returning(|_| Ok(crate::storage::UserConfig::default()));
    mock.expect_update_user_config()
        .times(1)
        .withf(|user_id, config| *user_id == 77 && config.contexts.contains_key("-100777:313"))
        .returning(|_, _| Ok(()));
    expect_forum_topic_provision_profile_calls(&mut mock);
    expect_forum_topic_provision_binding_calls(&mut mock);
    expect_forum_topic_provision_infra_calls(&mut mock);
    mock.expect_append_audit_event()
        .times(4)
        .returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                options.agent_id.as_deref(),
                &options.action,
                options.payload,
            ))
        });
    mock
}

fn audit_event(
    version: u64,
    topic_id: Option<&str>,
    agent_id: Option<&str>,
    action: &str,
    payload: serde_json::Value,
) -> crate::storage::AuditEventRecord {
    crate::storage::AuditEventRecord {
        schema_version: 1,
        version,
        event_id: format!("evt-{version}"),
        user_id: 77,
        topic_id: topic_id.map(str::to_string),
        agent_id: agent_id.map(str::to_string),
        action: action.to_string(),
        payload,
        created_at: 100,
    }
}

fn topic_delete_user_config() -> crate::storage::UserConfig {
    crate::storage::UserConfig {
        contexts: std::collections::HashMap::from([(
            "-100999:42".to_string(),
            crate::storage::UserContextConfig {
                state: Some("agent_mode".to_string()),
                current_chat_uuid: Some("chat-1".to_string()),
                current_agent_flow_id: Some("flow-1".to_string()),
                chat_id: Some(-100999),
                thread_id: Some(42),
                forum_topic_name: Some("topic-42".to_string()),
                forum_topic_icon_color: Some(7_322_096),
                forum_topic_icon_custom_emoji_id: None,
                forum_topic_closed: false,
            },
        )]),
        ..crate::storage::UserConfig::default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum LifecycleCall {
    Create(ForumTopicCreateRequest),
    Edit(ForumTopicEditRequest),
    Close(ForumTopicThreadRequest),
    Reopen(ForumTopicThreadRequest),
    Delete(ForumTopicThreadRequest),
}

struct FakeTopicLifecycle {
    calls: std::sync::Mutex<Vec<LifecycleCall>>,
}

impl FakeTopicLifecycle {
    fn new() -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<LifecycleCall> {
        self.calls.lock().expect("mutex poisoned").clone()
    }
}

#[async_trait]
impl ManagerTopicLifecycle for FakeTopicLifecycle {
    fn default_forum_chat_id(&self) -> Option<i64> {
        Some(-100_777)
    }

    async fn forum_topic_create(
        &self,
        request: ForumTopicCreateRequest,
    ) -> Result<ForumTopicCreateResult> {
        self.calls
            .lock()
            .expect("mutex poisoned")
            .push(LifecycleCall::Create(request.clone()));
        Ok(ForumTopicCreateResult {
            chat_id: request.chat_id.unwrap_or(-100_777),
            thread_id: 313,
            name: request.name,
            icon_color: request.icon_color.unwrap_or(9_367_192),
            icon_custom_emoji_id: request.icon_custom_emoji_id,
        })
    }

    async fn forum_topic_edit(
        &self,
        request: ForumTopicEditRequest,
    ) -> Result<ForumTopicEditResult> {
        self.calls
            .lock()
            .expect("mutex poisoned")
            .push(LifecycleCall::Edit(request.clone()));
        Ok(ForumTopicEditResult {
            chat_id: request.chat_id.unwrap_or(-100_777),
            thread_id: request.thread_id,
            name: request.name,
            icon_custom_emoji_id: request.icon_custom_emoji_id,
        })
    }

    async fn forum_topic_close(
        &self,
        request: ForumTopicThreadRequest,
    ) -> Result<ForumTopicActionResult> {
        self.calls
            .lock()
            .expect("mutex poisoned")
            .push(LifecycleCall::Close(request.clone()));
        Ok(ForumTopicActionResult {
            chat_id: request.chat_id.unwrap_or(-100_777),
            thread_id: request.thread_id,
        })
    }

    async fn forum_topic_reopen(
        &self,
        request: ForumTopicThreadRequest,
    ) -> Result<ForumTopicActionResult> {
        self.calls
            .lock()
            .expect("mutex poisoned")
            .push(LifecycleCall::Reopen(request.clone()));
        Ok(ForumTopicActionResult {
            chat_id: request.chat_id.unwrap_or(-100_777),
            thread_id: request.thread_id,
        })
    }

    async fn forum_topic_delete(
        &self,
        request: ForumTopicThreadRequest,
    ) -> Result<ForumTopicActionResult> {
        self.calls
            .lock()
            .expect("mutex poisoned")
            .push(LifecycleCall::Delete(request.clone()));
        Ok(ForumTopicActionResult {
            chat_id: request.chat_id.unwrap_or(-100_777),
            thread_id: request.thread_id,
        })
    }
}

struct FakeTopicSandboxCleanup {
    calls: std::sync::Mutex<Vec<(i64, i64, i64)>>,
}

impl FakeTopicSandboxCleanup {
    fn new() -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<(i64, i64, i64)> {
        self.calls.lock().expect("mutex poisoned").clone()
    }
}

#[async_trait]
impl ManagerTopicSandboxCleanup for FakeTopicSandboxCleanup {
    async fn cleanup_topic_sandbox(
        &self,
        user_id: i64,
        topic: &ForumTopicActionResult,
    ) -> Result<()> {
        self.calls
            .lock()
            .expect("mutex poisoned")
            .push((user_id, topic.chat_id, topic.thread_id));
        Ok(())
    }
}

struct FakeTopicSandboxControl {
    records: std::sync::Mutex<std::collections::HashMap<String, SandboxContainerRecord>>,
    ensured: std::sync::Mutex<Vec<String>>,
    recreated: std::sync::Mutex<Vec<String>>,
    deleted: std::sync::Mutex<Vec<String>>,
}

impl FakeTopicSandboxControl {
    fn new(records: Vec<SandboxContainerRecord>) -> Self {
        Self {
            records: std::sync::Mutex::new(
                records
                    .into_iter()
                    .map(|record| (record.container_name.clone(), record))
                    .collect(),
            ),
            ensured: std::sync::Mutex::new(Vec::new()),
            recreated: std::sync::Mutex::new(Vec::new()),
            deleted: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn sandbox_record(user_id: i64, topic_id: &str) -> SandboxContainerRecord {
        let (chat_id, thread_id) =
            ManagerControlPlaneProvider::parse_canonical_forum_topic_id(topic_id)
                .expect("topic id must be canonical");
        let scope = SandboxScope::new(user_id, topic_id.to_string())
            .with_transport_metadata(Some(chat_id), Some(thread_id));
        SandboxContainerRecord {
            container_id: format!("ctr-{}", scope.container_name()),
            container_name: scope.container_name(),
            image: Some("agent-sandbox:latest".to_string()),
            created_at: Some(100),
            state: Some("running".to_string()),
            status: Some("Up 1 hour".to_string()),
            running: true,
            user_id: Some(user_id),
            scope: Some(topic_id.to_string()),
            chat_id: Some(chat_id),
            thread_id: Some(thread_id),
            labels: scope.docker_labels(),
        }
    }

    fn ensured(&self) -> Vec<String> {
        self.ensured.lock().expect("mutex poisoned").clone()
    }

    fn recreated(&self) -> Vec<String> {
        self.recreated.lock().expect("mutex poisoned").clone()
    }

    fn deleted(&self) -> Vec<String> {
        self.deleted.lock().expect("mutex poisoned").clone()
    }
}

#[async_trait]
impl ManagerTopicSandboxControl for FakeTopicSandboxControl {
    async fn list_topic_sandboxes(&self, user_id: i64) -> Result<Vec<SandboxContainerRecord>> {
        Ok(self
            .records
            .lock()
            .expect("mutex poisoned")
            .values()
            .filter(|record| record.user_id == Some(user_id))
            .cloned()
            .collect())
    }

    async fn get_topic_sandbox(
        &self,
        user_id: i64,
        container_name: &str,
    ) -> Result<Option<SandboxContainerRecord>> {
        Ok(self
            .records
            .lock()
            .expect("mutex poisoned")
            .get(container_name)
            .filter(|record| record.user_id == Some(user_id))
            .cloned())
    }

    async fn ensure_topic_sandbox(&self, scope: SandboxScope) -> Result<SandboxContainerRecord> {
        self.ensured
            .lock()
            .expect("mutex poisoned")
            .push(scope.namespace().to_string());
        let record = Self::sandbox_record(scope.owner_id(), scope.namespace());
        self.records
            .lock()
            .expect("mutex poisoned")
            .insert(record.container_name.clone(), record.clone());
        Ok(record)
    }

    async fn recreate_topic_sandbox(&self, scope: SandboxScope) -> Result<SandboxContainerRecord> {
        self.recreated
            .lock()
            .expect("mutex poisoned")
            .push(scope.namespace().to_string());
        let record = Self::sandbox_record(scope.owner_id(), scope.namespace());
        self.records
            .lock()
            .expect("mutex poisoned")
            .insert(record.container_name.clone(), record.clone());
        Ok(record)
    }

    async fn delete_topic_sandbox_by_scope(&self, scope: SandboxScope) -> Result<bool> {
        let container_name = scope.container_name();
        self.deleted
            .lock()
            .expect("mutex poisoned")
            .push(container_name.clone());
        Ok(self
            .records
            .lock()
            .expect("mutex poisoned")
            .remove(&container_name)
            .is_some())
    }

    async fn delete_topic_sandbox_by_name(
        &self,
        _user_id: i64,
        container_name: &str,
    ) -> Result<bool> {
        self.deleted
            .lock()
            .expect("mutex poisoned")
            .push(container_name.to_string());
        Ok(self
            .records
            .lock()
            .expect("mutex poisoned")
            .remove(container_name)
            .is_some())
    }
}

#[tokio::test]
async fn forum_topic_tools_unavailable_without_lifecycle_service() {
    let provider =
        ManagerControlPlaneProvider::new(Arc::new(crate::storage::MockStorageProvider::new()), 77);
    let tool_names: Vec<String> = provider.tools().into_iter().map(|tool| tool.name).collect();

    assert!(!tool_names
        .iter()
        .any(|name| name == TOOL_FORUM_TOPIC_CREATE));
    assert!(!tool_names.iter().any(|name| name == TOOL_FORUM_TOPIC_LIST));
    assert!(!provider.can_handle(TOOL_FORUM_TOPIC_CREATE));
    assert!(!provider.can_handle(TOOL_FORUM_TOPIC_LIST));
}

#[tokio::test]
async fn forum_topic_dry_run_mutations_do_not_call_lifecycle_service() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_append_audit_event()
        .times(3)
        .returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 1,
                event_id: "evt-1".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 100,
            })
        });

    let lifecycle = Arc::new(FakeTopicLifecycle::new());
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_lifecycle(lifecycle.clone());

    provider
        .execute(
            TOOL_FORUM_TOPIC_CREATE,
            r#"{"name":"topic-a","dry_run":true}"#,
            None,
            None,
        )
        .await
        .expect("create dry-run should succeed");
    provider
        .execute(
            TOOL_FORUM_TOPIC_EDIT,
            r#"{"thread_id":42,"name":"topic-b","dry_run":true}"#,
            None,
            None,
        )
        .await
        .expect("edit dry-run should succeed");
    provider
        .execute(
            TOOL_FORUM_TOPIC_DELETE,
            r#"{"thread_id":42,"dry_run":true}"#,
            None,
            None,
        )
        .await
        .expect("delete dry-run should succeed");

    assert!(lifecycle.calls().is_empty());
}

#[tokio::test]
async fn forum_topic_create_invokes_lifecycle_and_audits_success() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config()
        .times(1)
        .returning(|_| Ok(crate::storage::UserConfig::default()));
    mock.expect_update_user_config()
        .withf(|user_id, config| {
            *user_id == 77
                && config.contexts.get("-100999:313").is_some_and(|context| {
                    context.chat_id == Some(-100999)
                        && context.thread_id == Some(313)
                        && context.forum_topic_name.as_deref() == Some("topic-a")
                        && context.forum_topic_icon_color == Some(9_367_192)
                        && !context.forum_topic_closed
                })
        })
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.topic_id.as_deref() == Some("-100999:313")
                && options.action == TOOL_FORUM_TOPIC_CREATE
                && options.payload.get("outcome") == Some(&json!("applied"))
                && options
                    .payload
                    .get("result")
                    .and_then(|result| result.get("thread_id"))
                    == Some(&json!(313))
        })
        .returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 1,
                event_id: "evt-topic-create".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 100,
            })
        });

    let lifecycle = Arc::new(FakeTopicLifecycle::new());
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_lifecycle(lifecycle.clone());

    let response = provider
        .execute(
            TOOL_FORUM_TOPIC_CREATE,
            r#"{"chat_id":-100999,"name":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("forum topic create should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["topic"]["thread_id"], 313);
    assert_eq!(parsed["audit_status"], "written");
    assert_eq!(lifecycle.calls().len(), 1);
}

#[tokio::test]
async fn forum_topic_list_returns_persisted_topics_for_current_chat() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().times(1).returning(|_| {
        Ok(crate::storage::UserConfig {
            contexts: std::collections::HashMap::from([
                (
                    "-100777:12".to_string(),
                    crate::storage::UserContextConfig {
                        state: None,
                        current_chat_uuid: None,
                        current_agent_flow_id: None,
                        chat_id: Some(-100777),
                        thread_id: Some(12),
                        forum_topic_name: Some("Alfa".to_string()),
                        forum_topic_icon_color: Some(16_766_590),
                        forum_topic_icon_custom_emoji_id: Some("emoji-1".to_string()),
                        forum_topic_closed: false,
                    },
                ),
                (
                    "-100777:20".to_string(),
                    crate::storage::UserContextConfig {
                        state: None,
                        current_chat_uuid: None,
                        current_agent_flow_id: None,
                        chat_id: Some(-100777),
                        thread_id: Some(20),
                        forum_topic_name: Some("Beta".to_string()),
                        forum_topic_icon_color: Some(7_322_096),
                        forum_topic_icon_custom_emoji_id: None,
                        forum_topic_closed: true,
                    },
                ),
                (
                    "-100888:7".to_string(),
                    crate::storage::UserContextConfig {
                        state: None,
                        current_chat_uuid: None,
                        current_agent_flow_id: None,
                        chat_id: Some(-100888),
                        thread_id: Some(7),
                        forum_topic_name: Some("Gamma".to_string()),
                        forum_topic_icon_color: None,
                        forum_topic_icon_custom_emoji_id: None,
                        forum_topic_closed: false,
                    },
                ),
            ]),
            ..crate::storage::UserConfig::default()
        })
    });

    let lifecycle = Arc::new(FakeTopicLifecycle::new());
    let provider =
        ManagerControlPlaneProvider::new(Arc::new(mock), 77).with_topic_lifecycle(lifecycle);

    let response = provider
        .execute(TOOL_FORUM_TOPIC_LIST, r#"{}"#, None, None)
        .await
        .expect("forum topic list should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["chat_id"], -100777);
    assert_eq!(parsed["count"], 1);
    assert_eq!(parsed["topics"][0]["topic_id"], "-100777:12");
    assert_eq!(parsed["topics"][0]["name"], "Alfa");
    assert_eq!(parsed["topics"][0]["closed"], false);
}

#[tokio::test]
async fn forum_topic_delete_cleans_topic_storage_and_sandbox() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_clear_agent_memory_for_context()
        .with(eq(77_i64), eq("-100999:42".to_string()))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_clear_chat_history_for_context()
        .with(eq(77_i64), eq("-100999:42".to_string()))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_delete_topic_context()
        .with(eq(77_i64), eq("-100999:42".to_string()))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_delete_topic_agents_md()
        .with(eq(77_i64), eq("-100999:42".to_string()))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_delete_topic_infra_config()
        .with(eq(77_i64), eq("-100999:42".to_string()))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_delete_topic_binding()
        .with(eq(77_i64), eq("-100999:42".to_string()))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_delete_topic_binding()
        .with(eq(77_i64), eq("42".to_string()))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_get_user_config()
        .times(1)
        .returning(|_| Ok(topic_delete_user_config()));
    mock.expect_update_user_config()
        .withf(|user_id, config| *user_id == 77 && !config.contexts.contains_key("-100999:42"))
        .times(1)
        .returning(|_, _| Ok(()));
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.topic_id.as_deref() == Some("-100999:42")
                && options.action == TOOL_FORUM_TOPIC_DELETE
                && options
                    .payload
                    .get("cleanup")
                    .and_then(|cleanup| cleanup.get("deleted_container"))
                    == Some(&json!(true))
        })
        .times(1)
        .returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                None,
                &options.action,
                options.payload,
            ))
        });

    let lifecycle = Arc::new(FakeTopicLifecycle::new());
    let sandbox_cleanup = Arc::new(FakeTopicSandboxCleanup::new());
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_lifecycle(lifecycle.clone())
        .with_topic_sandbox_cleanup(sandbox_cleanup.clone());

    let response = provider
        .execute(
            TOOL_FORUM_TOPIC_DELETE,
            r#"{"chat_id":-100999,"thread_id":42}"#,
            None,
            None,
        )
        .await
        .expect("forum topic delete should clean topic artifacts");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    assert_eq!(parsed["ok"], true);
    let cleanup = &parsed["cleanup"];
    assert_eq!(parsed["topic"]["thread_id"], 42);
    assert_eq!(cleanup["context_key"], "-100999:42");
    assert_eq!(cleanup["deleted_topic_agents_md"], true);
    assert_eq!(cleanup["deleted_container"], true);
    assert_eq!(parsed["audit_status"], "written");
    assert_eq!(
        lifecycle.calls(),
        vec![LifecycleCall::Delete(ForumTopicThreadRequest {
            chat_id: Some(-100999),
            thread_id: 42
        })]
    );
    assert_eq!(sandbox_cleanup.calls(), vec![(77, -100999, 42)]);
}

#[tokio::test]
async fn topic_binding_set_rejects_empty_topic_id() {
    let storage = Arc::new(crate::storage::MockStorageProvider::new());
    let provider = ManagerControlPlaneProvider::new(storage, 77);
    let err = provider
        .execute(
            TOOL_TOPIC_BINDING_SET,
            r#"{"topic_id":"   ","agent_id":"agent-1"}"#,
            None,
            None,
        )
        .await
        .expect_err("expected validation error");

    assert!(err.to_string().contains("topic_id must not be empty"));
}

#[tokio::test]
async fn topic_binding_get_rejects_unknown_fields() {
    let storage = Arc::new(crate::storage::MockStorageProvider::new());
    let provider = ManagerControlPlaneProvider::new(storage, 77);
    let err = provider
        .execute(
            TOOL_TOPIC_BINDING_GET,
            r#"{"topic_id":"topic-a","extra":true}"#,
            None,
            None,
        )
        .await
        .expect_err("expected strict serde validation error");

    assert!(err.to_string().contains("unknown field"));
}

#[tokio::test]
async fn agent_profile_upsert_rejects_non_object_profile() {
    let storage = Arc::new(crate::storage::MockStorageProvider::new());
    let provider = ManagerControlPlaneProvider::new(storage, 77);
    let err = provider
        .execute(
            TOOL_AGENT_PROFILE_UPSERT,
            r#"{"agent_id":"agent-a","profile":[1,2,3]}"#,
            None,
            None,
        )
        .await
        .expect_err("expected profile validation error");

    assert!(err.to_string().contains("profile must be a JSON object"));
}

#[tokio::test]
async fn agent_profile_upsert_rejects_legacy_tools_shorthand() {
    let storage = Arc::new(crate::storage::MockStorageProvider::new());
    let provider = ManagerControlPlaneProvider::new(storage, 77);
    let err = provider
        .execute(
            TOOL_AGENT_PROFILE_UPSERT,
            r#"{"agent_id":"agent-a","profile":{"tools":["ssh"]}}"#,
            None,
            None,
        )
        .await
        .expect_err("expected unsupported profile.tools validation error");

    assert!(err.to_string().contains("allowedTools/blockedTools"));
}

#[tokio::test]
async fn topic_infra_upsert_resolves_unique_forum_topic_name_alias() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().returning(|_| {
        Ok(crate::storage::UserConfig {
            contexts: std::collections::HashMap::from([(
                "-100777:240".to_string(),
                crate::storage::UserContextConfig {
                    chat_id: Some(-100777),
                    thread_id: Some(240),
                    forum_topic_name: Some("n-ru1".to_string()),
                    forum_topic_icon_color: Some(9_367_192),
                    ..crate::storage::UserContextConfig::default()
                },
            )]),
            ..crate::storage::UserConfig::default()
        })
    });
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_upsert_topic_infra_config()
        .withf(|options| options.topic_id == "-100777:240")
        .returning(|options| {
            Ok(TopicInfraConfigRecord {
                schema_version: 1,
                version: 1,
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
                created_at: 10,
                updated_at: 10,
            })
        });
    mock.expect_append_audit_event().returning(|options| {
        Ok(audit_event(
            1,
            options.topic_id.as_deref(),
            options.agent_id.as_deref(),
            &options.action,
            options.payload,
        ))
    });

    let lifecycle = Arc::new(FakeTopicLifecycle::new());
    let provider =
        ManagerControlPlaneProvider::new(Arc::new(mock), 77).with_topic_lifecycle(lifecycle);
    let response = provider
        .execute(
            TOOL_TOPIC_INFRA_UPSERT,
            r#"{"topic_id":"n-ru1","target_name":"n-ru1","host":"213.171.27.211","port":31924,"remote_user":"user1","auth_mode":"none","allowed_tool_modes":["exec"]}"#,
            None,
            None,
        )
        .await
        .expect("alias resolution should canonicalize forum topic name");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    assert_eq!(parsed["topic_infra"]["topic_id"], "-100777:240");
}

#[tokio::test]
async fn topic_agent_tools_get_hides_blocked_tools_from_effective_snapshot() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(AgentProfileRecord {
                schema_version: 1,
                version: 1,
                user_id: 77,
                agent_id: "agent-a".to_string(),
                profile: json!({
                    "blockedTools": TOPIC_AGENT_YTDLP_TOOLS,
                }),
                created_at: 10,
                updated_at: 10,
            }))
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools get should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    let active_tools = parsed["tools"]["active_tools"]
        .as_array()
        .expect("active_tools must be an array");
    assert!(!active_tools
        .iter()
        .any(|tool| { TOPIC_AGENT_YTDLP_TOOLS.contains(&tool.as_str().unwrap_or_default()) }));

    let ytdlp_status = parsed["tools"]["provider_statuses"]
        .as_array()
        .expect("provider_statuses must be an array")
        .iter()
        .find(|entry| entry["provider"] == "ytdlp")
        .expect("ytdlp provider status must be present");
    assert_eq!(ytdlp_status["enabled"], false);

    let reminder_status = parsed["tools"]["provider_statuses"]
        .as_array()
        .expect("provider_statuses must be an array")
        .iter()
        .find(|entry| entry["provider"] == "reminder")
        .expect("reminder provider status must be present");
    assert_eq!(reminder_status["enabled"], true);
    assert!(reminder_status["available_tools"]
        .as_array()
        .is_some_and(|tools| tools.iter().any(|tool| tool == "reminder_schedule")));
}

#[tokio::test]
async fn topic_agent_tools_get_keeps_reminders_enabled_for_allowlisted_profiles() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(AgentProfileRecord {
                schema_version: 1,
                version: 1,
                user_id: 77,
                agent_id: "agent-a".to_string(),
                profile: json!({
                    "allowedTools": ["execute_command"],
                }),
                created_at: 10,
                updated_at: 10,
            }))
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools get should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    let active_tools = parsed["tools"]["active_tools"]
        .as_array()
        .expect("active_tools must be an array");
    assert!(active_tools
        .iter()
        .any(|tool| tool.as_str() == Some("reminder_schedule")));
    let reminder_status = parsed["tools"]["provider_statuses"]
        .as_array()
        .expect("provider_statuses must be an array")
        .iter()
        .find(|entry| entry["provider"] == "reminder")
        .expect("reminder provider status must be present");
    assert_eq!(reminder_status["enabled"], true);
    assert!(reminder_status["active_tools"]
        .as_array()
        .is_some_and(|tools| tools.iter().any(|tool| tool == "reminder_schedule")));
}

#[tokio::test]
async fn topic_agent_tools_disable_expands_provider_alias_and_persists_profile() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().returning(|_| {
        Ok(crate::storage::UserConfig {
            contexts: std::collections::HashMap::from([(
                "-100777:240".to_string(),
                crate::storage::UserContextConfig {
                    chat_id: Some(-100777),
                    thread_id: Some(240),
                    forum_topic_name: Some("n-ru1".to_string()),
                    forum_topic_icon_color: Some(9_367_192),
                    ..crate::storage::UserContextConfig::default()
                },
            )]),
            ..crate::storage::UserConfig::default()
        })
    });
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "-100777:240", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(AgentProfileRecord {
                schema_version: 1,
                version: 3,
                user_id: 77,
                agent_id: "agent-a".to_string(),
                profile: json!({
                    "systemPrompt": "infra agent",
                }),
                created_at: 10,
                updated_at: 20,
            }))
        });
    mock.expect_upsert_agent_profile()
        .withf(|options| {
            options.agent_id == "agent-a"
                && options.profile["systemPrompt"] == "infra agent"
                && options
                    .profile
                    .get("blockedTools")
                    .and_then(|value| value.as_array())
                    .is_some_and(|tools| {
                        TOPIC_AGENT_YTDLP_TOOLS
                            .iter()
                            .all(|tool| tools.iter().any(|value| value.as_str() == Some(*tool)))
                    })
        })
        .returning(|options| {
            Ok(AgentProfileRecord {
                schema_version: 1,
                version: 4,
                user_id: options.user_id,
                agent_id: options.agent_id,
                profile: options.profile,
                created_at: 10,
                updated_at: 30,
            })
        });
    mock.expect_append_audit_event().returning(|options| {
        Ok(audit_event(
            1,
            options.topic_id.as_deref(),
            options.agent_id.as_deref(),
            &options.action,
            options.payload,
        ))
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_lifecycle(Arc::new(FakeTopicLifecycle::new()));
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_DISABLE,
            r#"{"topic_id":"n-ru1","tools":["ytdlp"]}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools disable should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    let blocked_tools = parsed["profile"]["profile"]["blockedTools"]
        .as_array()
        .expect("blockedTools must be present");
    assert_eq!(blocked_tools.len(), TOPIC_AGENT_YTDLP_TOOLS.len());
    let ytdlp_status = parsed["tools"]["provider_statuses"]
        .as_array()
        .expect("provider_statuses must be an array")
        .iter()
        .find(|entry| entry["provider"] == "ytdlp")
        .expect("ytdlp provider status must be present");
    assert_eq!(ytdlp_status["enabled"], false);
}

#[tokio::test]
async fn topic_agent_tools_enable_accepts_reminder_provider_alias() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config()
        .returning(|_| Ok(crate::storage::UserConfig::default()));
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(AgentProfileRecord {
                schema_version: 1,
                version: 1,
                user_id: 77,
                agent_id: "agent-a".to_string(),
                profile: json!({
                    "blockedTools": TOPIC_AGENT_REMINDER_TOOLS,
                }),
                created_at: 10,
                updated_at: 10,
            }))
        });
    mock.expect_upsert_agent_profile()
        .withf(|options| {
            options.agent_id == "agent-a" && options.profile.get("blockedTools").is_none()
        })
        .returning(|options| {
            Ok(AgentProfileRecord {
                schema_version: 1,
                version: 2,
                user_id: options.user_id,
                agent_id: options.agent_id,
                profile: options.profile,
                created_at: 10,
                updated_at: 20,
            })
        });
    mock.expect_append_audit_event().returning(|options| {
        Ok(audit_event(
            1,
            options.topic_id.as_deref(),
            options.agent_id.as_deref(),
            &options.action,
            options.payload,
        ))
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_ENABLE,
            r#"{"topic_id":"topic-a","tools":["reminder"]}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools enable should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    let reminder_status = parsed["tools"]["provider_statuses"]
        .as_array()
        .expect("provider_statuses must be an array")
        .iter()
        .find(|entry| entry["provider"] == "reminder")
        .expect("reminder provider status must be present");
    assert_eq!(reminder_status["enabled"], true);
}

#[cfg(feature = "browser_use")]
#[tokio::test]
async fn topic_agent_tools_get_reports_browser_use_provider_status_when_enabled() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("BROWSER_USE_URL", "http://browser-use:8000");
    std::env::set_var("BROWSER_USE_ENABLED", "true");

    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| Ok(None));

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools get should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    let browser_status = parsed["tools"]["provider_statuses"]
        .as_array()
        .expect("provider_statuses must be an array")
        .iter()
        .find(|entry| entry["provider"] == "browser_use")
        .expect("browser_use provider status must be present");

    assert_eq!(browser_status["enabled"], true);
    assert!(browser_status["available_tools"]
        .as_array()
        .is_some_and(|tools| {
            tools.iter().any(|tool| tool == "browser_use_run_task")
                && tools.iter().any(|tool| tool == "browser_use_get_session")
                && tools.iter().any(|tool| tool == "browser_use_close_session")
        }));

    std::env::remove_var("BROWSER_USE_ENABLED");
    std::env::remove_var("BROWSER_USE_URL");
}

#[cfg(feature = "browser_use")]
#[tokio::test]
async fn topic_agent_tools_disable_accepts_browser_provider_alias() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("BROWSER_USE_URL", "http://browser-use:8000");
    std::env::set_var("BROWSER_USE_ENABLED", "true");

    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config()
        .returning(|_| Ok(crate::storage::UserConfig::default()));
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(AgentProfileRecord {
                schema_version: 1,
                version: 3,
                user_id: 77,
                agent_id: "agent-a".to_string(),
                profile: json!({
                    "systemPrompt": "browser agent",
                }),
                created_at: 10,
                updated_at: 20,
            }))
        });
    mock.expect_upsert_agent_profile()
        .withf(|options| {
            options.agent_id == "agent-a"
                && options.profile["systemPrompt"] == "browser agent"
                && options
                    .profile
                    .get("blockedTools")
                    .and_then(|value| value.as_array())
                    .is_some_and(|tools| {
                        [
                            "browser_use_run_task",
                            "browser_use_get_session",
                            "browser_use_close_session",
                        ]
                        .iter()
                        .all(|tool| tools.iter().any(|value| value.as_str() == Some(*tool)))
                    })
        })
        .returning(|options| {
            Ok(AgentProfileRecord {
                schema_version: 1,
                version: 4,
                user_id: options.user_id,
                agent_id: options.agent_id,
                profile: options.profile,
                created_at: 10,
                updated_at: 30,
            })
        });
    mock.expect_append_audit_event().returning(|options| {
        Ok(audit_event(
            1,
            options.topic_id.as_deref(),
            options.agent_id.as_deref(),
            &options.action,
            options.payload,
        ))
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_DISABLE,
            r#"{"topic_id":"topic-a","tools":["browser"]}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools disable should accept browser alias");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    let blocked_tools = parsed["profile"]["profile"]["blockedTools"]
        .as_array()
        .expect("blockedTools must be present");
    assert_eq!(blocked_tools.len(), 3);

    let browser_status = parsed["tools"]["provider_statuses"]
        .as_array()
        .expect("provider_statuses must be an array")
        .iter()
        .find(|entry| entry["provider"] == "browser_use")
        .expect("browser_use provider status must be present");
    assert_eq!(browser_status["enabled"], false);

    std::env::remove_var("BROWSER_USE_ENABLED");
    std::env::remove_var("BROWSER_USE_URL");
}

#[tokio::test]
async fn topic_agent_tools_enable_accepts_ssh_send_file_to_user_when_topic_has_infra() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config()
        .returning(|_| Ok(crate::storage::UserConfig::default()));
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(topic_infra(77, "topic-a", 1))));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(AgentProfileRecord {
                schema_version: 1,
                version: 1,
                user_id: 77,
                agent_id: "agent-a".to_string(),
                profile: json!({
                    "blockedTools": ["ssh_send_file_to_user"],
                }),
                created_at: 10,
                updated_at: 10,
            }))
        });
    mock.expect_upsert_agent_profile()
        .withf(|options| {
            options.agent_id == "agent-a" && options.profile.get("blockedTools").is_none()
        })
        .returning(|options| {
            Ok(AgentProfileRecord {
                schema_version: 1,
                version: 2,
                user_id: options.user_id,
                agent_id: options.agent_id,
                profile: options.profile,
                created_at: 10,
                updated_at: 20,
            })
        });
    mock.expect_append_audit_event().returning(|options| {
        Ok(audit_event(
            1,
            options.topic_id.as_deref(),
            options.agent_id.as_deref(),
            &options.action,
            options.payload,
        ))
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_ENABLE,
            r#"{"topic_id":"topic-a","tools":["ssh_send_file_to_user"]}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools enable should accept ssh_send_file_to_user");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    let ssh_status = parsed["tools"]["provider_statuses"]
        .as_array()
        .expect("provider_statuses must be an array")
        .iter()
        .find(|entry| entry["provider"] == "ssh")
        .expect("ssh provider status must be present");
    assert!(ssh_status["available_tools"]
        .as_array()
        .expect("available_tools must be present")
        .iter()
        .any(|value| value.as_str() == Some("ssh_send_file_to_user")));
    assert_eq!(ssh_status["enabled"], true);
}

#[tokio::test]
async fn topic_agent_tools_disable_sandbox_triggers_container_cleanup() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "-100777:240", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(AgentProfileRecord {
                schema_version: 1,
                version: 3,
                user_id: 77,
                agent_id: "agent-a".to_string(),
                profile: json!({
                    "systemPrompt": "infra agent",
                }),
                created_at: 10,
                updated_at: 20,
            }))
        });
    mock.expect_upsert_agent_profile()
        .withf(|options| {
            options.agent_id == "agent-a"
                && TOPIC_AGENT_SANDBOX_TOOLS.iter().all(|tool| {
                    options.profile["blockedTools"]
                        .as_array()
                        .is_some_and(|tools| {
                            tools.iter().any(|value| value.as_str() == Some(*tool))
                        })
                })
        })
        .returning(|options| {
            Ok(AgentProfileRecord {
                schema_version: 1,
                version: 4,
                user_id: options.user_id,
                agent_id: options.agent_id,
                profile: options.profile,
                created_at: 10,
                updated_at: 30,
            })
        });
    mock.expect_append_audit_event()
        .withf(|options| {
            options.action == TOOL_TOPIC_AGENT_TOOLS_DISABLE
                && options
                    .payload
                    .get("sandbox_cleanup")
                    .and_then(|value| value.get("deleted_container"))
                    == Some(&json!(true))
        })
        .returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                options.agent_id.as_deref(),
                &options.action,
                options.payload,
            ))
        });

    let sandbox_cleanup = Arc::new(FakeTopicSandboxCleanup::new());
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_sandbox_cleanup(sandbox_cleanup.clone());
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_DISABLE,
            r#"{"topic_id":"-100777:240","tools":["sandbox"]}"#,
            None,
            None,
        )
        .await
        .expect("topic agent sandbox disable should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    assert_eq!(parsed["sandbox_cleanup"]["deleted_container"], true);
    assert_eq!(sandbox_cleanup.calls(), vec![(77, -100777, 240)],);
}

#[tokio::test]
async fn topic_sandbox_list_marks_disabled_container_as_orphaned() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().times(1).returning(|_| {
        Ok(crate::storage::UserConfig {
            contexts: std::collections::HashMap::from([(
                "-100777:240".to_string(),
                crate::storage::UserContextConfig {
                    chat_id: Some(-100777),
                    thread_id: Some(240),
                    ..crate::storage::UserContextConfig::default()
                },
            )]),
            ..crate::storage::UserConfig::default()
        })
    });
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .times(1)
        .returning(|_, _| Ok(Some(binding(77, "-100777:240", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .times(1)
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .times(1)
        .returning(|_, _| {
            Ok(Some(AgentProfileRecord {
                schema_version: 1,
                version: 3,
                user_id: 77,
                agent_id: "agent-a".to_string(),
                profile: json!({
                    "blockedTools": TOPIC_AGENT_SANDBOX_TOOLS,
                }),
                created_at: 10,
                updated_at: 20,
            }))
        });

    let sandbox_control = Arc::new(FakeTopicSandboxControl::new(vec![
        FakeTopicSandboxControl::sandbox_record(77, "-100777:240"),
    ]));
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_sandbox_control(sandbox_control);
    let response = provider
        .execute(
            TOOL_TOPIC_SANDBOX_LIST,
            r#"{"orphaned_only":true}"#,
            None,
            None,
        )
        .await
        .expect("topic sandbox list should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    assert_eq!(parsed["count"], 1);
    assert_eq!(parsed["sandboxes"][0]["orphan_reason"], "sandbox_disabled");
    assert_eq!(parsed["sandboxes"][0]["sandbox_tools_enabled"], false);
}

#[tokio::test]
async fn topic_sandbox_create_ensures_container_for_tracked_topic() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().times(2).returning(|_| {
        Ok(crate::storage::UserConfig {
            contexts: std::collections::HashMap::from([(
                "-100777:240".to_string(),
                crate::storage::UserContextConfig {
                    chat_id: Some(-100777),
                    thread_id: Some(240),
                    ..crate::storage::UserContextConfig::default()
                },
            )]),
            ..crate::storage::UserConfig::default()
        })
    });
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .times(1)
        .returning(|_, _| Ok(None));
    mock.expect_append_audit_event()
        .times(1)
        .returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                options.agent_id.as_deref(),
                &options.action,
                options.payload,
            ))
        });

    let sandbox_control = Arc::new(FakeTopicSandboxControl::new(Vec::new()));
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_sandbox_control(sandbox_control.clone());
    let response = provider
        .execute(
            TOOL_TOPIC_SANDBOX_CREATE,
            r#"{"topic_id":"-100777:240"}"#,
            None,
            None,
        )
        .await
        .expect("topic sandbox create should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    assert_eq!(parsed["sandbox"]["topic_id"], "-100777:240");
    assert_eq!(sandbox_control.ensured(), vec!["-100777:240".to_string()]);
}

#[tokio::test]
async fn topic_sandbox_delete_supports_container_name_lookup() {
    let sandbox_record = FakeTopicSandboxControl::sandbox_record(77, "-100777:240");
    let container_name = sandbox_record.container_name.clone();

    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().times(1).returning(|_| {
        Ok(crate::storage::UserConfig {
            contexts: std::collections::HashMap::from([(
                "-100777:240".to_string(),
                crate::storage::UserContextConfig {
                    chat_id: Some(-100777),
                    thread_id: Some(240),
                    ..crate::storage::UserContextConfig::default()
                },
            )]),
            ..crate::storage::UserConfig::default()
        })
    });
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .times(1)
        .returning(|_, _| Ok(None));
    mock.expect_append_audit_event()
        .times(1)
        .returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                options.agent_id.as_deref(),
                &options.action,
                options.payload,
            ))
        });

    let sandbox_control = Arc::new(FakeTopicSandboxControl::new(vec![sandbox_record]));
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_sandbox_control(sandbox_control.clone());
    let response = provider
        .execute(
            TOOL_TOPIC_SANDBOX_DELETE,
            &format!(r#"{{"container_name":"{container_name}"}}"#),
            None,
            None,
        )
        .await
        .expect("topic sandbox delete should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    assert_eq!(parsed["deleted"], true);
    assert_eq!(sandbox_control.deleted(), vec![container_name]);
}

#[tokio::test]
async fn topic_sandbox_recreate_calls_control_plane() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().times(1).returning(|_| {
        Ok(crate::storage::UserConfig {
            contexts: std::collections::HashMap::from([(
                "-100777:240".to_string(),
                crate::storage::UserContextConfig {
                    chat_id: Some(-100777),
                    thread_id: Some(240),
                    ..crate::storage::UserContextConfig::default()
                },
            )]),
            ..crate::storage::UserConfig::default()
        })
    });
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .times(1)
        .returning(|_, _| Ok(None));
    mock.expect_append_audit_event()
        .times(1)
        .returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                options.agent_id.as_deref(),
                &options.action,
                options.payload,
            ))
        });

    let sandbox_control = Arc::new(FakeTopicSandboxControl::new(Vec::new()));
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_sandbox_control(sandbox_control.clone());
    let response = provider
        .execute(
            TOOL_TOPIC_SANDBOX_RECREATE,
            r#"{"topic_id":"-100777:240"}"#,
            None,
            None,
        )
        .await
        .expect("topic sandbox recreate should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    assert_eq!(parsed["sandbox"]["topic_id"], "-100777:240");
    assert_eq!(sandbox_control.recreated(), vec!["-100777:240".to_string()]);
}

#[tokio::test]
async fn topic_sandbox_prune_dry_run_reports_binding_missing_candidates() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config().times(1).returning(|_| {
        Ok(crate::storage::UserConfig {
            contexts: std::collections::HashMap::from([(
                "-100777:240".to_string(),
                crate::storage::UserContextConfig {
                    chat_id: Some(-100777),
                    thread_id: Some(240),
                    ..crate::storage::UserContextConfig::default()
                },
            )]),
            ..crate::storage::UserConfig::default()
        })
    });
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("-100777:240".to_string()))
        .times(1)
        .returning(|_, _| Ok(None));
    mock.expect_append_audit_event()
        .times(1)
        .returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                options.agent_id.as_deref(),
                &options.action,
                options.payload,
            ))
        });

    let sandbox_control = Arc::new(FakeTopicSandboxControl::new(vec![
        FakeTopicSandboxControl::sandbox_record(77, "-100777:240"),
    ]));
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
        .with_topic_sandbox_control(sandbox_control.clone());
    let response = provider
        .execute(
            TOOL_TOPIC_SANDBOX_PRUNE,
            r#"{"reason":"binding_missing","dry_run":true}"#,
            None,
            None,
        )
        .await
        .expect("topic sandbox prune dry-run should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    assert_eq!(parsed["count"], 1);
    assert_eq!(parsed["candidates"][0]["orphan_reason"], "binding_missing");
    assert!(sandbox_control.deleted().is_empty());
}

#[tokio::test]
async fn topic_agent_tools_enable_is_noop_without_existing_profile() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_append_audit_event().returning(|options| {
        Ok(audit_event(
            1,
            options.topic_id.as_deref(),
            options.agent_id.as_deref(),
            &options.action,
            options.payload,
        ))
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_TOOLS_ENABLE,
            r#"{"topic_id":"topic-a","tools":["ytdlp_get_video_metadata"]}"#,
            None,
            None,
        )
        .await
        .expect("topic agent tools enable should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    assert_eq!(parsed["updated"], false);
    assert_eq!(parsed["tools"]["blocked_tools"], json!([]));
}

#[tokio::test]
async fn topic_agent_hooks_get_reports_manageable_and_protected_hooks() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(AgentProfileRecord {
                schema_version: 1,
                version: 1,
                user_id: 77,
                agent_id: "agent-a".to_string(),
                profile: json!({
                    "disabledHooks": ["search_budget"],
                }),
                created_at: 10,
                updated_at: 10,
            }))
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_HOOKS_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic agent hooks get should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    assert_eq!(
        parsed["hooks"]["active_hooks"].as_array().map(Vec::len),
        Some(5)
    );
    assert_eq!(parsed["hooks"]["disabled_hooks"], json!(["search_budget"]));
    assert!(parsed["hooks"]["hook_statuses"]
        .as_array()
        .expect("hook_statuses must be an array")
        .iter()
        .any(|entry| entry["hook"] == "completion_check" && entry["protected"] == true));
}

#[tokio::test]
async fn topic_agent_hooks_disable_rejects_protected_hook() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let err = provider
        .execute(
            TOOL_TOPIC_AGENT_HOOKS_DISABLE,
            r#"{"topic_id":"topic-a","hooks":["completion_check"]}"#,
            None,
            None,
        )
        .await
        .expect_err("protected hook must not be disableable");

    assert!(err.to_string().contains("system-protected"));
}

#[tokio::test]
async fn topic_agent_hooks_disable_persists_manageable_hook_change() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(Some(binding(77, "topic-a", "agent-a", 1))));
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| {
            Ok(Some(AgentProfileRecord {
                schema_version: 1,
                version: 1,
                user_id: 77,
                agent_id: "agent-a".to_string(),
                profile: json!({}),
                created_at: 10,
                updated_at: 10,
            }))
        });
    mock.expect_upsert_agent_profile()
        .withf(|options| {
            options.agent_id == "agent-a"
                && options.profile["disabledHooks"] == json!(["timeout_report"])
        })
        .returning(|options| {
            Ok(AgentProfileRecord {
                schema_version: 1,
                version: 2,
                user_id: options.user_id,
                agent_id: options.agent_id,
                profile: options.profile,
                created_at: 10,
                updated_at: 20,
            })
        });
    mock.expect_append_audit_event().returning(|options| {
        Ok(audit_event(
            1,
            options.topic_id.as_deref(),
            options.agent_id.as_deref(),
            &options.action,
            options.payload,
        ))
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENT_HOOKS_DISABLE,
            r#"{"topic_id":"topic-a","hooks":["timeout"]}"#,
            None,
            None,
        )
        .await
        .expect("manageable hook disable should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    assert_eq!(parsed["hooks"]["disabled_hooks"], json!(["timeout_report"]));
    assert_eq!(
        parsed["profile"]["profile"]["disabledHooks"],
        json!(["timeout_report"])
    );
}

#[tokio::test]
async fn forum_topic_provision_ssh_agent_creates_canonical_binding_and_infra() {
    let lifecycle = Arc::new(FakeTopicLifecycle::new());
    let provider =
        ManagerControlPlaneProvider::new(Arc::new(mock_storage_for_forum_topic_provision()), 77)
            .with_topic_lifecycle(lifecycle.clone());
    let response = provider
        .execute(
            TOOL_FORUM_TOPIC_PROVISION_SSH_AGENT,
            r#"{"name":"n-ru1","host":"213.171.27.211","port":31924,"remote_user":"user1","auth_mode":"none"}"#,
            None,
            None,
        )
        .await
        .expect("atomic ssh topic provisioning should succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("response must be valid json");
    assert_eq!(parsed["topic"]["topic_id"], "-100777:313");
    assert_eq!(parsed["binding"]["topic_id"], "-100777:313");
    assert_eq!(parsed["topic_infra"]["topic_id"], "-100777:313");
    assert_eq!(
        parsed["profile"]["profile"]["blockedTools"],
        json!(topic_agent_default_blocked_tools())
    );
    let allowed_tools = parsed["profile"]["profile"]["allowedTools"]
        .as_array()
        .expect("allowedTools must be present");
    assert!(allowed_tools
        .iter()
        .any(|value| value.as_str() == Some("ssh_send_file_to_user")));
    assert!(TOPIC_AGENT_REMINDER_TOOLS.iter().all(|tool| {
        allowed_tools
            .iter()
            .any(|value| value.as_str() == Some(*tool))
    }));
    assert_eq!(
        parsed["topic_infra"]["allowed_tool_modes"],
        json!([
            "exec",
            "sudo_exec",
            "read_file",
            "apply_file_edit",
            "check_process",
            "transfer"
        ])
    );
    assert_eq!(parsed["preflight"]["provider_enabled"], true);

    let calls = lifecycle.calls();
    assert!(matches!(calls.first(), Some(LifecycleCall::Create(_))));
}

#[tokio::test]
async fn topic_binding_set_persists_and_audits() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));

    mock.expect_upsert_topic_binding()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.agent_id == "agent-a"
                && options.binding_kind.is_none()
                && options.chat_id == OptionalMetadataPatch::Keep
                && options.thread_id == OptionalMetadataPatch::Keep
                && options.expires_at == OptionalMetadataPatch::Keep
                && options.last_activity_at.is_none()
        })
        .returning(|options| {
            Ok(TopicBindingRecord {
                schema_version: 1,
                version: 2,
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                chat_id: options.chat_id.for_new_record(),
                thread_id: options.thread_id.for_new_record(),
                expires_at: options.expires_at.for_new_record(),
                last_activity_at: options.last_activity_at,
                created_at: 100,
                updated_at: 200,
            })
        });

    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.action == TOOL_TOPIC_BINDING_SET
                && options.topic_id.as_deref() == Some("topic-a")
                && options.agent_id.as_deref() == Some("agent-a")
        })
        .returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 1,
                event_id: "evt-1".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 300,
            })
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_SET,
            r#"{"topic_id":"topic-a","agent_id":"agent-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic binding set should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed.get("ok"), Some(&serde_json::Value::Bool(true)));
    assert_eq!(parsed["binding"]["topic_id"], "topic-a");
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_binding_set_supports_explicit_null_to_clear_metadata() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| {
            Ok(Some(TopicBindingRecord {
                schema_version: 1,
                version: 1,
                user_id: 77,
                topic_id: "topic-a".to_string(),
                agent_id: "agent-a".to_string(),
                binding_kind: TopicBindingKind::Runtime,
                chat_id: Some(100),
                thread_id: Some(7),
                expires_at: Some(10_000),
                last_activity_at: Some(20),
                created_at: 10,
                updated_at: 20,
            }))
        });

    mock.expect_upsert_topic_binding()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.agent_id == "agent-a"
                && options.chat_id == OptionalMetadataPatch::Clear
                && options.thread_id == OptionalMetadataPatch::Clear
                && options.expires_at == OptionalMetadataPatch::Clear
        })
        .returning(|options| {
            Ok(TopicBindingRecord {
                schema_version: 1,
                version: 2,
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                chat_id: options.chat_id.for_new_record(),
                thread_id: options.thread_id.for_new_record(),
                expires_at: options.expires_at.for_new_record(),
                last_activity_at: options.last_activity_at,
                created_at: 10,
                updated_at: 30,
            })
        });

    mock.expect_append_audit_event().returning(|options| {
        Ok(crate::storage::AuditEventRecord {
            schema_version: 1,
            version: 1,
            event_id: "evt-1".to_string(),
            user_id: options.user_id,
            topic_id: options.topic_id,
            agent_id: options.agent_id,
            action: options.action,
            payload: options.payload,
            created_at: 300,
        })
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_SET,
            r#"{"topic_id":"topic-a","agent_id":"agent-a","chat_id":null,"thread_id":null,"expires_at":null}"#,
            None,
            None,
        )
        .await
        .expect("topic binding set should support null clears");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed.get("ok"), Some(&serde_json::Value::Bool(true)));
    assert_eq!(parsed["binding"]["chat_id"], serde_json::Value::Null);
    assert_eq!(parsed["binding"]["thread_id"], serde_json::Value::Null);
    assert_eq!(parsed["binding"]["expires_at"], serde_json::Value::Null);
}

#[tokio::test]
async fn topic_binding_set_succeeds_when_audit_write_fails() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));

    mock.expect_upsert_topic_binding()
        .withf(|options| {
            options.user_id == 77 && options.topic_id == "topic-a" && options.agent_id == "agent-a"
        })
        .returning(|options| {
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
                created_at: 100,
                updated_at: 100,
            })
        });

    mock.expect_append_audit_event().returning(|_| {
        Err(crate::storage::StorageError::Config(
            "audit unavailable".to_string(),
        ))
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_SET,
            r#"{"topic_id":"topic-a","agent_id":"agent-a"}"#,
            None,
            None,
        )
        .await
        .expect("mutation should succeed even when audit write fails");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["binding"]["topic_id"], "topic-a");
    assert_eq!(parsed["audit_status"], "write_failed");
    assert!(parsed["audit_error"].as_str().is_some());
}

#[tokio::test]
async fn topic_binding_set_dry_run_does_not_persist() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.action == TOOL_TOPIC_BINDING_SET
                && options.payload.get("outcome") == Some(&json!("dry_run"))
        })
        .returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 1,
                event_id: "evt-dry-run".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 300,
            })
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_SET,
            r#"{"topic_id":"topic-a","agent_id":"agent-a","dry_run":true}"#,
            None,
            None,
        )
        .await
        .expect("dry-run set should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["dry_run"], true);
    assert_eq!(parsed["preview"]["operation"], "upsert");
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_binding_set_dry_run_reports_audit_write_failure() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_upsert_topic_binding().times(0);
    mock.expect_append_audit_event().returning(|_| {
        Err(crate::storage::StorageError::Config(
            "audit unavailable".to_string(),
        ))
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_SET,
            r#"{"topic_id":"topic-a","agent_id":"agent-a","dry_run":true}"#,
            None,
            None,
        )
        .await
        .expect("dry-run should succeed even when audit write fails");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["dry_run"], true);
    assert_eq!(parsed["audit_status"], "write_failed");
}

#[tokio::test]
async fn topic_binding_rollback_restores_previous_snapshot() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| Ok(Some(binding(77, &topic_id, "agent-new", 4))));
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .returning(|_, _, _| {
            Ok(vec![crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 9,
                event_id: "evt-9".to_string(),
                user_id: 77,
                topic_id: Some("topic-a".to_string()),
                agent_id: Some("agent-new".to_string()),
                action: TOOL_TOPIC_BINDING_SET.to_string(),
                payload: json!({
                    "topic_id": "topic-a",
                    "agent_id": "agent-new",
                    "previous": {
                        "schema_version": 1,
                        "version": 3,
                        "user_id": 77,
                        "topic_id": "topic-a",
                        "agent_id": "agent-old",
                        "created_at": 1,
                        "updated_at": 2
                    },
                    "outcome": "applied"
                }),
                created_at: 100,
            }])
        });
    mock.expect_upsert_topic_binding()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.agent_id == "agent-old"
        })
        .returning(|options| {
            Ok(binding(
                options.user_id,
                &options.topic_id,
                &options.agent_id,
                5,
            ))
        });
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.action == TOOL_TOPIC_BINDING_ROLLBACK
                && options.payload.get("operation") == Some(&json!("restore"))
        })
        .returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 10,
                event_id: "evt-10".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 110,
            })
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_ROLLBACK,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic rollback should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["operation"], "restore");
    assert_eq!(parsed["binding"]["agent_id"], "agent-old");
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_binding_rollback_succeeds_when_audit_write_fails() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| {
            Ok(Some(TopicBindingRecord {
                schema_version: 1,
                version: 4,
                user_id: 77,
                topic_id,
                agent_id: "agent-new".to_string(),
                binding_kind: TopicBindingKind::Manual,
                chat_id: None,
                thread_id: None,
                expires_at: None,
                last_activity_at: Some(20),
                created_at: 10,
                updated_at: 20,
            }))
        });
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .returning(|_, _, _| {
            Ok(vec![crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 9,
                event_id: "evt-9".to_string(),
                user_id: 77,
                topic_id: Some("topic-a".to_string()),
                agent_id: Some("agent-new".to_string()),
                action: TOOL_TOPIC_BINDING_SET.to_string(),
                payload: json!({
                    "topic_id": "topic-a",
                    "previous": {
                        "schema_version": 1,
                        "version": 3,
                        "user_id": 77,
                        "topic_id": "topic-a",
                        "agent_id": "agent-old",
                        "created_at": 1,
                        "updated_at": 2
                    },
                    "outcome": "applied"
                }),
                created_at: 100,
            }])
        });
    mock.expect_upsert_topic_binding().returning(|options| {
        Ok(TopicBindingRecord {
            schema_version: 1,
            version: 5,
            user_id: options.user_id,
            topic_id: options.topic_id,
            agent_id: options.agent_id,
            binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
            chat_id: options.chat_id.for_new_record(),
            thread_id: options.thread_id.for_new_record(),
            expires_at: options.expires_at.for_new_record(),
            last_activity_at: options.last_activity_at,
            created_at: 10,
            updated_at: 30,
        })
    });
    mock.expect_append_audit_event().returning(|_| {
        Err(crate::storage::StorageError::Config(
            "audit unavailable".to_string(),
        ))
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_ROLLBACK,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("rollback should succeed even when audit write fails");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["operation"], "restore");
    assert_eq!(parsed["audit_status"], "write_failed");
}

#[tokio::test]
async fn topic_binding_rollback_scans_multiple_audit_pages() {
    let mut mock = crate::storage::MockStorageProvider::new();
    let mut sequence = Sequence::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| Ok(Some(binding(77, &topic_id, "agent-new", 8))));
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _, _| {
            Ok(vec![audit_event(
                500,
                Some("other-topic"),
                Some("agent-z"),
                TOOL_TOPIC_BINDING_SET,
                json!({"outcome":"applied"}),
            )])
        });
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(Some(500_u64)), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _, _| {
            Ok(vec![audit_event(
                499,
                Some("topic-a"),
                Some("agent-new"),
                TOOL_TOPIC_BINDING_SET,
                json!({
                    "topic_id": "topic-a",
                    "previous": {
                        "schema_version": 1,
                        "version": 7,
                        "user_id": 77,
                        "topic_id": "topic-a",
                        "agent_id": "agent-old",
                        "created_at": 1,
                        "updated_at": 2
                    },
                    "outcome": "applied"
                }),
            )])
        });
    mock.expect_upsert_topic_binding()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.agent_id == "agent-old"
        })
        .returning(|options| {
            Ok(binding(
                options.user_id,
                &options.topic_id,
                &options.agent_id,
                9,
            ))
        });
    mock.expect_append_audit_event().returning(|options| {
        Ok(crate::storage::AuditEventRecord {
            user_id: options.user_id,
            topic_id: options.topic_id,
            agent_id: options.agent_id,
            action: options.action,
            payload: options.payload,
            ..audit_event(501, None, None, TOOL_TOPIC_BINDING_ROLLBACK, json!({}))
        })
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_BINDING_ROLLBACK,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("rollback should search across audit pages");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["operation"], "restore");
    assert_eq!(parsed["binding"]["agent_id"], "agent-old");
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn agent_profile_rollback_deletes_when_previous_snapshot_absent() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, agent_id| {
            Ok(Some(AgentProfileRecord {
                schema_version: 1,
                version: 2,
                user_id: 77,
                agent_id,
                profile: json!({"mode":"current"}),
                created_at: 10,
                updated_at: 20,
            }))
        });
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .returning(|_, _, _| {
            Ok(vec![crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 3,
                event_id: "evt-3".to_string(),
                user_id: 77,
                topic_id: None,
                agent_id: Some("agent-a".to_string()),
                action: TOOL_AGENT_PROFILE_DELETE.to_string(),
                payload: json!({"agent_id":"agent-a","previous":null,"outcome":"applied"}),
                created_at: 30,
            }])
        });
    mock.expect_delete_agent_profile()
        .with(eq(77_i64), eq("agent-a".to_string()))
        .returning(|_, _| Ok(()));
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.action == TOOL_AGENT_PROFILE_ROLLBACK
                && options.payload.get("operation") == Some(&json!("delete"))
        })
        .returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 4,
                event_id: "evt-4".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 40,
            })
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_AGENT_PROFILE_ROLLBACK,
            r#"{"agent_id":"agent-a"}"#,
            None,
            None,
        )
        .await
        .expect("agent rollback should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["operation"], "delete");
    assert!(parsed["profile"].is_null());
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_context_upsert_persists_and_audits() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_context()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_upsert_topic_context()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.context == "Use maintenance window rules"
        })
        .returning(|options| {
            Ok(TopicContextRecord {
                schema_version: 1,
                version: 1,
                user_id: options.user_id,
                topic_id: options.topic_id,
                context: options.context,
                created_at: 10,
                updated_at: 10,
            })
        });
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.topic_id.as_deref() == Some("topic-a")
                && options.action == TOOL_TOPIC_CONTEXT_UPSERT
        })
        .returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 1,
                event_id: "evt-1".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 11,
            })
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_CONTEXT_UPSERT,
            r#"{"topic_id":"topic-a","context":"Use maintenance window rules"}"#,
            None,
            None,
        )
        .await
        .expect("topic context upsert should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["topic_context"]["topic_id"], "topic-a");
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_context_get_reports_missing_record() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_context()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_CONTEXT_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic context get should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["found"], false);
    assert!(parsed["topic_context"].is_null());
}

#[tokio::test]
async fn topic_context_upsert_rejects_agents_style_content() {
    let mock = crate::storage::MockStorageProvider::new();
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);

    let error = provider
        .execute(
            TOOL_TOPIC_CONTEXT_UPSERT,
            r##"{"topic_id":"topic-a","context":"# AGENTS\nUse release checklist"}"##,
            None,
            None,
        )
        .await
        .expect_err("AGENTS-style topic context must be rejected");

    assert!(error
        .to_string()
        .contains("store AGENTS.md-style documents in topic_agents_md"));
}

#[tokio::test]
async fn topic_context_upsert_surfaces_duplicate_topic_prompt_error() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_context()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_upsert_topic_context().returning(|_| {
        Err(crate::storage::StorageError::DuplicateTopicPromptContent {
            topic_id: "topic-a".to_string(),
            existing_kind: "topic_agents_md".to_string(),
            attempted_kind: "topic_context".to_string(),
        })
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let error = provider
        .execute(
            TOOL_TOPIC_CONTEXT_UPSERT,
            r#"{"topic_id":"topic-a","context":"Use release checklist"}"#,
            None,
            None,
        )
        .await
        .expect_err("duplicate topic prompt must be rejected");

    assert!(error
        .to_string()
        .contains("duplicate topic prompt content for topic topic-a"));
}

#[tokio::test]
async fn topic_context_rollback_restores_previous_snapshot() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_context()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| {
            Ok(Some(TopicContextRecord {
                schema_version: 1,
                version: 3,
                user_id: 77,
                topic_id,
                context: "current context".to_string(),
                created_at: 10,
                updated_at: 30,
            }))
        });
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .returning(|_, _, _| {
            Ok(vec![crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 2,
                event_id: "evt-2".to_string(),
                user_id: 77,
                topic_id: Some("topic-a".to_string()),
                agent_id: None,
                action: TOOL_TOPIC_CONTEXT_UPSERT.to_string(),
                payload: json!({
                    "topic_id": "topic-a",
                    "previous": {
                        "schema_version": 1,
                        "version": 1,
                        "user_id": 77,
                        "topic_id": "topic-a",
                        "context": "previous context",
                        "created_at": 5,
                        "updated_at": 6
                    },
                    "outcome": "applied"
                }),
                created_at: 20,
            }])
        });
    mock.expect_upsert_topic_context()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.context == "previous context"
        })
        .returning(|options| {
            Ok(TopicContextRecord {
                schema_version: 1,
                version: 4,
                user_id: options.user_id,
                topic_id: options.topic_id,
                context: options.context,
                created_at: 5,
                updated_at: 40,
            })
        });
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.action == TOOL_TOPIC_CONTEXT_ROLLBACK
                && options.payload.get("operation") == Some(&json!("restore"))
        })
        .returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 4,
                event_id: "evt-4".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 50,
            })
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_CONTEXT_ROLLBACK,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic context rollback should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["operation"], "restore");
    assert_eq!(parsed["topic_context"]["context"], "previous context");
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_context_rollback_rejects_duplicate_topic_prompt_restore() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_context()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| {
            Ok(Some(TopicContextRecord {
                schema_version: 1,
                version: 3,
                user_id: 77,
                topic_id,
                context: "current context".to_string(),
                created_at: 10,
                updated_at: 30,
            }))
        });
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .returning(|_, _, _| {
            Ok(vec![crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 2,
                event_id: "evt-2".to_string(),
                user_id: 77,
                topic_id: Some("topic-a".to_string()),
                agent_id: None,
                action: TOOL_TOPIC_CONTEXT_UPSERT.to_string(),
                payload: json!({
                    "topic_id": "topic-a",
                    "previous": {
                        "schema_version": 1,
                        "version": 1,
                        "user_id": 77,
                        "topic_id": "topic-a",
                        "context": "duplicate context",
                        "created_at": 5,
                        "updated_at": 6
                    },
                    "outcome": "applied"
                }),
                created_at: 20,
            }])
        });
    mock.expect_upsert_topic_context().returning(|_| {
        Err(crate::storage::StorageError::DuplicateTopicPromptContent {
            topic_id: "topic-a".to_string(),
            existing_kind: "topic_agents_md".to_string(),
            attempted_kind: "topic_context".to_string(),
        })
    });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let error = provider
        .execute(
            TOOL_TOPIC_CONTEXT_ROLLBACK,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect_err("duplicate rollback restore must be rejected");

    assert!(error
        .to_string()
        .contains("duplicate topic prompt content for topic topic-a"));
}

#[tokio::test]
async fn topic_agents_md_upsert_persists_and_audits() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_agents_md()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_upsert_topic_agents_md()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.agents_md == "# Topic AGENTS\nFollow release process"
        })
        .returning(|options| {
            Ok(TopicAgentsMdRecord {
                schema_version: 1,
                version: 1,
                user_id: options.user_id,
                topic_id: options.topic_id,
                agents_md: options.agents_md,
                created_at: 10,
                updated_at: 10,
            })
        });
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.topic_id.as_deref() == Some("topic-a")
                && options.action == TOOL_TOPIC_AGENTS_MD_UPSERT
        })
        .returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 1,
                event_id: "evt-1".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 11,
            })
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENTS_MD_UPSERT,
            r##"{"topic_id":"topic-a","agents_md":"# Topic AGENTS\nFollow release process"}"##,
            None,
            None,
        )
        .await
        .expect("topic AGENTS.md upsert should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["topic_agents_md"]["topic_id"], "topic-a");
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_agents_md_get_reports_missing_record() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_agents_md()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENTS_MD_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic AGENTS.md get should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["found"], false);
    assert!(parsed["topic_agents_md"].is_null());
}

#[tokio::test]
async fn topic_agents_md_rollback_restores_previous_snapshot() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_agents_md()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| {
            Ok(Some(TopicAgentsMdRecord {
                schema_version: 1,
                version: 3,
                user_id: 77,
                topic_id,
                agents_md: "# Current AGENTS".to_string(),
                created_at: 10,
                updated_at: 30,
            }))
        });
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .returning(|_, _, _| {
            Ok(vec![crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 2,
                event_id: "evt-2".to_string(),
                user_id: 77,
                topic_id: Some("topic-a".to_string()),
                agent_id: None,
                action: TOOL_TOPIC_AGENTS_MD_UPSERT.to_string(),
                payload: json!({
                    "topic_id": "topic-a",
                    "previous": {
                        "schema_version": 1,
                        "version": 1,
                        "user_id": 77,
                        "topic_id": "topic-a",
                        "agents_md": "# Previous AGENTS",
                        "created_at": 5,
                        "updated_at": 6
                    },
                    "outcome": "applied"
                }),
                created_at: 20,
            }])
        });
    mock.expect_upsert_topic_agents_md()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.agents_md == "# Previous AGENTS"
        })
        .returning(|options| {
            Ok(TopicAgentsMdRecord {
                schema_version: 1,
                version: 4,
                user_id: options.user_id,
                topic_id: options.topic_id,
                agents_md: options.agents_md,
                created_at: 5,
                updated_at: 40,
            })
        });
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.action == TOOL_TOPIC_AGENTS_MD_ROLLBACK
                && options.payload.get("operation") == Some(&json!("restore"))
        })
        .returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 4,
                event_id: "evt-4".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 50,
            })
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_AGENTS_MD_ROLLBACK,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic AGENTS.md rollback should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["operation"], "restore");
    assert_eq!(parsed["topic_agents_md"]["agents_md"], "# Previous AGENTS");
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_agents_md_upsert_rejects_more_than_300_lines() {
    let mock = crate::storage::MockStorageProvider::new();
    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let oversized = vec!["line"; crate::storage::TOPIC_AGENTS_MD_MAX_LINES + 1].join("\n");
    let arguments = serde_json::json!({
        "topic_id": "topic-a",
        "agents_md": oversized,
    })
    .to_string();

    let error = provider
        .execute(TOOL_TOPIC_AGENTS_MD_UPSERT, &arguments, None, None)
        .await
        .expect_err("oversized AGENTS.md must be rejected");

    assert!(error
        .to_string()
        .contains("agents_md must not exceed 300 lines"));
}

#[tokio::test]
async fn topic_infra_upsert_persists_and_audits() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, _| Ok(None));
    mock.expect_get_secret_value().returning(|_, _| Ok(None));
    mock.expect_upsert_topic_infra_config()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.target_name == "prod-app"
                && options.host == "prod.example.com"
                && options.remote_user == "deploy"
        })
        .returning(|options| {
            Ok(TopicInfraConfigRecord {
                schema_version: 1,
                version: 1,
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
                created_at: 10,
                updated_at: 10,
            })
        });
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.topic_id.as_deref() == Some("topic-a")
                && options.action == TOOL_TOPIC_INFRA_UPSERT
        })
        .returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                None,
                &options.action,
                options.payload,
            ))
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_INFRA_UPSERT,
            r#"{"topic_id":"topic-a","target_name":"prod-app","host":"prod.example.com","remote_user":"deploy","auth_mode":"private_key","secret_ref":"storage:ssh/prod-key","sudo_secret_ref":"storage:ssh/prod-sudo","environment":"prod","tags":["prod"],"allowed_tool_modes":["exec","read_file"],"approval_required_modes":["sudo_exec"]}"#,
            None,
            None,
        )
        .await
        .expect("topic infra upsert should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["topic_infra"]["topic_id"], "topic-a");
    assert_eq!(parsed["topic_infra"]["target_name"], "prod-app");
    assert_eq!(parsed["preflight"]["provider_enabled"], false);
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn topic_infra_rollback_restores_previous_snapshot() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_infra_config()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| Ok(Some(topic_infra(77, &topic_id, 3))));
    mock.expect_get_secret_value().returning(|_, _| Ok(None));
    mock.expect_list_audit_events_page()
        .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
        .returning(|_, _, _| {
            Ok(vec![audit_event(
                2,
                Some("topic-a"),
                None,
                TOOL_TOPIC_INFRA_UPSERT,
                json!({
                    "topic_id": "topic-a",
                    "previous": topic_infra(77, "topic-a", 1),
                    "outcome": "applied"
                }),
            )])
        });
    mock.expect_upsert_topic_infra_config()
        .withf(|options| {
            options.user_id == 77
                && options.topic_id == "topic-a"
                && options.target_name == "prod-app"
                && options.host == "prod.example.com"
        })
        .returning(|options| {
            Ok(TopicInfraConfigRecord {
                schema_version: 1,
                version: 4,
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
                created_at: 10,
                updated_at: 40,
            })
        });
    mock.expect_append_audit_event()
        .withf(|options: &AppendAuditEventOptions| {
            options.user_id == 77
                && options.action == TOOL_TOPIC_INFRA_ROLLBACK
                && options.payload.get("operation") == Some(&json!("restore"))
        })
        .returning(|options| {
            Ok(audit_event(
                4,
                options.topic_id.as_deref(),
                None,
                &options.action,
                options.payload,
            ))
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_TOPIC_INFRA_ROLLBACK,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("topic infra rollback should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["operation"], "restore");
    assert_eq!(parsed["topic_infra"]["host"], "prod.example.com");
    assert_eq!(parsed["preflight"]["provider_enabled"], false);
    assert_eq!(parsed["audit_status"], "written");
}

#[tokio::test]
async fn private_secret_probe_reports_presence_without_exposing_content() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_secret_value()
        .with(eq(77_i64), eq("vds".to_string()))
        .returning(|_, _| {
            Ok(Some(
                "-----BEGIN OPENSSH PRIVATE KEY-----\ninvalid\n-----END OPENSSH PRIVATE KEY-----\n"
                    .to_string(),
            ))
        });

    let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
    let response = provider
        .execute(
            TOOL_PRIVATE_SECRET_PROBE,
            r#"{"secret_ref":"storage:vds","kind":"ssh_private_key"}"#,
            None,
            None,
        )
        .await
        .expect("private secret probe should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["secret_probe"]["secret_ref"], "storage:vds");
    assert_eq!(parsed["secret_probe"]["present"], true);
    assert!(parsed["secret_probe"].get("content").is_none());
}

#[tokio::test]
async fn tool_registry_routes_to_manager_provider() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_agent_profile()
        .with(eq(77_i64), eq("agent-x".to_string()))
        .returning(|user_id, agent_id| {
            Ok(Some(AgentProfileRecord {
                schema_version: 1,
                version: 5,
                user_id,
                agent_id,
                profile: json!({"role":"support"}),
                created_at: 10,
                updated_at: 20,
            }))
        });

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ManagerControlPlaneProvider::new(
        Arc::new(mock),
        77,
    )));

    let response = registry
        .execute(
            TOOL_AGENT_PROFILE_GET,
            r#"{"agent_id":"agent-x"}"#,
            None,
            None,
        )
        .await
        .expect("registry execution should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["found"], true);
    assert_eq!(parsed["profile"]["agent_id"], "agent-x");
}

#[tokio::test]
async fn tool_registry_routes_topic_agents_md_to_manager_provider() {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_topic_agents_md()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|_, topic_id| {
            Ok(Some(TopicAgentsMdRecord {
                schema_version: 1,
                version: 1,
                user_id: 77,
                topic_id,
                agents_md: "# Topic AGENTS".to_string(),
                created_at: 10,
                updated_at: 10,
            }))
        });

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ManagerControlPlaneProvider::new(
        Arc::new(mock),
        77,
    )));

    let response = registry
        .execute(
            TOOL_TOPIC_AGENTS_MD_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect("registry execution should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&response).expect("response must be json");
    assert_eq!(parsed["found"], true);
    assert_eq!(parsed["topic_agents_md"]["topic_id"], "topic-a");
}

#[tokio::test]
async fn tool_registry_without_manager_provider_rejects_manager_tools() {
    let registry = ToolRegistry::new();
    let err = registry
        .execute(
            TOOL_TOPIC_BINDING_GET,
            r#"{"topic_id":"topic-a"}"#,
            None,
            None,
        )
        .await
        .expect_err("manager tools must be unavailable without provider");

    assert!(err.to_string().contains("Unknown tool"));
}
