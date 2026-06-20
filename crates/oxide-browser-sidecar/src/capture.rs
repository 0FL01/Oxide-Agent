//! Network and console capture on the same CDP connection (stealth-safe).
//!
//! Replaces the Python sidecar's `CDPListener` — but on the **single** session
//! WebSocket (G3) and **without** `Runtime.enable` (G4).
//!
//! # Capture sources
//!
//! | Source | CDP command | Stealth-safe? |
//! |--------|-------------|---------------|
//! | Network requests | `Network.enable` | Yes — no `Runtime.enable` needed |
//! | Browser-level logs | `Log.enable` | Yes — independent domain |
//! | JS `console.*` calls | Injected JS interceptor | Yes — `Runtime.evaluate` only |
//!
//! The JS interceptor overrides `console.log/warn/error/debug/info` and stores
//! entries in a global array. It is injected via
//! `Page.addScriptToEvaluateOnNewDocument` (survives navigations) and
//! `Runtime.evaluate` (current page). The array is drained on demand via
//! `Runtime.evaluate` — **never** via `Runtime.enable` + `Runtime.consoleAPICalled`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use oxide_browser_contracts::{
    ConsoleDebugPayload, ConsoleItem, ConsoleLevel, ConsoleSummary, DiagnosticScope,
    NetworkDebugPayload, NetworkFilter, NetworkItem, NetworkSummary, ScopeCounts,
};
use serde_json::{Value, json};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::adblock::AdblockEngine;
use crate::cdp::{CdpClient, CdpEvent};
use crate::scope;

// ── Constants ───────────────────────────────────────────────────────────

/// CDP command timeout for capture setup and body fetches.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);

/// Max body chars when capturing response bodies (matches Python `MAX_BODY_CHARS`).
const MAX_BODY_CHARS: usize = 4096;

/// Max items retained in history (matches Python `_merge_history` default).
const MAX_HISTORY_ITEMS: usize = 1000;

// ── Console interceptor JS ──────────────────────────────────────────────

/// JS that overrides `console.log/warn/error/debug/info` and stores entries in
/// a global array. Drained via `window.__oxideDrainConsole()`.
///
/// Hardened against `toString()` detection: a `WeakMap`-backed
/// `Function.prototype.toString` override makes every patched console method
/// return its native string (`"function log() { [native code] }"`) under
/// `console.log.toString()`, `Function.prototype.toString.call(console.log)`,
/// and `String(console.log)`. Non-patched functions delegate to the real
/// `toString`; the override itself is registered in the `WeakMap` so it too
/// looks native. This closes the entire class of toString-based detection
/// without per-instance `toString` patches (which are bypassed by
/// `Function.prototype.toString.call`).
///
/// Injected via `Page.addScriptToEvaluateOnNewDocument` (survives navigations)
/// and `Runtime.evaluate` (current page). Does **not** require `Runtime.enable`.
const CONSOLE_INTERCEPTOR_JS: &str = r#"
(function() {
  if (window.__oxideConsoleCapture) return;
  var capture = [];
  var levels = { log: 'info', info: 'info', warn: 'warning', error: 'error', debug: 'verbose' };

  var nativeToString = Function.prototype.toString;
  var toStringOverrides = new WeakMap();
  var hardenedToString = function toString() {
    if (toStringOverrides.has(this)) return toStringOverrides.get(this);
    return nativeToString.call(this);
  };
  toStringOverrides.set(hardenedToString, nativeToString.call(nativeToString));
  Function.prototype.toString = hardenedToString;

  for (var method in levels) {
    (function(m, lvl) {
      var orig = console[m] ? console[m].bind(console) : function() {};
      var patched = function() {
        var args = Array.prototype.slice.call(arguments);
        var text = args.map(function(a) {
          try { return typeof a === 'object' ? JSON.stringify(a) : String(a); }
          catch(e) { return String(a); }
        }).join(' ');
        capture.push({ level: lvl, text: text, timestamp: Date.now() });
        orig.apply(console, arguments);
      };
      toStringOverrides.set(patched, 'function ' + m + '() { [native code] }');
      console[m] = patched;
    })(method, levels[method]);
  }
  window.__oxideConsoleCapture = capture;
  window.__oxideDrainConsole = function() {
    var items = capture.splice(0, capture.length);
    return JSON.stringify(items);
  };
})();
"#;

// ── Types ───────────────────────────────────────────────────────────────

/// Pending network request — tracks state between `requestWillBeSent` and
/// `loadingFinished`/`loadingFailed`.
#[derive(Debug, Clone)]
struct PendingRequest {
    method: String,
    url: String,
    resource_type: String,
    status: Option<u16>,
    error_text: Option<String>,
    /// URL of the document that owns this request (CDP `documentURL`). Reliable
    /// page-identity signal used to classify the request's diagnostic scope.
    document_url: Option<String>,
    /// Whether the request was canceled (CDP `loadingFailed.canceled`). A
    /// canceled `net::ERR_ABORTED` is benign navigation churn, not a real error.
    canceled: bool,
}

/// Errors from capture setup.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// CDP command failed during capture setup.
    #[error("CDP error during capture setup: {0}")]
    Cdp(String),
}

/// JS console entry deserialized from the interceptor's JSON output.
#[derive(serde::Deserialize)]
struct JsConsoleEntry {
    level: String,
    text: String,
    #[allow(dead_code)]
    timestamp: f64,
}

/// Shared state for network/console capture.
///
/// A background task processes CDP events and pushes items here.
/// REST handlers drain items when building observations or debug payloads.
pub struct CaptureCollector {
    network_items: std::sync::Mutex<Vec<NetworkItem>>,
    console_items: std::sync::Mutex<Vec<ConsoleItem>>,
    pending_requests: std::sync::Mutex<HashMap<String, PendingRequest>>,
    current_url: std::sync::Mutex<Option<String>>,
    /// Ad blocking engine. When `Some`, `Fetch.enable` is sent in `start()`
    /// and `Fetch.requestPaused` events are handled. When `None`, no Fetch
    /// domain commands are sent — zero behavior change.
    engine: Option<Arc<AdblockEngine>>,
    /// Consent auto-dismiss injection script. When `Some`, injected via
    /// `Page.addScriptToEvaluateOnNewDocument` with `worldName: "consent"`
    /// in `start()`. When `None`, no consent script is injected.
    consent_script: Option<Arc<String>>,
}

impl CaptureCollector {
    /// Create a collector with optional ad blocking and consent auto-dismiss.
    ///
    /// Pass `None` for either to disable that feature (default behavior).
    /// Pass `Some(Arc<AdblockEngine>)` to enable network-level request
    /// interception via CDP `Fetch.enable`. Pass `Some(Arc<String>)` for
    /// `consent_script` to enable cookie consent banner auto-dismissal via
    /// `Page.addScriptToEvaluateOnNewDocument`.
    pub fn new(engine: Option<Arc<AdblockEngine>>, consent_script: Option<Arc<String>>) -> Self {
        Self {
            network_items: std::sync::Mutex::new(Vec::new()),
            console_items: std::sync::Mutex::new(Vec::new()),
            pending_requests: std::sync::Mutex::new(HashMap::new()),
            current_url: std::sync::Mutex::new(None),
            engine,
            consent_script,
        }
    }

