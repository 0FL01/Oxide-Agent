//! DOM snapshot — compact list of interactive elements.
//!
//! Ported verbatim from the Python sidecar's `DOM_SNAPSHOT_SCRIPT` (lines
//! 867–920). Collects up to 150 interactive elements (a, button, input,
//! textarea, select, [role=button/link], [data-clipboard-text]) with resolved
//! URLs, data-* attributes, and visible text.

use std::collections::BTreeSet;
use std::time::Duration;

use oxide_browser_contracts::{
    DOM_EXTRACT_DEFAULT_MAX_RESULTS, DOM_EXTRACT_DEFAULT_MAX_TOTAL_CHARS,
    DOM_EXTRACT_DEFAULT_MAX_VALUE_CHARS, DOM_EXTRACT_MAX_FIELDS_LIMIT,
    DOM_EXTRACT_MAX_RESULTS_LIMIT, DOM_EXTRACT_MAX_TOTAL_CHARS_LIMIT,
    DOM_EXTRACT_MAX_VALUE_CHARS_LIMIT, DomExtractField, DomExtractPayload, DomExtractRequest,
    DomSnapshotNode, SidecarErrorBody,
};
use serde::Serialize;
use serde_json::Value;

use crate::cdp::CdpClient;

/// CDP timeout for DOM snapshot eval.
const DOM_TIMEOUT: Duration = Duration::from_secs(15);
const DOM_EXTRACT_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_DOM_EXTRACT_ATTRIBUTE: &str = "innerText";

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

