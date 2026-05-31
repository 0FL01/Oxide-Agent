use chrono::{DateTime, Utc};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::warn;

pub const DEFAULT_MODELS_URL: &str = "https://opencode.ai/zen/go/v1/models";
pub const DEFAULT_MODEL_DISCOVERY_TTL_SECS: u64 = 30 * 60;
pub const MIN_MODEL_DISCOVERY_TTL_SECS: u64 = 10 * 60;
pub const MAX_MODEL_DISCOVERY_TTL_SECS: u64 = 60 * 60;

const FALLBACK_MODEL_IDS: &[&str] = &[
    "minimax-m2.7",
    "minimax-m2.5",
    "kimi-k2.6",
    "kimi-k2.5",
    "glm-5.1",
    "glm-5",
    "deepseek-v4-pro",
    "deepseek-v4-flash",
    "qwen3.7-max",
    "qwen3.6-plus",
    "qwen3.5-plus",
    "mimo-v2-pro",
    "mimo-v2-omni",
    "mimo-v2.5-pro",
    "mimo-v2.5",
    "hy3-preview",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RawOpenCodeGoModel {
    pub id: String,
    pub object: String,
    #[serde(default)]
    pub created: Option<u64>,
    #[serde(default)]
    pub owned_by: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProtocol {
    OpenAiChatCompletions,
    AnthropicMessages,
    Unknown,
}

impl ModelProtocol {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiChatCompletions => "openai_chat_completions",
            Self::AnthropicMessages => "anthropic_messages",
            Self::Unknown => "unknown",
        }
    }
}

