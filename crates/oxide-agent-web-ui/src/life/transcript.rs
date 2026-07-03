use crate::life::state::life_turn_activity_run_id;
use crate::markdown::render_markdown;
use crate::tasks::composer::MessageAttachments;
use leptos::prelude::*;
use oxide_agent_web_contracts::{ApiLifeTurnResponse, TaskAttachment};

/// Render the life transcript — a chronological list of user and assistant
/// turns with "Load older" paging at the top.
#[component]
pub(crate) fn LifeTranscript(
    turns: ReadSignal<Vec<ApiLifeTurnResponse>>,
    selected_activity_run_id: ReadSignal<Option<String>>,
    drawer_open: ReadSignal<bool>,
    open_activity: Callback<String>,
    has_more: Signal<bool>,
    loading_older: Signal<bool>,
    load_older: Callback<leptos::ev::MouseEvent>,
) -> impl IntoView {
    view! {
        <div class="life-transcript">
            {move || {
                if has_more.get() {
                    view! {
                        <div class="life-load-older">
                            <button
                                type="button"
                                class="secondary"
                                disabled=loading_older
                                on:click=move |ev| load_older.run(ev)
                            >
                                {move || if loading_older.get() { "Loading older..." } else { "Load older messages" }}
                            </button>
                        </div>
                    }.into_any()
                } else {
                    ().into_any()
                }
            }}
            {move || {
                let items = turns.get();
                if items.is_empty() {
                    view! {
                        <div class="life-empty-state">
                            <div class="life-empty-title">"Permanent Chat"</div>
                            <div class="life-empty-text">
                                "Your continuous conversation with the agent — no session reset."
                            </div>
                        </div>
                    }.into_any()
                } else {
                    items
                        .into_iter()
                        .map(|turn| view! {
                            <LifeTurnCard
                                turn=turn
                                selected_activity_run_id=selected_activity_run_id
                                drawer_open=drawer_open
                                open_activity=open_activity
                            />
                        })
                        .collect_view()
                        .into_any()
                }
            }}
        </div>
    }
}

#[component]
fn LifeTurnCard(
    turn: ApiLifeTurnResponse,
    selected_activity_run_id: ReadSignal<Option<String>>,
    drawer_open: ReadSignal<bool>,
    open_activity: Callback<String>,
) -> impl IntoView {
    let is_user = turn.role == "user";
    let class = if is_user {
        "life-turn user"
    } else {
        "life-turn assistant"
    };
    let role_label = if is_user { "You" } else { "Agent" };
    let activity_run_id = life_turn_activity_run_id(&turn).map(ToOwned::to_owned);
    let content_html = if is_user {
        // User messages are plain text — escape and preserve line breaks.
        escape_html_with_breaks(&turn.content)
    } else {
        // Assistant messages are markdown.
        render_markdown(&turn.content)
    };
    let attachments = parse_attachments(&turn.attachments);

    view! {
        <div class=class>
            <div class="life-turn-header">
                <span class="life-turn-role">{role_label}</span>
            </div>
            <div class="life-turn-content" inner_html=content_html></div>
            {move || {
                if attachments.is_empty() {
                    ().into_any()
                } else {
                    view! { <MessageAttachments attachments=attachments.clone() /> }.into_any()
                }
            }}
            {move || {
                activity_run_id
                    .as_ref()
                    .map(|run_id| {
                        let run_id_for_click = run_id.clone();
                        let open = drawer_open.get()
                            && selected_activity_run_id.get().as_deref() == Some(run_id.as_str());
                        view! {
                            <button
                                class=if open { "life-activity-toggle open" } else { "life-activity-toggle" }
                                type="button"
                                on:click=move |_| open_activity.run(run_id_for_click.clone())
                            >
                                <span>"Activity"</span>
                                <span class="chevron">"›"</span>
                            </button>
                        }
                    })
            }}
        </div>
    }
}

/// Parse `serde_json::Value` attachments into typed `TaskAttachment`s.
fn parse_attachments(value: &serde_json::Value) -> Vec<TaskAttachment> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

/// Escape HTML special characters and convert newlines to `<br>`.
fn escape_html_with_breaks(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    escaped.replace('\n', "<br>")
}
