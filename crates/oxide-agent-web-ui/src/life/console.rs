use leptos::prelude::*;

/// Permanent chat console — C7 placeholder.
///
/// C8 will replace this with the full transcript + composer + activity + paging UI.
/// The placeholder renders inside `AppLayout` when the route is `AppRoute::Life`.
#[component]
pub fn LifeConsole() -> impl IntoView {
    view! {
        <section class="life-console">
            <div class="life-placeholder">
                <h2>"Permanent Chat"</h2>
                <p>"Your continuous conversation with the agent — no session reset."</p>
            </div>
        </section>
    }
}
