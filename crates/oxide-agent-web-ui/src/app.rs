use crate::auth::{AuthContext, AuthState, BootstrapPage, LoginPage, RegisterPage, SettingsPage};
use crate::components::AppLayout;
use crate::routes::{AppRoute, RouteContext};
use crate::utils::{browser_pathname, intercept_in_app_click, navigate, spawn_ui};
use futures_util::join;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

#[component]
pub fn App() -> impl IntoView {
    let (auth, set_auth) = signal(AuthState {
        loading: true,
        ..AuthState::default()
    });
    let auth_context = AuthContext { auth, set_auth };
    provide_context(auth_context);

    let (route, set_route) = signal(AppRoute::current());
    provide_context(RouteContext { set_route });

    let (loaded, set_loaded) = signal(false);

    Effect::new(move |_| {
        if loaded.get() {
            return;
        }
        set_loaded.set(true);
        spawn_ui(async move {
            let client = auth_context.client();
            let (config_result, me_result) = join!(client.public_config(), client.me());
            if let Ok(config) = config_result {
                auth_context.set_auth.update(|state| {
                    state.max_task_input_chars = config.max_task_input_chars;
                    state.large_input_attachments_supported =
                        config.large_input_attachments_supported;
                });
            }
            match me_result {
                Ok(response) => {
                    auth_context.set_authenticated(response.user, Some(response.csrf_token))
                }
                Err(_) => {
                    auth_context.set_auth.update(|state| {
                        state.loading = false;
                        state.user = None;
                        state.csrf_token = None;
                    });
                }
            }
        });
    });

    Effect::new(move |_| {
        let current_route = route.get();
        let state = auth.get();
        if route_requires_auth(&current_route) && !state.loading && state.user.is_none() {
            navigate("/login");
        }
    });

    // Listen for browser back/forward (popstate) and update the route signal
    // so the SPA stays in sync with the URL bar.  The closure is leaked with
    // `forget()` because the listener lives for the entire page lifetime —
    // `App` is the root component and is never unmounted.
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            let popstate_cb = Closure::wrap(Box::new(move |_| {
                set_route.set(AppRoute::from_path(&browser_pathname()));
            }) as Box<dyn FnMut(web_sys::Event)>);
            let _ = window
                .add_event_listener_with_callback("popstate", popstate_cb.as_ref().unchecked_ref());
            popstate_cb.forget();
        }
    }

    // `is_app_route` stays `true` across `App ↔ Session(_)` transitions.
    // The view closure depends on this Memo, not on `route` directly,
    // so `AppLayout` stays mounted when switching between welcome and chat.
    let is_app_route =
        Memo::new(move |_| matches!(route.get(), AppRoute::App | AppRoute::Session(_)));

    view! {
        <div class="root" on:click=move |ev| intercept_in_app_click(&ev)>
            {move || {
                if is_app_route.get() {
                    let state = auth.get();
                    if state.loading {
                        loading_view()
                    } else if state.user.is_none() {
                        redirecting_view()
                    } else {
                        view! { <AppLayout route=route /> }.into_any()
                    }
                } else {
                    match route.get() {
                        AppRoute::Login => view! { <LoginPage /> }.into_any(),
                        AppRoute::Register => view! { <RegisterPage /> }.into_any(),
                        AppRoute::Bootstrap => view! { <BootstrapPage /> }.into_any(),
                        AppRoute::Settings => {
                            let state = auth.get();
                            if state.loading {
                                loading_view()
                            } else if state.user.is_none() {
                                redirecting_view()
                            } else {
                                view! { <SettingsPage /> }.into_any()
                            }
                        }
                        AppRoute::NotFound => view! {
                            <section class="not-found">
                                <h1>"Not found"</h1>
                                <a class="button" href="/app">"Open app"</a>
                            </section>
                        }.into_any(),
                        // AppRoute::App | AppRoute::Session(_) handled by is_app_route above
                        _ => ().into_any(),
                    }
                }
            }}
        </div>
    }
}

fn route_requires_auth(route: &AppRoute) -> bool {
    matches!(
        route,
        AppRoute::App | AppRoute::Session(_) | AppRoute::Settings
    )
}

fn loading_view() -> AnyView {
    view! {
        <section class="auth-page">
            <div class="loading">"Loading"</div>
        </section>
    }
    .into_any()
}

fn redirecting_view() -> AnyView {
    view! {
        <section class="auth-page">
            <div class="loading">"Redirecting"</div>
        </section>
    }
    .into_any()
}
