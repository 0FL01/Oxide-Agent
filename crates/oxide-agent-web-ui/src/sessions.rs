use crate::auth::use_auth;
use crate::components::ErrorBanner;
use crate::confirm_dialog::ConfirmDialog;
use crate::utils::{navigate, spawn_ui};
use leptos::{html, prelude::*};
use oxide_agent_web_contracts::{SessionSummary, UpdateSessionRequest};

/// Pending deletion target — the session the user is about to delete.
#[derive(Clone)]
struct DeleteTarget {
    id: String,
    title: String,
}

#[component]
pub fn SessionSidebar(
    selected: Memo<Option<String>>,
    is_life: Memo<bool>,
    sessions: ReadSignal<Vec<SessionSummary>>,
    set_sessions: WriteSignal<Vec<SessionSummary>>,
) -> impl IntoView {
    let auth = use_auth();
    let (error, set_error) = signal(None::<String>);
    let (loading, set_loading) = signal(false);
    let (loaded, set_loaded) = signal(false);
    let (search, set_search) = signal(String::new());
    let (confirm_target, set_confirm_target) = signal(None::<DeleteTarget>);

    let load_sessions = move || {
        set_loading.set(true);
        set_error.set(None);
        spawn_ui(async move {
            match auth.client().list_sessions().await {
                Ok(response) => {
                    set_sessions.set(response.sessions);
                }
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
        navigate("/app");
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

    let confirm_title = Signal::derive(move || {
        confirm_target
            .get()
            .map(|t| t.title.clone())
            .unwrap_or_default()
    });
    let confirm_message = Signal::derive(move || {
        confirm_target
            .get()
            .map(|t| format!("\"{}\" will be permanently deleted.", t.title))
            .unwrap_or_default()
    });

    let on_confirm = Callback::new(move |_| {
        let target = confirm_target.get();
        set_confirm_target.set(None);
        if let Some(target) = target {
            let active = selected.get().as_ref() == Some(&target.id);
            let session_id = target.id;
            set_error.set(None);
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
            });
        }
    });

    let on_cancel = Callback::new(move |_| {
        set_confirm_target.set(None);
    });

    view! {
        <aside class="sidebar">
            <div class="sidebar-header">
                <svg class="logo-icon" width="20" height="20" viewBox="0 0 24 24"
                     fill="none" stroke="currentColor" stroke-width="2.2"
                     stroke-linecap="round" stroke-linejoin="round"
                     style="color: var(--accent); flex-shrink: 0;">
                    <path d="M12 2L2 7l10 5 10-5-10-5z"/>
                    <path d="M2 17l10 5 10-5"/>
                    <path d="M2 12l10 5 10-5"/>
                </svg>
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
            <div class="sidebar-permanent">
                <a
                    class=move || if is_life.get() { "permanent-entry active" } else { "permanent-entry" }
                    href="/life"
                >
                    <span class="permanent-icon">
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none"
                             stroke="currentColor" stroke-width="2.2"
                             stroke-linecap="round" stroke-linejoin="round">
                            <path d="M18.178 8c5.296 6.661-11.047 13.817-14.356 6.661C.376 7.555 13.834-2.824 18.178 8z"/>
                            <path d="M5.822 16c-5.296-6.661 11.047-13.817 14.356-6.661C23.624 16.445 10.166 26.824 5.822 16z"/>
                        </svg>
                    </span>
                    <span class="permanent-label">"Permanent"</span>
                </a>
            </div>
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
                        view! {
                            <ul class="session-list">
                                <For
                                    each=move || filtered.clone()
                                    key=|session| session.session_id.clone()
                                    children=move |session| {
                                        let session_id = session.session_id.clone();
                                        let active = Signal::derive(move || {
                                            selected.get().as_deref() == Some(session_id.as_str())
                                        });
                                        view! {
                                            <SessionItem
                                                session=session
                                                active=active
                                                set_confirm_target=set_confirm_target
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
        <ConfirmDialog
            open=Signal::derive(move || confirm_target.get().is_some())
            title=confirm_title
            message=confirm_message
            confirm_label="Delete".to_string()
            on_confirm=on_confirm
            on_cancel=on_cancel
        />
    }
}

#[component]
fn SessionItem(
    session: SessionSummary,
    active: Signal<bool>,
    set_confirm_target: WriteSignal<Option<DeleteTarget>>,
    set_sessions: WriteSignal<Vec<SessionSummary>>,
    set_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    let auth = use_auth();
    let session_id = session.session_id.clone();
    let session_title = display_session_title(&session);
    let original_title = session.title.clone();

    let (is_renaming, set_is_renaming) = signal(false);
    let (draft_title, set_draft_title) = signal(original_title.clone());
    let (is_saving, set_is_saving) = signal(false);

    let input_ref = NodeRef::<html::Input>::new();
    Effect::new(move |_| {
        if is_renaming.get()
            && !is_saving.get()
            && let Some(el) = input_ref.get()
        {
            let _ = el.focus();
            el.select();
        }
    });

    let request_rename = Callback::new({
        let original_title = original_title.clone();
        move |ev: leptos::ev::MouseEvent| {
            ev.prevent_default();
            ev.stop_propagation();
            set_draft_title.set(original_title.clone());
            set_is_renaming.set(true);
        }
    });

    let request_delete = Callback::new({
        let session_id = session_id.clone();
        let session_title = session_title.clone();
        move |ev: leptos::ev::MouseEvent| {
            ev.prevent_default();
            ev.stop_propagation();
            set_confirm_target.set(Some(DeleteTarget {
                id: session_id.clone(),
                title: session_title.clone(),
            }));
        }
    });

    let cancel_rename = Callback::new(move |_: ()| {
        if is_saving.get() {
            return;
        }
        set_is_renaming.set(false);
    });

    let commit_rename = Callback::new({
        let session_id = session_id.clone();
        let original_title = original_title.clone();
        move |_: ()| {
            if !is_renaming.get() || is_saving.get() {
                return;
            }
            let title = draft_title.get().trim().to_string();
            if title.is_empty() || title == original_title {
                set_is_renaming.set(false);
                return;
            }
            set_is_saving.set(true);
            set_error.set(None);
            let session_id = session_id.clone();
            spawn_ui(async move {
                match auth
                    .client()
                    .update_session(&session_id, &UpdateSessionRequest { title })
                    .await
                {
                    Ok(resp) => {
                        set_sessions.update(|items| {
                            if let Some(item) =
                                items.iter_mut().find(|i| i.session_id == session_id)
                            {
                                item.title = resp.session.title;
                            }
                        });
                        set_is_renaming.set(false);
                    }
                    Err(e) => set_error.set(Some(e.to_string())),
                }
                set_is_saving.set(false);
            });
        }
    });

    view! {
        <li class="session-list-item">
            {move || {
                if is_renaming.get() {
                    view! {
                        <div class="session-rename-row">
                            <input
                                class="session-rename-input"
                                type="text"
                                node_ref=input_ref
                                prop:value=draft_title
                                disabled=is_saving
                                on:input=move |ev| set_draft_title.set(event_target_value(&ev))
                                on:keydown=move |ev| {
                                    match ev.key().as_str() {
                                        "Enter" => {
                                            ev.prevent_default();
                                            commit_rename.run(());
                                        }
                                        "Escape" => {
                                            ev.prevent_default();
                                            cancel_rename.run(());
                                        }
                                        _ => {}
                                    }
                                }
                                on:focusout=move |_| commit_rename.run(())
                            />
                        </div>
                    }
                        .into_any()
                } else {
                    let session = session.clone();
                    view! {
                        <a
                            class=move || if active.get() { "session-item active" } else { "session-item" }
                            href=format!("/app/session/{}", session.session_id)
                        >
                            <span class="session-copy">
                                <span class="session-id">{display_session_title(&session)}</span>
                            </span>
                        </a>
                        <div class="session-actions">
                            <button
                                class="session-action-button rename"
                                type="button"
                                title="Rename session"
                                on:click=move |ev| request_rename.run(ev)
                            >
                                <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
                                     stroke="currentColor" stroke-width="2"
                                     stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M12 20h9"/>
                                    <path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"/>
                                </svg>
                            </button>
                            <button
                                class="session-action-button delete"
                                type="button"
                                title="Delete session"
                                on:click=move |ev| request_delete.run(ev)
                            >
                                "Del"
                            </button>
                        </div>
                    }
                        .into_any()
                }
            }}
        </li>
    }
}

fn display_session_title(session: &SessionSummary) -> String {
    let trimmed = session.title.trim();
    if trimmed.is_empty() || trimmed == "New session" || looks_like_timestamp_title(trimmed) {
        return "New chat".to_string();
    }
    concise_title(&session.title)
}

fn concise_title(value: &str) -> String {
    concise_text(value, 32)
}

fn concise_text(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut out = normalized
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn looks_like_timestamp_title(value: &str) -> bool {
    let value = value.trim();
    let bytes = value.as_bytes();
    bytes.len() >= 16
        && bytes
            .get(0..4)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
        && bytes.get(4) == Some(&b'-')
        && bytes
            .get(5..7)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
        && bytes.get(7) == Some(&b'-')
        && bytes
            .get(8..10)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
        && matches!(bytes.get(10), Some(b' ' | b'T'))
        && bytes
            .get(11..13)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
        && bytes.get(13) == Some(&b':')
        && bytes
            .get(14..16)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
}

#[cfg(test)]
mod tests {
    use super::looks_like_timestamp_title;

    #[test]
    fn detects_chrono_timestamp_titles() {
        assert!(looks_like_timestamp_title("2026-05-29 20:53:47.208618014"));
        assert!(looks_like_timestamp_title("2026-05-29T20:53:47Z"));
        assert!(!looks_like_timestamp_title("Cloud storage limits"));
    }
}
