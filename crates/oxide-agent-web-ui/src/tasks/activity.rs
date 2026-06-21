use leptos::prelude::*;
use oxide_agent_web_contracts::{
    PersistedTaskEvent, ProgressSnapshot, TaskDetail, TaskEventKind, TaskStatus, TaskSummary,
};
use serde_json::{Value, json};

use super::delivered_files::{DeliveredFileEventBody, delivered_file_link};
use super::payload::{is_sub_agent_event, payload_str_event, sub_agent_event_name};
use super::state::{
    ActivityTiming, activity_elapsed_seconds, format_duration, latest_pinned_todos,
    should_render_global_activity_chip,
};
use super::tool_cards::{
    ToolCard, ToolDetailsWithClass, parse_todo_items_from_value, render_todo_list,
    tool_card_header_with_icon_class, tool_meta, tool_meta_danger, tool_pre_stream,
    tool_preview_with_class,
};

#[component]
pub(super) fn ActivityStatusChip(
    tasks: ReadSignal<Vec<TaskSummary>>,
    active_task: ReadSignal<Option<TaskDetail>>,
    visible_task_ids: Signal<Vec<String>>,
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    activity_task_id: ReadSignal<Option<String>>,
    set_activity_task_id: WriteSignal<Option<String>>,
) -> impl IntoView {
    view! {
        {move || {
            let Some(status) = latest_activity_status(active_task, tasks) else {
                return ().into_any();
            };
            let task_id = latest_activity_task_id(active_task, tasks);
            if !should_render_global_activity_chip(task_id.as_deref(), &visible_task_ids.get()) {
                return ().into_any();
            }
            if status == TaskStatus::Completed {
                return ().into_any();
            }
            let class = match status {
                TaskStatus::Queued | TaskStatus::Running => "status-chip active",
                TaskStatus::WaitingForUserInput => "status-chip active waiting",
                TaskStatus::Failed | TaskStatus::Interrupted => "status-chip warning",
                TaskStatus::Cancelled => "status-chip error",
                TaskStatus::Completed => "status-chip",
            };
            let label = match status {
                TaskStatus::Queued | TaskStatus::Running => "Thinking",
                TaskStatus::WaitingForUserInput => "Waiting for your input",
                TaskStatus::Failed => "Task failed",
                TaskStatus::Cancelled => "Cancelled",
                TaskStatus::Interrupted => "Interrupted",
                TaskStatus::Completed => "Completed",
            };
            view! {
                <div class="status-wrap">
                    <button class=move || if open.get() { format!("{class} open") } else { class.to_string() } type="button" on:click=move |_| toggle_drawer_for_task(open, set_open, activity_task_id, set_activity_task_id, task_id.clone())>
                        <span class="dot"></span>
                        <span>{label}</span>
                        <span class="chevron">"›"</span>
                    </button>
                </div>
            }.into_any()
        }}
    }
}

#[component]
pub(super) fn ThinkingButton(
    label: Memo<String>,
    open: bool,
    on_click: Callback<leptos::ev::MouseEvent>,
) -> impl IntoView {
    view! {
        <button class=if open { "thinking-button open" } else { "thinking-button" } type="button" on:click=move |ev| on_click.run(ev)>
            <span class="dot"></span>
            <span>{move || label.get()}</span>
            <span class="chevron">"›"</span>
        </button>
    }
}

fn toggle_drawer_for_task(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    activity_task_id: ReadSignal<Option<String>>,
    set_activity_task_id: WriteSignal<Option<String>>,
    task_id: Option<String>,
) {
    if open.get() && activity_task_id.get() == task_id {
        set_open.set(false);
        set_activity_task_id.set(None);
    } else {
        set_activity_task_id.set(task_id);
        set_open.set(true);
    }
}

