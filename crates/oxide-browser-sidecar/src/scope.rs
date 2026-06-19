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

/// Classify a network request by its relationship to the page under test.
///
/// Order (first match wins):
/// 1. Request or owning-document scheme is browser-internal → `BrowserInternal`.
/// 2. Request scheme is `data`/`blob`, URL is a favicon, or it is a canceled
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

    let is_favicon = req_url
        .split(['?', '#'])
        .next()
        .unwrap_or(req_url)
        .ends_with("/favicon.ico");
    let is_aborted = canceled && error_text == Some("net::ERR_ABORTED");
    if req_scheme.as_deref().is_some_and(is_benign_scheme) || is_favicon || is_aborted {
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
/// console output → `SiteRelated`.
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
}
