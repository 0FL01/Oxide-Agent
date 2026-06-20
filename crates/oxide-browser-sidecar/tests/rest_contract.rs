//! Integration test: full REST contract against real Chromium.
//!
//! Verifies all 9 REST endpoints (G5): healthz, sessions, goto, observe, action,
//! screenshot/latest, debug/network, debug/console. Uses reqwest to drive the
//! axum server directly — the same HTTP contract that `BrowserSidecarClient`
//! consumes.

use oxide_browser_contracts::{
    ActionRequest, BrowserAction, BrowserProfile, CloseReason, CloseSessionRequest,
    CreateSessionRequest, GotoRequest, Viewport, WaitUntil,
};
use oxide_browser_sidecar::{AppState, create_app};
use serde_json::Value;
use std::sync::Arc;
use tokio::net::TcpListener;

const TOKEN: &str = "test-token-cp6";

const TEST_PAGE: &str = "data:text/html,\
<html><head><title>CP6 Test Page</title></head><body>\
<h1 id=\"welcome\">Welcome</h1>\
<button id=\"btn\" onclick=\"this.textContent='Clicked!'\">Login</button>\
<input id=\"email\" type=\"text\" placeholder=\"Email\" />\
<a href=\"https://example.com\" id=\"link\">Example Link</a>\
<div id=\"dynamic\" style=\"display:none\">Dynamic Content</div>\
</body></html>";

async fn start_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let state = AppState {
        sessions: Arc::new(Default::default()),
    };
    let app = create_app(state, TOKEN.to_string());
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    format!("http://{addr}")
}

fn auth_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .expect("client")
}

