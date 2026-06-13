use crate::auth::{AuthContext, DEFAULT_MAX_TASK_INPUT_CHARS};
use crate::utils::spawn_ui;
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    AgentEffort, AgentProfileView, TaskAttachment, UpdateUserSettingsRequest,
};

use super::profile::{
    PROFILE_VALUE_DEFAULT, PROFILE_VALUE_NONE, agent_effort_value, missing_profile_option_label,
};

pub(super) const MAX_TASK_INPUT_CHARS: usize = DEFAULT_MAX_TASK_INPUT_CHARS;

#[derive(Clone)]
pub(super) struct PendingAttachmentFile {
    id: usize,
    file: web_sys::File,
}

#[component]
pub(super) fn AgentProfileSelect(
    profiles: ReadSignal<Vec<AgentProfileView>>,
    selected_profile: ReadSignal<String>,
    disabled: Signal<bool>,
    include_default: bool,
    on_change: Callback<leptos::ev::Event>,
) -> impl IntoView {
    view! {
        <select
            class="agent-profile-select"
            prop:value=selected_profile
            disabled=move || disabled.get()
            on:change=move |ev| on_change.run(ev)
        >
            {include_default.then(|| view! {
                <option value=PROFILE_VALUE_DEFAULT>"Default profile"</option>
            })}
            {move || selected_profile_missing_option(profiles, selected_profile)}
            <option value=PROFILE_VALUE_NONE>"No profile"</option>
            <For
                each=move || profiles.get()
                key=|profile| profile.agent_id.clone()
                children=move |profile| {
                    let value = profile.agent_id.clone();
                    view! { <option value=value.clone()>{profile.display_name}</option> }
                }
            />
        </select>
    }
}

#[component]
pub(super) fn AgentEffortSelect(
    selected_effort: ReadSignal<AgentEffort>,
    disabled: Signal<bool>,
    on_change: Callback<leptos::ev::Event>,
) -> impl IntoView {
    view! {
        <select
            class="composer-effort-select"
            title="Effort controls agent loop depth and research budget"
            aria-label="Agent effort"
            prop:value=move || agent_effort_value(selected_effort.get())
            disabled=move || disabled.get()
            on:change=move |ev| on_change.run(ev)
        >
            <option value="standard">"Standard"</option>
            <option value="extended">"Extended"</option>
            <option value="heavy">"Heavy"</option>
        </select>
    }
}

pub(super) fn persist_default_effort(
    auth: AuthContext,
    effort: AgentEffort,
    set_error: WriteSignal<Option<String>>,
) {
    spawn_ui(async move {
        let client = auth.client();
        let settings = match client.settings().await {
            Ok(settings) => settings,
            Err(error) => {
                set_error.set(Some(error.to_string()));
                return;
            }
        };
        let request = UpdateUserSettingsRequest {
            default_model_selection: settings.default_model_selection,
            default_agent_profile_id: settings.default_agent_profile_id,
            default_effort: Some(effort),
        };
        if let Err(error) = client.update_settings(&request).await {
            set_error.set(Some(error.to_string()));
        }
    });
}

fn selected_profile_missing_option(
    profiles: ReadSignal<Vec<AgentProfileView>>,
    selected_profile: ReadSignal<String>,
) -> Option<impl IntoView> {
    let selected = selected_profile.get();
    let label = missing_profile_option_label(&profiles.get(), &selected)?;
    Some(view! {
        <option value=selected.clone()>{label}</option>
    })
}

