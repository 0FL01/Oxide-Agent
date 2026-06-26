use leptos::prelude::WriteSignal;

/// Reactive routing context — provided by `App`, consumed by `navigate()`
/// to update the current route without a full page reload.
#[derive(Clone)]
pub struct RouteContext {
    pub set_route: WriteSignal<AppRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppRoute {
    Login,
    Register,
    Bootstrap,
    App,
    Session(String),
    Settings,
    NotFound,
}

impl AppRoute {
    #[must_use]
    pub fn from_path(path: &str) -> Self {
        match path {
            "/" | "/app" => Self::App,
            "/login" => Self::Login,
            "/register" => Self::Register,
            "/bootstrap" => Self::Bootstrap,
            "/settings" => Self::Settings,
            _ => path
                .strip_prefix("/app/session/")
                .filter(|session_id| !session_id.is_empty())
                .map(|session_id| Self::Session(session_id.to_string()))
                .unwrap_or(Self::NotFound),
        }
    }

    #[must_use]
    pub fn current() -> Self {
        Self::from_path(&crate::utils::browser_pathname())
    }
}
