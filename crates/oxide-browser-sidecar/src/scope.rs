//! Diagnostic scope classification for captured network/console items.
//!
//! Classifies each captured item by its relationship to the page under test so
//! that compact summaries surface real site errors (first-party XHR/fetch and
//! navigation failures) while suppressing the dominant noise classes verified
//! against real Chromium: the `chrome://new-tab-page` startup storm
//! (browser-internal schemes), third-party subresources (ads/beacons on
//! unrelated hosts), and benign artifacts (`data:`/`blob:` URLs, favicons,
//! canceled aborts).
//!
//! Page identity is the top-level URL (`current_url`, from `frameNavigated` on
//! the main frame); when it is not yet known the request's `documentURL` host
//! is used as a fallback. `initiator.url` is intentionally not used: it is
//! unreliable as page identity (sometimes `{type:"other"}`, sometimes nested
//! under `stack.callFrames[0].url`).
//!
//! Same-site is approximated without a public-suffix list (no new dependency):
//! exact host equality plus a subdomain/parent relationship. The known
//! limitation — cross-domain same-owner hosts (e.g. `example.co.uk` vs a
//! separate CDN domain) are treated as third-party — is acceptable for personal
//! scale and avoids carrying a PSL.

use oxide_browser_contracts::DiagnosticScope;

/// URL schemes that denote browser-internal surfaces rather than real site
/// traffic. Classified as [`DiagnosticScope::BrowserInternal`].
const INTERNAL_SCHEMES: &[&str] = &[
    "chrome",
    "chrome-untrusted",
    "devtools",
    "chrome-extension",
    "edge",
    "about",
    "view-source",
];

/// Extract the lowercased URL scheme (the part before the first `:`).
///
/// Returns `None` when there is no `:` or the candidate scheme contains
/// characters that are not valid in a scheme (RFC 3986: ALPHA / DIGIT / `+` /
/// `-` / `.`, first char ALPHA). This avoids misreading a bare path or host as
/// a scheme.
#[must_use]
pub fn scheme_of(url: &str) -> Option<String> {
    let (scheme, _rest) = url.split_once(':')?;
    if scheme.is_empty() {
        return None;
    }
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    Some(scheme.to_ascii_lowercase())
}

/// Extract the lowercased host of a hierarchical (`scheme://host/...`) URL.
///
/// Strips any `userinfo@` prefix and `:port` suffix. Returns `None` for
/// non-hierarchical URLs (`data:`, `blob:`, `about:`) or when no host is
/// present.
#[must_use]
pub fn host_of(url: &str) -> Option<String> {
    let (_scheme, rest) = url.split_once("://")?;
    // Authority ends at the first '/', '?', or '#'.
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    // Drop userinfo.
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
    // Drop port. IPv6 literals are bracketed; keep the brackets' contents.
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        // `[::1]:8080` -> `::1`
        stripped.split_once(']').map_or(host_port, |(h, _)| h)
    } else {
        host_port.rsplit_once(':').map_or(host_port, |(h, _port)| h)
    };
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// Whether two hosts are same-site by exact match or subdomain/parent
/// relationship (no public-suffix list).
///
/// `api.example.com` and `example.com` are related; `www.example.com` and
/// `example.com` are related; `example.com` and `evil.com` are not.
#[must_use]
pub fn hosts_related(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))
}

fn is_internal_scheme(scheme: &str) -> bool {
    INTERNAL_SCHEMES.contains(&scheme)
}

/// Whether a URL belongs to a browser-internal surface (`chrome://`,
/// `chrome-untrusted://`, `devtools://`, extensions, `about:`, ...).
///
/// Used to keep `current_url` (page identity) from ever being set to an
/// internal surface such as `chrome://new-tab-page/`.
#[must_use]
pub fn is_internal_url(url: &str) -> bool {
    scheme_of(url).as_deref().is_some_and(is_internal_scheme)
}

fn is_benign_scheme(scheme: &str) -> bool {
    matches!(scheme, "data" | "blob" | "filesystem")
}

