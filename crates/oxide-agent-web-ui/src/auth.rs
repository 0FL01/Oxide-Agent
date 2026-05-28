use crate::api::ApiClient;
use crate::utils::{navigate, spawn_ui};
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    BootstrapRequest, ChangePasswordRequest, CurrentUser, LoginRequest, RegisterRequest, UserRole,
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
