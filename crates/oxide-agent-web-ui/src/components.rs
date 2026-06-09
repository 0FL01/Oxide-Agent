use crate::routes::AppRoute;
use crate::sessions::SessionSidebar;
use crate::tasks::TaskConsole;
use leptos::prelude::*;
use oxide_agent_web_contracts::{PersistedTaskEvent, ProgressSnapshot, SessionSummary};

#[component]
pub fn AppLayout(route: AppRoute) -> impl IntoView {
    let (events, set_events) = signal(Vec::<PersistedTaskEvent>::new());
    let (progress, set_progress) = signal(None::<ProgressSnapshot>);
    let (sessions, set_sessions) = signal(Vec::<SessionSummary>::new());

    view! {
        <div class="app-layout">
            <SessionSidebar selected=selected_session_id(&route) sessions=sessions set_sessions=set_sessions />
            <main class="workspace-main">
                <TaskConsole
                    route=route
                    events=events
                    progress=progress
                    set_events=set_events
                    set_progress=set_progress
                    set_sessions=set_sessions
                />
            </main>
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