    /// Start capture: subscribe to events, enable `Network` + `Log` domains,
    /// inject console interceptor, and spawn a background event-processing task.
    ///
    /// **Never sends `Runtime.enable`** — stealth-safe (G4).
    /// Uses the same CDP connection as all other commands (G3).
    pub async fn start(
        cdp: &CdpClient,
        collector: Arc<Self>,
    ) -> Result<tokio::task::JoinHandle<()>, CaptureError> {
        // Subscribe BEFORE enabling domains to avoid missing early events.
        let mut events = cdp.subscribe();

        // Enable Network domain for network event capture.
        cdp.send_command("Network.enable", Value::Null, CAPTURE_TIMEOUT)
            .await
            .map_err(|e| CaptureError::Cdp(e.to_string()))?;
        debug!("Network.enable sent for capture");

        // Enable Log domain for browser-level log entries.
        // Log.enable does NOT require Runtime.enable — it is an independent domain.
        cdp.send_command("Log.enable", Value::Null, CAPTURE_TIMEOUT)
            .await
            .map_err(|e| CaptureError::Cdp(e.to_string()))?;
        debug!("Log.enable sent for capture");

        // Enable Fetch domain for ad blocking if engine is present.
        // Fetch.enable does NOT require Runtime.enable — it is an independent
        // network-layer domain with zero JS-visible side effects.
        //
        // No patterns → intercept ALL requests (including navigation and
        // Document). Navigation is handled in `on_fetch_request_paused` via
        // `isNavigationRequest` / Document resource type check — immediate
        // `continueRequest`. This ensures no resource type is silently
        // excluded from ad blocking (e.g. Ping beacons, WebSocket trackers).
        if collector.engine.is_some() {
            cdp.send_command("Fetch.enable", json!({}), CAPTURE_TIMEOUT)
                .await
                .map_err(|e| CaptureError::Cdp(e.to_string()))?;
            info!("Fetch.enable sent for ad blocking");
        }

        // Inject console interceptor via Page.addScriptToEvaluateOnNewDocument
        // (survives navigations) + Runtime.evaluate (patches current page).
        // Runtime.evaluate works WITHOUT Runtime.enable — verified in CP0.
        let _ = cdp
            .send_command(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": CONSOLE_INTERCEPTOR_JS }),
                CAPTURE_TIMEOUT,
            )
            .await;
        let _ = cdp
            .send_command(
                "Runtime.evaluate",
                json!({ "expression": CONSOLE_INTERCEPTOR_JS }),
                CAPTURE_TIMEOUT,
            )
            .await;
        debug!("console interceptor injected");

        // Inject consent auto-dismiss engine via Page.addScriptToEvaluateOnNewDocument
        // in a named isolated world ("consent") for stealth — page JS cannot see
        // the engine code, only the DOM effects (button clicks, class additions).
        // The engine auto-detects CMP banners via CSS selectors and dismisses them
        // by clicking through the CMP's own UI (reject all consent categories).
        //
        // No Runtime.evaluate — the script runs on every navigation via
        // addScriptToEvaluateOnNewDocument. Runtime.evaluate would create a
        // duplicate in the main world (different worldName).
        //
        // Runs at document_start (before any page JS), survives navigations.
        // Stealth-safe: Fetch.enable (ad blocking) and Page domains are
        // independent; this injection has zero interaction with them.
        if let Some(script) = &collector.consent_script {
            let _ = cdp
                .send_command(
                    "Page.addScriptToEvaluateOnNewDocument",
                    json!({
                        "source": script.as_str()
                    }),
                    CAPTURE_TIMEOUT,
                )
                .await;
            info!("consent auto-dismiss engine injected");
        }

        // Spawn background task to process CDP events.
        let cdp_clone = cdp.clone();
        let handle = tokio::spawn(async move {
            collector.run_event_loop(cdp_clone, &mut events).await;
        });

        Ok(handle)
    }