#[component]
pub(super) fn ActivityDrawer(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    activity_task_id: ReadSignal<Option<String>>,
    set_activity_task_id: WriteSignal<Option<String>>,
    tasks: ReadSignal<Vec<TaskSummary>>,
    active_task: ReadSignal<Option<TaskDetail>>,
    events: ReadSignal<Vec<PersistedTaskEvent>>,
    progress: ReadSignal<Option<ProgressSnapshot>>,
    has_older_events: Signal<bool>,
    loading_older_events: Signal<bool>,
    load_older_events: Callback<leptos::ev::MouseEvent>,
    now_millis: ReadSignal<i64>,
) -> impl IntoView {
    let (show_sub_agent_events, set_show_sub_agent_events) = signal(true);

    view! {
        <aside class=move || if open.get() && activity_task_id.get().is_some() { "activity-drawer open" } else { "activity-drawer" }>
            <header class="activity-header">
                <div class="activity-title-row">
                    <span class="activity-title">"Activity"</span>
                    {move || activity_elapsed_label(activity_task_id, active_task, tasks, now_millis).map(|elapsed| view! {
                        <span class="activity-title-separator">"·"</span>
                        <span class="activity-elapsed">{elapsed}</span>
                    })}
                </div>
                <div class="activity-actions">
                    <button
                        class=move || if show_sub_agent_events.get() { "activity-filter active" } else { "activity-filter" }
                        type="button"
                        on:click=move |_| set_show_sub_agent_events.update(|value| *value = !*value)
                    >
                        {move || if show_sub_agent_events.get() { "Sub-agents" } else { "Root only" }}
                    </button>
                    <button class="activity-close" type="button" on:click=move |_| {
                        set_open.set(false);
                        set_activity_task_id.set(None);
                    }>"×"</button>
                </div>
            </header>
            <ContextCard progress=progress />
            <div class="activity-timeline">
                {move || has_older_events.get().then(|| view! {
                    <div class="activity-load-older">
                        <button
                            type="button"
                            class="secondary"
                            disabled=loading_older_events
                            on:click=move |ev| load_older_events.run(ev)
                        >
                            {move || if loading_older_events.get() { "Loading older activity..." } else { "Load older activity" }}
                        </button>
                    </div>
                })}
                {move || {
                    let Some(task_id) = activity_task_id.get() else {
                        return view! { <div class="activity-empty">"No activity yet."</div> }.into_any();
                    };
                    let include_sub_agents = show_sub_agent_events.get();
                    let task_events: Vec<PersistedTaskEvent> = events
                        .get()
                        .into_iter()
                        .filter(|event| event.task_id == task_id)
                        .filter(|event| include_sub_agents || !is_sub_agent_event(event))
                        .filter(|event| is_chat_visible_event(&event.kind))
                        .filter(is_useful_event)
                        .collect();
                    let task_is_terminal = activity_task_status(&task_id, active_task, tasks)
                        .is_some_and(|status| status.is_terminal());
                    let todos = latest_pinned_todos(&task_events);
                    let items = group_activity_events(task_events, task_is_terminal);
                    if items.is_empty() && todos.is_none() {
                        return view! { <div class="activity-empty">"No activity yet."</div> }.into_any();
                    }
                    view! {
                        {todos.map(|value| view! { <TodosCard todos=value /> })}
                        {items.into_iter().map(|item| view! { <ActivityItemCard item=item /> }).collect::<Vec<_>>()}
                    }.into_any()
                }}
            </div>
        </aside>
    }
}