/// Extract bounded structured DOM rows via read-only eval in the isolated world.
///
/// The sidecar owns all projection limits: root match count, field count,
/// per-value characters, and aggregate returned characters. Core/LLM callers
/// provide intent only; they never receive raw element diagnostics such as full
/// `innerHTML`, all properties, or all attributes unless those are explicitly
/// requested as bounded fields.
pub async fn extract_dom(
    cdp: &CdpClient,
    context_id: Option<u64>,
    request: &DomExtractRequest,
) -> Result<DomExtractPayload, SidecarErrorBody> {
    let config = normalize_extract_request(request)?;
    let script = dom_extract_script(&config).map_err(|error| {
        dom_extract_error(
            "dom_extract_invalid_config",
            &format!("failed to encode DOM extract config: {error}"),
            "retry with a simpler selector or field list",
            Value::Null,
            false,
        )
    })?;
    let value = cdp
        .eval_readonly(context_id, &script, DOM_EXTRACT_TIMEOUT)
        .await
        .map_err(|error| {
            dom_extract_error(
                "dom_extract_failed",
                &format!("CDP error evaluating DOM extract script: {error}"),
                "check the selector syntax and retry after the page has finished rendering",
                Value::Null,
                true,
            )
        })?;

    serde_json::from_value::<DomExtractPayload>(value).map_err(|error| {
        dom_extract_error(
            "dom_extract_invalid_shape",
            "DOM extract script returned JSON that does not match the typed contract",
            "inspect browser_debug output before retrying",
            serde_json::json!({"error": error.to_string()}),
            true,
        )
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeDomExtractConfig {
    selector: String,
    fields: Vec<RuntimeDomExtractField>,
    max_results: u32,
    max_fields: u32,
    max_value_chars: u32,
    max_total_chars: u32,
    fields_truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeDomExtractField {
    name: String,
    selector: Option<String>,
    attribute: String,
    max_chars: u32,
}

fn normalize_extract_request(
    request: &DomExtractRequest,
) -> Result<RuntimeDomExtractConfig, SidecarErrorBody> {
    let selector = request.selector.trim();
    if selector.is_empty() {
        return Err(dom_extract_error(
            "dom_extract_invalid_request",
            "dom extract requires a non-empty root selector",
            "set selector to a CSS selector that identifies result rows",
            Value::Null,
            false,
        ));
    }

    let max_results = bounded_limit(
        request.max_results,
        DOM_EXTRACT_DEFAULT_MAX_RESULTS,
        DOM_EXTRACT_MAX_RESULTS_LIMIT,
    );
    let max_value_chars = bounded_limit(
        request.max_value_chars,
        DOM_EXTRACT_DEFAULT_MAX_VALUE_CHARS,
        DOM_EXTRACT_MAX_VALUE_CHARS_LIMIT,
    );
    let max_total_chars = bounded_limit(
        request.max_total_chars,
        DOM_EXTRACT_DEFAULT_MAX_TOTAL_CHARS,
        DOM_EXTRACT_MAX_TOTAL_CHARS_LIMIT,
    );
    let default_attribute = clean_attribute(request.attribute.as_deref());
    let fields_truncated = request.fields.len() > DOM_EXTRACT_MAX_FIELDS_LIMIT as usize;
    let raw_fields: Vec<DomExtractField> = if request.fields.is_empty() {
        vec![DomExtractField {
            name: "value".to_string(),
            selector: None,
            attribute: Some(default_attribute.clone()),
            max_chars: Some(max_value_chars),
        }]
    } else {
        request
            .fields
            .iter()
            .take(DOM_EXTRACT_MAX_FIELDS_LIMIT as usize)
            .cloned()
            .collect()
    };

    let mut seen_names = BTreeSet::new();
    let mut fields = Vec::with_capacity(raw_fields.len());
    for (index, field) in raw_fields.into_iter().enumerate() {
        let name = field.name.trim();
        if name.is_empty() {
            return Err(dom_extract_error(
                "dom_extract_invalid_request",
                &format!("dom extract field at index {index} has an empty name"),
                "set every field name to a stable non-empty identifier",
                Value::Null,
                false,
            ));
        }
        if !seen_names.insert(name.to_string()) {
            return Err(dom_extract_error(
                "dom_extract_invalid_request",
                &format!("dom extract field name '{name}' is duplicated"),
                "use unique field names so returned rows cannot overwrite values",
                Value::Null,
                false,
            ));
        }
        fields.push(RuntimeDomExtractField {
            name: name.to_string(),
            selector: clean_optional_string(field.selector.as_deref()),
            attribute: field
                .attribute
                .as_deref()
                .map(|value| clean_attribute(Some(value)))
                .unwrap_or_else(|| default_attribute.clone()),
            max_chars: field
                .max_chars
                .map(|value| {
                    bounded_limit(value, max_value_chars, DOM_EXTRACT_MAX_VALUE_CHARS_LIMIT)
                })
                .unwrap_or(max_value_chars),
        });
    }

    Ok(RuntimeDomExtractConfig {
        selector: selector.to_string(),
        fields,
        max_results,
        max_fields: DOM_EXTRACT_MAX_FIELDS_LIMIT,
        max_value_chars,
        max_total_chars,
        fields_truncated,
    })
}

fn bounded_limit(value: u32, default_value: u32, max_value: u32) -> u32 {
    let value = if value == 0 { default_value } else { value };
    value.clamp(1, max_value)
}

fn clean_optional_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn clean_attribute(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_DOM_EXTRACT_ATTRIBUTE)
        .to_string()
}

fn dom_extract_script(config: &RuntimeDomExtractConfig) -> Result<String, serde_json::Error> {
    let config_json = serde_json::to_string(config)?;
    Ok(format!(
        r#"(() => {{
  const config = {config_json};
  const roots = Array.from(document.querySelectorAll(config.selector));
  let usedChars = 0;
  let budgetTruncated = false;

  function tagName(el) {{
    return el && el.tagName ? String(el.tagName).toLowerCase() : null;
  }}

  function rawValue(el, attribute) {{
    if (!el) return {{ source: 'missing', found: false, value: null }};
    const attr = String(attribute || 'innerText');
    if (attr === 'href') {{
      const href = typeof el.href === 'string' && el.href.length > 0 ? el.href : el.getAttribute && el.getAttribute('href');
      if (href) {{
        try {{ return {{ source: 'computed', found: true, value: new URL(href, location.href).href }}; }}
        catch (_) {{ return {{ source: 'attribute', found: true, value: href }}; }}
      }}
    }}
    if (attr in el && typeof el[attr] !== 'function') {{
      const value = el[attr];
      if (value !== undefined && value !== null) return {{ source: 'property', found: true, value }};
    }}
    if (el.getAttribute) {{
      const direct = el.getAttribute(attr);
      if (direct !== null) return {{ source: 'attribute', found: true, value: direct }};
      const lower = attr.toLowerCase();
      if (lower !== attr) {{
        const lowered = el.getAttribute(lower);
        if (lowered !== null) return {{ source: 'attribute', found: true, value: lowered }};
      }}
    }}
    return {{ source: 'missing', found: false, value: null }};
  }}

  function boundedValue(raw, attribute, maxChars) {{
    if (!raw.found || raw.value === null || raw.value === undefined) {{
      return {{ attribute, source: raw.source, found: false, value: null, truncated: false, original_chars: 0 }};
    }}
    const text = String(raw.value).trim();
    const originalChars = text.length;
    const remaining = Math.max(0, config.maxTotalChars - usedChars);
    const limit = Math.max(0, Math.min(maxChars, remaining));
    const value = text.slice(0, limit);
    usedChars += value.length;
    const truncated = originalChars > limit;
    if (truncated && originalChars > maxChars) budgetTruncated = true;
    if (truncated && remaining <= maxChars) budgetTruncated = true;
    return {{ attribute, source: raw.source, found: true, value, truncated, original_chars: originalChars }};
  }}

  const rows = [];
  for (let index = 0; index < roots.length && rows.length < config.maxResults; index += 1) {{
    const root = roots[index];
    const fields = {{}};
    for (const field of config.fields) {{
      let target = root;
      if (field.selector) target = root.querySelector(field.selector);
      fields[field.name] = boundedValue(rawValue(target, field.attribute), field.attribute, field.maxChars);
    }}
    rows.push({{ index, tag: tagName(root) || 'unknown', fields }});
    if (usedChars >= config.maxTotalChars) {{
      budgetTruncated = true;
      break;
    }}
  }}

  return {{
    selector: config.selector,
    total_matches: roots.length,
    returned_matches: rows.length,
    truncated: roots.length > rows.length || budgetTruncated || config.fieldsTruncated,
    limits: {{
      max_results: config.maxResults,
      max_fields: config.maxFields,
      max_value_chars: config.maxValueChars,
      max_total_chars: config.maxTotalChars
    }},
    matches: rows
  }};
}})()"#
    ))
}

