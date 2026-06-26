//! Reusable confirmation modal overlay.
//!
//! Renders a centered dialog with a darkened backdrop. Clicking the backdrop
//! or pressing Escape triggers `on_cancel`; clicking inside the card does
//! nothing (stop-propagation). The component renders nothing when `open`
//! is false, so it can stay mounted on a parent without visual cost.
//!
//! Mount as a **sibling** of any container that may be `display:none` on some
//! viewport (e.g. the sidebar) — `position:fixed` children of a `display:none`
//! ancestor are invisible, so the dialog must live outside such containers.

use leptos::{html, prelude::*};

/// Reusable confirmation modal. All content (title, message, confirm label)
/// is driven by signals so it can reflect the current target without
/// re-mounting. The confirm button uses the danger style — this component
/// exists for destructive confirmations (delete, remove).
#[component]
pub fn ConfirmDialog(
    open: Signal<bool>,
    title: Signal<String>,
    message: Signal<String>,
    confirm_label: String,
    on_confirm: Callback<()>,
    on_cancel: Callback<()>,
) -> impl IntoView {
    // Focus the Cancel button when the dialog opens so keyboard users land on
    // the safe action first. Escape still works via event bubbling to the
    // backdrop listener.
    let cancel_ref = NodeRef::<html::Button>::new();
    Effect::new(move |_| {
        if open.get()
            && let Some(el) = cancel_ref.get()
        {
            let _ = el.focus();
        }
    });

    view! {
        {move || {
            if open.get() {
                view! {
                    <div
                        class="modal-backdrop"
                        tabindex="0"
                        on:click=move |_| on_cancel.run(())
                        on:keydown=move |ev| {
                            if ev.key() == "Escape" {
                                on_cancel.run(());
                            }
                        }
                    >
                        <div class="modal-card" on:click=move |ev| ev.stop_propagation()>
                            <h3 class="modal-title">{move || title.get()}</h3>
                            <p class="modal-message">{move || message.get()}</p>
                            <div class="modal-actions">
                                <button
                                    class="secondary"
                                    type="button"
                                    node_ref=cancel_ref
                                    on:click=move |_| on_cancel.run(())
                                >
                                    "Cancel"
                                </button>
                                <button
                                    class="btn-danger"
                                    type="button"
                                    on:click=move |_| on_confirm.run(())
                                >
                                    {confirm_label.clone()}
                                </button>
                            </div>
                        </div>
                    </div>
                }
                .into_any()
            } else {
                ().into_any()
            }
        }}
    }
    .into_any()
}
