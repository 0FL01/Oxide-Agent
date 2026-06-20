//! Cookie consent banner auto-dismissal — Consent-O-Matic rule engine.
//!
//! Stripped Consent-O-Matic engine (2,130 lines, zero chrome.* deps) injected
//! via CDP `Page.addScriptToEvaluateOnNewDocument` in a named isolated world
//! (`"consent"`). The engine detects CMP banners via CSS selectors and
//! dismisses them by clicking through the CMP's own UI (reject all consent
//! categories). Rules are loaded from `CONSENT_FILTERS` file path.
//!
//! Enabled by default (`CONSENT_AUTO_DISMISS` unset or not `"false"`/`"0"`).
//! When disabled, no script is injected — zero behavior change, zero stealth
//! impact.

use tracing::{info, warn};

/// Stripped Consent-O-Matic engine JS (classes only, no bootstrap).
///
/// Source: https://github.com/cavi-au/Consent-O-Matic (MIT)
/// Stripped: ES module imports/exports removed, `chrome.*` APIs removed.
/// UI methods (showProgressDialog, hideProgressDialog, enablePip, etc.) are
/// overridden as no-ops by the bootstrap generated in [`build_script`].
const CONSENT_ENGINE_JS: &str = include_str!("consent_engine.js");

/// Consent auto-dismiss configuration.
///
/// Built once at startup. [`ConsentConfig::injection_script`] produces the
/// full JS string (engine classes + bootstrap with rules) that is injected
/// into every page via `Page.addScriptToEvaluateOnNewDocument`.
pub struct ConsentConfig {
    injection_script: String,
}

impl ConsentConfig {
    /// Build from environment variables.
    ///
    /// - `CONSENT_AUTO_DISMISS`: enabled by default. Set to `"false"` or
    ///   `"0"` to disable.
    /// - `CONSENT_FILTERS`: comma-separated paths to Consent-O-Matic
    ///   `Rules.json` files. The first readable file is used.
    ///
    /// Returns `None` if disabled or no rules file could be loaded.
    /// Missing/unreadable/invalid files are logged as warnings but do not
    /// prevent the sidecar from starting.
    pub fn from_env() -> Option<Self> {
        let enabled =
            consent_enabled_from_env_var(std::env::var("CONSENT_AUTO_DISMISS").ok().as_deref());
        if !enabled {
            info!("consent auto-dismiss disabled by CONSENT_AUTO_DISMISS=false");
            return None;
        }

        let filters_env = std::env::var("CONSENT_FILTERS").ok()?;
        let path = filters_env.split(',').next()?.trim();
        if path.is_empty() {
            return None;
        }

        Self::from_rules_path(path)
    }

    /// Build from a rules file path.
    ///
    /// Reads the file, validates JSON, and builds the injection script.
    /// Returns `None` if the file cannot be read or is not valid JSON.
    pub fn from_rules_path(path: &str) -> Option<Self> {
        let rules_json = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) => {
                warn!("consent: failed to read rules from {path}: {e}");
                return None;
            }
        };

        // Validate JSON before injecting — malformed JSON would break the page.
        if serde_json::from_str::<serde_json::Value>(&rules_json).is_err() {
            warn!("consent: rules file {path} is not valid JSON");
            return None;
        }

        let script = build_script(&rules_json);
        info!("consent auto-dismiss enabled (rules from {path})");
        Some(Self {
            injection_script: script,
        })
    }

    /// The full injection script (engine classes + bootstrap + rules).
    ///
    /// Inject via `Page.addScriptToEvaluateOnNewDocument` with
    /// `worldName: "consent"` for stealth-safe isolated world execution.
    pub fn injection_script(&self) -> &str {
        &self.injection_script
    }
}

/// Build the full injection script: engine classes + bootstrap.
///
/// The bootstrap:
/// 1. No-ops UI methods (no extension UI in sidecar context).
/// 2. Sets `hideInsteadOfPIP: true` (no PiP in sidecar).
/// 3. Rejects all consent categories (D/A/B/E/F/X = `false`).
/// 4. Creates `ConsentEngine` with rules — auto-detects and dismisses CMPs.
fn build_script(rules_json: &str) -> String {
    let bootstrap = format!(
        r#"
(function() {{
    if (document.contentType !== "text/html") return;
    // No-op UI methods — no extension UI in sidecar context.
    ConsentEngine.prototype.showProgressDialog = function() {{}};
    ConsentEngine.prototype.hideProgressDialog = function() {{}};
    ConsentEngine.prototype.enablePip = function() {{}};
    ConsentEngine.prototype.calculateProgress = function() {{}};
    ConsentEngine.prototype.updateProgress = function() {{}};
    ConsentEngine.enforceScrollBehaviours = function() {{}};
    // Debug values: all disabled. dontHideProgressDialog=false ensures
    // stopObservers + unHideAll run after consent is saved.
    ConsentEngine.debugValues = {{
        debugLog: false, debugRules: false, debugClicks: false,
        clickDelay: false, paintMatchers: false,
        skipHideMethod: false, skipOpenMethod: false, skipSubmit: false,
        dontHideProgressDialog: false
    }};
    // General settings: hide instead of PiP (no PiP UI in sidecar).
    ConsentEngine.generalSettings = {{ hideInsteadOfPIP: true }};
    // Top frame URL (simplified — no cross-origin iframe access).
    ConsentEngine.topFrameUrl = location.href;
    // Reject all consent categories (D/A/B/E/F/X).
    var consentTypes = {{
        D: false, A: false, B: false, E: false, F: false, X: false
    }};
    // Rules loaded from file by the Rust sidecar.
    var rules = {rules};
    // Create engine — auto-detects and dismisses CMP banners.
    new ConsentEngine(rules, consentTypes, null);
}})();
"#,
        rules = rules_json
    );

    format!("{CONSENT_ENGINE_JS}\n{bootstrap}")
}

