use crate::api::ApiClient;
use crate::utils::{navigate, spawn_ui};
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    BootstrapRequest, ChangePasswordRequest, CurrentUser, LoginRequest, ModelRouteProtocolView,
    ModelRouteSourceView, ModelRouteView, ModelSelection, RegisterRequest,
    UpdateUserSettingsRequest, UserRole,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthState {
    pub user: Option<CurrentUser>,
    pub csrf_token: Option<String>,
    pub loading: bool,
    pub session_expired: bool,
}

#[derive(Clone, Copy)]
pub struct AuthContext {
    pub auth: ReadSignal<AuthState>,
    pub set_auth: WriteSignal<AuthState>,
}

impl AuthContext {
    #[must_use]
    pub fn client(self) -> ApiClient {
        ApiClient::new(self.auth.get().csrf_token)
    }

    pub fn set_authenticated(self, user: CurrentUser, csrf_token: Option<String>) {
        self.set_auth.update(|state| {
            state.user = Some(user);
            state.csrf_token = csrf_token;
            state.loading = false;
            state.session_expired = false;
        });
    }

    pub fn clear(self) {
        self.set_auth.set(AuthState {
            session_expired: true,
            ..AuthState::default()
        });
    }
}

#[must_use]
pub fn use_auth() -> AuthContext {
    match use_context::<AuthContext>() {
        Some(context) => context,
        None => panic!("AuthContext is provided by App"),
    }
}

