//! Static file serving and SPA fallback for the web console frontend.

use super::AppState;
use axum::{
    extract::State,
    http::{header::{CACHE_CONTROL, CONTENT_TYPE}, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use std::path::{Component, Path as FsPath, PathBuf};

/// Axum fallback handler: serves static assets or SPA index.html for browser routes.
pub(crate) async fn static_assets_handler(
    State(state): State<AppState>,
    uri: Uri,
) -> Response {
    let path = uri.path();
    if path.starts_with("/api/") {
        return StatusCode::NOT_FOUND.into_response();
    }

    let Some(assets_dir) = state.web_assets.dir.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match static_asset_path(assets_dir, path) {
        Some(asset_path) if asset_path.is_file() => serve_static_file(asset_path).await,
        Some(_) if static_path_is_browser_route(path) => {
            serve_static_file(assets_dir.join("index.html")).await
        }
        Some(_) => StatusCode::NOT_FOUND.into_response(),
        None => StatusCode::BAD_REQUEST.into_response(),
    }
}

async fn serve_static_file(path: PathBuf) -> Response {
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let content_type = static_content_type(&path);
    let cache_control = if path.file_name().and_then(|name| name.to_str()) == Some("index.html") {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };

    (
        [
            (CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (CACHE_CONTROL, HeaderValue::from_static(cache_control)),
        ],
        bytes,
    )
        .into_response()
}

fn static_asset_path(assets_dir: &FsPath, uri_path: &str) -> Option<PathBuf> {
    let relative_path = uri_path.trim_start_matches('/');
    if relative_path.is_empty() {
        return Some(assets_dir.join("index.html"));
    }
    let mut path = PathBuf::new();
    for component in FsPath::new(relative_path).components() {
        match component {
            Component::Normal(part) => path.push(part),
            _ => return None,
        }
    }
    Some(assets_dir.join(path))
}

fn static_path_is_browser_route(path: &str) -> bool {
    path == "/"
        || path == "/app"
        || path.starts_with("/app/")
        || path == "/login"
        || path == "/register"
        || path == "/bootstrap"
        || path == "/settings"
}

fn static_content_type(path: &FsPath) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}
