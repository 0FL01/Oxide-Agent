//! BrowserAction → CDP command translation.
//!
//! Each `BrowserAction` variant is translated to native CDP commands:
//! - `ClickXy` / `ClickSelector` — `Input.dispatchMouseEvent` at coordinates
//!   (more realistic than JS `.click()`; verified in CP0).
//! - `Fill` / `TypeText` — semantic input JS ported from the Python sidecar
//!   (React/Vue/Angular native setter events; no CDP equivalent).
//! - `Press` — JS `KeyboardEvent` for all keys (simple + combos).
//! - `Scroll` / `GetElementValue` / `ExecuteJavaScript` — `Runtime.evaluate`.
//! - `Wait` — `tokio::time::sleep`.
//! - `WaitForSelector` / `WaitForText` — polling via `Runtime.evaluate`.
//! - `Script` — iterate steps, break on failure.
//! - `Navigate` — returns `Failed` (handled by `/goto` endpoint, not `/action`).

use std::time::{Duration, Instant};

use oxide_browser_contracts::{ActionResult, ActionStatus, BrowserAction};
use serde_json::{Value, json};

use crate::cdp::CdpClient;

/// CDP command timeout for action operations.
const CDP_TIMEOUT: Duration = Duration::from_secs(15);

/// Polling interval for wait actions.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Execute a `BrowserAction` via CDP and return an `ActionResult`.
///
/// `action_seq` is set by the caller (the REST handler in CP6).
pub async fn execute_action(
    cdp: &CdpClient,
    action: &BrowserAction,
    timeout: Duration,
) -> ActionResult {
    let started = Instant::now();
    let kind = action_kind(action);

    let (status, result) = run_action(cdp, action, timeout).await;

    let duration_ms = started.elapsed().as_millis() as u64;
    let technical_success = matches!(status, ActionStatus::Executed | ActionStatus::NoOp);

    ActionResult {
        action_seq: 0,
        kind: kind.to_string(),
        status,
        duration_ms,
        technical_success,
        hint: None,
        result,
    }
}

/// Dispatch a `BrowserAction` to the appropriate CDP translation.
async fn run_action(
    cdp: &CdpClient,
    action: &BrowserAction,
    timeout: Duration,
) -> (ActionStatus, Option<String>) {
    match action {
        BrowserAction::ClickXy { x, y, .. } => click_xy(cdp, *x, *y).await,
        BrowserAction::ClickSelector { selector } => click_selector(cdp, selector).await,
        BrowserAction::Fill { selector, value } => semantic_input(cdp, selector, value, true).await,
        BrowserAction::TypeText { selector, value } => {
            semantic_input(cdp, selector, value, false).await
        }
        BrowserAction::Press { key } => press(cdp, key).await,
        BrowserAction::Scroll { delta_x, delta_y } => scroll(cdp, *delta_x, *delta_y).await,
        BrowserAction::GetElementValue { selector } => get_element_value(cdp, selector).await,
        BrowserAction::ExecuteJavaScript { expression } => {
            execute_javascript(cdp, expression).await
        }
        BrowserAction::Wait { timeout_ms } => wait(*timeout_ms).await,
        BrowserAction::WaitForSelector {
            selector,
            timeout_ms,
        } => wait_for_selector(cdp, selector, *timeout_ms).await,
        BrowserAction::WaitForText { text, timeout_ms } => {
            wait_for_text(cdp, text, *timeout_ms).await
        }
        BrowserAction::Script { steps } => script(cdp, steps, timeout).await,
        BrowserAction::Navigate { .. } => (
            ActionStatus::Failed,
            Some("navigate must be sent to /goto endpoint".to_string()),
        ),
    }
}

