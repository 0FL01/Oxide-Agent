use crate::auth::{AuthContext, AuthState, BootstrapPage, LoginPage, RegisterPage, SettingsPage};
use crate::components::AppLayout;
use crate::routes::AppRoute;
use crate::utils::{navigate, spawn_ui};
use leptos::prelude::*;

#[component]
pub fn App() -> impl IntoView {
    let (auth, set_auth) = signal(AuthState {
        loading: true,
        ..AuthState::default()
    });
    let auth_context = AuthContext { auth, set_auth };
    provide_context(auth_context);

    let (route, _set_route) = signal(AppRoute::current());
    let (loaded, set_loaded) = signal(false);

    Effect::new(move |_| {
        if loaded.get() {
            return;
        }
        set_loaded.set(true);
        spawn_ui(async move {
            match auth_context.client().me().await {
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

    view! {
        <div class="root">
            {move || match route.get() {
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
                AppRoute::App | AppRoute::Session(_) => {
                    let state = auth.get();
                    if state.loading {
                        loading_view()
                    } else if state.user.is_none() {
                        redirecting_view()
                    } else {
                        view! { <AppLayout route=route.get() /> }.into_any()
                    }
                }
                AppRoute::NotFound => view! {
                    <section class="not-found">
                        <h1>"Not found"</h1>
                        <a class="button" href="/app">"Open app"</a>
                    </section>
                }.into_any(),
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