#[component]
pub(super) fn PendingAttachmentList(
    attachments: ReadSignal<Vec<PendingAttachmentFile>>,
    set_attachments: WriteSignal<Vec<PendingAttachmentFile>>,
) -> impl IntoView {
    view! {
        {move || {
            let items = attachments.get();
            if items.is_empty() {
                ().into_any()
            } else {
                view! {
                    <ul class="pending-attachments" aria-label="Pending attachments">
                        {items
                            .into_iter()
                            .map(|attachment| {
                                let attachment_id = attachment.id;
                                let file_name = attachment.file.name();
                                let meta = format_attachment_meta(
                                    attachment.file.size() as u64,
                                    attachment.file.type_(),
                                );
                                view! {
                                    <li class="pending-attachment-item">
                                        <div class="pending-attachment-copy">
                                            <span class="pending-attachment-name">{file_name}</span>
                                            <span class="pending-attachment-meta">{meta}</span>
                                        </div>
                                        <button
                                            class="message-action-button"
                                            type="button"
                                            title="Remove attachment"
                                            aria-label="Remove attachment"
                                            on:click=move |_| {
                                                set_attachments
                                                    .update(|items| items.retain(|item| item.id != attachment_id));
                                            }
                                        >
                                            "✕"
                                        </button>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                }
                .into_any()
            }
        }}
    }
}

#[component]
pub(super) fn MessageAttachments(attachments: Vec<TaskAttachment>) -> impl IntoView {
    if attachments.is_empty() {
        return ().into_any();
    }

    view! {
        <ul class="message-attachments" aria-label="Message attachments">
            {attachments
                .into_iter()
                .map(|attachment| {
                    let meta = format_attachment_meta(
                        attachment.size_bytes,
                        attachment.mime_type.clone().unwrap_or_default(),
                    );
                    let sandbox_path = attachment.sandbox_path.clone();
                    let sandbox_title = sandbox_path.clone();
                    view! {
                        <li class="message-attachment-item" title=sandbox_title>
                            <div class="message-attachment-copy">
                                <span class="message-attachment-name">{attachment.file_name}</span>
                                <span class="message-attachment-meta">{meta}</span>
                            </div>
                            <code class="message-attachment-path">{sandbox_path}</code>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
    .into_any()
}

pub(super) fn can_submit_input(input: &str, attachments: &[PendingAttachmentFile]) -> bool {
    !input.trim().is_empty() || !attachments.is_empty()
}

pub(super) fn task_input_char_count(input: &str) -> usize {
    input.chars().count()
}

pub(super) fn task_input_too_long(input: &str, max_chars: usize) -> bool {
    task_input_char_count(input) > max_chars
}

pub(super) fn task_input_limit_notice(
    input: &str,
    max_chars: usize,
    large_input_attachments_supported: bool,
) -> Option<(String, bool)> {
    let count = task_input_char_count(input);
    if count <= max_chars {
        return None;
    }

    if large_input_attachments_supported {
        Some((
            format!(
                "Message is large ({count}/{max_chars} characters) and will be uploaded as a sandbox attachment."
            ),
            false,
        ))
    } else {
        Some((
            format!(
                "Message is too large ({count}/{max_chars} characters). Sandbox attachments are not available."
            ),
            true,
        ))
    }
}

pub(super) fn handle_composer_drag(
    ev: &leptos::ev::DragEvent,
    set_drag_active: WriteSignal<bool>,
    active: bool,
) {
    ev.prevent_default();
    set_drag_active.set(active);
}

pub(super) fn handle_composer_drop(
    ev: &leptos::ev::DragEvent,
    set_drag_active: WriteSignal<bool>,
    next_id: ReadSignal<usize>,
    set_next_id: WriteSignal<usize>,
    set_attachments: WriteSignal<Vec<PendingAttachmentFile>>,
) {
    ev.prevent_default();
    set_drag_active.set(false);
    append_pending_browser_files(
        next_id,
        set_next_id,
        set_attachments,
        browser_files_from_drag_event(ev),
    );
}

pub(super) fn handle_composer_input(ev: &leptos::ev::Event, set_input: WriteSignal<String>) {
    set_input.set(event_target_value(ev));
    resize_textarea_from_input_event(ev);
}

pub(super) fn reset_composer_textarea_height(textarea_ref: NodeRef<html::Textarea>) {
    if let Some(textarea) = textarea_ref.get() {
        textarea.style().remove_property("height").ok();
    }
}

pub(super) fn handle_composer_paste(
    ev: &leptos::ev::ClipboardEvent,
    next_id: ReadSignal<usize>,
    set_next_id: WriteSignal<usize>,
    set_attachments: WriteSignal<Vec<PendingAttachmentFile>>,
) {
    append_pasted_image_files(ev, next_id, set_next_id, set_attachments);
}

pub(super) fn submit_parent_form_on_ctrl_enter(ev: &leptos::ev::KeyboardEvent) {
    if !ev.ctrl_key() || ev.key() != "Enter" {
        return;
    }
    ev.prevent_default();

    let Some(target) = ev.target() else {
        return;
    };
    use wasm_bindgen::JsCast;
    let el: web_sys::HtmlElement = target.unchecked_into();
    if let Ok(Some(form_el)) = el.closest("form")
        && let Ok(Some(btn)) = form_el.query_selector("button[type=submit]")
    {
        let btn: web_sys::HtmlElement = btn.unchecked_into();
        btn.click();
    }
}

fn resize_textarea_from_input_event(ev: &leptos::ev::Event) {
    const MAX_TEXTAREA_HEIGHT_PX: f64 = 208.0;

    let Some(target) = ev.target() else {
        return;
    };
    use wasm_bindgen::JsCast;
    let el: web_sys::HtmlElement = target.unchecked_into();
    el.style().set_property("height", "auto").ok();
    let scroll = el.scroll_height();
    let height = (scroll as f64).min(MAX_TEXTAREA_HEIGHT_PX);
    el.style()
        .set_property("height", &format!("{height}px"))
        .ok();
}

pub(super) fn append_pending_browser_files(
    next_id: ReadSignal<usize>,
    set_next_id: WriteSignal<usize>,
    set_attachments: WriteSignal<Vec<PendingAttachmentFile>>,
    files: Vec<web_sys::File>,
) {
    if files.is_empty() {
        return;
    }
    let start_id = next_id.get_untracked();
    let new_files = into_pending_attachment_files(files, start_id);
    set_next_id.set(start_id + new_files.len());
    set_attachments.update(|items| items.extend(new_files));
}

fn append_pasted_image_files(
    ev: &leptos::ev::ClipboardEvent,
    next_id: ReadSignal<usize>,
    set_next_id: WriteSignal<usize>,
    set_attachments: WriteSignal<Vec<PendingAttachmentFile>>,
) {
    append_pending_browser_files(
        next_id,
        set_next_id,
        set_attachments,
        browser_image_files_from_clipboard_event(ev),
    );
}

fn into_pending_attachment_files(
    files: Vec<web_sys::File>,
    start_id: usize,
) -> Vec<PendingAttachmentFile> {
    files
        .into_iter()
        .enumerate()
        .map(|(offset, file)| PendingAttachmentFile {
            id: start_id + offset,
            file,
        })
        .collect()
}

pub(super) fn browser_files(attachments: &[PendingAttachmentFile]) -> Vec<web_sys::File> {
    attachments
        .iter()
        .map(|attachment| attachment.file.clone())
        .collect()
}

pub(super) fn browser_files_from_input_event(ev: &leptos::ev::Event) -> Vec<web_sys::File> {
    use wasm_bindgen::JsCast;

    let Some(target) = ev.target() else {
        return Vec::new();
    };
    let Ok(input) = target.dyn_into::<web_sys::HtmlInputElement>() else {
        return Vec::new();
    };
    let files = input
        .files()
        .map(browser_files_from_file_list)
        .unwrap_or_default();
    input.set_value("");
    files
}

fn browser_files_from_drag_event(ev: &leptos::ev::DragEvent) -> Vec<web_sys::File> {
    ev.data_transfer()
        .and_then(|transfer| transfer.files())
        .map(browser_files_from_file_list)
        .unwrap_or_default()
}

fn browser_image_files_from_clipboard_event(ev: &leptos::ev::ClipboardEvent) -> Vec<web_sys::File> {
    ev.clipboard_data()
        .and_then(|transfer| transfer.files())
        .map(browser_image_files_from_file_list)
        .unwrap_or_default()
}

fn browser_files_from_file_list(file_list: web_sys::FileList) -> Vec<web_sys::File> {
    (0..file_list.length())
        .filter_map(|index| file_list.item(index))
        .collect()
}

fn browser_image_files_from_file_list(file_list: web_sys::FileList) -> Vec<web_sys::File> {
    browser_files_from_file_list(file_list)
        .into_iter()
        .filter(|file| is_image_file_metadata(&file.type_(), &file.name()))
        .collect()
}

fn is_image_file_metadata(mime_type: &str, file_name: &str) -> bool {
    let mime_type = mime_type.trim().to_ascii_lowercase();
    if mime_type.starts_with("image/") {
        return true;
    }

    let file_name = file_name.trim().to_ascii_lowercase();
    [
        ".avif", ".bmp", ".gif", ".heic", ".heif", ".jpeg", ".jpg", ".png", ".svg", ".tif",
        ".tiff", ".webp",
    ]
    .iter()
    .any(|extension| file_name.ends_with(extension))
}

pub(super) fn format_attachment_meta(size_bytes: u64, mime_type: String) -> String {
    let size = format_file_size(size_bytes);
    let mime = mime_type.trim();
    if mime.is_empty() {
        size
    } else {
        format!("{size} • {mime}")
    }
}

fn format_file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;

    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
