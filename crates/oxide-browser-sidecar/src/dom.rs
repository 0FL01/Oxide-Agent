//! DOM snapshot — compact list of interactive elements.
//!
//! Ported verbatim from the Python sidecar's `DOM_SNAPSHOT_SCRIPT` (lines
//! 867–920). Collects up to 150 interactive elements (a, button, input,
//! textarea, select, [role=button/link], [data-clipboard-text]) with resolved
//! URLs, data-* attributes, and visible text.

use std::time::Duration;

use oxide_browser_contracts::{DomSnapshotNode, SidecarErrorBody};
use serde_json::Value;

use crate::cdp::CdpClient;

/// CDP timeout for DOM snapshot eval.
const DOM_TIMEOUT: Duration = Duration::from_secs(15);

/// JavaScript that collects interactive elements from the page.
///
/// Returns a JSON string representing an array of objects with fields:
/// `tag`, `selector`, `attributes`, `href`, `value`, `text`.
const DOM_SNAPSHOT_SCRIPT: &str = r#"
(() => {
  const MAX = 150;
  const TEXT_MAX = 200;
  const ATTR_MAX = 512;
  function selectorHint(el) {
    const tag = el.tagName.toLowerCase();
    const parts = [tag];
    if (el.id) parts.push('#' + el.id);
    const testId = el.getAttribute('data-testid');
    if (testId) parts.push('[data-testid="' + String(testId).replace(/"/g, '\\"') + '"]');
    if (el.name) parts.push('[name="' + String(el.name).replace(/"/g, '\\"') + '"]');
    const cls = Array.from(el.classList).slice(0, 2).join('.');
    if (cls) parts.push('.' + cls);
    return parts.join('');
  }
  const candidates = new Set();
  const add = (el) => { if (el) candidates.add(el); };
  document.querySelectorAll('a, button, input, textarea, select, [role="button"], [role="link"], [data-clipboard-text]').forEach(add);
  document.querySelectorAll('[data-clipboard-text]').forEach(add);
  const seen = new Set();
  const result = [];
  for (const el of candidates) {
    if (seen.has(el)) continue;
    seen.add(el);
    if (result.length >= MAX) break;
    const tag = el.tagName.toLowerCase();
    const attrs = {};
    for (const name of el.getAttributeNames()) {
      if (name.startsWith('data-')) {
        attrs[name] = String(el.getAttribute(name) || '').slice(0, ATTR_MAX);
      }
    }
    let href = null;
    if (tag === 'a') {
      try {
        href = new URL(el.href, location.href).href;
      } catch (e) {
        href = el.getAttribute('href');
      }
    }
    let value = null;
    if ('value' in el) {
      value = el.value;
    }
    let text = (el.innerText || '').trim().slice(0, TEXT_MAX);
    if (!text) {
      text = el.getAttribute('aria-label') || el.getAttribute('title') || '';
    }
    result.push({ tag, selector: selectorHint(el), attributes: attrs, href, value, text });
  }
  return JSON.stringify(result);
})()
"#;

/// Capture a DOM snapshot via eval in the isolated world (with main-world
/// fallback).
///
/// Returns `(snapshot, error)` — exactly one is `Some`. On success, `snapshot`
/// is a `Vec<DomSnapshotNode>`. On failure, `error` is a `SidecarErrorBody`
/// matching the Python sidecar's error codes.
///
/// Running in the isolated world hides the `querySelectorAll` call from page
/// JS. `document.querySelectorAll` is a DOM method accessible from any world
/// (it operates on the shared C++ DOM tree), but page JS that monkey-patches
/// `document.querySelectorAll` in the main world does NOT affect the
/// isolated world's fresh DOM wrappers.
pub async fn capture_dom_snapshot(
    cdp: &CdpClient,
    context_id: Option<u64>,
) -> (Option<Vec<DomSnapshotNode>>, Option<SidecarErrorBody>) {
    let value = match cdp
        .eval_readonly(context_id, DOM_SNAPSHOT_SCRIPT, DOM_TIMEOUT)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            return (
                None,
                Some(dom_snapshot_error(
                    "dom_snapshot_failed",
                    &format!("CDP error evaluating DOM snapshot script: {e}"),
                    "inspect action_result and browser_debug output before retrying",
                    Value::Null,
                )),
            );
        }
    };

    // The script returns a JSON string; parse it into an array.
    let json_str = match value.as_str() {
        Some(s) if !s.is_empty() => s,
        _ => {
            return (
                None,
                Some(dom_snapshot_error(
                    "dom_snapshot_empty_result",
                    "DOM snapshot script returned no JSON string",
                    "retry after the page has finished rendering",
                    Value::Null,
                )),
            );
        }
    };

    let parsed: Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(e) => {
            return (
                None,
                Some(dom_snapshot_error(
                    "dom_snapshot_invalid_json",
                    "DOM snapshot script returned invalid JSON",
                    "inspect browser_debug output before retrying",
                    serde_json::json!({"error": e.to_string()}),
                )),
            );
        }
    };

    let arr = match parsed.as_array() {
        Some(a) => a,
        None => {
            return (
                None,
                Some(dom_snapshot_error(
                    "dom_snapshot_invalid_shape",
                    "DOM snapshot script returned JSON that is not an array",
                    "inspect browser_debug output before retrying",
                    Value::Null,
                )),
            );
        }
    };

    let nodes: Vec<DomSnapshotNode> = arr
        .iter()
        .filter_map(|item| serde_json::from_value::<DomSnapshotNode>(item.clone()).ok())
        .collect();

    (Some(nodes), None)
}

/// Build a `SidecarErrorBody` for DOM snapshot failures.
fn dom_snapshot_error(code: &str, message: &str, hint: &str, details: Value) -> SidecarErrorBody {
    SidecarErrorBody {
        code: code.to_string(),
        message: message.to_string(),
        retryable: true,
        hint: Some(hint.to_string()),
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dom_snapshot_script_contains_key_selectors() {
        assert!(DOM_SNAPSHOT_SCRIPT.contains("a, button, input, textarea, select"));
        assert!(DOM_SNAPSHOT_SCRIPT.contains("[role=\"button\"]"));
        assert!(DOM_SNAPSHOT_SCRIPT.contains("[role=\"link\"]"));
        assert!(DOM_SNAPSHOT_SCRIPT.contains("[data-clipboard-text]"));
    }

    #[test]
    fn dom_snapshot_script_has_max_150() {
        assert!(DOM_SNAPSHOT_SCRIPT.contains("const MAX = 150"));
    }

    #[test]
    fn dom_snapshot_error_codes() {
        let err = dom_snapshot_error("dom_snapshot_failed", "test", "hint", Value::Null);
        assert_eq!(err.code, "dom_snapshot_failed");
        assert!(err.retryable);
        assert_eq!(err.hint, Some("hint".to_string()));
    }

    #[test]
    fn dom_snapshot_deserializes_from_js_shape() {
        let json = serde_json::json!([
            {
                "tag": "a",
                "selector": "a#login.btn",
                "attributes": {"data-testid": "login-btn"},
                "href": "https://example.com/login",
                "value": null,
                "text": "Login"
            },
            {
                "tag": "input",
                "selector": "input[name=email]",
                "attributes": {},
                "href": null,
                "value": "user@example.com",
                "text": ""
            }
        ]);

        let nodes: Vec<DomSnapshotNode> = json
            .as_array()
            .expect("array")
            .iter()
            .filter_map(|item| serde_json::from_value::<DomSnapshotNode>(item.clone()).ok())
            .collect();

        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].tag, "a");
        assert_eq!(nodes[0].selector, "a#login.btn");
        assert_eq!(nodes[0].href, Some("https://example.com/login".to_string()));
        assert_eq!(nodes[0].text, Some("Login".to_string()));
        assert_eq!(
            nodes[0].attributes.get("data-testid"),
            Some(&"login-btn".to_string())
        );

        assert_eq!(nodes[1].tag, "input");
        assert_eq!(nodes[1].value, Some("user@example.com".to_string()));
        assert!(nodes[1].text.as_ref().is_some_and(|t| t.is_empty()));
    }
}
