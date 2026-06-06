use async_trait::async_trait;
use axum::http::HeaderMap;
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(feature = "profile-lite")]
use std::time::Duration;
use std::time::Instant;

use oxide_agent_core::agent::progress::{FileDeliveryKind, LlmRetryState, ProgressState};
use oxide_agent_core::agent::AgentMemory;
use oxide_agent_core::agent::{TodoItem, TodoList, TodoStatus};
use oxide_agent_core::config::{AgentSettings, ModelInfo};
#[cfg(feature = "profile-lite")]
use oxide_agent_core::llm::{ChatResponse, ChatWithToolsRequest, LlmError, Message};
use oxide_agent_core::llm::{LlmClient, LlmProvider};
use oxide_agent_core::sandbox::{SandboxContainerRecord, SandboxScope};
use oxide_agent_runtime::SessionRegistry;
use oxide_agent_web_contracts::{
    AgentEffort, AgentProfileSelection, CreateAgentProfileRequest,
    CreateSessionRequest as ApiCreateSessionRequest,
    CreateTaskVersionRequest as ApiCreateTaskVersionRequest, ErrorCode, LoginRequest,
    ModelSelection, PersistedTaskEvent, ProgressSnapshot, RegisterRequest, TaskAttachment,
    TaskEventKind, TaskStatus as ApiTaskStatus, UpdateSessionProfileRequest,
    UpdateUserSettingsRequest, WebTaskRecord,
};
#[cfg(feature = "profile-lite")]
use oxide_agent_web_contracts::{
    CreateTaskRequest as ApiCreateTaskRequest, PendingUserInputView,
    ResumeTaskRequest as ApiResumeTaskRequest, UserInputKind as ApiUserInputKind,
    UserMessageEventPayload,
};
#[cfg(feature = "profile-lite")]
use tokio::sync::Notify;
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use crate::persistence::{WebTaskFileRecord, WEB_TASK_FILE_SCHEMA_VERSION};

#[cfg(feature = "storage-sqlx")]
use super::WebStoreKind;
use super::{
    api_cancel_task, api_create_agent_profile, api_create_session, api_create_session_with_request,
    api_create_task_version, api_delete_session, api_get_session, api_get_settings,
    api_get_task_events, api_get_task_progress, api_list_sessions, api_update_session_profile,
    api_update_settings, auth_cookie_value, csrf_header_value, parse_web_bool, AppState,
    TaskEventsQuery, WebAssetsConfig, WebSandboxControl, WebStartupError, AUTH_COOKIE_NAME,
    WEB_TASK_SCHEMA_VERSION,
};
#[cfg(feature = "profile-lite")]
use super::{api_create_task, api_get_task, api_list_tasks, api_resume_task};
use crate::auth::{login_user, register_user};
use crate::scripted_llm::{ScriptedLlmProvider, ScriptedResponse};
use crate::session::WebSessionManager;

#[derive(Clone, Default)]
struct FakeSandboxControl {
    state: Arc<Mutex<FakeSandboxState>>,
}

#[derive(Default)]
struct FakeSandboxState {
    ensured_scopes: Vec<SandboxScope>,
    destroyed_scopes: Vec<SandboxScope>,
    deleted_names: Vec<String>,
    sandboxes: Vec<SandboxContainerRecord>,
}

impl FakeSandboxControl {
    fn with_sandboxes(sandboxes: Vec<SandboxContainerRecord>) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeSandboxState {
                sandboxes,
                ..FakeSandboxState::default()
            })),
        }
    }

    fn ensured_scopes(&self) -> Vec<SandboxScope> {
        self.state
            .lock()
            .expect("fake sandbox state")
            .ensured_scopes
            .clone()
    }

    fn destroyed_scopes(&self) -> Vec<SandboxScope> {
        self.state
            .lock()
            .expect("fake sandbox state")
            .destroyed_scopes
            .clone()
    }

    fn deleted_names(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("fake sandbox state")
            .deleted_names
            .clone()
    }
}

fn fake_sandbox_record(scope: SandboxScope) -> SandboxContainerRecord {
    SandboxContainerRecord {
        container_id: scope.stable_name(),
        container_name: scope.container_name(),
        image: Some("fake-image".to_string()),
        created_at: None,
        state: Some("running".to_string()),
        status: Some("running".to_string()),
        running: true,
        user_id: Some(scope.owner_id()),
        scope: Some(scope.namespace().to_string()),
        chat_id: scope.chat_id(),
        thread_id: scope.thread_id(),
        labels: scope.docker_labels(),
    }
}

#[async_trait]
impl WebSandboxControl for FakeSandboxControl {
    async fn destroy_scope(&self, scope: SandboxScope) -> anyhow::Result<()> {
        let mut state = self.state.lock().expect("fake sandbox state");
        state.destroyed_scopes.push(scope.clone());
        state
            .sandboxes
            .retain(|sandbox| sandbox.container_name != scope.container_name());
        Ok(())
    }

    async fn list_user_sandboxes(
        &self,
        user_id: i64,
    ) -> anyhow::Result<Vec<SandboxContainerRecord>> {
        let state = self.state.lock().expect("fake sandbox state");
        Ok(state
            .sandboxes
            .iter()
            .filter(|sandbox| sandbox.user_id == Some(user_id))
            .cloned()
            .collect())
    }

    async fn ensure_scope_sandbox(
        &self,
        scope: SandboxScope,
    ) -> anyhow::Result<SandboxContainerRecord> {
        let mut state = self.state.lock().expect("fake sandbox state");
        state.ensured_scopes.push(scope.clone());
        let record = fake_sandbox_record(scope);
        state
            .sandboxes
            .retain(|sandbox| sandbox.container_name != record.container_name);
        state.sandboxes.push(record.clone());
        Ok(record)
    }

    async fn delete_sandbox_by_name(
        &self,
        user_id: i64,
        container_name: &str,
    ) -> anyhow::Result<bool> {
        let mut state = self.state.lock().expect("fake sandbox state");
        state.deleted_names.push(container_name.to_string());
        let before = state.sandboxes.len();
        state.sandboxes.retain(|sandbox| {
            !(sandbox.user_id == Some(user_id) && sandbox.container_name == container_name)
        });
        Ok(before != state.sandboxes.len())
    }
}

#[cfg(feature = "profile-lite")]
struct AutoTitleTestLlmProvider {
    title_response: String,
    agent_response: String,
    block_title: bool,
    title_started: Arc<Notify>,
    title_release: Arc<Notify>,
    title_returned: Arc<Notify>,
}

#[cfg(feature = "profile-lite")]
impl AutoTitleTestLlmProvider {
    fn new(title_response: impl Into<String>, agent_response: impl Into<String>) -> Arc<Self> {
        Self::with_title_blocking(title_response, agent_response, false)
    }

    fn blocking_title(
        title_response: impl Into<String>,
        agent_response: impl Into<String>,
    ) -> Arc<Self> {
        Self::with_title_blocking(title_response, agent_response, true)
    }

    fn with_title_blocking(
        title_response: impl Into<String>,
        agent_response: impl Into<String>,
        block_title: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            title_response: title_response.into(),
            agent_response: agent_response.into(),
            block_title,
            title_started: Arc::new(Notify::new()),
            title_release: Arc::new(Notify::new()),
            title_returned: Arc::new(Notify::new()),
        })
    }

    async fn wait_title_started(&self) {
        self.title_started.notified().await;
    }

    async fn wait_title_returned(&self) {
        self.title_returned.notified().await;
    }

    fn release_title(&self) {
        self.title_release.notify_one();
    }

    fn is_auto_title_request(request: &ChatWithToolsRequest<'_>) -> bool {
        request
            .system_prompt
            .contains("You generate short chat titles")
    }

    fn agent_response(&self) -> ChatResponse {
        ChatResponse {
            content: Some(
                serde_json::json!({
                    "thought": "Responding to user",
                    "final_answer": self.agent_response.as_str(),
                })
                .to_string(),
            ),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            reasoning_content: None,
            usage: None,
        }
    }
}

#[cfg(feature = "profile-lite")]
#[async_trait]
impl LlmProvider for AutoTitleTestLlmProvider {
    async fn complete_internal_text(
        &self,
        _system_prompt: &str,
        _history: &[Message],
        _user_message: &str,
        _model_id: &str,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        Ok(self.agent_response.clone())
    }

    async fn chat_with_tools<'a>(
        &self,
        request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        if Self::is_auto_title_request(&request) {
            self.title_started.notify_one();
            if self.block_title {
                self.title_release.notified().await;
            }
            self.title_returned.notify_one();
            return Ok(ChatResponse {
                content: Some(self.title_response.clone()),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                reasoning_content: None,
                usage: None,
            });
        }

        Ok(self.agent_response())
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("transcribe not implemented".to_string()))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "analyze_image not implemented".to_string(),
        ))
    }
}

#[test]
fn parse_web_bool_accepts_common_enabled_values() {
    for value in ["1", "true", "TRUE", "yes", "on", " on "] {
        assert!(parse_web_bool(value), "{value:?} should be enabled");
    }
}

#[test]
fn parse_web_bool_rejects_disabled_or_unknown_values() {
    for value in ["", "0", "false", "no", "off", "enabled"] {
        assert!(!parse_web_bool(value), "{value:?} should be disabled");
    }
}

#[test]
fn bootstrap_required_depends_on_registration_users_and_token() {
    assert!(super::web_bootstrap_required(false, 0, true));
    assert!(!super::web_bootstrap_required(true, 0, true));
    assert!(!super::web_bootstrap_required(false, 1, true));
    assert!(!super::web_bootstrap_required(false, 0, false));
}

#[test]
fn markdown_preview_strips_common_markdown_title_markup() {
    let preview = super::markdown_preview(
        "# Browser smoke\n\n- item one\n- item two\n\n```rust\nfn main() {}\n```",
    );

    assert_eq!(preview, "Browser smoke item one item two");
}

#[test]
fn auth_cookie_and_csrf_values_are_extracted_from_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::COOKIE,
        format!("theme=light; {AUTH_COOKIE_NAME}=token-123; other=1")
            .parse()
            .expect("cookie header"),
    );
    headers.insert("x-csrf-token", "csrf-123".parse().expect("csrf header"));

    assert_eq!(
        auth_cookie_value(&headers).expect("auth cookie"),
        "token-123"
    );
    assert_eq!(csrf_header_value(&headers).expect("csrf"), "csrf-123");
}

