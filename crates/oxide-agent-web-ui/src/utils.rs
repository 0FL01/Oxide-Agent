use std::future::Future;

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

pub fn navigate(path: &str) {
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_href(path);
    }

    #[cfg(not(target_arch = "wasm32"))]
    let _ = path;
}

#[must_use]
pub fn friendly_time(value: impl ToString) -> String {
    value.to_string().replace('T', " ")
}
