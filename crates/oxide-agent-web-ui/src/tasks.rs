use crate::auth::use_auth;
use crate::components::{EmptyState, ErrorBanner, StatusBadge};
use crate::markdown::MarkdownContent;
use crate::routes::AppRoute;
use crate::sse::{spawn_task_stream, TaskStreamConfig};
use crate::utils::{friendly_time, spawn_ui};
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    CreateTaskRequest, EditTaskInputRequest, ErrorCode, PersistedTaskEvent, ProgressSnapshot,
    ResumeTaskRequest, SseConnectionState, TaskDetail, TaskEventKind, TaskStatus, TaskSummary,
};

#[component]
pub fn TaskConsole(
    route: AppRoute,
    events: ReadSignal<Vec<PersistedTaskEvent>>,
    progress: ReadSignal<Option<ProgressSnapshot>>,
    set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    set_sse_state: WriteSignal<SseConnectionState>,
    set_progress: WriteSignal<Option<ProgressSnapshot>>,
) -> impl IntoView {
    match route {
        AppRoute::Session(session_id) => view! {
            <SessionWorkspace
                session_id=session_id
                events=events
                progress=progress
                set_events=set_events
                set_sse_state=set_sse_state
                set_progress=set_progress
            />
        }
        .into_any(),
        _ => view! {
            <section class="console-empty">
                <EmptyState title="Select or create a session" />
            </section>
        }
        .into_any(),
    }
}

