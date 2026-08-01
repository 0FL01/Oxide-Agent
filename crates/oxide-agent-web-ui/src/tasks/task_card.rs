use crate::auth::use_auth;
use crate::markdown::MarkdownContent;
use crate::utils::spawn_ui;
use leptos::{html, prelude::*};
use oxide_agent_web_contracts::{
    CreateTaskVersionRequest, PersistedTaskEvent, TaskAttachment, TaskEventKind, TaskStatus,
    TaskSummary, UserMessageEventPayload,
};
use std::collections::HashMap;

use super::WEB_AGENT_EFFORT;
use super::activity::ThinkingButton;
use super::composer::MessageAttachments;
use super::delivered_files::{
    DeliveredFilesMessage, delivered_files_for_task, linkify_delivered_files_in_markdown,
};
use super::state::{
    InlineTaskProgress, TaskActivityState, activity_button_label, inline_task_progress,
    summary_to_detail, upsert_task_summary,
};
use super::streaming::{StreamUiSignals, start_task_stream};
use super::versions::{selected_version_index, versions_for_group};

// ── Task Card ────────────────────────────────────────────────────────────

#[derive(Clone)]
pub(super) struct TaskCardModel {
    pub(super) session_id: String,
    pub(super) version_group_id: String,
    pub(super) tasks: ReadSignal<Vec<TaskSummary>>,
    pub(super) editable_task_id: Option<String>,
    pub(super) now_millis: ReadSignal<i64>,
}

/// Reactive snapshot of the currently selected version within a group.
/// Derived from the live `tasks` signal (not a by-value prop) so SSE
/// status/timestamp updates reach the card without recreating it.
#[derive(Clone, PartialEq)]
struct SelectedTaskView {
    task: TaskSummary,
    selected_index: usize,
    version_count: usize,
    previous_task: Option<TaskSummary>,
    next_task: Option<TaskSummary>,
}

#[derive(Clone, Copy)]
pub(super) struct TaskCardSignals {
    pub(super) events: ReadSignal<Vec<PersistedTaskEvent>>,
    pub(super) activity_states: ReadSignal<HashMap<String, TaskActivityState>>,
    pub(super) selected_versions: ReadSignal<HashMap<String, String>>,
    pub(super) set_selected_versions: WriteSignal<HashMap<String, String>>,
    pub(super) set_activity: Callback<Option<String>>,
    pub(super) stream_signals: StreamUiSignals,
    pub(super) set_error: WriteSignal<Option<String>>,
}

