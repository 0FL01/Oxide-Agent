//! Integration test: launch Chromium through the native sidecar and verify
//! the session lifecycle (POST /sessions, DELETE /sessions/{id}).
//!
//! Requires a real Chromium binary.  Run with:
//!
//! ```sh
//! cargo test -p oxide-browser-sidecar --test cdp_integration -- --ignored --nocapture
//! ```

use oxide_browser_contracts::{
    BrowserMode, BrowserProfile, CloseReason, CloseSessionRequest, CreateSessionRequest, Viewport,
};
use oxide_browser_sidecar::{AppState, create_app, session::SessionManager};
use std::sync::Arc;

#[tokio::test]
#[ignore = "requires Chromium binary"]
async fn session_lifecycle() {
    let state = AppState {
        sessions: Arc::new(SessionManager::default()),
    };
    let app = create_app(state, "test-token".to_string());

    // Bind to a random port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind test server");
    let addr = listener
        .local_addr()
        .expect("failed to get test server addr");
    let base_url = format!("http://{addr}");

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("test server failed");
    });

    let client = reqwest::Client::new();

    // 1. Healthz (no auth).
    let resp = client
        .get(format!("{base_url}/healthz"))
        .send()
        .await
        .expect("healthz request failed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("healthz body");
    assert_eq!(body["ok"], true);
    assert_eq!(body["native"], true);

    // 2. Create session — navigate to a data URL (no network needed).
    let create_req = CreateSessionRequest {
        task_id: "integration-test".to_string(),
        profile: BrowserProfile::Ephemeral,
        mode: BrowserMode::StealthClean,
        viewport: Viewport::default(),
        timezone: None,
        locale: None,
        record_console: false,
        record_network: false,
        allow_downloads: false,
        allow_uploads: false,
        start_url: Some(
            "data:text/html,<html><head><title>CDP Test</title></head><body><h1>Hello</h1></body></html>"
                .to_string(),
        ),
    };

    let resp = client
        .post(format!("{base_url}/sessions"))
        .header("authorization", "Bearer test-token")
        .json(&create_req)
        .send()
        .await
        .expect("create session request failed");

    assert_eq!(resp.status(), 200);
    let create_resp: serde_json::Value = resp.json().await.expect("create session body");

    assert_eq!(
        create_resp["ok"], true,
        "session creation failed: {create_resp}"
    );
    assert_eq!(create_resp["browser"]["cdp_connected"], true);
    let session_id = create_resp["session_id"]
        .as_str()
        .expect("session_id in response")
        .to_string();

    // 3. Close session.
    let close_req = CloseSessionRequest {
        purge_profile: true,
        keep_artifacts: true,
        reason: CloseReason::Done,
    };

    let resp = client
        .delete(format!("{base_url}/sessions/{session_id}"))
        .header("authorization", "Bearer test-token")
        .json(&close_req)
        .send()
        .await
        .expect("close session request failed");

    assert_eq!(resp.status(), 200);
    let close_resp: serde_json::Value = resp.json().await.expect("close session body");
    assert_eq!(close_resp["ok"], true);
    assert_eq!(close_resp["closed"], true);

    // 4. Auth: wrong token → 401.
    let resp = client
        .post(format!("{base_url}/sessions"))
        .header("authorization", "Bearer wrong-token")
        .json(&create_req)
        .send()
        .await
        .expect("auth check request failed");
    assert_eq!(resp.status(), 401);
}
