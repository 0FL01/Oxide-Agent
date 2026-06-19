//! Observation building — concurrent a11y + screenshot + URL/title + DOM.
//!
//! Replaces the Python sidecar's `build_observation` function. Runs the three
//! primary CDP commands (a11y snapshot, screenshot, URL/title eval) concurrently
//! on the single session WebSocket (~1.6x speedup vs sequential, verified in
//! CP0). DOM snapshot runs separately when `include_dom=true`.

use std::time::Duration;

use oxide_browser_contracts::{BrowserObservation, DomSnapshotNode, LoadingState};
use serde_json::Value;

use crate::capture;
use crate::cdp::CdpClient;
use crate::dom;
use crate::screenshot;
use crate::session::BrowserSession;
use crate::snapshot;

/// CDP timeout for URL/title eval.
const EVAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Build a full `BrowserObservation` from the current page state.
///
/// Runs a11y snapshot + screenshot + URL/title concurrently via `tokio::join!`,
/// then optionally captures DOM snapshot, drains network/console, merges into
/// history, and builds summaries.
///
/// When `fresh=false`, returns the cached last observation if available.
pub async fn build_observation(
    session: &BrowserSession,
    action_seq: u64,
    include_dom: bool,
    include_network: bool,
    include_console: bool,
    fresh: bool,
    max_debug_items: u32,
) -> BrowserObservation {
    // Return cached observation when not fresh.
    if !fresh && let Some(last) = session.last_observation() {
        return last;
    }

    let cdp = session.cdp().await;
    let capture = session.capture().await;
    let viewport = session.viewport;

    // Concurrent: a11y snapshot + screenshot + URL/title eval.
    let screenshot_id = format!("shot-{}-{}", session.id, session.next_screenshot_seq());

    let (snapshot_result, screenshot_result, url_title) = tokio::join!(
        snapshot::take_snapshot(&cdp),
        screenshot::capture_screenshot(&cdp, viewport, &session.artifact_root, &screenshot_id),
        get_url_title(&cdp),
    );

    let (screenshot_artifact, screenshot_bytes) = screenshot_result;

    // Store bytes in-memory for the binary screenshot endpoint (no disk I/O).
    session.set_latest_screenshot_bytes(screenshot_bytes);

    // Update session URL/title.
    let (url, title) = url_title;
    if !url.is_empty() {
        session.set_url(&url);
    }
    let effective_url = if url.is_empty() { session.url() } else { url };

    // Build a11y_summary from structured nodes (or empty on error).
    let (a11y_summary, title_from_snapshot): (Vec<Value>, Option<String>) = match &snapshot_result {
        Ok(result) => {
            let nodes: Vec<Value> = result
                .nodes
                .iter()
                .map(|node| serde_json::to_value(node).unwrap_or(Value::Null))
                .collect();
            let title = title_from_a11y(result);
            (nodes, title)
        }
        Err(_) => (Vec::new(), None),
    };

    let effective_title = if title.is_empty() {
        title_from_snapshot.unwrap_or_else(|| session.title())
    } else {
        session.set_title(&title);
        title
    };

    // DOM snapshot (if requested) — runs after concurrent batch.
    let (dom_snapshot, dom_snapshot_error) = if include_dom {
        dom::capture_dom_snapshot(&cdp).await
    } else {
        (Some(Vec::<DomSnapshotNode>::new()), None)
    };

    // Drain network + console from capture collector.
    // Network: from CDP Network.* events accumulated by the background loop.
    // Console: from Log.entryAdded (drain_console) + JS interceptor (drain_console_js).
    let net_items = capture.drain_network();
    let con_items: Vec<_> = capture::drain_console_js(&cdp)
        .await
        .into_iter()
        .chain(capture.drain_console().into_iter())
        .collect();

    // Merge into persistent history.
    session.merge_network_history(net_items, action_seq);
    session.merge_console_history(con_items, action_seq);

    // Build compact summaries scoped to the CURRENT action/page only. History
    // retains all actions (browser_debug with all_history exposes it), but the
    // observation summary deliberately reflects just this action so old errors
    // from earlier navigations do not drown the current page's diagnostics.
    // Repeats refresh their action_seq on merge, so a still-occurring error
    // re-surfaces in the current action rather than disappearing.
    let network_summary = if include_network {
        let history = session.network_history();
        let items: Vec<_> = history
            .iter()
            .filter(|(_, seq)| *seq == action_seq)
            .map(|(item, _)| item.clone())
            .collect();
        Some(capture::summarize_network(&items, max_debug_items as usize))
    } else {
        None
    };

    let console_summary = if include_console {
        let history = session.console_history();
        let items: Vec<_> = history
            .iter()
            .filter(|(_, seq)| *seq == action_seq)
            .map(|(item, _)| item.clone())
            .collect();
        Some(capture::summarize_console(&items, max_debug_items as usize))
    } else {
        None
    };

    let observation_seq = session.next_observation_seq();
    let observation = BrowserObservation {
        observation_id: format!("obs-{}-{}", session.id, observation_seq),
        action_seq,
        captured_at: capture::now_iso(),
        url: effective_url,
        title: effective_title,
        viewport,
        loading_state: LoadingState::Idle,
        screenshot: screenshot_artifact,
        a11y_summary,
        dom_snapshot: dom_snapshot.unwrap_or_default(),
        dom_snapshot_error,
        network_summary,
        console_summary,
    };

    session.set_last_observation(observation.clone());
    observation
}