#[component]
pub(super) fn TaskCard(model: TaskCardModel, signals: TaskCardSignals) -> impl IntoView {
    let TaskCardModel {
        session_id,
        version_group_id,
        tasks,
        editable_task_id,
        now_millis,
    } = model;
    let TaskCardSignals {
        events,
        activity_states,
        selected_versions,
        set_selected_versions,
        set_activity,
        stream_signals,
        set_error,
    } = signals;
    let (editing, set_editing) = signal(false);
    let initial_versions = versions_for_group(&tasks.get_untracked(), &version_group_id);
    let (draft, set_draft) = signal(
        initial_versions
            .last()
            .map(|task| task.input_markdown.clone())
            .unwrap_or_default(),
    );
    let (saving, set_saving) = signal(false);
    let selected = Memo::new({
        let version_group_id = version_group_id.clone();
        move |_| {
            let versions = versions_for_group(&tasks.get(), &version_group_id);
            let selected_task_id = selected_versions.get().get(&version_group_id).cloned();
            let selected_index = selected_version_index(&versions, selected_task_id.as_deref());
            let version_count = versions.len();
            let task = versions.get(selected_index).cloned();
            let previous_task = selected_index
                .checked_sub(1)
                .and_then(|index| versions.get(index).cloned());
            let next_task = (selected_index + 1 < version_count)
                .then(|| versions.get(selected_index + 1).cloned())
                .flatten();
            task.map(|task| SelectedTaskView {
                task,
                selected_index,
                version_count,
                previous_task,
                next_task,
            })
        }
    });

    Effect::new(move |_| {
        if let Some(view) = selected.get()
            && !editing.get()
        {
            set_draft.set(view.task.input_markdown.clone());
        }
    });

    // The elapsed label ticks with the shared browser clock, independent of
    // SSE/task updates. Isolating it in its own memo keeps the 1s re-render
    // scoped to this label text only — the rest of the card does not re-run
    // every second.
    let activity_label = Memo::new(move |_| {
        selected
            .get()
            .map(|view| activity_button_label(&view.task, now_millis.get()))
            .unwrap_or_default()
    });

    let edit_signals = TaskInputEditSignals {
        draft,
        set_draft,
        saving,
        set_saving,
        set_editing,
        set_selected_versions,
        set_activity,
        stream_signals,
        set_error,
    };
    let version_signals = VersionSwitchSignals {
        set_editing,
        set_draft,
        set_selected_versions,
        set_activity,
    };

    view! {
        {move || {
            let Some(view) = selected.get() else {
                return ().into_any();
            };
            let task = view.task;
            let selected_index = view.selected_index;
            let version_count = view.version_count;
            let editable = editable_task_id.as_ref() == Some(&task.task_id);
            let original_input = task.input_markdown.clone();
            let input_markdown = task.input_markdown.clone();
            let attachments = task.attachments.clone();
            let final_response_markdown = task.final_response_markdown.clone();
            let error_message = task.error_message.clone();
            let pending_user_input = task.pending_user_input.clone();
            let task_events = events.get();
            let resume_messages = resume_user_messages_for_task(&task_events, &task.task_id);
            let delivered_files = delivered_files_for_task(&task_events, &task.task_id);
            let inline_progress = inline_task_progress(
                &task.task_id,
                task.status,
                &task_events,
                activity_states.get().get(&task.task_id),
            );
            let assistant_output = match task.status {
                TaskStatus::Queued | TaskStatus::Running => inline_progress
                    .map(|progress| view! { <InlineAssistantProgress progress=progress /> }.into_any())
                    .unwrap_or_else(|| ().into_any()),
                TaskStatus::WaitingForUserInput => pending_user_input
                    .map(|pending| {
                        view! { <div class="message pending-message">{pending.prompt}</div> }
                            .into_any()
                    })
                    .unwrap_or_else(|| ().into_any()),
                TaskStatus::Completed => final_response_markdown
                    .map(|answer| {
                        view! { <AssistantMessage answer=answer files=delivered_files.clone() /> }
                            .into_any()
                    })
                    .unwrap_or_else(|| ().into_any()),
                TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Interrupted => {
                    let message = error_message.unwrap_or_else(|| match task.status {
                        TaskStatus::Failed => "Task failed.".to_string(),
                        TaskStatus::Cancelled => "Task cancelled.".to_string(),
                        TaskStatus::Interrupted => "Task interrupted.".to_string(),
                        _ => unreachable!("terminal error branch only"),
                    });
                    view! { <div class="message error-message">{message}</div> }.into_any()
                }
            };
            let task_id_for_activity = task.task_id.clone();
            let open_activity = Callback::new(move |_| {
                set_activity.run(Some(task_id_for_activity.clone()));
            });
            let previous_task = view.previous_task;
            let next_task = view.next_task;

            view! {
                <article class="task-card">
                    <TaskUserMessagePanel
                        data=UserMessagePanelData {
                            session_id: session_id.clone(),
                            task_id: task.task_id.clone(),
                            version_group_id: version_group_id.clone(),
                            original_input,
                            input_markdown,
                            attachments,
                            editable,
                            version_count,
                            selected_index,
                            previous_task,
                            next_task,
                        }
                        signals=UserMessagePanelSignals {
                            editing,
                            edit_signals,
                            version_signals,
                        }
                    />
                    <ResumeUserMessages messages=resume_messages />
                    {assistant_output}
                    <div class="task-action-row">
                        <ThinkingButton label=activity_label on_click=open_activity />
                    </div>
                    {(!delivered_files.is_empty()).then(|| view! {
                        <DeliveredFilesMessage files=delivered_files.clone() />
                    })}
                </article>
            }
                .into_any()
        }}
    }
}

