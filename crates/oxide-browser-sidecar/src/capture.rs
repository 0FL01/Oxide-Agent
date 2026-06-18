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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use oxide_browser_contracts::{
    ConsoleDebugPayload, ConsoleItem, ConsoleLevel, ConsoleSummary, NetworkDebugPayload,
    NetworkFilter, NetworkItem, NetworkSummary,
};
use serde_json::{Value, json};
use thiserror::Error;
use tracing::{debug, warn};

use crate::cdp::{CdpClient, CdpEvent};

// ── Constants ───────────────────────────────────────────────────────────

/// CDP command timeout for capture setup and body fetches.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);

/// Max body chars when capturing response bodies (matches Python `MAX_BODY_CHARS`).
const MAX_BODY_CHARS: usize = 4096;

/// Max items retained in history (matches Python `_merge_history` default).
const MAX_HISTORY_ITEMS: usize = 1000;

/// URL suffixes considered noise (favicon requests).
const NOISE_URL_SUFFIXES: &[&str] = &["/favicon.ico"];

// ── Console interceptor JS ──────────────────────────────────────────────

/// JS that overrides `console.log/warn/error/debug/info` and stores entries in
/// a global array. Drained via `window.__oxideDrainConsole()`.
///
/// Injected via `Page.addScriptToEvaluateOnNewDocument` (survives navigations)
/// and `Runtime.evaluate` (current page). Does **not** require `Runtime.enable`.
const CONSOLE_INTERCEPTOR_JS: &str = r#"
(function() {
  if (window.__oxideConsoleCapture) return;
  var capture = [];
  var levels = { log: 'info', info: 'info', warn: 'warning', error: 'error', debug: 'verbose' };
  for (var method in levels) {
    (function(m, lvl) {
      var orig = console[m] ? console[m].bind(console) : function() {};
      console[m] = function() {
        var args = Array.prototype.slice.call(arguments);
        var text = args.map(function(a) {
          try { return typeof a === 'object' ? JSON.stringify(a) : String(a); }
          catch(e) { return String(a); }
        }).join(' ');
        capture.push({ level: lvl, text: text, timestamp: Date.now() });
        orig.apply(console, arguments);
      };
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
}

impl CaptureCollector {
    /// Create an empty collector.
    pub fn new() -> Self {
        Self {
            network_items: std::sync::Mutex::new(Vec::new()),
            console_items: std::sync::Mutex::new(Vec::new()),
            pending_requests: std::sync::Mutex::new(HashMap::new()),
            current_url: std::sync::Mutex::new(None),
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

        let pending = PendingRequest {
            method,
            url,
            resource_type,
            status: None,
            error_text: None,
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

        let mut item = finalize_network_item(entry);

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

        if !is_noise_network(&item) {
            self.network_items.lock_guard().push(item);
        }
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

        let mut item = finalize_network_item(entry);

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

        if !is_noise_network(&item) {
            self.network_items.lock_guard().push(item);
        }
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

        if is_noise_console(&text, level_str) {
            return;
        }

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
        if url.is_empty() || url.starts_with("chrome-") || url == "about:blank" {
            return;
        }

        *self.current_url.lock_guard() = Some(url.to_string());
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
        Self::new()
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
fn finalize_network_item(entry: PendingRequest) -> NetworkItem {
    NetworkItem {
        timestamp: now_iso(),
        method: entry.method,
        url_redacted: entry.url,
        status: entry.status,
        resource_type: entry.resource_type,
        error_text: entry.error_text,
        body: None,
    }
}

// ── Free functions: noise/failure classification ───────────────────────

/// Check if a network item is a failure (status >= 400 OR has `error_text`).
///
/// Matches the Python sidecar's `_is_network_failure` — the CP-B criterion.
pub fn is_network_failure(item: &NetworkItem) -> bool {
    item.status.is_some_and(|s| s >= 400) || item.error_text.is_some()
}

/// Check if a network item is noise (favicon.ico with None/404 status).
fn is_noise_network(item: &NetworkItem) -> bool {
    if NOISE_URL_SUFFIXES
        .iter()
        .any(|suffix| item.url_redacted.ends_with(suffix))
    {
        return item.status.is_none() || item.status == Some(404);
    }
    false
}

/// Check if a console entry is noise (text contains `favicon.ico` at non-error level).
fn is_noise_console(text: &str, level: &str) -> bool {
    text.contains("favicon.ico") && matches!(level, "info" | "warning" | "verbose")
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
            if is_noise_console(&entry.text, &entry.level) {
                return None;
            }
            Some(ConsoleItem {
                timestamp: now_iso(),
                level,
                text_redacted: entry.text,
                source: Some("console-api".to_string()),
                line: None,
            })
        })
        .collect()
}

// ── Free functions: summarization ──────────────────────────────────────

/// Build a `NetworkSummary` from a list of network items.
///
/// Ported from the Python sidecar's `summarize_network`.
pub fn summarize_network(items: &[NetworkItem], limit: usize) -> NetworkSummary {
    let failures: Vec<NetworkItem> = items
        .iter()
        .filter(|i| is_network_failure(i))
        .cloned()
        .collect();
    let failed_count = failures.len() as u32;
    let request_count = items.len() as u32;
    let recent_failures = failures.into_iter().take(limit).collect();
    let recent_requests = items.iter().take(limit).cloned().collect();
    NetworkSummary {
        failed_count,
        recent_failures,
        request_count,
        recent_requests,
    }
}

/// Build a `ConsoleSummary` from a list of console items.
///
/// Ported from the Python sidecar's `summarize_console`. Only Error-level items
/// are stored in `recent_errors`; Warning items are counted but not stored
/// (matching the contract).
pub fn summarize_console(items: &[ConsoleItem], limit: usize) -> ConsoleSummary {
    let errors: Vec<ConsoleItem> = items
        .iter()
        .filter(|i| i.level == ConsoleLevel::Error)
        .cloned()
        .collect();
    let warning_count = items
        .iter()
        .filter(|i| i.level == ConsoleLevel::Warning)
        .count() as u32;
    let error_count = errors.len() as u32;
    let recent_errors = errors.into_iter().take(limit).collect();
    ConsoleSummary {
        error_count,
        warning_count,
        recent_errors,
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
    include_bodies: bool,
    limit: usize,
) -> NetworkDebugPayload {
    let mut items: Vec<NetworkItem> = history
        .iter()
        .filter(|(_, seq)| *seq >= since_action_seq)
        .map(|(item, _)| item.clone())
        .collect();

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
pub fn merge_network_history(
    history: &mut Vec<(NetworkItem, u64)>,
    fresh: Vec<NetworkItem>,
    action_seq: u64,
) {
    let mut seen: HashSet<String> = history
        .iter()
        .map(|(item, _)| network_item_key(item))
        .collect();
    for item in fresh {
        let key = network_item_key(&item);
        if seen.contains(&key) {
            continue;
        }
        history.push((item, action_seq));
        seen.insert(key);
    }
    let start = history.len().saturating_sub(MAX_HISTORY_ITEMS);
    if start > 0 {
        history.drain(..start);
    }
}

/// Append new console items to history, deduplicating by content key.
///
/// Ported from the Python sidecar's `_merge_history` (console variant).
pub fn merge_console_history(
    history: &mut Vec<(ConsoleItem, u64)>,
    fresh: Vec<ConsoleItem>,
    action_seq: u64,
) {
    let mut seen: HashSet<String> = history
        .iter()
        .map(|(item, _)| console_item_key(item))
        .collect();
    for item in fresh {
        let key = console_item_key(&item);
        if seen.contains(&key) {
            continue;
        }
        history.push((item, action_seq));
        seen.insert(key);
    }
    let start = history.len().saturating_sub(MAX_HISTORY_ITEMS);
    if start > 0 {
        history.drain(..start);
    }
}

/// Dedup key for a network item (timestamp|method|url|status|resource_type|error_text).
fn network_item_key(item: &NetworkItem) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        item.timestamp,
        item.method,
        item.url_redacted,
        item.status.map(|s| s.to_string()).unwrap_or_default(),
        item.resource_type,
        item.error_text.as_deref().unwrap_or(""),
    )
}

/// Dedup key for a console item (timestamp|level|text|source|line).
fn console_item_key(item: &ConsoleItem) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        item.timestamp,
        match item.level {
            ConsoleLevel::Warning => "warning",
            ConsoleLevel::Error => "error",
        },
        item.text_redacted,
        item.source.as_deref().unwrap_or(""),
        item.line.map(|l| l.to_string()).unwrap_or_default(),
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

    // ── is_noise_network ────────────────────────────────────────────────

    #[test]
    fn noise_favicon_404() {
        let item = net_item("GET", "https://x.com/favicon.ico", Some(404), None);
        assert!(is_noise_network(&item));
    }

    #[test]
    fn noise_favicon_no_status() {
        let item = net_item("GET", "https://x.com/favicon.ico", None, None);
        assert!(is_noise_network(&item));
    }

    #[test]
    fn noise_favicon_200_kept() {
        let item = net_item("GET", "https://x.com/favicon.ico", Some(200), None);
        assert!(!is_noise_network(&item));
    }

    #[test]
    fn noise_non_favicon_kept() {
        let item = net_item("GET", "https://x.com/api/data", Some(404), None);
        assert!(!is_noise_network(&item));
    }

    // ── is_noise_console ────────────────────────────────────────────────

    #[test]
    fn noise_console_favicon_warning() {
        assert!(is_noise_console("GET /favicon.ico 404", "warning"));
    }

    #[test]
    fn noise_console_favicon_error_kept() {
        assert!(!is_noise_console("GET /favicon.ico 404", "error"));
    }

    #[test]
    fn noise_console_non_favicon_kept() {
        assert!(!is_noise_console("real error", "error"));
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
        let payload = build_network_debug_payload(&history, 0, NetworkFilter::Failed, false, 10);
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
        let payload = build_network_debug_payload(&history, 0, NetworkFilter::Xhr, false, 10);
        assert_eq!(payload.items.len(), 1);
    }

    #[test]
    fn debug_network_since_action_seq() {
        let history = vec![
            (net_item("GET", "https://x.com/a", Some(200), None), 1),
            (net_item("GET", "https://x.com/b", Some(200), None), 5),
        ];
        let payload = build_network_debug_payload(&history, 3, NetworkFilter::All, false, 10);
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
        let payload = build_network_debug_payload(&history, 0, NetworkFilter::All, true, 10);
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
        let payload = build_network_debug_payload(&history, 0, NetworkFilter::All, false, 10);
        assert!(payload.items[0].body.is_none());
    }

    // ── build_console_debug_payload ─────────────────────────────────────

    #[test]
    fn debug_console_min_level_error() {
        let history = vec![
            (console_item(ConsoleLevel::Warning, "warn"), 0),
            (console_item(ConsoleLevel::Error, "err"), 0),
        ];
        let payload = build_console_debug_payload(&history, 0, ConsoleLevel::Error, 10);
        assert_eq!(payload.items.len(), 1);
        assert_eq!(payload.items[0].text_redacted, "err");
    }

    #[test]
    fn debug_console_min_level_warning() {
        let history = vec![
            (console_item(ConsoleLevel::Warning, "warn"), 0),
            (console_item(ConsoleLevel::Error, "err"), 0),
        ];
        let payload = build_console_debug_payload(&history, 0, ConsoleLevel::Warning, 10);
        assert_eq!(payload.items.len(), 2);
    }

    // ── merge_network_history ───────────────────────────────────────────

    #[test]
    fn merge_network_dedup() {
        let mut history = Vec::new();
        let item = net_item("GET", "https://x.com/a", Some(200), None);
        merge_network_history(&mut history, vec![item.clone()], 1);
        merge_network_history(&mut history, vec![item], 2);
        assert_eq!(history.len(), 1); // deduped
        assert_eq!(history[0].1, 1); // keeps first action_seq
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
    fn merge_console_dedup() {
        let mut history = Vec::new();
        let item = console_item(ConsoleLevel::Error, "error");
        merge_console_history(&mut history, vec![item.clone()], 1);
        merge_console_history(&mut history, vec![item], 2);
        assert_eq!(history.len(), 1);
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
        }
    }

    fn console_item(level: ConsoleLevel, text: &str) -> ConsoleItem {
        ConsoleItem {
            timestamp: "2026-01-01T00:00:00Z".into(),
            level,
            text_redacted: text.into(),
            source: None,
            line: None,
        }
    }
}
