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
