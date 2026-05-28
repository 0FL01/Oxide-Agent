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

    // Shared signals — lifted from SessionWorkspace so MetricsPanel (right column) can read them
    let (events, set_events) = signal(Vec::<PersistedTaskEvent>::new());
    let (sse_state, set_sse_state) = signal(SseConnectionState::Disconnected);
    let (progress, set_progress) = signal(None::<ProgressSnapshot>);

    view! {
        <div class="app-layout">
            <SessionSidebar selected=selected_session_id(&route) />
            <main class="workspace-main">
                <Header auth=auth />
                <TaskConsole
                    route=route
                    set_events=set_events
                    set_sse_state=set_sse_state
                    set_progress=set_progress
                />
            </main>
            <aside class="events-panel">
                <MetricsPanel
                    events=events
                    sse_state=sse_state
                    progress=progress
                />
            </aside>
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

// ── Metrics Panel (right column) ────────────────────────────────────────────

#[component]
fn MetricsPanel(
    events: ReadSignal<Vec<PersistedTaskEvent>>,
    sse_state: ReadSignal<SseConnectionState>,
    progress: ReadSignal<Option<ProgressSnapshot>>,
) -> impl IntoView {
    view! {
        <div class="panel-header">
            <h2>"Metrics"</h2>
        </div>
        <div class="metrics-content">
            // Stream status
            <div class="sse-state">
                <span class=move || match sse_state.get() {
                    SseConnectionState::Connected => "sse-dot",
                    _ => "sse-dot disconnected",
                }></span>
                {move || match sse_state.get() {
                    SseConnectionState::Connected => "Stream connected",
                    SseConnectionState::Disconnected => "Stream disconnected",
                    SseConnectionState::Reconnecting => "Stream reconnecting",
                    SseConnectionState::TerminalClosed => "Stream closed",
                }}
            </div>

            // Progress
            {move || progress.get().map(|p| view! {
                <div class="progress-panel">
                    <div>
                        <strong>"Progress"</strong>
                        <span>{format!("{} / {}", p.current_iteration, p.max_iterations)}</span>
                    </div>
                    {p.current_thought.map(|t| view! { <p>{t}</p> })}
                    {p.provider_failover_notice.map(|n| view! { <p class="notice">{n}</p> })}
                    {p.error.map(|e| view! { <p class="error-text">{e}</p> })}
                </div>
            })}

            // Event stream summary
            <div class="metrics-group">
                <div class="metrics-group-title">"Event Stream"</div>
                <div class="metrics-row">
                    <span class="metrics-label">"Events"</span>
                    <span class="metrics-value">
                        {move || format!("{}", events.get().len())}
                    </span>
                </div>
                <div class="metrics-row">
                    <span class="metrics-label">"Stream"</span>
                    <span class="metrics-value">
                        {move || match sse_state.get() {
                            SseConnectionState::Connected => "Connected",
                            SseConnectionState::Disconnected => "Disconnected",
                            SseConnectionState::Reconnecting => "Reconnecting",
                            SseConnectionState::TerminalClosed => "Closed",
                        }}
                    </span>
                </div>
            </div>

            // Recent events
            {move || {
                if !events.get().is_empty() {
                    let evts = events.get();
                    let items: Vec<_> = evts.into_iter().rev().take(30).collect();
                    view! {
                        <div class="metrics-group">
                            <div class="metrics-group-title">"Recent Events"</div>
                            <ol class="event-list">
                                {items.into_iter().map(|event| {
                                    view! { <EventRow event=event /> }
                                }).collect::<Vec<_>>()}
                            </ol>
                        </div>
                    }
                    .into_any()
                } else {
                    ().into_any()
                }
            }}
        </div>
    }
}

#[component]
fn EventRow(event: PersistedTaskEvent) -> impl IntoView {
    view! {
        <li class="event-row">
            <span class="event-seq">{event.seq}</span>
            <span class="event-kind">{format!("{:?}", event.kind)}</span>
            <span class="event-summary">{event.summary}</span>
        </li>
    }
}
