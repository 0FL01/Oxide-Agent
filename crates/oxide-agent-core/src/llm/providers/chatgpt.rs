use crate::llm::providers::protocol_profiles::CHAT_LIKE_TOOL_PROFILE;
use crate::llm::support::http::{create_http_client, parse_retry_after, APP_USER_AGENT};
use crate::llm::{
    ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, TokenUsage, ToolCall,
    ToolDefinition,
};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
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
const CHATGPT_CODEX_API_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
const OAUTH_POLLING_SAFETY_MARGIN_MS: u64 = 3_000;

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
struct ChatGptSession {
    access_token: String,
    refresh_token: String,
    expires_at_ms: i64,
    account_id: String,
}

#[derive(Debug, Clone)]
struct ChatGptAuthManager {
    auth_path: PathBuf,
    http_client: HttpClient,
    state: Arc<Mutex<Option<ChatGptSession>>>,
}

/// ChatGPT headless OAuth provider.
#[derive(Debug, Clone)]
pub struct ChatGptProvider {
    http_client: HttpClient,
    auth: ChatGptAuthManager,
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

impl ChatGptProvider {
    #[must_use]
    pub fn new(auth_path: impl Into<PathBuf>) -> Self {
        let http_client = create_http_client();
        Self::new_with_client(auth_path, http_client)
    }

    #[must_use]
    pub fn new_with_client(auth_path: impl Into<PathBuf>, http_client: HttpClient) -> Self {
        let auth = ChatGptAuthManager::new(auth_path.into(), http_client.clone());
        Self { http_client, auth }
    }

    /// Starts the headless ChatGPT device-auth flow.
    pub async fn begin_headless_login() -> Result<ChatGptDeviceAuthorization> {
        let http_client = create_http_client();
        let response = http_client
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

    /// Polls device-auth until completion, then writes `auth.json`.
    pub async fn complete_headless_login(
        auth_path: impl Into<PathBuf>,
        authorization: &ChatGptDeviceAuthorization,
    ) -> Result<ChatGptAuthRecord> {
        let http_client = create_http_client();
        let poll_response = loop {
            let response = http_client
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
            &http_client,
            &poll_response.authorization_code,
            &poll_response.code_verifier,
        )
        .await?;
        persist_auth_record(auth_path.into().as_path(), &record).await?;
        Ok(record)
    }

    async fn chat_request(&self, body: Value) -> Result<Value, LlmError> {
        let session = self.auth.get_valid_session().await?;

        let mut request = self
            .http_client
            .post(CHATGPT_CODEX_API_ENDPOINT)
            .header("Authorization", format!("Bearer {}", session.access_token))
            .header("Content-Type", "application/json");

        if !session.account_id.is_empty() {
            request = request.header("ChatGPT-Account-Id", session.account_id);
        }

        let response = request
            .json(&body)
            .send()
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let wait_secs = parse_retry_after(response.headers());
                let body = response.text().await.unwrap_or_default();
                return Err(LlmError::RateLimit {
                    wait_secs,
                    message: body,
                });
            }

            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!(
                "ChatGPT API error: {status} - {body}"
            )));
        }

        response
            .json()
            .await
            .map_err(|error| LlmError::JsonError(error.to_string()))
    }
}

impl ChatGptAuthManager {
    fn new(auth_path: PathBuf, http_client: HttpClient) -> Self {
        Self {
            auth_path,
            http_client,
            state: Arc::new(Mutex::new(None)),
        }
    }

