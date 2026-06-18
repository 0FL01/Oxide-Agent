//! Browser session management — one CDP connection per session, stored in
//! a shared map keyed by session ID.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use oxide_browser_contracts::{
    CloseSessionRequest, CloseSessionResponse, CreateSessionRequest, CreateSessionResponse,
    SidecarErrorBody, Viewport,
};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::browser::ChromiumProcess;
use crate::cdp::CdpClient;

/// Default navigation timeout (matches Python sidecar's 60s goto).
const NAV_TIMEOUT: Duration = Duration::from_secs(60);

/// One browser session: Chromium process + CDP client + metadata.
#[allow(dead_code)]
pub struct BrowserSession {
    pub id: String,
    pub task_id: String,
    pub viewport: Viewport,
    chromium: Mutex<ChromiumProcess>,
    cdp: CdpClient,
    pub page_id: String,
    pub artifact_root: String,
    action_seq: AtomicU64,
}

impl BrowserSession {
    /// Launch Chromium, connect CDP, navigate to `start_url`.
    pub async fn new(req: &CreateSessionRequest, session_id: &str) -> Result<Self> {
        let (chromium, cdp) = ChromiumProcess::launch(&req.viewport)
            .await
            .context("launch Chromium")?;

        let page_id = chromium.page_target_id().to_string();

        let mut start_url = req
            .start_url
            .clone()
            .unwrap_or_else(|| "https://www.google.com".to_string());
        if start_url == "about:blank" {
            start_url = "https://www.google.com".to_string();
        }

        // Navigate to the start URL (with stealth patches on first navigation).
        navigate_to(&cdp, &start_url, NAV_TIMEOUT, true)
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
            chromium: Mutex::new(chromium),
            cdp,
            page_id,
            artifact_root,
            action_seq: AtomicU64::new(0),
        })
    }

    /// Navigate to a URL via `Page.navigate` and wait for load event.
    #[allow(dead_code)]
    pub async fn navigate(&self, url: &str, timeout: Duration) -> Result<()> {
        navigate_to(&self.cdp, url, timeout, false).await
    }

    /// Increment and return the action sequence number.
    #[allow(dead_code)]
    pub fn next_action_seq(&self) -> u64 {
        self.action_seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Shut down the Chromium process.
    pub async fn shutdown(&self) -> Result<()> {
        let mut chromium = self.chromium.lock().await;
        chromium.shutdown().await
    }
}

/// Navigate to a URL via `Page.navigate` and wait for `Page.loadEventFired`.
///
/// Free function so it can be called during session construction before
/// `BrowserSession` is fully assembled. When `stealth` is true, anti-detection
/// patches are applied between `Page.enable` and `Page.navigate` (once per
/// session — `Page.addScriptToEvaluateOnNewDocument` survives navigations).
pub async fn navigate_to(
    cdp: &CdpClient,
    url: &str,
    timeout: Duration,
    stealth: bool,
) -> Result<()> {
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

    Ok(())
}

/// Shared session store keyed by session ID.
#[derive(Default)]
pub struct SessionManager {
    sessions: Mutex<HashMap<String, Arc<BrowserSession>>>,
}

impl SessionManager {
    /// Create a new browser session.
    pub async fn create(&self, req: CreateSessionRequest) -> CreateSessionResponse {
        let session_id = new_session_id();
        let request_id = new_request_id();

        match BrowserSession::new(&req, &session_id).await {
            Ok(session) => {
                let viewport = session.viewport;
                let page_id = session.page_id.clone();
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
                        page_id,
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
    #[allow(dead_code)]
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
        // no special handling needed for CP2 — the session is always closed.

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
fn new_request_id() -> String {
    format!("req-{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

/// Generate a session ID: `br-{12 hex chars}`.
fn new_session_id() -> String {
    format!("br-{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

/// Sanitize a string for use in filesystem paths (alphanumeric + `-_`).
fn safe(value: &str) -> String {
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