#[component]
fn InlineAssistantProgress(progress: InlineTaskProgress) -> impl IntoView {
    view! {
        <div
            class="message assistant-progress"
            role="status"
            aria-live="polite"
            aria-atomic="true"
            aria-label="Assistant progress"
        >
            <div class="assistant-progress-narrative">
                <MarkdownContent markdown=progress.narrative />
            </div>
            {progress.operation.map(|operation| view! {
                <div class="assistant-progress-operation">{operation}</div>
            })}
        </div>
    }
}

#[derive(Clone)]
struct UserMessagePanelData {
    session_id: String,
    task_id: String,
    version_group_id: String,
    original_input: String,
    input_markdown: String,
    attachments: Vec<TaskAttachment>,
    editable: bool,
    version_count: usize,
    selected_index: usize,
    previous_task: Option<TaskSummary>,
    next_task: Option<TaskSummary>,
}

#[derive(Clone, Copy)]
struct UserMessagePanelSignals {
    editing: ReadSignal<bool>,
    edit_signals: TaskInputEditSignals,
    version_signals: VersionSwitchSignals,
}

#[component]
fn TaskUserMessagePanel(
    data: UserMessagePanelData,
    signals: UserMessagePanelSignals,
) -> impl IntoView {
    let UserMessagePanelData {
        session_id,
        task_id,
        version_group_id,
        original_input,
        input_markdown,
        attachments,
        editable,
        version_count,
        selected_index,
        previous_task,
        next_task,
    } = data;
    let edit_target = TaskInputEditTarget {
        session_id,
        task_id,
        version_group_id: version_group_id.clone(),
        original_input,
        attachments: attachments.clone(),
    };
    let actions_data = UserMessageActionsData {
        input_markdown: input_markdown.clone(),
        editable,
        version_group_id,
        version_count,
        selected_index,
        previous_task,
        next_task,
    };

    view! {
        <div class="message user-message-wrap">
            <div class="user-message">
                {move || {
                    if signals.editing.get() {
                        view! {
                            <TaskInputEditForm
                                target=edit_target.clone()
                                signals=signals.edit_signals
                            />
                        }
                        .into_any()
                    } else {
                        view! {
                            <UserMessageBody
                                input_markdown=input_markdown.clone()
                                attachments=attachments.clone()
                            />
                        }
                        .into_any()
                    }
                }}
            </div>
            <UserMessageActions data=actions_data signals=signals.version_signals />
        </div>
    }
}

#[derive(Clone)]
struct UserMessageActionsData {
    input_markdown: String,
    editable: bool,
    version_group_id: String,
    version_count: usize,
    selected_index: usize,
    previous_task: Option<TaskSummary>,
    next_task: Option<TaskSummary>,
}

#[derive(Clone, Copy)]
struct VersionSwitchSignals {
    set_editing: WriteSignal<bool>,
    set_draft: WriteSignal<String>,
    set_selected_versions: WriteSignal<HashMap<String, String>>,
    set_activity: Callback<Option<String>>,
}

