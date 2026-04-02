use super::response::{format_http_error, is_retryable_error};
use super::*;
use crate::agent::tool_runtime::scope_tool_model_route;
use reqwest::StatusCode;
use std::env;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn test_settings() -> Arc<crate::config::AgentSettings> {
    Arc::new(crate::config::AgentSettings {
        gemini_api_key: Some("gemini-secret".to_string()),
        minimax_api_key: Some("minimax-secret".to_string()),
        zai_api_key: Some("zai-secret".to_string()),
        openrouter_api_key: Some("openrouter-secret".to_string()),
        ..crate::config::AgentSettings::default()
    })
}

fn test_settings_with_dedicated_browser_use_model() -> Arc<crate::config::AgentSettings> {
    Arc::new(crate::config::AgentSettings {
        gemini_api_key: Some("gemini-secret".to_string()),
        minimax_api_key: Some("minimax-secret".to_string()),
        zai_api_key: Some("zai-secret".to_string()),
        openrouter_api_key: Some("openrouter-secret".to_string()),
        browser_use_model_id: Some("GLM-4.6V".to_string()),
        browser_use_model_provider: Some("zai".to_string()),
        ..crate::config::AgentSettings::default()
    })
}

#[test]
fn test_args_deserialize() {
    let run: Result<RunTaskArgs, _> = serde_json::from_str(
        r#"{"task":"Open docs","start_url":"https://example.com","session_id":"s1","timeout_secs":120,"reuse_profile":true,"profile_id":"browser-profile-1"}"#,
    );
    assert!(run.is_ok());

    let session: Result<SessionArgs, _> = serde_json::from_str(r#"{"session_id":"s1"}"#);
    assert!(session.is_ok());

    let extract: Result<ExtractContentArgs, _> =
        serde_json::from_str(r#"{"session_id":"s1","format":"html","max_chars":4000}"#);
    assert!(extract.is_ok());

    let screenshot: Result<ScreenshotArgs, _> =
        serde_json::from_str(r#"{"session_id":"s1","full_page":true}"#);
    assert!(screenshot.is_ok());
}

#[test]
fn run_task_request_body_serializes_profile_reuse_hints() {
    let payload = serde_json::to_value(RunTaskRequestBody {
        task: "Open docs".to_string(),
        start_url: None,
        session_id: None,
        timeout_secs: None,
        reuse_profile: true,
        profile_id: Some("browser-profile-1".to_string()),
        profile_scope: Some("topic-a".to_string()),
        execution_mode: RunTaskExecutionMode::Autonomous,
        browser_llm_config: None,
    })
    .expect("serialize request body");

    assert_eq!(payload["reuse_profile"], serde_json::Value::Bool(true));
    assert_eq!(payload["profile_id"], "browser-profile-1");
    assert_eq!(payload["profile_scope"], "topic-a");
    assert_eq!(payload["execution_mode"], "autonomous");
}

#[test]
fn run_task_steering_classifies_screenshot_and_extract_requests() {
    let screenshot = classify_run_task_steering("Open the dashboard and take a screenshot");
    assert!(screenshot.prefer_screenshot_tool);
    assert!(!screenshot.prefer_extract_tool);
    assert!(!screenshot.prefer_visual_description);

    let extract = classify_run_task_steering("Открой страницу и извлеки текст страницы");
    assert!(!extract.prefer_screenshot_tool);
    assert!(extract.prefer_extract_tool);

    let visual = classify_run_task_steering("Describe what you see on the homepage and its colors");
    assert!(!visual.prefer_screenshot_tool);
    assert!(!visual.prefer_extract_tool);
    assert!(visual.prefer_visual_description);
}

#[tokio::test]
async fn run_task_rejects_profile_reuse_without_runtime_scope() {
    let provider = BrowserUseProvider::new("http://localhost:8002", test_settings());

    let error = provider
        .execute(
            TOOL_RUN_TASK,
            r#"{"task":"Open docs","reuse_profile":true}"#,
            None,
            None,
        )
        .await
        .expect_err("profile reuse without runtime scope should fail");

    assert!(error
        .to_string()
        .contains("Browser Use profile reuse requires a topic-scoped runtime context"));
}

#[test]
fn test_url_building() {
    let provider = BrowserUseProvider::with_config(
        "http://localhost:8002/",
        test_settings(),
        Duration::from_secs(1),
        0,
        Duration::from_secs(1),
        Duration::from_secs(10),
    );
    let url = provider.endpoint_url("/sessions/run");
    assert_eq!(url, "http://localhost:8002/sessions/run");
}

#[test]
fn test_http_error_formatting() {
    let msg = format_http_error(StatusCode::SERVICE_UNAVAILABLE, "bridge unavailable");
    assert!(msg.contains("503"));
    assert!(msg.contains("bridge unavailable"));
}

#[test]
fn browser_session_not_alive_errors_are_not_retryable() {
    assert!(!is_retryable_error(
        "Browser Use error: 409 - {\"detail\":{\"error\":\"browser_session_not_alive\",\"message\":\"session is dead\"}}"
    ));
}

#[test]
fn generic_bridge_500_errors_are_not_retryable() {
    assert!(!is_retryable_error(
        "Browser Use error: 500 - browser_use bridge could not create screenshot from active session"
    ));
    assert!(is_retryable_error(
        "Browser Use error: 503 - bridge unavailable"
    ));
}

#[tokio::test]
async fn run_task_posts_to_bridge() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(
            r#"{"session_id":"browser-use-123","status":"completed","final_url":"https://example.com","summary":"Done"}"#,
        ),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );

    let result = provider
        .execute(TOOL_RUN_TASK, r#"{"task":"Open example"}"#, None, None)
        .await;

    assert!(result.is_ok());
    let output = result.unwrap_or_default();
    assert!(output.contains("browser-use-123"));
    assert!(!output.contains("Follow-up guidance:"));
    assert!(state
        .request_line()
        .await
        .contains("POST /sessions/run HTTP/1.1"));
    assert!(state
        .request_body()
        .await
        .contains(r#""execution_mode":"autonomous""#));
}

#[tokio::test]
async fn run_task_rewrites_screenshot_focused_requests_and_adds_follow_up_guidance() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(
            r#"{"session_id":"browser-use-123","status":"completed","final_url":"https://example.com/dashboard","summary":"Ready","browser_runtime_alive":true,"browser_runtime_dead_reason":null}"#,
        ),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );

    let output = provider
        .execute(
            TOOL_RUN_TASK,
            r#"{"task":"Open the dashboard and take a screenshot"}"#,
            None,
            None,
        )
        .await
        .expect("screenshot-focused run should succeed");

    assert!(output.contains("browser-use-123"));
    assert!(output.contains(
        "- Use `browser_use_screenshot` with `session_id` `browser-use-123` to capture the final page state."
    ));
    let body = state.request_body().await;
    assert!(body.contains("Navigation goal for this run:\\nOpen the dashboard"));
    assert!(!body.contains("Original task:"));
    assert!(!body.contains("Open the dashboard and take a screenshot"));
    assert!(body.contains("Oxide will call `browser_use_screenshot` after navigation is complete."));
    assert!(body.contains("Do not take screenshots, describe the final visual result, save PDFs, or perform final page-content extraction in this step."));
    assert!(body.contains(r#""execution_mode":"navigation_only""#));
}

#[tokio::test]
async fn run_task_replaces_follow_up_guidance_when_bridge_reports_dead_runtime() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(
            r#"{"session_id":"browser-use-123","status":"completed","final_url":"https://example.com/dashboard","summary":"Ready","browser_runtime_alive":false,"browser_runtime_dead_reason":"browser session is not alive: browser runtime is closed"}"#,
        ),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );

    let output = provider
        .execute(
            TOOL_RUN_TASK,
            r#"{"task":"Open the dashboard and take a screenshot"}"#,
            None,
            None,
        )
        .await
        .expect("dead-runtime run should still return output");

    assert!(output.contains("browser-use-123"));
    assert!(output.contains(
        "Follow-up note: Browser Use reported that this session runtime with `session_id` `browser-use-123` is no longer alive."
    ));
    assert!(output.contains(
        "Re-run `browser_use_run_task` before calling `browser_use_screenshot` or `browser_use_extract_content`."
    ));
    assert!(output
        .contains("Reported reason: `browser session is not alive: browser runtime is closed`."));
    assert!(!output.contains(
        "Follow-up guidance: use `browser_use_screenshot` with `session_id` `browser-use-123`"
    ));
}

#[tokio::test]
async fn run_task_rewrites_extract_focused_requests_and_adds_follow_up_guidance() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(
            r#"{"session_id":"browser-use-123","status":"completed","final_url":"https://example.com/docs","summary":"Ready"}"#,
        ),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );

    let output = provider
        .execute(
            TOOL_RUN_TASK,
            r#"{"task":"Открой документацию и извлеки текст страницы"}"#,
            None,
            None,
        )
        .await
        .expect("extract-focused run should succeed");

    assert!(output.contains("browser-use-123"));
    assert!(output.contains(
        "- Use `browser_use_extract_content` with `session_id` `browser-use-123` for the final text or HTML extraction."
    ));
    let body = state.request_body().await;
    assert!(body.contains("Navigation goal for this run:\\nОткрой документацию"));
    assert!(!body.contains("Original task:"));
    assert!(!body.contains("Открой документацию и извлеки текст страницы"));
    assert!(body
        .contains("Oxide will call `browser_use_extract_content` after navigation is complete."));
    assert!(body.contains("Leave the session on the target page for Oxide follow-up tools."));
    assert!(body.contains(r#""execution_mode":"navigation_only""#));
}

#[tokio::test]
async fn get_session_reads_bridge_json() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(
            r#"{"session_id":"browser-use-123","status":"completed","current_url":"https://example.com"}"#,
        ),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );

    let result = provider
        .execute(
            TOOL_GET_SESSION,
            r#"{"session_id":"browser-use-123"}"#,
            None,
            None,
        )
        .await;

    assert!(result.is_ok());
    let output = result.unwrap_or_default();
    assert!(output.contains("current_url"));
    assert!(state
        .request_line()
        .await
        .contains("GET /sessions/browser-use-123 HTTP/1.1"));
}

