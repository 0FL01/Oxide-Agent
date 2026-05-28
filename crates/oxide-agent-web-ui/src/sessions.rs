use crate::auth::use_auth;
use crate::components::{EmptyState, ErrorBanner, StatusBadge};
use crate::utils::{friendly_time, navigate, spawn_ui};
use leptos::prelude::*;
use oxide_agent_web_contracts::{SessionSummary, UpdateSessionRequest};

#[component]
pub fn SessionSidebar(selected: Option<String>) -> impl IntoView {
    let auth = use_auth();
    let (sessions, set_sessions) = signal(Vec::<SessionSummary>::new());
    let (error, set_error) = signal(None::<String>);
    let (loading, set_loading) = signal(false);
    let (loaded, set_loaded) = signal(false);

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

    view! {
        <aside class="sidebar">
            <div class="sidebar-header">
                <h2>"Sessions"</h2>
                <button type="button" title="New session" on:click=create_session disabled=loading>
                    "+"
                </button>
            </div>
            <ErrorBanner message=error />
            {move || {
                if loading.get() && sessions.get().is_empty() {
                    view! { <div class="loading">"Loading"</div> }.into_any()
                } else if sessions.get().is_empty() {
                    view! { <EmptyState title="No sessions" /> }.into_any()
                } else {
                    let selected_for_rows = selected.clone();
                    view! {
                        <ul class="session-list">
                            <For
                                each=move || sessions.get()
                                key=|session| session.session_id.clone()
                                children=move |session| {
                                    let active =
                                        selected_for_rows.as_ref() == Some(&session.session_id);
                                    view! { <SessionRow session=session active=active /> }
                                }
                            />
                        </ul>
                    }.into_any()
                }
            }}
        </aside>
    }
}

#[component]
fn SessionRow(session: SessionSummary, active: bool) -> impl IntoView {
    let class_name = if active {
        "session-row active"
    } else {
        "session-row"
    };

    view! {
        <li>
            <a class=class_name href=format!("/app/session/{}", session.session_id)>
                <span class="session-title">{session.title}</span>
                {session.last_task_status.map(|status| view! { <StatusBadge status=status /> })}
                <span class="session-preview">
                    {session.last_preview.unwrap_or_else(|| "No task yet".to_string())}
                </span>
                <span class="session-time">{friendly_time(session.updated_at)}</span>
            </a>
        </li>
    }
}

#[component]
pub fn RenameSessionForm(session_id: String, current_title: String) -> impl IntoView {
    let auth = use_auth();
    let (title, set_title) = signal(current_title);
    let (error, set_error) = signal(None::<String>);
    let (saving, set_saving) = signal(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        set_saving.set(true);
        set_error.set(None);
        let request = UpdateSessionRequest { title: title.get() };
        let session_id = session_id.clone();
        spawn_ui(async move {
            match auth.client().update_session(&session_id, &request).await {
                Ok(_) => navigate(&format!("/app/session/{session_id}")),
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_saving.set(false);
        });
    };

    view! {
        <form class="rename-form" on:submit=submit>
            <input value=title on:input=move |ev| set_title.set(event_target_value(&ev)) />
            <button type="submit" disabled=saving title="Save title">"Save"</button>
            <ErrorBanner message=error />
        </form>
    }
}
