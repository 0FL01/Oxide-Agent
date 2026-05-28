use crate::auth::{use_auth, AuthContext};
use crate::routes::AppRoute;
use crate::sessions::SessionSidebar;
use crate::tasks::TaskConsole;
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    PersistedTaskEvent, ProgressSnapshot, SseConnectionState, TaskStatus,
};
use serde::Deserialize;

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

// ── Token snapshot for context budget display ──────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct TokenSnapshotUi {
    hot_memory_tokens: usize,
    system_prompt_tokens: usize,
    tool_schema_tokens: usize,
    reserved_output_tokens: usize,
    hard_reserve_tokens: usize,
    projected_total_tokens: usize,
    context_window_tokens: usize,
    headroom_tokens: usize,
    budget_state: String,
}

/// Format token count: 9000 → "9k", 172000 → "172k", 8192 → "8.2k"
fn fmt_tokens(n: usize) -> String {
    if n >= 1000 {
        let k = n as f64 / 1000.0;
        if k.fract() < 0.05 {
            format!("{:.0}k", k)
        } else {
            format!("{:.1}k", k)
        }
    } else {
        format!("{}", n)
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

            // Context budget (live from SSE progress)
            {move || {
                progress.get().and_then(|p| {
                    p.latest_token_snapshot.as_ref().and_then(|v| {
                        serde_json::from_value::<TokenSnapshotUi>(v.clone()).ok()
                    })
                }).map(|snap| {
                    let budget_class = match snap.budget_state.as_str() {
                        "Healthy" => "context-budget-ok",
                        "Warning" => "context-budget-warn",
                        _ => "context-budget-over",
                    };
                    view! {
                        <div class="metrics-group">
                            <div class="metrics-group-title">"Context"</div>
                            <div class="context-lines">
                                <span class="context-line">
                                    {format!(
                                        "flow {} | prompt {} | tools {}",
                                        fmt_tokens(snap.hot_memory_tokens),
                                        fmt_tokens(snap.system_prompt_tokens),
                                        fmt_tokens(snap.tool_schema_tokens),
                                    )}
                                </span>
                                <span class="context-line">
                                    {format!(
                                        "{} + {} = {} | {} free",
                                        fmt_tokens(snap.reserved_output_tokens),
                                        fmt_tokens(snap.hard_reserve_tokens),
                                        fmt_tokens(snap.projected_total_tokens),
                                        fmt_tokens(snap.headroom_tokens),
                                    )}
                                </span>
                                <span class=budget_class>
                                    {format!("Budget: {}", snap.budget_state)}
                                </span>
                            </div>
                        </div>
                    }
                })
            }}

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