impl FromStr for ModelProtocol {
    type Err = OpenCodeGoDiscoveryError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai_chat_completions" | "openai" | "chat_completions" => {
                Ok(Self::OpenAiChatCompletions)
            }
            "anthropic_messages" | "anthropic" | "messages" => Ok(Self::AnthropicMessages),
            "unknown" => Ok(Self::Unknown),
            other => Err(OpenCodeGoDiscoveryError::InvalidProtocol(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    Network,
    Cache,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DiscoveredOpenCodeGoModel {
    pub provider_id: String,
    pub model_id: String,
    pub qualified_id: String,
    pub display_name: String,
    pub protocol: ModelProtocol,
    pub source: DiscoverySource,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct OpenCodeGoDiscoveryConfig {
    pub models_url: String,
    pub ttl: Duration,
    pub protocol_overrides: BTreeMap<String, ModelProtocol>,
}

impl OpenCodeGoDiscoveryConfig {
    #[must_use]
    pub fn new(
        models_url: impl Into<String>,
        ttl: Duration,
        protocol_overrides: BTreeMap<String, ModelProtocol>,
    ) -> Self {
        Self {
            models_url: models_url.into(),
            ttl,
            protocol_overrides,
        }
    }

    #[must_use]
    pub fn from_env() -> Self {
        let models_url = std::env::var("OPENCODE_GO_MODELS_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_MODELS_URL.to_string());
        let ttl_secs = std::env::var("OPENCODE_GO_MODEL_CACHE_TTL_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_MODEL_DISCOVERY_TTL_SECS)
            .clamp(MIN_MODEL_DISCOVERY_TTL_SECS, MAX_MODEL_DISCOVERY_TTL_SECS);

        Self::new(models_url, Duration::from_secs(ttl_secs), BTreeMap::new())
    }
}

#[derive(Debug)]
pub struct OpenCodeGoModelCatalog {
    http_client: HttpClient,
    api_key: String,
    config: OpenCodeGoDiscoveryConfig,
    state: RwLock<ModelCatalogState>,
}

#[derive(Debug, Default)]
struct ModelCatalogState {
    last_good: Vec<DiscoveredOpenCodeGoModel>,
    fetched_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Error)]
pub enum OpenCodeGoDiscoveryError {
    #[error("OpenCode Go model discovery request failed: {0}")]
    Network(String),
    #[error("OpenCode Go model discovery returned HTTP {0}")]
    HttpStatus(u16),
    #[error("OpenCode Go model discovery response parse failed: {0}")]
    Parse(String),
    #[error("unsupported OpenCode Go model protocol override: {0}")]
    InvalidProtocol(String),
    #[error("OpenCode Go model discovery returned an empty model list")]
    EmptyModelList,
}

impl OpenCodeGoModelCatalog {
    #[must_use]
    pub fn new(
        http_client: HttpClient,
        api_key: String,
        config: OpenCodeGoDiscoveryConfig,
    ) -> Self {
        Self {
            http_client,
            api_key,
            config,
            state: RwLock::new(ModelCatalogState::default()),
        }
    }

    pub fn spawn_background_refresh(self: Arc<Self>) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        handle.spawn(async move {
            self.refresh().await;
            loop {
                tokio::time::sleep(self.config.ttl).await;
                self.refresh().await;
            }
        });
    }

    pub async fn models(&self) -> Vec<DiscoveredOpenCodeGoModel> {
        if let Some(cached) = self.cached_models_if_fresh().await {
            return cached;
        }
        self.refresh().await
    }

    pub async fn refresh(&self) -> Vec<DiscoveredOpenCodeGoModel> {
        self.refresh_with_fetcher(|| self.fetch_network_models())
            .await
    }

    pub(crate) async fn refresh_with_fetcher<F, Fut>(
        &self,
        fetcher: F,
    ) -> Vec<DiscoveredOpenCodeGoModel>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Vec<RawOpenCodeGoModel>, OpenCodeGoDiscoveryError>>,
    {
        match fetcher().await.and_then(validate_non_empty_models) {
            Ok(raw_models) => {
                let fetched_at = Utc::now();
                let models = normalize_models(
                    raw_models,
                    DiscoverySource::Network,
                    fetched_at,
                    &self.config.protocol_overrides,
                );
                let mut state = self.state.write().await;
                state.last_good = models.clone();
                state.fetched_at = Some(fetched_at);
                models
            }
            Err(error) => {
                warn!(%error, "OpenCode Go model discovery failed; using cached or fallback models");
                self.cached_or_fallback_models().await
            }
        }
    }

    async fn fetch_network_models(
        &self,
    ) -> Result<Vec<RawOpenCodeGoModel>, OpenCodeGoDiscoveryError> {
        let response = self
            .http_client
            .get(&self.config.models_url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|error| OpenCodeGoDiscoveryError::Network(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(OpenCodeGoDiscoveryError::HttpStatus(status.as_u16()));
        }
        let value = response
            .json::<serde_json::Value>()
            .await
            .map_err(|error| OpenCodeGoDiscoveryError::Parse(error.to_string()))?;
        parse_models_value(value)
    }

    async fn cached_models_if_fresh(&self) -> Option<Vec<DiscoveredOpenCodeGoModel>> {
        let state = self.state.read().await;
        let fetched_at = state.fetched_at?;
        if state.last_good.is_empty() {
            return None;
        }
        let age = Utc::now().signed_duration_since(fetched_at).to_std().ok()?;
        (age < self.config.ttl).then(|| with_source(&state.last_good, DiscoverySource::Cache))
    }

    async fn cached_or_fallback_models(&self) -> Vec<DiscoveredOpenCodeGoModel> {
        let state = self.state.read().await;
        if !state.last_good.is_empty() {
            return with_source(&state.last_good, DiscoverySource::Cache);
        }
        fallback_models(&self.config.protocol_overrides)
    }
}

pub fn parse_models_json(input: &str) -> Result<Vec<RawOpenCodeGoModel>, OpenCodeGoDiscoveryError> {
    let value = serde_json::from_str::<serde_json::Value>(input)
        .map_err(|error| OpenCodeGoDiscoveryError::Parse(error.to_string()))?;
    parse_models_value(value)
}

fn parse_models_value(
    value: serde_json::Value,
) -> Result<Vec<RawOpenCodeGoModel>, OpenCodeGoDiscoveryError> {
    let response = serde_json::from_value::<RawOpenCodeGoModelsResponse>(value)
        .map_err(|error| OpenCodeGoDiscoveryError::Parse(error.to_string()))?;
    Ok(response.data)
}

#[derive(Debug, Deserialize)]
struct RawOpenCodeGoModelsResponse {
    data: Vec<RawOpenCodeGoModel>,
}

fn validate_non_empty_models(
    models: Vec<RawOpenCodeGoModel>,
) -> Result<Vec<RawOpenCodeGoModel>, OpenCodeGoDiscoveryError> {
    if models.is_empty() {
        Err(OpenCodeGoDiscoveryError::EmptyModelList)
    } else {
        Ok(models)
    }
}

pub fn normalize_models(
    raw_models: Vec<RawOpenCodeGoModel>,
    source: DiscoverySource,
    fetched_at: DateTime<Utc>,
    protocol_overrides: &BTreeMap<String, ModelProtocol>,
) -> Vec<DiscoveredOpenCodeGoModel> {
    raw_models
        .into_iter()
        .filter_map(|model| {
            let model_id = raw_model_id(&model.id);
            (!model_id.is_empty())
                .then(|| discovered_model(model_id, source, fetched_at, protocol_overrides))
        })
        .collect()
}

pub fn fallback_models(
    protocol_overrides: &BTreeMap<String, ModelProtocol>,
) -> Vec<DiscoveredOpenCodeGoModel> {
    let fetched_at = Utc::now();
    FALLBACK_MODEL_IDS
        .iter()
        .map(|id| {
            discovered_model(
                (*id).to_string(),
                DiscoverySource::Fallback,
                fetched_at,
                protocol_overrides,
            )
        })
        .collect()
}

pub fn infer_protocol(
    model_id: &str,
    protocol_overrides: &BTreeMap<String, ModelProtocol>,
) -> ModelProtocol {
    let model_id = raw_model_id(model_id);
    if let Some(protocol) = protocol_overrides
        .get(&model_id)
        .or_else(|| protocol_overrides.get(&qualified_model_id(&model_id)))
    {
        return *protocol;
    }

    let lower = model_id.to_ascii_lowercase();
    if lower.starts_with("glm-")
        || lower.starts_with("kimi-")
        || lower.starts_with("deepseek-")
        || lower.starts_with("mimo-v2.5")
    {
        return ModelProtocol::OpenAiChatCompletions;
    }
    if lower.starts_with("minimax-") || lower.starts_with("qwen") {
        return ModelProtocol::AnthropicMessages;
    }
    ModelProtocol::Unknown
}

#[must_use]
pub fn raw_model_id(model_id: &str) -> String {
    let trimmed = model_id.trim();
    trimmed
        .strip_prefix("opencode-go/")
        .unwrap_or(trimmed)
        .to_string()
}

#[must_use]
pub fn qualified_model_id(model_id: &str) -> String {
    format!("opencode-go/{}", raw_model_id(model_id))
}

fn discovered_model(
    model_id: String,
    source: DiscoverySource,
    fetched_at: DateTime<Utc>,
    protocol_overrides: &BTreeMap<String, ModelProtocol>,
) -> DiscoveredOpenCodeGoModel {
    let qualified_id = qualified_model_id(&model_id);
    DiscoveredOpenCodeGoModel {
        provider_id: "opencode-go".to_string(),
        model_id: model_id.clone(),
        qualified_id: qualified_id.clone(),
        display_name: qualified_id,
        protocol: infer_protocol(&model_id, protocol_overrides),
        source,
        fetched_at,
    }
}

fn with_source(
    models: &[DiscoveredOpenCodeGoModel],
    source: DiscoverySource,
) -> Vec<DiscoveredOpenCodeGoModel> {
    models
        .iter()
        .cloned()
        .map(|mut model| {
            model.source = source;
            model
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discovery_config() -> OpenCodeGoDiscoveryConfig {
        OpenCodeGoDiscoveryConfig::new(DEFAULT_MODELS_URL, Duration::from_secs(60), BTreeMap::new())
    }

    #[test]
    fn parses_openai_compatible_models_response() {
        let models = parse_models_json(
            r#"{
                "object":"list",
                "data":[{"id":"kimi-k2.6","object":"model","created":1780207178,"owned_by":"opencode"}]
            }"#,
        )
        .expect("models response parses");

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "kimi-k2.6");
        assert_eq!(models[0].owned_by.as_deref(), Some("opencode"));
    }

    #[test]
    fn normalizes_raw_models_to_qualified_ids() {
        let models = normalize_models(
            vec![RawOpenCodeGoModel {
                id: "kimi-k2.6".to_string(),
                object: "model".to_string(),
                created: Some(1780207178),
                owned_by: Some("opencode".to_string()),
            }],
            DiscoverySource::Network,
            Utc::now(),
            &BTreeMap::new(),
        );

        assert_eq!(models[0].provider_id, "opencode-go");
        assert_eq!(models[0].model_id, "kimi-k2.6");
        assert_eq!(models[0].qualified_id, "opencode-go/kimi-k2.6");
        assert_eq!(models[0].protocol, ModelProtocol::OpenAiChatCompletions);
    }

    #[test]
    fn infers_known_protocol_families_and_unknowns() {
        let overrides = BTreeMap::new();

        assert_eq!(
            infer_protocol("opencode-go/glm-5.1", &overrides),
            ModelProtocol::OpenAiChatCompletions
        );
        assert_eq!(
            infer_protocol("kimi-k2.6", &overrides),
            ModelProtocol::OpenAiChatCompletions
        );
        assert_eq!(
            infer_protocol("minimax-m2.7", &overrides),
            ModelProtocol::AnthropicMessages
        );
        assert_eq!(
            infer_protocol("qwen3.7-max", &overrides),
            ModelProtocol::AnthropicMessages
        );
        assert_eq!(
            infer_protocol("hy3-preview", &overrides),
            ModelProtocol::Unknown
        );
    }

    #[test]
    fn protocol_override_accepts_raw_or_qualified_ids() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "opencode-go/hy3-preview".to_string(),
            ModelProtocol::OpenAiChatCompletions,
        );

        assert_eq!(
            infer_protocol("hy3-preview", &overrides),
            ModelProtocol::OpenAiChatCompletions
        );
    }

    #[tokio::test]
    async fn refresh_uses_last_known_good_cache_after_error() {
        let catalog = OpenCodeGoModelCatalog::new(
            HttpClient::new(),
            "test-key".to_string(),
            discovery_config(),
        );

        let network = catalog
            .refresh_with_fetcher(|| async {
                Ok(vec![RawOpenCodeGoModel {
                    id: "kimi-k2.6".to_string(),
                    object: "model".to_string(),
                    created: None,
                    owned_by: None,
                }])
            })
            .await;
        assert_eq!(network[0].source, DiscoverySource::Network);

        let cached = catalog
            .refresh_with_fetcher(|| async {
                Err(OpenCodeGoDiscoveryError::Network("offline".to_string()))
            })
            .await;

        assert_eq!(cached[0].qualified_id, "opencode-go/kimi-k2.6");
        assert_eq!(cached[0].source, DiscoverySource::Cache);
    }

    #[tokio::test]
    async fn refresh_uses_embedded_fallback_when_cache_is_empty() {
        let catalog = OpenCodeGoModelCatalog::new(
            HttpClient::new(),
            "test-key".to_string(),
            discovery_config(),
        );

        let models = catalog
            .refresh_with_fetcher(|| async {
                Err(OpenCodeGoDiscoveryError::Network("offline".to_string()))
            })
            .await;

        assert!(models
            .iter()
            .any(|model| model.qualified_id == "opencode-go/kimi-k2.6"));
        assert!(models
            .iter()
            .all(|model| model.source == DiscoverySource::Fallback));
    }
}