#[tokio::test]
#[ignore = "requires Chromium binary"]
async fn full_rest_contract_on_real_chromium() {
    let base = start_server().await;
    let http = auth_client();

    // 1. healthz (no auth).
    let resp: Value = http
        .get(format!("{base}/healthz"))
        .send()
        .await
        .expect("healthz")
        .json()
        .await
        .expect("healthz json");
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["native"], true);

    // 2. Create session.
    let create_req = CreateSessionRequest {
        task_id: "cp6-test".to_string(),
        profile: BrowserProfile::Ephemeral,
        viewport: Viewport::default(),
        timezone: None,
        locale: None,
        record_console: true,
        record_network: true,
        allow_downloads: false,
        allow_uploads: false,
        start_url: Some(TEST_PAGE.to_string()),
    };
    let resp: Value = http
        .post(format!("{base}/sessions"))
        .bearer_auth(TOKEN)
        .json(&create_req)
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session json");
    assert_eq!(resp["ok"], true, "create session ok");
    let session_id = resp["session_id"].as_str().expect("session_id").to_string();
    assert!(!session_id.is_empty());

    // 3. Goto (with capture_after).
    let goto_req = GotoRequest {
        url: TEST_PAGE.to_string(),
        wait_until: WaitUntil::Load,
        timeout_ms: 30_000,
        action_seq: 1,
        capture_after: true,
        force_reload: false,
    };
    let resp: Value = http
        .post(format!("{base}/sessions/{session_id}/goto"))
        .bearer_auth(TOKEN)
        .json(&goto_req)
        .send()
        .await
        .expect("goto")
        .json()
        .await
        .expect("goto json");
    assert_eq!(resp["ok"], true, "goto ok");
    assert_eq!(resp["navigation"]["status"], "loaded");
    assert!(resp["observation"].is_object(), "goto has observation");

    // 4. Observe (fresh, with DOM).
    let resp: Value = http
        .get(format!("{base}/sessions/{session_id}/observe"))
        .bearer_auth(TOKEN)
        .query(&[
            ("fresh", "true"),
            ("include_dom", "true"),
            ("include_a11y", "true"),
            ("include_network_summary", "true"),
            ("include_console_summary", "true"),
            ("max_debug_items", "20"),
        ])
        .send()
        .await
        .expect("observe")
        .json()
        .await
        .expect("observe json");
    assert_eq!(resp["ok"], true, "observe ok");
    let obs = &resp["observation"];
    assert!(
        obs["screenshot"]["artifact_uri"].as_str().is_some(),
        "has screenshot"
    );
    assert!(obs["a11y_summary"].is_array(), "has a11y_summary");
    assert!(obs["dom_snapshot"].is_array(), "has dom_snapshot");
    assert!(obs["url"].as_str().is_some(), "has url");

    // 5. Action: click_selector.
    let action_req = ActionRequest {
        action_seq: 1,
        action: BrowserAction::ClickSelector {
            selector: "#btn".to_string(),
        },
        expected_result: "button text changes to Clicked!".to_string(),
        timeout_ms: 10_000,
        capture_after: true,
        wait_for_stability: false,
    };
    let resp: Value = http
        .post(format!("{base}/sessions/{session_id}/action"))
        .bearer_auth(TOKEN)
        .json(&action_req)
        .send()
        .await
        .expect("action")
        .json()
        .await
        .expect("action json");
    assert_eq!(resp["ok"], true, "action ok");
    assert_eq!(
        resp["action_result"]["status"], "executed",
        "action executed"
    );
    assert_eq!(resp["action_result"]["kind"], "click_selector");
    assert!(resp["post_observation"].is_object(), "has post_observation");

    // 6. Action: fill.
    let action_req = ActionRequest {
        action_seq: 2,
        action: BrowserAction::Fill {
            selector: "#email".to_string(),
            value: "test@example.com".to_string(),
        },
        expected_result: "input filled".to_string(),
        timeout_ms: 10_000,
        capture_after: false,
        wait_for_stability: false,
    };
    let resp: Value = http
        .post(format!("{base}/sessions/{session_id}/action"))
        .bearer_auth(TOKEN)
        .json(&action_req)
        .send()
        .await
        .expect("fill action")
        .json()
        .await
        .expect("fill action json");
    assert_eq!(resp["ok"], true, "fill ok");
    assert_eq!(resp["action_result"]["status"], "executed");

    // 7. Screenshot/latest (metadata).
    let resp: Value = http
        .get(format!("{base}/sessions/{session_id}/screenshot/latest"))
        .bearer_auth(TOKEN)
        .query(&[("format", "metadata"), ("redacted", "false")])
        .send()
        .await
        .expect("screenshot metadata")
        .json()
        .await
        .expect("screenshot metadata json");
    assert_eq!(resp["ok"], true, "screenshot ok");
    assert!(
        resp["screenshot"]["artifact_uri"].as_str().is_some(),
        "has artifact_uri"
    );
    assert_eq!(resp["screenshot"]["mime_type"], "image/png");

    // 8. Screenshot/latest (binary).
    let resp = http
        .get(format!("{base}/sessions/{session_id}/screenshot/latest"))
        .bearer_auth(TOKEN)
        .query(&[("format", "binary"), ("redacted", "false")])
        .send()
        .await
        .expect("screenshot binary");
    assert!(resp.status().is_success(), "screenshot binary status");
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .expect("content-type");
    assert!(
        content_type.contains("image/png"),
        "binary content-type is image/png"
    );
    let bytes = resp.bytes().await.expect("screenshot bytes");
    assert!(!bytes.is_empty(), "screenshot bytes non-empty");
    // PNG signature.
    assert_eq!(
        &bytes[..8],
        &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
    );

    // 9. Debug/network.
    let resp: Value = http
        .get(format!("{base}/sessions/{session_id}/debug/network"))
        .bearer_auth(TOKEN)
        .query(&[("filter", "all"), ("limit", "10")])
        .send()
        .await
        .expect("debug network")
        .json()
        .await
        .expect("debug network json");
    assert_eq!(resp["ok"], true, "debug network ok");
    assert!(
        resp["network"]["items"].is_array(),
        "network items is array"
    );

    // 10. Debug/console.
    let resp: Value = http
        .get(format!("{base}/sessions/{session_id}/debug/console"))
        .bearer_auth(TOKEN)
        .query(&[("min_level", "warning"), ("limit", "10")])
        .send()
        .await
        .expect("debug console")
        .json()
        .await
        .expect("debug console json");
    assert_eq!(resp["ok"], true, "debug console ok");
    assert!(
        resp["console"]["items"].is_array(),
        "console items is array"
    );

    // 11. Close session.
    let close_req = CloseSessionRequest {
        purge_profile: true,
        keep_artifacts: true,
        reason: CloseReason::Done,
    };
    let resp: Value = http
        .delete(format!("{base}/sessions/{session_id}"))
        .bearer_auth(TOKEN)
        .json(&close_req)
        .send()
        .await
        .expect("close session")
        .json()
        .await
        .expect("close session json");
    assert_eq!(resp["ok"], true, "close ok");
    assert_eq!(resp["closed"], true);

    // 12. Verify session is gone (observe returns not_found).
    let resp: Value = http
        .get(format!("{base}/sessions/{session_id}/observe"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("observe after close")
        .json()
        .await
        .expect("observe after close json");
    assert_eq!(resp["ok"], false, "observe after close not ok");
    assert_eq!(resp["error"]["code"], "not_found");
}

#[tokio::test]
#[ignore = "requires Chromium binary"]
async fn goto_force_reload_works() {
    let base = start_server().await;
    let http = auth_client();

    // Create session.
    let create_req = CreateSessionRequest {
        task_id: "cp6-reload".to_string(),
        profile: BrowserProfile::Ephemeral,
        viewport: Viewport::default(),
        timezone: None,
        locale: None,
        record_console: false,
        record_network: false,
        allow_downloads: false,
        allow_uploads: false,
        start_url: Some(TEST_PAGE.to_string()),
    };
    let resp: Value = http
        .post(format!("{base}/sessions"))
        .bearer_auth(TOKEN)
        .json(&create_req)
        .send()
        .await
        .expect("create")
        .json()
        .await
        .expect("create json");
    assert_eq!(resp["ok"], true);
    let session_id = resp["session_id"].as_str().expect("session_id").to_string();

    // Goto with force_reload.
    let goto_req = GotoRequest {
        url: TEST_PAGE.to_string(),
        wait_until: WaitUntil::Load,
        timeout_ms: 30_000,
        action_seq: 1,
        capture_after: true,
        force_reload: true,
    };
    let resp: Value = http
        .post(format!("{base}/sessions/{session_id}/goto"))
        .bearer_auth(TOKEN)
        .json(&goto_req)
        .send()
        .await
        .expect("goto force_reload")
        .json()
        .await
        .expect("goto json");
    assert_eq!(resp["ok"], true, "force_reload ok");
    assert_eq!(resp["navigation"]["force_reload"], true);
    assert_eq!(resp["navigation"]["status"], "loaded");
    assert!(
        resp["observation"].is_object(),
        "has observation after reload"
    );

    // Cleanup.
    let close_req = CloseSessionRequest {
        purge_profile: true,
        keep_artifacts: true,
        reason: CloseReason::Done,
    };
    let _ = http
        .delete(format!("{base}/sessions/{session_id}"))
        .bearer_auth(TOKEN)
        .json(&close_req)
        .send()
        .await;
}

#[tokio::test]
#[ignore = "requires Chromium binary"]
async fn action_get_element_value_returns_result() {
    let base = start_server().await;
    let http = auth_client();

    let create_req = CreateSessionRequest {
        task_id: "cp6-value".to_string(),
        profile: BrowserProfile::Ephemeral,
        viewport: Viewport::default(),
        timezone: None,
        locale: None,
        record_console: false,
        record_network: false,
        allow_downloads: false,
        allow_uploads: false,
        start_url: Some(TEST_PAGE.to_string()),
    };
    let resp: Value = http
        .post(format!("{base}/sessions"))
        .bearer_auth(TOKEN)
        .json(&create_req)
        .send()
        .await
        .expect("create")
        .json()
        .await
        .expect("create json");
    let session_id = resp["session_id"].as_str().expect("session_id").to_string();

    // Fill the input first.
    let fill_req = ActionRequest {
        action_seq: 1,
        action: BrowserAction::Fill {
            selector: "#email".to_string(),
            value: "hello@example.com".to_string(),
        },
        expected_result: String::new(),
        timeout_ms: 10_000,
        capture_after: false,
        wait_for_stability: false,
    };
    let _ = http
        .post(format!("{base}/sessions/{session_id}/action"))
        .bearer_auth(TOKEN)
        .json(&fill_req)
        .send()
        .await;

    // Get element value.
    let get_value_req = ActionRequest {
        action_seq: 2,
        action: BrowserAction::GetElementValue {
            selector: "#email".to_string(),
        },
        expected_result: String::new(),
        timeout_ms: 10_000,
        capture_after: false,
        wait_for_stability: false,
    };
    let resp: Value = http
        .post(format!("{base}/sessions/{session_id}/action"))
        .bearer_auth(TOKEN)
        .json(&get_value_req)
        .send()
        .await
        .expect("get_element_value")
        .json()
        .await
        .expect("get_element_value json");
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["action_result"]["status"], "executed");
    let result = resp["action_result"]["result"].as_str().expect("result");
    assert!(
        result.contains("hello@example.com"),
        "result contains email"
    );

    // Cleanup.
    let _ = http
        .delete(format!("{base}/sessions/{session_id}"))
        .bearer_auth(TOKEN)
        .json(&CloseSessionRequest {
            purge_profile: true,
            keep_artifacts: true,
            reason: CloseReason::Done,
        })
        .send()
        .await;
}
