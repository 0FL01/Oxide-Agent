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
    let value = value.to_string().replace('T', " ");
    if value.len() >= 16
        && value
            .as_bytes()
            .get(11..13)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
        && value.as_bytes().get(13) == Some(&b':')
        && value
            .as_bytes()
            .get(14..16)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
    {
        return value[11..16].to_string();
    }
    value
}

#[cfg(test)]
mod tests {
    use super::friendly_time;

    #[test]
    fn friendly_time_compacts_chrono_timestamp() {
        assert_eq!(friendly_time("2026-05-29 20:53:47.208618014 UTC"), "20:53");
        assert_eq!(friendly_time("2026-05-29T20:53:47Z"), "20:53");
    }
}
