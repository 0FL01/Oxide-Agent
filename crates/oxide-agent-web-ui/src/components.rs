use crate::auth::{use_auth, AuthContext};
use crate::routes::AppRoute;
use crate::sessions::SessionSidebar;
use crate::tasks::TaskConsole;
use leptos::prelude::*;
use oxide_agent_web_contracts::TaskStatus;

#[component]
pub fn AppLayout(route: AppRoute) -> impl IntoView {
    let auth = use_auth();

    view! {
        <div class="app-layout">
            <Header auth=auth />
            <div class="workspace">
                <SessionSidebar selected=selected_session_id(&route) />
                <main class="workspace-main">
                    <TaskConsole route=route />
                </main>
            </div>
        </div>
    }
}

#[component]
fn Header(auth: AuthContext) -> impl IntoView {
    view! {
        <header class="topbar">
            <a class="brand" href="/app">"Oxide Agent"</a>
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
    let label = match status {
        TaskStatus::Queued => "queued",
        TaskStatus::Running => "running",
        TaskStatus::WaitingForUserInput => "waiting",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::Interrupted => "interrupted",
    };
    let class_name = format!("status-badge status-{label}");
    view! { <span class=class_name>{label}</span> }
}

#[component]
pub fn EmptyState(title: &'static str) -> impl IntoView {
    view! { <div class="empty-state">{title}</div> }
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
