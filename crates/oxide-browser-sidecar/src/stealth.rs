//! Stealth anti-detection patches — ported from chrome-agent's `setup.rs`.
//!
//! Applies CDP-level patches to make headless Chromium less detectable:
//! - `navigator.webdriver` → `undefined`
//! - `chrome.runtime` mock
//! - Permissions API consistency fix
//! - WebGL vendor/renderer mask
//! - MouseEvent screenX/screenY leak fix
//! - User-Agent override (removes "HeadlessChrome")
//!
//! **Critical:** `Runtime.enable` is NEVER called — it's a detection vector.
//! Console capture uses an injected JS interceptor instead (CP5).

use std::time::Duration;

use serde_json::json;
use tracing::debug;

use crate::cdp::CdpClient;

/// CDP command timeout for stealth patches.
const STEALTH_TIMEOUT: Duration = Duration::from_secs(5);

/// Stealth patches injected via `Page.addScriptToEvaluateOnNewDocument`.
///
/// Ported verbatim from chrome-agent `setup.rs` `STEALTH_PATCHES_JS`.
/// Runs before any page JS, survives navigations.
const STEALTH_PATCHES_JS: &str = r#"
    Object.defineProperty(navigator, 'webdriver', { get: () => undefined });
    // Mask chrome.runtime (headless doesn't have it)
    if (!window.chrome) window.chrome = {};
    if (!window.chrome.runtime) window.chrome.runtime = { connect: () => {}, sendMessage: () => {} };
    // Mask Permissions API inconsistency (headless returns "prompt" for notifications)
    const origQuery = window.Permissions && Permissions.prototype.query;
    if (origQuery) {
        Permissions.prototype.query = (params) => (
            params.name === 'notifications'
                ? Promise.resolve({ state: Notification.permission })
                : origQuery.call(Permissions.prototype, params)
        );
    }
    // Mask webGL vendor/renderer (headless gives "Google Inc." / "ANGLE")
    const getParam = WebGLRenderingContext.prototype.getParameter;
    WebGLRenderingContext.prototype.getParameter = function(param) {
        if (param === 37445) return 'Intel Inc.';
        if (param === 37446) return 'Intel Iris OpenGL Engine';
        return getParam.call(this, param);
    };
    // Fix CDP input leak: screenX/screenY == pageX/pageY reveals automation.
    const __screenOffset = { x: Math.floor(Math.random() * 100) + 50, y: Math.floor(Math.random() * 100) + 80 };
    const origMouseEvent = MouseEvent;
    window.MouseEvent = class extends origMouseEvent {
        constructor(type, init = {}) {
            if (init.screenX !== undefined) init.screenX += __screenOffset.x;
            if (init.screenY !== undefined) init.screenY += __screenOffset.y;
            super(type, init);
        }
    };
"#;

/// User-Agent string that replaces the headless UA (removes "HeadlessChrome").
const STEALTH_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// Apply stealth anti-detection patches.
///
/// Must be called after `Page.enable`. Does NOT call `Runtime.enable`.
///
/// # Patches applied
///
/// 1. `Page.addScriptToEvaluateOnNewDocument` — injects `STEALTH_PATCHES_JS`
///    before any page JS runs. Survives navigations.
/// 2. `Runtime.evaluate` — patches `navigator.webdriver` on the current page
///    (in case we connected mid-session). Does NOT enable the Runtime domain.
/// 3. `Network.setUserAgentOverride` — replaces the User-Agent string to
///    remove "HeadlessChrome".
pub async fn apply_stealth(cdp: &CdpClient) {
    // 1. Inject stealth patches before any page JS runs (survives navigations).
    let _ = cdp
        .send_command(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": STEALTH_PATCHES_JS }),
            STEALTH_TIMEOUT,
        )
        .await;
    debug!("injected stealth patches via addScriptToEvaluateOnNewDocument");

    // 2. Patch the current page immediately (in case we connected mid-session).
    //    Runtime.evaluate works WITHOUT Runtime.enable — verified in CP0.
    let _ = cdp
        .send_command(
            "Runtime.evaluate",
            json!({
                "expression": "Object.defineProperty(navigator, 'webdriver', { get: () => undefined });"
            }),
            STEALTH_TIMEOUT,
        )
        .await;
    debug!("patched navigator.webdriver on current page");

    // 3. Override user-agent to remove "HeadlessChrome".
    let _ = cdp
        .send_command(
            "Network.setUserAgentOverride",
            json!({
                "userAgent": STEALTH_USER_AGENT,
                "acceptLanguage": "en-US,en;q=0.9",
                "platform": "MacIntel"
            }),
            STEALTH_TIMEOUT,
        )
        .await;
    debug!("overrode user-agent to remove HeadlessChrome");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stealth_patches_js_contains_key_patches() {
        assert!(STEALTH_PATCHES_JS.contains("webdriver"));
        assert!(STEALTH_PATCHES_JS.contains("chrome.runtime"));
        assert!(STEALTH_PATCHES_JS.contains("Permissions"));
        assert!(STEALTH_PATCHES_JS.contains("WebGLRenderingContext"));
        assert!(STEALTH_PATCHES_JS.contains("screenX"));
        assert!(STEALTH_PATCHES_JS.contains("screenY"));
    }

    #[test]
    fn stealth_user_agent_has_no_headless_chrome() {
        assert!(!STEALTH_USER_AGENT.contains("HeadlessChrome"));
        assert!(STEALTH_USER_AGENT.contains("Chrome/131"));
    }

    #[test]
    fn stealth_patches_js_does_not_call_runtime_enable() {
        // The JS string must not contain any reference to Runtime.enable.
        // (This is a JS-side check; the CDP-level check is in the Rust code.)
        assert!(!STEALTH_PATCHES_JS.contains("Runtime.enable"));
    }
}