#[test]
fn csrf_origin_check_accepts_same_origin_and_rejects_cross_origin() {
    let mut same_origin = HeaderMap::new();
    same_origin.insert("x-forwarded-proto", "https".parse().expect("proto"));
    same_origin.insert("x-forwarded-host", "app.example".parse().expect("host"));
    same_origin.insert(
        axum::http::header::ORIGIN,
        "https://app.example".parse().expect("origin"),
    );
    assert!(super::validate_csrf_request_origin(&same_origin).is_ok());

    let mut same_referer = HeaderMap::new();
    same_referer.insert("x-forwarded-proto", "https".parse().expect("proto"));
    same_referer.insert("x-forwarded-host", "app.example".parse().expect("host"));
    same_referer.insert(
        axum::http::header::REFERER,
        "https://app.example/app/session/1"
            .parse()
            .expect("referer"),
    );
    assert!(super::validate_csrf_request_origin(&same_referer).is_ok());

    let mut cross_origin = same_origin;
    cross_origin.insert(
        axum::http::header::ORIGIN,
        "https://evil.example".parse().expect("origin"),
    );
    let (status, axum::Json(error)) =
        super::validate_csrf_request_origin(&cross_origin).expect_err("cross origin");
    assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
    assert_eq!(error.error.code, ErrorCode::CsrfInvalid);
}

#[test]
fn auth_rate_limiter_uses_fixed_window() {
    let mut limiter = super::AuthRateLimiter::new();
    let now = Instant::now();
    let key = "127.0.0.1:alice";

    for _ in 0..super::AUTH_RATE_LIMIT_MAX_FAILURES {
        assert!(!limiter.is_limited(key, now));
        limiter.record_failure(key.to_string(), now);
    }
    assert!(limiter.is_limited(key, now));
    assert!(!limiter.is_limited(key, now + super::AUTH_RATE_LIMIT_WINDOW));
}

#[tokio::test]
async fn api_login_rate_limits_by_ip_and_login_key() {
    let state = test_app_state();
    let now = chrono::Utc::now();
    register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register user");

    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", "198.51.100.10".parse().expect("ip"));
    for _ in 0..super::AUTH_RATE_LIMIT_MAX_FAILURES {
        let (status, axum::Json(error)) = super::api_login(
            axum::extract::State(state.clone()),
            headers.clone(),
            axum::Json(LoginRequest {
                login: "alice".to_string(),
                password: "wrong password".to_string(),
            }),
        )
        .await
        .expect_err("wrong password should fail");
        assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);
        assert_eq!(error.error.code, ErrorCode::InvalidCredentials);
    }

    let (status, axum::Json(error)) = super::api_login(
        axum::extract::State(state.clone()),
        headers,
        axum::Json(LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        }),
    )
    .await
    .expect_err("same key should be rate limited before password verification");
    assert_eq!(status, axum::http::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error.error.code, ErrorCode::RateLimited);

    let mut other_ip_headers = HeaderMap::new();
    other_ip_headers.insert("x-forwarded-for", "198.51.100.20".parse().expect("ip"));
    let (_headers, axum::Json(response)) = super::api_login(
        axum::extract::State(state),
        other_ip_headers,
        axum::Json(LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        }),
    )
    .await
    .expect("different IP/login key should not be rate limited");
    assert_eq!(response.user.login, "alice");
}

#[tokio::test]
async fn api_register_failures_are_rate_limited() {
    let _lock = web_env_mutex().lock().await;
    let _guard = EnvGuard::capture(&["OXIDE_WEB_REGISTRATION_ENABLED"]);
    std::env::set_var("OXIDE_WEB_REGISTRATION_ENABLED", "false");

    let state = test_app_state();
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", "203.0.113.10".parse().expect("ip"));
    for _ in 0..super::AUTH_RATE_LIMIT_MAX_FAILURES {
        let (status, axum::Json(error)) = super::api_register(
            axum::extract::State(state.clone()),
            headers.clone(),
            axum::Json(RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            }),
        )
        .await
        .expect_err("disabled registration should fail");
        assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
        assert_eq!(error.error.code, ErrorCode::RegistrationDisabled);
    }

    let (status, axum::Json(error)) = super::api_register(
        axum::extract::State(state),
        headers,
        axum::Json(RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        }),
    )
    .await
    .expect_err("disabled registration should become rate limited");
    assert_eq!(status, axum::http::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error.error.code, ErrorCode::RateLimited);
}

#[tokio::test]
async fn api_register_starts_browser_auth_session() {
    let _lock = web_env_mutex().lock().await;
    let _guard = EnvGuard::capture(&["OXIDE_WEB_REGISTRATION_ENABLED"]);
    std::env::set_var("OXIDE_WEB_REGISTRATION_ENABLED", "true");

    let state = test_app_state();
    let (response_headers, axum::Json(response)) = super::api_register(
        axum::extract::State(state.clone()),
        HeaderMap::new(),
        axum::Json(RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        }),
    )
    .await
    .expect("register should create auth session");
    assert_eq!(response.user.login, "alice");
    let csrf_token = response.csrf_token.expect("register returns csrf token");
    let raw_cookie = response_headers
        .get(axum::http::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .expect("set-cookie header");
    assert!(raw_cookie.contains("HttpOnly"));
    let raw_token = raw_cookie
        .strip_prefix(&format!("{AUTH_COOKIE_NAME}="))
        .and_then(|value| value.split(';').next())
        .expect("session cookie value")
        .to_string();

    let axum::Json(me) = super::api_me(axum::extract::State(state), auth_headers(&raw_token, None))
        .await
        .expect("registered auth session can load current user");
    assert_eq!(me.user.login, "alice");
    assert_eq!(me.csrf_token, csrf_token);
}

#[tokio::test]
async fn mutating_session_api_rejects_cross_origin_csrf_request() {
    let state = test_app_state();
    let now = chrono::Utc::now();
    register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register user");
    let (_, auth_session, token) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login user");
    let mut headers = auth_headers(&token, Some(&auth_session.csrf_token));
    headers.insert("x-forwarded-proto", "https".parse().expect("proto"));
    headers.insert("x-forwarded-host", "app.example".parse().expect("host"));
    headers.insert(
        axum::http::header::ORIGIN,
        "https://evil.example".parse().expect("origin"),
    );

    let (status, axum::Json(error)) = api_create_session(axum::extract::State(state), headers)
        .await
        .expect_err("cross-origin mutating request should fail");
    assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
    assert_eq!(error.error.code, ErrorCode::CsrfInvalid);
}

#[tokio::test]
async fn api_list_model_routes_returns_empty_models_when_discovery_is_unavailable() {
    let _lock = web_env_mutex().lock().await;
    let _guard = EnvGuard::capture(&[
        "OPENCODE_API_KEY",
        "OPENCODE_ZEN_API_KEY",
        "OPENCODE_GO_API_KEY",
        "OPENCODE_GO_MODELS_URL",
        "OPENCODE_ZEN_MODELS_URL",
        "LLM_HTTP_TIMEOUT_SECS",
    ]);
    std::env::set_var("OPENCODE_API_KEY", "test-opencode-key");
    std::env::remove_var("OPENCODE_ZEN_API_KEY");
    std::env::remove_var("OPENCODE_GO_API_KEY");
    std::env::set_var("OPENCODE_GO_MODELS_URL", "http://127.0.0.1:9/models");
    std::env::set_var("OPENCODE_ZEN_MODELS_URL", "http://127.0.0.1:9/models");
    std::env::set_var("LLM_HTTP_TIMEOUT_SECS", "1");

    let state = test_app_state();
    let now = chrono::Utc::now();
    register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register user");
    let (_, auth_session, token) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login user");

    let axum::Json(response) = super::api_list_model_routes(
        axum::extract::State(state.clone()),
        auth_headers(&token, None),
    )
    .await
    .expect("model routes response");

    assert!(response.provider_available);
    assert_eq!(
        response.default_model_id.as_deref(),
        Some("opencode-go/deepseek-v4-flash")
    );
    assert!(response.routes.is_empty());

    let axum::Json(refreshed) = super::api_refresh_model_routes(
        axum::extract::State(state),
        auth_headers(&token, Some(&auth_session.csrf_token)),
    )
    .await
    .expect("refresh model routes response");
    assert!(refreshed.routes.is_empty());
}

#[tokio::test]
async fn api_settings_round_trips_default_model_selection() {
    let state = test_app_state();
    let now = chrono::Utc::now();
    let user = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register user");
    let (_, auth_session, token) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login user");

    let axum::Json(initial) = api_get_settings(
        axum::extract::State(state.clone()),
        auth_headers(&token, None),
    )
    .await
    .expect("settings response");
    assert_eq!(initial.default_model_selection, None);
    assert_eq!(initial.default_effort, None);

    let selected = ModelSelection {
        qualified_id: "kimi-k2.6".to_string(),
    };
    let axum::Json(updated) = api_update_settings(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
        axum::Json(UpdateUserSettingsRequest {
            default_model_selection: Some(selected),
            default_agent_profile_id: None,
            default_effort: Some(AgentEffort::Heavy),
        }),
    )
    .await
    .expect("update settings");
    assert_eq!(
        updated.default_model_selection,
        Some(ModelSelection {
            qualified_id: "opencode-go/kimi-k2.6".to_string(),
        })
    );
    assert_eq!(updated.default_effort, Some(AgentEffort::Heavy));
    let stored = state
        .web_store
        .load_user(user.user_id)
        .await
        .expect("load user")
        .expect("user exists");
    assert_eq!(
        stored.default_model_selection,
        updated.default_model_selection
    );
    assert_eq!(stored.default_effort, updated.default_effort);

    let axum::Json(updated_zen) = api_update_settings(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
        axum::Json(UpdateUserSettingsRequest {
            default_model_selection: Some(ModelSelection {
                qualified_id: "opencode-zen/deepseek-v4-flash-free".to_string(),
            }),
            default_agent_profile_id: None,
            default_effort: None,
        }),
    )
    .await
    .expect("update zen settings");
    assert_eq!(
        updated_zen.default_model_selection,
        Some(ModelSelection {
            qualified_id: "opencode-zen/deepseek-v4-flash-free".to_string(),
        })
    );

    let (status, axum::Json(error)) = api_update_settings(
        axum::extract::State(state),
        auth_headers(&token, Some(&auth_session.csrf_token)),
        axum::Json(UpdateUserSettingsRequest {
            default_model_selection: Some(ModelSelection {
                qualified_id: "other-provider/model".to_string(),
            }),
            default_agent_profile_id: None,
            default_effort: None,
        }),
    )
    .await
    .expect_err("non-opencode model selection should fail");
    assert_eq!(status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error.error.code, ErrorCode::ValidationError);
}

