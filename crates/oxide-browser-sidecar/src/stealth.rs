//! Stealth anti-detection patches — hardened from Patchright donor.
//!
//! Applies CDP-level patches to make headless Chromium less detectable:
//! - `navigator.webdriver` → handled at C++ level via `--disable-blink-features=AutomationControlled`
//! - `chrome.runtime` mock
//! - Permissions API consistency fix
//! - WebGL vendor/renderer mask
//! - MouseEvent screenX/screenY leak fix
//! - `navigator.serviceWorker.register` no-op (Patchright browserContextPatch.ts:31)
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
/// Hardened from Patchright donor. Runs before any page JS, survives navigations.
/// NOTE: `navigator.webdriver` is NOT patched here — the `--disable-blink-features=
/// AutomationControlled` launch flag makes Blink set `navigator.webdriver = false`
/// at C++ level, which is undetectable. A JS override would change `false`→
/// `undefined` (detectable via `navigator.webdriver === false`) and install a
/// custom getter on the prototype (detectable via descriptor inspection).
const STEALTH_PATCHES_JS: &str = r#"
    // navigator.webdriver is handled by --disable-blink-features=AutomationControlled
    // at the Blink/C++ level — no JS override needed (and one would be harmful).
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
    // Patchright browserContextPatch.ts:31 — no-op service worker registration.
    if (navigator.serviceWorker) navigator.serviceWorker.register = async () => { };
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
/// 2. `Network.setUserAgentOverride` — replaces the User-Agent string to
///    remove "HeadlessChrome".
///
/// `navigator.webdriver` is handled by the `--disable-blink-features=
/// AutomationControlled` launch flag (Blink sets `false` at C++ level) —
/// no JS patch needed.
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

    // 2. Override user-agent to remove "HeadlessChrome".
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
        assert!(STEALTH_PATCHES_JS.contains("chrome.runtime"));
        assert!(STEALTH_PATCHES_JS.contains("Permissions"));
        assert!(STEALTH_PATCHES_JS.contains("WebGLRenderingContext"));
        assert!(STEALTH_PATCHES_JS.contains("screenX"));
        assert!(STEALTH_PATCHES_JS.contains("screenY"));
        // serviceWorker.register no-op (Patchright browserContextPatch.ts:31)
        assert!(STEALTH_PATCHES_JS.contains("navigator.serviceWorker.register"));
    }

    #[test]
    fn stealth_patches_js_does_not_patch_webdriver() {
        // navigator.webdriver is handled by --disable-blink-features=
        // AutomationControlled at the C++ level. A JS override would be
        // detectable (returns undefined instead of false, custom getter).
        // Check for the actual override patterns, not comments mentioning it.
        assert!(
            !STEALTH_PATCHES_JS.contains("Object.defineProperty(navigator, 'webdriver'"),
            "webdriver defineProperty override must not be present — Blink flag handles it"
        );
        assert!(
            !STEALTH_PATCHES_JS.contains("navigator.webdriver ="),
            "webdriver assignment override must not be present — Blink flag handles it"
        );
        assert!(
            !STEALTH_PATCHES_JS.contains("navigator, 'webdriver'"),
            "webdriver descriptor override must not be present — Blink flag handles it"
        );
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