/// Map a `BrowserAction` variant to its string kind name.
fn action_kind(action: &BrowserAction) -> &'static str {
    match action {
        BrowserAction::ClickXy { .. } => "click_xy",
        BrowserAction::ClickSelector { .. } => "click_selector",
        BrowserAction::Fill { .. } => "fill",
        BrowserAction::TypeText { .. } => "type_text",
        BrowserAction::Press { .. } => "press",
        BrowserAction::Scroll { .. } => "scroll",
        BrowserAction::GetElementValue { .. } => "get_element_value",
        BrowserAction::ExecuteJavaScript { .. } => "execute_javascript",
        BrowserAction::Wait { .. } => "wait",
        BrowserAction::WaitForSelector { .. } => "wait_for_selector",
        BrowserAction::WaitForText { .. } => "wait_for_text",
        BrowserAction::Script { .. } => "script",
        BrowserAction::Navigate { .. } => "navigate",
    }
}

// ── CDP helpers ────────────────────────────────────────────────────────

/// Evaluate a JS expression via `Runtime.evaluate` and return the result as a
/// string. If the result is a string starting with `"Error:"`, it is treated
/// as a failure (matching the Python sidecar's error detection convention).
async fn eval_js(cdp: &CdpClient, expression: &str) -> Result<String, String> {
    let params = json!({
        "expression": expression,
        "returnByValue": true,
        "awaitPromise": true,
    });

    let result = cdp
        .send_command("Runtime.evaluate", params, CDP_TIMEOUT)
        .await
        .map_err(|e| format!("Error: CDP command failed: {e}"))?;

    if let Some(exception) = result.get("exceptionDetails") {
        let msg = exception
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(|d| d.as_str())
            .or_else(|| exception.get("text").and_then(|t| t.as_str()))
            .unwrap_or("unknown JS error");
        return Err(format!("Error: {msg}"));
    }

    let value = result
        .get("result")
        .and_then(|r| r.get("value"))
        .unwrap_or(&Value::Null);
    let string_value = value_to_string(value);

    if string_value.starts_with("Error:") {
        Err(string_value)
    } else {
        Ok(string_value)
    }
}

/// Convert a `serde_json::Value` to a display string.
fn value_to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "null".to_string()),
    }
}

/// Dispatch a real CDP mouse click (mousePressed + mouseReleased) at `(x, y)`.
async fn dispatch_mouse_click(cdp: &CdpClient, x: f64, y: f64) -> Result<(), String> {
    let press = json!({"type":"mousePressed","x":x,"y":y,"button":"left","clickCount":1});
    let release = json!({"type":"mouseReleased","x":x,"y":y,"button":"left","clickCount":1});

    cdp.send_command("Input.dispatchMouseEvent", press, CDP_TIMEOUT)
        .await
        .map_err(|e| format!("Error: mousePressed failed: {e}"))?;
    cdp.send_command("Input.dispatchMouseEvent", release, CDP_TIMEOUT)
        .await
        .map_err(|e| format!("Error: mouseReleased failed: {e}"))?;
    Ok(())
}

// ── Action implementations ─────────────────────────────────────────────

/// Click at coordinates via `Input.dispatchMouseEvent`.
async fn click_xy(cdp: &CdpClient, x: u32, y: u32) -> (ActionStatus, Option<String>) {
    match dispatch_mouse_click(cdp, x as f64, y as f64).await {
        Ok(()) => (
            ActionStatus::Executed,
            Some(format!("clicked at ({x},{y})")),
        ),
        Err(e) => (ActionStatus::Failed, Some(e)),
    }
}

/// Click an element by CSS selector — get center coordinates via JS
/// `getBoundingClientRect`, then dispatch a real CDP mouse click.
async fn click_selector(cdp: &CdpClient, selector: &str) -> (ActionStatus, Option<String>) {
    let selector_json = json_str(selector);
    let expr = format!(
        "(() => {{ const el = document.querySelector({selector_json}); \
         if (!el) return 'Error: element not found'; \
         const r = el.getBoundingClientRect(); \
         return JSON.stringify({{ x: r.x + r.width/2, y: r.y + r.height/2 }}); }})()"
    );

    match eval_js(cdp, &expr).await {
        Ok(coords) => {
            let parsed: Value = match serde_json::from_str(&coords) {
                Ok(v) => v,
                Err(_) => {
                    return (
                        ActionStatus::Failed,
                        Some("Error: invalid coordinates from JS".to_string()),
                    );
                }
            };
            let x = parsed.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = parsed.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            match dispatch_mouse_click(cdp, x, y).await {
                Ok(()) => (
                    ActionStatus::Executed,
                    Some(format!("clicked selector '{selector}'")),
                ),
                Err(e) => (ActionStatus::Failed, Some(e)),
            }
        }
        Err(e) => (ActionStatus::Failed, Some(e)),
    }
}

