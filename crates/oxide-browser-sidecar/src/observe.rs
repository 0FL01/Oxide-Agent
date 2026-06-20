//! Observation building — concurrent a11y + screenshot + URL/title + DOM.
//!
//! Replaces the Python sidecar's `build_observation` function. Runs the three
//! primary CDP commands (a11y snapshot, screenshot, URL/title eval) concurrently
//! on the single session WebSocket (~1.6x speedup vs sequential, verified in
//! CP0). DOM snapshot runs separately when `include_dom=true`.
//!
//! Before any capture, [`wait_for_page_quiescence`] waits for DOM stability +
//! network idle so SPA post-navigate snapshots are not racy.

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

/// Maximum time to wait for page quiescence before proceeding with observation.
///
/// Safety valve for pages with constant activity (SSE, long-polling, DOM
/// mutations from timers). On timeout the observation proceeds with the current
/// state rather than hanging.
const QUIESCENCE_TIMEOUT: Duration = Duration::from_secs(3);

/// Duration the DOM fingerprint and network pending count must remain unchanged
/// before the page is considered quiescent.
///
/// 500ms catches SPA renders that happen shortly after `Page.loadEventFired`.
/// Verified on real Chromium: a setTimeout(500)-based form render changes the
/// fingerprint at T+500, resetting the timer before the quiet window elapses.
const QUIESCENCE_QUIET_WINDOW: Duration = Duration::from_millis(500);

/// Poll interval for quiescence checks.
const QUIESCENCE_POLL: Duration = Duration::from_millis(50);

/// JavaScript expression that produces a compact DOM fingerprint.
///
/// Returns a JSON string with three integer fields:
/// - `els`: total element count
/// - `interactive`: count of interactive elements (same selectors as DOM snapshot)
/// - `text`: `document.body.textContent.length`
///
/// Two fingerprints are equal iff the DOM has not structurally changed between
/// samples. This is a structural signal, not a text search — it detects SPA
/// renders, route changes, and dynamic content injection without knowing
/// page-specific selectors.
const DOM_FINGERPRINT_EXPR: &str = r#"JSON.stringify({
  els: document.getElementsByTagName('*').length,
  interactive: document.querySelectorAll('a,button,input,textarea,select,[role="button"],[role="link"],[data-clipboard-text]').length,
  text: (document.body && document.body.textContent) ? document.body.textContent.length : 0
})"#;

/// Whether the page is quiescent given the current fingerprint, the last
/// fingerprint, and the pending network request count.
///
/// Extracted as a pure function for unit testing. Conditions:
/// 1. Current fingerprint is `Some` (eval succeeded, page is readable).
/// 2. Fingerprint unchanged since the last sample.
/// 3. No pending network requests (in-flight requests may inject DOM changes).
fn is_quiescent_signal(
    current_fp: &Option<String>,
    last_fp: &Option<String>,
    pending: usize,
) -> bool {
    current_fp.is_some() && current_fp == last_fp && pending == 0
}

/// Evaluate the DOM fingerprint expression in the isolated world (with
/// main-world fallback).
///
/// Returns `None` on CDP error or JS exception (page navigating, execution
/// context destroyed). `None` is treated as "not stable" by the quiescence
/// loop.
///
/// Running in the isolated world hides the `querySelectorAll` / DOM query
/// from page JS that may have monkey-patched `document.querySelectorAll` to
/// detect automation.
async fn get_dom_fingerprint(cdp: &CdpClient, context_id: Option<u64>) -> Option<String> {
    cdp.eval_readonly(context_id, DOM_FINGERPRINT_EXPR, EVAL_TIMEOUT)
        .await
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}

