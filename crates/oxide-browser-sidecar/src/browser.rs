//! Chromium process lifecycle — launch, discover DevTools, connect CDP, shutdown.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tracing::{debug, info, warn};

use crate::cdp::CdpClient;
use oxide_browser_contracts::Viewport;

/// Fallback Chromium binary name when no system Chrome is found and
/// `CHROMIUM_BIN` is not set.
const DEFAULT_CHROMIUM_BIN: &str = "chromium";

/// System Chrome binary names to try (in priority order) when `CHROMIUM_BIN`
/// is not set.  Real Chrome has a different binary fingerprint than bundled
/// Chromium — the 2026 benchmark shows this matters as much as JS patches.
const SYSTEM_CHROME_CANDIDATES: &[&str] = &["google-chrome", "google-chrome-stable"];

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
        let chromium_bin = resolve_chromium_binary();

        let user_data_dir = tempfile::tempdir().context("create temp user-data-dir")?;
        let dir_path = user_data_dir.path().to_path_buf();

        let args = build_launch_args(viewport, &dir_path);

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

/// Resolve which Chromium binary to launch.
///
/// Resolution order:
/// 1. `CHROMIUM_BIN` env var (if set and non-empty) — highest priority
/// 2. `google-chrome` found in `PATH` — system Chrome preferred
/// 3. `google-chrome-stable` found in `PATH`
/// 4. `chromium` — fallback (assumes it is in `PATH`)
fn resolve_chromium_binary() -> String {
    let env_bin = std::env::var("CHROMIUM_BIN").ok().filter(|s| !s.is_empty());
    let path_env = std::env::var("PATH").unwrap_or_default();
    resolve_chromium_binary_impl(env_bin.as_deref(), &path_env)
}

/// Pure resolution logic — extracted for testability without env mutation.
fn resolve_chromium_binary_impl(env_bin: Option<&str>, path_env: &str) -> String {
    if let Some(bin) = env_bin
        && !bin.is_empty()
    {
        return bin.to_string();
    }
    for candidate in SYSTEM_CHROME_CANDIDATES {
        if let Some(path) = find_in_path(candidate, path_env) {
            return path.to_string_lossy().into_owned();
        }
    }
    DEFAULT_CHROMIUM_BIN.to_string()
}