#[component]
fn SessionWorkspace(
    session_id: String,
    events: ReadSignal<Vec<PersistedTaskEvent>>,
    progress: ReadSignal<Option<ProgressSnapshot>>,
    set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    set_sse_state: WriteSignal<SseConnectionState>,
    set_progress: WriteSignal<Option<ProgressSnapshot>>,
) -> impl IntoView {
    let auth = use_auth();
    let (_session_title, set_session_title) = signal("Session".to_string());
    let (tasks, set_tasks) = signal(Vec::<TaskSummary>::new());
    let (input, set_input) = signal(String::new());
    let (error, set_error) = signal(None::<String>);
    let (loading, set_loading) = signal(false);
    let (active_task, set_active_task) = signal(None::<TaskDetail>);
    let (_streaming_task_id, set_streaming_task_id) = signal(None::<String>);
    let (loaded, set_loaded) = signal(false);

    // Determine which task owns the live activity display (active or most recent)
    let latest_activity_task_id = move || {
        active_task.get().map(|task| task.task_id).or_else(|| {
            tasks
                .get()
                .into_iter()
                .max_by_key(|task| task.updated_at)
                .map(|task| task.task_id)
        })
    };

    let session_id_for_load = session_id.clone();
    let load_all = move || {
        set_loading.set(true);
        set_error.set(None);
        // Clear stale state before loading
        set_events.set(Vec::new());
        set_progress.set(None);
        set_active_task.set(None);
        let session_id = session_id_for_load.clone();
        spawn_ui(async move {
            let client = auth.client();
            match client.get_session(&session_id).await {
                Ok(response) => set_session_title.set(response.session.title),
                Err(error) => set_error.set(Some(error.to_string())),
            }
            match client.list_tasks(&session_id).await {
                Ok(response) => {
                    let latest = latest_task(response.tasks.clone());
                    set_tasks.set(response.tasks);
                    if let Some(task) = latest {
                        let task_id = task.task_id.clone();
                        if let Ok(response) = client.get_task(&session_id, &task_id).await {
                            set_progress.set(response.task.last_progress.clone());
                            if matches!(
                                response.task.status,
                                TaskStatus::Queued | TaskStatus::Running
                            ) {
                                set_active_task.set(Some(response.task));
                                start_task_stream(
                                    client.clone(),
                                    session_id.clone(),
                                    task_id.clone(),
                                    StreamUiSignals {
                                        set_events,
                                        set_session_title,
                                        set_progress,
                                        set_active_task,
                                        set_tasks,
                                        set_sse_state,
                                        set_error,
                                        set_streaming_task_id,
                                    },
                                );
                            } else if response.task.status == TaskStatus::WaitingForUserInput {
                                set_active_task.set(Some(response.task));
                            } else {
                                set_active_task.set(None);
                            }
                        }
                        match client.task_events(&session_id, &task_id, 0).await {
                            Ok(response) => set_events.set(response.events),
                            Err(error) => set_error.set(Some(error.to_string())),
                        }
                    } else {
                        // Empty session — clear signals
                        set_events.set(Vec::new());
                        set_progress.set(None);
                        set_active_task.set(None);
                    }
                }
                Err(error) => set_error.set(Some(task_submit_error_message(&error))),
            }
            set_loading.set(false);
        });
    };

    Effect::new(move |_| {
        if !loaded.get() {
            set_loaded.set(true);
            load_all();
        }
    });

    let session_id_for_submit = session_id.clone();
    let submit_task = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let text = input.get();
        if text.trim().is_empty() {
            return;
        }
        set_loading.set(true);
        set_error.set(None);
        // Clear stale activity for the new task
        set_events.set(Vec::new());
        set_progress.set(None);
        let session_id = session_id_for_submit.clone();
        spawn_ui(async move {
            let client = auth.client();
            let resume_task_id = active_task
                .get()
                .filter(|task| task.status == TaskStatus::WaitingForUserInput)
                .map(|task| task.task_id);
            let result = match resume_task_id.as_deref() {
                Some(task_id) => client
                    .resume_task(
                        &session_id,
                        task_id,
                        &ResumeTaskRequest {
                            input_markdown: text,
                        },
                    )
                    .await
                    .map(|response| response.task),
                _ => client
                    .create_task(
                        &session_id,
                        &CreateTaskRequest {
                            input_markdown: text,
                        },
                    )
                    .await
                    .map(|response| response.task),
            };

            match result {
                Ok(task) => {
                    set_input.set(String::new());
                    set_active_task.set(Some(summary_to_detail(&session_id, &task)));
                    start_task_stream(
                        client,
                        session_id.clone(),
                        task.task_id.clone(),
                        StreamUiSignals {
                            set_events,
                            set_session_title,
                            set_progress,
                            set_active_task,
                            set_tasks,
                            set_sse_state,
                            set_error,
                            set_streaming_task_id,
                        },
                    );
                    if resume_task_id.is_some() {
                        set_tasks.update(|items| upsert_task_summary(items, task));
                    } else {
                        set_tasks.update(|items| items.push(task));
                    }
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_loading.set(false);
        });
    };

    let session_id_for_cancel = session_id.clone();
    let cancel_active = move |_| {
        let Some(task) = active_task.get() else {
            return;
        };
        set_loading.set(true);
        set_error.set(None);
        let session_id = session_id_for_cancel.clone();
        spawn_ui(async move {
            match auth.client().cancel_task(&session_id, &task.task_id).await {
                Ok(_) => {
                    let task_id = task.task_id.clone();
                    set_active_task.set(None);
                    set_tasks.update(|items| {
                        for item in items {
                            if item.task_id == task_id {
                                item.status = TaskStatus::Cancelled;
                            }
                        }
                    });
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_loading.set(false);
        });
    };

    let session_id_for_cards = session_id.clone();

    let is_waiting = move || {
        active_task
            .get()
            .is_some_and(|task| task.status == TaskStatus::WaitingForUserInput)
    };

    let is_running = move || {
        active_task
            .get()
            .is_some_and(|task| matches!(task.status, TaskStatus::Queued | TaskStatus::Running))
    };

    view! {
        <ErrorBanner message=error />
        <section class="session-workspace">
            // Agent results — task cards with output
            <div class="results-panel">
                {move || {
                    if loading.get() && tasks.get().is_empty() {
                        view! { <div class="empty-state">"Loading..."</div> }.into_any()
                    } else if tasks.get().is_empty() {
                        view! {
                            <div class="empty-state">
                                <div class="empty-state-title">"No active session"</div>
                                <div class="empty-state-text">
                                    "Enter a prompt below and click \"Run Agent\" to start a new session. The agent's reasoning, tool calls, and outputs will appear here in real time."
                                </div>
                            </div>
                        }
                        .into_any()
                    } else {
                        let latest_editable_task_id = latest_terminal_task_id(&tasks.get());
                        let session_id_for_cards = session_id_for_cards.clone();
                        view! {
                            <For
                                each=move || tasks.get()
                                key=|task| task.task_id.clone()
                                children=move |task| {
                                    let editable =
                                        latest_editable_task_id.as_ref() == Some(&task.task_id);
                                    let activity_owner = latest_activity_task_id()
                                        .as_ref() == Some(&task.task_id);
                                    view! {
                                        <TaskCard
                                            session_id=session_id_for_cards.clone()
                                            task=task
                                            editable=editable
                                            events=events
                                            progress=progress
                                            activity_owner=activity_owner
                                            set_tasks=set_tasks
                                            set_error=set_error
                                        />
                                    }
                                }
                            />
                        }
                        .into_any()
                    }
                }}
            </div>

            // Prompt input
            <form class="composer" on:submit=submit_task>
                <ComposerNotice active_task=active_task />
                <div class="composer-label">"Agent Prompt"</div>
                <textarea
                    placeholder="Enter your prompt here...\n\nThe agent will process your request and show its reasoning, tool calls, and outputs in the panel above."
                    prop:value=input
                    disabled=is_running
                    on:input=move |ev| set_input.set(event_target_value(&ev))
                    on:keydown=move |ev| {
                        if ev.ctrl_key() && ev.key() == "Enter" {
                            ev.prevent_default();
                            if let Some(target) = ev.target() {
                                use wasm_bindgen::JsCast;
                                let el: web_sys::HtmlElement = target.unchecked_into();
                                if let Ok(Some(form_el)) = el.closest("form") {
                                    if let Ok(Some(btn)) = form_el.query_selector("button[type=submit]") {
                                        let btn: web_sys::HtmlElement = btn.unchecked_into();
                                        btn.click();
                                    }
                                }
                            }
                        }
                    }
                />
                <div class="composer-actions">
                    <div class="composer-stats">
                        {move || {
                            let len = input.get().len();
                            let lines = input.get().lines().count().max(1);
                            format!("{} chars \u{00b7} {} lines", len, lines)
                        }}
                    </div>
                    <div style="display:flex;gap:8px;">
                        <button
                            type="submit"
                            disabled=move || loading.get() || is_running()
                            class=move || if is_waiting() { "btn-primary" } else { "" }
                        >
                            {move || {
                                if is_waiting() { "Resume" } else { "Run Agent" }
                            }}
                        </button>
                        <button
                            class="secondary"
                            type="button"
                            disabled=move || active_task.get().is_none() || !is_running()
                            on:click=cancel_active
                        >
                            "Stop"
                        </button>
                    </div>
                </div>
            </form>
        </section>
    }
}

#[component]
fn ComposerNotice(active_task: ReadSignal<Option<TaskDetail>>) -> impl IntoView {
    view! {
        {move || match active_task.get().map(|task| task.status) {
            Some(TaskStatus::WaitingForUserInput) => view! {
                <p class="composer-notice waiting">"The task is waiting for your reply. Sending will resume the same task."</p>
            }.into_any(),
            Some(TaskStatus::Queued | TaskStatus::Running) => view! {
                <p class="composer-notice busy">"This session is busy. Stop the active task before starting another one."</p>
            }.into_any(),
            _ => ().into_any(),
        }}
    }
}

#[derive(Clone, Copy)]
struct StreamUiSignals {
    set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    set_session_title: WriteSignal<String>,
    set_progress: WriteSignal<Option<ProgressSnapshot>>,
    set_active_task: WriteSignal<Option<TaskDetail>>,
    set_tasks: WriteSignal<Vec<TaskSummary>>,
    set_sse_state: WriteSignal<SseConnectionState>,
    set_error: WriteSignal<Option<String>>,
    set_streaming_task_id: WriteSignal<Option<String>>,
}

fn start_task_stream(
    client: crate::api::ApiClient,
    session_id: String,
    task_id: String,
    signals: StreamUiSignals,
) {
    signals.set_streaming_task_id.set(Some(task_id.clone()));
    spawn_task_stream(TaskStreamConfig {
        client,
        session_id,
        task_id,
        set_session_title: signals.set_session_title,
        set_events: signals.set_events,
        set_progress: signals.set_progress,
        set_active_task: signals.set_active_task,
        set_tasks: signals.set_tasks,
        set_state: signals.set_sse_state,
        set_error: signals.set_error,
        set_streaming_task_id: signals.set_streaming_task_id,
    });
}

fn summary_to_detail(session_id: &str, task: &TaskSummary) -> TaskDetail {
    TaskDetail {
        task_id: task.task_id.clone(),
        session_id: session_id.to_string(),
        status: task.status,
        input_markdown: task.input_markdown.clone(),
        input_edited_at: task.input_edited_at,
        final_response_markdown: task.final_response_markdown.clone(),
        error_message: task.error_message.clone(),
        pending_user_input: task.pending_user_input.clone(),
        last_progress: None,
        last_event_seq: task.last_event_seq,
        created_at: task.created_at,
        started_at: task.started_at,
        updated_at: task.updated_at,
        finished_at: task.finished_at,
    }
}

// ── Task Card ────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
#[component]
fn TaskCard(
    session_id: String,
    task: TaskSummary,
    editable: bool,
    events: ReadSignal<Vec<PersistedTaskEvent>>,
    progress: ReadSignal<Option<ProgressSnapshot>>,
    activity_owner: bool,
    set_tasks: WriteSignal<Vec<TaskSummary>>,
    set_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    let task_id = task.task_id.clone();
    let activity_task_id = task_id.clone();
    let (editing, set_editing) = signal(false);
    let (draft, set_draft) = signal(task.input_markdown.clone());
    let (saving, set_saving) = signal(false);
    let original_input = task.input_markdown.clone();

    let card_status_class = match task.status {
        TaskStatus::Running | TaskStatus::Queued => "running",
        TaskStatus::Completed => "success",
        TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Interrupted => "error",
        _ => "",
    };
    let card_class = format!("task-card {card_status_class}");

    view! {
        <article class=card_class>
            <div class="task-card-header">
                <StatusBadge status=task.status />
                <time>{friendly_time(task.updated_at)}</time>
            </div>
            <div class="message user-message">
                {move || if editing.get() {
                    let session_id = session_id.clone();
                    let task_id = task_id.clone();
                    let original_input = original_input.clone();
                    view! {
                        <TaskInputEditForm
                            session_id=session_id
                            task_id=task_id
                            original_input=original_input
                            draft=draft
                            set_draft=set_draft
                            saving=saving
                            set_saving=set_saving
                            set_editing=set_editing
                            set_tasks=set_tasks
                            set_error=set_error
                        />
                    }.into_any()
                } else {
                    view! {
                        <MarkdownContent markdown=task.input_markdown.clone() />
                    }.into_any()
                }}
            </div>
            {editable.then(|| view! {
                <button
                    class="secondary edit-input-button"
                    type="button"
                    on:click=move |_| set_editing.set(true)
                    style="margin-top:8px;"
                >
                    "Edit input"
                </button>
            })}

            // Inline agent activity: tool calls, results, todos — between user message and final answer
            <AgentActivity
                task_id=activity_task_id
                events=events
                progress=progress
                activity_owner=activity_owner
            />

            {task.final_response_markdown.map(|answer| view! {
                <div class="message assistant-message">
                    <MarkdownContent markdown=answer />
                </div>
            })}
            {task.error_message.map(|error| view! {
                <div class="message error-message">{error}</div>
            })}
            {task.pending_user_input.map(|pending| view! {
                <div class="message pending-message">{pending.prompt}</div>
            })}
        </article>
    }
}

// ── Agent Activity (inline between user message and final answer) ────────

#[component]
fn AgentActivity(
    task_id: String,
    events: ReadSignal<Vec<PersistedTaskEvent>>,
    progress: ReadSignal<Option<ProgressSnapshot>>,
    activity_owner: bool,
) -> impl IntoView {
    view! {
        {move || {
            let task_events = events
                .get()
                .into_iter()
                .filter(|event| event.task_id == task_id)
                .filter(|event| is_chat_visible_event(&event.kind))
                .collect::<Vec<_>>();

            let todos = if activity_owner {
                progress.get().and_then(|p| p.current_todos)
            } else {
                None
            };

            if task_events.is_empty() && todos.is_none() {
                return ().into_any();
            }

            view! {
                <div class="agent-activity">
                    {todos.map(|value| view! {
                        <TodosCard todos=value />
                    })}
                    {task_events.into_iter().map(|event| {
                        view! { <AgentEventCard event=event /> }
                    }).collect::<Vec<_>>()}
                </div>
            }.into_any()
        }}
    }
}

// ── Agent Event Card ─────────────────────────────────────────────────────

#[component]
fn AgentEventCard(event: PersistedTaskEvent) -> impl IntoView {
    let kind = event.kind.clone();
    let title = event_title(&event);
    let body = event_body(&event);
    let default_open = matches!(kind, TaskEventKind::ToolCall | TaskEventKind::ToolResult);

    view! {
        <details class="agent-event-card" open=default_open>
            <summary class="agent-event-summary">
                <span class="agent-event-kind">{event_kind_label(&kind)}</span>
                <span class="agent-event-title">{title}</span>
                {event.truncated.then(|| view! { <span class="agent-event-flag">"truncated"</span> })}
                {event.redacted.then(|| view! { <span class="agent-event-flag danger">"redacted"</span> })}
            </summary>
            {body.map(|text| view! {
                <div class="agent-event-body">
                    <pre class="agent-event-pre">{text}</pre>
                </div>
            })}
        </details>
    }
}

// ── Todos Card ───────────────────────────────────────────────────────────

#[component]
fn TodosCard(todos: serde_json::Value) -> impl IntoView {
    let items = todos
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if items.is_empty() {
        return ().into_any();
    }

    view! {
        <section class="todos-card">
            <div class="todos-card-title">"Todos"</div>
            <ol class="todo-list">
                {items.into_iter().map(|item| {
                    let description = item
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    let status = item
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("pending")
                        .to_string();

                    view! {
                        <li class=format!("todo-item {}", status)>
                            <span class="todo-status">{todo_status_label(&status)}</span>
                            <span class="todo-description">{description}</span>
                        </li>
                    }
                }).collect::<Vec<_>>()}
            </ol>
        </section>
    }
    .into_any()
}

// ── Event helpers ────────────────────────────────────────────────────────

fn is_chat_visible_event(kind: &TaskEventKind) -> bool {
    matches!(
        kind,
        TaskEventKind::Reasoning
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
            | TaskEventKind::RateLimitRetrying
            | TaskEventKind::LlmRetrying
            | TaskEventKind::ProviderFailoverActivated
            | TaskEventKind::Finished
    )
}

fn event_kind_label(kind: &TaskEventKind) -> &'static str {
    match kind {
        TaskEventKind::ToolCall => "tool call",
        TaskEventKind::ToolResult => "tool result",
        TaskEventKind::TodosUpdated => "todos",
        TaskEventKind::Reasoning => "reasoning",
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
        TaskEventKind::ToolCall | TaskEventKind::ToolResult => {
            payload_str(event, "name").unwrap_or_else(|| event.summary.clone())
        }
        _ => event.summary.clone(),
    }
}

fn event_body(event: &PersistedTaskEvent) -> Option<String> {
    if event.redacted {
        return Some("Payload redacted".to_string());
    }

    match event.kind {
        TaskEventKind::ToolCall => {
            payload_str(event, "command_preview").or_else(|| payload_str(event, "input_preview"))
        }
        TaskEventKind::ToolResult => payload_str(event, "output_preview"),
        TaskEventKind::TodosUpdated => None,
        TaskEventKind::Reasoning => payload_str(event, "summary"),
        _ => serde_json::to_string_pretty(&event.payload).ok(),
    }
}

fn payload_str(event: &PersistedTaskEvent, key: &str) -> Option<String> {
    event
        .payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

fn todo_status_label(status: &str) -> &'static str {
    match status {
        "completed" => "done",
        "in_progress" => "doing",
        "blocked_on_user" => "blocked",
        "cancelled" => "cancelled",
        _ => "todo",
    }
}

// ── Task input edit form ─────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
#[component]
fn TaskInputEditForm(
    session_id: String,
    task_id: String,
    original_input: String,
    draft: ReadSignal<String>,
    set_draft: WriteSignal<String>,
    saving: ReadSignal<bool>,
    set_saving: WriteSignal<bool>,
    set_editing: WriteSignal<bool>,
    set_tasks: WriteSignal<Vec<TaskSummary>>,
    set_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    let auth = use_auth();
    let submit_edit = {
        let session_id = session_id.clone();
        let task_id = task_id.clone();
        move |ev: leptos::ev::SubmitEvent| {
            ev.prevent_default();
            set_saving.set(true);
            set_error.set(None);
            let request = EditTaskInputRequest {
                input_markdown: draft.get(),
            };
            let session_id = session_id.clone();
            let task_id = task_id.clone();
            spawn_ui(async move {
                match auth
                    .client()
                    .edit_task_input(&session_id, &task_id, &request)
                    .await
                {
                    Ok(response) => {
                        set_tasks.update(|items| {
                            if let Some(item) =
                                items.iter_mut().find(|item| item.task_id == task_id)
                            {
                                *item = response.task;
                            }
                        });
                        set_editing.set(false);
                    }
                    Err(error) => set_error.set(Some(error.to_string())),
                }
                set_saving.set(false);
            });
        }
    };
    let cancel_edit = move |_| {
        set_draft.set(original_input.clone());
        set_editing.set(false);
    };

    view! {
        <form class="inline-edit" on:submit=submit_edit>
            <textarea
                prop:value=draft
                on:input=move |ev| set_draft.set(event_target_value(&ev))
            />
            <div class="composer-actions">
                <button type="submit" disabled=saving>"Save"</button>
                <button class="secondary" type="button" on:click=cancel_edit>"Cancel"</button>
            </div>
        </form>
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn latest_task(tasks: Vec<TaskSummary>) -> Option<TaskSummary> {
    tasks.into_iter().max_by_key(|task| task.updated_at)
}

fn latest_terminal_task_id(tasks: &[TaskSummary]) -> Option<String> {
    tasks
        .iter()
        .filter(|task| task.status.is_terminal())
        .max_by_key(|task| task.updated_at)
        .map(|task| task.task_id.clone())
}

fn upsert_task_summary(items: &mut Vec<TaskSummary>, task: TaskSummary) {
    if let Some(existing) = items.iter_mut().find(|item| item.task_id == task.task_id) {
        *existing = task;
    } else {
        items.push(task);
    }
}

fn task_submit_error_message(error: &crate::api::ApiClientError) -> String {
    match error.error_code() {
        Some(ErrorCode::SessionBusy) => {
            "This session already has an active task. Stop it or wait for it to finish.".to_string()
        }
        Some(ErrorCode::TaskWaitingForUserInput) => {
            "The active task is waiting for input. Reply in the composer to resume it.".to_string()
        }
        _ => error.to_string(),
    }
}