#[tokio::test]
async fn api_create_session_persists_request_user_default_and_fallback_model_selection() {
    let state = test_app_state();
    let now = chrono::Utc::now();
    register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register user");
    let (user, auth_session, token) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login user");

    let axum::Json(fallback_created) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
    )
    .await
    .expect("create fallback session");
    let fallback_record = state
        .web_store
        .load_session(user.user_id, &fallback_created.session.session_id)
        .await
        .expect("load fallback session")
        .expect("fallback session exists");
    assert_eq!(
        fallback_record.model_selection,
        Some(ModelSelection {
            qualified_id: "opencode-go/deepseek-v4-flash".to_string(),
        })
    );

    let axum::Json(_) = api_update_settings(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
        axum::Json(UpdateUserSettingsRequest {
            default_model_selection: Some(ModelSelection {
                qualified_id: "opencode-go/kimi-k2.6".to_string(),
            }),
            default_agent_profile_id: None,
            default_effort: None,
        }),
    )
    .await
    .expect("save user default");
    let axum::Json(default_created) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
    )
    .await
    .expect("create default session");
    let default_record = state
        .web_store
        .load_session(user.user_id, &default_created.session.session_id)
        .await
        .expect("load default session")
        .expect("default session exists");
    assert_eq!(
        default_record.model_selection,
        Some(ModelSelection {
            qualified_id: "opencode-go/kimi-k2.6".to_string(),
        })
    );

    let axum::Json(request_created) = api_create_session_with_request(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
        axum::Json(ApiCreateSessionRequest {
            model_selection: Some(ModelSelection {
                qualified_id: "glm-5".to_string(),
            }),
            agent_profile_selection: AgentProfileSelection::Default,
        }),
    )
    .await
    .expect("create request-selected session");
    let request_record = state
        .web_store
        .load_session(user.user_id, &request_created.session.session_id)
        .await
        .expect("load request session")
        .expect("request session exists");
    assert_eq!(
        request_record.model_selection,
        Some(ModelSelection {
            qualified_id: "opencode-go/glm-5".to_string(),
        })
    );
}

#[tokio::test]
async fn api_agent_profile_default_and_session_selection_persist() {
    let state = test_app_state();
    let now = chrono::Utc::now();
    register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register user");
    let (user, auth_session, token) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login user");

    let axum::Json(created_profile) = api_create_agent_profile(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
        axum::Json(CreateAgentProfileRequest {
            display_name: "Reviewer".to_string(),
            system_prompt: "Focus on review notes.".to_string(),
        }),
    )
    .await
    .expect("create agent profile");
    assert_eq!(created_profile.profile.display_name, "Reviewer");

    let _ = api_update_settings(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
        axum::Json(UpdateUserSettingsRequest {
            default_model_selection: None,
            default_agent_profile_id: Some(created_profile.profile.agent_id.clone()),
            default_effort: None,
        }),
    )
    .await
    .expect("save default profile");

    let axum::Json(default_created) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
    )
    .await
    .expect("create default-profile session");
    let default_record = state
        .web_store
        .load_session(user.user_id, &default_created.session.session_id)
        .await
        .expect("load default-profile session")
        .expect("session exists");
    assert_eq!(
        default_record.agent_profile_id,
        Some(created_profile.profile.agent_id.clone())
    );

    let axum::Json(explicit_created) = api_create_session_with_request(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
        axum::Json(ApiCreateSessionRequest {
            model_selection: None,
            agent_profile_selection: AgentProfileSelection::None,
        }),
    )
    .await
    .expect("create no-profile session");
    let explicit_record = state
        .web_store
        .load_session(user.user_id, &explicit_created.session.session_id)
        .await
        .expect("load no-profile session")
        .expect("session exists");
    assert_eq!(explicit_record.agent_profile_id, None);

    let axum::Json(updated_session) = api_update_session_profile(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
        axum::extract::Path(explicit_created.session.session_id.clone()),
        axum::Json(UpdateSessionProfileRequest {
            agent_profile_id: Some(created_profile.profile.agent_id.clone()),
        }),
    )
    .await
    .expect("select profile for existing session");
    assert_eq!(
        updated_session.session.agent_profile_id,
        Some(created_profile.profile.agent_id.clone())
    );
}

#[tokio::test]
async fn startup_guard_requires_explicit_in_memory_for_web_enabled_mode() {
    let _lock = web_env_mutex().lock().await;
    let _guard = EnvGuard::capture(&[
        "RUN_MODE",
        "OXIDE_WEB_ENABLED",
        "OXIDE_WEB_REQUIRE_DURABLE_STORAGE",
        "OXIDE_WEB_ALLOW_IN_MEMORY_STORE",
    ]);
    std::env::remove_var("RUN_MODE");
    std::env::set_var("OXIDE_WEB_ENABLED", "true");
    std::env::remove_var("OXIDE_WEB_REQUIRE_DURABLE_STORAGE");
    std::env::remove_var("OXIDE_WEB_ALLOW_IN_MEMORY_STORE");

    let state = test_app_state();
    assert_eq!(
        state.validate_web_store_for_startup(),
        Err(WebStartupError::InMemoryStoreNotAllowed)
    );

    std::env::set_var("OXIDE_WEB_ALLOW_IN_MEMORY_STORE", "true");
    assert!(state.validate_web_store_for_startup().is_ok());
}

#[tokio::test]
async fn static_assets_startup_requires_index_when_configured() {
    let _lock = web_env_mutex().lock().await;
    let _guard = EnvGuard::capture(&["OXIDE_WEB_ALLOW_IN_MEMORY_STORE"]);
    std::env::set_var("OXIDE_WEB_ALLOW_IN_MEMORY_STORE", "true");

    let asset_dir = unique_test_asset_dir("missing-index");
    std::fs::create_dir_all(&asset_dir).expect("create asset dir");
    let mut state = test_app_state();
    state.web_assets = WebAssetsConfig::required_dir_for_tests(asset_dir.clone());

    let error = state
        .validate_web_store_for_startup()
        .expect_err("missing index should fail startup");
    assert!(matches!(error, WebStartupError::StaticAssetsUnavailable(_)));

    std::fs::write(asset_dir.join("index.html"), "<html>ok</html>").expect("write index");
    assert!(state.validate_web_store_for_startup().is_ok());
    let _ = std::fs::remove_dir_all(asset_dir);
}

#[tokio::test]
async fn router_serves_frontend_assets_and_security_headers() {
    use tower::Service as _;

    let asset_dir = unique_test_asset_dir("static-serving");
    std::fs::create_dir_all(&asset_dir).expect("create asset dir");
    std::fs::write(asset_dir.join("index.html"), "<main id=\"app\"></main>").expect("write index");
    std::fs::write(asset_dir.join("oxide.js"), "console.log('oxide')").expect("write js");
    std::fs::write(asset_dir.join("oxide.wasm"), [0_u8, 97, 115, 109]).expect("write wasm");

    let mut state = test_app_state();
    state.web_assets = WebAssetsConfig {
        dir: Some(asset_dir.clone()),
        required: false,
    };

    let mut app = super::build_router(state.clone());
    let response = app
        .call(
            axum::http::Request::builder()
                .method(axum::http::Method::GET)
                .uri("/app/session/session-1")
                .body(axum::body::Body::empty())
                .expect("browser route request"),
        )
        .await
        .expect("browser route response");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        response.headers()["x-content-type-options"],
        axum::http::HeaderValue::from_static("nosniff")
    );
    assert_eq!(
        response.headers()["x-frame-options"],
        axum::http::HeaderValue::from_static("DENY")
    );
    let csp = response
        .headers()
        .get("content-security-policy")
        .expect("content security policy");
    assert!(csp
        .to_str()
        .expect("valid csp")
        .contains("script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'"));
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("browser route body");
    assert!(String::from_utf8_lossy(&body).contains("app"));

    let mut app = super::build_router(state);
    let response = app
        .call(
            axum::http::Request::builder()
                .method(axum::http::Method::GET)
                .uri("/oxide.wasm")
                .body(axum::body::Body::empty())
                .expect("wasm request"),
        )
        .await
        .expect("wasm response");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_TYPE],
        axum::http::HeaderValue::from_static("application/wasm")
    );

    let mut app = super::build_router(test_app_state());
    let response = app
        .call(
            axum::http::Request::builder()
                .method(axum::http::Method::GET)
                .uri("/api/v1/does-not-exist")
                .body(axum::body::Body::empty())
                .expect("missing api request"),
        )
        .await
        .expect("missing api response");
    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);

    let _ = std::fs::remove_dir_all(asset_dir);
}

#[cfg(feature = "storage-sqlx")]
#[tokio::test]
async fn sqlx_backed_app_state_builder_requires_database_config() {
    let _lock = web_env_mutex().lock().await;
    let _guard = EnvGuard::capture(&["OXIDE_DATABASE_URL", "DATABASE_URL"]);
    std::env::remove_var("OXIDE_DATABASE_URL");
    std::env::remove_var("DATABASE_URL");

    let settings = Arc::new(AgentSettings::default());
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let Err(error) =
        super::build_sqlx_backed_app_state(SessionRegistry::new(), llm, settings).await
    else {
        panic!("missing database config should fail before startup");
    };
    assert!(
        error.to_string().contains("OXIDE_DATABASE_URL"),
        "unexpected startup error: {error}"
    );
}

#[cfg(feature = "storage-sqlx")]
#[tokio::test]
async fn sqlx_backed_app_state_uses_sqlx_store_when_database_configured() {
    let Some(database_url) = std::env::var("OXIDE_DATABASE_TEST_URL").ok() else {
        eprintln!("skipping SQLx startup smoke: OXIDE_DATABASE_TEST_URL is not set");
        return;
    };

    let _lock = web_env_mutex().lock().await;
    let _guard = EnvGuard::capture(&[
        "OXIDE_DATABASE_URL",
        "DATABASE_URL",
        "OXIDE_DATABASE_MIGRATE_ON_STARTUP",
        "OXIDE_DATABASE_MIGRATIONS_DIR",
        "OXIDE_WEB_REQUIRE_STATIC_ASSETS",
    ]);
    std::env::set_var("OXIDE_DATABASE_URL", database_url);
    std::env::remove_var("DATABASE_URL");
    std::env::set_var("OXIDE_DATABASE_MIGRATE_ON_STARTUP", "true");
    std::env::set_var(
        "OXIDE_DATABASE_MIGRATIONS_DIR",
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("migrations"),
    );
    std::env::remove_var("OXIDE_WEB_REQUIRE_STATIC_ASSETS");

    let settings = Arc::new(AgentSettings::default());
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let state = super::build_sqlx_backed_app_state(SessionRegistry::new(), llm, settings)
        .await
        .expect("SQLx-backed app state should build from database config");

    assert_eq!(state.web_store_kind(), WebStoreKind::Sqlx);
}

