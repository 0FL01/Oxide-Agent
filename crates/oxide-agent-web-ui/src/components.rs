use crate::routes::AppRoute;
use crate::sessions::SessionSidebar;
use crate::tasks::TaskConsole;
use leptos::prelude::*;
use oxide_agent_web_contracts::{PersistedTaskEvent, ProgressSnapshot, SessionSummary};

#[component]
pub fn AppLayout(route: ReadSignal<AppRoute>) -> impl IntoView {
    let (events, set_events) = signal(Vec::<PersistedTaskEvent>::new());
    let (progress, set_progress) = signal(None::<ProgressSnapshot>);
    let (sessions, set_sessions) = signal(Vec::<SessionSummary>::new());

    // Derive `session_id` reactively from the route signal.
    // `App → Session(id)` changes this from `None` to `Some(id)` without
    // recreating `AppLayout` or any of its children.
    let session_id = Memo::new(move |_| match route.get() {
        AppRoute::Session(id) => Some(id),
        _ => None,
    });

    view! {
        <div class="app-layout">
            <SessionSidebar
                selected=session_id
                sessions=sessions
                set_sessions=set_sessions
            />
            <main class="workspace-main">
                <TaskConsole
                    session_id=session_id
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
