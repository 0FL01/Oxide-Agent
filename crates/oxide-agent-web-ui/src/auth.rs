use crate::api::ApiClient;
use crate::utils::{navigate, spawn_ui};
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    AgentEffort, AgentProfileView, BootstrapRequest, ChangePasswordRequest,
    CreateAgentProfileRequest, CurrentUser, LoginRequest, ModelRouteProtocolView,
    ModelRouteSourceView, ModelRouteView, ModelSelection, RegisterRequest,
    UpdateAgentProfileRequest, UpdateUserSettingsRequest, UserRole,
};

const DEFAULT_PROFILE_NONE: &str = "__none__";
pub const DEFAULT_MAX_TASK_INPUT_CHARS: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthState {
    pub user: Option<CurrentUser>,
    pub csrf_token: Option<String>,
    pub loading: bool,
    pub session_expired: bool,
    pub max_task_input_chars: usize,
    pub large_input_attachments_supported: bool,
}

impl Default for AuthState {
    fn default() -> Self {
        Self {
            user: None,
            csrf_token: None,
            loading: false,
            session_expired: false,
            max_task_input_chars: DEFAULT_MAX_TASK_INPUT_CHARS,
            large_input_attachments_supported: false,
        }
    }
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
        let current = self.auth.get();
        self.set_auth.set(AuthState {
            session_expired: true,
            max_task_input_chars: current.max_task_input_chars,
            large_input_attachments_supported: current.large_input_attachments_supported,
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
                <AgentProfilesPanel />
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
    let (saved_default_profile, set_saved_default_profile) = signal(None::<String>);
    let (saved_default_effort, set_saved_default_effort) = signal(None::<AgentEffort>);
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
                    set_saved_default_profile.set(settings.default_agent_profile_id);
                    set_saved_default_effort.set(settings.default_effort);
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
                    if should_apply_default && let Some(default_model_id) = default_model_id {
                        set_selected_model.set(default_model_id);
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
            set_error.set(Some("Select a runnable model.".to_string()));
            return;
        }
        set_saving.set(true);
        set_error.set(None);
        set_message.set(None);
        let request = UpdateUserSettingsRequest {
            default_model_selection: Some(ModelSelection {
                qualified_id: selected_model.get(),
            }),
            default_agent_profile_id: saved_default_profile.get(),
            default_effort: saved_default_effort.get(),
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
                    set_saved_default_profile.set(settings.default_agent_profile_id);
                    set_saved_default_effort.set(settings.default_effort);
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
    let selected_image_input = move || {
        selected_route(routes, selected_model)
            .map(|route| model_route_image_support_label(route.supports_image_input).to_string())
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
            <p class="muted">"Providers: OpenCode Go / Zen Free / OpenAI Base"</p>
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
                                <option value=value.clone() disabled=!route.runnable>{model_route_option_label(&route)}</option>
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
                <dt>"Image input"</dt>
                <dd>{selected_image_input}</dd>
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

#[component]
fn AgentProfilesPanel() -> impl IntoView {
    let auth = use_auth();
    let (profiles, set_profiles) = signal(Vec::<AgentProfileView>::new());
    let (default_model_selection, set_default_model_selection) = signal(None::<ModelSelection>);
    let (default_effort, set_default_effort) = signal(None::<AgentEffort>);
    let (selected_default_profile, set_selected_default_profile) =
        signal(DEFAULT_PROFILE_NONE.to_string());
    let (editing_profile_id, set_editing_profile_id) = signal(None::<String>);
    let (display_name, set_display_name) = signal(String::new());
    let (system_prompt, set_system_prompt) = signal(String::new());
    let (loaded, set_loaded) = signal(false);
    let (loading, set_loading) = signal(false);
    let (saving_profile, set_saving_profile) = signal(false);
    let (saving_default, set_saving_default) = signal(false);
    let (message, set_message) = signal(None::<String>);
    let (error, set_error) = signal(None::<String>);

    let load_profiles = move || {
        set_loading.set(true);
        set_error.set(None);
        set_message.set(None);
        spawn_ui(async move {
            let settings_result = auth.client().settings().await;
            let profiles_result = auth.client().list_agent_profiles().await;
            match (settings_result, profiles_result) {
                (Ok(settings), Ok(response)) => {
                    set_default_model_selection.set(settings.default_model_selection);
                    set_default_effort.set(settings.default_effort);
                    set_selected_default_profile.set(
                        settings
                            .default_agent_profile_id
                            .unwrap_or_else(|| DEFAULT_PROFILE_NONE.to_string()),
                    );
                    set_profiles.set(response.profiles);
                }
                (Err(error), _) | (_, Err(error)) => set_error.set(Some(error.to_string())),
            }
            set_loading.set(false);
        });
    };

    Effect::new(move |_| {
        if !loaded.get() {
            set_loaded.set(true);
            load_profiles();
        }
    });

    let reset_form = move || {
        set_editing_profile_id.set(None);
        set_display_name.set(String::new());
        set_system_prompt.set(String::new());
    };

    let save_profile = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        set_saving_profile.set(true);
        set_error.set(None);
        set_message.set(None);
        let name = display_name.get();
        let prompt = system_prompt.get();
        let editing = editing_profile_id.get();

        spawn_ui(async move {
            let client = auth.client();
            let result = if let Some(agent_id) = editing.as_deref() {
                client
                    .update_agent_profile(
                        agent_id,
                        &UpdateAgentProfileRequest {
                            display_name: name,
                            system_prompt: prompt,
                        },
                    )
                    .await
                    .map(|response| response.profile)
            } else {
                client
                    .create_agent_profile(&CreateAgentProfileRequest {
                        display_name: name,
                        system_prompt: prompt,
                    })
                    .await
                    .map(|response| response.profile)
            };

            match result {
                Ok(profile) => {
                    set_profiles.update(|profiles| upsert_agent_profile_view(profiles, profile));
                    reset_form();
                    set_message.set(Some("Agent profile saved.".to_string()));
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_saving_profile.set(false);
        });
    };

    let save_default_profile = move |_| {
        set_saving_default.set(true);
        set_error.set(None);
        set_message.set(None);
        let request = UpdateUserSettingsRequest {
            default_model_selection: default_model_selection.get(),
            default_agent_profile_id: default_profile_value_to_id(&selected_default_profile.get()),
            default_effort: default_effort.get(),
        };
        spawn_ui(async move {
            match auth.client().update_settings(&request).await {
                Ok(settings) => {
                    set_default_model_selection.set(settings.default_model_selection);
                    set_default_effort.set(settings.default_effort);
                    set_selected_default_profile.set(
                        settings
                            .default_agent_profile_id
                            .unwrap_or_else(|| DEFAULT_PROFILE_NONE.to_string()),
                    );
                    set_message.set(Some("Default profile saved.".to_string()));
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_saving_default.set(false);
        });
    };

    let read_prompt_file = move |ev: leptos::ev::Event| {
        use wasm_bindgen::JsCast;
        let Some(target) = ev.target() else {
            return;
        };
        let input: web_sys::HtmlInputElement = target.unchecked_into();
        let Some(files) = input.files() else {
            return;
        };
        let Some(file) = files.get(0) else {
            return;
        };
        spawn_ui(async move {
            match wasm_bindgen_futures::JsFuture::from(file.text()).await {
                Ok(value) => {
                    if let Some(text) = value.as_string() {
                        set_system_prompt.set(text);
                    } else {
                        set_error.set(Some("Selected file did not contain text.".to_string()));
                    }
                }
                Err(_) => set_error.set(Some("Failed to read selected file.".to_string())),
            }
        });
    };

    view! {
        <section class="panel agent-profiles-panel">
            <h2>"Agent profiles"</h2>
            <p class="muted">"V1 stores only additional system prompt instructions."</p>
            <label>
                <span>"Default for new chats"</span>
                <select
                    prop:value=selected_default_profile
                    disabled=move || loading.get() || saving_default.get()
                    on:change=move |ev| set_selected_default_profile.set(event_target_value(&ev))
                >
                    <option value=DEFAULT_PROFILE_NONE>"No default profile"</option>
                    <For
                        each=move || profiles.get()
                        key=|profile| profile.agent_id.clone()
                        children=move |profile| {
                            let value = profile.agent_id.clone();
                            view! { <option value=value.clone()>{profile.display_name}</option> }
                        }
                    />
                </select>
            </label>
            <button
                class="secondary"
                type="button"
                disabled=move || loading.get() || saving_default.get()
                on:click=save_default_profile
            >
                {move || if saving_default.get() { "Saving default" } else { "Save default" }}
            </button>

            <form class="agent-profile-form" on:submit=save_profile>
                <h3>{move || if editing_profile_id.get().is_some() { "Edit profile" } else { "Create profile" }}</h3>
                <label>
                    <span>"Name"</span>
                    <input
                        prop:value=display_name
                        disabled=saving_profile
                        on:input=move |ev| set_display_name.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    <span>"System prompt"</span>
                    <textarea
                        prop:value=system_prompt
                        disabled=saving_profile
                        rows="8"
                        on:input=move |ev| set_system_prompt.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    <span>"Upload prompt"</span>
                    <input
                        type="file"
                        accept=".txt,.md,text/plain,text/markdown"
                        disabled=saving_profile
                        on:change=read_prompt_file
                    />
                </label>
                <div class="model-settings-actions">
                    <button class="btn-primary" type="submit" disabled=saving_profile>
                        {move || if saving_profile.get() { "Saving" } else { "Save profile" }}
                    </button>
                    <button class="secondary" type="button" on:click=move |_| reset_form()>
                        "Clear form"
                    </button>
                </div>
            </form>

            <div class="agent-profile-list">
                <For
                    each=move || profiles.get()
                    key=|profile| profile.agent_id.clone()
                    children=move |profile| {
                        let edit_profile = profile.clone();
                        let delete_profile = profile.clone();
                        view! {
                            <article class="agent-profile-list-item">
                                <strong>{profile.display_name.clone()}</strong>
                                <p class="muted">{profile.agent_id.clone()}</p>
                                <div class="model-settings-actions">
                                    <button
                                        class="secondary"
                                        type="button"
                                        on:click=move |_| {
                                            set_editing_profile_id.set(Some(edit_profile.agent_id.clone()));
                                            set_display_name.set(edit_profile.display_name.clone());
                                            set_system_prompt.set(edit_profile.system_prompt.clone());
                                        }
                                    >"Edit"</button>
                                    <button
                                        class="btn-danger"
                                        type="button"
                                        on:click=move |_| {
                                            let agent_id = delete_profile.agent_id.clone();
                                            set_error.set(None);
                                            set_message.set(None);
                                            spawn_ui(async move {
                                                match auth.client().delete_agent_profile(&agent_id).await {
                                                    Ok(_) => {
                                                        set_profiles.update(|profiles| {
                                                            profiles.retain(|profile| profile.agent_id != agent_id);
                                                        });
                                                        if editing_profile_id.get().as_deref() == Some(agent_id.as_str()) {
                                                            reset_form();
                                                        }
                                                        if selected_default_profile.get() == agent_id {
                                                            set_selected_default_profile.set(DEFAULT_PROFILE_NONE.to_string());
                                                        }
                                                        set_message.set(Some("Agent profile deleted.".to_string()));
                                                    }
                                                    Err(error) => set_error.set(Some(error.to_string())),
                                                }
                                            });
                                        }
                                    >"Delete"</button>
                                </div>
                            </article>
                        }
                    }
                />
            </div>
            {move || loading.get().then(|| view! { <p class="muted">"Loading profiles..."</p> })}
            <ErrorText error=error />
            {move || message.get().map(|text| view! { <p class="success">{text}</p> })}
        </section>
    }
}

fn default_profile_value_to_id(value: &str) -> Option<String> {
    (value != DEFAULT_PROFILE_NONE && !value.trim().is_empty()).then(|| value.to_string())
}

fn upsert_agent_profile_view(profiles: &mut Vec<AgentProfileView>, profile: AgentProfileView) {
    if let Some(existing) = profiles
        .iter_mut()
        .find(|existing| existing.agent_id == profile.agent_id)
    {
        *existing = profile;
    } else {
        profiles.push(profile);
    }
    profiles.sort_by(|left, right| {
        left.display_name
            .to_ascii_lowercase()
            .cmp(&right.display_name.to_ascii_lowercase())
            .then_with(|| left.agent_id.cmp(&right.agent_id))
    });
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

fn model_route_option_label(route: &ModelRouteView) -> String {
    if route.supports_image_input {
        format!("{} · image", route.qualified_id)
    } else {
        route.qualified_id.clone()
    }
}

const fn model_route_image_support_label(supports_image_input: bool) -> &'static str {
    if supports_image_input {
        "supported"
    } else {
        "text only"
    }
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