/// Fill or type text using semantic input JS (native setter events for
/// React/Vue/Angular). Ported from the Python sidecar's
/// `_semantic_input_script`.
async fn semantic_input(
    cdp: &CdpClient,
    selector: &str,
    value: &str,
    fill: bool,
) -> (ActionStatus, Option<String>) {
    let script = semantic_input_script(selector, value, fill);
    match eval_js(cdp, &script).await {
        Ok(result) => (ActionStatus::Executed, Some(result)),
        Err(e) => (ActionStatus::Failed, Some(e)),
    }
}

/// Press a key (simple or combo) via JS `KeyboardEvent`.
async fn press(cdp: &CdpClient, key: &str) -> (ActionStatus, Option<String>) {
    let script = press_key_script(key);
    match eval_js(cdp, &script).await {
        Ok(result) => (ActionStatus::Executed, Some(result)),
        Err(e) => (ActionStatus::Failed, Some(e)),
    }
}

/// Scroll the page by `(delta_x, delta_y)` pixels.
async fn scroll(cdp: &CdpClient, dx: i32, dy: i32) -> (ActionStatus, Option<String>) {
    let expr = format!("window.scrollBy({dx},{dy}); true");
    match eval_js(cdp, &expr).await {
        Ok(_) => (
            ActionStatus::Executed,
            Some(format!("scrolled by ({dx},{dy})")),
        ),
        Err(e) => (ActionStatus::Failed, Some(e)),
    }
}

/// Get the value of an element by CSS selector.
async fn get_element_value(cdp: &CdpClient, selector: &str) -> (ActionStatus, Option<String>) {
    let selector_json = json_str(selector);
    let expr = format!(
        "(() => {{ const el = document.querySelector({selector_json}); \
         if (!el) return 'Error: element not found'; \
         const tag = el.tagName.toLowerCase(); \
         const type = (el.getAttribute('type') || '').toLowerCase(); \
         if (tag === 'input' && (type === 'checkbox' || type === 'radio')) return String(el.checked); \
         return el.value !== undefined ? el.value : el.textContent; }})()"
    );
    match eval_js(cdp, &expr).await {
        Ok(result) => (ActionStatus::Executed, Some(result)),
        Err(e) => (ActionStatus::Failed, Some(e)),
    }
}

/// Execute a JavaScript expression with try/catch wrapping.
async fn execute_javascript(cdp: &CdpClient, expression: &str) -> (ActionStatus, Option<String>) {
    let expr = format!(
        "(() => {{ try {{ return ({expression}); }} catch (err) {{ return 'Error: ' + (err.message || err); }} }})()"
    );
    match eval_js(cdp, &expr).await {
        Ok(result) => (ActionStatus::Executed, Some(result)),
        Err(e) => (ActionStatus::Failed, Some(e)),
    }
}

/// Wait for a fixed duration (no CDP command needed).
async fn wait(timeout_ms: u64) -> (ActionStatus, Option<String>) {
    let duration = Duration::from_millis(timeout_ms.max(1));
    tokio::time::sleep(duration).await;
    (ActionStatus::NoOp, Some(format!("waited {timeout_ms}ms")))
}

/// Poll until an element matching `selector` exists in the DOM.
async fn wait_for_selector(
    cdp: &CdpClient,
    selector: &str,
    timeout_ms: u64,
) -> (ActionStatus, Option<String>) {
    let selector_json = json_str(selector);
    let expr = format!("document.querySelector({selector_json}) !== null");
    match poll_condition(cdp, &expr, timeout_ms).await {
        Ok(true) => (
            ActionStatus::Executed,
            Some(format!("selector '{selector}' found")),
        ),
        Ok(false) => (
            ActionStatus::Failed,
            Some(format!(
                "Error: selector '{selector}' not found within {timeout_ms}ms"
            )),
        ),
        Err(e) => (ActionStatus::Failed, Some(e)),
    }
}

