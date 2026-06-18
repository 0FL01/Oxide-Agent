//! Chromium process lifecycle — launch, discover DevTools, connect CDP, shutdown.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tracing::{debug, info, warn};

use crate::cdp::CdpClient;
use oxide_browser_contracts::Viewport;

/// Default Chromium binary name (overridable via `CHROMIUM_BIN` env var).
const DEFAULT_CHROMIUM_BIN: &str = "chromium";

/// Timeout for waiting Chromium DevTools to become ready.
const DEVTOOLS_READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Polling interval for checking DevTools readiness.
const DEVTOOLS_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// A launched Chromium process with an active CDP connection.
pub struct ChromiumProcess {
    child: Child,
    port: u16,
    _user_data_dir: TempDir,
    /// The page target ID from `/json/list`.
    page_target_id: String,
}

/// A DevTools target entry from `/json/list`.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
#[serde(rename_all = "camelCase")]
struct DevToolsTarget {
    #[serde(rename = "type")]
    target_type: String,
    id: String,
    url: String,
    /// Not all targets (e.g. browser-level) expose a WebSocket URL.
    #[serde(default)]
    web_socket_debugger_url: Option<String>,
}

impl ChromiumProcess {
    /// Launch Chromium, wait for DevTools, discover the page target, and
    /// connect a CDP WebSocket.
    ///
    /// Returns the process handle and a connected `CdpClient`.
    pub async fn launch(viewport: &Viewport) -> Result<(Self, CdpClient)> {
        let chromium_bin =
            std::env::var("CHROMIUM_BIN").unwrap_or_else(|_| DEFAULT_CHROMIUM_BIN.to_string());

        let user_data_dir = tempfile::tempdir().context("create temp user-data-dir")?;
        let dir_path = user_data_dir.path().to_path_buf();

        let mut args = vec![
            "--headless=new".to_string(),
            "--no-sandbox".to_string(),
            "--disable-setuid-sandbox".to_string(),
            "--disable-dev-shm-usage".to_string(),
            "--disable-gpu".to_string(),
            "--remote-debugging-port=0".to_string(),
            "--remote-debugging-address=127.0.0.1".to_string(),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            "--disable-extensions".to_string(),
            "--disable-popup-blocking".to_string(),
            format!("--user-data-dir={}", dir_path.display()),
            format!("--window-size={},{}", viewport.width, viewport.height),
        ];

        if viewport.device_scale_factor != 1.0 {
            args.push(format!(
                "--force-device-scale-factor={}",
                viewport.device_scale_factor
            ));
        }

        debug!(bin = %chromium_bin, args = ?args, "launching Chromium");

        let mut child = Command::new(&chromium_bin)
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn Chromium: {chromium_bin}"))?;

        // Drain stdout/stderr to prevent pipe buffer deadlock.
        Self::drain_pipes(&mut child);

        // Wait for DevToolsActivePort file → port.
        let port = Self::wait_for_devtools_port(&dir_path, &mut child)
            .await
            .context("wait for DevTools")?;

        info!(port, "Chromium DevTools ready");

        // Discover page target (may not be immediately available — poll).
        let page_target = Self::wait_for_page_target(port)
            .await
            .context("discover page target")?;

        let ws_url = page_target
            .web_socket_debugger_url
            .clone()
            .unwrap_or_default();
        let page_target_id = page_target.id.clone();

        // Connect CDP WebSocket.
        let (cdp_client, _event_rx) = CdpClient::connect(&ws_url)
            .await
            .map_err(|e| anyhow::anyhow!("CDP connect: {e}"))?;

        info!(port, page_id = %page_target_id, "CDP connected");

        Ok((
            Self {
                child,
                port,
                _user_data_dir: user_data_dir,
                page_target_id,
            },
            cdp_client,
        ))
    }

    /// Port the DevTools HTTP endpoint is listening on.
    #[allow(dead_code)]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Page target ID.
    pub fn page_target_id(&self) -> &str {
        &self.page_target_id
    }

    /// Gracefully shut down Chromium (kill + wait).
    pub async fn shutdown(&mut self) -> Result<()> {
        debug!(port = self.port, "shutting down Chromium");
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        // TempDir is cleaned up automatically on Drop.
        Ok(())
    }

    // ── Internal ──────────────────────────────────────────────────────

    /// Spawn tasks to drain stdout and stderr so the pipe buffer doesn't
    /// fill and deadlock Chromium.
    fn drain_pipes(child: &mut Child) {
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buf = vec![0u8; 4096];
                let mut stdout = stdout;
                loop {
                    match stdout.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buf = vec![0u8; 4096];
                let mut stderr = stderr;
                loop {
                    match stderr.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            });
        }
    }

    /// Wait for the `DevToolsActivePort` file in the user-data-dir.
    ///
    /// Chromium writes this file once DevTools is ready.  The first line is
    /// the port number.  If the child exits before the file appears, we bail.
    async fn wait_for_devtools_port(dir: &Path, child: &mut Child) -> Result<u16> {
        let port_file = dir.join("DevToolsActivePort");
        let deadline = tokio::time::Instant::now() + DEVTOOLS_READY_TIMEOUT;

        loop {
            // Check if Chromium exited unexpectedly.
            match child.try_wait() {
                Ok(Some(status)) => {
                    bail!("Chromium exited early with status {status}");
                }
                Ok(None) => {} // still running
                Err(e) => {
                    bail!("failed to poll Chromium process: {e}");
                }
            }

            if let Ok(content) = tokio::fs::read_to_string(&port_file).await
                && let Some(first_line) = content.lines().next()
                && let Ok(port) = first_line.trim().parse::<u16>()
            {
                return Ok(port);
            }

            if tokio::time::Instant::now() >= deadline {
                bail!("DevToolsActivePort not found within {DEVTOOLS_READY_TIMEOUT:?}");
            }

            tokio::time::sleep(DEVTOOLS_POLL_INTERVAL).await;
        }
    }

    /// Fetch `/json/list` from the DevTools HTTP endpoint.
    async fn list_targets(port: u16) -> Result<Vec<DevToolsTarget>> {
        let url = format!("http://127.0.0.1:{port}/json/list");
        // DevTools HTTP endpoint is HTTP/1.1 only and closes connection after
        // each response — force HTTP/1.1 via a dedicated client.
        let client = reqwest::Client::builder()
            .http1_only()
            .build()
            .context("build reqwest client")?;
        let resp = client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let targets: Vec<DevToolsTarget> = resp.json().await.context("parse /json/list")?;
        Ok(targets)
    }

    /// Poll `/json/list` until a page target with a WebSocket URL appears.
    async fn wait_for_page_target(port: u16) -> Result<DevToolsTarget> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let targets = Self::list_targets(port).await?;
            if let Some(target) = targets
                .iter()
                .find(|t| t.target_type == "page" && t.web_socket_debugger_url.is_some())
            {
                return Ok(DevToolsTarget {
                    target_type: target.target_type.clone(),
                    id: target.id.clone(),
                    url: target.url.clone(),
                    web_socket_debugger_url: target.web_socket_debugger_url.clone(),
                });
            }

            if tokio::time::Instant::now() >= deadline {
                bail!("no page target with WebSocket URL within 10s");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

impl Drop for ChromiumProcess {
    fn drop(&mut self) {
        // Best-effort kill if not already shut down.
        if self.child.try_wait().ok().flatten().is_none() {
            warn!(
                port = self.port,
                "ChromiumProcess dropped without shutdown — killing"
            );
            let _ = self.child.start_kill();
        }
    }
}
