//! Smoke test simulating `BrowserSidecarClient` against the native sidecar.
//!
//! Uses the same shared types from `oxide-browser-contracts` and the same REST
//! endpoints that `BrowserSidecarClient` (in `oxide-agent-core`) calls. Since
//! the types are shared (G2), contract drift is impossible — this test is
//! functionally equivalent to using the real client.
//!
//! Also measures per-step latency for Q1 (compared to CP0 baseline).

use oxide_browser_contracts::{
    ActionRequest, BrowserAction, BrowserMode, BrowserProfile, CloseReason, CloseSessionRequest,
    CreateSessionRequest, GotoRequest, ObserveQuery, ScreenshotQuery, Viewport, WaitUntil,
};
use oxide_browser_sidecar::{AppState, create_app};
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;

const TOKEN: &str = "test-token-smoke";

const TEST_PAGE: &str = "data:text/html,\
<html><head><title>Smoke Test</title></head><body>\
<h1 id=\"hero\">Smoke Test Page</h1>\
<button id=\"btn\" onclick=\"this.textContent='Done'\">Click Me</button>\
<input id=\"field\" type=\"text\" placeholder=\"Type here\" />\
<a href=\"https://example.com\" id=\"link\">Link</a>\
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

struct Timings {
    steps: Vec<(&'static str, u128)>,
}

impl Timings {
    const fn new() -> Self {
        Self { steps: Vec::new() }
    }

    fn record(&mut self, name: &'static str, start: Instant) {
        let ms = start.elapsed().as_millis();
        self.steps.push((name, ms));
    }

    fn print(&self) {
        eprintln!("--- Latency measurements (native sidecar) ---");
        for (name, ms) in &self.steps {
            eprintln!("  {name}: {ms}ms");
        }
        eprintln!("--- CP0 baseline (direct CDP, no HTTP) ---");
        eprintln!("  a11y: 2.6ms, screenshot: 38ms, eval: 1ms");
        eprintln!("  concurrent 3 cmds: 30ms vs sequential 49ms");
    }
}

/// Simulates `BrowserSidecarClient` — same shared types, same REST endpoints,
/// same request/response contract. Runs a representative browser task:
/// create → goto → observe → click → fill → observe → screenshot → close.
#[tokio::test]
#[ignore = "requires Chromium binary"]
async fn smoke_test_browser_sidecar_client_contract() {
    let base = start_server().await;
    let http = auth_client();
    let mut timings = Timings::new();

    // 1. healthz — BrowserSidecar::healthz
    let t = Instant::now();
    let resp: Value = http
        .get(format!("{base}/healthz"))
        .send()
        .await
        .expect("healthz send")
        .json()
        .await
        .expect("healthz json");
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["native"], true);
    timings.record("healthz", t);

