use crate::life::state::life_event_to_persisted;
use crate::tasks::activity::{
    ActivityItemCard, group_activity_events, is_chat_visible_event, is_useful_event,
};
use crate::tasks::payload::is_sub_agent_event;
use crate::tasks::state::latest_pinned_todos;
use leptos::prelude::*;
use oxide_agent_web_contracts::{ApiLifeEventResponse, ApiLifeRunSummary, PersistedTaskEvent};

/// Life activity drawer — renders life_events for the active run.
///
/// Reuses the shared event rendering (tool cards, grouping, filtering)
/// from `tasks/activity.rs` by converting `ApiLifeEventResponse` to
/// `PersistedTaskEvent` at the boundary.
#[component]
pub(crate) fn LifeActivityDrawer(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    events: ReadSignal<Vec<ApiLifeEventResponse>>,
    active_run: ReadSignal<Option<ApiLifeRunSummary>>,
    has_older: Signal<bool>,
    loading_older: Signal<bool>,
    load_older: Callback<leptos::ev::MouseEvent>,
) -> impl IntoView {
    let (show_sub_agents, set_show_sub_agents) = signal(true);

    view! {
        <aside class=move || if open.get() { "activity-drawer open" } else { "activity-drawer" }>
            <header class="activity-header">
                <div class="activity-title-row">
                    <span class="activity-title">"Activity"</span>
                    {move || {
                        active_run.get().map(|run| view! {
                            <span class="activity-title-separator">"·"</span>
                            <span class="activity-elapsed">{format!("Run {}", &run.run_id[..8])}</span>
                        })
                    }}
                </div>
                <div class="activity-actions">
                    <button
                        class=move || if show_sub_agents.get() { "activity-filter active" } else { "activity-filter" }
                        type="button"
                        on:click=move |_| set_show_sub_agents.update(|value| *value = !*value)
                    >
                        {move || if show_sub_agents.get() { "Sub-agents" } else { "Root only" }}
                    </button>
                    <button class="activity-close" type="button" on:click=move |_| {
                        set_open.set(false);
                    }>"×"</button>
                </div>
            </header>
            <div class="activity-timeline">
                {move || {
                    has_older.get().then(|| view! {
                        <div class="activity-load-older">
                            <button
                                type="button"
                                class="secondary"
                                disabled=loading_older
                                on:click=move |ev| load_older.run(ev)
                            >
                                {move || if loading_older.get() { "Loading older activity..." } else { "Load older activity" }}
                            </button>
                        </div>
                    })
                }}
                {move || {
                    let all_events = events.get();
                    let run_id = active_run.get().map(|r| r.run_id);

                    // Filter to active run events and convert to PersistedTaskEvent.
                    let persisted: Vec<PersistedTaskEvent> = all_events
                        .iter()
                        .filter(|e| run_id.as_deref().is_none_or(|rid| e.run_id == *rid))
                        .filter_map(life_event_to_persisted)
                        .filter(|e| include_sub_agent(e, show_sub_agents.get()))
                        .filter(|e| is_chat_visible_event(&e.kind))
                        .filter(is_useful_event)
                        .collect();

                    if persisted.is_empty() {
                        return view! { <div class="activity-empty">"No activity yet."</div> }.into_any();
                    }

                    let todos = latest_pinned_todos(&persisted);
                    let items = group_activity_events(persisted, false);

                    if items.is_empty() && todos.is_none() {
                        return view! { <div class="activity-empty">"No activity yet."</div> }.into_any();
                    }

                    view! {
                        {todos.map(|value| view! { <LifeTodosCard todos=value /> })}
                        {items.into_iter().map(|item| view! { <ActivityItemCard item=item /> }).collect::<Vec<_>>()}
                    }.into_any()
                }}
            </div>
        </aside>
    }
}

fn include_sub_agent(event: &PersistedTaskEvent, show: bool) -> bool {
    show || !is_sub_agent_event(event)
}

/// Minimal todos card for life activity. Reuses the same JSON shape as
/// the task todos card but renders independently to avoid coupling to
/// task-specific types.
#[component]
fn LifeTodosCard(todos: serde_json::Value) -> impl IntoView {
    let items = todos
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if items.is_empty() {
        return ().into_any();
    }

    view! {
        <div class="todos-card">
            <div class="todos-card-title">"Todos"</div>
            <ul class="todos-list">
                {items
                    .into_iter()
                    .map(|item| {
                        let desc = item
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let status = item
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("pending")
                            .to_string();
                        let checkbox = if status == "completed" { "✓" } else { "○" };
                        view! {
                            <li class=move || match status.as_str() {
                                "completed" => "todo-item completed",
                                "in_progress" => "todo-item in-progress",
                                _ => "todo-item",
                            }>
                                <span class="todo-checkbox">{checkbox}</span>
                                <span class="todo-text">{desc.clone()}</span>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        </div>
    }
    .into_any()
}
