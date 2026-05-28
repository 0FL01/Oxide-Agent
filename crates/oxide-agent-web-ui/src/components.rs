use crate::auth::{use_auth, AuthContext};
use crate::routes::AppRoute;
use crate::sessions::SessionSidebar;
use crate::tasks::TaskConsole;
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    PersistedTaskEvent, ProgressSnapshot, SseConnectionState, TaskStatus,
};

#[component]
pub fn AppLayout(route: AppRoute) -> impl IntoView {
    let auth = use_auth();

    let (events, set_events) = signal(Vec::<PersistedTaskEvent>::new());
    let (_sse_state, set_sse_state) = signal(SseConnectionState::Disconnected);
    let (progress, set_progress) = signal(None::<ProgressSnapshot>);

    view! {
        <div class="app-layout">
            <SessionSidebar selected=selected_session_id(&route) />
            <main class="workspace-main">
                <Header auth=auth />
                <TaskConsole
                    route=route
                    events=events
                    progress=progress
                    set_events=set_events
                    set_sse_state=set_sse_state
                    set_progress=set_progress
                />
            </main>
        </div>
    }
}

#[component]
fn Header(auth: AuthContext) -> impl IntoView {
    view! {
        <header class="topbar">
            <div class="topbar-left">
                <a class="brand" href="/app">"Oxide Agent"</a>
            </div>
            <nav class="topnav">
                <a href="/settings">"Settings"</a>
                {move || {
                    auth.auth.get().user.map(|user| {
                        view! { <span class="user-pill">{user.login}</span> }.into_any()
                    }).unwrap_or_else(|| view! { <a href="/login">"Sign in"</a> }.into_any())
                }}
            </nav>
        </header>
    }
}

#[component]
pub fn StatusBadge(status: TaskStatus) -> impl IntoView {
    let (label, css_class) = match status {
        TaskStatus::Queued => ("queued", "status-badge idle"),
        TaskStatus::Running => ("running", "status-badge running"),
        TaskStatus::WaitingForUserInput => ("waiting", "status-badge disconnected"),
        TaskStatus::Completed => ("completed", "status-badge completed"),
        TaskStatus::Failed => ("failed", "status-badge failed"),
        TaskStatus::Cancelled => ("cancelled", "status-badge failed"),
        TaskStatus::Interrupted => ("interrupted", "status-badge failed"),
    };
    view! {
        <span class=css_class>
            <span class="dot"></span>
            {label}
        </span>
    }
}

#[component]
pub fn EmptyState(title: &'static str) -> impl IntoView {
    view! {
        <div class="empty-state">
            <div class="empty-state-title">{title}</div>
        </div>
    }
}

#[component]
pub fn ErrorBanner(message: ReadSignal<Option<String>>) -> impl IntoView {
    view! {
        {move || {
            message.get().map(|text| view! { <div class="error-banner">{text}</div> })
        }}
    }
}

fn selected_session_id(route: &AppRoute) -> Option<String> {
    match route {
        AppRoute::Session(session_id) => Some(session_id.clone()),
        _ => None,
    }
}
