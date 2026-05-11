use crate::llm::support::http::{create_http_client, APP_USER_AGENT};
use crate::llm::LlmError;
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Utc;
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

const CHATGPT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CHATGPT_DEVICE_AUTH_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const CHATGPT_DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const CHATGPT_OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CHATGPT_DEVICE_VERIFY_URL: &str = "https://auth.openai.com/codex/device";
const CHATGPT_DEVICE_CALLBACK_URL: &str = "https://auth.openai.com/deviceauth/callback";
const OAUTH_POLLING_SAFETY_MARGIN_MS: u64 = 3_000;
const DEFAULT_AUTH_FILE_PATH: &str = "config/chatgpt/auth.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ChatGptAuthFile {
    openai: ChatGptStoredAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ChatGptStoredAuth {
    #[serde(rename = "type")]
    auth_type: String,
    refresh: String,
    access: String,
    expires: i64,
    #[serde(rename = "accountId")]
    account_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatGptSession {
    pub(crate) access_token: String,
    refresh_token: String,
    expires_at_ms: i64,
    pub(crate) account_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ChatGptAuthManager {
    auth_path: PathBuf,
    http_client: HttpClient,
    state: Arc<Mutex<Option<ChatGptSession>>>,
}

/// Device-authorization payload shown to the operator.
#[derive(Debug, Clone)]
pub struct ChatGptDeviceAuthorization {
    /// URL where the user should enter the code.
    pub verification_url: String,
    /// User-facing device code.
    pub user_code: String,
    device_auth_id: String,
    interval_secs: u64,
}

/// Persisted ChatGPT OAuth record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatGptAuthRecord {
    /// Short-lived access token.
    pub access_token: String,
    /// Long-lived refresh token.
    pub refresh_token: String,
    /// Expiration instant in Unix milliseconds.
    pub expires_at_ms: i64,
    /// ChatGPT account identifier.
    pub account_id: String,
}

/// Current state of a ChatGPT OAuth auth file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatGptAuthStatus {
    Missing {
        auth_path: PathBuf,
    },
    Available {
        auth_path: PathBuf,
        record: ChatGptAuthRecord,
        expired: bool,
    },
}

#[derive(Debug, Clone)]
pub struct ChatGptAuthFlow {
    auth_path: PathBuf,
    http_client: HttpClient,
}

#[derive(Debug, Deserialize)]
struct DeviceAuthorizationResponse {
    device_auth_id: String,
    user_code: String,
    interval: String,
}

#[derive(Debug, Deserialize)]
struct DeviceTokenPollResponse {
    authorization_code: String,
    code_verifier: String,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    #[serde(default)]
    id_token: Option<String>,
}

pub fn resolve_auth_file_path(path: Option<&str>) -> Result<PathBuf> {
    let raw_path = path
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_AUTH_FILE_PATH);
    let path = Path::new(raw_path);
    if !path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    let cwd = std::env::current_dir().context("failed to resolve current directory")?;
    if path == Path::new("/app/config/chatgpt/auth.json") {
        return Ok(cwd.join(DEFAULT_AUTH_FILE_PATH));
    }

    Ok(path.to_path_buf())
}

impl ChatGptAuthStatus {
    #[must_use]
    pub fn auth_path(&self) -> &Path {
        match self {
            Self::Missing { auth_path } | Self::Available { auth_path, .. } => auth_path.as_path(),
        }
    }

    #[must_use]
    pub fn record(&self) -> Option<&ChatGptAuthRecord> {
        match self {
            Self::Missing { .. } => None,
            Self::Available { record, .. } => Some(record),
        }
    }

    #[must_use]
    pub fn is_expired(&self) -> bool {
        matches!(self, Self::Available { expired: true, .. })
    }
}

impl ChatGptAuthFlow {
    #[must_use]
    pub fn new(auth_path: impl Into<PathBuf>) -> Self {
        let http_client = create_http_client();
        Self::new_with_client(auth_path, http_client)
    }

    #[must_use]
    pub fn new_with_client(auth_path: impl Into<PathBuf>, http_client: HttpClient) -> Self {
        Self {
            auth_path: auth_path.into(),
            http_client,
        }
    }

    #[must_use]
    pub fn auth_path(&self) -> &Path {
        self.auth_path.as_path()
    }

    pub async fn start(&self) -> Result<ChatGptDeviceAuthorization> {
        let response = self
            .http_client
            .post(CHATGPT_DEVICE_AUTH_URL)
            .header("Content-Type", "application/json")
            .header("User-Agent", APP_USER_AGENT)
            .json(&json!({ "client_id": CHATGPT_CLIENT_ID }))
            .send()
            .await
            .context("failed to request ChatGPT device code")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("ChatGPT device-code request failed: {status} - {body}");
        }

        let payload: DeviceAuthorizationResponse = response
            .json()
            .await
            .context("failed to parse ChatGPT device-code response")?;