/// Search for an executable by name in the given PATH string (colon-separated).
///
/// If `name` contains `/`, it is treated as a direct path.  Otherwise each
/// directory in `path_env` is checked for an executable file.
fn find_in_path(name: &str, path_env: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let is_executable = |path: &Path| -> bool {
        std::fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };

    if name.contains('/') {
        let path = Path::new(name);
        return if is_executable(path) {
            Some(path.to_path_buf())
        } else {
            None
        };
    }

    for dir in path_env.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Build Chromium launch arguments for the given viewport and user-data-dir.
///
/// Extracted from `ChromiumProcess::launch` for testability.
///
/// Anti-detection flags are aligned with Patchright (`chromiumSwitchesPatch.ts`):
/// - `--disable-blink-features=AutomationControlled` — Blink sets
///   `navigator.webdriver = false` at C++ level (undetectable, unlike a JS
///   override which returns `undefined` and installs a custom getter).
/// - `--disable-features=...` — removes automation-specific feature disables
///   that real Chrome does not set.
/// - Removed: `--disable-extensions`, `--disable-popup-blocking`,
///   `--disable-gpu` — each is a fingerprint that distinguishes automation
///   from real Chrome.
fn build_launch_args(viewport: &Viewport, user_data_dir: &Path) -> Vec<String> {
    let mut args = vec![
        "--headless=new".to_string(),
        "--no-sandbox".to_string(),
        "--disable-setuid-sandbox".to_string(),
        "--disable-dev-shm-usage".to_string(),
        "--disable-blink-features=AutomationControlled".to_string(),
        "--disable-features=ImprovedCookieControls,LazyFrameLoading,GlobalMediaControls,DestroyProfileOnBrowserClose,MediaRouter,DialMediaRouteProvider,AcceptCHFrame,AutoExpandDetailsElement,CertificateTransparencyComponentUpdater,AvoidUnnecessaryBeforeUnloadCheckSync,Translate,HttpsUpgrades,PaintHolding,ThirdPartyStoragePartitioning,LensOverlay,PlzDedicatedWorker".to_string(),
        "--remote-debugging-port=0".to_string(),
        "--remote-debugging-address=127.0.0.1".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        format!("--user-data-dir={}", user_data_dir.display()),
        format!("--window-size={},{}", viewport.width, viewport.height),
    ];

    if viewport.device_scale_factor != 1.0 {
        args.push(format!(
            "--force-device-scale-factor={}",
            viewport.device_scale_factor
        ));
    }

    args
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

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_browser_contracts::Viewport;

    fn default_viewport() -> Viewport {
        Viewport {
            width: 1280,
            height: 720,
            device_scale_factor: 1.0,
        }
    }

    #[test]
    fn launch_args_include_anti_detection_flags() {
        let dir = tempfile::tempdir().unwrap();
        let args = build_launch_args(&default_viewport(), dir.path());

        // Blink-level webdriver=false — undetectable, unlike JS override.
        assert!(
            args.iter()
                .any(|a| a == "--disable-blink-features=AutomationControlled"),
            "must include --disable-blink-features=AutomationControlled"
        );

        // Patchright --disable-features list (chromiumSwitchesPatch.ts:33).
        let disable_features = args
            .iter()
            .find(|a| a.starts_with("--disable-features="))
            .expect("must include --disable-features");
        for feature in [
            "ImprovedCookieControls",
            "LazyFrameLoading",
            "GlobalMediaControls",
            "ThirdPartyStoragePartitioning",
            "PlzDedicatedWorker",
        ] {
            assert!(
                disable_features.contains(feature),
                "--disable-features must contain {feature}"
            );
        }
    }

    #[test]
    fn launch_args_exclude_fingerprint_flags() {
        let dir = tempfile::tempdir().unwrap();
        let args = build_launch_args(&default_viewport(), dir.path());

        // These flags are fingerprints that distinguish automation from real
        // Chrome.  Patchright removes them (chromiumSwitchesPatch.ts:20-33).
        assert!(
            !args.iter().any(|a| a == "--disable-extensions"),
            "must NOT include --disable-extensions (fingerprint)"
        );
        assert!(
            !args.iter().any(|a| a == "--disable-popup-blocking"),
            "must NOT include --disable-popup-blocking (fingerprint)"
        );
        assert!(
            !args.iter().any(|a| a == "--disable-gpu"),
            "must NOT include --disable-gpu (headless giveaway)"
        );
        assert!(
            !args.iter().any(|a| a == "--enable-automation"),
            "must NOT include --enable-automation (automation signal)"
        );
    }

    #[test]
    fn launch_args_include_window_size_and_user_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let args = build_launch_args(&default_viewport(), dir.path());

        assert!(
            args.iter().any(|a| a == "--window-size=1280,720"),
            "must include window-size"
        );
        assert!(
            args.iter().any(|a| a.starts_with("--user-data-dir=")),
            "must include user-data-dir"
        );
    }

    #[test]
    fn launch_args_include_scale_factor_when_non_default() {
        let dir = tempfile::tempdir().unwrap();
        let viewport = Viewport {
            width: 1280,
            height: 720,
            device_scale_factor: 2.0,
        };
        let args = build_launch_args(&viewport, dir.path());

        assert!(
            args.iter().any(|a| a == "--force-device-scale-factor=2"),
            "must include force-device-scale-factor when non-default"
        );
    }

    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn resolve_prefers_env_override_over_system_chrome() {
        let dir = tempfile::tempdir().unwrap();
        let chrome = dir.path().join("google-chrome");
        std::fs::write(&chrome, "#!/bin/sh\n").unwrap();
        make_executable(&chrome);

        let path_env = dir.path().to_string_lossy().to_string();
        let result = resolve_chromium_binary_impl(Some("/custom/chrome"), &path_env);
        assert_eq!(result, "/custom/chrome");
    }

    #[test]
    fn resolve_prefers_google_chrome_over_chromium() {
        let dir = tempfile::tempdir().unwrap();
        let chrome = dir.path().join("google-chrome");
        std::fs::write(&chrome, "#!/bin/sh\n").unwrap();
        make_executable(&chrome);

        let path_env = dir.path().to_string_lossy().to_string();
        let result = resolve_chromium_binary_impl(None, &path_env);
        assert_eq!(result, chrome.to_string_lossy().to_string());
    }

    #[test]
    fn resolve_uses_google_chrome_stable_when_google_chrome_absent() {
        let dir = tempfile::tempdir().unwrap();
        let stable = dir.path().join("google-chrome-stable");
        std::fs::write(&stable, "#!/bin/sh\n").unwrap();
        make_executable(&stable);

        let path_env = dir.path().to_string_lossy().to_string();
        let result = resolve_chromium_binary_impl(None, &path_env);
        assert_eq!(result, stable.to_string_lossy().to_string());
    }

    #[test]
    fn resolve_falls_back_to_chromium_when_no_system_chrome() {
        let result = resolve_chromium_binary_impl(None, "");
        assert_eq!(result, DEFAULT_CHROMIUM_BIN);
    }

    #[test]
    fn resolve_ignores_empty_env_override() {
        let result = resolve_chromium_binary_impl(Some(""), "");
        assert_eq!(result, DEFAULT_CHROMIUM_BIN);
    }

    #[test]
    fn find_in_path_locates_executable() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("mybin");
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        make_executable(&bin);

        let path_env = dir.path().to_string_lossy().to_string();
        let found = find_in_path("mybin", &path_env);
        assert_eq!(found, Some(bin));
    }

    #[test]
    fn find_in_path_ignores_non_executable() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("mybin");
        std::fs::write(&bin, "not executable").unwrap();

        let path_env = dir.path().to_string_lossy().to_string();
        let found = find_in_path("mybin", &path_env);
        assert_eq!(found, None);
    }
}