#[tokio::test]
async fn router_exposes_api_v1_without_legacy_unversioned_routes() {
    use tower::Service as _;

    let state = test_app_state();
    let mut app = super::build_router(state.clone());
    let public_config = app
        .call(
            axum::http::Request::builder()
                .method(axum::http::Method::GET)
                .uri("/api/v1/public-config")
                .body(axum::body::Body::empty())
                .expect("public-config request"),
        )
        .await
        .expect("public-config response");
    assert_eq!(public_config.status(), axum::http::StatusCode::OK);

    let legacy_root = format!("{}{}", "/session", "s");
    let debug_logs_path = format!("{}{}", "/debug", "/event_logs");
    for (method, path) in [
        (axum::http::Method::POST, legacy_root.clone()),
        (axum::http::Method::GET, format!("{legacy_root}/session-1")),
        (
            axum::http::Method::DELETE,
            format!("{legacy_root}/session-1"),
        ),
        (
            axum::http::Method::POST,
            format!("{legacy_root}/session-1/tasks"),
        ),
        (
            axum::http::Method::GET,
            format!("{legacy_root}/session-1/tasks/task-1/progress"),
        ),
        (
            axum::http::Method::GET,
            format!("{legacy_root}/session-1/tasks/task-1/events"),
        ),
        (
            axum::http::Method::GET,
            format!("{legacy_root}/session-1/tasks/task-1/stream"),
        ),
        (
            axum::http::Method::GET,
            format!("{legacy_root}/session-1/tasks/task-1/timeline"),
        ),
        (
            axum::http::Method::POST,
            format!("{legacy_root}/session-1/tasks/task-1/cancel"),
        ),
        (axum::http::Method::GET, debug_logs_path),
    ] {
        let response = super::build_router(state.clone())
            .call(
                axum::http::Request::builder()
                    .method(method)
                    .uri(path.as_str())
                    .body(axum::body::Body::empty())
                    .expect("legacy route request"),
            )
            .await
            .expect("legacy route response");
        assert_eq!(
            response.status(),
            axum::http::StatusCode::NOT_FOUND,
            "legacy route {path} should not be exposed"
        );
    }
}

#[test]
fn sse_start_seq_uses_query_before_last_event_id() {
    let mut headers = HeaderMap::new();
    headers.insert("last-event-id", "41".parse().expect("last-event-id"));

    assert_eq!(
        super::sse::sse_start_seq(
            &headers,
            &TaskEventsQuery {
                after_seq: None,
                limit: None,
            },
        ),
        41
    );
    assert_eq!(
        super::sse::sse_start_seq(
            &headers,
            &TaskEventsQuery {
                after_seq: Some(9),
                limit: None,
            },
        ),
        9
    );
}

#[tokio::test]
async fn api_sessions_are_auth_scoped_and_use_web_session_context() {
    let (state, sandbox_control) =
        test_app_state_with_responses(vec![ScriptedResponse::Text("ok".to_string())]);
    let now = chrono::Utc::now();
    let user_one = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register first user");
    let user_two = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "bob".to_string(),
            password: "another correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register second user");
    let (_, session_one, token_one) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login first user");
    let (_, _, token_two) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "bob".to_string(),
            password: "another correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login second user");

    let axum::Json(created) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, Some(&session_one.csrf_token)),
    )
    .await
    .expect("create session");
    let session_id = created.session.session_id;
    let record = state
        .web_store
        .load_session(user_one.user_id, &session_id)
        .await
        .expect("load session")
        .expect("session exists");
    assert_eq!(record.context_key, format!("web-session-{session_id}"));
    assert_eq!(record.agent_flow_id, "main");
    assert_eq!(sandbox_control.ensured_scopes().len(), 1);
    assert_eq!(
        sandbox_control.ensured_scopes()[0].namespace(),
        record.context_key,
        "web session sandbox should be scoped per session context"
    );

    let axum::Json(listed) = api_list_sessions(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, None),
    )
    .await
    .expect("list sessions");
    assert_eq!(listed.sessions.len(), 1);

    let axum::Json(foreign_listed) = api_list_sessions(
        axum::extract::State(state.clone()),
        auth_headers(&token_two, None),
    )
    .await
    .expect("list foreign sessions");
    assert!(foreign_listed.sessions.is_empty());

    let foreign_get = api_get_session(
        axum::extract::State(state.clone()),
        auth_headers(&token_two, None),
        axum::extract::Path(session_id.clone()),
    )
    .await;
    assert_eq!(
        foreign_get.expect_err("foreign session should be hidden").0,
        axum::http::StatusCode::NOT_FOUND
    );

    let create_without_csrf =
        api_create_session(axum::extract::State(state), auth_headers(&token_one, None)).await;
    assert_eq!(
        create_without_csrf.expect_err("missing csrf should fail").0,
        axum::http::StatusCode::FORBIDDEN
    );
    assert_ne!(user_one.user_id, user_two.user_id);
}

#[tokio::test]
async fn api_download_task_file_serves_owned_file_and_supports_inline_preview() {
    use tower::Service as _;

    let state = test_app_state();
    let now = chrono::Utc::now();
    let user = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register user");
    let (_, auth_session, token) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login user");

    let axum::Json(created_session) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
    )
    .await
    .expect("create session");
    let session_id = created_session.session.session_id;
    let task_id = "task-download".to_string();
    state
        .web_store
        .save_task(task_record(
            user.user_id,
            &session_id,
            &task_id,
            ApiTaskStatus::Completed,
            "Deliver a file",
            now,
        ))
        .await
        .expect("save task");

    let file_id = "file-1".to_string();
    state
        .web_store
        .save_task_file(
            WebTaskFileRecord {
                schema_version: WEB_TASK_FILE_SCHEMA_VERSION,
                user_id: user.user_id,
                session_id: session_id.clone(),
                task_id: task_id.clone(),
                file_id: file_id.clone(),
                file_name: "report\n2026.pdf".to_string(),
                content_type: "application/pdf".to_string(),
                size_bytes: 7,
                delivery_kind: FileDeliveryKind::Document,
                created_at: now,
            },
            b"pdf-ish".to_vec(),
        )
        .await
        .expect("save task file");

    let mut app = super::build_router(state.clone());
    let response = app
        .call(
            axum::http::Request::builder()
                .method(axum::http::Method::GET)
                .uri(format!(
                    "/api/v1/sessions/{session_id}/tasks/{task_id}/files/{file_id}"
                ))
                .header(
                    axum::http::header::COOKIE,
                    format!("{AUTH_COOKIE_NAME}={token}"),
                )
                .body(axum::body::Body::empty())
                .expect("download request"),
        )
        .await
        .expect("download response");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        response.headers()[axum::http::header::CACHE_CONTROL],
        axum::http::HeaderValue::from_static("private, no-store")
    );
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_TYPE],
        axum::http::HeaderValue::from_static("application/pdf")
    );
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_LENGTH],
        axum::http::HeaderValue::from_static("7")
    );
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_DISPOSITION],
        axum::http::HeaderValue::from_static("attachment; filename=\"report_2026.pdf\"")
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read download body");
    assert_eq!(body.as_ref(), b"pdf-ish");

    let response = app
            .call(
                axum::http::Request::builder()
                    .method(axum::http::Method::GET)
                    .uri(format!(
                        "/api/v1/sessions/{session_id}/tasks/{task_id}/files/{file_id}?disposition=inline"
                    ))
                    .header(
                        axum::http::header::COOKIE,
                        format!("{AUTH_COOKIE_NAME}={token}"),
                    )
                    .body(axum::body::Body::empty())
                    .expect("inline preview request"),
            )
            .await
            .expect("inline preview response");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_DISPOSITION],
        axum::http::HeaderValue::from_static("inline; filename=\"report_2026.pdf\"")
    );
}

#[tokio::test]
async fn api_download_task_file_hides_foreign_or_missing_files() {
    use tower::Service as _;

    let state = test_app_state();
    let now = chrono::Utc::now();
    let owner = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register owner");
    register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "bob".to_string(),
            password: "another correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register second user");
    let (_, owner_session, owner_token) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login owner");
    let (_, _, foreign_token) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "bob".to_string(),
            password: "another correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login foreign user");

    let axum::Json(created_session) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&owner_token, Some(&owner_session.csrf_token)),
    )
    .await
    .expect("create session");
    let session_id = created_session.session.session_id;
    let task_id = "task-download".to_string();
    state
        .web_store
        .save_task(task_record(
            owner.user_id,
            &session_id,
            &task_id,
            ApiTaskStatus::Completed,
            "Deliver a file",
            now,
        ))
        .await
        .expect("save task");
    state
        .web_store
        .save_task_file(
            WebTaskFileRecord {
                schema_version: WEB_TASK_FILE_SCHEMA_VERSION,
                user_id: owner.user_id,
                session_id: session_id.clone(),
                task_id: task_id.clone(),
                file_id: "file-1".to_string(),
                file_name: "report.pdf".to_string(),
                content_type: "application/pdf".to_string(),
                size_bytes: 7,
                delivery_kind: FileDeliveryKind::Document,
                created_at: now,
            },
            b"pdf-ish".to_vec(),
        )
        .await
        .expect("save task file");

    let mut app = super::build_router(state);
    let foreign_response = app
        .call(
            axum::http::Request::builder()
                .method(axum::http::Method::GET)
                .uri(format!(
                    "/api/v1/sessions/{session_id}/tasks/{task_id}/files/file-1"
                ))
                .header(
                    axum::http::header::COOKIE,
                    format!("{AUTH_COOKIE_NAME}={foreign_token}"),
                )
                .body(axum::body::Body::empty())
                .expect("foreign request"),
        )
        .await
        .expect("foreign response");
    assert_eq!(foreign_response.status(), axum::http::StatusCode::NOT_FOUND);

    let missing_response = app
        .call(
            axum::http::Request::builder()
                .method(axum::http::Method::GET)
                .uri(format!(
                    "/api/v1/sessions/{session_id}/tasks/{task_id}/files/missing"
                ))
                .header(
                    axum::http::header::COOKIE,
                    format!("{AUTH_COOKIE_NAME}={owner_token}"),
                )
                .body(axum::body::Body::empty())
                .expect("missing file request"),
        )
        .await
        .expect("missing file response");
    assert_eq!(missing_response.status(), axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_create_session_prunes_orphan_web_sandboxes() {
    let (mut state, _) =
        test_app_state_with_responses(vec![ScriptedResponse::Text("ok".to_string())]);
    let now = chrono::Utc::now();
    let user = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register user");
    let (_, auth_session, token) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login user");
    let sandbox_control = FakeSandboxControl::with_sandboxes(vec![
        fake_sandbox_record(SandboxScope::new(user.user_id, "web")),
        fake_sandbox_record(SandboxScope::new(user.user_id, "web-session-orphan")),
        fake_sandbox_record(SandboxScope::new(user.user_id, "topic-live")),
    ]);
    state.set_sandbox_control(Arc::new(sandbox_control.clone()));

    let axum::Json(created) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
    )
    .await
    .expect("create session");

    let deleted_names = sandbox_control.deleted_names();
    assert!(deleted_names
        .iter()
        .any(|name| name == &SandboxScope::new(user.user_id, "web").container_name()));
    assert!(deleted_names.iter().any(|name| {
        name == &SandboxScope::new(user.user_id, "web-session-orphan").container_name()
    }));
    assert!(!deleted_names.iter().any(|name| {
        name == &SandboxScope::new(
            user.user_id,
            format!("web-session-{}", created.session.session_id),
        )
        .container_name()
    }));
}

