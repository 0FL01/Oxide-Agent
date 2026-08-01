use crate::api::{ApiClient, ApiClientError};
use crate::auth::use_auth;
use crate::components::ErrorBanner;
use crate::utils::{navigate, spawn_ui};
use crate::voice::VoiceRecorderControl;
use futures_util::join;
use leptos::{html, prelude::*};
use oxide_agent_web_contracts::{
    AgentProfileView, CreateSessionRequest, CreateTaskRequest, ErrorCode, ModelRouteView,
    ModelSelection, PersistedTaskEvent, ProgressSnapshot, ResumeTaskRequest, SessionSummary,
    TaskAttachment, TaskDetail, TaskEventsResponse, TaskStatus, TaskSummary,
    UpdateSessionModelRequest, UpdateSessionProfileRequest, UpdateUserSettingsRequest,
    UserSettingsResponse,
};
use std::{cell::RefCell, cmp::Ordering, collections::HashMap, time::Duration};

use super::WEB_AGENT_EFFORT;
use super::activity::{ActivityDrawer, ActivityStatusChip};
use super::composer::{
    AgentProfileSelect, ModelRouteSelect, PendingAttachmentFile, PendingAttachmentList,
    append_pending_browser_files, browser_files, browser_files_from_input_event, can_submit_input,
    handle_composer_drag, handle_composer_drop, handle_composer_input, handle_composer_paste,
    merge_voice_transcript, reset_composer_textarea_height, submit_parent_form_on_ctrl_enter,
    task_input_limit_notice, task_input_too_long,
};
use super::lightbox::{Lightbox, LightboxContext, LightboxImage};
use super::profile::{
    PROFILE_VALUE_DEFAULT, PROFILE_VALUE_NONE, agent_profile_selection_from_value,
    profile_value_to_id,
};
use super::state::{
    browser_now_millis, latest_editable_task_id, latest_task, remove_session_summary,
    summary_to_detail, upsert_session_summary, upsert_task_summary,
};
use super::streaming::{StreamUiSignals, start_task_stream};
use super::task_card::{TaskCard, TaskCardModel, TaskCardSignals};
use super::versions::{group_task_versions, selected_visible_activity_task_ids};

const TASK_EVENTS_INITIAL_LIMIT: usize = 100;
const TASK_EVENTS_OLDER_LIMIT: usize = 500;
const TASKS_PAGE_LIMIT: usize = 20;
const SETTINGS_PROFILES_CACHE_TTL_MS: f64 = 30_000.0;

#[derive(Clone, Copy, Default)]
struct ActivityPageState {
    before_seq: u64,
    has_more: bool,
    loading: bool,
}

#[derive(Clone)]
struct SettingsProfilesCacheEntry {
    loaded_at_ms: f64,
    settings: UserSettingsResponse,
    profiles: Vec<AgentProfileView>,
}

thread_local! {
    static SETTINGS_PROFILES_CACHE: RefCell<Option<SettingsProfilesCacheEntry>> = const { RefCell::new(None) };
}

fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map(|performance| performance.now())
        .unwrap_or_default()
}

fn cached_settings_profiles() -> Option<SettingsProfilesCacheEntry> {
    let now = now_ms();
    SETTINGS_PROFILES_CACHE.with(|cache| {
        cache
            .borrow()
            .as_ref()
            .filter(|entry| now - entry.loaded_at_ms <= SETTINGS_PROFILES_CACHE_TTL_MS)
            .cloned()
    })
}

fn update_cached_settings(settings: UserSettingsResponse) {
    SETTINGS_PROFILES_CACHE.with(|cache| {
        if let Some(entry) = cache.borrow_mut().as_mut() {
            entry.loaded_at_ms = now_ms();
            entry.settings = settings;
        }
    });
}

async fn load_settings_profiles(
    client: &ApiClient,
) -> (Option<UserSettingsResponse>, Option<Vec<AgentProfileView>>) {
    if let Some(entry) = cached_settings_profiles() {
        return (Some(entry.settings), Some(entry.profiles));
    }

    let (settings_result, profiles_result) = join!(client.settings(), client.list_agent_profiles());
    if let (Ok(settings), Ok(profiles)) = (&settings_result, &profiles_result) {
        SETTINGS_PROFILES_CACHE.with(|cache| {
            *cache.borrow_mut() = Some(SettingsProfilesCacheEntry {
                loaded_at_ms: now_ms(),
                settings: settings.clone(),
                profiles: profiles.profiles.clone(),
            });
        });
    }

    (
        settings_result.ok(),
        profiles_result.ok().map(|response| response.profiles),
    )
}