/// Whether a URL refers to a browser-discovered static asset whose absence
/// (e.g. a 404) is benign diagnostic noise rather than a real site error.
///
/// Covers:
/// - `favicon.ico`, `robots.txt`, `manifest.json` — exact filename match
///   (a bare `.json` like `/api/data.json` is NOT benign).
/// - `apple-touch-icon*` — filename prefix, covers `apple-touch-icon.png`,
///   `apple-touch-icon-precomposed.png`, and sized variants
///   `apple-touch-icon-120x120.png` (browser-discovered iOS home-screen icon).
/// - `*.webmanifest` — PWA web app manifest extension (`site.webmanifest`,
///   `manifest.webmanifest`).
/// - Font files: `.woff`/`.woff2`/`.ttf`/`.otf`/`.eot`, or CDP
///   `resource_type == "font"`.
///
/// Query and fragment are stripped before matching, and matching is
/// case-insensitive on the last path segment. Shared by network and console
/// classification so the benign-asset class is applied consistently across
/// both capture sources.
#[must_use]
pub fn is_benign_static_asset(url: &str, resource_type: Option<&str>) -> bool {
    if resource_type.is_some_and(|rt| rt.eq_ignore_ascii_case("font")) {
        return true;
    }
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let lower = path.to_ascii_lowercase();
    let filename = lower
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(&lower);
    if filename == "favicon.ico"
        || filename == "robots.txt"
        || filename == "manifest.json"
        || filename.starts_with("apple-touch-icon")
    {
        return true;
    }
    let ext = filename.rsplit_once('.').map(|(_, ext)| ext);
    matches!(
        ext,
        Some("woff" | "woff2" | "ttf" | "otf" | "eot" | "webmanifest")
    )
}

/// Classify a network request by its relationship to the page under test.
///
/// Order (first match wins):
/// 1. Request or owning-document scheme is browser-internal → `BrowserInternal`.
/// 2. Request scheme is `data`/`blob`, URL is a benign static asset
///    (`favicon.ico`/`robots.txt`/font files), or it is a canceled
///    `net::ERR_ABORTED` → `Benign`.
/// 3. Otherwise compare the request host to the page host (preferring
///    `page_url`/`current_url`, falling back to `document_url`):
///    - related → `SiteRelated` for the top-level document, else `FirstParty`.
///    - both known but unrelated → `ThirdPartySubresource`.
///    - request host known, page host unknown → fail safe toward surfacing.
///    - request host unknown → `Benign`.
#[must_use]
pub fn classify_network(
    req_url: &str,
    document_url: Option<&str>,
    page_url: Option<&str>,
    resource_type: &str,
    canceled: bool,
    error_text: Option<&str>,
) -> DiagnosticScope {
    let req_scheme = scheme_of(req_url);
    let doc_scheme = document_url.and_then(scheme_of);

    if req_scheme.as_deref().is_some_and(is_internal_scheme)
        || doc_scheme.as_deref().is_some_and(is_internal_scheme)
    {
        return DiagnosticScope::BrowserInternal;
    }

    let is_aborted = canceled && error_text == Some("net::ERR_ABORTED");
    if req_scheme.as_deref().is_some_and(is_benign_scheme)
        || is_benign_static_asset(req_url, Some(resource_type))
        || is_aborted
    {
        return DiagnosticScope::Benign;
    }

    let req_host = host_of(req_url);
    let page_host = page_url
        .and_then(host_of)
        .or_else(|| document_url.and_then(host_of));
    let is_document = resource_type.eq_ignore_ascii_case("document");

    match (req_host, page_host) {
        (Some(req), Some(page)) => {
            if hosts_related(&req, &page) {
                if is_document {
                    DiagnosticScope::SiteRelated
                } else {
                    DiagnosticScope::FirstParty
                }
            } else {
                DiagnosticScope::ThirdPartySubresource
            }
        }
        // Request host known but page identity unknown: fail safe toward
        // surfacing rather than hiding a possibly-real error.
        (Some(_), None) => {
            if is_document {
                DiagnosticScope::SiteRelated
            } else {
                DiagnosticScope::FirstParty
            }
        }
        // No usable request host (already handled data:/blob: above): benign.
        (None, _) => DiagnosticScope::Benign,
    }
}