#[tokio::test]
async fn api_delete_session_destroys_web_sandbox_and_clears_flow_memory() {
    let (mut state, _) =
        test_app_state_with_responses(vec![ScriptedResponse::Text("ok".to_string())]);
    let sandbox_control = FakeSandboxControl::default();
    state.set_sandbox_control(Arc::new(sandbox_control.clone()));
    let now = chrono::Utc::now();
    let user = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register user");
    let (_, auth_session, token) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login user");

    let axum::Json(created) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
    )
    .await
    .expect("create session");
    let record = state
        .web_store
        .load_session(user.user_id, &created.session.session_id)
        .await
        .expect("load session")
        .expect("session exists");
    let memory = AgentMemory::new(usize::MAX);
    state
        .session_manager
        .storage()
        .save_agent_memory_for_flow(
            user.user_id,
            record.context_key.clone(),
            record.agent_flow_id.clone(),
            &memory,
        )
        .await
        .expect("save flow memory");

    let axum::Json(response) = api_delete_session(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
        axum::extract::Path(created.session.session_id.clone()),
    )
    .await
    .expect("delete session");

    assert!(response.ok);
    assert!(state
        .web_store
        .load_session(user.user_id, &created.session.session_id)
        .await
        .expect("load deleted session")
        .is_none());
    assert!(state
        .session_manager
        .storage()
        .load_agent_memory_for_flow(
            user.user_id,
            record.context_key.clone(),
            record.agent_flow_id.clone(),
        )
        .await
        .expect("load flow memory")
        .is_none());
    assert_eq!(sandbox_control.destroyed_scopes().len(), 1);
    assert_eq!(
        sandbox_control.destroyed_scopes()[0].namespace(),
        record.context_key,
        "delete session should destroy the per-session sandbox"
    );
}

#[tokio::test]
async fn api_create_task_version_and_cancel_task_are_auth_scoped_and_status_checked() {
    let state = test_app_state();
    let now = chrono::Utc::now();
    let user_one = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register first user");
    register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "bob".to_string(),
            password: "another correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register second user");
    let (_, session_one, token_one) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login first user");
    let (_, session_two, token_two) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "bob".to_string(),
            password: "another correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login second user");

    let axum::Json(created_session) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, Some(&session_one.csrf_token)),
    )
    .await
    .expect("create session");
    let session_id = created_session.session.session_id;
    let original_context_key = state
        .web_store
        .load_session(user_one.user_id, &session_id)
        .await
        .expect("load session")
        .expect("session exists")
        .context_key;

    let completed = task_record(
        user_one.user_id,
        &session_id,
        "task-completed",
        ApiTaskStatus::Completed,
        "Original prompt",
        now,
    );
    state
        .web_store
        .save_task(completed)
        .await
        .expect("save completed task");

    let axum::Json(versioned) = api_create_task_version(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, Some(&session_one.csrf_token)),
        axum::extract::Path((session_id.clone(), "task-completed".to_string())),
        axum::Json(ApiCreateTaskVersionRequest {
            input_markdown: "Edited prompt".to_string(),
            attachments: Vec::new(),
            effort: None,
        }),
    )
    .await
    .expect("create task version");
    assert_eq!(versioned.task.input_markdown, "Edited prompt");
    assert!(versioned.task.input_edited_at.is_some());
    assert_eq!(versioned.task.version_group_id, "task-completed");
    assert_eq!(versioned.task.version_index, 2);
    assert_eq!(
        versioned.task.parent_task_id.as_deref(),
        Some("task-completed")
    );
    assert_ne!(versioned.task.task_id, "task-completed");
    let edited_session = state
        .web_store
        .load_session(user_one.user_id, &session_id)
        .await
        .expect("load edited session")
        .expect("edited session exists");
    assert_ne!(edited_session.context_key, original_context_key);
    assert!(edited_session
        .context_key
        .starts_with(&format!("web-session-{session_id}-branch-")));

    let original = state
        .web_store
        .load_task(user_one.user_id, &session_id, "task-completed")
        .await
        .expect("load original task")
        .expect("original task exists");
    assert_eq!(original.input_markdown, "Original prompt");
    assert!(original.input_edited_at.is_none());

    let running = task_record(
        user_one.user_id,
        &session_id,
        "task-running",
        ApiTaskStatus::Running,
        "Running prompt",
        now + chrono::Duration::seconds(1),
    );
    state
        .web_store
        .save_task(running)
        .await
        .expect("save running task");
    let mut session = state
        .web_store
        .load_session(user_one.user_id, &session_id)
        .await
        .expect("load session")
        .expect("session exists");
    session.active_task_id = Some("task-running".to_string());
    session.last_task_status = Some(ApiTaskStatus::Running);
    state
        .web_store
        .save_session(session)
        .await
        .expect("save active session");

    let edit_running = api_create_task_version(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, Some(&session_one.csrf_token)),
        axum::extract::Path((session_id.clone(), "task-running".to_string())),
        axum::Json(ApiCreateTaskVersionRequest {
            input_markdown: "Should fail".to_string(),
            attachments: Vec::new(),
            effort: None,
        }),
    )
    .await;
    let (status, axum::Json(error)) = edit_running.expect_err("running edit should fail");
    assert_eq!(status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.error.code, ErrorCode::TaskActive);

    let edit_non_latest = api_create_task_version(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, Some(&session_one.csrf_token)),
        axum::extract::Path((session_id.clone(), "task-completed".to_string())),
        axum::Json(ApiCreateTaskVersionRequest {
            input_markdown: "Should also fail".to_string(),
            attachments: Vec::new(),
            effort: None,
        }),
    )
    .await;
    let (status, axum::Json(error)) = edit_non_latest.expect_err("non-latest edit should fail");
    assert_eq!(status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.error.code, ErrorCode::Conflict);

    let foreign_cancel = api_cancel_task(
        axum::extract::State(state.clone()),
        auth_headers(&token_two, Some(&session_two.csrf_token)),
        axum::extract::Path((session_id.clone(), "task-running".to_string())),
    )
    .await;
    assert_eq!(
        foreign_cancel.expect_err("foreign task should be hidden").0,
        axum::http::StatusCode::NOT_FOUND
    );

    let axum::Json(cancelled) = api_cancel_task(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, Some(&session_one.csrf_token)),
        axum::extract::Path((session_id.clone(), "task-running".to_string())),
    )
    .await
    .expect("cancel active task");
    assert!(cancelled.ok);
    assert_eq!(cancelled.status, ApiTaskStatus::Cancelled);

    let task = state
        .web_store
        .load_task(user_one.user_id, &session_id, "task-running")
        .await
        .expect("load task")
        .expect("task exists");
    assert_eq!(task.status, ApiTaskStatus::Cancelled);
    assert!(task.finished_at.is_some());

    let session = state
        .web_store
        .load_session(user_one.user_id, &session_id)
        .await
        .expect("load session")
        .expect("session exists");
    assert_eq!(session.active_task_id, None);
    assert_eq!(session.last_task_status, Some(ApiTaskStatus::Cancelled));

    let axum::Json(cancelled_again) = api_cancel_task(
        axum::extract::State(state),
        auth_headers(&token_one, Some(&session_one.csrf_token)),
        axum::extract::Path((session_id, "task-running".to_string())),
    )
    .await
    .expect("cancel is idempotent");
    assert!(cancelled_again.ok);
    assert_eq!(cancelled_again.status, ApiTaskStatus::Cancelled);
}

#[tokio::test]
async fn api_delete_session_clears_all_edit_branch_memory_scopes() {
    let (state, sandbox_control) =
        test_app_state_with_responses(vec![ScriptedResponse::Text("branch answer".to_string())]);
    let now = chrono::Utc::now();
    let user = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register user");
    let (_, auth_session, token) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login user");

    let axum::Json(created_session) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
    )
    .await
    .expect("create session");
    let session_id = created_session.session.session_id;
    let original_session = state
        .web_store
        .load_session(user.user_id, &session_id)
        .await
        .expect("load session")
        .expect("session exists");
    let original_context_key = original_session.context_key.clone();
    let flow_id = original_session.agent_flow_id.clone();
    let memory = AgentMemory::new(usize::MAX);
    state
        .session_manager
        .storage()
        .save_agent_memory_for_flow(
            user.user_id,
            original_context_key.clone(),
            flow_id.clone(),
            &memory,
        )
        .await
        .expect("save original memory");

    let completed = task_record(
        user.user_id,
        &session_id,
        "task-completed",
        ApiTaskStatus::Completed,
        "Original prompt",
        now,
    );
    state
        .web_store
        .save_task(completed)
        .await
        .expect("save completed task");

    let axum::Json(_versioned) = api_create_task_version(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
        axum::extract::Path((session_id.clone(), "task-completed".to_string())),
        axum::Json(ApiCreateTaskVersionRequest {
            input_markdown: "Edited prompt".to_string(),
            attachments: Vec::new(),
            effort: None,
        }),
    )
    .await
    .expect("create task version");

    let edited_session = state
        .web_store
        .load_session(user.user_id, &session_id)
        .await
        .expect("load edited session")
        .expect("edited session exists");
    let branch_context_key = edited_session.context_key.clone();
    assert_ne!(branch_context_key, original_context_key);
    state
        .session_manager
        .storage()
        .save_agent_memory_for_flow(
            user.user_id,
            branch_context_key.clone(),
            flow_id.clone(),
            &memory,
        )
        .await
        .expect("save branch memory");

    let axum::Json(deleted) = api_delete_session(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
        axum::extract::Path(session_id.clone()),
    )
    .await
    .expect("delete session");
    assert!(deleted.ok);

    for context_key in [&original_context_key, &branch_context_key] {
        assert!(
            state
                .session_manager
                .storage()
                .load_agent_memory_for_flow(user.user_id, context_key.clone(), flow_id.clone())
                .await
                .expect("load memory")
                .is_none(),
            "delete session should clear flow memory for context {context_key}"
        );
    }
    let destroyed_names = sandbox_control
        .destroyed_scopes()
        .into_iter()
        .map(|scope| scope.namespace().to_string())
        .collect::<Vec<_>>();
    assert!(destroyed_names.contains(&original_context_key));
    assert!(destroyed_names.contains(&branch_context_key));
}

