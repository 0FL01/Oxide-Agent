//! Observation building — concurrent a11y + screenshot + URL/title + DOM.
//!
//! All four capture channels run concurrently via `tokio::join!` so they
//! observe the same DOM state. Before any capture, [`wait_for_page_quiescence`]
//! waits for DOM stability (MutationObserver age + fingerprint unchanged) +
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

/// CDP timeout for URL/title and quiescence probe eval.
const EVAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum time to wait for page quiescence before proceeding with observation.
///
/// Safety valve for pages with constant activity (SSE, long-polling, DOM
/// mutations from timers). On timeout the observation proceeds with the current
/// state rather than hanging.
const QUIESCENCE_TIMEOUT: Duration = Duration::from_secs(3);

/// Duration the DOM must remain mutation-free (MutationObserver age) AND the
/// fingerprint unchanged AND no pending network requests before the page is
/// considered quiescent.
const QUIESCENCE_QUIET_WINDOW: Duration = Duration::from_millis(500);

/// Poll interval for quiescence checks.
const QUIESCENCE_POLL: Duration = Duration::from_millis(50);

/// JavaScript expression that sets up a MutationObserver and returns a
/// quiescence probe result.
///
/// Lazily installs a `MutationObserver` on `document.documentElement` that
/// records the timestamp of every DOM mutation. On each call, returns a JSON
/// string with:
/// - `age`: milliseconds since the last DOM mutation (primary quiescence signal)
/// - `fp`: structural fingerprint (element count + interactive count + text length)
///
/// The MutationObserver catches ALL DOM mutations regardless of trigger
/// mechanism (setTimeout, requestAnimationFrame, microtask, async callback) —
/// unlike fingerprint polling which only samples every 50ms and can miss
/// mutations between polls.
///
/// The observer persists in the isolated world's global for the lifetime of the
/// execution context. After navigation, the context is recreated and the
/// observer is re-installed on first call.
///
/// `age = 0` on first call (just installed) grows naturally: if no mutations
/// occur, `age` increases by the poll interval each call. If a mutation occurs,
/// `age` resets to 0. Quiescence requires `age >= QUIESCENCE_QUIET_WINDOW`.
const QUIESCENCE_PROBE_EXPR: &str = r#"(() => {
  if (!window.__oxideQuiescence) {
    let lastMutation = Date.now();
    try {
      const observer = new MutationObserver(function() { lastMutation = Date.now(); });
      observer.observe(document.documentElement || document.body, {
        childList: true, subtree: true, attributes: true, characterData: true
      });
    } catch(e) {}
    window.__oxideQuiescence = { getLastMutation: function() { return lastMutation; } };
  }
  const age = Date.now() - window.__oxideQuiescence.getLastMutation();
  const fp = JSON.stringify({
    els: document.getElementsByTagName('*').length,
    interactive: document.querySelectorAll('a,button,input,textarea,select,[role="button"],[role="link"],[data-clipboard-text]').length,
    text: (document.body && document.body.textContent) ? document.body.textContent.length : 0
  });
  return JSON.stringify({ age: age, fp: fp });
})()"#;

/// Parsed quiescence probe result.
#[derive(Debug, Clone)]
struct QuiescenceProbe {
    age_ms: u64,
    fingerprint: Option<String>,
}

/// Whether the page is quiescent given the probe, the last fingerprint, and the
/// pending network request count.
///
/// Extracted as a pure function for unit testing. Conditions:
/// 1. MutationObserver age >= quiet window (no DOM mutations recently).
/// 2. Fingerprint is `Some` and unchanged since the last sample.
/// 3. No pending network requests (in-flight requests may inject DOM changes).
fn is_quiescent_signal(probe: &QuiescenceProbe, last_fp: &Option<String>, pending: usize) -> bool {
    probe.age_ms >= QUIESCENCE_QUIET_WINDOW.as_millis() as u64
        && probe.fingerprint.is_some()
        && &probe.fingerprint == last_fp
        && pending == 0
}

/// Evaluate the quiescence probe expression in the isolated world (with
/// main-world fallback).
///
/// Returns `None` on CDP error or JS exception (page navigating, execution
/// context destroyed). `None` is treated as "not stable" by the quiescence
/// loop.
async fn get_quiescence_probe(cdp: &CdpClient, context_id: Option<u64>) -> Option<QuiescenceProbe> {
    let value = cdp
        .eval_readonly(context_id, QUIESCENCE_PROBE_EXPR, EVAL_TIMEOUT)
        .await
        .ok()?;
    let json_str = value.as_str()?;
    let parsed: Value = serde_json::from_str(json_str).ok()?;
    Some(QuiescenceProbe {
        age_ms: parsed.get("age")?.as_u64()?,
        fingerprint: parsed.get("fp")?.as_str().map(|s| s.to_string()),
    })
}

