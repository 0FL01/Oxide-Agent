//! Integration test for network + console capture on real Chromium.
//!
//! Verifies:
//! - JS console.warn/error captured via injected interceptor (no Runtime.enable)
//! - console.log/info NOT captured (only Warning/Error in contract)
//! - Network requests captured via Network.enable
//! - favicon.ico noise classified as benign diagnostics
//! - Capture collector starts and runs on the same CDP WebSocket

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::get;
use oxide_browser_contracts::{ConsoleLevel, DiagnosticScope, Viewport};
use oxide_browser_sidecar::browser::ChromiumProcess;
use oxide_browser_sidecar::capture::{self, CaptureCollector};
use oxide_browser_sidecar::session::navigate_to;
use tokio::net::TcpListener;

/// Test page with console.warn/error/log and a failed fetch request.
///
/// Uses `https://invalid.invalid/` for the fetch — guaranteed DNS failure,
/// no external dependency. The `<link rel="icon" href="favicon.ico">` tests
/// benign diagnostic classification.
const TEST_PAGE: &str = "data:text/html,\
<!DOCTYPE html>\
<html><head><link rel=\"icon\" href=\"favicon.ico\"></head>\
<body><h1>Capture Test</h1>\
<script>\
console.warn(\"test warning from JS\");\
console.error(\"test error from JS\");\
console.log(\"test info should be filtered\");\
fetch(\"https://invalid.invalid/api/test\").catch(function(){});\
</script>\
</body></html>";

async fn mixed_page() -> Html<&'static str> {
    Html(
        r#"<!DOCTYPE html>
<html><head><link rel="icon" href="/favicon.ico"></head>
<body><h1>Mixed Capture Test</h1>
<script>
console.warn("mixed page warning");
console.error("mixed page error");
fetch("/api/isWritable").catch(function(){});
fetch("https://invalid.invalid/third-party-ad.js").catch(function(){});
</script>
</body></html>"#,
    )
}

async fn is_writable() -> (StatusCode, &'static str) {
    (StatusCode::NOT_FOUND, "not writable")
}

async fn favicon() -> (StatusCode, &'static str) {
    (StatusCode::NOT_FOUND, "")
}

#[tokio::test]
#[ignore = "requires Chromium installed"]
async fn capture_network_and_console_on_real_chromium() {
    let viewport = Viewport {
        width: 1280,
        height: 720,
        device_scale_factor: 1.0,
    };

    // Launch Chromium and get CDP client.
    let (mut chromium, cdp) = ChromiumProcess::launch(&viewport)
        .await
        .expect("launch Chromium");

    // Start capture collector before navigation.
    let collector = Arc::new(CaptureCollector::new());
    CaptureCollector::start(&cdp, collector.clone())
        .await
        .expect("start capture");

    // Navigate to the test page (with stealth — also injects interceptor).
    navigate_to(&cdp, TEST_PAGE, Duration::from_secs(30), true)
        .await
        .expect("navigate");

    // Wait for events to settle (fetch DNS failure + console calls).
    tokio::time::sleep(Duration::from_millis(500)).await;

    // ── Console capture (JS interceptor) ───────────────────────────────
    let js_console = capture::drain_console_js(&cdp).await;

    // Should have warning and error, but NOT info.
    let has_warning = js_console
        .iter()
        .any(|c| c.level == ConsoleLevel::Warning && c.text_redacted.contains("test warning"));
    let has_error = js_console
        .iter()
        .any(|c| c.level == ConsoleLevel::Error && c.text_redacted.contains("test error"));
    let has_info = js_console
        .iter()
        .any(|c| c.text_redacted.contains("test info should be filtered"));

    assert!(
        has_warning,
        "JS console warning should be captured: {js_console:?}"
    );
    assert!(
        has_error,
        "JS console error should be captured: {js_console:?}"
    );
    assert!(
        !has_info,
        "JS console.info should be filtered (only Warning/Error in contract): {js_console:?}"
    );

    // ── Console capture (Log.entryAdded) ───────────────────────────────
    let log_console = collector.drain_console();
    // Log.entryAdded may or may not capture JS console messages depending on
    // Chrome version. We just verify it doesn't crash and returns a Vec.
    // The JS interceptor is the primary console capture mechanism.
    let _ = log_console;

    // ── Network capture ────────────────────────────────────────────────
    let network = collector.drain_network();

    // The fetch to invalid.invalid should produce a failed network item.
    let has_failed_fetch = network
        .iter()
        .any(|n| n.url_redacted.contains("invalid.invalid") && n.error_text.is_some());
    assert!(
        has_failed_fetch,
        "failed fetch to invalid.invalid should be captured: {network:?}"
    );

    // favicon.ico may or may not be requested by Chromium for data URLs. If it
    // is requested, capture keeps it as a benign diagnostic instead of dropping
    // it, so compact summaries/debug defaults can suppress it by typed scope.
    for item in network
        .iter()
        .filter(|n| n.url_redacted.ends_with("/favicon.ico"))
    {
        assert_eq!(
            item.scope,
            DiagnosticScope::Benign,
            "favicon.ico should be classified as benign: {network:?}"
        );
    }

    // ── Current URL tracking ───────────────────────────────────────────
    // For data URLs, frameNavigated may not update current_url (data URLs
    // are sometimes considered internal). Just verify it doesn't crash.
    let _ = collector.current_url();

    // ── Verify no Runtime.enable was sent ──────────────────────────────
    // This is verified by source-level grep in the test suite, not at runtime.
    // The fact that the JS interceptor works (console.warn/error captured)
    // proves Runtime.enable was not needed.

    // Cleanup.
    chromium.shutdown().await.expect("shutdown");
}