#[tokio::test]
async fn api_task_events_are_auth_scoped_and_replay_after_seq() {
    let state = test_app_state();
    let now = chrono::Utc::now();
    let user_one = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register first user");
    register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "bob".to_string(),
            password: "another correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register second user");
    let (_, session_one, token_one) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login first user");
    let (_, _, token_two) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "bob".to_string(),
            password: "another correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login second user");

    let axum::Json(created_session) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, Some(&session_one.csrf_token)),
    )
    .await
    .expect("create session");
    let session_id = created_session.session.session_id;
    let task = task_record(
        user_one.user_id,
        &session_id,
        "task-events",
        ApiTaskStatus::Completed,
        "Prompt",
        now,
    );
    state.web_store.save_task(task).await.expect("save task");
    state
        .web_store
        .append_task_events(
            user_one.user_id,
            &session_id,
            "task-events",
            vec![
                persisted_event(
                    user_one.user_id,
                    &session_id,
                    "task-events",
                    1,
                    TaskEventKind::Thinking,
                ),
                persisted_event(
                    user_one.user_id,
                    &session_id,
                    "task-events",
                    2,
                    TaskEventKind::ToolResult,
                ),
            ],
        )
        .await
        .expect("append events");

    let axum::Json(response) = api_get_task_events(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, None),
        axum::extract::Path((session_id.clone(), "task-events".to_string())),
        axum::extract::Query(TaskEventsQuery {
            after_seq: Some(1),
            limit: Some(1),
        }),
    )
    .await
    .expect("get task events");
    assert_eq!(response.events.len(), 1);
    assert_eq!(response.events[0].seq, 2);
    assert_eq!(response.events[0].kind, TaskEventKind::ToolResult);
    assert_eq!(response.last_seq, 2);
    assert!(!response.has_more);

    let foreign = api_get_task_events(
        axum::extract::State(state),
        auth_headers(&token_two, None),
        axum::extract::Path((session_id, "task-events".to_string())),
        axum::extract::Query(TaskEventsQuery {
            after_seq: Some(0),
            limit: Some(200),
        }),
    )
    .await;
    assert_eq!(
        foreign.expect_err("foreign events should be hidden").0,
        axum::http::StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn api_task_progress_is_auth_scoped_and_reads_persisted_snapshot() {
    let state = test_app_state();
    let now = chrono::Utc::now();
    let user_one = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register first user");
    register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "bob".to_string(),
            password: "another correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register second user");
    let (_, session_one, token_one) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login first user");
    let (_, _, token_two) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "bob".to_string(),
            password: "another correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login second user");

    let axum::Json(created_session) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, Some(&session_one.csrf_token)),
    )
    .await
    .expect("create session");
    let session_id = created_session.session.session_id;
    let mut task = task_record(
        user_one.user_id,
        &session_id,
        "task-progress",
        ApiTaskStatus::Running,
        "Prompt",
        now,
    );
    task.last_event_seq = 7;
    task.last_progress = Some(ProgressSnapshot {
        current_iteration: 3,
        max_iterations: 100,
        is_finished: false,
        error: None,
        current_thought: Some("Collecting evidence".to_string()),
        current_todos: Some(serde_json::json!({ "items": [] })),
        last_compaction_status: Some("Compaction: compacted history".to_string()),
        repeated_compaction_warning: None,
        last_history_repair_status: Some("History repaired".to_string()),
        latest_token_snapshot: None,
        llm_retry: Some(serde_json::json!({ "attempt": 2 })),
        provider_failover_notice: Some("Failover: primary -> backup".to_string()),
    });
    state.web_store.save_task(task).await.expect("save task");

    let axum::Json(response) = api_get_task_progress(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, None),
        axum::extract::Path((session_id.clone(), "task-progress".to_string())),
    )
    .await
    .expect("get persisted task progress");
    let progress = response.progress.expect("progress snapshot");
    assert_eq!(response.status, ApiTaskStatus::Running);
    assert_eq!(response.last_event_seq, 7);
    assert_eq!(progress.current_iteration, 3);
    assert_eq!(
        progress.current_todos.expect("todos snapshot")["items"],
        serde_json::json!([])
    );
    assert_eq!(progress.llm_retry.expect("retry snapshot")["attempt"], 2);
    assert_eq!(
        progress.provider_failover_notice.as_deref(),
        Some("Failover: primary -> backup")
    );

    let foreign = api_get_task_progress(
        axum::extract::State(state),
        auth_headers(&token_two, None),
        axum::extract::Path((session_id, "task-progress".to_string())),
    )
    .await;
    assert_eq!(
        foreign.expect_err("foreign progress should be hidden").0,
        axum::http::StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn live_progress_persister_updates_running_task_record() {
    let state = test_app_state();
    let now = chrono::Utc::now();
    let user_id = 77;
    let session_id = "session-live-progress";
    let task_id = "task-live-progress";
    state
        .web_store
        .save_task(task_record(
            user_id,
            session_id,
            task_id,
            ApiTaskStatus::Running,
            "Prompt",
            now,
        ))
        .await
        .expect("save running task");

    let web_task = super::task_executor::WebTaskPersistence {
        web_store: state.web_store.clone(),
        user_id,
        session_id: session_id.to_string(),
        task_id: task_id.to_string(),
    };
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = super::task_executor::spawn_live_progress_persister(web_task, rx);

    let mut progress = ProgressState::new(100);
    progress.current_iteration = 4;
    progress.current_thought = Some("Persisting progress".to_string());
    progress.current_todos = Some(TodoList {
        items: vec![TodoItem {
            description: "Persist progress".to_string(),
            status: TodoStatus::InProgress,
        }],
        updated_at: Some(now),
    });
    progress.llm_retry = Some(LlmRetryState {
        attempt: 2,
        max_attempts: 5,
        unbounded: false,
        wait_secs: Some(3),
        provider: "mock".to_string(),
        error_class: Some("rate_limit".to_string()),
    });
    progress.provider_failover_notice = Some("Failover: mock:a -> mock:b".to_string());
    tx.send(progress).expect("send live progress");

    let snapshot = wait_for_persisted_progress(&state, user_id, session_id, task_id).await;
    assert_eq!(snapshot.current_iteration, 4);
    assert_eq!(
        snapshot.current_thought.as_deref(),
        Some("Persisting progress")
    );
    assert_eq!(
        snapshot.current_todos.expect("todos persisted")["items"][0]["description"],
        "Persist progress"
    );
    assert_eq!(snapshot.llm_retry.expect("retry persisted")["attempt"], 2);
    assert_eq!(
        snapshot.provider_failover_notice.as_deref(),
        Some("Failover: mock:a -> mock:b")
    );

    drop(tx);
    handle.await.expect("live progress persister joins");
}

#[tokio::test]
async fn api_task_stream_replays_persisted_events_after_seq() {
    use tower::Service as _;

    let state = test_app_state();
    let now = chrono::Utc::now();
    let user_one = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register first user");
    register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "bob".to_string(),
            password: "another correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register second user");
    let (_, session_one, token_one) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login first user");
    let (_, _, token_two) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "bob".to_string(),
            password: "another correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login second user");

    let axum::Json(created_session) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, Some(&session_one.csrf_token)),
    )
    .await
    .expect("create session");
    let session_id = created_session.session.session_id;
    let mut task = task_record(
        user_one.user_id,
        &session_id,
        "task-events",
        ApiTaskStatus::Completed,
        "Prompt",
        now,
    );
    task.last_event_seq = 2;
    state.web_store.save_task(task).await.expect("save task");
    state
        .web_store
        .append_task_events(
            user_one.user_id,
            &session_id,
            "task-events",
            vec![
                persisted_event(
                    user_one.user_id,
                    &session_id,
                    "task-events",
                    1,
                    TaskEventKind::Thinking,
                ),
                persisted_event(
                    user_one.user_id,
                    &session_id,
                    "task-events",
                    2,
                    TaskEventKind::ToolResult,
                ),
            ],
        )
        .await
        .expect("append events");

    let mut app = super::build_router(state.clone());
    let response = app
        .call(sse_request(&session_id, "task-events", &token_one, Some(1)))
        .await
        .expect("sse response");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("sse body");
    let body = String::from_utf8(body.to_vec()).expect("sse body utf8");
    assert!(body.contains("event: snapshot"));
    assert!(body.contains("event: task_event"));
    assert!(body.contains("id: 2"));
    assert!(!body.contains("\"seq\":1"));
    assert!(body.contains("event: task_status"));
    assert!(body.contains("\"status\":\"completed\""));

    let mut app = super::build_router(state);
    let response = app
        .call(sse_request(&session_id, "task-events", &token_two, Some(0)))
        .await
        .expect("foreign sse response");
    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