/// Poll until `text` appears in `document.body.textContent`.
///
/// Uses `textContent` (not `innerText`) because `innerText` requires a
/// reflow to compute, which may not happen between CDP commands in headless
/// mode. `textContent` reads directly from the DOM and is always current.
async fn wait_for_text(
    cdp: &CdpClient,
    text: &str,
    timeout_ms: u64,
) -> (ActionStatus, Option<String>) {
    let text_json = json_str(text);
    let expr = format!("document.body.textContent.includes({text_json})");
    match poll_condition(cdp, &expr, timeout_ms).await {
        Ok(true) => (ActionStatus::Executed, Some(format!("text '{text}' found"))),
        Ok(false) => (
            ActionStatus::Failed,
            Some(format!(
                "Error: text '{text}' not found within {timeout_ms}ms"
            )),
        ),
        Err(e) => (ActionStatus::Failed, Some(e)),
    }
}

/// Execute a script (sequence of action steps), breaking on first failure.
async fn script(
    cdp: &CdpClient,
    steps: &[BrowserAction],
    timeout: Duration,
) -> (ActionStatus, Option<String>) {
    if steps.is_empty() {
        return (
            ActionStatus::Failed,
            Some("Error: script has no steps".to_string()),
        );
    }

    let mut last_result: Option<String> = None;
    for step in steps {
        // Box::pin breaks the async fn recursion cycle
        // (run_action → script → run_action).
        let (status, result) = Box::pin(run_action(cdp, step, timeout)).await;
        if status == ActionStatus::Failed {
            return (ActionStatus::Failed, result);
        }
        last_result = result;
    }

    (ActionStatus::Executed, last_result)
}

// ── Polling helper ──────────────────────────────────────────────────────

