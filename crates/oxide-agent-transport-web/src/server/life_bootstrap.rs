use crate::auth::normalize_login;
use crate::persistence::WebUiStore;

use super::WebStartupError;
use chrono::Utc;
use oxide_agent_life::domain::{
    BindingId, LifeIdentityLink, LifePrincipal, LifeTransportBinding, LifeTransportId,
    PrincipalUserId, ProviderSubject, TELEGRAM_TRANSPORT_ID, WEB_TRANSPORT_ID,
};
use oxide_agent_life::storage::{LifeStorageRepository, SqlxLifeStorage};
use serde_json::json;

const LIFE_OWNER_WEB_LOGIN_ENV: &str = "LIFE_OWNER_WEB_LOGIN";
const LIFE_TELEGRAM_CHAT_ID_ENV: &str = "LIFE_TELEGRAM_CHAT_ID";
const LIFE_TELEGRAM_BOT_TOKEN_ENV: &str = "LIFE_TELEGRAM_BOT_TOKEN";

#[derive(Debug, Clone, PartialEq, Eq)]
struct SoloBridgeBootstrapConfig {
    owner_web_login: String,
    telegram_chat_id: Option<String>,
}

impl SoloBridgeBootstrapConfig {
    fn from_env() -> Result<Option<Self>, WebStartupError> {
        let owner_web_login = trimmed_env(LIFE_OWNER_WEB_LOGIN_ENV);
        let telegram_chat_id = trimmed_env(LIFE_TELEGRAM_CHAT_ID_ENV);
        let telegram_token_configured = trimmed_env(LIFE_TELEGRAM_BOT_TOKEN_ENV).is_some();

        if owner_web_login.is_none() && telegram_chat_id.is_none() && !telegram_token_configured {
            return Ok(None);
        }

        let Some(owner_web_login) = owner_web_login else {
            return Err(WebStartupError::StoreUnavailable(format!(
                "{LIFE_OWNER_WEB_LOGIN_ENV} must be configured when Life bridge Telegram env is configured"
            )));
        };

        if telegram_token_configured && telegram_chat_id.is_none() {
            return Err(WebStartupError::StoreUnavailable(format!(
                "{LIFE_TELEGRAM_CHAT_ID_ENV} must be configured when {LIFE_TELEGRAM_BOT_TOKEN_ENV} is configured"
            )));
        }

        Ok(Some(Self {
            owner_web_login,
            telegram_chat_id,
        }))
    }
}

pub(crate) async fn bootstrap_life_solo_bridge_from_env(
    web_store: &dyn WebUiStore,
    life_storage: &SqlxLifeStorage,
) -> Result<(), WebStartupError> {
    let Some(config) = SoloBridgeBootstrapConfig::from_env()? else {
        return Ok(());
    };

    bootstrap_life_solo_bridge(web_store, life_storage, config).await
}

async fn bootstrap_life_solo_bridge(
    web_store: &dyn WebUiStore,
    life_storage: &SqlxLifeStorage,
    config: SoloBridgeBootstrapConfig,
) -> Result<(), WebStartupError> {
    let normalized_login =
        normalize_login(&config.owner_web_login).map_err(|error| match error {
            crate::auth::AuthError::Validation(message) => WebStartupError::StoreUnavailable(
                format!("invalid {LIFE_OWNER_WEB_LOGIN_ENV}: {message}"),
            ),
            other => WebStartupError::StoreUnavailable(format!(
                "invalid {LIFE_OWNER_WEB_LOGIN_ENV}: {other:?}"
            )),
        })?;
    let login_index = web_store
        .load_login_index(&normalized_login)
        .await
        .map_err(|error| WebStartupError::StoreUnavailable(error.to_string()))?
        .ok_or_else(|| {
            WebStartupError::StoreUnavailable(format!(
                "{LIFE_OWNER_WEB_LOGIN_ENV} user '{normalized_login}' does not exist"
            ))
        })?;

    let principal_user_id = PrincipalUserId::new(login_index.user_id)
        .map_err(|error| WebStartupError::StoreUnavailable(error.to_string()))?;
    let now = oxide_agent_life::domain::TimestampMillis::new(Utc::now().timestamp_millis());
    let principal = LifePrincipal {
        principal_user_id,
        profile_state: json!({}),
        operating_profile: json!({}),
        settings: json!({}),
        schema_version: 1,
        created_at: now,
        updated_at: now,
    };
    life_storage
        .upsert_principal(&principal)
        .await
        .map_err(|error| WebStartupError::StoreUnavailable(error.to_string()))?;

    let web_transport = LifeTransportId::new(WEB_TRANSPORT_ID)
        .map_err(|error| WebStartupError::StoreUnavailable(error.to_string()))?;
    let web_subject = ProviderSubject::new(login_index.user_id.to_string())
        .map_err(|error| WebStartupError::StoreUnavailable(error.to_string()))?;
    upsert_identity_and_binding(
        life_storage,
        principal_user_id,
        web_transport,
        web_subject,
        json!({ "user_id": login_index.user_id }),
        json!({ "user_id": login_index.user_id, "login": normalized_login }),
        now,
    )
    .await?;

    if let Some(chat_id) = config.telegram_chat_id {
        let telegram_transport = LifeTransportId::new(TELEGRAM_TRANSPORT_ID)
            .map_err(|error| WebStartupError::StoreUnavailable(error.to_string()))?;
        let telegram_subject = ProviderSubject::new(chat_id.clone())
            .map_err(|error| WebStartupError::StoreUnavailable(error.to_string()))?;
        upsert_identity_and_binding(
            life_storage,
            principal_user_id,
            telegram_transport,
            telegram_subject,
            json!({ "chat_id": chat_id }),
            json!({ "chat_id": chat_id }),
            now,
        )
        .await?;
    }

    Ok(())
}