/// Build a `SidecarErrorBody` for DOM snapshot failures.
fn dom_snapshot_error(code: &str, message: &str, hint: &str, details: Value) -> SidecarErrorBody {
    dom_extract_error(code, message, hint, details, true)
}

fn dom_extract_error(
    code: &str,
    message: &str,
    hint: &str,
    details: Value,
    retryable: bool,
) -> SidecarErrorBody {
    SidecarErrorBody {
        code: code.to_string(),
        message: message.to_string(),
        retryable,
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
    fn dom_extract_normalization_builds_legacy_bounded_field() {
        let request = DomExtractRequest {
            selector: " [data-marker='item'] ".to_string(),
            attribute: Some(" value ".to_string()),
            fields: Vec::new(),
            max_results: 500,
            max_value_chars: 50_000,
            max_total_chars: 50_000,
        };

        let config = normalize_extract_request(&request).expect("normalized");

        assert_eq!(config.selector, "[data-marker='item']");
        assert_eq!(config.max_results, DOM_EXTRACT_MAX_RESULTS_LIMIT);
        assert_eq!(config.max_value_chars, DOM_EXTRACT_MAX_VALUE_CHARS_LIMIT);
        assert_eq!(config.max_total_chars, DOM_EXTRACT_MAX_TOTAL_CHARS_LIMIT);
        assert_eq!(config.fields.len(), 1);
        assert_eq!(config.fields[0].name, "value");
        assert_eq!(config.fields[0].attribute, "value");
        assert_eq!(
            config.fields[0].max_chars,
            DOM_EXTRACT_MAX_VALUE_CHARS_LIMIT
        );
    }

    #[test]
    fn dom_extract_normalization_rejects_duplicate_fields() {
        let request = DomExtractRequest {
            selector: ".row".to_string(),
            attribute: None,
            fields: vec![
                DomExtractField {
                    name: "title".to_string(),
                    selector: None,
                    attribute: None,
                    max_chars: None,
                },
                DomExtractField {
                    name: " title ".to_string(),
                    selector: Some(".name".to_string()),
                    attribute: None,
                    max_chars: None,
                },
            ],
            max_results: 10,
            max_value_chars: 512,
            max_total_chars: 16_000,
        };

        let error = normalize_extract_request(&request).expect_err("duplicate rejected");

        assert_eq!(error.code, "dom_extract_invalid_request");
        assert!(!error.retryable);
        assert!(error.message.contains("duplicated"));
    }

    #[test]
    fn dom_extract_script_uses_bounded_projection_not_raw_dumps() {
        let request = DomExtractRequest {
            selector: ".row".to_string(),
            attribute: None,
            fields: vec![DomExtractField {
                name: "title".to_string(),
                selector: Some(".title".to_string()),
                attribute: Some("innerText".to_string()),
                max_chars: Some(120),
            }],
            max_results: 15,
            max_value_chars: 512,
            max_total_chars: 16_000,
        };
        let config = normalize_extract_request(&request).expect("normalized");
        let script = dom_extract_script(&config).expect("script");

        assert!(script.contains("maxTotalChars"));
        assert!(script.contains("boundedValue"));
        assert!(!script.contains("Object.fromEntries(Array.from(el.attributes"));
        assert!(!script.contains("properties:"));
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