#[tokio::test]
async fn extract_content_posts_to_bridge() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(
            r#"{"session_id":"browser-use-123","status":"completed","format":"text","content":"hello","truncated":false,"total_chars":5}"#,
        ),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );

    let result = provider
        .execute(
            TOOL_EXTRACT_CONTENT,
            r#"{"session_id":"browser-use-123","format":"html","max_chars":2048}"#,
            None,
            None,
        )
        .await;

    assert!(result.is_ok());
    assert!(state
        .request_line()
        .await
        .contains("POST /sessions/browser-use-123/extract_content HTTP/1.1"));
    let body = state.request_body().await;
    assert!(body.contains(r#""format":"html""#));
    assert!(body.contains(r#""max_chars":2048"#));
}

#[tokio::test]
async fn screenshot_posts_to_bridge() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(
            r#"{"session_id":"browser-use-123","status":"completed","artifact":{"kind":"screenshot","path":"/tmp/test.png"}}"#,
        ),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );

    let result = provider
        .execute(
            TOOL_SCREENSHOT,
            r#"{"session_id":"browser-use-123","full_page":true}"#,
            None,
            None,
        )
        .await;

    assert!(result.is_ok());
    assert!(state
        .request_line()
        .await
        .contains("POST /sessions/browser-use-123/screenshot HTTP/1.1"));
    let body = state.request_body().await;
    assert!(body.contains(r#""full_page":true"#));
}

#[test]
fn browser_llm_config_maps_minimax_route() {
    let provider = BrowserUseProvider::new("http://localhost:8002", test_settings());
    let route = crate::config::ModelInfo {
        id: "MiniMax-M2.7".to_string(),
        provider: "minimax".to_string(),
        max_output_tokens: 4096,
        context_window_tokens: 128_000,
        weight: 1,
    };

    let (config, api_key) = provider
        .browser_llm_config_for_route(&route)
        .expect("minimax route config");

    assert_eq!(config.provider, "minimax");
    assert_eq!(config.model, "MiniMax-M2.7");
    assert_eq!(config.api_base.as_deref(), Some(MINIMAX_DEFAULT_API_BASE));
    assert_eq!(config.api_key_ref, None);
    assert_eq!(api_key, "minimax-secret");
    assert!(!config.supports_vision);
    assert!(config.supports_tools);
}

#[test]
fn browser_llm_config_marks_text_only_openrouter_models_without_vision() {
    let provider = BrowserUseProvider::new("http://localhost:8002", test_settings());
    let route = crate::config::ModelInfo {
        id: "deepseek/deepseek-chat".to_string(),
        provider: "openrouter".to_string(),
        max_output_tokens: 4096,
        context_window_tokens: 128_000,
        weight: 1,
    };

    let (config, _) = provider
        .browser_llm_config_for_route(&route)
        .expect("openrouter route config");

    assert!(!config.supports_vision);
}

#[test]
fn browser_llm_config_marks_vision_openrouter_models() {
    let provider = BrowserUseProvider::new("http://localhost:8002", test_settings());
    let route = crate::config::ModelInfo {
        id: "google/gemini-3-flash-preview".to_string(),
        provider: "openrouter".to_string(),
        max_output_tokens: 4096,
        context_window_tokens: 128_000,
        weight: 1,
    };

    let (config, _) = provider
        .browser_llm_config_for_route(&route)
        .expect("openrouter route config");

    assert!(config.supports_vision);
}

#[test]
fn browser_llm_config_marks_glm_4_6v_as_vision_capable() {
    let provider = BrowserUseProvider::new("http://localhost:8002", test_settings());
    let route = crate::config::ModelInfo {
        id: "GLM-4.6V".to_string(),
        provider: "zai".to_string(),
        max_output_tokens: 4096,
        context_window_tokens: 128_000,
        weight: 1,
    };

    let (config, _) = provider
        .browser_llm_config_for_route(&route)
        .expect("zai route config");

    assert!(config.supports_vision);
}

#[test]
fn browser_llm_config_requires_configured_secret() {
    let provider = BrowserUseProvider::new(
        "http://localhost:8002",
        Arc::new(crate::config::AgentSettings::default()),
    );
    let route = crate::config::ModelInfo {
        id: "glm-5-turbo".to_string(),
        provider: "zai".to_string(),
        max_output_tokens: 4096,
        context_window_tokens: 128_000,
        weight: 1,
    };

    let error = provider
        .browser_llm_config_for_route(&route)
        .expect_err("missing key should fail");

    assert!(error.to_string().contains(
        "Browser Use route inheritance requires configured credential for provider `zai`"
    ));
}

#[tokio::test]
async fn run_task_posts_inherited_browser_llm_config() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(r#"{"session_id":"browser-use-123","status":"completed","summary":"Done"}"#),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );
    let route = crate::config::ModelInfo {
        id: "glm-5-turbo".to_string(),
        provider: "zai".to_string(),
        max_output_tokens: 4096,
        context_window_tokens: 128_000,
        weight: 1,
    };

    let result = scope_tool_model_route(
        route,
        provider.execute(TOOL_RUN_TASK, r#"{"task":"Open example"}"#, None, None),
    )
    .await;

    assert!(result.is_ok());
    let request_body = state.request_body().await;
    assert!(request_body.contains("\"browser_llm_config\":"));
    assert!(request_body.contains("\"provider\":\"zai\""));
    assert!(request_body.contains("\"model\":\"glm-5-turbo\""));
    assert!(!request_body.contains("api_key_ref"));
    assert_eq!(
        state.header_value(OXIDE_BROWSER_LLM_API_KEY_HEADER).await,
        Some("zai-secret".to_string())
    );
}

#[tokio::test]
async fn run_task_prefers_dedicated_browser_use_model_over_active_route() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(r#"{"session_id":"browser-use-123","status":"completed","summary":"Done"}"#),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings_with_dedicated_browser_use_model(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );
    let active_route = crate::config::ModelInfo {
        id: "MiniMax-M2.7".to_string(),
        provider: "minimax".to_string(),
        max_output_tokens: 4096,
        context_window_tokens: 128_000,
        weight: 1,
    };

    let result = scope_tool_model_route(
        active_route,
        provider.execute(TOOL_RUN_TASK, r#"{"task":"Open example"}"#, None, None),
    )
    .await;

    assert!(result.is_ok());
    let request_body = state.request_body().await;
    assert!(request_body.contains("\"browser_llm_config\":"));
    assert!(request_body.contains("\"provider\":\"zai\""));
    assert!(request_body.contains("\"model\":\"GLM-4.6V\""));
    assert!(!request_body.contains("MiniMax-M2.7"));
    assert_eq!(
        state.header_value(OXIDE_BROWSER_LLM_API_KEY_HEADER).await,
        Some("zai-secret".to_string())
    );
}

#[tokio::test]
async fn run_task_posts_runtime_profile_scope_for_reuse() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(r#"{"session_id":"browser-use-123","status":"completed","summary":"Done"}"#),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    )
    .with_profile_scope("topic-a");

    provider
        .execute(
            TOOL_RUN_TASK,
            r#"{"task":"Open example","reuse_profile":true}"#,
            None,
            None,
        )
        .await
        .expect("scoped reuse run should succeed");

    let request_body = state.request_body().await;
    assert!(request_body.contains(r#""reuse_profile":true"#));
    assert!(request_body.contains(r#""profile_scope":"topic-a""#));
}

#[tokio::test]
async fn run_task_warns_for_ui_heavy_text_only_route() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(r#"{"session_id":"browser-use-123","status":"completed","summary":"Done"}"#),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );
    let route = crate::config::ModelInfo {
        id: "MiniMax-M2.7".to_string(),
        provider: "minimax".to_string(),
        max_output_tokens: 4096,
        context_window_tokens: 128_000,
        weight: 1,
    };

    let output = scope_tool_model_route(
        route,
        provider.execute(
            TOOL_RUN_TASK,
            r#"{"task":"Click the login button and submit the form"}"#,
            None,
            None,
        ),
    )
    .await
    .expect("ui-heavy task should still run");

    assert!(output.contains("Warning: Browser Use is running with text-only route"));
    assert!(output.contains("browser-use-123"));
}

#[tokio::test]
async fn run_task_warns_for_russian_ui_heavy_text_only_route() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(r#"{"session_id":"browser-use-123","status":"completed","summary":"Done"}"#),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );
    let route = crate::config::ModelInfo {
        id: "MiniMax-M2.7".to_string(),
        provider: "minimax".to_string(),
        max_output_tokens: 4096,
        context_window_tokens: 128_000,
        weight: 1,
    };

    let output = scope_tool_model_route(
        route,
        provider.execute(
            TOOL_RUN_TASK,
            r#"{"task":"Нажми кнопку входа и отправь форму"}"#,
            None,
            None,
        ),
    )
    .await
    .expect("Russian UI-heavy task should still run");

    assert!(output.contains("Warning: Browser Use is running with text-only route"));
    assert!(output.contains("browser-use-123"));
}

#[tokio::test]
async fn run_task_splits_visual_description_into_navigation_only_follow_up_on_text_only_route() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(r#"{"session_id":"browser-use-123","status":"completed","summary":"Ready"}"#),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );
    let route = crate::config::ModelInfo {
        id: "glm-5-turbo".to_string(),
        provider: "zai".to_string(),
        max_output_tokens: 4096,
        context_window_tokens: 128_000,
        weight: 1,
    };

    let output = scope_tool_model_route(
        route,
        provider.execute(
            TOOL_RUN_TASK,
            r#"{"task":"Describe the visual layout and colors of the homepage"}"#,
            None,
            None,
        ),
    )
    .await
    .expect("visual description should split into navigation-only follow-up");

    assert!(output.contains("browser-use-123"));
    assert!(output.contains("describe_image_file"));
    assert!(output.contains("artifact.stable_path"));
    assert!(!output.contains("Warning: Browser Use is running with text-only route"));
    let request_body = state.request_body().await;
    assert!(request_body.contains(r#""execution_mode":"navigation_only""#));
    assert!(request_body.contains("Navigation goal for this run"));
    assert!(!request_body.contains("Describe the visual layout and colors of the homepage"));
}

#[tokio::test]
async fn run_task_splits_russian_visual_description_on_text_only_route() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(r#"{"session_id":"browser-use-123","status":"completed","summary":"Ready"}"#),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );
    let route = crate::config::ModelInfo {
        id: "MiniMax-M2.7".to_string(),
        provider: "minimax".to_string(),
        max_output_tokens: 4096,
        context_window_tokens: 128_000,
        weight: 1,
    };

    let output = scope_tool_model_route(
        route,
        provider.execute(
            TOOL_RUN_TASK,
            r#"{"task":"Опиши визуально, как выглядит главная страница и какие там цвета"}"#,
            None,
            None,
        ),
    )
    .await
    .expect("Russian visual description should split into navigation-only follow-up");

    assert!(output.contains("browser-use-123"));
    assert!(output.contains("browser_use_screenshot"));
    assert!(output.contains("describe_image_file"));
    assert!(output.contains("artifact.stable_path"));
    assert!(!output.contains("Warning: Browser Use is running with text-only route"));
    let request_body = state.request_body().await;
    assert!(request_body.contains(r#""execution_mode":"navigation_only""#));
    assert!(request_body.contains("Navigation goal for this run"));
}

#[tokio::test]
async fn run_task_allows_russian_visual_analysis_on_dedicated_glm_4_6v_route() {
    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(r#"{"session_id":"browser-use-123","status":"completed","summary":"Done"}"#),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings_with_dedicated_browser_use_model(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );

    let output = provider
        .execute(
            TOOL_RUN_TASK,
            r#"{"task":"Опиши визуально, как выглядит главная страница и какие там цвета"}"#,
            None,
            None,
        )
        .await
        .expect("vision-capable dedicated route should allow Russian visual analysis");

    assert!(!output.contains("Warning: Browser Use is running with text-only route"));
    assert!(output.contains("browser-use-123"));
    assert!(output.contains("describe_image_file"));
    assert!(output.contains("artifact.stable_path"));
    let request_body = state.request_body().await;
    assert!(request_body.contains("\"model\":\"GLM-4.6V\""));
    assert!(request_body.contains(r#""execution_mode":"navigation_only""#));
}

#[tokio::test]
async fn run_task_fast_fails_autonomous_visual_task_on_unstable_glm_4_6v_route() {
    let provider = BrowserUseProvider::new(
        "http://localhost:8002",
        test_settings_with_dedicated_browser_use_model(),
    );

    let error = provider
        .execute(
            TOOL_RUN_TASK,
            r#"{"task":"Open the login page and solve the captcha to continue"}"#,
            None,
            None,
        )
        .await
        .expect_err("autonomous visual tasks should fast-fail on unstable glm route");

    let message = error.to_string();
    assert!(message.contains("autonomous visual tasks are disabled"));
    assert!(message.contains("zai/GLM-4.6V"));
    assert!(message.contains("browser_use_screenshot"));
    assert!(message.contains("describe_image_file"));
}

#[tokio::test]
async fn run_task_allows_autonomous_visual_task_when_unstable_routes_disabled() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    env::set_var(BROWSER_USE_UNSTABLE_VISUAL_ROUTES_ENV, "off");

    let state = Arc::new(TestServerState::default());
    let server = TestServer::spawn(
        Arc::clone(&state),
        json_response(r#"{"session_id":"browser-use-123","status":"completed","summary":"Done"}"#),
    )
    .await;
    let provider = BrowserUseProvider::with_config(
        &server.base_url,
        test_settings_with_dedicated_browser_use_model(),
        Duration::from_secs(3),
        0,
        Duration::from_secs(1),
        Duration::from_secs(2),
    );

    let output = provider
        .execute(
            TOOL_RUN_TASK,
            r#"{"task":"Open the login page and solve the captcha to continue"}"#,
            None,
            None,
        )
        .await
        .expect("route blocklist disabled should allow autonomous visual task");

    assert!(output.contains("browser-use-123"));
    let request_body = state.request_body().await;
    assert!(request_body.contains(r#""execution_mode":"autonomous""#));

    env::remove_var(BROWSER_USE_UNSTABLE_VISUAL_ROUTES_ENV);
}

#[test]
fn unstable_visual_route_patterns_parse_csv_and_model_only_entries() {
    let patterns =
        parse_unstable_visual_route_patterns(" zai/glm-4.6v, glm-4.7 , openrouter/qwen-vl ");
    assert_eq!(
        patterns,
        vec![
            "zai/glm-4.6v".to_string(),
            "glm-4.7".to_string(),
            "openrouter/qwen-vl".to_string()
        ]
    );
}

#[test]
fn route_matches_pattern_supports_provider_and_model_patterns() {
    assert!(route_matches_pattern("zai/glm-4.6v", "zai/glm-4.6v"));
    assert!(route_matches_pattern("zai/glm-4.7", "glm-4.7"));
    assert!(!route_matches_pattern(
        "openrouter/gpt-4o-mini",
        "zai/glm-4.6v"
    ));
}

#[tokio::test]
async fn run_task_rejects_unsupported_inherited_route() {
    let provider = BrowserUseProvider::new("http://localhost:8002", test_settings());
    let route = crate::config::ModelInfo {
        id: "llama-3.3".to_string(),
        provider: "groq".to_string(),
        max_output_tokens: 4096,
        context_window_tokens: 128_000,
        weight: 1,
    };

    let error = scope_tool_model_route(
        route,
        provider.execute(TOOL_RUN_TASK, r#"{"task":"Open example"}"#, None, None),
    )
    .await
    .expect_err("unsupported route should fail");

    assert!(error
        .to_string()
        .contains("Browser Use route inheritance does not support provider `groq` yet"));
}

#[derive(Default)]
struct TestServerState {
    request: tokio::sync::Mutex<String>,
}

impl TestServerState {
    async fn record(&self, request: String) {
        *self.request.lock().await = request;
    }

    async fn request_line(&self) -> String {
        self.request
            .lock()
            .await
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    async fn request_body(&self) -> String {
        self.request
            .lock()
            .await
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or_default()
            .to_string()
    }

    async fn header_value(&self, name: &str) -> Option<String> {
        let prefix = format!("{}:", name.to_ascii_lowercase());
        self.request.lock().await.lines().find_map(|line| {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with(&prefix) {
                line.split_once(':')
                    .map(|(_, value)| value.trim().to_string())
            } else {
                None
            }
        })
    }
}

struct TestServer {
    base_url: String,
}

impl TestServer {
    async fn spawn(state: Arc<TestServerState>, response: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read local addr");
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = vec![0_u8; 4096];
                if let Ok(read) = socket.read(&mut buffer).await {
                    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                    state.record(request).await;
                }
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        Self {
            base_url: format!("http://{addr}"),
        }
    }
}

fn json_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body,
    )
}