async fn load_latest_task_events(
    client: &ApiClient,
    session_id: &str,
    task_id: &str,
    last_event_seq: u64,
) -> Result<TaskEventsResponse, ApiClientError> {
    client
        .task_events_before_page(
            session_id,
            task_id,
            last_event_seq.saturating_add(1),
            TASK_EVENTS_INITIAL_LIMIT,
        )
        .await
}

async fn prepare_task_input(
    client: &ApiClient,
    session_id: &str,
    text: String,
    files: &[PendingAttachmentFile],
    max_task_input_chars: usize,
) -> Result<(String, Vec<TaskAttachment>), String> {
    // A message exceeding the input limit is a hard error: large pastes are
    // diverted to attachments at paste time, so a too-long textarea means the
    // diversion did not happen (manual entry or sandbox unavailable) and the
    // content cannot be staged without losing the user's preamble.
    if task_input_too_long(&text, max_task_input_chars) {
        return Err(format!(
            "Message is too large ({} / {max_task_input_chars} characters). Large pastes are attached as files; shorten the message or attach the content.",
            text.chars().count()
        ));
    }

    let attachments = if files.is_empty() {
        Vec::new()
    } else {
        client
            .upload_task_attachments(session_id, &browser_files(files))
            .await
            .map_err(|error| error.to_string())?
            .attachments
    };

    Ok((text, attachments))
}

fn merge_task_events(
    set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    new_events: Vec<PersistedTaskEvent>,
) {
    set_events.update(|items| {
        let mut needs_sort = false;
        for event in new_events {
            if !items
                .iter()
                .any(|item| item.task_id == event.task_id && item.seq == event.seq)
            {
                needs_sort |= items
                    .last()
                    .is_some_and(|last| compare_task_events(last, &event) == Ordering::Greater);
                items.push(event);
            }
        }
        if needs_sort {
            items.sort_by(compare_task_events);
        }
    });
}

fn compare_task_events(a: &PersistedTaskEvent, b: &PersistedTaskEvent) -> Ordering {
    a.created_at
        .cmp(&b.created_at)
        .then_with(|| a.task_id.cmp(&b.task_id))
        .then_with(|| a.seq.cmp(&b.seq))
}

fn max_event_seq(events: &[PersistedTaskEvent]) -> u64 {
    events
        .iter()
        .map(|event| event.seq)
        .max()
        .unwrap_or_default()
}

fn merge_task_summaries(items: &mut Vec<TaskSummary>, tasks: Vec<TaskSummary>) {
    for task in tasks {
        upsert_task_summary(items, task);
    }
}

#[component]
pub fn TaskConsole(
    session_id: Memo<Option<String>>,
    events: ReadSignal<Vec<PersistedTaskEvent>>,
    progress: ReadSignal<Option<ProgressSnapshot>>,
    set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    set_progress: WriteSignal<Option<ProgressSnapshot>>,
    set_sessions: WriteSignal<Vec<SessionSummary>>,
) -> impl IntoView {
    view! {
        <Workspace
            session_id=session_id
            events=events
            progress=progress
            set_events=set_events
            set_progress=set_progress
            set_sessions=set_sessions
        />
    }
}

