//! Browser session management — one CDP connection per session, stored in
//! a shared map keyed by session ID.
//!
//! The replaceable parts (Chromium process, CDP client, capture collector,
//! page ID) live inside `BrowserInner` behind a `tokio::sync::Mutex` so
//! `force_reload` can swap them atomically. Session state (URL, title,
//! observation/screenshot counters, network/console history, last screenshot)
//! lives directly on `BrowserSession` behind `std::sync::Mutex` / `AtomicU64`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use oxide_browser_contracts::{
    BrowserObservation, CloseSessionRequest, CloseSessionResponse, ConsoleItem,
    CreateSessionRequest, CreateSessionResponse, NetworkItem, ScreenshotArtifact, SidecarErrorBody,
    Viewport,
};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::adblock::AdblockEngine;
use crate::browser::ChromiumProcess;
use crate::capture::CaptureCollector;
use crate::cdp::CdpClient;

/// Default navigation timeout (matches Python sidecar's 60s goto).
const NAV_TIMEOUT: Duration = Duration::from_secs(60);

/// Replaceable browser connection state — swapped on `force_reload`.
struct BrowserInner {
    chromium: ChromiumProcess,
    cdp: CdpClient,
    capture: Arc<CaptureCollector>,
    page_id: String,
    /// Execution context ID for the isolated world used by read-only
    /// internal JS (DOM queries, snapshots).  `None` if creation failed.
    /// Recreated on every navigation (navigations destroy frames → old
    /// context becomes invalid).
    isolated_context_id: Option<u64>,
}

/// One browser session: Chromium process + CDP client + capture collector + metadata.
///
/// The connection (`BrowserInner`) is behind a `tokio::sync::Mutex` so it can be
/// replaced atomically during `force_reload`. Session state (URL, title, history,
/// counters) is separate and survives across reloads.
pub struct BrowserSession {
    pub id: String,
    pub task_id: String,
    pub viewport: Viewport,
    pub artifact_root: String,
    inner: Mutex<BrowserInner>,
    action_seq: AtomicU64,
    observation_seq: AtomicU64,
    screenshot_seq: AtomicU64,
    url: StdMutex<String>,
    title: StdMutex<String>,
    last_screenshot: StdMutex<Option<ScreenshotArtifact>>,
    last_screenshot_bytes: StdMutex<Option<Vec<u8>>>,
    last_observation: StdMutex<Option<BrowserObservation>>,
    network_history: StdMutex<Vec<(NetworkItem, u64)>>,
    console_history: StdMutex<Vec<(ConsoleItem, u64)>>,
    /// Ad blocking engine. Stored outside `BrowserInner` so it survives
    /// `force_reload`. `None` when ad blocking is disabled.
    adblock: Option<Arc<AdblockEngine>>,
    /// Consent auto-dismiss injection script. Stored outside `BrowserInner`
    /// so it survives `force_reload`. `None` when consent dismissal is
    /// disabled.
    consent_script: Option<Arc<String>>,
}

impl BrowserSession {
    /// Launch Chromium, connect CDP, start capture, navigate to `start_url`.
    ///
    /// `adblock` — optional shared ad blocking engine. When `Some`, the
    /// capture collector enables CDP `Fetch.enable` and blocks matching
    /// requests. When `None`, no ad blocking occurs.
    pub async fn new(
        req: &CreateSessionRequest,
        session_id: &str,
        adblock: Option<Arc<AdblockEngine>>,
        consent_script: Option<Arc<String>>,
    ) -> Result<Self> {
        let (chromium, cdp) = ChromiumProcess::launch(&req.viewport)
            .await
            .context("launch Chromium")?;

        let page_id = chromium.page_target_id().to_string();

        // Start capture collector before navigation so we catch the initial
        // page load's network events. Runs on the same CDP WebSocket (G3)
        // and never sends Runtime enable (G4).
        let capture = Arc::new(CaptureCollector::new(
            adblock.clone(),
            consent_script.clone(),
        ));
        CaptureCollector::start(&cdp, capture.clone())
            .await
            .context("start capture collector")?;

        let mut start_url = req
            .start_url
            .clone()
            .unwrap_or_else(|| "https://www.google.com".to_string());
        if start_url == "about:blank" {
            start_url = "https://www.google.com".to_string();
        }

        // Navigate to the start URL (with stealth patches on first navigation).
        let isolated_context_id = navigate_to(&cdp, &start_url, NAV_TIMEOUT, true)
            .await
            .context("initial navigation")?;

        let artifact_root = format!(
            "artifact://browser/{}/{}/",
            safe(&req.task_id),
            safe(session_id)
        );

        info!(session_id, task_id = %req.task_id, page_id = %page_id, url = %start_url, "session created");

        Ok(Self {
            id: session_id.to_string(),
            task_id: req.task_id.clone(),
            viewport: req.viewport,
            artifact_root,
            inner: Mutex::new(BrowserInner {
                chromium,
                cdp,
                capture,
                page_id,
                isolated_context_id,
            }),
            action_seq: AtomicU64::new(0),
            observation_seq: AtomicU64::new(0),
            screenshot_seq: AtomicU64::new(0),
            url: StdMutex::new(String::new()),
            title: StdMutex::new(String::new()),
            last_screenshot: StdMutex::new(None),
            last_screenshot_bytes: StdMutex::new(None),
            last_observation: StdMutex::new(None),
            network_history: StdMutex::new(Vec::new()),
            console_history: StdMutex::new(Vec::new()),
            adblock,
            consent_script,
        })
    }