        Ok(ChatGptDeviceAuthorization {
            verification_url: CHATGPT_DEVICE_VERIFY_URL.to_string(),
            user_code: payload.user_code,
            device_auth_id: payload.device_auth_id,
            interval_secs: payload.interval.parse::<u64>().unwrap_or(5),
        })
    }

    pub async fn wait_for_completion(
        &self,
        authorization: &ChatGptDeviceAuthorization,
    ) -> Result<ChatGptAuthRecord> {
        let poll_response = loop {
            let response = self
                .http_client
                .post(CHATGPT_DEVICE_TOKEN_URL)
                .header("Content-Type", "application/json")
                .header("User-Agent", APP_USER_AGENT)
                .json(&json!({
                    "device_auth_id": authorization.device_auth_id,
                    "user_code": authorization.user_code,
                }))
                .send()
                .await
                .context("failed to poll ChatGPT device authorization")?;

            match response.status() {
                reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::NOT_FOUND => {
                    tokio::time::sleep(std::time::Duration::from_millis(
                        authorization.interval_secs.saturating_mul(1_000)
                            + OAUTH_POLLING_SAFETY_MARGIN_MS,
                    ))
                    .await;
                }
                status if status.is_success() => {
                    let payload: DeviceTokenPollResponse = response
                        .json()
                        .await
                        .context("failed to parse ChatGPT device token poll response")?;
                    break payload;
                }
                status => {
                    let body = response.text().await.unwrap_or_default();
                    bail!("ChatGPT device authorization failed: {status} - {body}");
                }
            }
        };

        let record = exchange_authorization_code(
            &self.http_client,
            &poll_response.authorization_code,
            &poll_response.code_verifier,
        )
        .await?;
        persist_auth_record(self.auth_path(), &record).await?;
        Ok(record)
    }

    pub async fn status(&self) -> Result<ChatGptAuthStatus> {
        let exists = tokio::fs::try_exists(&self.auth_path)
            .await
            .with_context(|| format!("failed to stat {}", self.auth_path.display()))?;
        if !exists {
            return Ok(ChatGptAuthStatus::Missing {
                auth_path: self.auth_path.clone(),
            });
        }

        let record = load_auth_record(&self.auth_path).await?;
        Ok(ChatGptAuthStatus::Available {
            auth_path: self.auth_path.clone(),
            expired: record.expires_at_ms <= Utc::now().timestamp_millis(),
            record,
        })
    }
}

impl ChatGptAuthManager {
    pub(crate) fn new(auth_path: PathBuf, http_client: HttpClient) -> Self {
        Self {
            auth_path,
            http_client,
            state: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) async fn get_valid_session(&self) -> Result<ChatGptSession, LlmError> {
        let mut state = self.state.lock().await;
        let mut session = match state.clone() {
            Some(session) => session,
            None => {
                let loaded = load_auth_record(&self.auth_path)
                    .await
                    .map_err(|error| LlmError::MissingConfig(error.to_string()))?;
                let session = ChatGptSession::from(loaded);
                *state = Some(session.clone());
                session
            }
        };

        if session.expires_at_ms <= Utc::now().timestamp_millis() {
            let refreshed = refresh_access_token(&self.http_client, &session.refresh_token)
                .await
                .map_err(|error| LlmError::ApiError(error.to_string()))?;
            persist_auth_record(&self.auth_path, &refreshed)
                .await
                .map_err(|error| LlmError::ApiError(error.to_string()))?;
            session = ChatGptSession::from(refreshed);
            *state = Some(session.clone());
        }

        Ok(session)
    }
}

impl From<ChatGptAuthRecord> for ChatGptSession {
    fn from(value: ChatGptAuthRecord) -> Self {
        Self {
            access_token: value.access_token,
            refresh_token: value.refresh_token,
            expires_at_ms: value.expires_at_ms,
            account_id: value.account_id,
        }
    }
}

async fn load_auth_record(auth_path: &Path) -> Result<ChatGptAuthRecord> {
    let content = tokio::fs::read_to_string(auth_path)
        .await
        .with_context(|| format!("failed to read ChatGPT auth file {}", auth_path.display()))?;
    let file: ChatGptAuthFile = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse ChatGPT auth file {}", auth_path.display()))?;

    if file.openai.auth_type != "oauth" {
        bail!(
            "unsupported ChatGPT auth type `{}` in {}",
            file.openai.auth_type,
            auth_path.display()
        );
    }

    Ok(ChatGptAuthRecord {
        access_token: file.openai.access,
        refresh_token: file.openai.refresh,
        expires_at_ms: file.openai.expires,
        account_id: file.openai.account_id,
    })
}

async fn persist_auth_record(auth_path: &Path, record: &ChatGptAuthRecord) -> Result<()> {
    let parent = auth_path
        .parent()
        .ok_or_else(|| anyhow!("invalid ChatGPT auth file path {}", auth_path.display()))?;
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("failed to create {}", parent.display()))?;