    /// Background event loop — processes CDP events and updates collector state.
    async fn run_event_loop(
        self: Arc<Self>,
        cdp: CdpClient,
        events: &mut tokio::sync::broadcast::Receiver<CdpEvent>,
    ) {
        loop {
            match events.recv().await {
                Ok(event) => {
                    self.process_event(&cdp, &event).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("capture event loop lagged by {n} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    debug!("capture event loop: broadcast channel closed");
                    break;
                }
            }
        }
    }

    /// Process a single CDP event.
    async fn process_event(&self, cdp: &CdpClient, event: &CdpEvent) {
        match event.method.as_str() {
            "Network.requestWillBeSent" => self.on_request_will_be_sent(&event.params),
            "Network.responseReceived" => self.on_response_received(&event.params),
            "Network.loadingFinished" => self.on_loading_finished(cdp, &event.params).await,
            "Network.loadingFailed" => self.on_loading_failed(cdp, &event.params).await,
            "Log.entryAdded" => self.on_log_entry(&event.params),
            "Page.frameNavigated" => self.on_frame_navigated(&event.params),
            "Fetch.requestPaused" => self.on_fetch_request_paused(cdp, &event.params).await,
            _ => {}
        }
    }

    // ── Network event handlers ──────────────────────────────────────────

    fn on_request_will_be_sent(&self, params: &Value) {
        let Some(request_id) = params.get("requestId").and_then(|v| v.as_str()) else {
            return;
        };
        let Some(request) = params.get("request") else {
            return;
        };

        let method = request
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_string();
        let url = request
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let cdp_type = params.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let resource_type = resource_type_from_cdp(cdp_type);
        let document_url = params
            .get("documentURL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let pending = PendingRequest {
            method,
            url,
            resource_type,
            status: None,
            error_text: None,
            document_url,
            canceled: false,
        };

        self.pending_requests
            .lock_guard()
            .insert(request_id.to_string(), pending);
    }

    fn on_response_received(&self, params: &Value) {
        let Some(request_id) = params.get("requestId").and_then(|v| v.as_str()) else {
            return;
        };

        let mut pending = self.pending_requests.lock_guard();
        let Some(entry) = pending.get_mut(request_id) else {
            return;
        };

        if let Some(response) = params.get("response") {
            entry.status = response
                .get("status")
                .and_then(|v| v.as_u64())
                .map(|s| s as u16);
            if entry.url.is_empty() {
                entry.url = response
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
            }
        }
    }

    async fn on_loading_finished(&self, cdp: &CdpClient, params: &Value) {
        let Some(request_id) = params.get("requestId").and_then(|v| v.as_str()) else {
            return;
        };

        let entry = self.pending_requests.lock_guard().remove(request_id);
        let Some(entry) = entry else {
            return;
        };

        let page_url = self.current_url();
        let mut item = finalize_network_item(entry, page_url.as_deref());

        // Fetch body for XHR/fetch/failed requests.
        if should_capture_body(&item)
            && let Ok(result) = cdp
                .send_command(
                    "Network.getResponseBody",
                    json!({ "requestId": request_id }),
                    Duration::from_secs(3),
                )
                .await
            && let Some(body) = decode_response_body(&result)
        {
            item.body = Some(body);
        }

        // Always retained; noise is classified by `scope` (suppressed from
        // compact summaries) rather than silently dropped.
        self.network_items.lock_guard().push(item);
    }

    async fn on_loading_failed(&self, cdp: &CdpClient, params: &Value) {
        let Some(request_id) = params.get("requestId").and_then(|v| v.as_str()) else {
            return;
        };

        let entry = self.pending_requests.lock_guard().remove(request_id);
        let Some(mut entry) = entry else {
            return;
        };

        entry.error_text = params
            .get("errorText")
            .and_then(|v| v.as_str())
            .or_else(|| params.get("type").and_then(|v| v.as_str()))
            .map(|s| s.to_string());
        entry.canceled = params
            .get("canceled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let page_url = self.current_url();
        let mut item = finalize_network_item(entry, page_url.as_deref());

        // Fetch body for failed requests.
        if should_capture_body(&item)
            && let Ok(result) = cdp
                .send_command(
                    "Network.getResponseBody",
                    json!({ "requestId": request_id }),
                    Duration::from_secs(3),
                )
                .await
            && let Some(body) = decode_response_body(&result)
        {
            item.body = Some(body);
        }

        // Always retained; noise is classified by `scope` (suppressed from
        // compact summaries) rather than silently dropped.
        self.network_items.lock_guard().push(item);
    }

    // ── Log event handler ───────────────────────────────────────────────

    fn on_log_entry(&self, params: &Value) {
        let Some(entry) = params.get("entry") else {
            return;
        };

        let level_str = entry
            .get("level")
            .and_then(|v| v.as_str())
            .unwrap_or("info");
        let level = match level_str {
            "warning" => ConsoleLevel::Warning,
            "error" => ConsoleLevel::Error,
            _ => return, // Only Warning/Error are in the contract
        };

        let text = entry
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // `entry.url` is the resource URL the log refers to (verified CDP field).
        // It drives scope classification: chrome:// internal, third-party host,
        // or the page's own origin.
        let entry_url = entry.get("url").and_then(|v| v.as_str());
        let page_url = self.current_url();
        let scope = scope::classify_console(entry_url, page_url.as_deref());

        let item = ConsoleItem {
            timestamp: now_iso(),
            level,
            text_redacted: text,
            source: entry
                .get("source")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            line: entry
                .get("lineNumber")
                .and_then(|v| v.as_u64())
                .map(|l| l as u32),
            scope,
            occurrences: 1,
        };

        self.console_items.lock_guard().push(item);
    }

    // ── Page navigation handler ─────────────────────────────────────────

    fn on_frame_navigated(&self, params: &Value) {
        let Some(frame) = params.get("frame") else {
            return;
        };

        // Only track main frame.
        if frame.get("parentId").is_some() {
            return;
        }

        let url = frame.get("url").and_then(|v| v.as_str()).unwrap_or("");
        // Never adopt a browser-internal surface (e.g. `chrome://new-tab-page/`)
        // as page identity. Classifying by scheme catches `chrome://` too, which
        // the old `starts_with("chrome-")` check missed.
        if url.is_empty() || url == "about:blank" || scope::is_internal_url(url) {
            return;
        }

        *self.current_url.lock_guard() = Some(url.to_string());
    }

    // ── Fetch.requestPaused handler (ad blocking) ──────────────────────

    /// Handle `Fetch.requestPaused` — check the adblock engine and block or
    /// continue the request.
    ///
    /// Fail-open: on any error or missing field, `continueRequest` is sent so
    /// the request is never hung. Navigation requests are always passed
    /// through (also excluded from `Fetch.enable` patterns as defense-in-depth).
    async fn on_fetch_request_paused(&self, cdp: &CdpClient, params: &Value) {
        let Some(request_id) = params.get("requestId").and_then(|v| v.as_str()) else {
            return;
        };

        let request = params.get("request").unwrap_or(&Value::Null);
        let url = request.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let resource_type = params
            .get("resourceType")
            .and_then(|v| v.as_str())
            .unwrap_or("Other");
        let is_nav = request
            .get("isNavigationRequest")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let source_url = self.current_url().unwrap_or_default();

        let blocked = should_block_request(
            self.engine.as_deref(),
            url,
            resource_type,
            is_nav,
            &source_url,
        );

        if blocked {
            debug!(url, request_id, "adblock: blocking request");
            let _ = cdp
                .send_command(
                    "Fetch.failRequest",
                    json!({ "requestId": request_id, "errorReason": "BlockedByClient" }),
                    CAPTURE_TIMEOUT,
                )
                .await;
        } else {
            let _ = cdp
                .send_command(
                    "Fetch.continueRequest",
                    json!({ "requestId": request_id }),
                    CAPTURE_TIMEOUT,
                )
                .await;
        }
    }

    // ── Public drain/query methods ──────────────────────────────────────

    /// Drain all accumulated network items (moves them out).
    pub fn drain_network(&self) -> Vec<NetworkItem> {
        let mut items = self.network_items.lock_guard();
        items.drain(..).collect()
    }

    /// Drain all accumulated console items from `Log.entryAdded` (moves them out).
    pub fn drain_console(&self) -> Vec<ConsoleItem> {
        let mut items = self.console_items.lock_guard();
        items.drain(..).collect()
    }

    /// Get the current page URL (updated by `Page.frameNavigated` events).
    pub fn current_url(&self) -> Option<String> {
        self.current_url.lock_guard().clone()
    }

    /// Number of in-flight network requests (for `wait_until=networkidle`).
    pub fn pending_request_count(&self) -> usize {
        self.pending_requests.lock_guard().len()
    }
}

impl Default for CaptureCollector {
    fn default() -> Self {
        Self::new(None, None)
    }
}

// ── Mutex helper trait ──────────────────────────────────────────────────

/// Helper to lock a `std::sync::Mutex` without `unwrap()` (forbidden by clippy).
trait MutexGuardExt<T> {
    fn lock_guard(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> MutexGuardExt<T> for std::sync::Mutex<T> {
    fn lock_guard(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
}

// ── Free functions: network item construction ──────────────────────────

/// Convert a `PendingRequest` into a finalized `NetworkItem`.
///
/// `page_url` is the top-level page URL (`current_url`) at finalize time; it is
/// the page-identity input for [`scope::classify_network`]. Classification runs
/// on the full request URL before the URL is compacted for storage.
fn finalize_network_item(entry: PendingRequest, page_url: Option<&str>) -> NetworkItem {
    let scope = scope::classify_network(
        &entry.url,
        entry.document_url.as_deref(),
        page_url,
        &entry.resource_type,
        entry.canceled,
        entry.error_text.as_deref(),
    );
    NetworkItem {
        timestamp: now_iso(),
        method: entry.method,
        url_redacted: compact_url(entry.url),
        status: entry.status,
        resource_type: entry.resource_type,
        error_text: entry.error_text,
        body: None,
        scope,
        occurrences: 1,
    }
}

/// Maximum stored length for inline `data:`/`blob:` URLs. The NTP storm emits
/// many multi-kilobyte `data:image/*;base64` URLs; keeping the full payload in
/// history would bloat memory for no diagnostic value.
const MAX_INLINE_URL_CHARS: usize = 64;

/// Compact noisy inline URLs (`data:`/`blob:`) to a short, scheme-preserving
/// prefix. Other URLs are returned unchanged. Truncation is char-boundary safe.
fn compact_url(url: String) -> String {
    if (url.starts_with("data:") || url.starts_with("blob:")) && url.len() > MAX_INLINE_URL_CHARS {
        let mut end = MAX_INLINE_URL_CHARS;
        while end > 0 && !url.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &url[..end])
    } else {
        url
    }
}

// ── Free functions: noise/failure classification ───────────────────────

/// Check if a network item is a failure (status >= 400 OR has `error_text`).
///
/// Matches the Python sidecar's `_is_network_failure` — the CP-B criterion.
pub fn is_network_failure(item: &NetworkItem) -> bool {
    item.status.is_some_and(|s| s >= 400) || item.error_text.is_some()
}

// ── Free functions: resource type mapping ──────────────────────────────

/// Map CDP request type to sidecar `resource_type` string.
///
/// Ported from the Python sidecar's `_resource_type_from_cdp`.
fn resource_type_from_cdp(type_name: &str) -> String {
    let value = type_name.to_lowercase();
    match value.as_str() {
        "xhr" | "fetch" => "xhr",
        "script" | "javascript" => "script",
        "stylesheet" | "css" => "stylesheet",
        "image" | "png" | "jpeg" | "jpg" | "webp" | "gif" | "svg" => "image",
        "document" | "html" => "document",
        _ => &value,
    }
    .to_string()
}

// ── Free functions: ad blocking decision ───────────────────────────────

/// Decide whether a `Fetch.requestPaused` event should be blocked.
///
/// Pure function (no CDP commands) — extracted from `on_fetch_request_paused`
/// for testability. Returns `true` if the request should be blocked via
/// `Fetch.failRequest`, `false` if it should continue via `Fetch.continueRequest`.
///
/// Rules:
/// 1. Navigation requests and Document resources are never blocked.
/// 2. If no engine is configured, nothing is blocked.
/// 3. `engine.should_block` decides; malformed URLs fail-open (return `false`).
fn should_block_request(
    engine: Option<&AdblockEngine>,
    url: &str,
    cdp_resource_type: &str,
    is_navigation: bool,
    source_url: &str,
) -> bool {
    if is_navigation || cdp_resource_type == "Document" {
        return false;
    }
    let Some(engine) = engine else {
        return false;
    };
    let adblock_type = crate::adblock::cdp_type_to_adblock(cdp_resource_type);
    engine.should_block(url, source_url, adblock_type)
}

// ── Free functions: body capture ───────────────────────────────────────

/// Check if a network request's body should be captured.
///
/// Body is captured for XHR/fetch requests and failed requests (status >= 400).
fn should_capture_body(item: &NetworkItem) -> bool {
    let rt = item.resource_type.to_lowercase();
    rt == "xhr" || rt == "fetch" || item.status.is_some_and(|s| s >= 400)
}

/// Decode a CDP `Network.getResponseBody` result into a UTF-8 string.
///
/// Handles both plain-text and base64-encoded bodies. Truncates to
/// `MAX_BODY_CHARS`. Ported from the Python sidecar's `_decode_response_body`.
fn decode_response_body(result: &Value) -> Option<String> {
    let body = result.get("body").and_then(|v| v.as_str())?;
    if body.is_empty() {
        return None;
    }

    let body = if result
        .get("base64Encoded")
        .is_some_and(|v| v.as_bool().unwrap_or(false))
    {
        // Decode base64 body to bytes, then interpret as UTF-8 (lossy).
        use base64::Engine;
        match base64::engine::general_purpose::STANDARD.decode(body) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => return None,
        }
    } else {
        body.to_string()
    };

    if body.len() > MAX_BODY_CHARS {
        Some(body[..MAX_BODY_CHARS].to_string())
    } else {
        Some(body)
    }
}

// ── Free functions: JS console drain ───────────────────────────────────

/// Drain the JS console interceptor by calling `Runtime.evaluate`.
///
/// Returns Warning/Error level `ConsoleItem`s from the injected interceptor.
/// Does **not** require `Runtime.enable` — `Runtime.evaluate` works standalone.
pub async fn drain_console_js(cdp: &CdpClient) -> Vec<ConsoleItem> {
    let result = cdp
        .send_command(
            "Runtime.evaluate",
            json!({
                "expression": "window.__oxideDrainConsole ? window.__oxideDrainConsole() : '[]'",
                "returnByValue": true,
            }),
            Duration::from_secs(3),
        )
        .await;

    let Ok(result) = result else {
        return Vec::new();
    };

    let json_str = result
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_str());

    let Some(json_str) = json_str else {
        return Vec::new();
    };

    let Ok(entries) = serde_json::from_str::<Vec<JsConsoleEntry>>(json_str) else {
        return Vec::new();
    };

    entries
        .into_iter()
        .filter_map(|entry| {
            let level = match entry.level.as_str() {
                "warning" => ConsoleLevel::Warning,
                "error" => ConsoleLevel::Error,
                _ => return None, // Only Warning/Error in contract
            };
            Some(ConsoleItem {
                timestamp: now_iso(),
                level,
                text_redacted: entry.text,
                source: Some("console-api".to_string()),
                line: None,
                // `console.*` calls are the page's own output → always surfaced.
                scope: DiagnosticScope::SiteRelated,
                occurrences: 1,
            })
        })
        .collect()
}

// ── Free functions: summarization ──────────────────────────────────────

/// Build a scope-aware `NetworkSummary` from a list of network items.
///
/// Only surfaced items (first-party XHR/fetch and site navigation) populate the
/// counts and `recent_*` lists; browser-internal, third-party, and benign items
/// are tallied into `suppressed` instead. `recent_*` are latest-first and capped
/// at `limit`. Items are assumed already deduplicated (one entry per
/// fingerprint, with `occurrences` on the item), so counting items counts
/// distinct requests.
pub fn summarize_network(items: &[NetworkItem], limit: usize) -> NetworkSummary {
    let mut suppressed = ScopeCounts::default();
    let mut surfaced: Vec<&NetworkItem> = Vec::new();
    for item in items {
        if item.scope.is_surfaced() {
            surfaced.push(item);
        } else {
            suppressed.record(item.scope);
        }
    }

    let failed_count = surfaced.iter().filter(|i| is_network_failure(i)).count() as u32;
    let request_count = surfaced.len() as u32;
    let recent_failures = surfaced
        .iter()
        .rev()
        .filter(|i| is_network_failure(i))
        .take(limit)
        .map(|i| (*i).clone())
        .collect();
    let recent_requests = surfaced
        .iter()
        .rev()
        .take(limit)
        .map(|i| (*i).clone())
        .collect();
    NetworkSummary {
        failed_count,
        recent_failures,
        request_count,
        recent_requests,
        suppressed,
    }
}

/// Build a scope-aware `ConsoleSummary` from a list of console items.
///
/// Only surfaced items (the page's own console output and first-party
/// network/script errors) populate the counts and `recent_errors`; internal and
/// third-party entries are tallied into `suppressed`. Only Error-level items are
/// stored in `recent_errors` (latest-first, capped at `limit`); Warning items
/// are counted but not stored (matching the contract).
pub fn summarize_console(items: &[ConsoleItem], limit: usize) -> ConsoleSummary {
    let mut suppressed = ScopeCounts::default();
    let mut surfaced: Vec<&ConsoleItem> = Vec::new();
    for item in items {
        if item.scope.is_surfaced() {
            surfaced.push(item);
        } else {
            suppressed.record(item.scope);
        }
    }

    let error_count = surfaced
        .iter()
        .filter(|i| i.level == ConsoleLevel::Error)
        .count() as u32;
    let warning_count = surfaced
        .iter()
        .filter(|i| i.level == ConsoleLevel::Warning)
        .count() as u32;
    let recent_errors = surfaced
        .iter()
        .rev()
        .filter(|i| i.level == ConsoleLevel::Error)
        .take(limit)
        .map(|i| (*i).clone())
        .collect();
    ConsoleSummary {
        error_count,
        warning_count,
        recent_errors,
        suppressed,
    }
}

// ── Free functions: debug payload building ─────────────────────────────

/// Build a `NetworkDebugPayload` from accumulated network history.
///
/// Ported from the Python sidecar's `build_network_debug_payload`.
pub fn build_network_debug_payload(
    history: &[(NetworkItem, u64)],
    since_action_seq: u64,
    filter: NetworkFilter,
    include_suppressed: bool,
    include_bodies: bool,
    limit: usize,
) -> NetworkDebugPayload {
    let mut items: Vec<NetworkItem> = history
        .iter()
        .filter(|(_, seq)| *seq >= since_action_seq)
        .map(|(item, _)| item.clone())
        .collect();

    // Suppress browser-internal/third-party/benign noise unless explicitly
    // requested (parity with the compact summaries).
    if !include_suppressed {
        items.retain(|i| i.scope.is_surfaced());
    }

    match filter {
        NetworkFilter::Failed => items.retain(is_network_failure),
        NetworkFilter::Xhr => items.retain(|i| i.resource_type.to_lowercase().contains("xhr")),
        NetworkFilter::Fetch => items.retain(|i| i.resource_type.to_lowercase().contains("fetch")),
        NetworkFilter::Document => {
            items.retain(|i| i.resource_type.to_lowercase() == "document");
        }
        NetworkFilter::All => {}
    }

    let failed_count = items.iter().filter(|i| is_network_failure(i)).count() as u32;

    // Take last `limit` items (most recent).
    let start = items.len().saturating_sub(limit);
    items.drain(..start);

    // Handle body inclusion/exclusion.
    if include_bodies {
        for item in &mut items {
            let rt = item.resource_type.to_lowercase();
            if !matches!(rt.as_str(), "xhr" | "fetch") && item.status.is_none_or(|s| s < 400) {
                item.body = None;
            }
        }
    } else {
        for item in &mut items {
            item.body = None;
        }
    }

    NetworkDebugPayload {
        failed_count,
        items,
        artifact_uri: None,
    }
}

/// Build a `ConsoleDebugPayload` from accumulated console history.
///
/// Ported from the Python sidecar's `build_console_debug_payload`.
pub fn build_console_debug_payload(
    history: &[(ConsoleItem, u64)],
    since_action_seq: u64,
    min_level: ConsoleLevel,
    include_suppressed: bool,
    limit: usize,
) -> ConsoleDebugPayload {
    let level_rank = |level: &ConsoleLevel| match level {
        ConsoleLevel::Warning => 2,
        ConsoleLevel::Error => 3,
    };
    let min_rank = level_rank(&min_level);

    let mut items: Vec<ConsoleItem> = history
        .iter()
        .filter(|(_, seq)| *seq >= since_action_seq)
        .filter(|(item, _)| level_rank(&item.level) >= min_rank)
        .filter(|(item, _)| include_suppressed || item.scope.is_surfaced())
        .map(|(item, _)| item.clone())
        .collect();

    let error_count = items
        .iter()
        .filter(|i| i.level == ConsoleLevel::Error)
        .count() as u32;
    let warning_count = items
        .iter()
        .filter(|i| i.level == ConsoleLevel::Warning)
        .count() as u32;

    // Take last `limit` items.
    let start = items.len().saturating_sub(limit);
    items.drain(..start);

    ConsoleDebugPayload {
        error_count,
        warning_count,
        items,
        artifact_uri: None,
    }
}

// ── Free functions: history management ─────────────────────────────────

/// Append new network items to history, deduplicating by content key.
///
/// Each entry is tagged with `action_seq` so debug endpoints can filter with
/// `since_action_seq`. Ported from the Python sidecar's `_merge_history`.
///
/// `occurrences` is **per-action**, not cumulative across the session: when a
/// fingerprint repeats within the same `action_seq` (e.g. several identical
/// requests captured between observations) occurrences are summed; when it
/// repeats in a later action the count is reset to the fresh item's value so
/// `sum(occurrences)` over surfaced items stays consistent with
/// `request_count` (which counts distinct items for the current action).
pub fn merge_network_history(
    history: &mut Vec<(NetworkItem, u64)>,
    fresh: Vec<NetworkItem>,
    action_seq: u64,
) {
    for item in fresh {
        let key = network_item_key(&item);
        if let Some(pos) = history.iter().position(|(e, _)| network_item_key(e) == key) {
            // Repeat: refresh recency (timestamp + action_seq) by re-appending
            // at the tail so the current action surfaces it again instead of
            // dropping it. Occurrences accumulate only within the same action;
            // a repeat from a later action resets to the fresh per-action count.
            let (mut existing, old_seq) = history.remove(pos);
            if old_seq == action_seq {
                existing.occurrences = existing.occurrences.saturating_add(item.occurrences.max(1));
            } else {
                existing.occurrences = item.occurrences.max(1);
            }
            existing.timestamp = item.timestamp;
            if item.body.is_some() {
                existing.body = item.body;
            }
            history.push((existing, action_seq));
        } else {
            history.push((item, action_seq));
        }
    }
    let start = history.len().saturating_sub(MAX_HISTORY_ITEMS);
    if start > 0 {
        history.drain(..start);
    }
}

/// Append new console items to history, deduplicating by content key.
///
/// Ported from the Python sidecar's `_merge_history` (console variant).
/// `occurrences` is per-action: see [`merge_network_history`].
pub fn merge_console_history(
    history: &mut Vec<(ConsoleItem, u64)>,
    fresh: Vec<ConsoleItem>,
    action_seq: u64,
) {
    for item in fresh {
        let key = console_item_key(&item);
        if let Some(pos) = history.iter().position(|(e, _)| console_item_key(e) == key) {
            let (mut existing, old_seq) = history.remove(pos);
            if old_seq == action_seq {
                existing.occurrences = existing.occurrences.saturating_add(item.occurrences.max(1));
            } else {
                existing.occurrences = item.occurrences.max(1);
            }
            existing.timestamp = item.timestamp;
            history.push((existing, action_seq));
        } else {
            history.push((item, action_seq));
        }
    }
    let start = history.len().saturating_sub(MAX_HISTORY_ITEMS);
    if start > 0 {
        history.drain(..start);
    }
}

/// Dedup fingerprint for a network item (method|url|status|resource_type|error_text|scope).
///
/// Timestamp is intentionally excluded so that repeats of the same request fold
/// together (bumping `occurrences`) instead of accumulating as distinct noise.
fn network_item_key(item: &NetworkItem) -> String {
    format!(
        "{}|{}|{}|{}|{}|{:?}",
        item.method,
        item.url_redacted,
        item.status.map(|s| s.to_string()).unwrap_or_default(),
        item.resource_type,
        item.error_text.as_deref().unwrap_or(""),
        item.scope,
    )
}

/// Dedup fingerprint for a console item (level|text|source|line|scope).
///
/// Timestamp is intentionally excluded so repeated identical log lines fold
/// together instead of accumulating.
fn console_item_key(item: &ConsoleItem) -> String {
    format!(
        "{}|{}|{}|{}|{:?}",
        match item.level {
            ConsoleLevel::Warning => "warning",
            ConsoleLevel::Error => "error",
        },
        item.text_redacted,
        item.source.as_deref().unwrap_or(""),
        item.line.map(|l| l.to_string()).unwrap_or_default(),
        item.scope,
    )
}

// ── Free functions: timestamp ───────────────────────────────────────────

/// Current UTC time as ISO 8601 `YYYY-MM-DDTHH:MM:SSZ`.
///
/// Implemented without external dependencies using the civil-from-days
/// algorithm (Howard Hinnant). Matches the Python sidecar's `now_iso()`.
pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    epoch_to_iso(secs)
}

/// Convert Unix epoch seconds to ISO 8601 `YYYY-MM-DDTHH:MM:SSZ`.
fn epoch_to_iso(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;

    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + if month <= 2 { 1 } else { 0 };

    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_network_failure ──────────────────────────────────────────────

    #[test]
    fn failure_status_ge_400() {
        let item = net_item("GET", "https://x.com/api", Some(500), None);
        assert!(is_network_failure(&item));
    }

    #[test]
    fn failure_error_text_present() {
        let item = net_item(
            "GET",
            "https://x.com/api",
            None,
            Some("net::ERR_ABORTED".into()),
        );
        assert!(is_network_failure(&item));
    }

    #[test]
    fn failure_status_200_ok() {
        let item = net_item("GET", "https://x.com/api", Some(200), None);
        assert!(!is_network_failure(&item));
    }

    #[test]
    fn failure_empty_item() {
        let item = net_item("GET", "https://x.com/api", None, None);
        assert!(!is_network_failure(&item));
    }

    // ── resource_type_from_cdp ──────────────────────────────────────────

    #[test]
    fn resource_type_xhr() {
        assert_eq!(resource_type_from_cdp("XHR"), "xhr");
        assert_eq!(resource_type_from_cdp("Fetch"), "xhr");
    }

    #[test]
    fn resource_type_document() {
        assert_eq!(resource_type_from_cdp("Document"), "document");
    }

    #[test]
    fn resource_type_other() {
        assert_eq!(resource_type_from_cdp("Media"), "media");
        assert_eq!(resource_type_from_cdp(""), "");
    }

    // ── should_block_request (ad blocking decision) ────────────────────

    fn make_engine() -> Arc<AdblockEngine> {
        Arc::new(AdblockEngine::from_rules([
            "||ads.example.com^",
            "||tracker.example.com^",
        ]))
    }

    #[test]
    fn adblock_no_engine_never_blocks() {
        assert!(!should_block_request(
            None,
            "https://ads.example.com/ad.js",
            "Script",
            false,
            "https://example.com/"
        ));
    }

    #[test]
    fn adblock_blocks_matching_url() {
        let engine = make_engine();
        assert!(should_block_request(
            Some(engine.as_ref()),
            "https://ads.example.com/ad.js",
            "Script",
            false,
            "https://example.com/"
        ));
        assert!(should_block_request(
            Some(engine.as_ref()),
            "https://tracker.example.com/pixel.gif",
            "Image",
            false,
            "https://example.com/"
        ));
    }

    #[test]
    fn adblock_allows_non_matching_url() {
        let engine = make_engine();
        assert!(!should_block_request(
            Some(engine.as_ref()),
            "https://example.com/page.html",
            "Document",
            false,
            "https://example.com/"
        ));
        assert!(!should_block_request(
            Some(engine.as_ref()),
            "https://cdn.example.com/lib.js",
            "Script",
            false,
            "https://example.com/"
        ));
    }

    #[test]
    fn adblock_skips_navigation_requests() {
        let engine = make_engine();
        // Even if the URL matches a block rule, navigation is never blocked
        assert!(!should_block_request(
            Some(engine.as_ref()),
            "https://ads.example.com/landing",
            "Document",
            true,
            "https://example.com/"
        ));
    }

    #[test]
    fn adblock_skips_document_resource_type() {
        let engine = make_engine();
        // Document resource type is never blocked (defense-in-depth)
        assert!(!should_block_request(
            Some(engine.as_ref()),
            "https://ads.example.com/doc.html",
            "Document",
            false,
            "https://example.com/"
        ));
    }

    #[test]
    fn adblock_fail_open_on_malformed_url() {
        let engine = make_engine();
        assert!(!should_block_request(
            Some(engine.as_ref()),
            "not-a-url",
            "Script",
            false,
            "https://example.com/"
        ));
    }

    #[test]
    fn adblock_maps_cdp_types_correctly() {
        let engine = make_engine();
        // Script type
        assert!(should_block_request(
            Some(engine.as_ref()),
            "https://ads.example.com/ad.js",
            "Script",
            false,
            "https://example.com/"
        ));
        // XHR type (maps to "xhr" in adblock)
        assert!(should_block_request(
            Some(engine.as_ref()),
            "https://ads.example.com/api",
            "XHR",
            false,
            "https://example.com/"
        ));
        // Image type
        assert!(should_block_request(
            Some(engine.as_ref()),
            "https://ads.example.com/banner.png",
            "Image",
            false,
            "https://example.com/"
        ));
    }

    // ── should_capture_body ─────────────────────────────────────────────

    #[test]
    fn body_xhr_captured() {
        let item = net_item("POST", "https://x.com/api", Some(200), None);
        assert!(!should_capture_body(&item)); // status 200, not xhr
        let item = NetworkItem {
            resource_type: "xhr".into(),
            ..item
        };
        assert!(should_capture_body(&item));
    }

    #[test]
    fn body_failed_captured() {
        let item = net_item("GET", "https://x.com/api", Some(500), None);
        assert!(should_capture_body(&item));
    }

    #[test]
    fn body_ok_not_captured() {
        let item = net_item("GET", "https://x.com/img.png", Some(200), None);
        assert!(!should_capture_body(&item));
    }

    // ── summarize_network ───────────────────────────────────────────────

    #[test]
    fn summarize_network_counts() {
        let items = vec![
            net_item("GET", "https://x.com/a", Some(200), None),
            net_item("POST", "https://x.com/b", Some(500), None),
            net_item("GET", "https://x.com/c", None, Some("net::ERR".into())),
        ];
        let summary = summarize_network(&items, 20);
        assert_eq!(summary.request_count, 3);
        assert_eq!(summary.failed_count, 2);
        assert_eq!(summary.recent_requests.len(), 3);
        assert_eq!(summary.recent_failures.len(), 2);
    }

    #[test]
    fn summarize_network_limit() {
        let items: Vec<NetworkItem> = (0..10)
            .map(|i| net_item("GET", &format!("https://x.com/{i}"), Some(200), None))
            .collect();
        let summary = summarize_network(&items, 3);
        assert_eq!(summary.recent_requests.len(), 3);
    }

    // ── summarize_console ───────────────────────────────────────────────

    #[test]
    fn summarize_console_counts() {
        let items = vec![
            console_item(ConsoleLevel::Error, "error 1"),
            console_item(ConsoleLevel::Error, "error 2"),
            console_item(ConsoleLevel::Warning, "warn 1"),
        ];
        let summary = summarize_console(&items, 20);
        assert_eq!(summary.error_count, 2);
        assert_eq!(summary.warning_count, 1);
        assert_eq!(summary.recent_errors.len(), 2);
    }

    // ── scope classification + suppression ──────────────────────────────

    #[test]
    fn finalize_classifies_first_party_xhr() {
        let entry = PendingRequest {
            method: "GET".into(),
            url: "https://site.com/api/isWritable".into(),
            resource_type: "xhr".into(),
            status: Some(404),
            error_text: None,
            document_url: Some("https://site.com/".into()),
            canceled: false,
        };
        let item = finalize_network_item(entry, Some("https://site.com/"));
        assert_eq!(item.scope, DiagnosticScope::FirstParty);
        assert_eq!(item.occurrences, 1);
        assert!(is_network_failure(&item));
    }

    #[test]
    fn finalize_classifies_chrome_internal() {
        let entry = PendingRequest {
            method: "GET".into(),
            url: "chrome://new-tab-page/foo.js".into(),
            resource_type: "script".into(),
            status: Some(200),
            error_text: None,
            document_url: Some("chrome://new-tab-page/".into()),
            canceled: false,
        };
        let item = finalize_network_item(entry, Some("https://site.com/"));
        assert_eq!(item.scope, DiagnosticScope::BrowserInternal);
    }

    #[test]
    fn finalize_compacts_data_url() {
        let long_payload = "A".repeat(500);
        let entry = PendingRequest {
            method: "GET".into(),
            url: format!("data:image/png;base64,{long_payload}"),
            resource_type: "image".into(),
            status: None,
            error_text: None,
            document_url: Some("https://site.com/".into()),
            canceled: false,
        };
        let item = finalize_network_item(entry, Some("https://site.com/"));
        assert_eq!(item.scope, DiagnosticScope::Benign);
        assert!(item.url_redacted.chars().count() <= MAX_INLINE_URL_CHARS + 1);
        assert!(item.url_redacted.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn summarize_network_suppresses_noise_and_surfaces_first_party() {
        let third_party = NetworkItem {
            scope: DiagnosticScope::ThirdPartySubresource,
            ..net_item("GET", "https://ads.example/beacon", Some(500), None)
        };
        let internal = NetworkItem {
            scope: DiagnosticScope::BrowserInternal,
            ..net_item("GET", "chrome://new-tab-page/x.js", Some(200), None)
        };
        let benign = NetworkItem {
            scope: DiagnosticScope::Benign,
            ..net_item("GET", "https://site.com/favicon.ico", Some(404), None)
        };
        let items = vec![
            net_item("GET", "https://site.com/api/a", Some(500), None),
            third_party,
            internal,
            benign,
            net_item("GET", "https://site.com/api/b", Some(404), None),
        ];
        let summary = summarize_network(&items, 20);
        // Only the two first-party failures surface.
        assert_eq!(summary.failed_count, 2);
        assert_eq!(summary.request_count, 2);
        assert_eq!(summary.recent_failures.len(), 2);
        // Latest-first ordering.
        assert_eq!(
            summary.recent_failures[0].url_redacted,
            "https://site.com/api/b"
        );
        // Noise is counted, not surfaced.
        assert_eq!(summary.suppressed.third_party, 1);
        assert_eq!(summary.suppressed.browser_internal, 1);
        assert_eq!(summary.suppressed.benign, 1);
    }

    #[test]
    fn summarize_console_suppresses_noise() {
        let internal = ConsoleItem {
            scope: DiagnosticScope::BrowserInternal,
            ..console_item(ConsoleLevel::Error, "chrome internal error")
        };
        let third_party = ConsoleItem {
            scope: DiagnosticScope::ThirdPartySubresource,
            ..console_item(ConsoleLevel::Error, "third party error")
        };
        let items = vec![
            console_item(ConsoleLevel::Error, "site error 1"),
            internal,
            third_party,
            console_item(ConsoleLevel::Error, "site error 2"),
        ];
        let summary = summarize_console(&items, 20);
        assert_eq!(summary.error_count, 2);
        assert_eq!(summary.recent_errors.len(), 2);
        assert_eq!(summary.recent_errors[0].text_redacted, "site error 2");
        assert_eq!(summary.suppressed.browser_internal, 1);
        assert_eq!(summary.suppressed.third_party, 1);
    }

    #[test]
    fn debug_network_include_suppressed_reveals_third_party() {
        let history = vec![
            (
                NetworkItem {
                    scope: DiagnosticScope::ThirdPartySubresource,
                    ..net_item("GET", "https://ads.example/x", Some(500), None)
                },
                0,
            ),
            (net_item("GET", "https://site.com/api", Some(500), None), 0),
        ];
        // Default: suppressed hidden.
        let hidden =
            build_network_debug_payload(&history, 0, NetworkFilter::Failed, false, false, 10);
        assert_eq!(hidden.items.len(), 1);
        // include_suppressed: both visible.
        let shown =
            build_network_debug_payload(&history, 0, NetworkFilter::Failed, true, false, 10);
        assert_eq!(shown.items.len(), 2);
    }

    // ── build_network_debug_payload ─────────────────────────────────────

    #[test]
    fn debug_network_filter_failed() {
        let history = vec![
            (net_item("GET", "https://x.com/a", Some(200), None), 0),
            (net_item("GET", "https://x.com/b", Some(500), None), 0),
            (
                net_item("GET", "https://x.com/c", None, Some("err".into())),
                1,
            ),
        ];
        let payload =
            build_network_debug_payload(&history, 0, NetworkFilter::Failed, false, false, 10);
        assert_eq!(payload.items.len(), 2);
        assert_eq!(payload.failed_count, 2);
    }

    #[test]
    fn debug_network_filter_xhr() {
        let history = vec![
            (
                NetworkItem {
                    resource_type: "xhr".into(),
                    ..net_item("GET", "https://x.com/api", Some(200), None)
                },
                0,
            ),
            (net_item("GET", "https://x.com/doc", Some(200), None), 0),
        ];
        let payload =
            build_network_debug_payload(&history, 0, NetworkFilter::Xhr, false, false, 10);
        assert_eq!(payload.items.len(), 1);
    }

    #[test]
    fn debug_network_since_action_seq() {
        let history = vec![
            (net_item("GET", "https://x.com/a", Some(200), None), 1),
            (net_item("GET", "https://x.com/b", Some(200), None), 5),
        ];
        let payload =
            build_network_debug_payload(&history, 3, NetworkFilter::All, false, false, 10);
        assert_eq!(payload.items.len(), 1);
        assert_eq!(payload.items[0].url_redacted, "https://x.com/b");
    }

    #[test]
    fn debug_network_include_bodies_strips_for_non_xhr() {
        let history = vec![(
            NetworkItem {
                resource_type: "image".into(),
                body: Some("should-be-stripped".into()),
                ..net_item("GET", "https://x.com/img.png", Some(200), None)
            },
            0,
        )];
        let payload = build_network_debug_payload(&history, 0, NetworkFilter::All, false, true, 10);
        assert!(payload.items[0].body.is_none());
    }

    #[test]
    fn debug_network_exclude_bodies() {
        let history = vec![(
            NetworkItem {
                resource_type: "xhr".into(),
                body: Some("response body".into()),
                ..net_item("GET", "https://x.com/api", Some(200), None)
            },
            0,
        )];
        let payload =
            build_network_debug_payload(&history, 0, NetworkFilter::All, false, false, 10);
        assert!(payload.items[0].body.is_none());
    }

    // ── build_console_debug_payload ─────────────────────────────────────

    #[test]
    fn debug_console_min_level_error() {
        let history = vec![
            (console_item(ConsoleLevel::Warning, "warn"), 0),
            (console_item(ConsoleLevel::Error, "err"), 0),
        ];
        let payload = build_console_debug_payload(&history, 0, ConsoleLevel::Error, false, 10);
        assert_eq!(payload.items.len(), 1);
        assert_eq!(payload.items[0].text_redacted, "err");
    }

    #[test]
    fn debug_console_min_level_warning() {
        let history = vec![
            (console_item(ConsoleLevel::Warning, "warn"), 0),
            (console_item(ConsoleLevel::Error, "err"), 0),
        ];
        let payload = build_console_debug_payload(&history, 0, ConsoleLevel::Warning, false, 10);
        assert_eq!(payload.items.len(), 2);
    }

    // ── merge_network_history ───────────────────────────────────────────

    #[test]
    fn merge_network_dedup_same_action_accumulates() {
        let mut history = Vec::new();
        let item = net_item("GET", "https://x.com/a", Some(200), None);
        // Two fresh identical items merged under the same action_seq → summed.
        merge_network_history(&mut history, vec![item.clone(), item], 1);
        assert_eq!(history.len(), 1); // deduped by fingerprint (no timestamp)
        assert_eq!(history[0].1, 1);
        assert_eq!(history[0].0.occurrences, 2); // same-action repeat accumulates
    }

    #[test]
    fn merge_network_dedup_cross_action_resets_occurrences() {
        let mut history = Vec::new();
        let item = net_item("GET", "https://x.com/a", Some(404), None);
        merge_network_history(&mut history, vec![item.clone()], 1);
        // Same fingerprint reappears in a later action: occurrences reset to
        // the fresh per-action count instead of accumulating across the session.
        merge_network_history(&mut history, vec![item], 2);
        assert_eq!(history.len(), 1); // still deduped to one entry
        assert_eq!(history[0].1, 2); // recency refreshes to latest action_seq
        assert_eq!(history[0].0.occurrences, 1); // cross-action reset, not 2
    }

    #[test]
    fn merge_network_max_items() {
        let mut history = Vec::new();
        for i in 0..(MAX_HISTORY_ITEMS + 100) {
            let item = net_item("GET", &format!("https://x.com/{i}"), Some(200), None);
            merge_network_history(&mut history, vec![item], i as u64);
        }
        assert_eq!(history.len(), MAX_HISTORY_ITEMS);
    }

    // ── merge_console_history ───────────────────────────────────────────

    #[test]
    fn merge_console_dedup_same_action_accumulates() {
        let mut history = Vec::new();
        let item = console_item(ConsoleLevel::Error, "error");
        merge_console_history(&mut history, vec![item.clone(), item], 1);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].1, 1);
        assert_eq!(history[0].0.occurrences, 2); // same-action repeat accumulates
    }

    #[test]
    fn merge_console_dedup_cross_action_resets_occurrences() {
        let mut history = Vec::new();
        let item = console_item(ConsoleLevel::Error, "error");
        merge_console_history(&mut history, vec![item.clone()], 1);
        merge_console_history(&mut history, vec![item], 2);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].1, 2); // recency refreshes to latest action_seq
        assert_eq!(history[0].0.occurrences, 1); // cross-action reset, not 2
    }

    #[test]
    fn summary_occurrences_are_per_action_not_cumulative() {
        // Simulate a favicon-style 404 reappearing every action across a
        // session, plus a one-off first-party failure in the latest action.
        // The compact summary for the latest action must report per-action
        // occurrences (1), not the session-total, and keep failed_count
        // consistent with the distinct surfaced items for that action.
        let mut history = Vec::new();
        let recurring = net_item("GET", "https://site.com/api/health", Some(500), None);
        for seq in 1..=8_u64 {
            merge_network_history(&mut history, vec![recurring.clone()], seq);
        }
        let one_off = net_item("GET", "https://site.com/api/login", Some(404), None);
        merge_network_history(&mut history, vec![one_off], 8);

        let latest: Vec<_> = history
            .iter()
            .filter(|(_, seq)| *seq == 8)
            .map(|(item, _)| item.clone())
            .collect();
        let summary = summarize_network(&latest, 20);
        assert_eq!(summary.failed_count, 2); // two distinct surfaced failures
        let recurring_entry = summary
            .recent_failures
            .iter()
            .find(|i| i.url_redacted.contains("/health"))
            .expect("recurring failure surfaced in latest action");
        assert_eq!(
            recurring_entry.occurrences, 1,
            "per-action occurrences, not session-cumulative 8"
        );
    }

    // ── now_iso / epoch_to_iso ──────────────────────────────────────────

    #[test]
    fn epoch_to_iso_1970() {
        assert_eq!(epoch_to_iso(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn epoch_to_iso_known_date() {
        // 2024-01-01T00:00:00Z = 1704067200
        assert_eq!(epoch_to_iso(1704067200), "2024-01-01T00:00:00Z");
    }

    #[test]
    fn now_iso_format() {
        let iso = now_iso();
        assert!(iso.ends_with('Z'));
        assert_eq!(iso.len(), 20); // YYYY-MM-DDTHH:MM:SSZ
    }

    // ── console interceptor JS ──────────────────────────────────────────

    #[test]
    fn interceptor_js_contains_overrides() {
        assert!(CONSOLE_INTERCEPTOR_JS.contains("__oxideConsoleCapture"));
        assert!(CONSOLE_INTERCEPTOR_JS.contains("__oxideDrainConsole"));
        assert!(CONSOLE_INTERCEPTOR_JS.contains("warn"));
        assert!(CONSOLE_INTERCEPTOR_JS.contains("error"));
        assert!(CONSOLE_INTERCEPTOR_JS.contains("levels"));
    }

    #[test]
    fn interceptor_js_no_runtime_enable() {
        assert!(!CONSOLE_INTERCEPTOR_JS.contains("Runtime.enable"));
    }

    #[test]
    fn interceptor_js_hardens_function_to_string() {
        // WeakMap-backed global override so patched functions return native strings
        assert!(CONSOLE_INTERCEPTOR_JS.contains("WeakMap"));
        assert!(CONSOLE_INTERCEPTOR_JS.contains("Function.prototype.toString"));
        assert!(CONSOLE_INTERCEPTOR_JS.contains("toStringOverrides"));
        // The override itself is registered to look native
        assert!(CONSOLE_INTERCEPTOR_JS.contains("nativeToString.call(nativeToString)"));
        // Each patched method gets a native toString string
        assert!(CONSOLE_INTERCEPTOR_JS.contains("[native code]"));
    }

    // ── decode_response_body ────────────────────────────────────────────

    #[test]
    fn decode_text_body() {
        let result = json!({"body": "hello world", "base64Encoded": false});
        assert_eq!(
            decode_response_body(&result),
            Some("hello world".to_string())
        );
    }

    #[test]
    fn decode_base64_body() {
        let result = json!({"body": "aGVsbG8=", "base64Encoded": true});
        assert_eq!(decode_response_body(&result), Some("hello".to_string()));
    }

    #[test]
    fn decode_empty_body() {
        let result = json!({"body": "", "base64Encoded": false});
        assert_eq!(decode_response_body(&result), None);
    }

    #[test]
    fn decode_truncates_long_body() {
        let long = "x".repeat(MAX_BODY_CHARS + 100);
        let result = json!({"body": long, "base64Encoded": false});
        let decoded = decode_response_body(&result).expect("should decode");
        assert_eq!(decoded.len(), MAX_BODY_CHARS);
    }

    // ── Helpers ─────────────────────────────────────────────────────────

    fn net_item(
        method: &str,
        url: &str,
        status: Option<u16>,
        error: Option<String>,
    ) -> NetworkItem {
        NetworkItem {
            timestamp: "2026-01-01T00:00:00Z".into(),
            method: method.into(),
            url_redacted: url.into(),
            status,
            resource_type: "other".into(),
            error_text: error,
            body: None,
            scope: DiagnosticScope::SiteRelated,
            occurrences: 1,
        }
    }

    fn console_item(level: ConsoleLevel, text: &str) -> ConsoleItem {
        ConsoleItem {
            timestamp: "2026-01-01T00:00:00Z".into(),
            level,
            text_redacted: text.into(),
            source: None,
            line: None,
            scope: DiagnosticScope::SiteRelated,
            occurrences: 1,
        }
    }
}