    async fn get_valid_session(&self) -> Result<ChatGptSession, LlmError> {
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

#[async_trait]
impl LlmProvider for ChatGptProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let mut messages = prepare_structured_messages(system_prompt, history);
        messages.push(json!({
            "role": "user",
            "content": user_message,
        }));

        let body = build_chat_request_body(messages, &[], model_id, max_tokens, None, false);
        let response = self.chat_request(body).await?;

        response
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|value| value.as_str())
            .map(ToString::to_string)
            .ok_or_else(|| LlmError::ApiError("Empty response".to_string()))
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "Audio transcription not implemented for ChatGPT OAuth".to_string(),
        ))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "Image analysis not implemented for ChatGPT OAuth".to_string(),
        ))
    }

    async fn chat_with_tools<'a>(
        &self,
        request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        let ChatWithToolsRequest {
            system_prompt,
            messages,
            tools,
            model_id,
            max_tokens,
            temperature,
            json_mode,
        } = request;

        let body = build_chat_request_body(
            prepare_structured_messages(system_prompt, messages),
            tools,
            model_id,
            max_tokens,
            temperature,
            json_mode,
        );
        let response = self.chat_request(body).await?;

        let content = response
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|value| value.as_str())
            .map(ToString::to_string);

        let tool_calls_value = response
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("tool_calls"));
        let tool_calls = match tool_calls_value {
            Some(value) if value.is_null() => Vec::new(),
            Some(value) if value.is_array() => parse_tool_calls(value)?,
            Some(_) => {
                return Err(LlmError::JsonError(
                    "Invalid tool_calls format from ChatGPT OAuth".to_string(),
                ))
            }
            None => Vec::new(),
        };

        if content.is_none() && tool_calls.is_empty() {
            return Err(LlmError::ApiError("Empty response".to_string()));
        }

        let finish_reason = response["choices"][0]["finish_reason"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason,
            reasoning_content: None,
            usage: response.get("usage").and_then(parse_usage),
        })
    }
}

fn build_chat_request_body(
    messages: Vec<Value>,
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    temperature: Option<f32>,
    json_mode: bool,
) -> Value {
    let mut body = json!({
        "model": model_id,
        "messages": messages,
        "max_tokens": max_tokens,
        "stream": false,
    });

    if let Some(temperature) = temperature {
        body["temperature"] = json!(temperature);
    }

    if !tools.is_empty() {
        body["tools"] = json!(prepare_tools_json(tools));
        body["tool_choice"] = json!("auto");
    }

    if json_mode && tools.is_empty() {
        body["response_format"] = json!({ "type": "json_object" });
    }

    if model_id.starts_with("gpt-5") {
        body["reasoning"] = json!({ "effort": "medium" });
    }

    body
}

fn prepare_structured_messages(system_prompt: &str, history: &[Message]) -> Vec<Value> {
    let mut messages = vec![json!({
        "role": "system",
        "content": system_prompt,
    })];

    for msg in history {
        match msg.role.as_str() {
            "system" => {
                messages.push(json!({
                    "role": "system",
                    "content": msg.content,
                }));
            }
            "assistant" => {
                let mut message = json!({
                    "role": "assistant",
                    "content": msg.content,
                });

                if let Some(tool_calls) = &msg.tool_calls {
                    let api_tool_calls: Vec<Value> = tool_calls
                        .iter()
                        .filter_map(|tool_call| {
                            CHAT_LIKE_TOOL_PROFILE
                                .encode_tool_call(tool_call)
                                .and_then(|call| call.into_chat_like())
                                .map(|call| {
                                    json!({
                                        "id": call.id,
                                        "type": "function",
                                        "function": {
                                            "name": call.name,
                                            "arguments": call.arguments,
                                        }
                                    })
                                })
                        })
                        .collect();

                    if !api_tool_calls.is_empty() {
                        message["tool_calls"] = json!(api_tool_calls);
                    }
                }

                messages.push(message);
            }
            "tool" => {
                if let Some(result) = CHAT_LIKE_TOOL_PROFILE
                    .encode_tool_result(msg)
                    .and_then(|result| result.into_chat_like())
                {
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": result.tool_call_id,
                        "content": result.content,
                    }));
                }
            }
            _ => {
                messages.push(json!({
                    "role": "user",
                    "content": msg.content,
                }));
            }
        }
    }

    messages
}

fn prepare_tools_json(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                }
            })
        })
        .collect()
}

