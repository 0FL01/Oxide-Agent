//! Ad blocking engine — network-level request interception via `adblock-rust`.
//!
//! Wraps `adblock::Engine` (Brave's Rust adblock engine) with a minimal API:
//! construct from filter lists, check individual requests. The engine is
//! `Send + Sync` (built with `default-features = false` to disable the
//! `single-thread` feature) and `check_network_request` takes `&self`, so
//! `Arc<AdblockEngine>` provides shared read-only access across all sessions
//! without a `Mutex`.
//!
//! Ad blocking is **enabled by default** when filter lists are available
//! (`ADBLOCK_FILTERS` env). Set `ADBLOCK_ENABLED=false` to disable. When
//! disabled, no filter lists are loaded and no `Fetch.enable` is sent —
//! zero behavior change, zero stealth impact.

use adblock::{Engine, FilterSet, lists::ParseOptions, request::Request};
use tracing::{info, warn};

/// Wrapper around `adblock::Engine` for network-level ad blocking.
///
/// Construct once at startup, share via `Arc<AdblockEngine>` across sessions.
/// `check_network_request(&self)` is immutable — no `Mutex` needed.
pub struct AdblockEngine {
    engine: Engine,
}

impl AdblockEngine {
    /// Build from an iterator of filter rules (Adblock Plus syntax).
    ///
    /// Uses `FilterSet::add_filters` + `Engine::from_filter_set(optimize=true)`.
    /// Debug mode is off (matched rule text is not retained).
    pub fn from_rules(rules: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        let mut filter_set = FilterSet::new(false);
        filter_set.add_filters(rules, ParseOptions::default());
        let engine = Engine::from_filter_set(filter_set, true);
        Self { engine }
    }

    /// Build from environment variables.
    ///
    /// - `ADBLOCK_ENABLED`: enabled by default. Set to `"false"` or `"0"` to
    ///   disable.
    /// - `ADBLOCK_FILTERS`: comma-separated paths to filter list files.
    ///   Required — without filter lists, ad blocking cannot activate.
    ///
    /// Returns `None` if adblocking is disabled or no filter lists could be
    /// loaded. Missing/unreadable files are logged as warnings but do not
    /// prevent the engine from starting with whatever lists did load.
    pub fn from_env() -> Option<Self> {
        let enabled =
            adblock_enabled_from_env_var(std::env::var("ADBLOCK_ENABLED").ok().as_deref());
        if !enabled {
            info!("adblocking disabled by ADBLOCK_ENABLED=false");
            return None;
        }

        let filters_env = std::env::var("ADBLOCK_FILTERS").ok()?;
        Self::from_filter_paths(filters_env.split(',').map(str::trim))
    }

    /// Build from an iterator of filter list file paths.
    ///
    /// Reads each file, adds its contents to a `FilterSet`, and constructs
    /// an optimized `Engine`. Files that fail to read are logged and skipped.
    /// Returns `None` if no files could be loaded.
    pub fn from_filter_paths<'a>(paths: impl IntoIterator<Item = &'a str>) -> Option<Self> {
        let mut filter_set = FilterSet::new(false);
        let mut loaded = 0u32;

        for path in paths {
            if path.is_empty() {
                continue;
            }
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    filter_set.add_filter_list(&content, ParseOptions::default());
                    loaded += 1;
                }
                Err(e) => warn!(path, error = %e, "failed to read adblock filter list — skipping"),
            }
        }

        if loaded == 0 {
            warn!("no adblock filter lists could be loaded — adblocking disabled");
            return None;
        }

        info!(lists_loaded = loaded, "adblock engine initialized");
        let engine = Engine::from_filter_set(filter_set, true);
        Some(Self { engine })
    }

    /// Check whether a network request should be blocked.
    ///
    /// Builds an `adblock::Request` from the URL, source URL, and resource
    /// type, then calls `engine.check_network_request`. Returns `false`
    /// (fail-open) if the URL is malformed — blocking ads is a feature,
    /// hanging the page on a bad URL is a bug.
    pub fn should_block(&self, url: &str, source_url: &str, resource_type: &str) -> bool {
        let request = match Request::new(url, source_url, resource_type) {
            Ok(req) => req,
            Err(e) => {
                tracing::debug!(url, error = ?e, "adblock: Request::new failed — fail-open");
                return false;
            }
        };
        self.engine.check_network_request(&request).matched
    }
}

