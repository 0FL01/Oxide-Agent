use crate::auth::AuthContext;
use crate::tasks::composer::{
    PendingAttachmentFile, PendingAttachmentList, append_pending_browser_files, browser_files,
    browser_files_from_input_event, can_submit_input, handle_composer_drag, handle_composer_drop,
    handle_composer_input, handle_composer_paste, reset_composer_textarea_height,
    submit_parent_form_on_ctrl_enter, task_input_limit_notice, task_input_too_long,
};
use crate::utils::spawn_ui;
use leptos::{html, prelude::*};
use oxide_agent_web_contracts::{ApiLifeSubmitRequest, ApiLifeSubmitResponse, TaskAttachment};

/// Life chat composer — textarea, attachments, submit.
///
/// Simpler than the task composer: no session_id, no profile/effort selects,
/// no resume flow. Submits to `POST /api/v1/life/inputs`.
#[component]
pub(crate) fn LifeComposer(
    auth: AuthContext,
    input: ReadSignal<String>,
    set_input: WriteSignal<String>,
    pending_files: ReadSignal<Vec<PendingAttachmentFile>>,
    set_pending_files: WriteSignal<Vec<PendingAttachmentFile>>,
    next_pending_file_id: ReadSignal<usize>,
    set_next_pending_file_id: WriteSignal<usize>,
    loading: ReadSignal<bool>,
    is_running: Signal<bool>,
    set_loading: WriteSignal<bool>,
    set_error: WriteSignal<Option<String>>,
    on_submitted: Callback<ApiLifeSubmitResponse>,
) -> impl IntoView {
    let textarea_ref = NodeRef::<html::Textarea>::new();
    let (drag_active, set_drag_active) = signal(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let text = input.get();
        let files = pending_files.get();
        if !can_submit_input(&text, &files) {
            return;
        }
        let auth_state = auth.auth.get();
        let max_chars = auth_state.max_task_input_chars;
        if let Some(message) = task_input_limit_notice(&text, max_chars) {
            set_error.set(Some(message));
            return;
        }
        set_loading.set(true);
        set_error.set(None);

        spawn_ui(async move {
            let client = auth.client();

            // Upload attachments via the life sandbox endpoint.
            let attachments: Vec<TaskAttachment> = if files.is_empty() {
                Vec::new()
            } else {
                match client.upload_life_attachments(&browser_files(&files)).await {
                    Ok(response) => response.attachments,
                    Err(error) => {
                        set_error.set(Some(error.to_string()));
                        set_loading.set(false);
                        return;
                    }
                }
            };

            let request = ApiLifeSubmitRequest {
                content: text,
                attachments,
                metadata: serde_json::json!({}),
                sensitivity: oxide_agent_web_contracts::ApiLifeInputSensitivity::Normal,
            };

            match client.submit_life_input(&request).await {
                Ok(response) => {
                    set_input.set(String::new());
                    reset_composer_textarea_height(textarea_ref);
                    set_pending_files.set(Vec::new());
                    on_submitted.run(response);
                }
                Err(error) => {
                    set_error.set(Some(error.to_string()));
                }
            }
            set_loading.set(false);
        });
    };

    view! {
        <form class="composer life-composer" on:submit=submit>
            <div
                class="composer-inner"
                class:drag-active=drag_active
                on:dragenter=move |ev| {
                    handle_composer_drag(&ev, set_drag_active, true);
                }
                on:dragover=move |ev| {
                    handle_composer_drag(&ev, set_drag_active, true);
                }
                on:dragleave=move |ev| {
                    handle_composer_drag(&ev, set_drag_active, false);
                }
                on:drop=move |ev| {
                    handle_composer_drop(
                        &ev,
                        set_drag_active,
                        next_pending_file_id,
                        set_next_pending_file_id,
                        set_pending_files,
                    );
                }
            >
                <textarea
                    node_ref=textarea_ref
                    placeholder=move || if is_running.get() { "Agent is working…" } else { "Message your agent…" }
                    prop:value=input
                    disabled=move || loading.get() || is_running.get()
                    on:input=move |ev| {
                        handle_composer_input(&ev, set_input);
                    }
                    on:paste=move |ev| {
                        let auth_state = auth.auth.get();
                        handle_composer_paste(
                            &ev,
                            &input.get(),
                            auth_state.max_task_input_chars,
                            auth_state.large_input_attachments_supported,
                            next_pending_file_id,
                            set_next_pending_file_id,
                            set_pending_files,
                        );
                    }
                    on:keydown=move |ev| {
                        submit_parent_form_on_ctrl_enter(&ev);
                    }
                />
                <PendingAttachmentList
                    attachments=pending_files
                    set_attachments=set_pending_files
                />
                {move || {
                    let auth_state = auth.auth.get();
                    task_input_limit_notice(&input.get(), auth_state.max_task_input_chars)
                        .map(|message| {
                            view! {
                                <p class="composer-validation" class:error=true>{message}</p>
                            }
                        })
                }}
                <div class="composer-footer">
                    <div class="composer-actions" class:btn-hidden=move || {
                        let auth_state = auth.auth.get();
                        let input_blocked = task_input_too_long(&input.get(), auth_state.max_task_input_chars)
                            && !auth_state.large_input_attachments_supported;
                        input_blocked || (!can_submit_input(&input.get(), &pending_files.get()) && !is_running.get())
                    }>
                        <label class="button secondary composer-attach-button">
                            <input
                                class="composer-file-input"
                                type="file"
                                multiple
                                disabled=move || loading.get() || is_running.get()
                                on:change=move |ev| {
                                    append_pending_browser_files(
                                        next_pending_file_id,
                                        set_next_pending_file_id,
                                        set_pending_files,
                                        browser_files_from_input_event(&ev),
                                    );
                                }
                            />
                            "Attach"
                        </label>
                        <button
                            type="submit"
                            disabled=move || {
                                let auth_state = auth.auth.get();
                                let input_blocked = task_input_too_long(&input.get(), auth_state.max_task_input_chars)
                                    && !auth_state.large_input_attachments_supported;
                                loading.get() || is_running.get() || input_blocked || !can_submit_input(&input.get(), &pending_files.get())
                            }
                            class="btn-primary"
                        >
                            "Send"
                        </button>
                    </div>
                </div>
            </div>
        </form>
    }
}