#[component]
fn UserMessageActions(
    data: UserMessageActionsData,
    signals: VersionSwitchSignals,
) -> impl IntoView {
    let UserMessageActionsData {
        input_markdown,
        editable,
        version_group_id,
        version_count,
        selected_index,
        previous_task,
        next_task,
    } = data;
    let copy_input = input_markdown.clone();
    let can_select_previous = previous_task.is_some();
    let can_select_next = next_task.is_some();
    let version_counter = format!("{}/{}", selected_index + 1, version_count);
    let previous_version_group_id = version_group_id.clone();
    let next_version_group_id = version_group_id.clone();

    view! {
        <div class="user-message-actions" aria-label="User message actions">
            {editable.then(|| view! {
                <button
                    class="message-action-button"
                    type="button"
                    title="Edit input"
                    aria-label="Edit input"
                    on:click=move |_| signals.set_editing.set(true)
                >
                    "✎"
                </button>
            })}
            <button
                class="message-action-button"
                type="button"
                title="Copy user message"
                aria-label="Copy user message"
                on:click=move |_| copy_to_clipboard(&copy_input)
            >
                "⧉"
            </button>
            {(version_count > 1).then(|| {
                view! {
                    <div class="message-version-switcher" aria-label="Task version history">
                        <button
                            class="message-action-button"
                            type="button"
                            title="Previous version"
                            aria-label="Previous version"
                            disabled=!can_select_previous
                            on:click=move |_| {
                                select_task_version(
                                    previous_task.clone(),
                                    &previous_version_group_id,
                                    signals,
                                );
                            }
                        >
                            "‹"
                        </button>
                        <div class="message-version-counter">{version_counter.clone()}</div>
                        <button
                            class="message-action-button"
                            type="button"
                            title="Next version"
                            aria-label="Next version"
                            disabled=!can_select_next
                            on:click=move |_| {
                                select_task_version(
                                    next_task.clone(),
                                    &next_version_group_id,
                                    signals,
                                );
                            }
                        >
                            "›"
                        </button>
                    </div>
                }
            })}
        </div>
    }
}

fn select_task_version(
    task: Option<TaskSummary>,
    version_group_id: &str,
    signals: VersionSwitchSignals,
) {
    if let Some(task) = task {
        signals.set_editing.set(false);
        signals.set_draft.set(task.input_markdown.clone());
        signals.set_selected_versions.update(|items| {
            items.insert(version_group_id.to_string(), task.task_id);
        });
        signals.set_activity.run(None);
    }
}

#[component]
fn ResumeUserMessages(messages: Vec<ResumeUserMessage>) -> impl IntoView {
    messages
        .into_iter()
        .map(|message| {
            view! {
                <div class="message user-message-wrap">
                    <div class="user-message">
                        <UserMessageBody
                            input_markdown=message.input_markdown
                            attachments=message.attachments
                        />
                    </div>
                </div>
            }
        })
        .collect_view()
}

#[component]
fn AssistantMessage(
    answer: String,
    files: Vec<super::delivered_files::DeliveredFileLink>,
) -> impl IntoView {
    let raw_answer = answer.clone();
    let rendered_answer = linkify_delivered_files_in_markdown(&answer, &files);

    view! {
        <div class="message assistant-message-wrap">
            <div class="assistant-message">
                <MarkdownContent markdown=rendered_answer />
            </div>
            <div class="assistant-message-actions" aria-label="Assistant message actions">
                <button
                    class="message-action-button"
                    type="button"
                    title="Copy final response"
                    aria-label="Copy final response"
                    on:click=move |_| copy_to_clipboard(&raw_answer)
                >
                    "⧉"
                </button>
            </div>
        </div>
    }
}

fn copy_to_clipboard(text: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window.navigator().clipboard().write_text(text);
    }
}

#[component]
fn CollapsibleMessageBody(markdown: String) -> impl IntoView {
    let (expanded, set_expanded) = signal(false);
    let (overflowing, set_overflowing) = signal(false);
    let body_ref = NodeRef::<html::Div>::new();
    let measure_ref = body_ref;

    Effect::new(move |_| {
        let Some(body) = measure_ref.get() else {
            return;
        };
        if expanded.get() {
            return;
        }
        set_overflowing.set(body.scroll_height() > body.client_height() + 1);
    });

    view! {
        <div class="message-collapsible">
            <div
                class=move || {
                    if expanded.get() {
                        "message-collapsible-body is-expanded"
                    } else {
                        "message-collapsible-body is-collapsed"
                    }
                }
                node_ref=body_ref
            >
                <MarkdownContent markdown=markdown />
                {move || {
                    if overflowing.get() && !expanded.get() {
                        view! { <div class="message-collapsible-fade"></div> }.into_any()
                    } else {
                        ().into_any()
                    }
                }}
            </div>
            {move || {
                if overflowing.get() {
                    view! {
                        <button
                            class="message-expand-button secondary"
                            type="button"
                            on:click=move |_| set_expanded.update(|value| *value = !*value)
                        >
                            {move || if expanded.get() { "Show less" } else { "Show more" }}
                        </button>
                    }
                    .into_any()
                } else {
                    ().into_any()
                }
            }}
        </div>
    }
}