    /// Clone the CDP client out of the inner connection (cheap — `CdpClient` is `Arc`-backed).
    pub async fn cdp(&self) -> CdpClient {
        self.inner.lock().await.cdp.clone()
    }

    /// Clone the capture collector out of the inner connection.
    pub async fn capture(&self) -> Arc<CaptureCollector> {
        self.inner.lock().await.capture.clone()
    }

    /// Get the current page target ID.
    pub async fn page_id(&self) -> String {
        self.inner.lock().await.page_id.clone()
    }

    /// Get the isolated world execution context ID, if one was created.
    ///
    /// `None` if isolated world creation failed during the last navigation.
    /// Read-only internal JS should check this and fall back to main-world
    /// eval when `None`.
    pub async fn isolated_context_id(&self) -> Option<u64> {
        self.inner.lock().await.isolated_context_id
    }

    /// Navigate to a URL via `Page.navigate` and wait for load event.
    pub async fn navigate(&self, url: &str, timeout: Duration) -> Result<()> {
        let cdp = self.cdp().await;
        let context_id = navigate_to(&cdp, url, timeout, false).await?;
        self.inner.lock().await.isolated_context_id = context_id;
        Ok(())
    }

    /// Force a full browser reload: shut down Chromium, relaunch, reconnect CDP,
    /// restart capture, navigate to `url`. Preserves session state (history,
    /// counters) but drops the JS heap and in-memory SPA state.
    ///
    /// Matches the Python sidecar's `restart_pipe_with_fresh_browser` — closes
    /// the managed browser without purging the profile, then navigates to the
    /// target URL including its hash.
    pub async fn force_reload(&self, url: &str) -> Result<()> {
        let mut inner = self.inner.lock().await;

        // Shut down old Chromium (preserves profile data — no purge).
        if let Err(e) = inner.chromium.shutdown().await {
            warn!("force_reload: old Chromium shutdown error: {e:#}");
        }

        // Launch fresh Chromium.
        let (new_chromium, new_cdp) = ChromiumProcess::launch(&self.viewport)
            .await
            .context("force_reload: launch Chromium")?;

        let new_page_id = new_chromium.page_target_id().to_string();

        // Start fresh capture collector (reuse the same adblock engine
        // and consent script).
        let new_capture = Arc::new(CaptureCollector::new(
            self.adblock.clone(),
            self.consent_script.clone(),
        ));
        CaptureCollector::start(&new_cdp, new_capture.clone())
            .await
            .context("force_reload: start capture collector")?;

        // Navigate to the target URL (with stealth patches — fresh browser).
        let isolated_context_id = navigate_to(&new_cdp, url, NAV_TIMEOUT, true)
            .await
            .context("force_reload: navigate")?;

        // Reset transient state.
        *self.url.lock().unwrap_or_else(|e| e.into_inner()) = String::new();
        *self.title.lock().unwrap_or_else(|e| e.into_inner()) = String::new();
        *self
            .last_screenshot
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;

        // Swap inner connection.
        inner.chromium = new_chromium;
        inner.cdp = new_cdp;
        inner.capture = new_capture;
        inner.page_id = new_page_id;
        inner.isolated_context_id = isolated_context_id;

        info!(session_id = %self.id, url, "force_reload complete");
        Ok(())
    }

