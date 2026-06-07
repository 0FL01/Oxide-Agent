//! Authentication helpers for the web console.

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use oxide_agent_web_contracts::{
    BootstrapRequest, ChangePasswordRequest, CurrentUser, LoginRequest, RegisterRequest, UserRole,
};
use sha2::{Digest, Sha256};
use std::time::Instant;
use uuid::Uuid;

use crate::persistence::{
    WEB_AUTH_SCHEMA_VERSION, WebAuthSessionRecord, WebUiStore, WebUiStoreError, WebUserRecord,
    WebUserStatus,
};

const LOGIN_MIN_LEN: usize = 3;
const LOGIN_MAX_LEN: usize = 64;
const PASSWORD_MIN_LEN: usize = 12;
const PASSWORD_MAX_LEN: usize = 1024;
const USER_ID_ATTEMPTS: usize = 16;
pub const AUTH_SESSION_TTL_SECS: i64 = 60 * 60 * 24 * 14;
const WEB_LATENCY_TARGET: &str = "oxide_agent_transport_web::web_latency";

fn log_auth_store_phase(phase: &'static str, started_at: Instant, user_id: Option<i64>) {
    tracing::debug!(
        target: WEB_LATENCY_TARGET,
        phase,
        user_id = ?user_id,
        elapsed_ms = started_at.elapsed().as_millis(),
        "web auth store latency"
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    Validation(String),
    Unauthorized,
    InvalidCredentials,
    CsrfInvalid,
    RegistrationDisabled,
    BootstrapUnavailable,
    Conflict(String),
    StoreUnavailable(String),
}

pub fn normalize_login(login: &str) -> Result<String, AuthError> {
    let login = login.trim();
    if login.len() < LOGIN_MIN_LEN || login.len() > LOGIN_MAX_LEN {
        return Err(AuthError::Validation(format!(
            "login must be {LOGIN_MIN_LEN}-{LOGIN_MAX_LEN} ASCII characters"
        )));
    }
    if !login
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(AuthError::Validation(
            "login may contain only ASCII letters, digits, '.', '_' and '-'".to_string(),
        ));
    }
    Ok(login.to_ascii_lowercase())
}

pub fn validate_password(password: &str) -> Result<(), AuthError> {
    if password.len() < PASSWORD_MIN_LEN {
        return Err(AuthError::Validation(format!(
            "password must be at least {PASSWORD_MIN_LEN} characters"
        )));
    }
    if password.len() > PASSWORD_MAX_LEN {
        return Err(AuthError::Validation(format!(
            "password must be at most {PASSWORD_MAX_LEN} characters"
        )));
    }
    Ok(())
}

pub fn hash_password(password: &str) -> Result<String, AuthError> {
    validate_password(password)?;
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| AuthError::StoreUnavailable(format!("password hashing failed: {error}")))
}