async fn upsert_identity_and_binding(
    life_storage: &SqlxLifeStorage,
    principal_user_id: PrincipalUserId,
    transport_id: LifeTransportId,
    provider_subject: ProviderSubject,
    inbound_address: serde_json::Value,
    delivery_address: serde_json::Value,
    now: oxide_agent_life::domain::TimestampMillis,
) -> Result<(), WebStartupError> {
    life_storage
        .link_identity(&LifeIdentityLink {
            transport_id: transport_id.clone(),
            provider_subject,
            principal_user_id,
            verified_at: Some(now),
            created_at: now,
            updated_at: now,
        })
        .await
        .map_err(|error| WebStartupError::StoreUnavailable(error.to_string()))?;

    life_storage
        .upsert_transport_binding(&LifeTransportBinding {
            binding_id: BindingId::new_v4(),
            principal_user_id,
            transport_id,
            inbound_address,
            delivery_address,
            enabled: true,
            created_at: now,
            updated_at: now,
        })
        .await
        .map_err(|error| WebStartupError::StoreUnavailable(error.to_string()))
}

fn trimmed_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn test_set_env(key: impl AsRef<std::ffi::OsStr>, value: impl AsRef<std::ffi::OsStr>) {
        unsafe { std::env::set_var(key, value) };
    }

    fn test_remove_env(key: impl AsRef<std::ffi::OsStr>) {
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn bootstrap_config_is_noop_when_unconfigured() {
        let _guard = ENV_LOCK.lock().expect("env test lock");
        test_remove_env(LIFE_OWNER_WEB_LOGIN_ENV);
        test_remove_env(LIFE_TELEGRAM_CHAT_ID_ENV);
        test_remove_env(LIFE_TELEGRAM_BOT_TOKEN_ENV);

        assert_eq!(
            SoloBridgeBootstrapConfig::from_env().expect("unconfigured bootstrap should parse"),
            None
        );
    }

    #[test]
    fn bootstrap_config_requires_owner_for_telegram_binding() {
        let _guard = ENV_LOCK.lock().expect("env test lock");
        test_remove_env(LIFE_OWNER_WEB_LOGIN_ENV);
        test_set_env(LIFE_TELEGRAM_CHAT_ID_ENV, "424242");
        test_remove_env(LIFE_TELEGRAM_BOT_TOKEN_ENV);

        let error = SoloBridgeBootstrapConfig::from_env()
            .expect_err("telegram binding without owner must fail");
        assert!(error.to_string().contains(LIFE_OWNER_WEB_LOGIN_ENV));

        test_remove_env(LIFE_TELEGRAM_CHAT_ID_ENV);
    }

    #[test]
    fn bootstrap_config_never_exposes_bot_token_value() {
        let _guard = ENV_LOCK.lock().expect("env test lock");
        test_set_env(LIFE_OWNER_WEB_LOGIN_ENV, "alice");
        test_remove_env(LIFE_TELEGRAM_CHAT_ID_ENV);
        test_set_env(LIFE_TELEGRAM_BOT_TOKEN_ENV, "123456:SECRET");

        let error = SoloBridgeBootstrapConfig::from_env()
            .expect_err("bot token without chat binding must fail");
        let message = error.to_string();
        assert!(message.contains(LIFE_TELEGRAM_CHAT_ID_ENV));
        assert!(!message.contains("SECRET"));

        test_remove_env(LIFE_OWNER_WEB_LOGIN_ENV);
        test_remove_env(LIFE_TELEGRAM_BOT_TOKEN_ENV);
    }
}
