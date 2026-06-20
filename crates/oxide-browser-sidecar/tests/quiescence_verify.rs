//! Integration test: page quiescence gate catches SPA post-navigate renders.
//!
//! Verifies that `goto` with `capture_after=true` on an SPA page where
//! interactive elements appear after `Page.loadEventFired` returns an
//! observation containing those elements — without any manual
//! `wait_for_selector`. The quiescence gate (DOM-stable + network-idle) inside
//! `build_observation` handles the wait.
//!
//! Run: cargo test -p oxide-browser-sidecar --test quiescence_verify -- --ignored --nocapture

use oxide_browser_contracts::{
    BrowserProfile, CreateSessionRequest, GotoRequest, Viewport, WaitUntil,
};
use oxide_browser_sidecar::{AppState, create_app};
use serde_json::Value;
use std::sync::Arc;
use tokio::net::TcpListener;

const TOKEN: &str = "test-token-quiescence";

/// SPA page: shell renders at load, form (textarea + button) appears 500ms
/// later via setTimeout. This simulates the race the user reported —
/// `Page.loadEventFired` fires before the SPA renders interactive elements.
const SPA_SET_TIMEOUT_PAGE: &str = "data:text/html,\
<!DOCTYPE html>\
<html><head><title>SPA Quiescence Test</title></head>\
<body>\
<div id='app'><h1>Loading...</h1></div>\
<script>\
setTimeout(function() {\
  document.getElementById('app').innerHTML =\
  '<h1>Contact Form</h1>' +\
  '<textarea id=\"msg\" placeholder=\"Message\"></textarea>' +\
  '<button id=\"submit\">Submit</button>';\
}, 500);\
</script>\
</body></html>";

/// Static page used as session start_url so the SPA page is only loaded via
/// `/goto`, ensuring `loadEventFired` + quiescence is tested fresh.
const STATIC_START_PAGE: &str = "data:text/html,<html><body><h1>Start</h1></body></html>";

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

/// Create a session and return its ID.
async fn create_session(base: &str, http: &reqwest::Client) -> String {
    let create_req = CreateSessionRequest {
        task_id: "quiescence-test".to_string(),
        profile: BrowserProfile::Ephemeral,
        viewport: Viewport::default(),
        timezone: None,
        locale: None,
        record_console: true,
        record_network: true,
        allow_downloads: false,
        allow_uploads: false,
        start_url: Some(STATIC_START_PAGE.to_string()),
    };
    let resp: Value = http
        .post(format!("{base}/sessions"))
        .header("authorization", format!("Bearer {TOKEN}"))
        .json(&create_req)
        .send()
        .await
        .expect("create session")
        .json()
        .await
        .expect("create session json");
    resp["session_id"].as_str().expect("session_id").to_string()
}

/// Navigate to a URL with `capture_after=true` and return the observation.
async fn goto_with_capture(
    base: &str,
    http: &reqwest::Client,
    session_id: &str,
    url: &str,
) -> Value {
    let req = GotoRequest {
        url: url.to_string(),
        wait_until: WaitUntil::Load,
        timeout_ms: 15_000,
        capture_after: true,
        force_reload: false,
    };
    let resp: Value = http
        .post(format!("{base}/sessions/{session_id}/goto"))
        .header("authorization", format!("Bearer {TOKEN}"))
        .json(&req)
        .send()
        .await
        .expect("goto")
        .json()
        .await
        .expect("goto json");
    resp["observation"].clone()
}

/// Count interactive elements in `dom_snapshot` matching a tag name.
fn count_dom_elements(observation: &Value, tag: &str) -> usize {
    observation["dom_snapshot"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter(|n| n["tag"].as_str() == Some(tag))
                .count()
        })
        .unwrap_or(0)
}

/// Verify: goto to an SPA page where form appears 500ms after load returns
/// an observation with textarea and button — without wait_for_selector.
///
/// Run multiple iterations to catch intermittent races.
#[tokio::test]
#[ignore = "requires Chromium binary"]
async fn quiescence_catches_spa_post_navigate_render() {
    let base = start_server().await;
    let http = auth_client();

    for i in 0..3 {
        let session_id = create_session(&base, &http).await;
        let observation = goto_with_capture(&base, &http, &session_id, SPA_SET_TIMEOUT_PAGE).await;

        let textarea_count = count_dom_elements(&observation, "textarea");
        let button_count = count_dom_elements(&observation, "button");

        println!("[iteration {i}] textarea={textarea_count}, button={button_count}");
        println!(
            "[iteration {i}] dom_snapshot: {}",
            serde_json::to_string_pretty(&observation["dom_snapshot"]).unwrap_or_default()
        );

        assert!(
            textarea_count >= 1,
            "iteration {i}: textarea missing — quiescence gate did not wait for SPA render"
        );
        assert!(
            button_count >= 1,
            "iteration {i}: button missing — quiescence gate did not wait for SPA render"
        );

        // Clean up.
        let _ = http
            .post(format!("{base}/sessions/{session_id}/close"))
            .header("authorization", format!("Bearer {TOKEN}"))
            .json(&serde_json::json!({"reason": "test_complete"}))
            .send()
            .await;
    }
}