    let file = ChatGptAuthFile {
        openai: ChatGptStoredAuth {
            auth_type: "oauth".to_string(),
            refresh: record.refresh_token.clone(),
            access: record.access_token.clone(),
            expires: record.expires_at_ms,
            account_id: record.account_id.clone(),
        },
    };
    let serialized =
        serde_json::to_vec_pretty(&file).context("failed to serialize ChatGPT auth record")?;
    tokio::fs::write(auth_path, serialized)
        .await
        .with_context(|| format!("failed to write ChatGPT auth file {}", auth_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        tokio::fs::set_permissions(auth_path, std::fs::Permissions::from_mode(0o600))
            .await
            .with_context(|| format!("failed to chmod {}", auth_path.display()))?;
    }

    Ok(())
}

async fn exchange_authorization_code(
    http_client: &HttpClient,
    authorization_code: &str,
    code_verifier: &str,
) -> Result<ChatGptAuthRecord> {
    let response = http_client
        .post(CHATGPT_OAUTH_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", authorization_code),
            ("redirect_uri", CHATGPT_DEVICE_CALLBACK_URL),
            ("client_id", CHATGPT_CLIENT_ID),
            ("code_verifier", code_verifier),
        ])
        .send()
        .await
        .context("failed to exchange ChatGPT authorization code")?;

    parse_token_response(response).await
}

async fn refresh_access_token(
    http_client: &HttpClient,
    refresh_token: &str,
) -> Result<ChatGptAuthRecord> {
    let response = http_client
        .post(CHATGPT_OAUTH_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CHATGPT_CLIENT_ID),
        ])
        .send()
        .await
        .context("failed to refresh ChatGPT access token")?;

    parse_token_response(response).await
}

async fn parse_token_response(response: reqwest::Response) -> Result<ChatGptAuthRecord> {
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("ChatGPT OAuth token request failed: {status} - {body}");
    }

    let payload: OAuthTokenResponse = response
        .json()
        .await
        .context("failed to parse ChatGPT OAuth token response")?;
    let account_id = payload
        .id_token
        .as_deref()
        .and_then(extract_account_id_from_jwt)
        .or_else(|| extract_account_id_from_jwt(&payload.access_token))
        .ok_or_else(|| anyhow!("failed to extract ChatGPT account id from OAuth tokens"))?;

    Ok(ChatGptAuthRecord {
        access_token: payload.access_token,
        refresh_token: payload.refresh_token,
        expires_at_ms: Utc::now().timestamp_millis() + payload.expires_in.saturating_mul(1_000),
        account_id,
    })
}

fn extract_account_id_from_jwt(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let claims: Value = serde_json::from_slice(&decoded).ok()?;

    claims
        .get("chatgpt_account_id")
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
        .or_else(|| {
            claims
                .get("https://api.openai.com/auth")
                .and_then(|value| value.get("chatgpt_account_id"))
                .and_then(|value| value.as_str())
                .map(ToString::to_string)
        })
        .or_else(|| {
            claims
                .get("organizations")
                .and_then(|value| value.as_array())
                .and_then(|items| items.first())
                .and_then(|value| value.get("id"))
                .and_then(|value| value.as_str())
                .map(ToString::to_string)
        })
}

#[cfg(test)]
mod tests {
    use super::{
        extract_account_id_from_jwt, persist_auth_record, resolve_auth_file_path, ChatGptAuthFlow,
        ChatGptAuthRecord, ChatGptAuthStatus,
    };
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn auth_record_round_trips_on_disk() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("chatgpt/auth.json");
        let record = ChatGptAuthRecord {
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at_ms: 123_456,
            account_id: "acct_1".to_string(),
        };

        persist_auth_record(&path, &record)
            .await
            .expect("persist auth");
        let status = ChatGptAuthFlow::new(path.clone())
            .status()
            .await
            .expect("status");

        assert_eq!(
            status,
            ChatGptAuthStatus::Available {
                auth_path: path,
                record,
                expired: true,
            }
        );
    }

    #[tokio::test]
    async fn status_reports_missing_auth_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("chatgpt/auth.json");
        let status = ChatGptAuthFlow::new(path.clone())
            .status()
            .await
            .expect("status");

        assert_eq!(status, ChatGptAuthStatus::Missing { auth_path: path });
    }

    #[test]
    fn account_id_extraction_uses_documented_priority() {
        let claims = json!({
            "chatgpt_account_id": "acct-root",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct-nested"
            },
            "organizations": [{ "id": "org-fallback" }]
        });
        let jwt = format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("claims json"))
        );

        assert_eq!(
            extract_account_id_from_jwt(&jwt).as_deref(),
            Some("acct-root")
        );
    }

    #[test]
    fn container_auth_path_maps_to_repo_config_path() {
        let mapped =
            resolve_auth_file_path(Some("/app/config/chatgpt/auth.json")).expect("mapped path");

        assert!(mapped.ends_with("config/chatgpt/auth.json"));
    }
}
