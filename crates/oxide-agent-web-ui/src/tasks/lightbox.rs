//! Full-screen image lightbox overlay.
//!
//! A session-scoped signal (`LightboxContext`) is provided via Leptos context.
//! Any component can open an image by calling `ctx.set_image.set(Some(...))`.
//! Clicking the darkened backdrop or pressing Escape closes the lightbox.

use leptos::{html, prelude::*};

/// Image payload for the lightbox — just what the overlay needs to render.
/// Callers (e.g. `BrowserToolCard`) supply the artifact URL and alt text;
/// the lightbox knows nothing about browser tools, sessions, or artifacts.
#[derive(Clone, PartialEq)]
pub struct LightboxImage {
    pub url: String,
    pub alt: String,
}

/// Context handle injected by `SessionWorkspace`. Both fields are `Copy`
/// (signals are `Copy` in Leptos 0.8), so the struct is `Copy` too —
/// no cloning needed when capturing in closures.
#[derive(Clone, Copy)]
pub struct LightboxContext {
    pub image: ReadSignal<Option<LightboxImage>>,
    pub set_image: WriteSignal<Option<LightboxImage>>,
}

/// Full-screen lightbox overlay. Renders nothing when the signal is `None`.
///
/// Mount this once at the session-workspace level. The component reads
/// `LightboxContext` from context and reacts to signal changes.
#[component]
pub(super) fn Lightbox() -> impl IntoView {
    let Some(ctx) = use_context::<LightboxContext>() else {
        return ().into_any();
    };

    // Focus the backdrop when it appears so keyboard events (Escape) are
    // captured without requiring a global window listener.
    let backdrop_ref = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        if ctx.image.get().is_some()
            && let Some(el) = backdrop_ref.get()
        {
            let _ = el.focus();
        }
    });

    view! {
        {move || {
            ctx.image
                .get()
                .map(|img| {
                    view! {
                        <div
                            class="lightbox-backdrop"
                            node_ref=backdrop_ref
                            tabindex="0"
                            on:click=move |_| ctx.set_image.set(None)
                            on:keydown=move |ev| {
                                if ev.key() == "Escape" {
                                    ctx.set_image.set(None);
                                }
                            }
                        >
                            <img
                                class="lightbox-image"
                                src=img.url.clone()
                                alt=img.alt.clone()
                                on:click=move |ev| ev.stop_propagation()
                            />
                        </div>
                    }
                    .into_any()
                })
                .unwrap_or_else(|| ().into_any())
        }}
    }
    .into_any()
}