fn parse_tool_calls(value: &Value) -> Result<Vec<ToolCall>, LlmError> {
    let Some(array) = value.as_array() else {
        return Err(LlmError::JsonError(
            "Invalid tool_calls format from ChatGPT OAuth".to_string(),
        ));
    };

    let mut tool_calls = Vec::with_capacity(array.len());
    for call in array {
        let Some(function) = call.get("function") else {
            continue;
        };
        let Some(name) = function.get("name").and_then(|value| value.as_str()) else {
            continue;
        };
        let arguments = function
            .get("arguments")
            .and_then(|value| {
                value
                    .as_str()
                    .map(ToString::to_string)
                    .or_else(|| serde_json::to_string(value).ok())
            })
            .unwrap_or_default();
        let wire_id = call
            .get("id")
            .and_then(|value| value.as_str())
            .filter(|id| !id.trim().is_empty());
        tool_calls.push(match wire_id {
            Some(wire_id) => CHAT_LIKE_TOOL_PROFILE.inbound_provider_tool_call(
                wire_id,
                None,
                name.to_string(),
                arguments,
            ),
            None => {
                CHAT_LIKE_TOOL_PROFILE.inbound_uncorrelated_tool_call(name.to_string(), arguments)
            }
        });
    }

    Ok(tool_calls)
}

fn parse_usage(value: &Value) -> Option<TokenUsage> {
    Some(TokenUsage {
        prompt_tokens: value.get("prompt_tokens")?.as_u64()? as u32,
        completion_tokens: value.get("completion_tokens")?.as_u64()? as u32,
        total_tokens: value.get("total_tokens")?.as_u64()? as u32,
    })
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

/// Maps a container path like `/app/config/chatgpt/auth.json` back to the repo-local file.
pub fn auth_file_host_path_from_container_path(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    let cwd = std::env::current_dir().context("failed to resolve current directory")?;
    if path == Path::new("/app/config/chatgpt/auth.json") {
        return Ok(cwd.join("config/chatgpt/auth.json"));
    }

    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::{
        auth_file_host_path_from_container_path, build_chat_request_body,
        extract_account_id_from_jwt, load_auth_record, parse_tool_calls, persist_auth_record,
        prepare_structured_messages, ChatGptAuthRecord,
    };
    use crate::llm::{Message, ToolCall, ToolCallCorrelation, ToolCallFunction};
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
        let loaded = load_auth_record(&path).await.expect("load auth");

        assert_eq!(loaded, record);
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
    fn json_mode_uses_json_object_without_tools() {
        let body = build_chat_request_body(
            vec![json!({"role":"user","content":"hi"})],
            &[],
            "gpt-5.4",
            10,
            None,
            true,
        );

        assert_eq!(body["response_format"]["type"], json!("json_object"));
        assert_eq!(body["reasoning"]["effort"], json!("medium"));
    }

    #[test]
    fn structured_history_preserves_wire_tool_ids() {
        let history = vec![
            Message::assistant_with_tools(
                "Calling tools",
                vec![ToolCall::new(
                    "invoke-chatgpt-1",
                    ToolCallFunction {
                        name: "search".to_string(),
                        arguments: r#"{"query":"oxide"}"#.to_string(),
                    },
                    false,
                )
                .with_correlation(
                    ToolCallCorrelation::new("invoke-chatgpt-1")
                        .with_provider_tool_call_id("call-chatgpt-1"),
                )],
            ),
            Message::tool_with_correlation(
                "invoke-chatgpt-1",
                ToolCallCorrelation::new("invoke-chatgpt-1")
                    .with_provider_tool_call_id("call-chatgpt-1"),
                "search",
                "result",
            ),
        ];

        let messages = prepare_structured_messages("system", &history);

        assert_eq!(messages[1]["tool_calls"][0]["id"], json!("call-chatgpt-1"));
        assert_eq!(messages[2]["tool_call_id"], json!("call-chatgpt-1"));
    }

    #[test]
    fn parse_tool_calls_preserves_provider_ids() {
        let tool_calls = parse_tool_calls(&json!([
            {
                "id": "call-chatgpt-2",
                "type": "function",
                "function": {
                    "name": "search",
                    "arguments": "{\"query\":\"oxide\"}"
                }
            }
        ]))
        .expect("tool calls parse");

        assert_ne!(tool_calls[0].invocation_id().as_str(), "call-chatgpt-2");
        assert_eq!(tool_calls[0].wire_tool_call_id(), "call-chatgpt-2");
    }

    #[test]
    fn container_auth_path_maps_to_repo_config_path() {
        let mapped = auth_file_host_path_from_container_path("/app/config/chatgpt/auth.json")
            .expect("mapped path");

        assert!(mapped.ends_with("config/chatgpt/auth.json"));
    }
}