#[cfg(feature = "profile-lite")]
#[tokio::test]
async fn api_tasks_are_auth_scoped_and_persist_final_response() {
    let state = test_app_state();
    let now = chrono::Utc::now();
    let user_one = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register first user");
    let _user_two = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "bob".to_string(),
            password: "another correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register second user");
    let (_, session_one, token_one) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login first user");
    let (_, _, token_two) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "bob".to_string(),
            password: "another correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login second user");

    let axum::Json(created_session) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, Some(&session_one.csrf_token)),
    )
    .await
    .expect("create session");
    let session_id = created_session.session.session_id;

    let axum::Json(created_task) = api_create_task(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, Some(&session_one.csrf_token)),
        axum::extract::Path(session_id.clone()),
        axum::Json(ApiCreateTaskRequest {
            input_markdown: "Summarize this".to_string(),
            attachments: Vec::new(),
            effort: None,
        }),
    )
    .await
    .expect("create task");
    let task_id = created_task.task.task_id;

    let completed = wait_for_task_status(
        &state,
        user_one.user_id,
        &session_id,
        &task_id,
        ApiTaskStatus::Completed,
    )
    .await;
    assert_eq!(completed.final_response_markdown.as_deref(), Some("ok"));
    assert!(completed.finished_at.is_some());
    assert!(completed.last_progress.is_some());
    assert!(completed.last_event_seq > 0);

    let axum::Json(task_events) = api_get_task_events(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, None),
        axum::extract::Path((session_id.clone(), task_id.clone())),
        axum::extract::Query(TaskEventsQuery {
            after_seq: Some(0),
            limit: Some(200),
        }),
    )
    .await
    .expect("get persisted task events");
    assert!(!task_events.events.is_empty());
    assert_eq!(task_events.last_seq, completed.last_event_seq);

    let session_record = state
        .web_store
        .load_session(user_one.user_id, &session_id)
        .await
        .expect("load session")
        .expect("session exists");
    assert_eq!(session_record.active_task_id, None);
    assert_eq!(
        session_record.last_task_status,
        Some(ApiTaskStatus::Completed)
    );

    let axum::Json(listed_tasks) = api_list_tasks(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, None),
        axum::extract::Path(session_id.clone()),
    )
    .await
    .expect("list tasks");
    assert_eq!(listed_tasks.tasks.len(), 1);
    assert_eq!(listed_tasks.tasks[0].task_id, task_id);

    let axum::Json(task_detail) = api_get_task(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, None),
        axum::extract::Path((session_id.clone(), task_id.clone())),
    )
    .await
    .expect("get task");
    assert_eq!(
        task_detail.task.final_response_markdown.as_deref(),
        Some("ok")
    );

    let foreign_get = api_get_task(
        axum::extract::State(state.clone()),
        auth_headers(&token_two, None),
        axum::extract::Path((session_id.clone(), task_id.clone())),
    )
    .await;
    assert_eq!(
        foreign_get.expect_err("foreign task should be hidden").0,
        axum::http::StatusCode::NOT_FOUND
    );

    save_active_task(&state, &completed, ApiTaskStatus::Running, None).await;
    let busy_create = api_create_task(
        axum::extract::State(state.clone()),
        auth_headers(&token_one, Some(&session_one.csrf_token)),
        axum::extract::Path(session_id.clone()),
        axum::Json(ApiCreateTaskRequest {
            input_markdown: "Second task".to_string(),
            attachments: Vec::new(),
            effort: None,
        }),
    )
    .await;
    let (status, axum::Json(error)) = busy_create.expect_err("active task should fail");
    assert_eq!(status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.error.code, ErrorCode::SessionBusy);

    save_active_task(
        &state,
        &completed,
        ApiTaskStatus::WaitingForUserInput,
        Some(PendingUserInputView {
            kind: ApiUserInputKind::Text,
            prompt: "Need more input".to_string(),
        }),
    )
    .await;
    let waiting_create = api_create_task(
        axum::extract::State(state),
        auth_headers(&token_one, Some(&session_one.csrf_token)),
        axum::extract::Path(session_id),
        axum::Json(ApiCreateTaskRequest {
            input_markdown: "Third task".to_string(),
            attachments: Vec::new(),
            effort: None,
        }),
    )
    .await;
    let (status, axum::Json(error)) =
        waiting_create.expect_err("waiting task should fail distinctly");
    assert_eq!(status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.error.code, ErrorCode::TaskWaitingForUserInput);
    assert_eq!(
        error
            .error
            .details
            .as_ref()
            .and_then(|details| details.get("task_id").and_then(serde_json::Value::as_str)),
        Some("active-waiting")
    );
}

#[cfg(feature = "profile-lite")]
#[tokio::test]
async fn api_create_task_starts_runtime_without_waiting_for_auto_title() {
    let llm = AutoTitleTestLlmProvider::blocking_title("Авторизация для сервисов", "ok");
    let (mut state, _) = test_app_state_with_llm_provider(llm.clone());
    state.auto_title_enabled = true;
    let now = chrono::Utc::now();
    let user = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register user");
    let (_, auth_session, token) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login user");

    let axum::Json(created_session) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
    )
    .await
    .expect("create session");
    let session_id = created_session.session.session_id;

    let axum::Json(created_task) = tokio::time::timeout(
        Duration::from_secs(2),
        api_create_task(
            axum::extract::State(state.clone()),
            auth_headers(&token, Some(&auth_session.csrf_token)),
            axum::extract::Path(session_id.clone()),
            axum::Json(ApiCreateTaskRequest {
                input_markdown:
                    "Какие способы авторизации лучше использовать для внутреннего сервиса?"
                        .to_string(),
                attachments: Vec::new(),
                effort: None,
            }),
        ),
    )
    .await
    .expect("create task should not wait for auto title")
    .expect("create task");

    tokio::time::timeout(Duration::from_secs(2), llm.wait_title_started())
        .await
        .expect("auto title should start in background");

    let completed = wait_for_task_status(
        &state,
        user.user_id,
        &session_id,
        &created_task.task.task_id,
        ApiTaskStatus::Completed,
    )
    .await;
    assert_eq!(completed.final_response_markdown.as_deref(), Some("ok"));

    let session_record = state
        .web_store
        .load_session(user.user_id, &session_id)
        .await
        .expect("load session")
        .expect("session exists");
    assert_eq!(session_record.title, "New session");

    llm.release_title();
    wait_for_session_title(
        &state,
        user.user_id,
        &session_id,
        "Авторизация для сервисов",
    )
    .await;
}

#[cfg(feature = "profile-lite")]
#[tokio::test]
async fn api_create_task_does_not_store_preview_as_title_when_auto_title_is_empty() {
    let llm = AutoTitleTestLlmProvider::new("   ", "ok");
    let (mut state, _) = test_app_state_with_llm_provider(llm.clone());
    state.auto_title_enabled = true;
    let now = chrono::Utc::now();
    let user = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register user");
    let (_, auth_session, token) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login user");

    let axum::Json(created_session) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
    )
    .await
    .expect("create session");
    let session_id = created_session.session.session_id;

    let axum::Json(created_task) = api_create_task(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
        axum::extract::Path(session_id.clone()),
        axum::Json(ApiCreateTaskRequest {
            input_markdown: "Какая политика данных у https://crof.ai/ при инференсе моделей?"
                .to_string(),
            attachments: Vec::new(),
            effort: None,
        }),
    )
    .await
    .expect("create task");

    tokio::time::timeout(Duration::from_secs(2), llm.wait_title_returned())
        .await
        .expect("auto title LLM should return");

    let session_record = state
        .web_store
        .load_session(user.user_id, &session_id)
        .await
        .expect("load session")
        .expect("session exists");
    assert_eq!(session_record.title, "New session");
    assert_ne!(
        session_record.title,
        "Какая политика данных у https://crof.ai/ при инференсе моделей?"
    );

    let completed = wait_for_task_status(
        &state,
        user.user_id,
        &session_id,
        &created_task.task.task_id,
        ApiTaskStatus::Completed,
    )
    .await;
    assert_eq!(completed.final_response_markdown.as_deref(), Some("ok"));
}

#[cfg(feature = "profile-lite")]
#[tokio::test]
async fn api_resume_waiting_task_reuses_task_id_and_persists_completion() {
    let state = test_app_state_with_responses(vec![
            ScriptedResponse::ToolCalls {
                tool_calls: Vec::new(),
                final_text: Some(
                    r#"{"thought":"need details","tool_call":null,"final_answer":null,"awaiting_user_input":{"kind":"text","prompt":"Send scope"}}"#
                        .to_string(),
                ),
            },
            ScriptedResponse::Text("resumed ok".to_string()),
        ])
        .0;
    let now = chrono::Utc::now();
    let user = register_user(
        state.web_store.as_ref(),
        RegisterRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        true,
        now,
    )
    .await
    .expect("register user");
    let (_, auth_session, token) = login_user(
        state.web_store.as_ref(),
        LoginRequest {
            login: "alice".to_string(),
            password: "correct horse battery staple".to_string(),
        },
        now,
    )
    .await
    .expect("login user");

    let axum::Json(created_session) = api_create_session(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
    )
    .await
    .expect("create session");
    let session_id = created_session.session.session_id;

    let axum::Json(created_task) = api_create_task(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
        axum::extract::Path(session_id.clone()),
        axum::Json(ApiCreateTaskRequest {
            input_markdown: "Investigate Codex limits".to_string(),
            attachments: Vec::new(),
            effort: None,
        }),
    )
    .await
    .expect("create task");
    let task_id = created_task.task.task_id;

    let waiting = wait_for_task_status(
        &state,
        user.user_id,
        &session_id,
        &task_id,
        ApiTaskStatus::WaitingForUserInput,
    )
    .await;
    assert_eq!(
        waiting
            .pending_user_input
            .as_ref()
            .map(|input| input.prompt.as_str()),
        Some("Send scope")
    );

    let resume_attachments = vec![TaskAttachment {
        file_name: "scope.txt".to_string(),
        mime_type: Some("text/plain".to_string()),
        size_bytes: 17,
        sandbox_path: "/workspace/uploads/scope.txt".to_string(),
    }];
    let axum::Json(resumed) = api_resume_task(
        axum::extract::State(state.clone()),
        auth_headers(&token, Some(&auth_session.csrf_token)),
        axum::extract::Path((session_id.clone(), task_id.clone())),
        axum::Json(ApiResumeTaskRequest {
            input_markdown: "Scope is GPT-5.4-mini".to_string(),
            attachments: resume_attachments.clone(),
            effort: None,
        }),
    )
    .await
    .expect("resume waiting task");
    assert_eq!(resumed.task.task_id, task_id);
    assert_eq!(resumed.task.status, ApiTaskStatus::Running);

    let persisted_events = state
        .web_store
        .list_task_events(user.user_id, &session_id, &task_id, 0, 256)
        .await
        .expect("list task events")
        .events;
    let resume_event = persisted_events
        .iter()
        .find(|event| event.kind == TaskEventKind::UserMessage)
        .expect("resume user message event exists");
    let payload: UserMessageEventPayload =
        serde_json::from_value(resume_event.payload.clone()).expect("payload parses");
    assert_eq!(payload.input_markdown, "Scope is GPT-5.4-mini");
    assert_eq!(payload.attachments, resume_attachments);

    let completed = wait_for_task_status(
        &state,
        user.user_id,
        &session_id,
        &task_id,
        ApiTaskStatus::Completed,
    )
    .await;
    assert_eq!(
        completed.final_response_markdown.as_deref(),
        Some("resumed ok")
    );

    let session = state
        .web_store
        .load_session(user.user_id, &session_id)
        .await
        .expect("load session")
        .expect("session exists");
    assert_eq!(session.active_task_id, None);
    assert_eq!(session.last_task_status, Some(ApiTaskStatus::Completed));
}

