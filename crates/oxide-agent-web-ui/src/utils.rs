use std::future::Future;

use crate::routes::{AppRoute, RouteContext};
use leptos::prelude::{Set, use_context};

pub fn spawn_ui<F>(future: F)
where
    F: Future<Output = ()> + 'static,
{
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(future);

    #[cfg(not(target_arch = "wasm32"))]
    std::mem::drop(future);
}

#[must_use]
pub fn browser_pathname() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|window| window.location().pathname().ok())
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| "/".to_string())
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        "/".to_string()
    }
}

/// Navigate to an in-app path without a full page reload.
///
/// Uses `history.pushState` to update the URL bar, then sets the reactive
/// route signal via `RouteContext` so Leptos components update in place.
///
/// Falls back to `location.set_href` (full reload) only when no
/// `RouteContext` is available — e.g. during initial bootstrap before
/// the `App` component has mounted.
pub fn navigate(path: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(ctx) = use_context::<RouteContext>() {
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(window) = web_sys::window()
                    && let Ok(history) = window.history()
                {
                    let _ =
                        history.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(path));
                }
            }
            ctx.set_route.set(AppRoute::from_path(path));
            return;
        }
        // No RouteContext — fall back to full navigation
        if let Some(window) = web_sys::window() {
            let _ = window.location().set_href(path);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    let _ = path;
}

/// Intercept clicks on in-app `<a href="/...">` links and route them through
/// `navigate()` instead of a full page reload.
///
/// Skips: external URLs, `target="_blank"`, `download` attribute, and
/// modifier-key clicks (ctrl/cmd/shift/alt — user wants a new tab).
pub fn intercept_in_app_click(ev: &leptos::ev::MouseEvent) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;

        // Respect modifier keys — user wants a new tab
        if ev.ctrl_key() || ev.meta_key() || ev.shift_key() || ev.alt_key() {
            return;
        }

        let Some(element) = ev
            .target()
            .and_then(|t: web_sys::EventTarget| t.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let anchor: web_sys::Element = match element.closest("a") {
            Ok(Some(el)) => el,
            _ => return,
        };
        let anchor = anchor.unchecked_into::<web_sys::HtmlAnchorElement>();

        let href = anchor.get_attribute("href").unwrap_or_default();

        // Only intercept in-app paths (start with "/")
        if href.is_empty() || !href.starts_with('/') {
            return;
        }

        // Skip downloads and explicit new-tab targets
        if anchor.has_attribute("download")
            || anchor.get_attribute("target").as_deref() == Some("_blank")
        {
            return;
        }

        ev.prevent_default();
        navigate(&href);
    }

    #[cfg(not(target_arch = "wasm32"))]
    let _ = ev;
}
