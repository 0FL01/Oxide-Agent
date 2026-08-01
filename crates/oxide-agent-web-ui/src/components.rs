use crate::life::LifeConsole;
use crate::routes::AppRoute;
use crate::sessions::SessionSidebar;
use crate::tasks::TaskConsole;
use leptos::prelude::*;
use oxide_agent_web_contracts::{PersistedTaskEvent, SessionSummary};

#[component]
pub fn AppLayout(route: ReadSignal<AppRoute>) -> impl IntoView {
    let (events, set_events) = signal(Vec::<PersistedTaskEvent>::new());
    let (sessions, set_sessions) = signal(Vec::<SessionSummary>::new());

    // Derive `session_id` reactively from the route signal.
    // `App → Session(id)` changes this from `None` to `Some(id)` without
    // recreating `AppLayout` or any of its children.
    let session_id = Memo::new(move |_| match route.get() {
        AppRoute::Session(id) => Some(id),
        _ => None,
    });

    // `Life` route renders the permanent chat console instead of `TaskConsole`.
    let is_life = Memo::new(move |_| matches!(route.get(), AppRoute::Life));

    view! {
        <div class="app-layout">
            <SessionSidebar
                selected=session_id
                is_life=is_life
                sessions=sessions
                set_sessions=set_sessions
            />
            <main class="workspace-main">
                {move || {
                    if is_life.get() {
                        view! { <LifeConsole /> }.into_any()
                    } else {
                        view! {
                            <TaskConsole
                                session_id=session_id
                                events=events
                                set_events=set_events
                                set_sessions=set_sessions
                            />
                        }.into_any()
                    }
                }}
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