/// Parse `CONSENT_AUTO_DISMISS` env value — enabled by default.
///
/// Same semantics as `adblock_enabled_from_env_var`:
/// `None` or any value other than `"false"`/`"0"` → enabled.
fn consent_enabled_from_env_var(value: Option<&str>) -> bool {
    match value {
        Some(v) => v != "false" && v != "0",
        None => true,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── consent_enabled_from_env_var ───────────────────────────────────

    #[test]
    fn enabled_when_env_unset() {
        assert!(consent_enabled_from_env_var(None));
    }

    #[test]
    fn enabled_when_env_true() {
        assert!(consent_enabled_from_env_var(Some("true")));
        assert!(consent_enabled_from_env_var(Some("1")));
        assert!(consent_enabled_from_env_var(Some("yes")));
        assert!(consent_enabled_from_env_var(Some("")));
        assert!(consent_enabled_from_env_var(Some("anything")));
    }

    #[test]
    fn disabled_when_env_false() {
        assert!(!consent_enabled_from_env_var(Some("false")));
        assert!(!consent_enabled_from_env_var(Some("0")));
    }

    // ── build_script ───────────────────────────────────────────────────

    #[test]
    fn build_script_contains_engine_and_bootstrap() {
        let script = build_script(r#"{"TestCMP": {"detectors": [], "methods": []}}"#);
        // Engine classes present.
        assert!(script.contains("class ConsentEngine"));
        assert!(script.contains("class Tools"));
        assert!(script.contains("class CMP"));
        assert!(script.contains("class Action"));
        // Bootstrap present.
        assert!(script.contains("ConsentEngine.prototype.showProgressDialog"));
        assert!(script.contains("ConsentEngine.generalSettings"));
        assert!(script.contains("hideInsteadOfPIP: true"));
        assert!(script.contains("new ConsentEngine(rules, consentTypes, null)"));
        // Rules inlined.
        assert!(script.contains("TestCMP"));
    }

    #[test]
    fn build_script_rejects_all_consent() {
        let script = build_script(r#"{}"#);
        assert!(script.contains("D: false"));
        assert!(script.contains("A: false"));
        assert!(script.contains("B: false"));
        assert!(script.contains("E: false"));
        assert!(script.contains("F: false"));
        assert!(script.contains("X: false"));
    }

    #[test]
    fn build_script_noops_ui_methods() {
        let script = build_script(r#"{}"#);
        assert!(script.contains("showProgressDialog = function() {}"));
        assert!(script.contains("hideProgressDialog = function() {}"));
        assert!(script.contains("enablePip = function() {}"));
        assert!(script.contains("enforceScrollBehaviours = function() {}"));
    }

    #[test]
    fn build_script_skips_non_html() {
        let script = build_script(r#"{}"#);
        assert!(script.contains(r#"document.contentType !== "text/html""#));
    }

    #[test]
    fn build_script_no_chrome_refs() {
        let script = build_script(r#"{}"#);
        // No chrome.runtime or chrome.dom in executable code.
        assert!(!script.contains("chrome.runtime.sendMessage"));
        assert!(!script.contains("chrome.dom.openOrClosedShadowRoot"));
    }

    // ── from_rules_path ────────────────────────────────────────────────

    #[test]
    fn from_rules_path_loads_rules() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rules.json");
        std::fs::write(&path, r#"{"TestCMP": {"detectors": [], "methods": []}}"#)
            .expect("write rules");

        let config =
            ConsentConfig::from_rules_path(path.to_str().expect("path str")).expect("config");
        let script = config.injection_script();
        assert!(script.contains("TestCMP"));
        assert!(script.contains("class ConsentEngine"));
    }

    #[test]
    fn from_rules_path_returns_none_when_file_missing() {
        assert!(ConsentConfig::from_rules_path("/nonexistent/rules.json").is_none());
    }

    #[test]
    fn from_rules_path_returns_none_when_invalid_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "not json {{{").expect("write bad file");

        assert!(ConsentConfig::from_rules_path(path.to_str().expect("path str")).is_none());
    }

    #[test]
    fn from_rules_path_empty_path_returns_none() {
        assert!(ConsentConfig::from_rules_path("").is_none());
    }

    // ── Send + Sync ────────────────────────────────────────────────────

    #[test]
    fn consent_config_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ConsentConfig>();
        assert_send_sync::<std::sync::Arc<ConsentConfig>>();
    }
}