/// Poll a JS boolean expression until it returns `true` or the timeout expires.
///
/// Returns `Ok(true)` if the condition became true, `Ok(false)` if it timed out.
async fn poll_condition(
    cdp: &CdpClient,
    condition_expr: &str,
    timeout_ms: u64,
) -> Result<bool, String> {
    let timeout = Duration::from_millis(timeout_ms.max(100));
    let deadline = Instant::now() + timeout;
    let check_expr = format!("({condition_expr})");

    loop {
        let result = eval_js(cdp, &check_expr).await?;
        if result == "true" {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

// ── JS script builders ──────────────────────────────────────────────────

/// JSON-encode a string for safe embedding in JS.
fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// Build the semantic input JS script (ported from Python sidecar's
/// `_semantic_input_script`). Uses native value setters from
/// HTMLInputElement/TextAreaElement/SelectElement prototypes and dispatches
/// InputEvent with `insertReplacementText` (fill) or `insertText` (type_text)
/// so React/Vue/Angular observe the same event sequence as real user input.
fn semantic_input_script(selector: &str, value: &str, fill: bool) -> String {
    let selector_json = json_str(selector);
    let value_json = json_str(value);
    let action = if fill { "\"fill\"" } else { "\"type_text\"" };

    SEMANTIC_INPUT_TEMPLATE
        .replace("__SELECTOR__", &selector_json)
        .replace("__VALUE__", &value_json)
        .replace("__ACTION__", action)
}

/// Template for semantic input JS — ported from Python sidecar.
const SEMANTIC_INPUT_TEMPLATE: &str = r#"(() => {
  const selector = __SELECTOR__;
  const desired = __VALUE__;
  const action = __ACTION__;
  const el = document.querySelector(__SELECTOR__);
  if (!el) return 'Error: no element found for semantic input';
  const tag = (el.tagName || '').toLowerCase();
  const common = { bubbles: true, cancelable: true, composed: true };
  const dispatch = (type, init = {}) => {
    if (type === 'input' && typeof InputEvent === 'function') {
      el.dispatchEvent(new InputEvent(type, { ...common, data: desired, inputType: action === 'fill' ? 'insertReplacementText' : 'insertText', ...init }));
    } else { el.dispatchEvent(new Event(type, { ...common, ...init })); }
  };
  const setterFrom = proto => proto && Object.getOwnPropertyDescriptor(proto, 'value') && Object.getOwnPropertyDescriptor(proto, 'value').set;
  const setNativeValue = next => {
    const ownSetter = Object.getOwnPropertyDescriptor(el, 'value') && Object.getOwnPropertyDescriptor(el, 'value').set;
    let nativeSetter = null;
    if (el instanceof HTMLInputElement) nativeSetter = setterFrom(HTMLInputElement.prototype);
    else if (el instanceof HTMLTextAreaElement) nativeSetter = setterFrom(HTMLTextAreaElement.prototype);
    else if (el instanceof HTMLSelectElement) nativeSetter = setterFrom(HTMLSelectElement.prototype);
    else nativeSetter = setterFrom(Object.getPrototypeOf(el));
    const setter = nativeSetter && nativeSetter !== ownSetter ? nativeSetter : (ownSetter || nativeSetter);
    if (setter) setter.call(el, next); else el.value = next;
  };
  if (!(tag === 'input' || tag === 'textarea' || tag === 'select' || el.isContentEditable)) {
    return 'Error: element not fillable';
  }
  if (typeof el.focus === 'function') el.focus({ preventScroll: true });
  el.dispatchEvent(new FocusEvent('focus', common));
  el.dispatchEvent(new FocusEvent('focusin', common));
  if (el.isContentEditable) {
    dispatch('beforeinput');
    el.textContent = desired;
    dispatch('input');
  } else {
    dispatch('beforeinput');
    setNativeValue(desired);
    dispatch('input');
  }
  dispatch('change');
  el.dispatchEvent(new KeyboardEvent('keyup', { ...common, key: 'End' }));
  const finalValue = el.isContentEditable ? el.textContent : el.value;
  if (finalValue !== desired) return 'Error: semantic input value mismatch; final length ' + String(finalValue ?? '').length + ', expected length ' + desired.length;
  return { ok: true, action, selector, tag, type: el.getAttribute('type'), value_length: String(finalValue ?? '').length, expected_length: desired.length, value_matches: true };
})()"#;

/// Build the press key JS script — dispatches `keydown`, `keypress`, `keyup`
/// `KeyboardEvent`s on `document.activeElement`. Handles both simple keys
/// (`Enter`, `Tab`) and combos (`ctrl+a`, `shift+Tab`).
fn press_key_script(key: &str) -> String {
    let (resolved_key, modifiers, label) = parse_key(key);

    let key_json = json_str(&resolved_key);
    PRESS_KEY_TEMPLATE
        .replace("__KEY__", &key_json)
        .replace("__CTRL__", bool_str(modifiers.ctrl))
        .replace("__ALT__", bool_str(modifiers.alt))
        .replace("__SHIFT__", bool_str(modifiers.shift))
        .replace("__META__", bool_str(modifiers.meta))
        .replace("__LABEL__", &label)
}

/// Template for press key JS.
const PRESS_KEY_TEMPLATE: &str = r#"(() => {
  const target = document.activeElement || document.body;
  const init = { key: __KEY__, bubbles: true, cancelable: true, ctrlKey: __CTRL__, altKey: __ALT__, shiftKey: __SHIFT__, metaKey: __META__ };
  ['keydown','keypress','keyup'].forEach(t => target.dispatchEvent(new KeyboardEvent(t, init)));
  return 'dispatched __LABEL__';
})()"#;