/// Wait for page quiescence: DOM fingerprint stable for `QUIESCENCE_QUIET_WINDOW`
/// AND no pending network requests.
///
/// Replaces the previous `Page.loadEventFired`-only wait and fixed sleeps. SPA
/// pages often render interactive elements after `loadEventFired` (via
/// setTimeout, microtask, or fetch callback); the quiescence gate ensures the
/// snapshot is taken after the DOM has settled, not after the browser's load
/// event.
///
/// On timeout, proceeds with the current state — does not fail the observation.
/// This handles pages with persistent connections (SSE, long-polling) where
/// `pending_request_count` never reaches zero.
async fn wait_for_page_quiescence(
    cdp: &CdpClient,
    capture: &capture::CaptureCollector,
    context_id: Option<u64>,
) {
    let deadline = tokio::time::Instant::now() + QUIESCENCE_TIMEOUT;
    let mut last_fp: Option<String> = None;
    let mut last_change = tokio::time::Instant::now();

    loop {
        let fp = get_dom_fingerprint(cdp, context_id).await;
        let pending = capture.pending_request_count();

        if !is_quiescent_signal(&fp, &last_fp, pending) {
            last_change = tokio::time::Instant::now();
            last_fp = fp;
        }

        if last_change.elapsed() >= QUIESCENCE_QUIET_WINDOW {
            break;
        }

        if tokio::time::Instant::now() >= deadline {
            break;
        }

        tokio::time::sleep(QUIESCENCE_POLL).await;
    }
}

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
    let context_id = session.isolated_context_id().await;

    // Wait for page quiescence before capturing — DOM fingerprint stable +
    // network idle. Prevents racy snapshots on SPA pages where interactive
    // elements appear after `Page.loadEventFired`.
    wait_for_page_quiescence(&cdp, &capture, context_id).await;

    // Concurrent: a11y snapshot + screenshot + URL/title eval.
    let screenshot_id = format!("shot-{}-{}", session.id, session.next_screenshot_seq());

    let (snapshot_result, screenshot_result, url_title) = tokio::join!(
        snapshot::take_snapshot(&cdp),
        screenshot::capture_screenshot(&cdp, viewport, &session.artifact_root, &screenshot_id),
        get_url_title(&cdp, context_id),
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
        dom::capture_dom_snapshot(&cdp, context_id).await
    } else {
        (Some(Vec::<DomSnapshotNode>::new()), None)
    };

    // Drain network + console from capture collector.
    // Network: from CDP Network.* events accumulated by the background loop.
    // Console: from Log.entryAdded (drain_console) + JS interceptor
    // (drain_console_js). The JS drain stays in the main world because the
    // interceptor stores entries in `window.__oxideConsoleCapture` — a
    // main-world JS variable not accessible from the isolated world (each
    // world has its own global object; only the DOM is shared).
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

/// Get URL and title via a single eval call in the isolated world (with
/// main-world fallback).
///
/// Returns `(url, title)` — empty strings on failure.
///
/// Running in the isolated world hides this read from page JS. Both
/// `window.location.href` and `document.title` are standard DOM properties
/// accessible from any world (they read from the shared C++ DOM objects).
async fn get_url_title(cdp: &CdpClient, context_id: Option<u64>) -> (String, String) {
    let expr = r#"JSON.stringify({url: window.location.href || document.URL || '', title: document.title || ''})"#;
    let json_str = match cdp.eval_readonly(context_id, expr, EVAL_TIMEOUT).await {
        Ok(value) => value.as_str().unwrap_or("").to_string(),
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

    // ── Quiescence unit tests ───────────────────────────────────────────

    #[test]
    fn dom_fingerprint_expr_contains_key_selectors() {
        assert!(DOM_FINGERPRINT_EXPR.contains("getElementsByTagName('*')"));
        assert!(DOM_FINGERPRINT_EXPR.contains("querySelectorAll"));
        assert!(DOM_FINGERPRINT_EXPR.contains("textarea"));
        assert!(DOM_FINGERPRINT_EXPR.contains("button"));
        assert!(DOM_FINGERPRINT_EXPR.contains("textContent"));
        assert!(DOM_FINGERPRINT_EXPR.contains("JSON.stringify"));
    }

    #[test]
    fn quiescent_signal_requires_fingerprint_and_no_pending() {
        let fp = Some("abc".to_string());
        assert!(is_quiescent_signal(&fp, &fp, 0));
    }

    #[test]
    fn quiescent_signal_rejects_pending_requests() {
        let fp = Some("abc".to_string());
        assert!(!is_quiescent_signal(&fp, &fp, 1));
        assert!(!is_quiescent_signal(&fp, &fp, 5));
    }

    #[test]
    fn quiescent_signal_rejects_changed_fingerprint() {
        let fp_a = Some("abc".to_string());
        let fp_b = Some("def".to_string());
        assert!(!is_quiescent_signal(&fp_a, &fp_b, 0));
        assert!(!is_quiescent_signal(&fp_b, &fp_a, 0));
    }

    #[test]
    fn quiescent_signal_rejects_missing_fingerprint() {
        let fp = Some("abc".to_string());
        assert!(!is_quiescent_signal(&None, &None, 0));
        assert!(!is_quiescent_signal(&None, &fp, 0));
        assert!(!is_quiescent_signal(&fp, &None, 0));
    }
}