#[tokio::test]
#[ignore = "requires Chromium installed"]
async fn capture_summarize_and_debug_payloads() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local test server");
    let addr = listener.local_addr().expect("local server addr");
    let app = Router::new()
        .route("/", get(mixed_page))
        .route("/api/isWritable", get(is_writable))
        .route("/favicon.ico", get(favicon));
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve local test server");
    });

    let viewport = Viewport {
        width: 1280,
        height: 720,
        device_scale_factor: 1.0,
    };

    let (mut chromium, cdp) = ChromiumProcess::launch(&viewport)
        .await
        .expect("launch Chromium");

    let collector = Arc::new(CaptureCollector::new());
    CaptureCollector::start(&cdp, collector.clone())
        .await
        .expect("start capture");

    let page_url = format!("http://{addr}/");
    navigate_to(&cdp, &page_url, Duration::from_secs(30), true)
        .await
        .expect("navigate");

    tokio::time::sleep(Duration::from_millis(700)).await;

    // Drain and merge into history.
    let network_items = collector.drain_network();
    let console_items = capture::drain_console_js(&cdp).await;

    let mut net_history = Vec::new();
    capture::merge_network_history(&mut net_history, network_items, 1);

    let mut console_history = Vec::new();
    capture::merge_console_history(&mut console_history, console_items, 1);

    // Build summaries.
    let net_summary = capture::summarize_network(
        &net_history
            .iter()
            .map(|(i, _)| i.clone())
            .collect::<Vec<_>>(),
        20,
    );
    assert!(
        net_summary.request_count > 0,
        "should have network requests"
    );
    assert!(
        net_summary.recent_failures.iter().any(|item| {
            item.url_redacted.contains("/api/isWritable")
                && item.status == Some(404)
                && item.scope == DiagnosticScope::FirstParty
        }),
        "first-party /api/isWritable 404 should be surfaced: {net_summary:?}"
    );
    assert!(
        net_summary
            .recent_failures
            .iter()
            .all(|item| !item.url_redacted.contains("invalid.invalid")),
        "third-party failure should be suppressed from compact summary: {net_summary:?}"
    );
    assert!(
        net_summary.suppressed.third_party > 0,
        "third-party diagnostics should be counted as suppressed: {net_summary:?}"
    );

    let console_summary = capture::summarize_console(
        &console_history
            .iter()
            .map(|(i, _)| i.clone())
            .collect::<Vec<_>>(),
        20,
    );
    assert!(
        console_summary.error_count > 0 || console_summary.warning_count > 0,
        "should have console errors or warnings"
    );
    assert!(
        console_summary
            .recent_errors
            .iter()
            .any(|item| item.text_redacted.contains("mixed page error")),
        "site console error should be surfaced: {console_summary:?}"
    );

    // Build debug payloads.
    let net_debug = capture::build_network_debug_payload(
        &net_history,
        0,
        oxide_browser_contracts::NetworkFilter::All,
        false,
        false,
        10,
    );
    assert!(!net_debug.items.is_empty());
    assert!(
        net_debug
            .items
            .iter()
            .any(|item| item.url_redacted.contains("/api/isWritable")),
        "default debug should include surfaced first-party failure: {net_debug:?}"
    );
    assert!(
        net_debug
            .items
            .iter()
            .all(|item| !item.url_redacted.contains("invalid.invalid")),
        "default debug should hide suppressed third-party failure: {net_debug:?}"
    );

    let full_net_debug = capture::build_network_debug_payload(
        &net_history,
        0,
        oxide_browser_contracts::NetworkFilter::All,
        true,
        false,
        20,
    );
    assert!(
        full_net_debug
            .items
            .iter()
            .any(|item| item.url_redacted.contains("invalid.invalid")),
        "include_suppressed debug should reveal third-party failure: {full_net_debug:?}"
    );

    let console_debug =
        capture::build_console_debug_payload(&console_history, 0, ConsoleLevel::Warning, false, 10);
    assert!(!console_debug.items.is_empty());

    chromium.shutdown().await.expect("shutdown");
    server.abort();
}