/// Determine whether ad blocking is enabled based on the `ADBLOCK_ENABLED`
/// env var value.
///
/// Enabled by default (when the var is unset or any value other than
/// `"false"`/`"0"`). Extracted as a pure function for testability —
/// `unsafe_code = "forbid"` prevents `std::env::set_var` in tests.
fn adblock_enabled_from_env_var(value: Option<&str>) -> bool {
    match value {
        Some(v) => v != "false" && v != "0",
        None => true,
    }
}

/// Map a CDP `ResourceType` string (from `Fetch.requestPaused.resourceType`)
/// to the adblock request type string accepted by `Request::new`.
///
/// CDP sends PascalCase values: `"Script"`, `"XHR"`, `"Fetch"`, etc.
/// This function handles any casing via lowercasing. Unknown types map to
/// `"other"` — fail-open for ad blocking, never crash.
///
/// `Document` maps to `"document"` but is excluded from `Fetch.enable`
/// patterns, so it should never reach the handler. The mapping exists for
/// completeness and defense-in-depth.
pub fn cdp_type_to_adblock(cdp_type: &str) -> &'static str {
    match cdp_type.to_lowercase().as_str() {
        "script" => "script",
        "stylesheet" => "stylesheet",
        "image" => "image",
        "font" => "font",
        "media" => "media",
        "xhr" => "xhr",
        "fetch" => "xhr",
        "websocket" => "websocket",
        "ping" => "ping",
        "eventsource" => "xhr",
        "manifest" => "other",
        "cspviolationreport" => "csp_report",
        "prefetch" => "other",
        "signedexchange" => "other",
        "other" => "other",
        "document" => "document",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── cdp_type_to_adblock ─────────────────────────────────────────────

    #[test]
    fn cdp_type_mapping_all_known_types() {
        assert_eq!(cdp_type_to_adblock("Script"), "script");
        assert_eq!(cdp_type_to_adblock("Stylesheet"), "stylesheet");
        assert_eq!(cdp_type_to_adblock("Image"), "image");
        assert_eq!(cdp_type_to_adblock("Font"), "font");
        assert_eq!(cdp_type_to_adblock("Media"), "media");
        assert_eq!(cdp_type_to_adblock("XHR"), "xhr");
        assert_eq!(cdp_type_to_adblock("Fetch"), "xhr");
        assert_eq!(cdp_type_to_adblock("WebSocket"), "websocket");
        assert_eq!(cdp_type_to_adblock("Ping"), "ping");
        assert_eq!(cdp_type_to_adblock("EventSource"), "xhr");
        assert_eq!(cdp_type_to_adblock("Manifest"), "other");
        assert_eq!(cdp_type_to_adblock("CSPViolationReport"), "csp_report");
        assert_eq!(cdp_type_to_adblock("Prefetch"), "other");
        assert_eq!(cdp_type_to_adblock("SignedExchange"), "other");
        assert_eq!(cdp_type_to_adblock("Other"), "other");
        assert_eq!(cdp_type_to_adblock("Document"), "document");
    }

    #[test]
    fn cdp_type_mapping_case_insensitive() {
        assert_eq!(cdp_type_to_adblock("script"), "script");
        assert_eq!(cdp_type_to_adblock("SCRIPT"), "script");
        assert_eq!(cdp_type_to_adblock("Xhr"), "xhr");
        assert_eq!(cdp_type_to_adblock("FETCH"), "xhr");
    }

    #[test]
    fn cdp_type_mapping_unknown_defaults_to_other() {
        assert_eq!(cdp_type_to_adblock("UnknownType"), "other");
        assert_eq!(cdp_type_to_adblock(""), "other");
        assert_eq!(cdp_type_to_adblock("foobar"), "other");
    }

    // ── AdblockEngine::from_rules + should_block ────────────────────────

    #[test]
    fn engine_blocks_matching_domain() {
        let engine = AdblockEngine::from_rules(["||doubleclick.net^", "||googlesyndication.com^"]);
        assert!(engine.should_block(
            "https://doubleclick.net/ad.js",
            "https://example.com/",
            "script"
        ));
        assert!(engine.should_block(
            "https://www.googlesyndication.com/pagead/show_ads.js",
            "https://example.com/",
            "script"
        ));
    }

    #[test]
    fn engine_allows_non_matching_domain() {
        let engine = AdblockEngine::from_rules(["||doubleclick.net^"]);
        assert!(!engine.should_block(
            "https://example.com/page.html",
            "https://example.com/",
            "document"
        ));
        assert!(!engine.should_block(
            "https://cdn.example.com/style.css",
            "https://example.com/",
            "stylesheet"
        ));
    }

    #[test]
    fn engine_fail_open_on_malformed_url() {
        let engine = AdblockEngine::from_rules(["||doubleclick.net^"]);
        // URL without scheme — Request::new returns HostnameParseError
        assert!(!engine.should_block("not-a-url", "https://example.com/", "script"));
        // Empty URL
        assert!(!engine.should_block("", "https://example.com/", "script"));
    }

    #[test]
    fn engine_works_with_empty_source_url() {
        let engine = AdblockEngine::from_rules(["||ads.example.com^"]);
        // Empty source_url is valid — treated as third-party
        assert!(engine.should_block("https://ads.example.com/ad.js", "", "script"));
    }

    #[test]
    fn engine_respects_exception_rules() {
        let engine = AdblockEngine::from_rules(["||example.com^", "@@||example.com^$document"]);
        // Exception for document type — should NOT block
        assert!(!engine.should_block(
            "https://example.com/index.html",
            "https://example.com/",
            "document"
        ));
        // No exception for script type — should block
        assert!(engine.should_block("https://example.com/ad.js", "https://other.com/", "script"));
    }

    // ── from_env ────────────────────────────────────────────────────────

    #[test]
    fn from_filter_paths_loads_real_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("filters.txt");
        std::fs::write(&path, "||ads.example.com^\n||tracker.example.com^\n")
            .expect("write filter file");

        let engine = AdblockEngine::from_filter_paths([path.to_str().expect("path str")])
            .expect("engine from filter paths");
        assert!(engine.should_block(
            "https://ads.example.com/ad.js",
            "https://example.com/",
            "script"
        ));
        assert!(engine.should_block(
            "https://tracker.example.com/pixel.gif",
            "https://example.com/",
            "image"
        ));
        assert!(!engine.should_block(
            "https://example.com/page.html",
            "https://example.com/",
            "document"
        ));
    }

    #[test]
    fn from_filter_paths_skips_nonexistent_loads_valid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let valid = dir.path().join("good.txt");
        std::fs::write(&valid, "||ads.example.com^").expect("write");

        let engine = AdblockEngine::from_filter_paths([
            "/nonexistent/bad.txt",
            valid.to_str().expect("path"),
        ])
        .expect("engine should load from the valid file");
        assert!(engine.should_block(
            "https://ads.example.com/ad.js",
            "https://example.com/",
            "script"
        ));
    }

    #[test]
    fn from_filter_paths_all_nonexistent_returns_none() {
        let result = AdblockEngine::from_filter_paths(["/nonexistent/a.txt", "/nonexistent/b.txt"]);
        assert!(result.is_none());
    }

    #[test]
    fn from_filter_paths_empty_iterator_returns_none() {
        let result: Option<AdblockEngine> = AdblockEngine::from_filter_paths(Vec::<&str>::new());
        assert!(result.is_none());
    }

    #[test]
    fn from_filter_paths_skips_empty_strings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let valid = dir.path().join("good.txt");
        std::fs::write(&valid, "||ads.example.com^").expect("write");

        let engine = AdblockEngine::from_filter_paths(["", "   ", valid.to_str().expect("path")])
            .expect("engine should skip empty strings and load valid file");
        assert!(engine.should_block(
            "https://ads.example.com/ad.js",
            "https://example.com/",
            "script"
        ));
    }

    // ── adblock_enabled_from_env_var ───────────────────────────────────

    #[test]
    fn enabled_by_default_when_unset() {
        assert!(adblock_enabled_from_env_var(None));
    }

    #[test]
    fn enabled_when_true() {
        assert!(adblock_enabled_from_env_var(Some("true")));
        assert!(adblock_enabled_from_env_var(Some("1")));
        assert!(adblock_enabled_from_env_var(Some("yes")));
    }

    #[test]
    fn disabled_when_false_or_zero() {
        assert!(!adblock_enabled_from_env_var(Some("false")));
        assert!(!adblock_enabled_from_env_var(Some("0")));
    }

    #[test]
    fn enabled_when_empty_string() {
        // Empty string is not "false" or "0" — enabled by default
        assert!(adblock_enabled_from_env_var(Some("")));
    }

    // ── Send + Sync ─────────────────────────────────────────────────────

    #[test]
    fn adblock_engine_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AdblockEngine>();
        assert_send_sync::<std::sync::Arc<AdblockEngine>>();
    }
}