/// Classify a console entry by its relationship to the page under test.
///
/// Console entries carry the resource `url` for network-sourced errors and no
/// url for page script errors. Page-script errors (no url) are the page's own
/// console output → `SiteRelated`. Network-sourced log entries for missing
/// static assets (`favicon.ico`/`robots.txt`/fonts) are `Benign` so a 404 on
/// those does not inflate `console_summary.error_count`.
#[must_use]
pub fn classify_console(entry_url: Option<&str>, page_url: Option<&str>) -> DiagnosticScope {
    let Some(entry_url) = entry_url.filter(|u| !u.is_empty()) else {
        // No resource URL: page's own console (script error, warning) → surfaced.
        return DiagnosticScope::SiteRelated;
    };

    if scheme_of(entry_url)
        .as_deref()
        .is_some_and(is_internal_scheme)
    {
        return DiagnosticScope::BrowserInternal;
    }
    if scheme_of(entry_url)
        .as_deref()
        .is_some_and(is_benign_scheme)
    {
        return DiagnosticScope::Benign;
    }
    if is_benign_static_asset(entry_url, None) {
        return DiagnosticScope::Benign;
    }

    let entry_host = host_of(entry_url);
    let page_host = page_url.and_then(host_of);
    match (entry_host, page_host) {
        (Some(entry), Some(page)) => {
            if hosts_related(&entry, &page) {
                DiagnosticScope::FirstParty
            } else {
                DiagnosticScope::ThirdPartySubresource
            }
        }
        // Page identity unknown: fail safe toward surfacing.
        (Some(_), None) => DiagnosticScope::FirstParty,
        (None, _) => DiagnosticScope::SiteRelated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_and_host_parsing() {
        assert_eq!(
            scheme_of("https://example.com/a"),
            Some("https".to_string())
        );
        assert_eq!(
            scheme_of("CHROME://new-tab-page/"),
            Some("chrome".to_string())
        );
        assert_eq!(
            scheme_of("data:image/png;base64,AAAA"),
            Some("data".to_string())
        );
        assert_eq!(scheme_of("/relative/path"), None);
        assert_eq!(scheme_of("example.com/x"), None);

        assert_eq!(
            host_of("https://Example.com:8443/a?b#c"),
            Some("example.com".to_string())
        );
        assert_eq!(
            host_of("http://user:pw@api.example.com/x"),
            Some("api.example.com".to_string())
        );
        assert_eq!(host_of("http://[::1]:8080/x"), Some("::1".to_string()));
        assert_eq!(host_of("data:image/png;base64,AAAA"), None);
        assert_eq!(host_of("about:blank"), None);
    }

    #[test]
    fn hosts_related_matches_subdomains() {
        assert!(hosts_related("example.com", "example.com"));
        assert!(hosts_related("api.example.com", "example.com"));
        assert!(hosts_related("example.com", "www.example.com"));
        assert!(!hosts_related("example.com", "evil.com"));
        assert!(!hosts_related("notexample.com", "example.com"));
        assert!(!hosts_related("", "example.com"));
    }

    #[test]
    fn first_party_xhr_failure_is_surfaced() {
        let scope = classify_network(
            "http://127.0.0.1:8080/api/isWritable",
            Some("http://127.0.0.1:8080/"),
            Some("http://127.0.0.1:8080/"),
            "xhr",
            false,
            None,
        );
        assert_eq!(scope, DiagnosticScope::FirstParty);
    }

    #[test]
    fn top_level_document_is_site_related() {
        let scope = classify_network(
            "https://example.com/",
            Some("https://example.com/"),
            Some("https://example.com/"),
            "document",
            false,
            None,
        );
        assert_eq!(scope, DiagnosticScope::SiteRelated);
    }

    #[test]
    fn chrome_new_tab_page_storm_is_internal() {
        let scope = classify_network(
            "chrome://new-tab-page/foo.js",
            Some("chrome://new-tab-page/"),
            Some("http://127.0.0.1:8080/"),
            "script",
            false,
            None,
        );
        assert_eq!(scope, DiagnosticScope::BrowserInternal);
        // Internal even when documentURL is internal but request looks http.
        let scope2 = classify_network(
            "https://www.gstatic.com/x.js",
            Some("chrome://new-tab-page/"),
            None,
            "script",
            false,
            None,
        );
        assert_eq!(scope2, DiagnosticScope::BrowserInternal);
    }

    #[test]
    fn third_party_subresource_is_classified_without_blacklist() {
        let scope = classify_network(
            "https://ogads-pa.clients6.google.com/x",
            Some("http://127.0.0.1:8080/"),
            Some("http://127.0.0.1:8080/"),
            "xhr",
            false,
            None,
        );
        assert_eq!(scope, DiagnosticScope::ThirdPartySubresource);
    }

    #[test]
    fn data_and_favicon_and_abort_are_benign() {
        assert_eq!(
            classify_network(
                "data:image/png;base64,AAAA",
                None,
                Some("http://x/"),
                "image",
                false,
                None
            ),
            DiagnosticScope::Benign
        );
        assert_eq!(
            classify_network(
                "http://x/favicon.ico",
                Some("http://x/"),
                Some("http://x/"),
                "image",
                false,
                Some("net::ERR_FAILED")
            ),
            DiagnosticScope::Benign
        );
        assert_eq!(
            classify_network(
                "http://x/",
                Some("http://x/"),
                Some("http://x/"),
                "document",
                true,
                Some("net::ERR_ABORTED")
            ),
            DiagnosticScope::Benign
        );
    }

    #[test]
    fn console_classification() {
        // Page script error (no url) → surfaced.
        assert_eq!(
            classify_console(None, Some("http://x/")),
            DiagnosticScope::SiteRelated
        );
        // Network-sourced first-party error.
        assert_eq!(
            classify_console(
                Some("http://127.0.0.1:8080/api/isWritable"),
                Some("http://127.0.0.1:8080/")
            ),
            DiagnosticScope::FirstParty
        );
        // chrome:// internal.
        assert_eq!(
            classify_console(Some("chrome://new-tab-page/x"), Some("http://x/")),
            DiagnosticScope::BrowserInternal
        );
        // Third-party host.
        assert_eq!(
            classify_console(
                Some("https://ogads-pa.clients6.google.com/x"),
                Some("http://127.0.0.1:8080/")
            ),
            DiagnosticScope::ThirdPartySubresource
        );
    }

    #[test]
    fn benign_static_asset_classification() {
        // favicon / robots / fonts by path.
        assert!(is_benign_static_asset("https://x/favicon.ico", None));
        assert!(is_benign_static_asset("https://x/robots.txt", None));
        assert!(is_benign_static_asset("https://x/fonts/inter.woff2", None));
        assert!(is_benign_static_asset("https://x/f.woff", None));
        assert!(is_benign_static_asset("https://x/f.ttf", None));
        assert!(is_benign_static_asset("https://x/f.otf", None));
        assert!(is_benign_static_asset("https://x/f.eot", None));
        // PWA manifest + apple-touch-icon variants.
        assert!(is_benign_static_asset("https://x/manifest.json", None));
        assert!(is_benign_static_asset("https://x/site.webmanifest", None));
        assert!(is_benign_static_asset(
            "https://x/manifest.webmanifest",
            None
        ));
        assert!(is_benign_static_asset(
            "https://x/apple-touch-icon.png",
            None
        ));
        assert!(is_benign_static_asset(
            "https://x/apple-touch-icon-precomposed.png",
            None
        ));
        assert!(is_benign_static_asset(
            "https://x/apple-touch-icon-120x120.png",
            None
        ));
        // Font by CDP resource type, arbitrary URL.
        assert!(is_benign_static_asset("https://x/blob", Some("Font")));
        // Query/fragment stripped before matching.
        assert!(is_benign_static_asset("https://x/favicon.ico?v=2", None));
        assert!(is_benign_static_asset("https://x/robots.txt#frag", None));
        assert!(is_benign_static_asset("https://x/manifest.json?v=1", None));
        assert!(is_benign_static_asset(
            "https://x/apple-touch-icon.png#x",
            None
        ));
        // Case-insensitive.
        assert!(is_benign_static_asset("https://x/FAVICON.ICO", None));
        assert!(is_benign_static_asset("https://x/F.WOFF2", None));
        assert!(is_benign_static_asset("https://x/MANIFEST.JSON", None));
        assert!(is_benign_static_asset("https://x/SITE.WEBMANIFEST", None));
        assert!(is_benign_static_asset(
            "https://x/Apple-Touch-Icon.png",
            None
        ));
        // NOT benign: real API / script / stylesheet / bare root.
        assert!(!is_benign_static_asset("https://x/api/isWritable", None));
        assert!(!is_benign_static_asset("https://x/app.js", None));
        assert!(!is_benign_static_asset("https://x/style.css", None));
        assert!(!is_benign_static_asset("https://x/", None));
        // NOT benign: extensionless path that merely contains a font dir name.
        assert!(!is_benign_static_asset("https://x/fonts", None));
        // NOT benign: a .json that is not the PWA manifest.
        assert!(!is_benign_static_asset("https://x/api/data.json", None));
        assert!(!is_benign_static_asset("https://x/config.json", None));
        // NOT benign: apple.png is not an apple-touch-icon.
        assert!(!is_benign_static_asset("https://x/apple.png", None));
    }

    #[test]
    fn console_static_asset_404_is_benign() {
        // favicon/robots/font/manifest/apple-touch-icon 404 from
        // Log.entryAdded is benign, not first-party.
        let cases = [
            "https://site.com/favicon.ico",
            "https://site.com/robots.txt",
            "https://site.com/fonts/inter.woff2",
            "https://site.com/manifest.json",
            "https://site.com/site.webmanifest",
            "https://site.com/apple-touch-icon.png",
        ];
        for url in cases {
            assert_eq!(
                classify_console(Some(url), Some("https://site.com/")),
                DiagnosticScope::Benign,
                "{url} should be benign"
            );
        }
        // Real first-party API failure stays surfaced.
        assert_eq!(
            classify_console(
                Some("https://site.com/api/isWritable"),
                Some("https://site.com/")
            ),
            DiagnosticScope::FirstParty
        );
        // A non-manifest .json 404 stays surfaced.
        assert_eq!(
            classify_console(
                Some("https://site.com/api/data.json"),
                Some("https://site.com/")
            ),
            DiagnosticScope::FirstParty
        );
        // Page-script error (no url) still surfaced.
        assert_eq!(
            classify_console(None, Some("https://site.com/")),
            DiagnosticScope::SiteRelated
        );
    }
}