pub fn verify_password(password_hash: &str, password: &str) -> bool {
    let Ok(parsed_hash) = PasswordHash::new(password_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

#[must_use]
pub fn generate_opaque_token() -> String {
    URL_SAFE_NO_PAD.encode(Uuid::new_v4().as_bytes())
}

#[must_use]
pub fn hash_session_token(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

pub async fn register_user(
    store: &dyn WebUiStore,
    request: RegisterRequest,
    registration_enabled: bool,
    now: DateTime<Utc>,
) -> Result<CurrentUser, AuthError> {
    if !registration_enabled {
        return Err(AuthError::RegistrationDisabled);
    }
    let users_count = store.users_count().await.map_err(map_store_error)?;
    let role = if users_count == 0 {
        UserRole::Admin
    } else {
        UserRole::User
    };
    create_user(store, request.login, request.password, role, now).await
}

pub async fn bootstrap_user(
    store: &dyn WebUiStore,
    request: BootstrapRequest,
    configured_token: Option<&str>,
    now: DateTime<Utc>,
) -> Result<CurrentUser, AuthError> {
    let Some(configured_token) = configured_token.filter(|token| !token.trim().is_empty()) else {
        return Err(AuthError::BootstrapUnavailable);
    };
    if store.users_count().await.map_err(map_store_error)? != 0 {
        return Err(AuthError::BootstrapUnavailable);
    }
    if request.bootstrap_token != configured_token {
        return Err(AuthError::InvalidCredentials);
    }
    create_user(store, request.login, request.password, UserRole::Admin, now).await
}

pub async fn login_user(
    store: &dyn WebUiStore,
    request: LoginRequest,
    now: DateTime<Utc>,
) -> Result<(CurrentUser, WebAuthSessionRecord, String), AuthError> {
    let normalized_login =
        normalize_login(&request.login).map_err(|_| AuthError::InvalidCredentials)?;
    let Some(index) = store
        .load_login_index(&normalized_login)
        .await
        .map_err(map_store_error)?
    else {
        return Err(AuthError::InvalidCredentials);
    };
    let Some(user) = store
        .load_user(index.user_id)
        .await
        .map_err(map_store_error)?
    else {
        return Err(AuthError::InvalidCredentials);
    };
    if user.status != WebUserStatus::Active
        || !verify_password(&user.password_hash, &request.password)
    {
        return Err(AuthError::InvalidCredentials);
    }

    let (auth_session, raw_session_token) =
        create_auth_session_for_user(store, user.user_id, now).await?;
    Ok((
        current_user_from_record(&user),
        auth_session,
        raw_session_token,
    ))
}

pub async fn create_auth_session_for_user(
    store: &dyn WebUiStore,
    user_id: i64,
    now: DateTime<Utc>,
) -> Result<(WebAuthSessionRecord, String), AuthError> {
    let raw_session_token = generate_opaque_token();
    let auth_session = WebAuthSessionRecord {
        schema_version: WEB_AUTH_SCHEMA_VERSION,
        session_token_hash: hash_session_token(&raw_session_token),
        user_id,
        csrf_token: generate_opaque_token(),
        created_at: now,
        last_seen_at: now,
        expires_at: now + chrono::Duration::seconds(AUTH_SESSION_TTL_SECS),
        revoked_at: None,
    };
    store
        .save_auth_session(auth_session.clone())
        .await
        .map_err(map_store_error)?;
    Ok((auth_session, raw_session_token))
}

pub async fn current_user_for_token(
    store: &dyn WebUiStore,
    raw_session_token: &str,
    now: DateTime<Utc>,
) -> Result<(CurrentUser, WebAuthSessionRecord), AuthError> {
    let session_token_hash = hash_session_token(raw_session_token);
    let started_at = Instant::now();
    let Some(mut auth_session) = store
        .load_auth_session(&session_token_hash)
        .await
        .map_err(map_store_error)?
    else {
        log_auth_store_phase("load_auth_session", started_at, None);
        return Err(AuthError::Unauthorized);
    };
    log_auth_store_phase("load_auth_session", started_at, Some(auth_session.user_id));
    if auth_session.revoked_at.is_some() || auth_session.expires_at <= now {
        return Err(AuthError::Unauthorized);
    }
    let started_at = Instant::now();
    let Some(user) = store
        .load_user(auth_session.user_id)
        .await
        .map_err(map_store_error)?
    else {
        log_auth_store_phase("load_user", started_at, Some(auth_session.user_id));
        return Err(AuthError::Unauthorized);
    };
    log_auth_store_phase("load_user", started_at, Some(auth_session.user_id));
    if user.status != WebUserStatus::Active {
        return Err(AuthError::Unauthorized);
    }
    auth_session.last_seen_at = now;
    let started_at = Instant::now();
    store
        .save_auth_session(auth_session.clone())
        .await
        .map_err(map_store_error)?;
    log_auth_store_phase("save_auth_session", started_at, Some(auth_session.user_id));
    Ok((current_user_from_record(&user), auth_session))
}

pub async fn logout_session(
    store: &dyn WebUiStore,
    raw_session_token: &str,
    csrf_token: &str,
    now: DateTime<Utc>,
) -> Result<(), AuthError> {
    let (_, auth_session) = current_user_for_token(store, raw_session_token, now).await?;
    if auth_session.csrf_token != csrf_token {
        return Err(AuthError::CsrfInvalid);
    }
    store
        .revoke_auth_session(&auth_session.session_token_hash, now)
        .await
        .map_err(map_store_error)?;
    Ok(())
}

pub async fn change_password(
    store: &dyn WebUiStore,
    raw_session_token: &str,
    csrf_token: &str,
    request: ChangePasswordRequest,
    now: DateTime<Utc>,
) -> Result<(), AuthError> {
    let (current_user, auth_session) =
        current_user_for_token(store, raw_session_token, now).await?;
    if auth_session.csrf_token != csrf_token {
        return Err(AuthError::CsrfInvalid);
    }
    let Some(mut user) = store
        .load_user(current_user.user_id)
        .await
        .map_err(map_store_error)?
    else {
        return Err(AuthError::Unauthorized);
    };
    if !verify_password(&user.password_hash, &request.current_password) {
        return Err(AuthError::InvalidCredentials);
    }

    user.password_hash = hash_password(&request.new_password)?;
    user.updated_at = now;
    store.save_user(user).await.map_err(map_store_error)?;
    store
        .revoke_auth_sessions_for_user_except(
            current_user.user_id,
            &auth_session.session_token_hash,
            now,
        )
        .await
        .map_err(map_store_error)?;
    Ok(())
}

async fn create_user(
    store: &dyn WebUiStore,
    login: String,
    password: String,
    role: UserRole,
    now: DateTime<Utc>,
) -> Result<CurrentUser, AuthError> {
    let normalized_login = normalize_login(&login)?;
    if store
        .load_login_index(&normalized_login)
        .await
        .map_err(map_store_error)?
        .is_some()
    {
        return Err(AuthError::Conflict("login already exists".to_string()));
    }

    let user_id = allocate_user_id(store).await?;
    let password_hash = hash_password(&password)?;
    let display_login = login.clone();
    let record = WebUserRecord {
        schema_version: WEB_AUTH_SCHEMA_VERSION,
        user_id,
        login,
        normalized_login,
        password_hash,
        role,
        status: WebUserStatus::Active,
        default_model_selection: None,
        default_agent_profile_id: None,
        default_effort: None,
        created_at: now,
        updated_at: now,
        last_login_at: None,
    };
    store.save_user(record).await.map_err(map_store_error)?;
    Ok(CurrentUser {
        user_id,
        login: display_login,
        role,
    })
}

async fn allocate_user_id(store: &dyn WebUiStore) -> Result<i64, AuthError> {
    for _ in 0..USER_ID_ATTEMPTS {
        let user_id = random_positive_i64();
        if store
            .load_user(user_id)
            .await
            .map_err(map_store_error)?
            .is_none()
        {
            return Ok(user_id);
        }
    }
    Err(AuthError::StoreUnavailable(
        "could not allocate a unique web user id".to_string(),
    ))
}

fn random_positive_i64() -> i64 {
    let uuid = Uuid::new_v4();
    let bytes = uuid.as_bytes();
    let mut id_bytes = [0_u8; 8];
    id_bytes.copy_from_slice(&bytes[..8]);
    i64::from_be_bytes(id_bytes) & i64::MAX
}

fn map_store_error(error: WebUiStoreError) -> AuthError {
    match error {
        WebUiStoreError::Conflict(message) => AuthError::Conflict(message),
        WebUiStoreError::Unavailable(message) => AuthError::StoreUnavailable(message),
    }
}

fn current_user_from_record(user: &WebUserRecord) -> CurrentUser {
    CurrentUser {
        user_id: user.user_id,
        login: user.login.clone(),
        role: user.role,
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use oxide_agent_web_contracts::{
        BootstrapRequest, ChangePasswordRequest, LoginRequest, RegisterRequest, UserRole,
    };

    use crate::auth::{
        AuthError, bootstrap_user, change_password, current_user_for_token, hash_password,
        hash_session_token, login_user, logout_session, normalize_login, register_user,
        validate_password, verify_password,
    };
    use crate::persistence::{InMemoryWebUiStore, WebUiStore, WebUserStatus};

    #[test]
    fn login_normalization_is_ascii_only_and_case_insensitive() {
        assert_eq!(
            normalize_login(" Alice.Name_1 ").expect("valid login"),
            "alice.name_1"
        );
        assert!(matches!(
            normalize_login("алиса"),
            Err(AuthError::Validation(_))
        ));
        assert!(matches!(
            normalize_login("has space"),
            Err(AuthError::Validation(_))
        ));
    }

    #[test]
    fn password_hash_uses_argon2id_and_verifies() {
        let hash = hash_password("correct horse battery staple").expect("hash password");
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password(&hash, "correct horse battery staple"));
        assert!(!verify_password(&hash, "wrong password"));
        assert!(matches!(
            validate_password("short"),
            Err(AuthError::Validation(_))
        ));
    }

    #[tokio::test]
    async fn registration_creates_first_admin_then_users() {
        let store = InMemoryWebUiStore::new();
        let admin = register_user(
            &store,
            RegisterRequest {
                login: "admin".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            true,
            Utc::now(),
        )
        .await
        .expect("first registration succeeds");
        assert_eq!(admin.role, UserRole::Admin);

        let user = register_user(
            &store,
            RegisterRequest {
                login: "alice".to_string(),
                password: "another strong passphrase".to_string(),
            },
            true,
            Utc::now(),
        )
        .await
        .expect("second registration succeeds");
        assert_eq!(user.role, UserRole::User);
        assert_eq!(store.users_count().await.expect("count users"), 2);
    }

    #[tokio::test]
    async fn registration_disabled_and_bootstrap_token_are_enforced() {
        let store = InMemoryWebUiStore::new();
        let request = RegisterRequest {
            login: "admin".to_string(),
            password: "correct horse battery staple".to_string(),
        };
        assert!(matches!(
            register_user(&store, request, false, Utc::now()).await,
            Err(AuthError::RegistrationDisabled)
        ));

        let bootstrap = BootstrapRequest {
            login: "admin".to_string(),
            password: "correct horse battery staple".to_string(),
            bootstrap_token: "secret".to_string(),
        };
        assert!(matches!(
            bootstrap_user(&store, bootstrap.clone(), Some("wrong"), Utc::now()).await,
            Err(AuthError::InvalidCredentials)
        ));
        let user = bootstrap_user(&store, bootstrap, Some("secret"), Utc::now())
            .await
            .expect("bootstrap succeeds");
        assert_eq!(user.role, UserRole::Admin);
    }

    #[tokio::test]
    async fn login_creates_hashed_browser_session_and_current_user() {
        let store = InMemoryWebUiStore::new();
        let now = Utc::now();
        register_user(
            &store,
            RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register user");

        let (user, session, raw_token) = login_user(
            &store,
            LoginRequest {
                login: "ALICE".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login user");

        assert_eq!(user.login, "alice");
        assert_ne!(session.session_token_hash, raw_token);
        assert_eq!(session.session_token_hash, hash_session_token(&raw_token));

        let (current_user, current_session) = current_user_for_token(&store, &raw_token, now)
            .await
            .expect("current user");
        assert_eq!(current_user.user_id, user.user_id);
        assert_eq!(current_session.csrf_token, session.csrf_token);
    }

    #[tokio::test]
    async fn disabled_user_and_logout_invalidate_browser_session() {
        let store = InMemoryWebUiStore::new();
        let now = Utc::now();
        let user = register_user(
            &store,
            RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register user");
        let (_, session, raw_token) = login_user(
            &store,
            LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("login user");

        assert!(matches!(
            logout_session(&store, &raw_token, "wrong", now).await,
            Err(AuthError::CsrfInvalid)
        ));
        logout_session(&store, &raw_token, &session.csrf_token, now)
            .await
            .expect("logout succeeds");
        assert!(matches!(
            current_user_for_token(&store, &raw_token, now).await,
            Err(AuthError::Unauthorized)
        ));

        let mut record = store
            .load_user(user.user_id)
            .await
            .expect("load user")
            .expect("user exists");
        record.status = WebUserStatus::Disabled;
        store.save_user(record).await.expect("save disabled user");
        assert!(matches!(
            login_user(
                &store,
                LoginRequest {
                    login: "alice".to_string(),
                    password: "correct horse battery staple".to_string(),
                },
                now,
            )
            .await,
            Err(AuthError::InvalidCredentials)
        ));
    }

    #[tokio::test]
    async fn change_password_updates_hash_and_revokes_other_sessions() {
        let store = InMemoryWebUiStore::new();
        let now = Utc::now();
        register_user(
            &store,
            RegisterRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            true,
            now,
        )
        .await
        .expect("register user");
        let (_, session_one, token_one) = login_user(
            &store,
            LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("first login");
        let (_, _, token_two) = login_user(
            &store,
            LoginRequest {
                login: "alice".to_string(),
                password: "correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("second login");

        change_password(
            &store,
            &token_one,
            &session_one.csrf_token,
            ChangePasswordRequest {
                current_password: "correct horse battery staple".to_string(),
                new_password: "new correct horse battery staple".to_string(),
            },
            now,
        )
        .await
        .expect("change password");

        assert!(
            current_user_for_token(&store, &token_one, now)
                .await
                .is_ok()
        );
        assert!(matches!(
            current_user_for_token(&store, &token_two, now).await,
            Err(AuthError::Unauthorized)
        ));
        assert!(matches!(
            login_user(
                &store,
                LoginRequest {
                    login: "alice".to_string(),
                    password: "correct horse battery staple".to_string(),
                },
                now,
            )
            .await,
            Err(AuthError::InvalidCredentials)
        ));
        assert!(
            login_user(
                &store,
                LoginRequest {
                    login: "alice".to_string(),
                    password: "new correct horse battery staple".to_string(),
                },
                now,
            )
            .await
            .is_ok()
        );
    }
}