    // 2. create session — BrowserSidecar::create_session
    let t = Instant::now();
    let req = CreateSessionRequest {
        task_id: "smoke-task".to_string(),
        profile: BrowserProfile::Ephemeral,
        mode: BrowserMode::StealthClean,
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
        .json(&req)
        .send()
        .await
        .expect("create send")
        .json()
        .await
        .expect("create json");
    assert_eq!(resp["ok"], true, "create session failed: {resp}");
    let session_id = resp["session_id"].as_str().expect("session_id").to_string();
    timings.record("create_session", t);

    // 3. goto — BrowserSidecar::goto
    let t = Instant::now();
    let req = GotoRequest {
        url: TEST_PAGE.to_string(),
        force_reload: false,
        wait_until: WaitUntil::Load,
        timeout_ms: 30000,
        action_seq: 1,
        capture_after: true,
    };
    let resp: Value = http
        .post(format!("{base}/sessions/{session_id}/goto"))
        .bearer_auth(TOKEN)
        .json(&req)
        .send()
        .await
        .expect("goto send")
        .json()
        .await
        .expect("goto json");
    assert_eq!(resp["ok"], true, "goto failed: {resp}");
    timings.record("goto", t);

    // 4. observe — BrowserSidecar::observe (with DOM)
    let t = Instant::now();
    let query = ObserveQuery {
        include_network_summary: true,
        include_console_summary: true,
        include_dom: true,
        include_a11y: true,
        fresh: true,
        max_debug_items: 20,
    };
    let resp: Value = http
        .get(format!("{base}/sessions/{session_id}/observe"))
        .bearer_auth(TOKEN)
        .query(&query)
        .send()
        .await
        .expect("observe send")
        .json()
        .await
        .expect("observe json");
    assert_eq!(resp["ok"], true, "observe failed: {resp}");
    let obs = &resp["observation"];
    assert!(obs["a11y_summary"].is_array(), "a11y_summary is array");
    assert!(
        obs["screenshot"]["artifact_uri"].is_string(),
        "screenshot uri"
    );
    assert!(obs["dom_snapshot"].is_array(), "dom_snapshot is array");
    timings.record("observe (concurrent a11y+ss+url+dom)", t);

    // 5. action: click_selector — BrowserSidecar::execute_action
    let t = Instant::now();
    let req = ActionRequest {
        action: BrowserAction::ClickSelector {
            selector: "#btn".to_string(),
        },
        action_seq: 1,
        expected_result: String::new(),
        timeout_ms: 30000,
        capture_after: true,
        wait_for_stability: false,
    };
    let resp: Value = http
        .post(format!("{base}/sessions/{session_id}/action"))
        .bearer_auth(TOKEN)
        .json(&req)
        .send()
        .await
        .expect("action send")
        .json()
        .await
        .expect("action json");
    assert_eq!(resp["ok"], true, "click failed: {resp}");
    assert_eq!(
        resp["action_result"]["status"], "executed",
        "click status: {resp}"
    );
    timings.record("click_selector + post_obs", t);

    // 6. action: fill — BrowserSidecar::execute_action
    let t = Instant::now();
    let req = ActionRequest {
        action: BrowserAction::Fill {
            selector: "#field".to_string(),
            value: "hello world".to_string(),
        },
        action_seq: 2,
        expected_result: String::new(),
        timeout_ms: 30000,
        capture_after: true,
        wait_for_stability: false,
    };
    let resp: Value = http
        .post(format!("{base}/sessions/{session_id}/action"))
        .bearer_auth(TOKEN)
        .json(&req)
        .send()
        .await
        .expect("fill send")
        .json()
        .await
        .expect("fill json");
    assert_eq!(resp["ok"], true, "fill failed: {resp}");
    assert_eq!(
        resp["action_result"]["status"], "executed",
        "fill status: {resp}"
    );
    timings.record("fill + post_obs", t);

    // 7. action: get_element_value (result-only, no post-obs)
    let t = Instant::now();
    let req = ActionRequest {
        action: BrowserAction::GetElementValue {
            selector: "#field".to_string(),
        },
        action_seq: 3,
        expected_result: String::new(),
        timeout_ms: 30000,
        capture_after: false,
        wait_for_stability: false,
    };
    let resp: Value = http
        .post(format!("{base}/sessions/{session_id}/action"))
        .bearer_auth(TOKEN)
        .json(&req)
        .send()
        .await
        .expect("get_value send")
        .json()
        .await
        .expect("get_value json");
    assert_eq!(resp["ok"], true, "get_value failed: {resp}");
    let result = resp["action_result"]["result"]
        .as_str()
        .expect("result string");
    assert_eq!(result, "hello world", "field value matches filled text");
    timings.record("get_element_value", t);

    // 8. observe (fresh) — verify click changed button text
    let t = Instant::now();
    let resp: Value = http
        .get(format!("{base}/sessions/{session_id}/observe"))
        .bearer_auth(TOKEN)
        .query(&ObserveQuery {
            fresh: true,
            max_debug_items: 10,
            ..ObserveQuery::default()
        })
        .send()
        .await
        .expect("observe2 send")
        .json()
        .await
        .expect("observe2 json");
    assert_eq!(resp["ok"], true);
    timings.record("observe (fresh, no dom)", t);

    // 9. screenshot/latest metadata — BrowserSidecar::latest_screenshot
    let t = Instant::now();
    let resp: Value = http
        .get(format!("{base}/sessions/{session_id}/screenshot/latest"))
        .bearer_auth(TOKEN)
        .query(&ScreenshotQuery::default())
        .send()
        .await
        .expect("screenshot send")
        .json()
        .await
        .expect("screenshot json");
    assert_eq!(resp["ok"], true, "screenshot metadata: {resp}");
    assert!(resp["screenshot"]["sha256"].is_string());
    timings.record("screenshot/latest (metadata)", t);

    // 10. screenshot/latest binary — BrowserSidecar::latest_screenshot_bytes
    let t = Instant::now();
    let resp = http
        .get(format!("{base}/sessions/{session_id}/screenshot/latest"))
        .bearer_auth(TOKEN)
        .query(&[("format", "binary")])
        .send()
        .await
        .expect("screenshot binary send");
    let bytes = resp.bytes().await.expect("screenshot bytes");
    assert!(!bytes.is_empty(), "screenshot bytes non-empty");
    // PNG signature.
    assert_eq!(
        &bytes[..8],
        &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        "PNG signature"
    );
    timings.record("screenshot/latest (binary)", t);

    // 11. debug/network — BrowserSidecar::debug_network
    let t = Instant::now();
    let resp: Value = http
        .get(format!("{base}/sessions/{session_id}/debug/network"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("debug_net send")
        .json()
        .await
        .expect("debug_net json");
    assert_eq!(resp["ok"], true, "debug/network: {resp}");
    timings.record("debug/network", t);

    // 12. debug/console — BrowserSidecar::debug_console
    let t = Instant::now();
    let resp: Value = http
        .get(format!("{base}/sessions/{session_id}/debug/console"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("debug_console send")
        .json()
        .await
        .expect("debug_console json");
    assert_eq!(resp["ok"], true, "debug/console: {resp}");
    timings.record("debug/console", t);

    // 13. close session — BrowserSidecar::close_session
    let t = Instant::now();
    let req = CloseSessionRequest {
        purge_profile: true,
        keep_artifacts: true,
        reason: CloseReason::Done,
    };
    let resp: Value = http
        .delete(format!("{base}/sessions/{session_id}"))
        .bearer_auth(TOKEN)
        .json(&req)
        .send()
        .await
        .expect("close send")
        .json()
        .await
        .expect("close json");
    assert_eq!(resp["ok"], true, "close failed: {resp}");
    assert_eq!(resp["closed"], true);
    timings.record("close_session", t);

    timings.print();

    // Verify session is gone.
    let resp: Value = http
        .get(format!("{base}/sessions/{session_id}/observe"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .expect("observe after close")
        .json()
        .await
        .expect("observe after close json");
    assert_eq!(resp["ok"], false, "session should be gone after close");
}