/// Wait for page quiescence: MutationObserver age >= quiet window AND
/// fingerprint unchanged AND no pending network requests.
///
/// The MutationObserver continuously records all DOM mutations (not just
/// sampled fingerprints), making `age` an exact "time since last mutation"
/// signal. The fingerprint is a belt-and-suspenders check for CSS-only visual
/// changes that don't trigger MutationObserver.
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
    let mut quiescent_since = tokio::time::Instant::now();

    loop {
        let probe = get_quiescence_probe(cdp, context_id).await;
        let pending = capture.pending_request_count();

        if !is_quiescent_signal(&probe_or_default(&probe), &last_fp, pending) {
            quiescent_since = tokio::time::Instant::now();
            last_fp = probe.as_ref().and_then(|p| p.fingerprint.clone());
        }

        if quiescent_since.elapsed() >= QUIESCENCE_QUIET_WINDOW {
            break;
        }

        if tokio::time::Instant::now() >= deadline {
            break;
        }

        tokio::time::sleep(QUIESCENCE_POLL).await;
    }
}

/// Return a default probe (age 0, no fingerprint) when the eval fails.
fn probe_or_default(probe: &Option<QuiescenceProbe>) -> QuiescenceProbe {
    probe.clone().unwrap_or(QuiescenceProbe {
        age_ms: 0,
        fingerprint: None,
    })
}

/// Build a full `BrowserObservation` from the current page state.
///
/// Runs a11y snapshot + screenshot + URL/title + DOM snapshot concurrently via
/// `tokio::join!` so all four channels observe the same DOM state. Then drains
/// network/console, merges into history, and builds summaries.
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

    // Wait for page quiescence before capturing — MutationObserver age +
    // fingerprint stable + network idle. Prevents racy snapshots on SPA pages
    // where interactive elements appear after `Page.loadEventFired`.
    wait_for_page_quiescence(&cdp, &capture, context_id).await;

    // Concurrent: a11y snapshot + screenshot + URL/title + DOM snapshot.
    // All four CDP commands are sent near-simultaneously and capture the same
    // DOM state. Moving DOM into the join eliminates the desync where the DOM
    // snapshot could lag behind the screenshot by 100-500ms.
    let screenshot_id = format!("shot-{}-{}", session.id, session.next_screenshot_seq());

    let (snapshot_result, screenshot_result, url_title, dom_result) = tokio::join!(
        snapshot::take_snapshot(&cdp),
        screenshot::capture_screenshot(&cdp, viewport, &session.artifact_root, &screenshot_id),
        get_url_title(&cdp, context_id),
        async {
            if include_dom {
                dom::capture_dom_snapshot(&cdp, context_id).await
            } else {
                (Some(Vec::<DomSnapshotNode>::new()), None)
            }
        },
    );

    let (screenshot_artifact, screenshot_bytes) = screenshot_result;
    let (dom_snapshot, dom_snapshot_error) = dom_result;

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
    fn quiescence_probe_expr_installs_mutation_observer() {
        assert!(QUIESCENCE_PROBE_EXPR.contains("MutationObserver"));
        assert!(QUIESCENCE_PROBE_EXPR.contains("childList: true"));
        assert!(QUIESCENCE_PROBE_EXPR.contains("subtree: true"));
        assert!(QUIESCENCE_PROBE_EXPR.contains("attributes: true"));
        assert!(QUIESCENCE_PROBE_EXPR.contains("characterData: true"));
    }

    #[test]
    fn quiescence_probe_expr_returns_age_and_fingerprint() {
        assert!(
            QUIESCENCE_PROBE_EXPR.contains("\"age\"") || QUIESCENCE_PROBE_EXPR.contains("age:")
        );
        assert!(QUIESCENCE_PROBE_EXPR.contains("\"fp\"") || QUIESCENCE_PROBE_EXPR.contains("fp:"));
        assert!(QUIESCENCE_PROBE_EXPR.contains("getElementsByTagName('*')"));
        assert!(QUIESCENCE_PROBE_EXPR.contains("querySelectorAll"));
        assert!(QUIESCENCE_PROBE_EXPR.contains("textContent"));
    }

    #[test]
    fn quiescent_signal_requires_age_fingerprint_and_no_pending() {
        let probe = QuiescenceProbe {
            age_ms: 600,
            fingerprint: Some("abc".to_string()),
        };
        let fp = Some("abc".to_string());
        assert!(is_quiescent_signal(&probe, &fp, 0));
    }

    #[test]
    fn quiescent_signal_rejects_age_below_quiet_window() {
        let probe = QuiescenceProbe {
            age_ms: 499,
            fingerprint: Some("abc".to_string()),
        };
        let fp = Some("abc".to_string());
        assert!(!is_quiescent_signal(&probe, &fp, 0));
    }

    #[test]
    fn quiescent_signal_rejects_pending_requests() {
        let probe = QuiescenceProbe {
            age_ms: 600,
            fingerprint: Some("abc".to_string()),
        };
        let fp = Some("abc".to_string());
        assert!(!is_quiescent_signal(&probe, &fp, 1));
        assert!(!is_quiescent_signal(&probe, &fp, 5));
    }

    #[test]
    fn quiescent_signal_rejects_changed_fingerprint() {
        let probe = QuiescenceProbe {
            age_ms: 600,
            fingerprint: Some("abc".to_string()),
        };
        let fp_b = Some("def".to_string());
        assert!(!is_quiescent_signal(&probe, &fp_b, 0));
    }

    #[test]
    fn quiescent_signal_rejects_missing_fingerprint() {
        let probe = QuiescenceProbe {
            age_ms: 600,
            fingerprint: None,
        };
        let fp = Some("abc".to_string());
        assert!(!is_quiescent_signal(&probe, &fp, 0));
        assert!(!is_quiescent_signal(&probe, &None, 0));
    }
}