/// Get URL and title via a single `Runtime.evaluate` call.
///
/// Returns `(url, title)` — empty strings on failure.
async fn get_url_title(cdp: &CdpClient) -> (String, String) {
    let expr = r#"JSON.stringify({url: window.location.href || document.URL || '', title: document.title || ''})"#;
    let result = cdp
        .send_command(
            "Runtime.evaluate",
            serde_json::json!({
                "expression": expr,
                "returnByValue": true,
                "awaitPromise": true,
            }),
            EVAL_TIMEOUT,
        )
        .await;

    let json_str = match result {
        Ok(resp) => resp
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        Err(_) => return (String::new(), String::new()),
    };

    let parsed: Value = serde_json::from_str(&json_str).unwrap_or(Value::Null);
    let url = parsed
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let title = parsed
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    (url, title)
}

/// Extract page title from a11y `RootWebArea` role text.
///
/// Matches the Python sidecar's `_title_from_a11y` fallback.
fn title_from_a11y(result: &snapshot::SnapshotResult) -> Option<String> {
    result.nodes.iter().find_map(|node| {
        if node.role == "RootWebArea" && !node.text.is_empty() {
            Some(node.text.clone())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_from_a11y_finds_root_web_area() {
        let result = snapshot::SnapshotResult {
            nodes: vec![
                snapshot::A11yNode {
                    uid: "n1".to_string(),
                    role: "RootWebArea".to_string(),
                    text: "My Page Title".to_string(),
                    depth: 0,
                },
                snapshot::A11yNode {
                    uid: "n2".to_string(),
                    role: "button".to_string(),
                    text: "Click".to_string(),
                    depth: 1,
                },
            ],
            text: String::new(),
            uid_to_backend: std::collections::HashMap::new(),
        };
        assert_eq!(title_from_a11y(&result), Some("My Page Title".to_string()));
    }

    #[test]
    fn title_from_a11y_returns_none_when_no_root() {
        let result = snapshot::SnapshotResult {
            nodes: vec![snapshot::A11yNode {
                uid: "n1".to_string(),
                role: "button".to_string(),
                text: "Click".to_string(),
                depth: 0,
            }],
            text: String::new(),
            uid_to_backend: std::collections::HashMap::new(),
        };
        assert_eq!(title_from_a11y(&result), None);
    }

    #[test]
    fn title_from_a11y_returns_none_when_empty_text() {
        let result = snapshot::SnapshotResult {
            nodes: vec![snapshot::A11yNode {
                uid: "n1".to_string(),
                role: "RootWebArea".to_string(),
                text: String::new(),
                depth: 0,
            }],
            text: String::new(),
            uid_to_backend: std::collections::HashMap::new(),
        };
        assert_eq!(title_from_a11y(&result), None);
    }
}