#[derive(Clone)]
struct ResumeUserMessage {
    input_markdown: String,
    attachments: Vec<TaskAttachment>,
}

#[component]
fn UserMessageBody(input_markdown: String, attachments: Vec<TaskAttachment>) -> impl IntoView {
    let rendered_input_markdown = input_markdown.clone();

    view! {
        <div class="user-message-body">
            <MessageAttachments attachments=attachments />
            {(!rendered_input_markdown.trim().is_empty()).then(|| view! {
                <CollapsibleMessageBody markdown=rendered_input_markdown />
            })}
        </div>
    }
}

fn resume_user_messages_for_task(
    events: &[PersistedTaskEvent],
    task_id: &str,
) -> Vec<ResumeUserMessage> {
    events
        .iter()
        .filter(|event| event.task_id == task_id && event.kind == TaskEventKind::UserMessage)
        .filter_map(|event| {
            serde_json::from_value::<UserMessageEventPayload>(event.payload.clone()).ok()
        })
        .map(|payload| ResumeUserMessage {
            input_markdown: payload.input_markdown,
            attachments: payload.attachments,
        })
        .collect()
}

// ── Task input edit form ─────────────────────────────────────────────────

#[derive(Clone)]
struct TaskInputEditTarget {
    session_id: String,
    task_id: String,
    version_group_id: String,
    original_input: String,
    attachments: Vec<TaskAttachment>,
}

#[derive(Clone, Copy)]
struct TaskInputEditSignals {
    draft: ReadSignal<String>,
    set_draft: WriteSignal<String>,
    saving: ReadSignal<bool>,
    set_saving: WriteSignal<bool>,
    set_editing: WriteSignal<bool>,
    set_selected_versions: WriteSignal<HashMap<String, String>>,
    set_activity: Callback<Option<String>>,
    stream_signals: StreamUiSignals,
    set_error: WriteSignal<Option<String>>,
}

#[component]
fn TaskInputEditForm(target: TaskInputEditTarget, signals: TaskInputEditSignals) -> impl IntoView {
    let TaskInputEditTarget {
        session_id,
        task_id,
        version_group_id,
        original_input,
        attachments,
    } = target;
    let TaskInputEditSignals {
        draft,
        set_draft,
        saving,
        set_saving,
        set_editing,
        set_selected_versions,
        set_activity,
        stream_signals,
        set_error,
    } = signals;
    let auth = use_auth();
    let submit_edit = {
        let session_id = session_id.clone();
        let task_id = task_id.clone();
        let version_group_id = version_group_id.clone();
        move |ev: leptos::ev::SubmitEvent| {
            ev.prevent_default();
            set_saving.set(true);
            set_error.set(None);
            let request = CreateTaskVersionRequest {
                input_markdown: draft.get(),
                attachments: attachments.clone(),
                effort: Some(WEB_AGENT_EFFORT),
            };
            let session_id = session_id.clone();
            let task_id = task_id.clone();
            let version_group_id = version_group_id.clone();
            spawn_ui(async move {
                let client = auth.client();
                match client
                    .create_task_version(&session_id, &task_id, &request)
                    .await
                {
                    Ok(response) => {
                        let task = response.task;
                        stream_signals
                            .update_progress
                            .run((task.task_id.clone(), None));
                        stream_signals
                            .set_active_task
                            .set(Some(summary_to_detail(&session_id, &task)));
                        stream_signals
                            .set_tasks
                            .update(|items| upsert_task_summary(items, task.clone()));
                        set_selected_versions.update(|items| {
                            items.insert(version_group_id.clone(), task.task_id.clone());
                        });
                        set_activity.run(None);
                        start_task_stream(
                            client,
                            session_id.clone(),
                            task.task_id.clone(),
                            0,
                            stream_signals,
                        );
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
                rows="14"
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