/// Convert a bool to a JS boolean string.
fn bool_str(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

/// Modifier key state.
#[derive(Default)]
struct ModifierState {
    ctrl: bool,
    alt: bool,
    shift: bool,
    meta: bool,
}

/// Parse a key string (simple key or combo like "ctrl+a") into
/// (resolved_key, modifiers, label).
fn parse_key(key: &str) -> (String, ModifierState, String) {
    if !key.contains('+') {
        let resolved = resolve_key_alias(key);
        return (resolved, ModifierState::default(), key.to_string());
    }

    let parts: Vec<&str> = key.split('+').map(str::trim).collect();
    let mut modifiers = ModifierState::default();
    let mut keys: Vec<String> = Vec::new();

    for part in &parts {
        let lower = part.to_lowercase();
        if let Some(mod_key) = resolve_modifier_alias(&lower) {
            match mod_key {
                ModKey::Ctrl => modifiers.ctrl = true,
                ModKey::Alt => modifiers.alt = true,
                ModKey::Shift => modifiers.shift = true,
                ModKey::Meta => modifiers.meta = true,
            }
        } else if !part.is_empty() {
            keys.push(resolve_key_alias(&lower));
        }
    }

    if keys.is_empty() {
        return (
            "".to_string(),
            modifiers,
            "Error: no key in combo".to_string(),
        );
    }

    (keys[0].clone(), modifiers, key.to_string())
}

/// Modifier key enum.
enum ModKey {
    Ctrl,
    Alt,
    Shift,
    Meta,
}

/// Resolve a modifier alias string to a `ModKey`.
fn resolve_modifier_alias(s: &str) -> Option<ModKey> {
    match s {
        "ctrl" | "control" => Some(ModKey::Ctrl),
        "alt" => Some(ModKey::Alt),
        "shift" => Some(ModKey::Shift),
        "meta" | "command" | "cmd" | "win" => Some(ModKey::Meta),
        _ => None,
    }
}

/// Resolve a key alias to its `KeyboardEvent.key` value.
fn resolve_key_alias(s: &str) -> String {
    match s.to_lowercase().as_str() {
        "enter" | "return" => "Enter",
        "tab" => "Tab",
        "escape" | "esc" => "Escape",
        "space" | "spacebar" => " ",
        "backspace" => "Backspace",
        "delete" | "del" => "Delete",
        "arrowup" => "ArrowUp",
        "arrowdown" => "ArrowDown",
        "arrowleft" => "ArrowLeft",
        "arrowright" => "ArrowRight",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        _ => s,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_kind_all_variants() {
        assert_eq!(
            action_kind(&BrowserAction::ClickXy {
                x: 0,
                y: 0,
                target_description: None,
            }),
            "click_xy"
        );
        assert_eq!(
            action_kind(&BrowserAction::ClickSelector {
                selector: String::new()
            }),
            "click_selector"
        );
        assert_eq!(
            action_kind(&BrowserAction::Fill {
                selector: String::new(),
                value: String::new(),
            }),
            "fill"
        );
        assert_eq!(
            action_kind(&BrowserAction::TypeText {
                selector: String::new(),
                value: String::new(),
            }),
            "type_text"
        );
        assert_eq!(
            action_kind(&BrowserAction::Press { key: String::new() }),
            "press"
        );
        assert_eq!(
            action_kind(&BrowserAction::Scroll {
                delta_x: 0,
                delta_y: 0
            }),
            "scroll"
        );
        assert_eq!(
            action_kind(&BrowserAction::GetElementValue {
                selector: String::new()
            }),
            "get_element_value"
        );
        assert_eq!(
            action_kind(&BrowserAction::ExecuteJavaScript {
                expression: String::new()
            }),
            "execute_javascript"
        );
        assert_eq!(action_kind(&BrowserAction::Wait { timeout_ms: 0 }), "wait");
        assert_eq!(
            action_kind(&BrowserAction::WaitForSelector {
                selector: String::new(),
                timeout_ms: 0,
            }),
            "wait_for_selector"
        );
        assert_eq!(
            action_kind(&BrowserAction::WaitForText {
                text: String::new(),
                timeout_ms: 0,
            }),
            "wait_for_text"
        );
        assert_eq!(
            action_kind(&BrowserAction::Script { steps: vec![] }),
            "script"
        );
        assert_eq!(
            action_kind(&BrowserAction::Navigate {
                url: String::new(),
                force_reload: false,
            }),
            "navigate"
        );
    }

    #[test]
    fn semantic_input_contains_key_elements() {
        let script = semantic_input_script("#my-input", "hello world", true);
        assert!(script.contains("querySelector"));
        assert!(script.contains("hello world"));
        assert!(script.contains("insertReplacementText"));
        assert!(script.contains("setNativeValue"));
        assert!(script.contains("HTMLInputElement"));
    }

    #[test]
    fn semantic_input_type_text_uses_insert_text() {
        let script = semantic_input_script("#input", "abc", false);
        assert!(script.contains("\"type_text\""));
    }

    #[test]
    fn semantic_input_fill_uses_insert_replacement() {
        let script = semantic_input_script("#input", "abc", true);
        assert!(script.contains("\"fill\""));
    }

    #[test]
    fn press_key_simple() {
        let script = press_key_script("Enter");
        assert!(script.contains("\"Enter\""));
        assert!(script.contains("ctrlKey: false"));
        assert!(script.contains("dispatched Enter"));
    }

    #[test]
    fn press_key_combo_ctrl_a() {
        let script = press_key_script("ctrl+a");
        assert!(script.contains("\"a\""));
        assert!(script.contains("ctrlKey: true"));
        assert!(script.contains("altKey: false"));
        assert!(script.contains("dispatched ctrl+a"));
    }

    #[test]
    fn press_key_combo_shift_tab() {
        let script = press_key_script("shift+Tab");
        assert!(script.contains("\"Tab\""));
        assert!(script.contains("shiftKey: true"));
        assert!(script.contains("ctrlKey: false"));
    }

    #[test]
    fn press_key_combo_meta_enter() {
        let script = press_key_script("cmd+Enter");
        assert!(script.contains("\"Enter\""));
        assert!(script.contains("metaKey: true"));
    }

    #[test]
    fn press_key_alias_escape() {
        let script = press_key_script("esc");
        assert!(script.contains("\"Escape\""));
    }

    #[test]
    fn press_key_alias_arrow_up() {
        let script = press_key_script("ArrowUp");
        assert!(script.contains("\"ArrowUp\""));
    }

    #[test]
    fn parse_key_no_combo() {
        let (key, mods, label) = parse_key("Enter");
        assert_eq!(key, "Enter");
        assert!(!mods.ctrl && !mods.alt && !mods.shift && !mods.meta);
        assert_eq!(label, "Enter");
    }

    #[test]
    fn parse_key_ctrl_a() {
        let (key, mods, _) = parse_key("ctrl+a");
        assert_eq!(key, "a");
        assert!(mods.ctrl);
        assert!(!mods.alt && !mods.shift && !mods.meta);
    }

    #[test]
    fn parse_key_multi_modifier() {
        let (key, mods, _) = parse_key("ctrl+shift+a");
        assert_eq!(key, "a");
        assert!(mods.ctrl && mods.shift);
    }

    #[test]
    fn value_to_string_variants() {
        assert_eq!(value_to_string(&Value::Null), "");
        assert_eq!(value_to_string(&Value::Bool(true)), "true");
        assert_eq!(value_to_string(&json!(42)), "42");
        assert_eq!(value_to_string(&json!("hello")), "hello");
        assert_eq!(value_to_string(&json!({"a": 1})), r#"{"a":1}"#);
    }

    #[test]
    fn json_str_escapes_quotes() {
        assert_eq!(json_str("hello"), r#""hello""#);
        assert_eq!(json_str("it's"), r#""it's""#);
        assert_eq!(json_str(r#"say "hi""#), r#""say \"hi\"""#);
    }

    #[test]
    fn resolve_key_alias_variants() {
        assert_eq!(resolve_key_alias("enter"), "Enter");
        assert_eq!(resolve_key_alias("RETURN"), "Enter");
        assert_eq!(resolve_key_alias("esc"), "Escape");
        assert_eq!(resolve_key_alias("space"), " ");
        assert_eq!(resolve_key_alias("backspace"), "Backspace");
        assert_eq!(resolve_key_alias("arrowup"), "ArrowUp");
        assert_eq!(resolve_key_alias("a"), "a");
    }
}
