use crate::auth::use_auth;
use crate::components::ErrorBanner;
use crate::utils::{friendly_time, navigate, spawn_ui};
use leptos::prelude::*;
use oxide_agent_web_contracts::SessionSummary;

#[component]
pub fn SessionSidebar(selected: Option<String>) -> impl IntoView {
    let auth = use_auth();
    let (sessions, set_sessions) = signal(Vec::<SessionSummary>::new());
    let (error, set_error) = signal(None::<String>);
    let (loading, set_loading) = signal(false);
    let (loaded, set_loaded) = signal(false);
    let (search, set_search) = signal(String::new());

    let load_sessions = move || {
        set_loading.set(true);
        set_error.set(None);
        spawn_ui(async move {
            match auth.client().list_sessions().await {
                Ok(response) => set_sessions.set(response.sessions),
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_loading.set(false);
        });
    };

    Effect::new(move |_| {
        if !loaded.get() {
            set_loaded.set(true);
            load_sessions();
        }
    });

    let create_session = move |_| {
        set_loading.set(true);
        set_error.set(None);
        spawn_ui(async move {
            match auth.client().create_session().await {
                Ok(response) => navigate(&format!("/app/session/{}", response.session.session_id)),
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_loading.set(false);
        });
    };

    let filtered_sessions = move || {
        let query = search.get().to_lowercase();
        sessions
            .get()
            .into_iter()
            .filter(|session| {
                if query.is_empty() {
                    return true;
                }
                session.title.to_lowercase().contains(&query)
                    || session
                        .last_preview
                        .as_deref()
                        .is_some_and(|p| p.to_lowercase().contains(&query))
            })
            .collect::<Vec<_>>()
    };

    view! {
        <aside class="sidebar">
            <div class="sidebar-header">
                <h2>
                    "Oxide Agent"
                    <span>"v0.1"</span>
                </h2>
                <button type="button" title="New session" on:click=create_session disabled=loading>
                    "+"
                </button>
            </div>
            <div class="sidebar-search">
                <input
                    type="text"
                    placeholder="Search sessions..."
                    prop:value=search
                    on:input=move |ev| set_search.set(event_target_value(&ev))
                />
            </div>
            <ErrorBanner message=error />
            <div class="sessions-list">
                {move || {
                    if loading.get() && sessions.get().is_empty() {
                        view! { <div class="empty-state">"Loading..."</div> }.into_any()
                    } else if sessions.get().is_empty() {
                        view! {
                            <div class="empty-state">
                                <div class="empty-state-title">"No sessions"</div>
                                <div class="empty-state-text">"Create a new session to get started."</div>
                            </div>
                        }
                        .into_any()
                    } else {
                        let filtered = filtered_sessions();
                        let selected_clone = selected.clone();
                        view! {
                            <ul class="session-list">
                                <For
                                    each=move || filtered.clone()
                                    key=|session| session.session_id.clone()
                                    children=move |session| {
                                        let active = selected_clone
                                            .as_ref()
                                            == Some(&session.session_id);
                                        view! {
                                            <SessionItem
                                                session=session
                                                active=active
                                                set_sessions=set_sessions
                                                set_error=set_error
                                            />
                                        }
                                    }
                                />
                            </ul>
                        }
                        .into_any()
                    }
                }}
            </div>
            <div class="sidebar-footer">
                <a href="/settings">"Settings"</a>
            </div>
        </aside>
    }
}

#[component]
fn SessionItem(
    session: SessionSummary,
    active: bool,
    set_sessions: WriteSignal<Vec<SessionSummary>>,
    set_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    let auth = use_auth();
    let item_class = if active {
        "session-item active"
    } else {
        "session-item"
    };
    let session_id = session.session_id.clone();
    let session_title = session.title.clone();
    let (deleting, set_deleting) = signal(false);

    // Determine status dot class from last task status
    let status_class = match session.last_task_status {
        Some(oxide_agent_web_contracts::TaskStatus::Running) => "running",
        Some(oxide_agent_web_contracts::TaskStatus::Completed) => "success",
        Some(oxide_agent_web_contracts::TaskStatus::Failed) => "error",
        Some(oxide_agent_web_contracts::TaskStatus::Cancelled) => "error",
        Some(oxide_agent_web_contracts::TaskStatus::Interrupted) => "warning",
        _ => "idle",
    };

    let delete_session = move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        ev.stop_propagation();
        if !confirm_delete_session(&session_title) {
            return;
        }
        set_deleting.set(true);
        set_error.set(None);
        let session_id = session_id.clone();
        spawn_ui(async move {
            match auth.client().delete_session(&session_id).await {
                Ok(_) => {
                    set_sessions.update(|items| {
                        items.retain(|item| item.session_id != session_id);
                    });
                    if active {
                        navigate("/app");
                    }
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_deleting.set(false);
        });
    };

    view! {
        <li class="session-list-item">
            <a class=item_class href=format!("/app/session/{}", session.session_id)>
                <span class=format!("session-status-dot {}", status_class)></span>
                <span class="session-copy">
                    <span class="session-id">{session.title}</span>
                    <span class="session-preview">
                        {session.last_preview.unwrap_or_else(|| "No task yet".to_string())}
                    </span>
                </span>
                <span class="session-time">{friendly_time(session.updated_at)}</span>
            </a>
            <button
                class="session-delete-button"
                type="button"
                title="Delete session"
                disabled=deleting
                on:click=delete_session
            >
                "Del"
            </button>
        </li>
    }
}

fn confirm_delete_session(title: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|window| {
                window
                    .confirm_with_message(&format!("Delete session \"{title}\"?"))
                    .ok()
            })
            .unwrap_or(false)
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = title;
        true
    }
}
