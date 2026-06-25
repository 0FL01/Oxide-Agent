//! Integration test: a11y snapshot + stealth patches on real Chromium.
//!
//! Run with: `cargo test -p oxide-browser-sidecar --test snapshot_stealth -- --ignored --nocapture`

use std::time::Duration;

use oxide_browser_contracts::Viewport;
use oxide_browser_sidecar::browser::ChromiumProcess;
use oxide_browser_sidecar::session::navigate_to;
use oxide_browser_sidecar::snapshot::{self, A11yNode};

const TIMEOUT: Duration = Duration::from_secs(30);

/// Test page with a heading, button, link, and input.
const TEST_PAGE: &str = "data:text/html,\
<!DOCTYPE html>\
<html><head><title>Test Page</title></head>\
<body>\
<h1>Welcome</h1>\
<button id=\"btn\" onclick=\"this.textContent='Clicked'\">Login</button>\
<a href=\"#home\">Home</a>\
<input type=\"text\" id=\"name\" value=\"hello\" placeholder=\"Name\">\
<div><p>Some text</p></div>\
</body></html>";

#[tokio::test]
#[ignore = "requires Chromium binary"]
async fn snapshot_and_stealth_on_real_chromium() {
    let viewport = Viewport {
        width: 1280,
        height: 720,
        device_scale_factor: 1.0,
    };

    // Launch Chromium + connect CDP.
    let (mut chromium, cdp) = ChromiumProcess::launch(&viewport)
        .await
        .expect("launch Chromium");

    // Navigate with stealth patches.
    navigate_to(&cdp, TEST_PAGE, TIMEOUT, true)
        .await
        .expect("navigate to test page");

    // ── Verify stealth patches ──────────────────────────────────────

    // navigator.webdriver should be undefined.
    let webdriver_result = cdp
        .send_command(
            "Runtime.evaluate",
            serde_json::json!({
                "expression": "navigator.webdriver",
                "returnByValue": true
            }),
            TIMEOUT,
        )
        .await
        .expect("evaluate navigator.webdriver");

    let webdriver_value = webdriver_result.get("result").and_then(|r| r.get("value"));
    assert!(
        webdriver_value.is_none() || webdriver_value == Some(&serde_json::Value::Null),
        "navigator.webdriver should be undefined/null, got: {webdriver_value:?}"
    );

    // User-Agent should NOT contain "HeadlessChrome".
    let ua_result = cdp
        .send_command(
            "Runtime.evaluate",
            serde_json::json!({
                "expression": "navigator.userAgent",
                "returnByValue": true
            }),
            TIMEOUT,
        )
        .await
        .expect("evaluate navigator.userAgent");

    let ua = ua_result
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_str())
        .expect("user-agent string");
    assert!(
        !ua.contains("HeadlessChrome"),
        "UA should not contain HeadlessChrome, got: {ua}"
    );

    // ── Verify a11y snapshot ────────────────────────────────────────

    let snapshot_result = snapshot::take_snapshot(&cdp)
        .await
        .expect("take a11y snapshot");

    // Should have at least a few nodes (WebArea, heading, button, link, textbox).
    assert!(
        snapshot_result.nodes.len() >= 3,
        "expected at least 3 a11y nodes, got {}: {:#?}",
        snapshot_result.nodes.len(),
        snapshot_result.nodes
    );

    // Find the heading "Welcome".
    let has_heading = snapshot_result
        .nodes
        .iter()
        .any(|n| n.role == "heading" && n.text.contains("Welcome"));
    assert!(
        has_heading,
        "snapshot should contain heading 'Welcome': {:#?}",
        snapshot_result.nodes
    );

    // Find the button "Login".
    let has_button = snapshot_result
        .nodes
        .iter()
        .any(|n| n.role == "button" && n.text.contains("Login"));
    assert!(
        has_button,
        "snapshot should contain button 'Login': {:#?}",
        snapshot_result.nodes
    );

    // Verify UID stability: nodes with backendDOMNodeId should have "n" prefix.
    let has_stable_uid = snapshot_result.nodes.iter().any(|n| n.uid.starts_with('n'));
    assert!(
        has_stable_uid,
        "at least one node should have stable UID (n prefix): {:#?}",
        snapshot_result.nodes
    );

    // Verify uid_to_backend map is populated.
    assert!(
        !snapshot_result.uid_to_backend.is_empty(),
        "uid_to_backend map should be populated"
    );

    // ── Verify UID stability across two snapshots ──────────────────

    let snapshot2 = snapshot::take_snapshot(&cdp)
        .await
        .expect("second a11y snapshot");

    // Nodes with n-prefix UIDs should be stable across snapshots.
    let stable_nodes_1: Vec<&A11yNode> = snapshot_result
        .nodes
        .iter()
        .filter(|n| n.uid.starts_with('n'))
        .collect();
    let stable_nodes_2: Vec<&A11yNode> = snapshot2
        .nodes
        .iter()
        .filter(|n| n.uid.starts_with('n'))
        .collect();

    // Same number of stable nodes.
    assert_eq!(
        stable_nodes_1.len(),
        stable_nodes_2.len(),
        "stable node count should match across snapshots"
    );

    // Same UIDs.
    let uids_1: Vec<&str> = stable_nodes_1.iter().map(|n| n.uid.as_str()).collect();
    let uids_2: Vec<&str> = stable_nodes_2.iter().map(|n| n.uid.as_str()).collect();
    assert_eq!(uids_1, uids_2, "stable UIDs should match across snapshots");

    // ── Verify text format ─────────────────────────────────────────

    assert!(
        snapshot_result.text.contains("uid=n"),
        "text format should contain uid=n entries: {}",
        snapshot_result.text
    );
    assert!(
        snapshot_result.text.contains("heading"),
        "text format should contain heading role: {}",
        snapshot_result.text
    );

    // Cleanup.
    chromium.shutdown().await.expect("shutdown Chromium");
}