fn compact_tokens(tokens: u64) -> String {
    if tokens < 1000 {
        return tokens.to_string();
    }
    let value = tokens as f64 / 1000.0;
    format!("{value:.1}k")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn latest_activity_task_id(
    active_task: ReadSignal<Option<TaskDetail>>,
    tasks: ReadSignal<Vec<TaskSummary>>,
) -> Option<String> {
    active_task.get().map(|task| task.task_id).or_else(|| {
        tasks
            .get()
            .into_iter()
            .max_by_key(|task| task.updated_at)
            .map(|task| task.task_id)
    })
}

fn latest_activity_status(
    active_task: ReadSignal<Option<TaskDetail>>,
    tasks: ReadSignal<Vec<TaskSummary>>,
) -> Option<TaskStatus> {
    active_task.get().map(|task| task.status).or_else(|| {
        tasks
            .get()
            .into_iter()
            .max_by_key(|task| task.updated_at)
            .map(|task| task.status)
    })
}

fn activity_task_status(
    task_id: &str,
    active_task: ReadSignal<Option<TaskDetail>>,
    tasks: ReadSignal<Vec<TaskSummary>>,
) -> Option<TaskStatus> {
    active_task
        .get()
        .filter(|task| task.task_id == task_id)
        .map(|task| task.status)
        .or_else(|| {
            tasks
                .get()
                .into_iter()
                .find(|task| task.task_id == task_id)
                .map(|task| task.status)
        })
}

fn activity_elapsed_label(
    activity_task_id: ReadSignal<Option<String>>,
    active_task: ReadSignal<Option<TaskDetail>>,
    tasks: ReadSignal<Vec<TaskSummary>>,
    now_millis: ReadSignal<i64>,
) -> Option<String> {
    let task_id = activity_task_id.get()?;
    let timing = active_task
        .get()
        .filter(|task| task.task_id == task_id)
        .map(|task| ActivityTiming::from(&task))
        .or_else(|| {
            tasks
                .get()
                .into_iter()
                .find(|task| task.task_id == task_id)
                .map(|task| ActivityTiming::from(&task))
        })?;
    Some(format_duration(activity_elapsed_seconds(
        timing,
        now_millis.get(),
    )))
}

enum ActivityItem {
    Tool {
        call: Option<PersistedTaskEvent>,
        result: Option<PersistedTaskEvent>,
    },
    Event(PersistedTaskEvent),
}

fn group_activity_events(
    events: Vec<PersistedTaskEvent>,
    task_is_terminal: bool,
) -> Vec<ActivityItem> {
    let mut items: Vec<ActivityItem> = Vec::new();

    for event in events {
        match event.kind {
            TaskEventKind::ToolCall => {
                items.push(ActivityItem::Tool {
                    call: Some(event),
                    result: None,
                });
            }
            TaskEventKind::ToolResult => {
                let id = tool_pairing_key(&event);
                let name = payload_str_event(&event, "name");
                let mut attached = false;

                // Walk backwards to find the matching ToolCall without a result yet.
                // Prefer stable invocation id; fall back to legacy name matching for
                // older persisted events that do not carry an id.
                for item in items.iter_mut().rev() {
                    let ActivityItem::Tool { call, result } = item else {
                        continue;
                    };
                    if result.is_some() {
                        continue;
                    }
                    let call_id = call.as_ref().and_then(tool_pairing_key);
                    let call_name = call.as_ref().and_then(|c| payload_str_event(c, "name"));
                    let stable_id_matches = id.is_some() && id == call_id;
                    let legacy_name_matches =
                        id.is_none() && call_id.is_none() && call_name == name;
                    if stable_id_matches || legacy_name_matches {
                        *result = Some(event.clone());
                        attached = true;
                        break;
                    }
                }

                if !attached {
                    items.push(ActivityItem::Tool {
                        call: None,
                        result: Some(event),
                    });
                }
            }
            _ => items.push(ActivityItem::Event(event)),
        }
    }

    if task_is_terminal {
        close_terminal_unmatched_tools(&mut items);
    }

    items
}

fn tool_pairing_key(event: &PersistedTaskEvent) -> Option<String> {
    [
        "id",
        "invocation_id",
        "tool_call_id",
        "provider_tool_call_id",
    ]
    .into_iter()
    .find_map(|key| payload_str_event(event, key).filter(|value| !value.is_empty()))
}

fn close_terminal_unmatched_tools(items: &mut [ActivityItem]) {
    for item in items.iter_mut() {
        let ActivityItem::Tool { call, result } = item else {
            continue;
        };
        if result.is_none()
            && let Some(call_event) = call.as_ref()
        {
            *result = Some(missing_tool_result_event(call_event));
        }
    }
}

fn missing_tool_result_event(call: &PersistedTaskEvent) -> PersistedTaskEvent {
    let name = payload_str_event(call, "name").unwrap_or_else(|| call.summary.clone());
    let mut payload = json!({
        "id": payload_str_event(call, "id"),
        "source": payload_str_event(call, "source").unwrap_or_else(|| "root".to_string()),
        "name": name,
        "success": false,
        "result_summary": "missing result after task finished",
        "output_preview": "No matching tool result was received before the task finished.",
    });
    if let Some(object) = payload.as_object_mut() {
        for key in ["invocation_id", "tool_call_id", "provider_tool_call_id"] {
            if let Some(value) = payload_str_event(call, key) {
                object.insert(key.to_string(), json!(value));
            }
        }
    }

    let mut result = call.clone();
    result.kind = TaskEventKind::ToolResult;
    result.summary = "Missing tool result".to_string();
    result.payload = payload;
    result.redacted = false;
    result.truncated = false;
    result
}

#[component]
fn ActivityItemCard(item: ActivityItem) -> impl IntoView {
    match item {
        ActivityItem::Tool { call, result } => {
            view! { <ToolCard call=call result=result /> }.into_any()
        }
        ActivityItem::Event(event) => view! { <AgentEventCard event=event /> }.into_any(),
    }
}

#[component]
fn AgentEventCard(event: PersistedTaskEvent) -> impl IntoView {
    if event.kind == TaskEventKind::Reasoning {
        return view! { <ReasoningEventCard event=event /> }.into_any();
    }

    let kind = event.kind.clone();
    let title = event_title(&event);
    let body = event_body(&event);
    let delivered_file = delivered_file_link(&event);

    view! {
        <details class="agent-event-card">
            <summary class="agent-event-summary">
                <span class="agent-event-kind">{event_kind_label(&kind)}</span>
                <span class="agent-event-title">{title}</span>
                {is_sub_agent_event(&event).then(|| view! { <span class="agent-event-flag">{sub_agent_label(&event)}</span> })}
                {event.truncated.then(|| view! { <span class="agent-event-flag">"truncated"</span> })}
                {event.redacted.then(|| view! { <span class="agent-event-flag danger">"redacted"</span> })}
            </summary>
            {delivered_file.map(|file| view! { <DeliveredFileEventBody file=file /> })}
            {body.map(|text| view! {
                <div class="agent-event-body">
                    <pre class="agent-event-pre">{text}</pre>
                </div>
            })}
        </details>
    }
    .into_any()
}

#[component]
fn ReasoningEventCard(event: PersistedTaskEvent) -> impl IntoView {
    let summary = reasoning_event_summary(&event).unwrap_or_else(|| "Thinking".to_string());
    let preview = compact_reasoning_preview(&summary, 140);
    let show_details = event.truncated || event.redacted || preview != summary;
    let details_summary = summary.clone();

    let mut header_metas = Vec::new();
    if event.truncated {
        header_metas.push(tool_meta("truncated"));
    }
    if event.redacted {
        header_metas.push(tool_meta_danger("redacted"));
    }
    if is_sub_agent_event(&event) {
        header_metas.push(tool_meta(sub_agent_label(&event)));
    }
    let class = if is_sub_agent_event(&event) {
        "tool-card agent-event-card reasoning-event-card sub-agent"
    } else {
        "tool-card agent-event-card reasoning-event-card"
    };

    view! {
        <section class=class>
            {tool_card_header_with_icon_class(
                "∴",
                "tool-status-icon reasoning-status-icon",
                "Thinking",
                header_metas,
            )}
            {tool_preview_with_class(preview, "tool-preview reasoning-preview")}
            {show_details.then(|| view! {
                <ToolDetailsWithClass open=false class="tool-card-body reasoning-details">
                    {tool_pre_stream(Some("reasoning"), details_summary)}
                </ToolDetailsWithClass>
            })}
        </section>
    }
}

fn sub_agent_label(event: &PersistedTaskEvent) -> String {
    sub_agent_event_name(event).unwrap_or_else(|| "sub-agent".to_string())
}

#[component]
fn ContextCard(progress: ReadSignal<Option<ProgressSnapshot>>) -> impl IntoView {
    let snapshot_memo = Memo::new(move |_| progress.get().and_then(|p| p.latest_token_snapshot));

    view! {
        {move || {
            let Some(snapshot) = snapshot_memo.get() else {
                return ().into_any();
            };
            let free = snapshot.get("headroom_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let flow = snapshot.get("hot_memory_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let prompt = snapshot.get("system_prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let tools = snapshot.get("tool_schema_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let budget = snapshot.get("budget_state").and_then(|v| v.as_str()).unwrap_or("context");
            let health_class = match budget {
                "Healthy" => "context-health-ok",
                "Warning" | "Approaching" => "context-health-warn",
                "Critical" | "Over" => "context-health-over",
                _ => "context-health-ok",
            };
            view! {
                <section class="context-card">
                    <div class="context-card-grid">
                        <div class="context-card-cell">
                            <div class="context-card-cell-label">"Free"</div>
                            <div class="context-card-cell-value">{compact_tokens(free)}</div>
                        </div>
                        <div class="context-card-cell">
                            <div class="context-card-cell-label">"Flow"</div>
                            <div class="context-card-cell-value">{compact_tokens(flow)}</div>
                        </div>
                        <div class="context-card-cell">
                            <div class="context-card-cell-label">"Prompt"</div>
                            <div class="context-card-cell-value">{compact_tokens(prompt)}</div>
                        </div>
                        <div class="context-card-cell">
                            <div class="context-card-cell-label">"Tools"</div>
                            <div class="context-card-cell-value">{compact_tokens(tools)}</div>
                        </div>
                    </div>
                    <div class="context-card-health">
                        <span class={format!("context-health-dot {health_class}")}></span>
                        <span class="context-health-label">{budget.to_lowercase()}</span>
                    </div>
                </section>
            }.into_any()
        }}
    }
}

#[component]
fn TodosCard(todos: Value) -> impl IntoView {
    let items = parse_todo_items_from_value(&todos);

    if items.is_empty() {
        return ().into_any();
    }

    render_todo_list(items, true)
}

fn is_chat_visible_event(kind: &TaskEventKind) -> bool {
    matches!(
        kind,
        TaskEventKind::Reasoning
            | TaskEventKind::BrowserLive
            | TaskEventKind::ToolCall
            | TaskEventKind::ToolResult
            | TaskEventKind::TodosUpdated
            | TaskEventKind::FileToSend
            | TaskEventKind::Continuation
            | TaskEventKind::Cancelling
            | TaskEventKind::Cancelled
            | TaskEventKind::Error
            | TaskEventKind::LoopDetected
            | TaskEventKind::RuntimeCompactionStarted
            | TaskEventKind::RuntimeCompactionCompleted
            | TaskEventKind::RuntimeCompactionFailed
            | TaskEventKind::RuntimeCompactionSkipped
            | TaskEventKind::RepeatedCompactionWarning
            | TaskEventKind::HistoryRepairApplied
            | TaskEventKind::Finished
    )
}

/// Filter out empty reasoning events and other noise.
fn is_useful_event(event: &PersistedTaskEvent) -> bool {
    if event.kind == TaskEventKind::Reasoning {
        return reasoning_event_summary(event).is_some();
    }
    true
}

fn reasoning_event_summary(event: &PersistedTaskEvent) -> Option<String> {
    payload_str_event(event, "summary")
        .map(|summary| summary.trim().to_string())
        .filter(|summary| !summary.is_empty() && summary != "Reasoning")
        .or_else(|| {
            let summary = event.summary.trim();
            (!summary.is_empty() && summary != "Reasoning").then(|| summary.to_string())
        })
}

fn compact_reasoning_preview(summary: &str, max_chars: usize) -> String {
    let compact = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = compact.chars();
    let preview = chars.by_ref().take(max_chars).collect::<String>();

    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}

fn event_kind_label(kind: &TaskEventKind) -> &'static str {
    match kind {
        TaskEventKind::ToolCall => "tool call",
        TaskEventKind::ToolResult => "tool result",
        TaskEventKind::TodosUpdated => "todos",
        TaskEventKind::Reasoning => "reasoning",
        TaskEventKind::BrowserLive => "browser",
        TaskEventKind::Error => "error",
        TaskEventKind::Finished => "done",
        TaskEventKind::Cancelling => "cancelling",
        TaskEventKind::Cancelled => "cancelled",
        TaskEventKind::LoopDetected => "loop detected",
        TaskEventKind::RuntimeCompactionStarted => "compacting",
        TaskEventKind::RuntimeCompactionCompleted => "compacted",
        TaskEventKind::RuntimeCompactionFailed => "compaction failed",
        TaskEventKind::RuntimeCompactionSkipped => "compaction skipped",
        TaskEventKind::RepeatedCompactionWarning => "repeated compaction",
        TaskEventKind::HistoryRepairApplied => "history repair",
        TaskEventKind::RateLimitRetrying => "rate limit retry",
        TaskEventKind::LlmRetrying => "llm retry",
        TaskEventKind::ProviderFailoverActivated => "provider failover",
        TaskEventKind::FileToSend => "file",
        TaskEventKind::Continuation => "continuation",
        _ => "event",
    }
}

fn event_title(event: &PersistedTaskEvent) -> String {
    match event.kind {
        TaskEventKind::ToolCall => {
            payload_str_event(event, "name").unwrap_or_else(|| event.summary.clone())
        }
        TaskEventKind::ToolResult => {
            let name = payload_str_event(event, "name").unwrap_or_else(|| event.summary.clone());
            payload_str_event(event, "result_summary")
                .filter(|summary| !summary.is_empty() && summary != &name)
                .map(|summary| format!("{name} — {summary}"))
                .unwrap_or(name)
        }
        _ => event.summary.clone(),
    }
}

fn event_body(event: &PersistedTaskEvent) -> Option<String> {
    if event.kind == TaskEventKind::FileToSend
        && payload_str_event(event, "download_url").is_some()
        && payload_str_event(event, "delivery_error").is_none()
    {
        return None;
    }
    if event.redacted {
        return Some("Payload redacted".to_string());
    }

    match event.kind {
        TaskEventKind::ToolCall => payload_str_event(event, "command_preview")
            .or_else(|| payload_str_event(event, "input_preview")),
        TaskEventKind::ToolResult => payload_str_event(event, "output_preview"),
        TaskEventKind::TodosUpdated => None,
        TaskEventKind::Reasoning => payload_str_event(event, "summary"),
        TaskEventKind::BrowserLive => serde_json::to_string_pretty(&event.payload).ok(),
        _ => serde_json::to_string_pretty(&event.payload).ok(),
    }
}
