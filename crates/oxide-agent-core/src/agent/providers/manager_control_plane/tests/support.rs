use super::*;
use anyhow::Result;
use async_trait::async_trait;

pub(super) fn binding(
    user_id: i64,
    topic_id: &str,
    agent_id: &str,
    version: u64,
) -> TopicBindingRecord {
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

pub(super) fn topic_infra(user_id: i64, topic_id: &str, version: u64) -> TopicInfraConfigRecord {
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

pub(super) fn user_config_with_contexts(
    contexts: impl IntoIterator<Item = (String, UserContextConfig)>,
) -> UserConfig {
    UserConfig {
        contexts: contexts.into_iter().collect(),
        ..UserConfig::default()
    }
}

pub(super) fn forum_topic_context(
    chat_id: i64,
    thread_id: i64,
    forum_topic_name: Option<&str>,
    forum_topic_icon_color: Option<u32>,
    forum_topic_icon_custom_emoji_id: Option<&str>,
    forum_topic_closed: bool,
) -> UserContextConfig {
    UserContextConfig {
        chat_id: Some(chat_id),
        thread_id: Some(thread_id),
        forum_topic_name: forum_topic_name.map(str::to_string),
        forum_topic_icon_color,
        forum_topic_icon_custom_emoji_id: forum_topic_icon_custom_emoji_id.map(str::to_string),
        forum_topic_closed,
        ..UserContextConfig::default()
    }
}

pub(super) fn agent_profile_record(
    agent_id: impl Into<String>,
    version: u64,
    profile: serde_json::Value,
    created_at: i64,
    updated_at: i64,
) -> AgentProfileRecord {
    AgentProfileRecord {
        schema_version: 1,
        version,
        user_id: 77,
        agent_id: agent_id.into(),
        profile,
        created_at,
        updated_at,
    }
}

pub(super) fn topic_context_record(
    topic_id: impl Into<String>,
    version: u64,
    context: impl Into<String>,
    created_at: i64,
    updated_at: i64,
) -> TopicContextRecord {
    TopicContextRecord {
        schema_version: 1,
        version,
        user_id: 77,
        topic_id: topic_id.into(),
        context: context.into(),
        created_at,
        updated_at,
    }
}

pub(super) fn topic_agents_md_record(
    topic_id: impl Into<String>,
    version: u64,
    agents_md: impl Into<String>,
    created_at: i64,
    updated_at: i64,
) -> TopicAgentsMdRecord {
    TopicAgentsMdRecord {
        schema_version: 1,
        version,
        user_id: 77,
        topic_id: topic_id.into(),
        agents_md: agents_md.into(),
        created_at,
        updated_at,
    }
}

pub(super) fn parse_json_response(response: &str) -> serde_json::Value {
    serde_json::from_str(response).expect("response must be valid json")
}

pub(super) fn provider_status<'a>(
    parsed: &'a serde_json::Value,
    provider: &str,
) -> &'a serde_json::Value {
    parsed["tools"]["provider_statuses"]
        .as_array()
        .expect("provider_statuses must be an array")
        .iter()
        .find(|entry| entry["provider"] == provider)
        .unwrap_or_else(|| panic!("{provider} provider status must be present"))
}

pub(super) fn expect_forum_topic_provision_profile_calls(
    mock: &mut crate::storage::MockStorageProvider,
) {
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
            Ok(agent_profile_record(
                options.agent_id,
                1,
                options.profile,
                10,
                10,
            ))
        });
}

pub(super) fn expect_forum_topic_provision_binding_calls(
    mock: &mut crate::storage::MockStorageProvider,
) {
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

pub(super) fn expect_forum_topic_provision_infra_calls(
    mock: &mut crate::storage::MockStorageProvider,
) {
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

pub(super) fn mock_storage_for_forum_topic_provision() -> crate::storage::MockStorageProvider {
    let mut mock = crate::storage::MockStorageProvider::new();
    mock.expect_get_user_config()
        .times(1)
        .returning(|_| Ok(UserConfig::default()));
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

pub(super) fn audit_event(
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

pub(super) fn topic_delete_user_config() -> UserConfig {
    UserConfig {
        contexts: std::collections::HashMap::from([(
            "-100999:42".to_string(),
            UserContextConfig {
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
        ..UserConfig::default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LifecycleCall {
    Create(ForumTopicCreateRequest),
    Edit(ForumTopicEditRequest),
    Close(ForumTopicThreadRequest),
    Reopen(ForumTopicThreadRequest),
    Delete(ForumTopicThreadRequest),
}

pub(super) struct FakeTopicLifecycle {
    calls: std::sync::Mutex<Vec<LifecycleCall>>,
}

impl FakeTopicLifecycle {
    pub(super) fn new() -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub(super) fn calls(&self) -> Vec<LifecycleCall> {
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

pub(super) struct FakeTopicSandboxCleanup {
    calls: std::sync::Mutex<Vec<(i64, i64, i64)>>,
}

impl FakeTopicSandboxCleanup {
    pub(super) fn new() -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub(super) fn calls(&self) -> Vec<(i64, i64, i64)> {
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

pub(super) struct FakeTopicSandboxControl {
    records: std::sync::Mutex<std::collections::HashMap<String, SandboxContainerRecord>>,
    ensured: std::sync::Mutex<Vec<String>>,
    recreated: std::sync::Mutex<Vec<String>>,
    deleted: std::sync::Mutex<Vec<String>>,
}

impl FakeTopicSandboxControl {
    pub(super) fn new(records: Vec<SandboxContainerRecord>) -> Self {
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

    pub(super) fn sandbox_record(user_id: i64, topic_id: &str) -> SandboxContainerRecord {
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

    pub(super) fn ensured(&self) -> Vec<String> {
        self.ensured.lock().expect("mutex poisoned").clone()
    }

    pub(super) fn recreated(&self) -> Vec<String> {
        self.recreated.lock().expect("mutex poisoned").clone()
    }

    pub(super) fn deleted(&self) -> Vec<String> {
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