#[component]
pub fn LoginPage() -> impl IntoView {
    let auth = use_auth();
    let (login, set_login) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (error, set_error) = signal(None::<String>);
    let (loading, set_loading) = signal(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let login_value = login.get();
        let password_value = password.get();
        set_loading.set(true);
        set_error.set(None);

        spawn_ui(async move {
            let request = LoginRequest {
                login: login_value,
                password: password_value,
            };
            match ApiClient::new(None).login(&request).await {
                Ok(response) => {
                    auth.set_authenticated(response.user, response.csrf_token);
                    navigate("/app");
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_loading.set(false);
        });
    };

    view! {
        <section class="auth-page">
            <form class="auth-panel" on:submit=submit>
                <h1>"Oxide Agent"</h1>
                <label>
                    <span>"Login"</span>
                    <input
                        autocomplete="username"
                        value=login
                        on:input=move |ev| set_login.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    <span>"Password"</span>
                    <input
                        type="password"
                        autocomplete="current-password"
                        value=password
                        on:input=move |ev| set_password.set(event_target_value(&ev))
                    />
                </label>
                <button type="submit" disabled=loading>
                    {move || if loading.get() { "Signing in" } else { "Sign in" }}
                </button>
                <ErrorText error=error />
                {move || {
                    auth.auth
                        .get()
                        .session_expired
                        .then(|| view! { <p class="notice">"Session expired."</p> })
                }}
                <div class="auth-links">
                    <a href="/register">"Register"</a>
                    <a href="/bootstrap">"Bootstrap"</a>
                </div>
            </form>
        </section>
    }
}

#[component]
pub fn RegisterPage() -> impl IntoView {
    let auth = use_auth();
    let (login, set_login) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (confirm, set_confirm) = signal(String::new());
    let (error, set_error) = signal(None::<String>);
    let (loading, set_loading) = signal(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if password.get() != confirm.get() {
            set_error.set(Some("Passwords do not match.".to_string()));
            return;
        }
        set_loading.set(true);
        set_error.set(None);
        let request = RegisterRequest {
            login: login.get(),
            password: password.get(),
        };

        spawn_ui(async move {
            match ApiClient::new(None).register(&request).await {
                Ok(response) => {
                    auth.set_authenticated(response.user, response.csrf_token);
                    navigate("/app");
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_loading.set(false);
        });
    };

    view! {
        <section class="auth-page">
            <form class="auth-panel" on:submit=submit>
                <h1>"Create account"</h1>
                <TextField label="Login" value=login set_value=set_login />
                <PasswordField label="Password" value=password set_value=set_password />
                <PasswordField label="Confirm password" value=confirm set_value=set_confirm />
                <button type="submit" disabled=loading>
                    {move || if loading.get() { "Creating" } else { "Create account" }}
                </button>
                <ErrorText error=error />
                <div class="auth-links">
                    <a href="/login">"Sign in"</a>
                </div>
            </form>
        </section>
    }
}

#[component]
pub fn BootstrapPage() -> impl IntoView {
    let auth = use_auth();
    let (login, set_login) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (token, set_token) = signal(String::new());
    let (error, set_error) = signal(None::<String>);
    let (loading, set_loading) = signal(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        set_loading.set(true);
        set_error.set(None);
        let request = BootstrapRequest {
            login: login.get(),
            password: password.get(),
            bootstrap_token: token.get(),
        };

        spawn_ui(async move {
            match ApiClient::new(None).bootstrap(&request).await {
                Ok(response) => {
                    auth.set_authenticated(response.user, response.csrf_token);
                    navigate("/app");
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_loading.set(false);
        });
    };

    view! {
        <section class="auth-page">
            <form class="auth-panel" on:submit=submit>
                <h1>"Bootstrap"</h1>
                <TextField label="Login" value=login set_value=set_login />
                <PasswordField label="Password" value=password set_value=set_password />
                <PasswordField label="Bootstrap token" value=token set_value=set_token />
                <p class="notice">"Creates the first admin user."</p>
                <button type="submit" disabled=loading>
                    {move || if loading.get() { "Creating" } else { "Create admin" }}
                </button>
                <ErrorText error=error />
                <div class="auth-links">
                    <a href="/login">"Sign in"</a>
                </div>
            </form>
        </section>
    }
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let auth = use_auth();
    let (current_password, set_current_password) = signal(String::new());
    let (new_password, set_new_password) = signal(String::new());
    let (confirm, set_confirm) = signal(String::new());
    let (message, set_message) = signal(None::<String>);
    let (error, set_error) = signal(None::<String>);
    let (loading, set_loading) = signal(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if new_password.get() != confirm.get() {
            set_error.set(Some("Passwords do not match.".to_string()));
            return;
        }
        set_loading.set(true);
        set_error.set(None);
        set_message.set(None);
        let request = ChangePasswordRequest {
            current_password: current_password.get(),
            new_password: new_password.get(),
        };

        spawn_ui(async move {
            match auth.client().change_password(&request).await {
                Ok(_) => set_message.set(Some("Password changed.".to_string())),
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_loading.set(false);
        });
    };

    let logout = move |_| {
        spawn_ui(async move {
            let _ = auth.client().logout().await;
            auth.clear();
            navigate("/login");
        });
    };

    view! {
        <section class="settings-page">
            <div class="section-header">
                <h1>"Settings"</h1>
                <a class="button secondary" href="/app">"Back"</a>
            </div>
            <div class="settings-grid">
                <section class="panel">
                    <h2>"Account"</h2>
                    {move || {
                        auth.auth.get().user.map(|user| {
                            let role = match user.role {
                                UserRole::Admin => "admin",
                                UserRole::User => "user",
                            };
                            view! {
                                <dl class="meta-list">
                                    <dt>"Login"</dt>
                                    <dd>{user.login}</dd>
                                    <dt>"Role"</dt>
                                    <dd>{role}</dd>
                                </dl>
                            }.into_any()
                        }).unwrap_or_else(|| view! { <p class="muted">"Not signed in."</p> }.into_any())
                    }}
                    <button class="secondary" type="button" on:click=logout>"Logout"</button>
                </section>
                <ModelSettingsPanel />
                <form class="panel" on:submit=submit>
                    <h2>"Change password"</h2>
                    <PasswordField
                        label="Current password"
                        value=current_password
                        set_value=set_current_password
                    />
                    <PasswordField label="New password" value=new_password set_value=set_new_password />
                    <PasswordField label="Confirm password" value=confirm set_value=set_confirm />
                    <button type="submit" disabled=loading>
                        {move || if loading.get() { "Saving" } else { "Save" }}
                    </button>
                    <ErrorText error=error />
                    {move || message.get().map(|text| view! { <p class="success">{text}</p> })}
                </form>
            </div>
        </section>
    }
}

#[component]
fn ModelSettingsPanel() -> impl IntoView {
    let auth = use_auth();
    let (routes, set_routes) = signal(Vec::<ModelRouteView>::new());
    let (provider_available, set_provider_available) = signal(false);
    let (provider_default_model, set_provider_default_model) = signal(None::<String>);
    let (saved_default_model, set_saved_default_model) = signal(None::<String>);
    let (selected_model, set_selected_model) = signal(String::new());
    let (loaded, set_loaded) = signal(false);
    let (loading, set_loading) = signal(false);
    let (refreshing, set_refreshing) = signal(false);
    let (saving, set_saving) = signal(false);
    let (message, set_message) = signal(None::<String>);
    let (error, set_error) = signal(None::<String>);

    let load_model_settings = move || {
        set_loading.set(true);
        set_error.set(None);
        set_message.set(None);

        spawn_ui(async move {
            let settings_result = auth.client().settings().await;
            let routes_result = auth.client().list_model_routes().await;
            match (settings_result, routes_result) {
                (Ok(settings), Ok(model_routes)) => {
                    let saved_default = settings
                        .default_model_selection
                        .map(|selection| selection.qualified_id);
                    let selected = saved_default
                        .clone()
                        .or_else(|| model_routes.default_model_id.clone())
                        .unwrap_or_default();
                    apply_model_routes_response(
                        model_routes,
                        set_routes,
                        set_provider_available,
                        set_provider_default_model,
                    );
                    set_saved_default_model.set(saved_default);
                    set_selected_model.set(selected);
                }
                (Err(error), _) | (_, Err(error)) => set_error.set(Some(error.to_string())),
            }
            set_loading.set(false);
        });
    };

    Effect::new(move |_| {
        if !loaded.get() {
            set_loaded.set(true);
            load_model_settings();
        }
    });

    let refresh_models = move |_| {
        set_refreshing.set(true);
        set_error.set(None);
        set_message.set(None);
        spawn_ui(async move {
            match auth.client().refresh_model_routes().await {
                Ok(model_routes) => {
                    let default_model_id = model_routes.default_model_id.clone();
                    let should_apply_default = selected_model.get().is_empty();
                    apply_model_routes_response(
                        model_routes,
                        set_routes,
                        set_provider_available,
                        set_provider_default_model,
                    );
                    if should_apply_default {
                        if let Some(default_model_id) = default_model_id {
                            set_selected_model.set(default_model_id);
                        }
                    }
                    set_message.set(Some("Models refreshed.".to_string()));
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_refreshing.set(false);
        });
    };

    let save_model_settings = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if !selected_route_is_runnable(routes, selected_model) {
            set_error.set(Some("Select a runnable OpenCode model.".to_string()));
            return;
        }
        set_saving.set(true);
        set_error.set(None);
        set_message.set(None);
        let request = UpdateUserSettingsRequest {
            default_model_selection: Some(ModelSelection {
                qualified_id: selected_model.get(),
            }),
        };

        spawn_ui(async move {
            match auth.client().update_settings(&request).await {
                Ok(settings) => {
                    let saved_default = settings
                        .default_model_selection
                        .map(|selection| selection.qualified_id);
                    if let Some(selection) = saved_default.as_ref() {
                        set_selected_model.set(selection.clone());
                    }
                    set_saved_default_model.set(saved_default);
                    set_message.set(Some(
                        "Web default saved. New sessions use it before .env fallback.".to_string(),
                    ));
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_saving.set(false);
        });
    };

    let selected_source = move || {
        selected_route(routes, selected_model)
            .map(|route| model_route_source_label(route.source).to_string())
            .unwrap_or_else(|| "unavailable".to_string())
    };
    let selected_protocol = move || {
        selected_route(routes, selected_model)
            .map(|route| model_route_protocol_label(route.protocol).to_string())
            .unwrap_or_else(|| "unknown".to_string())
    };
    let selected_status = move || {
        if !provider_available.get() {
            return "provider unavailable".to_string();
        }
        selected_route(routes, selected_model).map_or_else(
            || "unavailable".to_string(),
            |route| {
                if route.runnable {
                    "runnable".to_string()
                } else {
                    "unknown protocol".to_string()
                }
            },
        )
    };
    let save_disabled = move || {
        loading.get() || saving.get() || !selected_route_is_runnable(routes, selected_model)
    };
    let select_disabled = move || loading.get() || saving.get() || routes.get().is_empty();

    view! {
        <form class="panel model-settings-form" on:submit=save_model_settings>
            <h2>"Model"</h2>
            <p class="muted">"Providers: OpenCode Go / Zen Free"</p>
            <p class="muted model-settings-note">
                "Saved web default has priority for new web sessions; .env route remains a fallback."
            </p>
            <label>
                <span>"Web default model"</span>
                <select
                    class="model-route-select"
                    prop:value=selected_model
                    disabled=select_disabled
                    on:change=move |ev| set_selected_model.set(event_target_value(&ev))
                >
                    {move || selected_unavailable_option(routes, selected_model)}
                    <For
                        each=move || routes.get()
                        key=|route| route.qualified_id.clone()
                        children=move |route| {
                            let value = route.qualified_id.clone();
                            view! {
                                <option value=value.clone() disabled=!route.runnable>{value.clone()}</option>
                            }
                        }
                    />
                </select>
            </label>
            <dl class="meta-list model-route-meta">
                <dt>"Source"</dt>
                <dd>{selected_source}</dd>
                <dt>"Protocol"</dt>
                <dd>{selected_protocol}</dd>
                <dt>"Status"</dt>
                <dd>{selected_status}</dd>
                <dt>"Saved web default"</dt>
                <dd>{move || saved_default_model.get().unwrap_or_else(|| "none".to_string())}</dd>
                <dt>"Env fallback"</dt>
                <dd>{move || provider_default_model.get().unwrap_or_else(|| "none".to_string())}</dd>
            </dl>
            <div class="model-settings-actions">
                <button class="btn-primary" type="submit" disabled=save_disabled>
                    {move || if saving.get() { "Saving" } else { "Save" }}
                </button>
                <button
                    class="secondary"
                    type="button"
                    disabled=move || loading.get() || refreshing.get()
                    on:click=refresh_models
                >
                    {move || if refreshing.get() { "Refreshing" } else { "Refresh models" }}
                </button>
            </div>
            {move || {
                if loading.get() {
                    Some(view! { <p class="muted">"Loading model settings..."</p> })
                } else {
                    None
                }
            }}
            <ErrorText error=error />
            {move || message.get().map(|text| view! { <p class="success">{text}</p> })}
        </form>
    }
}

fn apply_model_routes_response(
    response: oxide_agent_web_contracts::ListModelRoutesResponse,
    set_routes: WriteSignal<Vec<ModelRouteView>>,
    set_provider_available: WriteSignal<bool>,
    set_provider_default_model: WriteSignal<Option<String>>,
) {
    set_provider_available.set(response.provider_available);
    set_provider_default_model.set(response.default_model_id);
    set_routes.set(response.routes);
}

fn selected_route(
    routes: ReadSignal<Vec<ModelRouteView>>,
    selected_model: ReadSignal<String>,
) -> Option<ModelRouteView> {
    let selected = selected_model.get();
    routes
        .get()
        .into_iter()
        .find(|route| route.qualified_id == selected)
}

fn selected_route_is_runnable(
    routes: ReadSignal<Vec<ModelRouteView>>,
    selected_model: ReadSignal<String>,
) -> bool {
    selected_route(routes, selected_model).is_some_and(|route| route.runnable)
}

fn selected_unavailable_option(
    routes: ReadSignal<Vec<ModelRouteView>>,
    selected_model: ReadSignal<String>,
) -> Option<impl IntoView> {
    let selected = selected_model.get();
    if selected.is_empty()
        || routes
            .get()
            .iter()
            .any(|route| route.qualified_id == selected)
    {
        return None;
    }
    Some(view! {
        <option value=selected.clone() disabled=true>{format!("{selected} · unavailable")}</option>
    })
}

const fn model_route_source_label(source: ModelRouteSourceView) -> &'static str {
    match source {
        ModelRouteSourceView::Network => "network",
        ModelRouteSourceView::Cache => "cache",
        ModelRouteSourceView::Fallback => "fallback",
    }
}

const fn model_route_protocol_label(protocol: ModelRouteProtocolView) -> &'static str {
    match protocol {
        ModelRouteProtocolView::OpenAiChatCompletions => "openai chat completions",
        ModelRouteProtocolView::AnthropicMessages => "anthropic messages",
        ModelRouteProtocolView::Unknown => "unknown",
    }
}

#[component]
fn TextField(
    label: &'static str,
    value: ReadSignal<String>,
    set_value: WriteSignal<String>,
) -> impl IntoView {
    view! {
        <label>
            <span>{label}</span>
            <input value=value on:input=move |ev| set_value.set(event_target_value(&ev)) />
        </label>
    }
}

#[component]
fn PasswordField(
    label: &'static str,
    value: ReadSignal<String>,
    set_value: WriteSignal<String>,
) -> impl IntoView {
    view! {
        <label>
            <span>{label}</span>
            <input
                type="password"
                value=value
                on:input=move |ev| set_value.set(event_target_value(&ev))
            />
        </label>
    }
}

#[component]
fn ErrorText(error: ReadSignal<Option<String>>) -> impl IntoView {
    view! {
        {move || error.get().map(|text| view! { <p class="error-text">{text}</p> })}
    }
}