fn test_app_state() -> AppState {
    test_app_state_with_responses(vec![ScriptedResponse::Text("ok".to_string())]).0
}

fn test_app_state_with_responses(
    responses: Vec<ScriptedResponse>,
) -> (AppState, FakeSandboxControl) {
    let scripted = Arc::new(ScriptedLlmProvider::new(responses));
    test_app_state_with_llm_provider(scripted)
}

fn test_app_state_with_llm_provider(
    provider: Arc<dyn LlmProvider>,
) -> (AppState, FakeSandboxControl) {
    let settings = Arc::new(AgentSettings {
        agent_model_id: Some("opencode-go/deepseek-v4-flash".to_string()),
        agent_model_provider: Some("opencode_go".to_string()),
        agent_model_routes: Some(vec![ModelInfo {
            id: "opencode-go/deepseek-v4-flash".to_string(),
            provider: "opencode_go".to_string(),
            max_output_tokens: 32_000,
            context_window_tokens: 200_000,
            weight: 1,
        }]),
        ..AgentSettings::default()
    });
    let mut llm = LlmClient::new(&settings);
    llm.register_provider("opencode_go".to_string(), provider.clone());
    llm.register_provider("opencode-go".to_string(), provider.clone());
    llm.register_provider("llm-provider/opencode-go".to_string(), provider);
    let session_manager = WebSessionManager::new(SessionRegistry::new(), Arc::new(llm), settings);
    let mut state = AppState::new(Arc::new(session_manager));
    let sandbox_control = FakeSandboxControl::default();
    state.set_sandbox_control(Arc::new(sandbox_control.clone()));
    state.auto_title_enabled = false;
    (state, sandbox_control)
}

fn unique_test_asset_dir(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("oxide-web-assets-{label}-{}", uuid::Uuid::new_v4()))
}

fn auth_headers(raw_token: &str, csrf_token: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::COOKIE,
        format!("{AUTH_COOKIE_NAME}={raw_token}")
            .parse()
            .expect("cookie header"),
    );
    if let Some(csrf_token) = csrf_token {
        headers.insert("x-csrf-token", csrf_token.parse().expect("csrf header"));
    }
    headers
}

fn sse_request(
    session_id: &str,
    task_id: &str,
    raw_token: &str,
    after_seq: Option<u64>,
) -> axum::http::Request<axum::body::Body> {
    let mut uri = format!("/api/v1/sessions/{session_id}/tasks/{task_id}/stream");
    if let Some(after_seq) = after_seq {
        uri.push_str(&format!("?after_seq={after_seq}"));
    }

    axum::http::Request::builder()
        .uri(uri)
        .header(
            axum::http::header::COOKIE,
            format!("{AUTH_COOKIE_NAME}={raw_token}"),
        )
        .body(axum::body::Body::empty())
        .expect("sse request")
}

fn web_env_mutex() -> &'static AsyncMutex<()> {
    static LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| AsyncMutex::new(()))
}

struct EnvGuard {
    values: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    fn capture(keys: &[&'static str]) -> Self {
        Self {
            values: keys
                .iter()
                .map(|key| (*key, std::env::var(key).ok()))
                .collect(),
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.values {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
    }
}

#[test]
fn task_preview_source_falls_back_to_attachment_names() {
    let attachments = vec![TaskAttachment {
        file_name: "report.csv".to_string(),
        mime_type: Some("text/csv".to_string()),
        size_bytes: 42,
        sandbox_path: "/workspace/uploads/demo-report.csv".to_string(),
    }];

    assert_eq!(
        super::task_preview_source("   ", &attachments),
        "Attachment: report.csv"
    );
}

#[test]
fn build_task_execution_input_embeds_attachment_paths() {
    let attachments = vec![TaskAttachment {
        file_name: "report.csv".to_string(),
        mime_type: Some("text/csv".to_string()),
        size_bytes: 42,
        sandbox_path: "/workspace/uploads/demo-report.csv".to_string(),
    }];

    let execution_input = super::build_task_execution_input("Analyze this", &attachments);

    assert!(execution_input.contains("Analyze this"));
    assert!(execution_input.contains("report.csv"));
    assert!(execution_input.contains("/workspace/uploads/demo-report.csv"));
    assert!(execution_input.contains("sandbox-local"));
}

#[test]
fn build_task_agent_user_input_preserves_text_and_maps_image_refs() {
    let attachments = vec![
        TaskAttachment {
            file_name: "screenshot.jpg".to_string(),
            mime_type: Some("image/jpeg".to_string()),
            size_bytes: 42,
            sandbox_path: "/workspace/uploads/demo-screenshot.jpg".to_string(),
        },
        TaskAttachment {
            file_name: "report.pdf".to_string(),
            mime_type: Some("application/pdf".to_string()),
            size_bytes: 84,
            sandbox_path: "/workspace/uploads/demo-report.pdf".to_string(),
        },
    ];

    let input = super::build_task_agent_user_input("Analyze these", &attachments);

    assert!(input.text_projection().contains("Analyze these"));
    assert!(input
        .text_projection()
        .contains("/workspace/uploads/demo-screenshot.jpg"));
    assert!(input
        .text_projection()
        .contains("/workspace/uploads/demo-report.pdf"));
    assert_eq!(input.attachments.len(), 1);
    assert_eq!(
        input.attachments[0].kind,
        oxide_agent_core::agent::AgentMessageAttachmentKind::Image
    );
    assert_eq!(input.attachments[0].file_name, "screenshot.jpg");
    assert_eq!(
        input.attachments[0].sandbox_path,
        "/workspace/uploads/demo-screenshot.jpg"
    );
}

fn task_record(
    user_id: i64,
    session_id: &str,
    task_id: &str,
    status: ApiTaskStatus,
    input_markdown: &str,
    created_at: chrono::DateTime<chrono::Utc>,
) -> WebTaskRecord {
    WebTaskRecord {
        schema_version: WEB_TASK_SCHEMA_VERSION,
        task_id: task_id.to_string(),
        session_id: session_id.to_string(),
        user_id,
        version_group_id: task_id.to_string(),
        version_index: 1,
        parent_task_id: None,
        status,
        input_markdown: input_markdown.to_string(),
        attachments: Vec::new(),
        input_edited_at: None,
        final_response_markdown: status
            .is_terminal()
            .then(|| "terminal response".to_string()),
        error_message: None,
        pending_user_input: None,
        last_progress: None,
        last_event_seq: 0,
        created_at,
        started_at: Some(created_at),
        updated_at: created_at,
        finished_at: status.is_terminal().then_some(created_at),
    }
}

fn persisted_event(
    user_id: i64,
    session_id: &str,
    task_id: &str,
    seq: u64,
    kind: TaskEventKind,
) -> PersistedTaskEvent {
    PersistedTaskEvent {
        schema_version: 1,
        task_id: task_id.to_string(),
        session_id: session_id.to_string(),
        user_id,
        seq,
        created_at: chrono::Utc::now(),
        kind,
        summary: format!("event-{seq}"),
        payload: serde_json::json!({ "seq": seq }),
        redacted: false,
        truncated: false,
    }
}

#[cfg(feature = "profile-lite")]
async fn wait_for_task_status(
    state: &AppState,
    user_id: i64,
    session_id: &str,
    task_id: &str,
    status: ApiTaskStatus,
) -> WebTaskRecord {
    let mut last_task = None;
    for _ in 0..200 {
        let task = state
            .web_store
            .load_task(user_id, session_id, task_id)
            .await
            .expect("load task")
            .expect("task exists");
        if task.status == status {
            return task;
        }
        last_task = Some(task);
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("task {task_id} did not reach {status:?}; last state: {last_task:?}");
}

#[cfg(feature = "profile-lite")]
async fn wait_for_session_title(state: &AppState, user_id: i64, session_id: &str, expected: &str) {
    let mut last_title = None;
    for _ in 0..100 {
        let session = state
            .web_store
            .load_session(user_id, session_id)
            .await
            .expect("load session")
            .expect("session exists");
        if session.title == expected {
            return;
        }
        last_title = Some(session.title);
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("session {session_id} did not reach title {expected:?}; last title: {last_title:?}");
}

async fn wait_for_persisted_progress(
    state: &AppState,
    user_id: i64,
    session_id: &str,
    task_id: &str,
) -> ProgressSnapshot {
    for _ in 0..40 {
        let task = state
            .web_store
            .load_task(user_id, session_id, task_id)
            .await
            .expect("load task")
            .expect("task exists");
        if let Some(progress) = task.last_progress {
            return progress;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("task {task_id} did not receive persisted progress");
}

#[cfg(feature = "profile-lite")]
async fn save_active_task(
    state: &AppState,
    base_task: &WebTaskRecord,
    status: ApiTaskStatus,
    pending_user_input: Option<PendingUserInputView>,
) {
    let now = chrono::Utc::now();
    let mut task = base_task.clone();
    task.task_id = format!("active-{}", status_string(status));
    task.status = status;
    task.final_response_markdown = None;
    task.error_message = None;
    task.pending_user_input = pending_user_input;
    task.updated_at = now;
    task.finished_at = None;
    task.schema_version = WEB_TASK_SCHEMA_VERSION;
    state
        .web_store
        .save_task(task.clone())
        .await
        .expect("save active task");

    let mut session = state
        .web_store
        .load_session(task.user_id, &task.session_id)
        .await
        .expect("load session")
        .expect("session exists");
    session.active_task_id = Some(task.task_id);
    session.last_task_status = Some(status);
    session.updated_at = now;
    state
        .web_store
        .save_session(session)
        .await
        .expect("save active session");
}

#[cfg(feature = "profile-lite")]
fn status_string(status: ApiTaskStatus) -> &'static str {
    match status {
        ApiTaskStatus::Queued => "queued",
        ApiTaskStatus::Running => "running",
        ApiTaskStatus::WaitingForUserInput => "waiting",
        ApiTaskStatus::Completed => "completed",
        ApiTaskStatus::Failed => "failed",
        ApiTaskStatus::Cancelled => "cancelled",
        ApiTaskStatus::Interrupted => "interrupted",
    }
}
