use crate::auth::use_auth;
use crate::markdown::MarkdownContent;
use crate::utils::spawn_ui;
use leptos::{html, prelude::*};
use oxide_agent_web_contracts::{
    CreateTaskVersionRequest, PersistedTaskEvent, TaskAttachment, TaskEventKind, TaskStatus,
    TaskSummary, UserMessageEventPayload,
};
use std::collections::HashMap;

use super::composer::MessageAttachments;
use super::delivered_files::{
    delivered_files_for_task, linkify_delivered_files_in_markdown, DeliveredFilesMessage,
};
use super::state::{summary_to_detail, upsert_task_summary};
use super::streaming::{start_task_stream, StreamUiSignals};
use super::versions::selected_version_index;
use super::{thought_label, ThinkingButton};

// ── Task Card ────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
#[component]
pub(super) fn TaskCard(
    session_id: String,
    versions: Vec<TaskSummary>,
    events: ReadSignal<Vec<PersistedTaskEvent>>,
    editable_task_id: Option<String>,
    selected_versions: ReadSignal<HashMap<String, String>>,
    set_selected_versions: WriteSignal<HashMap<String, String>>,
    drawer_open: ReadSignal<bool>,
    set_drawer_open: WriteSignal<bool>,
    stream_signals: StreamUiSignals,
    set_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    let version_group_id = versions
        .first()
        .map(|task| task.effective_version_group_id().to_string())
        .unwrap_or_default();
    let version_count = versions.len();
    let (editing, set_editing) = signal(false);
    let (draft, set_draft) = signal(
        versions
            .last()
            .map(|task| task.input_markdown.clone())
            .unwrap_or_default(),
    );
    let (saving, set_saving) = signal(false);
    let selected_task = Memo::new({
        let versions = versions.clone();
        let version_group_id = version_group_id.clone();
        move |_| {
            let selected_task_id = selected_versions.get().get(&version_group_id).cloned();
            let selected_index = selected_version_index(&versions, selected_task_id.as_deref());
            versions[selected_index].clone()
        }
    });

    Effect::new(move |_| {
        let task = selected_task.get();
        if !editing.get() {
            set_draft.set(task.input_markdown.clone());
        }
    });

    view! {
        {move || {
            let task = selected_task.get();
            let selected_index = versions
                .iter()
                .position(|candidate| candidate.task_id == task.task_id)
                .unwrap_or_else(|| version_count.saturating_sub(1));
            let editable = editable_task_id.as_ref() == Some(&task.task_id);
            let card_status_class = match task.status {
                TaskStatus::Running | TaskStatus::Queued => "running",
                TaskStatus::Completed => "success",
                TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Interrupted => "error",
                _ => "",
            };
            let card_class = format!("task-card {card_status_class}");
            let thought_label = thought_label(&task);
            let original_input = task.input_markdown.clone();
            let input_markdown = task.input_markdown.clone();
            let attachments = task.attachments.clone();
            let final_response_markdown = task.final_response_markdown.clone();
            let task_events = events.get();
            let resume_messages = resume_user_messages_for_task(&task_events, &task.task_id);
            let delivered_files = delivered_files_for_task(&task_events, &task.task_id);
            let can_select_previous = selected_index > 0;
            let can_select_next = selected_index + 1 < version_count;
            let version_counter = format!("{}/{}", selected_index + 1, version_count);
            let previous_task = can_select_previous.then(|| versions[selected_index - 1].clone());
            let next_task = can_select_next.then(|| versions[selected_index + 1].clone());

            view! {
                <article class=card_class>
                    <div class="message user-message-wrap">
                        <div class="user-message">
                        {if editing.get() {
                            let session_id = session_id.clone();
                            let task_id = task.task_id.clone();
                            let version_group_id = version_group_id.clone();
                            view! {
                                <TaskInputEditForm
                                    session_id=session_id
                                    task_id=task_id
                                    version_group_id=version_group_id
                                    original_input=original_input
                                    attachments=attachments.clone()
                                    draft=draft
                                    set_draft=set_draft
                                    saving=saving
                                    set_saving=set_saving
                                    set_editing=set_editing
                                    set_selected_versions=set_selected_versions
                                    set_drawer_open=set_drawer_open
                                    stream_signals=stream_signals
                                    set_error=set_error
                                />
                            }.into_any()
                        } else {
                            view! {
                                <UserMessageBody
                                    input_markdown=input_markdown.clone()
                                    attachments=attachments.clone()
                                />
                            }.into_any()
                        }}
                        </div>
                        <div class="user-message-actions" aria-label="User message actions">
                            {editable.then(|| view! {
                                <button
                                    class="message-action-button"
                                    type="button"
                                    title="Edit input"
                                    aria-label="Edit input"
                                    on:click=move |_| set_editing.set(true)
                                >
                                    "✎"
                                </button>
                            })}
                            <button
                                class="message-action-button"
                                type="button"
                                title="Copy user message"
                                aria-label="Copy user message"
                                on:click=move |_| {
                                    if let Some(window) = web_sys::window() {
                                        let _ = window.navigator().clipboard().write_text(&input_markdown);
                                    }
                                }
                            >
                                "⧉"
                            </button>
                            {(version_count > 1).then(|| {
                                let previous_version_group_id = version_group_id.clone();
                                let next_version_group_id = version_group_id.clone();
                                view! {
                                    <div class="message-version-switcher" aria-label="Task version history">
                                        <button
                                            class="message-action-button"
                                            type="button"
                                            title="Previous version"
                                            aria-label="Previous version"
                                            disabled=!can_select_previous
                                            on:click=move |_| {
                                                if let Some(previous_task) = previous_task.clone() {
                                                    set_editing.set(false);
                                                    set_draft.set(previous_task.input_markdown.clone());
                                                    set_selected_versions.update(|items| {
                                                        items.insert(previous_version_group_id.clone(), previous_task.task_id.clone());
                                                    });
                                                }
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
                                                if let Some(next_task) = next_task.clone() {
                                                    set_editing.set(false);
                                                    set_draft.set(next_task.input_markdown.clone());
                                                    set_selected_versions.update(|items| {
                                                        items.insert(next_version_group_id.clone(), next_task.task_id.clone());
                                                    });
                                                }
                                            }
                                        >
                                            "›"
                                        </button>
                                    </div>
                                }
                            })}
                        </div>
                    </div>
                    {resume_messages
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
                        .collect_view()}
                    {editable.then(|| view! {
                        <div class="task-action-row">
                            <ThinkingButton label=thought_label open=drawer_open set_open=set_drawer_open />
                        </div>
                    })}

                    {final_response_markdown.map(|answer| {
                        let raw_answer = answer.clone();
                        let rendered_answer =
                            linkify_delivered_files_in_markdown(&answer, &delivered_files);
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
                                        on:click=move |_| {
                                            if let Some(window) = web_sys::window() {
                                                let _ = window.navigator().clipboard().write_text(&raw_answer);
                                            }
                                        }
                                    >
                                        "⧉"
                                    </button>
                                </div>
                            </div>
                        }
                    })}
                    {(!delivered_files.is_empty()).then(|| view! {
                        <DeliveredFilesMessage files=delivered_files.clone() />
                    })}
                    {task.error_message.map(|error| view! {
                        <div class="message error-message">{error}</div>
                    })}
                    {task.pending_user_input.map(|pending| view! {
                        <div class="message pending-message">{pending.prompt}</div>
                    })}
                </article>
            }
                .into_any()
        }}
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

#[allow(clippy::too_many_arguments)]
#[component]
fn TaskInputEditForm(
    session_id: String,
    task_id: String,
    version_group_id: String,
    original_input: String,
    attachments: Vec<TaskAttachment>,
    draft: ReadSignal<String>,
    set_draft: WriteSignal<String>,
    saving: ReadSignal<bool>,
    set_saving: WriteSignal<bool>,
    set_editing: WriteSignal<bool>,
    set_selected_versions: WriteSignal<HashMap<String, String>>,
    set_drawer_open: WriteSignal<bool>,
    stream_signals: StreamUiSignals,
    set_error: WriteSignal<Option<String>>,
) -> impl IntoView {
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
                effort: None,
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
                        stream_signals.set_events.set(Vec::new());
                        stream_signals.set_progress.set(None);
                        stream_signals
                            .set_active_task
                            .set(Some(summary_to_detail(&session_id, &task)));
                        stream_signals.set_last_terminal_status.set(None);
                        stream_signals
                            .set_tasks
                            .update(|items| upsert_task_summary(items, task.clone()));
                        set_selected_versions.update(|items| {
                            items.insert(version_group_id.clone(), task.task_id.clone());
                        });
                        set_drawer_open.set(false);
                        start_task_stream(
                            client,
                            session_id.clone(),
                            task.task_id.clone(),
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
