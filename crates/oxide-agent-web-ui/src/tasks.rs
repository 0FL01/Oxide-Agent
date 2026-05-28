use crate::auth::use_auth;
use crate::components::{EmptyState, ErrorBanner, StatusBadge};
use crate::markdown::MarkdownContent;
use crate::routes::AppRoute;
use crate::sessions::RenameSessionForm;
use crate::utils::{friendly_time, spawn_ui};
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    CreateTaskRequest, PersistedTaskEvent, ResumeTaskRequest, TaskDetail, TaskStatus, TaskSummary,
};

#[component]
pub fn TaskConsole(route: AppRoute) -> impl IntoView {
    match route {
        AppRoute::Session(session_id) => {
            view! { <SessionWorkspace session_id=session_id /> }.into_any()
        }
        _ => view! {
            <section class="console-empty">
                <EmptyState title="Select or create a session" />
            </section>
        }
        .into_any(),
    }
}

#[component]
fn SessionWorkspace(session_id: String) -> impl IntoView {
    let auth = use_auth();
    let (session_title, set_session_title) = signal("Session".to_string());
    let (tasks, set_tasks) = signal(Vec::<TaskSummary>::new());
    let (events, set_events) = signal(Vec::<PersistedTaskEvent>::new());
    let (input, set_input) = signal(String::new());
    let (error, set_error) = signal(None::<String>);
    let (loading, set_loading) = signal(false);
    let (active_task, set_active_task) = signal(None::<TaskDetail>);
    let (loaded, set_loaded) = signal(false);

    let session_id_for_load = session_id.clone();
    let load_all = move || {
        set_loading.set(true);
        set_error.set(None);
        let session_id = session_id_for_load.clone();
        spawn_ui(async move {
            let client = auth.client();
            match client.get_session(&session_id).await {
                Ok(response) => set_session_title.set(response.session.title),
                Err(error) => set_error.set(Some(error.to_string())),
            }
            match client.list_tasks(&session_id).await {
                Ok(response) => {
                    let latest_running = response
                        .tasks
                        .iter()
                        .rev()
                        .find(|task| task.status.is_active())
                        .map(|task| task.task_id.clone());
                    set_tasks.set(response.tasks);
                    if let Some(task_id) = latest_running {
                        if let Ok(response) = client.get_task(&session_id, &task_id).await {
                            set_active_task.set(Some(response.task));
                        }
                    }
                }
                Err(error) => set_error.set(Some(error.to_string())),
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
        let session_id = session_id_for_submit.clone();
        spawn_ui(async move {
            let client = auth.client();
            let result = match active_task.get() {
                Some(task) if task.status == TaskStatus::WaitingForUserInput => client
                    .resume_task(
                        &session_id,
                        &task.task_id,
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
                    set_tasks.update(|items| items.push(task));
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

    let session_id_for_events = session_id.clone();
    let refresh_events = move |_| {
        let Some(task) = latest_task(tasks.get()) else {
            return;
        };
        set_error.set(None);
        let session_id = session_id_for_events.clone();
        spawn_ui(async move {
            match auth
                .client()
                .task_events(&session_id, &task.task_id, 0)
                .await
            {
                Ok(response) => set_events.set(response.events),
                Err(error) => set_error.set(Some(error.to_string())),
            }
        });
    };

    view! {
        <section class="session-workspace">
            <header class="session-header">
                <div>
                    <h1>{move || session_title.get()}</h1>
                    <p>{session_id.clone()}</p>
                </div>
                <RenameSessionForm session_id=session_id.clone() current_title=session_title.get_untracked() />
            </header>
            <ErrorBanner message=error />
            <div class="console-grid">
                <section class="transcript-panel">
                    {move || {
                        if loading.get() && tasks.get().is_empty() {
                            view! { <div class="loading">"Loading"</div> }.into_any()
                        } else if tasks.get().is_empty() {
                            view! { <EmptyState title="No task yet" /> }.into_any()
                        } else {
                            view! {
                                <div class="task-list">
                                    <For
                                        each=move || tasks.get()
                                        key=|task| task.task_id.clone()
                                        children=move |task| view! { <TaskCard task=task /> }
                                    />
                                </div>
                            }.into_any()
                        }
                    }}
                    <form class="composer" on:submit=submit_task>
                        <textarea
                            placeholder="Markdown input"
                            prop:value=input
                            disabled=move || active_task.get().is_some_and(|task| task.status == TaskStatus::Running)
                            on:input=move |ev| set_input.set(event_target_value(&ev))
                        />
                        <div class="composer-actions">
                            <button type="submit" disabled=loading>"Send"</button>
                            <button
                                class="secondary"
                                type="button"
                                disabled=move || active_task.get().is_none()
                                on:click=cancel_active
                            >
                                "Stop"
                            </button>
                        </div>
                    </form>
                </section>
                <aside class="events-panel">
                    <div class="panel-header">
                        <h2>"Events"</h2>
                        <button class="secondary" type="button" on:click=refresh_events>"Refresh"</button>
                    </div>
                    {move || {
                        if events.get().is_empty() {
                            view! { <EmptyState title="No events" /> }.into_any()
                        } else {
                            view! {
                                <ol class="event-list">
                                    <For
                                        each=move || events.get()
                                        key=|event| event.seq
                                        children=move |event| view! { <EventRow event=event /> }
                                    />
                                </ol>
                            }.into_any()
                        }
                    }}
                </aside>
            </div>
        </section>
    }
}

#[component]
fn TaskCard(task: TaskSummary) -> impl IntoView {
    view! {
        <article class="task-card">
            <div class="task-card-header">
                <StatusBadge status=task.status />
                <time>{friendly_time(task.updated_at)}</time>
            </div>
            <div class="message user-message">
                <MarkdownContent markdown=task.input_markdown />
            </div>
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

fn latest_task(tasks: Vec<TaskSummary>) -> Option<TaskSummary> {
    tasks.into_iter().max_by_key(|task| task.updated_at)
}