/// Unified workspace that handles both welcome mode (no session selected)
/// and chat mode (session with tasks).  The `session_id` Memo drives
/// `load_all` reactively — switching from `None` to `Some(id)` or between
/// different session IDs re-fetches without recreating the component.
#[component]
fn Workspace(
    session_id: Memo<Option<String>>,
    events: ReadSignal<Vec<PersistedTaskEvent>>,
    progress: ReadSignal<Option<ProgressSnapshot>>,
    set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    set_progress: WriteSignal<Option<ProgressSnapshot>>,
    set_sessions: WriteSignal<Vec<SessionSummary>>,
) -> impl IntoView {
    let auth = use_auth();
    let (tasks, set_tasks) = signal(Vec::<TaskSummary>::new());
    let (tasks_has_more, set_tasks_has_more) = signal(false);
    let (tasks_next_offset, set_tasks_next_offset) = signal(0_usize);
    let (loading_older_tasks, set_loading_older_tasks) = signal(false);
    let (activity_pages, set_activity_pages) = signal(HashMap::<String, ActivityPageState>::new());
    let (input, set_input) = signal(String::new());
    let (error, set_error) = signal(None::<String>);
    let (loading, set_loading) = signal(false);
    let (active_task, set_active_task) = signal(None::<TaskDetail>);
    let (streaming_task_id, set_streaming_task_id) = signal(None::<String>);
    let (selected_versions, set_selected_versions) = signal(HashMap::<String, String>::new());
    let (pending_files, set_pending_files) = signal(Vec::<PendingAttachmentFile>::new());
    let (next_pending_file_id, set_next_pending_file_id) = signal(0_usize);
    let (voice_busy, set_voice_busy) = signal(false);
    let (drag_active, set_drag_active) = signal(false);
    let (profiles, set_profiles) = signal(Vec::<AgentProfileView>::new());
    let (profiles_loaded, set_profiles_loaded) = signal(false);
    let (selected_profile, set_selected_profile) = signal(PROFILE_VALUE_DEFAULT.to_string());
    let (model_routes, set_model_routes) = signal(Vec::<ModelRouteView>::new());
    let (selected_model, set_selected_model) = signal(String::new());
    let (welcome_model, set_welcome_model) = signal(String::new());
    let (model_touched, set_model_touched) = signal(false);
    let (model_updating, set_model_updating) = signal(false);
    let textarea_ref = NodeRef::<html::Textarea>::new();

    let (drawer_open, set_drawer_open) = signal(false);
    let (activity_task_id, set_activity_task_id) = signal(None::<String>);

    // Lightbox overlay state — session-scoped, provided via context so any
    // child component (e.g. BrowserToolCard) can open a full-screen image.
    let (lightbox_image, set_lightbox_image) = signal(None::<LightboxImage>);
    provide_context(LightboxContext {
        image: lightbox_image,
        set_image: set_lightbox_image,
    });

    // Shared wall-clock for all elapsed timers (task-card "Thinking for…"
    // label and the Activity drawer). A single 1s interval drives every
    // active-task timer so the UI ticks from the browser clock, not from
    // SSE/DB `updated_at` updates that can stall between events.
    let (elapsed_now_millis, set_elapsed_now_millis) =
        signal(browser_now_millis().unwrap_or_default());
    if let Ok(handle) = set_interval_with_handle(
        move || {
            let next = browser_now_millis()
                .unwrap_or_else(|| elapsed_now_millis.get_untracked().saturating_add(1_000));
            set_elapsed_now_millis.set(next);
        },
        Duration::from_secs(1),
    ) {
        on_cleanup(move || handle.clear());
    }

    // Load settings + profiles once on mount.
    Effect::new(move |_| {
        if profiles_loaded.get() {
            return;
        }
        set_profiles_loaded.set(true);
        spawn_ui(async move {
            let client = auth.client();
            let ((settings, loaded_profiles), routes_result) =
                join!(load_settings_profiles(&client), client.list_model_routes());
            if let Some(loaded_profiles) = loaded_profiles {
                set_profiles.set(loaded_profiles);
            }
            match routes_result {
                Ok(response) => {
                    let initial_model = settings
                        .and_then(|settings| settings.default_model_selection)
                        .map(|selection| selection.qualified_id)
                        .or(response.default_model_id)
                        .unwrap_or_default();
                    set_model_routes.set(response.routes);
                    if !model_touched.get() {
                        set_welcome_model.set(initial_model.clone());
                        if session_id.get_untracked().is_none() {
                            set_selected_model.set(initial_model);
                        }
                    }
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
        });
    });

    // `load_all` — fetches session + tasks for the given session ID.
    // Discards stale results if the user has navigated to a different session
    // while the fetch was in flight.
    let load_all = move |sid: String| {
        set_loading.set(true);
        set_error.set(None);
        // Clear stale state before loading (but NOT tasks — pre-populated
        // data from a just-submitted first message should stay visible).
        set_events.set(Vec::new());
        set_progress.set(None);
        set_active_task.set(None);
        set_streaming_task_id.set(None);
        set_selected_versions.set(HashMap::new());
        set_activity_pages.set(HashMap::new());
        set_activity_task_id.set(None);
        set_drawer_open.set(false);
        set_lightbox_image.set(None);
        spawn_ui(async move {
            let client = auth.client();
            let (session_result, tasks_result) = join!(
                client.get_session(&sid),
                client.list_tasks_page(&sid, TASKS_PAGE_LIMIT, 0)
            );

            // Discard stale results if the user navigated to a different session.
            if session_id.get_untracked().as_deref() != Some(sid.as_str()) {
                return;
            }

            match session_result {
                Ok(response) => {
                    set_selected_model.set(
                        response
                            .session
                            .model_selection
                            .as_ref()
                            .map(|selection| selection.qualified_id.clone())
                            .unwrap_or_default(),
                    );
                    set_selected_profile.set(
                        response
                            .session
                            .agent_profile_id
                            .clone()
                            .unwrap_or_else(|| PROFILE_VALUE_NONE.to_string()),
                    );
                    upsert_session_summary(set_sessions, response.session);
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }

            match tasks_result {
                Ok(response) => {
                    set_drawer_open.set(false);
                    set_tasks_has_more.set(response.has_more);
                    set_tasks_next_offset.set(response.next_offset);
                    let latest = latest_task(&response.tasks);
                    set_tasks.set(response.tasks);
                    if let Some(task) = latest {
                        let task_id = task.task_id.clone();
                        let task_detail = summary_to_detail(&sid, &task);
                        let initial_last_seq = match load_latest_task_events(
                            &client,
                            &sid,
                            &task_id,
                            task.last_event_seq,
                        )
                        .await
                        {
                            Ok(response) => {
                                let last_seq = max_event_seq(&response.events);
                                set_activity_pages.update(|items| {
                                    items.insert(
                                        task_id.clone(),
                                        ActivityPageState {
                                            before_seq: response.first_seq,
                                            has_more: response.has_more,
                                            loading: false,
                                        },
                                    );
                                });
                                merge_task_events(set_events, response.events);
                                last_seq
                            }
                            Err(error) => {
                                set_error.set(Some(error.to_string()));
                                0
                            }
                        };
                        if matches!(task_detail.status, TaskStatus::Queued | TaskStatus::Running) {
                            set_active_task.set(Some(task_detail));
                            start_task_stream(
                                client.clone(),
                                sid.clone(),
                                task_id.clone(),
                                initial_last_seq,
                                StreamUiSignals {
                                    set_events,
                                    set_progress,
                                    set_active_task,
                                    set_tasks,
                                    set_error,
                                    streaming_task_id,
                                    set_streaming_task_id,
                                    set_sessions,
                                },
                            );
                        } else {
                            if task_detail.status == TaskStatus::WaitingForUserInput {
                                set_active_task.set(Some(task_detail));
                            } else {
                                set_active_task.set(None);
                            }
                            // Hydrate persisted progress for non-streamed tasks so the
                            // activity context card (Free/Flow/Prompt/Tools + health)
                            // renders after reload, not only while streaming.
                            if let Ok(response) = client.task_progress(&sid, &task_id).await {
                                set_progress.set(response.progress);
                            }
                        }
                    } else {
                        // Empty session — clear signals
                        set_events.set(Vec::new());
                        set_progress.set(None);
                        set_active_task.set(None);
                        set_activity_pages.set(HashMap::new());
                        set_activity_task_id.set(None);
                        set_drawer_open.set(false);
                        set_lightbox_image.set(None);
                    }
                }
                Err(error) => set_error.set(Some(task_submit_error_message(&error))),
            }
            set_loading.set(false);
        });
    };

    // Reactive load trigger — fires when `session_id` changes.
    // `None` → welcome mode (clear chat state); `Some(id)` → load_all.
    Effect::new(move |_| {
        let sid = session_id.get();
        if let Some(id) = sid {
            load_all(id);
        } else {
            // Welcome mode — clear all chat state
            set_tasks.set(Vec::new());
            set_events.set(Vec::new());
            set_progress.set(None);
            set_active_task.set(None);
            set_streaming_task_id.set(None);
            set_selected_versions.set(HashMap::new());
            set_activity_pages.set(HashMap::new());
            set_activity_task_id.set(None);
            set_drawer_open.set(false);
            set_lightbox_image.set(None);
            set_loading.set(false);
            // Reset profile to welcome default
            set_selected_profile.set(PROFILE_VALUE_DEFAULT.to_string());
            set_selected_model.set(welcome_model.get());
        }
    });

    let load_older_tasks = Callback::new(move |_| {
        let Some(sid) = session_id.get() else {
            return;
        };
        if loading_older_tasks.get_untracked() || !tasks_has_more.get_untracked() {
            return;
        }
        set_loading_older_tasks.set(true);
        set_error.set(None);
        let offset = tasks_next_offset.get_untracked();
        spawn_ui(async move {
            let client = auth.client();
            match client.list_tasks_page(&sid, TASKS_PAGE_LIMIT, offset).await {
                Ok(response) => {
                    set_tasks.update(|items| merge_task_summaries(items, response.tasks));
                    set_tasks_has_more.set(response.has_more);
                    set_tasks_next_offset.set(response.next_offset);
                }
                Err(error) => set_error.set(Some(task_submit_error_message(&error))),
            }
            set_loading_older_tasks.set(false);
        });
    });

    let load_older_activity = Callback::new(move |_| {
        let Some(sid) = session_id.get() else {
            return;
        };
        let Some(task_id) = activity_task_id.get_untracked() else {
            return;
        };
        let page_state = activity_pages
            .get_untracked()
            .get(&task_id)
            .copied()
            .unwrap_or_default();
        if page_state.loading || !page_state.has_more {
            return;
        }

        let before_seq = page_state.before_seq;
        if before_seq == 0 {
            set_activity_pages.update(|items| {
                items.entry(task_id).or_default().has_more = false;
            });
            return;
        }

        set_activity_pages.update(|items| {
            items.entry(task_id.clone()).or_default().loading = true;
        });
        set_error.set(None);
        spawn_ui(async move {
            let client = auth.client();
            match client
                .task_events_before_page(&sid, &task_id, before_seq, TASK_EVENTS_OLDER_LIMIT)
                .await
            {
                Ok(response) => {
                    set_activity_pages.update(|items| {
                        items.insert(
                            task_id.clone(),
                            ActivityPageState {
                                before_seq: response.first_seq,
                                has_more: response.has_more,
                                loading: false,
                            },
                        );
                    });
                    merge_task_events(set_events, response.events);
                }
                Err(error) => set_error.set(Some(task_submit_error_message(&error))),
            }
            set_activity_pages.update(|items| {
                items.entry(task_id).or_default().loading = false;
            });
        });
    });

    // Profile change handler — in welcome mode just updates the signal;
    // in chat mode also persists to the server.
    let on_profile_change = Callback::new(move |ev: leptos::ev::Event| {
        let value = event_target_value(&ev);
        set_selected_profile.set(value.clone());
        if let Some(sid) = session_id.get() {
            set_error.set(None);
            spawn_ui(async move {
                let client = auth.client();
                let request = UpdateSessionProfileRequest {
                    agent_profile_id: profile_value_to_id(&value),
                };
                match client.update_session_profile(&sid, &request).await {
                    Ok(response) => {
                        set_selected_profile.set(
                            response
                                .session
                                .agent_profile_id
                                .clone()
                                .unwrap_or_else(|| PROFILE_VALUE_NONE.to_string()),
                        );
                        upsert_session_summary(set_sessions, response.session);
                    }
                    Err(error) => set_error.set(Some(error.to_string())),
                }
            });
        }
    });

    let on_model_change = Callback::new(move |ev: leptos::ev::Event| {
        let qualified_id = event_target_value(&ev);
        if !model_routes
            .get_untracked()
            .iter()
            .any(|route| route.qualified_id == qualified_id && route.runnable)
        {
            return;
        }
        let previous_model = selected_model.get_untracked();
        set_model_touched.set(true);
        set_selected_model.set(qualified_id.clone());
        set_error.set(None);
        if let Some(sid) = session_id.get_untracked() {
            set_model_updating.set(true);
            spawn_ui(async move {
                let client = auth.client();
                let request = UpdateSessionModelRequest {
                    model_selection: ModelSelection { qualified_id },
                };
                match client.update_session_model(&sid, &request).await {
                    Ok(response) => {
                        set_selected_model.set(
                            response
                                .session
                                .model_selection
                                .as_ref()
                                .map(|selection| selection.qualified_id.clone())
                                .unwrap_or_default(),
                        );
                        upsert_session_summary(set_sessions, response.session);
                    }
                    Err(error) => {
                        set_selected_model.set(previous_model);
                        set_error.set(Some(error.to_string()));
                    }
                }
                set_model_updating.set(false);
            });
            return;
        }

        set_welcome_model.set(qualified_id.clone());
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
                default_model_selection: Some(ModelSelection { qualified_id }),
                default_agent_profile_id: settings.default_agent_profile_id,
            };
            match client.update_settings(&request).await {
                Ok(settings) => update_cached_settings(settings),
                Err(error) => set_error.set(Some(error.to_string())),
            }
        });
    });

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

    // Unified submit handler — branches on `session_id`:
    // `None`  → welcome flow: create session + task, pre-populate, navigate.
    // `Some`  → chat flow: create/resume task, start stream.
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let text = input.get();
        let files = pending_files.get();
        if !can_submit_input(&text, &files) {
            return;
        }
        let auth_state = auth.auth.get();
        let max_task_input_chars = auth_state.max_task_input_chars;
        if let Some(message) = task_input_limit_notice(&text, max_task_input_chars) {
            set_error.set(Some(message));
            return;
        }
        if voice_busy.get_untracked() {
            return;
        }
        set_loading.set(true);
        set_error.set(None);

        match session_id.get() {
            // ── Welcome flow ──────────────────────────────────────────
            None => {
                let agent_profile_selection =
                    agent_profile_selection_from_value(&selected_profile.get());
                let model_selection = (!selected_model.get().is_empty()).then(|| ModelSelection {
                    qualified_id: selected_model.get(),
                });
                spawn_ui(async move {
                    let client = auth.client();
                    // 1. Create session
                    let session_id = match client
                        .create_session(&CreateSessionRequest {
                            model_selection,
                            agent_profile_selection,
                        })
                        .await
                    {
                        Ok(resp) => {
                            let id = resp.session.session_id.clone();
                            upsert_session_summary(set_sessions, resp.session);
                            id
                        }
                        Err(e) => {
                            set_error.set(Some(e.to_string()));
                            set_loading.set(false);
                            return;
                        }
                    };
                    // 2. Prepare task input (upload pending attachments)
                    let (task_input, attachments) = match prepare_task_input(
                        &client,
                        &session_id,
                        text,
                        &files,
                        max_task_input_chars,
                    )
                    .await
                    {
                        Ok(payload) => payload,
                        Err(error) => {
                            let _ = client.delete_session(&session_id).await;
                            remove_session_summary(set_sessions, &session_id);
                            set_error.set(Some(error));
                            set_loading.set(false);
                            return;
                        }
                    };
                    // 3. Create task and pre-populate so the card appears immediately
                    match client
                        .create_task(
                            &session_id,
                            &CreateTaskRequest {
                                input_markdown: task_input,
                                attachments,
                                effort: Some(WEB_AGENT_EFFORT),
                            },
                        )
                        .await
                    {
                        Ok(response) => {
                            let task = response.task;
                            set_input.set(String::new());
                            reset_composer_textarea_height(textarea_ref);
                            set_pending_files.set(Vec::new());
                            // Pre-populate tasks + active_task so the card
                            // is visible during load_all's fetch.
                            set_active_task.set(Some(summary_to_detail(&session_id, &task)));
                            set_tasks.update(|items| upsert_task_summary(items, task));
                            // Navigate — triggers load_all via the Effect.
                            // load_all will manage `loading` state.
                            navigate(&format!("/app/session/{session_id}"));
                        }
                        Err(e) => {
                            let _ = client.delete_session(&session_id).await;
                            remove_session_summary(set_sessions, &session_id);
                            set_error.set(Some(e.to_string()));
                            set_loading.set(false);
                        }
                    }
                });
            }
            // ── Chat flow ─────────────────────────────────────────────
            Some(sid) => {
                // Clear stale activity for the new task
                set_events.set(Vec::new());
                set_progress.set(None);
                set_activity_pages.set(HashMap::new());
                set_activity_task_id.set(None);
                set_drawer_open.set(false);
                set_lightbox_image.set(None);
                spawn_ui(async move {
                    let client = auth.client();
                    let (task_input, attachments) =
                        match prepare_task_input(&client, &sid, text, &files, max_task_input_chars)
                            .await
                        {
                            Ok(payload) => payload,
                            Err(error) => {
                                set_error.set(Some(error));
                                set_loading.set(false);
                                return;
                            }
                        };
                    let resume_task_id = active_task
                        .get()
                        .filter(|task| task.status == TaskStatus::WaitingForUserInput)
                        .map(|task| task.task_id);
                    let result = match resume_task_id.as_deref() {
                        Some(task_id) => client
                            .resume_task(
                                &sid,
                                task_id,
                                &ResumeTaskRequest {
                                    input_markdown: task_input,
                                    attachments,
                                    effort: Some(WEB_AGENT_EFFORT),
                                },
                            )
                            .await
                            .map(|response| response.task),
                        _ => client
                            .create_task(
                                &sid,
                                &CreateTaskRequest {
                                    input_markdown: task_input,
                                    attachments,
                                    effort: Some(WEB_AGENT_EFFORT),
                                },
                            )
                            .await
                            .map(|response| response.task),
                    };

                    match result {
                        Ok(task) => {
                            set_input.set(String::new());
                            reset_composer_textarea_height(textarea_ref);
                            set_pending_files.set(Vec::new());
                            set_active_task.set(Some(summary_to_detail(&sid, &task)));
                            set_selected_versions.update(|items| {
                                items.insert(
                                    task.effective_version_group_id().to_string(),
                                    task.task_id.clone(),
                                );
                            });
                            start_task_stream(
                                client,
                                sid.clone(),
                                task.task_id.clone(),
                                0,
                                StreamUiSignals {
                                    set_events,
                                    set_progress,
                                    set_active_task,
                                    set_tasks,
                                    set_error,
                                    streaming_task_id,
                                    set_streaming_task_id,
                                    set_sessions,
                                },
                            );
                            let task_summary = task.clone();
                            set_tasks.update(|items| upsert_task_summary(items, task_summary));
                        }
                        Err(error) => set_error.set(Some(task_submit_error_message(&error))),
                    }
                    set_loading.set(false);
                });
            }
        }
    };

    let cancel_active = Callback::new(move |_| {
        let Some(task) = active_task.get() else {
            return;
        };
        let Some(sid) = session_id.get() else {
            return;
        };
        set_loading.set(true);
        set_error.set(None);
        spawn_ui(async move {
            let client = auth.client();
            match client.cancel_task(&sid, &task.task_id).await {
                Ok(_) => {
                    let task_id = task.task_id.clone();
                    set_active_task.set(None);
                    if streaming_task_id.get_untracked().as_deref() == Some(task_id.as_str()) {
                        set_streaming_task_id.set(None);
                    }
                    set_tasks.update(|items| {
                        for item in items {
                            if item.task_id == task_id {
                                item.status = TaskStatus::Cancelled;
                            }
                        }
                    });
                    if let Ok(response) = client.get_session(&sid).await {
                        upsert_session_summary(set_sessions, response.session);
                    }
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_loading.set(false);
        });
    });

    let on_voice_transcript = Callback::new(move |transcript: String| {
        set_input.set(merge_voice_transcript(&input.get_untracked(), &transcript));
        reset_composer_textarea_height(textarea_ref);
    });

    let include_default_profile = Signal::derive(move || session_id.get().is_none());

    let session_id_for_cards = session_id;

    view! {
        <ErrorBanner message=error />
        <section class="session-workspace">
            <div class="chat-wrapper"
                class=("welcome-mode", move || {
                    tasks.get().is_empty() && (!loading.get() || session_id.get().is_none())
                })
            >
                // Agent results — task cards with output
                <div class="results-panel">
                    {move || {
                        if tasks.get().is_empty() {
                            view! {
                                <div class="empty-state">
                                    <div class="empty-state-title">"What can I help you with?"</div>
                                    <div class="empty-state-text">
                                        "Send a message to start a new agent session."
                                    </div>
                                </div>
                            }.into_any()
                        } else {
                            let latest_editable = latest_editable_task_id(&tasks.get());
                            let sid_for_cards = session_id_for_cards.get().unwrap_or_default();
                            view! {
                                {move || tasks_has_more.get().then(|| view! {
                                    <div class="load-older-tasks">
                                        <button
                                            type="button"
                                            class="secondary"
                                            disabled=loading_older_tasks
                                            on:click=move |ev| load_older_tasks.run(ev)
                                        >
                                            {move || if loading_older_tasks.get() { "Loading history..." } else { "Load older messages" }}
                                        </button>
                                    </div>
                                })}
                                <For
                                    each=move || group_task_versions(&tasks.get())
                                    key=|group| group.version_group_id.clone()
                                    children=move |group| {
                                        view! {
                                            <TaskCard
                                                model=TaskCardModel {
                                                    session_id: sid_for_cards.clone(),
                                                    version_group_id: group.version_group_id.clone(),
                                                    tasks,
                                                    editable_task_id: latest_editable.clone(),
                                                    now_millis: elapsed_now_millis,
                                                }
                                                signals=TaskCardSignals {
                                                    events,
                                                    selected_versions,
                                                    set_selected_versions,
                                                    drawer_open,
                                                    set_drawer_open,
                                                    activity_task_id,
                                                    set_activity_task_id,
                                                    stream_signals: StreamUiSignals {
                                                        set_events,
                                                        set_progress,
                                                        set_active_task,
                                                        set_tasks,
                                                        set_error,
                                                        streaming_task_id,
                                                        set_streaming_task_id,
                                                        set_sessions,
                                                    },
                                                    set_error,
                                                }
                                            />
                                        }
                                    }
                                />
                            }
                            .into_any()
                        }
                    }}
                    <ActivityStatusChip
                        tasks=tasks
                        active_task=active_task
                        visible_task_ids=Signal::derive(move || {
                            selected_visible_activity_task_ids(&tasks.get(), &selected_versions.get())
                        })
                        open=drawer_open
                        set_open=set_drawer_open
                        activity_task_id=activity_task_id
                        set_activity_task_id=set_activity_task_id
                    />
                </div>

                // Prompt input
                <form class="composer" on:submit=submit>
                    <ComposerNotice active_task=active_task />
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
                            placeholder=move || if is_running() { "Agent is working…" } else if is_waiting() { "Reply to resume the task…" } else { "Message Oxide Agent…" }
                            prop:value=input
                            disabled=move || loading.get() || is_running()
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
                                input_blocked || voice_busy.get() || (!can_submit_input(&input.get(), &pending_files.get()) && !is_waiting())
                            }>
                                <AgentProfileSelect
                                    profiles=profiles
                                    selected_profile=selected_profile
                                    disabled=Signal::derive(move || loading.get() || is_running() || is_waiting())
                                    include_default=include_default_profile
                                    on_change=on_profile_change
                                />
                                <ModelRouteSelect
                                    routes=model_routes
                                    selected_model=selected_model
                                    disabled=Signal::derive(move || {
                                        loading.get() || model_updating.get() || model_routes.get().is_empty()
                                    })
                                    on_change=on_model_change
                                />
                                <label class="button secondary composer-attach-button">
                                    <input
                                        class="composer-file-input"
                                        type="file"
                                        multiple
                                        disabled=move || loading.get() || is_running()
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
                                <VoiceRecorderControl
                                    auth=auth
                                    disabled=Signal::derive(move || loading.get() || is_running() || voice_busy.get())
                                    set_busy=set_voice_busy
                                    set_error=set_error
                                    on_transcript=on_voice_transcript
                                />
                                <button
                                    type="submit"
                                    disabled=move || {
                                        let auth_state = auth.auth.get();
                                        let input_blocked = task_input_too_long(&input.get(), auth_state.max_task_input_chars)
                                            && !auth_state.large_input_attachments_supported;
                                        loading.get() || is_running() || voice_busy.get() || input_blocked || (!can_submit_input(&input.get(), &pending_files.get()) && !is_waiting())
                                    }
                                    class="btn-primary"
                                    style=move || if is_running() { "display:none" } else { "" }
                                >
                                    {move || {
                                        if is_waiting() { "Resume" } else { "Send" }
                                    }}
                                </button>
                                <button
                                    class="btn-danger"
                                    type="button"
                                    style=move || if is_running() { "" } else { "display:none" }
                                    on:click=move |ev| cancel_active.run(ev)
                                >
                                    "Stop"
                                </button>
                            </div>
                        </div>
                    </div>
                </form>
            </div>
            <ActivityDrawer
                open=drawer_open
                set_open=set_drawer_open
                activity_task_id=activity_task_id
                set_activity_task_id=set_activity_task_id
                tasks=tasks
                active_task=active_task
                events=events
                progress=progress
                has_older_events=Signal::derive(move || {
                    activity_task_id
                        .get()
                        .and_then(|task_id| activity_pages.get().get(&task_id).copied())
                        .is_some_and(|state| state.has_more)
                })
                loading_older_events=Signal::derive(move || {
                    activity_task_id
                        .get()
                        .and_then(|task_id| activity_pages.get().get(&task_id).copied())
                        .is_some_and(|state| state.loading)
                })
                load_older_events=load_older_activity
                now_millis=elapsed_now_millis
            />
            <Lightbox />
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
            _ => ().into_any(),
        }}
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