    /// Increment and return the action sequence number.
    pub fn next_action_seq(&self) -> u64 {
        self.action_seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Increment and return the observation sequence number.
    pub fn next_observation_seq(&self) -> u64 {
        self.observation_seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Increment and return the screenshot sequence number.
    pub fn next_screenshot_seq(&self) -> u64 {
        self.screenshot_seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Get the current page URL.
    pub fn url(&self) -> String {
        self.url.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Set the current page URL.
    pub fn set_url(&self, url: impl Into<String>) {
        *self.url.lock().unwrap_or_else(|e| e.into_inner()) = url.into();
    }

    /// Get the current page title.
    pub fn title(&self) -> String {
        self.title.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Set the current page title.
    pub fn set_title(&self, title: impl Into<String>) {
        *self.title.lock().unwrap_or_else(|e| e.into_inner()) = title.into();
    }

    /// Get the last screenshot artifact (for non-fresh observe / screenshot/latest).
    pub fn last_screenshot(&self) -> Option<ScreenshotArtifact> {
        self.last_screenshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Set the last screenshot artifact.
    pub fn set_last_screenshot(&self, screenshot: ScreenshotArtifact) {
        *self
            .last_screenshot
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(screenshot);
    }

    /// Get the latest screenshot JPEG bytes (for the binary endpoint).
    pub fn latest_screenshot_bytes(&self) -> Option<Vec<u8>> {
        self.last_screenshot_bytes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Set the latest screenshot JPEG bytes.
    pub fn set_latest_screenshot_bytes(&self, bytes: Vec<u8>) {
        *self
            .last_screenshot_bytes
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(bytes);
    }

    /// Get the last observation (for `fresh=false` observe requests).
    pub fn last_observation(&self) -> Option<BrowserObservation> {
        self.last_observation
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Set the last observation.
    pub fn set_last_observation(&self, observation: BrowserObservation) {
        *self
            .last_observation
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(observation);
    }

    /// Merge fresh network items into history (dedup by key, tag with action_seq).
    pub fn merge_network_history(&self, fresh: Vec<NetworkItem>, action_seq: u64) {
        let mut history = self
            .network_history
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::capture::merge_network_history(&mut history, fresh, action_seq);
    }

    /// Merge fresh console items into history (dedup by key, tag with action_seq).
    pub fn merge_console_history(&self, fresh: Vec<ConsoleItem>, action_seq: u64) {
        let mut history = self
            .console_history
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::capture::merge_console_history(&mut history, fresh, action_seq);
    }

    /// Get a snapshot of the network history (cloned for building debug payloads).
    pub fn network_history(&self) -> Vec<(NetworkItem, u64)> {
        self.network_history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Get a snapshot of the console history (cloned for building debug payloads).
    pub fn console_history(&self) -> Vec<(ConsoleItem, u64)> {
        self.console_history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Shut down the Chromium process.
    pub async fn shutdown(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.chromium.shutdown().await
    }
}

/// Navigate to a URL via `Page.navigate` and wait for `Page.loadEventFired`.
///
/// Free function so it can be called during session construction before
/// `BrowserSession` is fully assembled. When `stealth` is true, anti-detection
/// patches are applied between `Page.enable` and `Page.navigate` (once per
/// session — `Page.addScriptToEvaluateOnNewDocument` survives navigations).
///
/// After the load event, an isolated execution world is created (best-effort)
/// for internal read-only JS.  Returns `Some(context_id)` on success, `None`
/// if creation failed (read-only evals fall back to main world — less stealthy
/// but functional).
pub async fn navigate_to(
    cdp: &CdpClient,
    url: &str,
    timeout: Duration,
    stealth: bool,
) -> Result<Option<u64>> {
    // Subscribe BEFORE sending commands to avoid missing the load event.
    let mut events = cdp.subscribe();

    // Enable Page domain to receive navigation lifecycle events.
    cdp.send_command("Page.enable", serde_json::Value::Null, timeout)
        .await
        .map_err(|e| anyhow::anyhow!("Page.enable: {e}"))?;

    // Apply stealth patches on first navigation (survives subsequent navigations).
    if stealth {
        crate::stealth::apply_stealth(cdp).await;
    }

    let params = serde_json::json!({"url": url});
    let result = cdp
        .send_command("Page.navigate", params, timeout)
        .await
        .map_err(|e| anyhow::anyhow!("Page.navigate: {e}"))?;

    if let Some(err_text) = result.get("errorText").and_then(|v| v.as_str()) {
        bail!("navigation error: {err_text}");
    }

    // Wait for Page.loadEventFired (with timeout).
    let _ = tokio::time::timeout(timeout, async {
        while let Ok(event) = events.recv().await {
            if event.method == "Page.loadEventFired" {
                break;
            }
        }
    })
    .await;

    // Best-effort: create an isolated world for internal read-only JS.
    // If this fails, read-only evals fall back to main world (less stealthy).
    let context_id = match create_isolated_world_for_page(cdp, timeout).await {
        Ok(id) => Some(id),
        Err(e) => {
            warn!("isolated world creation failed, falling back to main world: {e:#}");
            None
        }
    };

    Ok(context_id)
}

/// Get the main frame ID via `Page.getFrameTree` and create an isolated world
/// in that frame.  Returns the `executionContextId` for the new world.
async fn create_isolated_world_for_page(cdp: &CdpClient, timeout: Duration) -> Result<u64> {
    let frame_tree = cdp
        .send_command("Page.getFrameTree", serde_json::Value::Null, timeout)
        .await
        .map_err(|e| anyhow::anyhow!("Page.getFrameTree: {e}"))?;

    let frame_id = frame_tree
        .get("frameTree")
        .and_then(|ft| ft.get("frame"))
        .and_then(|f| f.get("id"))
        .and_then(|id| id.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing frame id in getFrameTree response"))?;

    cdp.create_isolated_world(frame_id, "oxide_internal", timeout)
        .await
        .map_err(|e| anyhow::anyhow!("createIsolatedWorld: {e}"))
}

/// Shared session store keyed by session ID.
#[derive(Default)]
pub struct SessionManager {
    sessions: Mutex<HashMap<String, Arc<BrowserSession>>>,
    /// Shared ad blocking engine. `None` when ad blocking is disabled.
    /// Cloned (cheap `Arc` clone) into each new `BrowserSession`.
    adblock: Option<Arc<AdblockEngine>>,
    /// Shared consent auto-dismiss injection script. `None` when consent
    /// dismissal is disabled. Cloned (cheap `Arc` clone) into each new
    /// `BrowserSession`.
    consent_script: Option<Arc<String>>,
}

impl SessionManager {
    /// Create a session manager with optional ad blocking and consent
    /// auto-dismissal.
    pub fn new(adblock: Option<Arc<AdblockEngine>>, consent_script: Option<Arc<String>>) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            adblock,
            consent_script,
        }
    }

    /// Create a new browser session.
    pub async fn create(&self, req: CreateSessionRequest) -> CreateSessionResponse {
        let session_id = new_session_id();
        let request_id = new_request_id();

        match BrowserSession::new(
            &req,
            &session_id,
            self.adblock.clone(),
            self.consent_script.clone(),
        )
        .await
        {
            Ok(session) => {
                let viewport = session.viewport;
                let artifact_root = session.artifact_root.clone();

                self.sessions
                    .lock()
                    .await
                    .insert(session_id.clone(), Arc::new(session));

                CreateSessionResponse {
                    request_id,
                    session_id,
                    ok: true,
                    browser: oxide_browser_contracts::BrowserDescriptor {
                        browser_id: "chromium".to_string(),
                        page_id: String::new(), // set by client via observe
                        cdp_connected: true,
                    },
                    viewport,
                    artifact_root,
                    error: None,
                }
            }
            Err(e) => {
                warn!("session creation failed: {e:#}");
                CreateSessionResponse {
                    request_id,
                    session_id,
                    ok: false,
                    browser: oxide_browser_contracts::BrowserDescriptor {
                        browser_id: String::new(),
                        page_id: String::new(),
                        cdp_connected: false,
                    },
                    viewport: req.viewport,
                    artifact_root: String::new(),
                    error: Some(SidecarErrorBody {
                        code: "sidecar_not_ready".to_string(),
                        message: format!("failed to create session: {e:#}"),
                        retryable: true,
                        hint: Some("ensure chromium is available".to_string()),
                        details: serde_json::Value::Null,
                    }),
                }
            }
        }
    }

    /// Get a session by ID.
    pub async fn get(&self, id: &str) -> Option<Arc<BrowserSession>> {
        self.sessions.lock().await.get(id).cloned()
    }

    /// Close and remove a session.
    pub async fn close(&self, id: &str, req: CloseSessionRequest) -> CloseSessionResponse {
        let request_id = new_request_id();

        let session = self.sessions.lock().await.remove(id);

        let Some(session) = session else {
            return CloseSessionResponse {
                request_id,
                session_id: id.to_string(),
                ok: true,
                closed: true,
                profile_purged: req.purge_profile,
                artifacts_kept: req.keep_artifacts,
                error: None,
            };
        };

        let shutdown_result = session.shutdown().await;

        let (ok, error) = match shutdown_result {
            Ok(()) => (true, None),
            Err(e) => {
                warn!("session shutdown error: {e:#}");
                (
                    true, // session was removed; report closed even if kill failed
                    Some(SidecarErrorBody {
                        code: "close_error".to_string(),
                        message: format!("shutdown error: {e:#}"),
                        retryable: false,
                        hint: None,
                        details: serde_json::Value::Null,
                    }),
                )
            }
        };

        // `req.reason` is informational (done/cancelled/error/timeout/user);
        // no special handling needed — the session is always closed.

        CloseSessionResponse {
            request_id,
            session_id: id.to_string(),
            ok,
            closed: true,
            profile_purged: req.purge_profile,
            artifacts_kept: req.keep_artifacts,
            error,
        }
    }
}

// ── Utilities ──────────────────────────────────────────────────────────

/// Generate a request ID: `req-{12 hex chars}`.
pub fn new_request_id() -> String {
    format!("req-{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

/// Generate a session ID: `br-{12 hex chars}`.
fn new_session_id() -> String {
    format!("br-{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

/// Sanitize a string for use in filesystem paths (alphanumeric + `-_`).
pub fn safe(value: &str) -> String {
    let sanitized: String = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}
