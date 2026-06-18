#[cfg(target_arch = "wasm32")]
mod api;
#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod auth;
#[cfg(target_arch = "wasm32")]
mod components;
#[cfg(any(target_arch = "wasm32", test))]
mod markdown;
#[cfg(target_arch = "wasm32")]
mod routes;
#[cfg(target_arch = "wasm32")]
mod sessions;
#[cfg(target_arch = "wasm32")]
mod sse;
#[cfg(target_arch = "wasm32")]
mod tasks;
#[cfg(target_arch = "wasm32")]
mod utils;

#[cfg(test)]
#[allow(dead_code)]
#[path = "tasks/state.rs"]
mod task_state;

fn main() {
    #[cfg(target_arch = "wasm32")]
    {
        use leptos::prelude::*;

        console_error_panic_hook::set_once();
        mount_to_body(app::App);
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        println!("oxide-agent-web-ui is a Leptos CSR frontend; build it for wasm32 with Trunk.");
    }
}

#[cfg(test)]
mod css_contract_tests {
    const ACTIVITY_CSS: &str = include_str!("styles/06-activity.css");
    const TOOL_CARDS_RS: &str = include_str!("tasks/tool_cards.rs");

    #[test]
    fn browser_screenshot_thumbnail_uses_fixed_ratio_viewport() {
        assert!(ACTIVITY_CSS.contains(".browser-tool-shot-link"));
        assert!(ACTIVITY_CSS.contains("height: 0;"));
        assert!(ACTIVITY_CSS.contains("padding-top: 56.25%;"));
        assert!(ACTIVITY_CSS.contains(".browser-tool-shot-image"));
        assert!(ACTIVITY_CSS.contains("position: absolute;"));
        assert!(ACTIVITY_CSS.contains("object-fit: contain;"));
    }

    #[test]
    fn browser_screenshot_thumbnail_contract_is_inline_on_dom_nodes() {
        assert!(TOOL_CARDS_RS.contains("BROWSER_TOOL_SHOT_LINK_STYLE"));
        assert!(TOOL_CARDS_RS.contains("height:0;"));
        assert!(TOOL_CARDS_RS.contains("padding-top:56.25%;"));
        assert!(TOOL_CARDS_RS.contains("BROWSER_TOOL_SHOT_IMAGE_STYLE"));
        assert!(TOOL_CARDS_RS.contains("position:absolute;"));
        assert!(TOOL_CARDS_RS.contains("object-fit:contain;"));
    }
}
