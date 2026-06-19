use chrono::{DateTime, Utc};
use lazy_regex::lazy_regex;
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::str::FromStr;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::warn;

pub const DEFAULT_MODELS_URL: &str = "https://opencode.ai/zen/go/v1/models";
pub const DEFAULT_MODEL_DISCOVERY_TTL_SECS: u64 = 30 * 60;
pub const MIN_MODEL_DISCOVERY_TTL_SECS: u64 = 10 * 60;
pub const MAX_MODEL_DISCOVERY_TTL_SECS: u64 = 60 * 60;

pub const OPENCODE_GO_PROVIDER_ID: &str = "opencode-go";
pub const OPENCODE_ZEN_PROVIDER_ID: &str = "opencode-zen";
pub const OPENAI_BASE_PROVIDER_ID: &str = "openai-base";

static FREE_MODEL_RE: lazy_regex::Lazy<regex::Regex> = lazy_regex!(r"(?i)(^|[-_\s])free($|[-_\s])");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RawOpenCodeGoModel {
    pub id: String,
    pub object: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, alias = "displayName")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub created: Option<u64>,
    #[serde(default)]
    pub owned_by: Option<String>,
    #[serde(default)]
    pub modalities: Option<RawOpenCodeGoModelModalities>,
    #[serde(
        default,
        alias = "inputModalities",
        alias = "supported_input_modalities",
        alias = "supportedInputModalities"
    )]
    pub input_modalities: Vec<String>,
    #[serde(default)]
    pub architecture: Option<RawOpenCodeGoModelArchitecture>,
    #[serde(default)]
    pub modality: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RawOpenCodeGoModelModalities {
    #[serde(default, alias = "inputModalities", alias = "input_modalities")]
    pub input: Vec<String>,
    #[serde(default, alias = "outputModalities", alias = "output_modalities")]
    pub output: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RawOpenCodeGoModelArchitecture {
    #[serde(default, alias = "inputModalities")]
    pub input_modalities: Vec<String>,
    #[serde(default, alias = "outputModalities")]
    pub output_modalities: Vec<String>,
    #[serde(default)]
    pub modality: Option<String>,
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
    pub supports_image_input: bool,
    pub source: DiscoverySource,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct OpenCodeGoDiscoveryConfig {
    pub provider_id: String,
    pub model_prefix: String,
    pub models_url: String,
    pub ttl: Duration,
    pub protocol_overrides: BTreeMap<String, ModelProtocol>,
    default_protocol: Option<ModelProtocol>,
    default_image_input: Option<bool>,
    filter: ModelDiscoveryFilter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelDiscoveryFilter {
    All,
    FreeOnly,
}

impl OpenCodeGoDiscoveryConfig {
    #[must_use]
    pub fn new(
        models_url: impl Into<String>,
        ttl: Duration,
        protocol_overrides: BTreeMap<String, ModelProtocol>,
    ) -> Self {
        Self::new_for_provider(
            OPENCODE_GO_PROVIDER_ID,
            OPENCODE_GO_PROVIDER_ID,
            models_url,
            ttl,
            protocol_overrides,
            None,
            None,
            ModelDiscoveryFilter::All,
        )
    }

    #[must_use]
    pub fn new_zen(
        models_url: impl Into<String>,
        ttl: Duration,
        protocol_overrides: BTreeMap<String, ModelProtocol>,
    ) -> Self {
        Self::new_for_provider(
            OPENCODE_ZEN_PROVIDER_ID,
            OPENCODE_ZEN_PROVIDER_ID,
            models_url,
            ttl,
            protocol_overrides,
            None,
            None,
            ModelDiscoveryFilter::FreeOnly,
        )
    }

    #[must_use]
    pub fn new_openai_base(models_url: impl Into<String>, ttl: Duration) -> Self {
        Self::new_openai_base_for_provider(OPENAI_BASE_PROVIDER_ID, models_url, ttl, None)
    }

    #[must_use]
    pub fn new_openai_base_for_provider(
        provider_id: impl Into<String>,
        models_url: impl Into<String>,
        ttl: Duration,
        default_image_input: Option<bool>,
    ) -> Self {
        let provider_id = provider_id.into();
        Self::new_for_provider(
            provider_id.clone(),
            provider_id,
            models_url,
            ttl,
            BTreeMap::new(),
            Some(ModelProtocol::OpenAiChatCompletions),
            default_image_input,
            ModelDiscoveryFilter::All,
        )
    }

    fn new_for_provider(
        provider_id: impl Into<String>,
        model_prefix: impl Into<String>,
        models_url: impl Into<String>,
        ttl: Duration,
        protocol_overrides: BTreeMap<String, ModelProtocol>,
        default_protocol: Option<ModelProtocol>,
        default_image_input: Option<bool>,
        filter: ModelDiscoveryFilter,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_prefix: model_prefix.into(),
            models_url: models_url.into(),
            ttl,
            protocol_overrides,
            default_protocol,
            default_image_input,
            filter,
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
    api_key: Option<String>,
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
    #[error("OpenCode model discovery request failed: {0}")]
    Network(String),
    #[error("OpenCode model discovery returned HTTP {0}")]
    HttpStatus(u16),
    #[error("OpenCode model discovery response parse failed: {0}")]
    Parse(String),
    #[error("unsupported OpenCode model protocol override: {0}")]
    InvalidProtocol(String),
    #[error("OpenCode model discovery returned an empty model list")]
    EmptyModelList,
}

impl OpenCodeGoModelCatalog {
    #[must_use]
    pub fn new(
        http_client: HttpClient,
        api_key: Option<String>,
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
                let models = normalize_models_for_config(
                    raw_models,
                    DiscoverySource::Network,
                    fetched_at,
                    &self.config,
                );
                if models.is_empty() {
                    let error = OpenCodeGoDiscoveryError::EmptyModelList;
                    warn!(%error, "OpenCode model discovery failed; using cached models if available");
                    return self.cached_or_empty_models().await;
                }
                let mut state = self.state.write().await;
                state.last_good = models.clone();
                state.fetched_at = Some(fetched_at);
                models
            }
            Err(error) => {
                warn!(%error, "OpenCode model discovery failed; using cached models if available");
                self.cached_or_empty_models().await
            }
        }
    }

    async fn fetch_network_models(
        &self,
    ) -> Result<Vec<RawOpenCodeGoModel>, OpenCodeGoDiscoveryError> {
        let mut request = self.http_client.get(&self.config.models_url);
        if let Some(api_key) = self.api_key.as_deref().filter(|key| !key.trim().is_empty()) {
            request = request.bearer_auth(api_key);
        }
        let response = request
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

    async fn cached_or_empty_models(&self) -> Vec<DiscoveredOpenCodeGoModel> {
        let state = self.state.read().await;
        if !state.last_good.is_empty() {
            return with_source(&state.last_good, DiscoverySource::Cache);
        }
        Vec::new()
    }
}

#[async_trait::async_trait]
impl crate::llm::DiscoveredModelSource for OpenCodeGoModelCatalog {
    async fn models(&self) -> Vec<crate::llm::DiscoveredLlmModel> {
        OpenCodeGoModelCatalog::models(self)
            .await
            .into_iter()
            .map(crate::llm::DiscoveredLlmModel::from)
            .collect()
    }
    async fn refresh(&self) -> Vec<crate::llm::DiscoveredLlmModel> {
        OpenCodeGoModelCatalog::refresh(self)
            .await
            .into_iter()
            .map(crate::llm::DiscoveredLlmModel::from)
            .collect()
    }
}

// === Models.dev catalog ===

/// Authoritative source for model modalities, used by opencode itself.
/// Bare OpenAI-compatible `/v1/models` endpoints (opencode-go, opencode-zen,
/// openai-base) do not expose modality metadata. Models.dev fills that gap.
const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";
const MODELS_DEV_TTL: Duration = Duration::from_secs(DEFAULT_MODEL_DISCOVERY_TTL_SECS);

#[derive(Debug)]
pub struct ModelsDevCatalog {
    http_client: HttpClient,
    state: StdRwLock<ModelsDevState>,
}

#[derive(Debug, Default)]
struct ModelsDevState {
    image_support: HashMap<String, bool>,
    fetched_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Error)]
enum ModelsDevError {
    #[error("models.dev catalog request failed: {0}")]
    Network(String),
    #[error("models.dev catalog returned HTTP {0}")]
    HttpStatus(u16),
    #[error("models.dev catalog response parse failed: {0}")]
    Parse(String),
}

#[derive(Debug, Deserialize)]
struct ModelsDevResponse {
    opencode: ModelsDevProvider,
}

#[derive(Debug, Deserialize)]
struct ModelsDevProvider {
    models: HashMap<String, ModelsDevModel>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevModel {
    #[serde(default)]
    modalities: Option<ModelsDevModalities>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevModalities {
    #[serde(default)]
    input: Vec<String>,
}

impl ModelsDevCatalog {
    #[must_use]
    pub fn new(http_client: HttpClient) -> Self {
        Self {
            http_client,
            state: StdRwLock::new(ModelsDevState::default()),
        }
    }

    pub fn spawn_background_refresh(self: Arc<Self>) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        handle.spawn(async move {
            self.refresh().await;
            loop {
                tokio::time::sleep(MODELS_DEV_TTL).await;
                self.refresh().await;
            }
        });
    }

    async fn refresh(&self) {
        match self.fetch().await {
            Ok(image_support) => {
                let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
                state.image_support = image_support;
                state.fetched_at = Some(Utc::now());
            }
            Err(error) => {
                warn!(%error, "Models.dev catalog refresh failed; using cached data if available");
            }
        }
    }

    async fn fetch(&self) -> Result<HashMap<String, bool>, ModelsDevError> {
        let response = self
            .http_client
            .get(MODELS_DEV_API_URL)
            .send()
            .await
            .map_err(|e| ModelsDevError::Network(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ModelsDevError::HttpStatus(status.as_u16()));
        }
        let body: ModelsDevResponse = response
            .json()
            .await
            .map_err(|e| ModelsDevError::Parse(e.to_string()))?;
        Ok(body
            .opencode
            .models
            .into_iter()
            .map(|(id, model)| {
                let supports_image = model
                    .modalities
                    .as_ref()
                    .map(|m| m.input.iter().any(|input| input == "image"))
                    .unwrap_or(false);
                (id, supports_image)
            })
            .collect())
    }

    /// Check if a model supports image input.
    /// Tries exact ID match first, then `{model_id}-free` (pricing tier
    /// normalization — `-free` is a pricing variant, not a capability change).
    fn supports_image(&self, model_id: &str) -> bool {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        if let Some(&supports) = state.image_support.get(model_id) {
            return supports;
        }
        let free_variant = format!("{model_id}-free");
        state
            .image_support
            .get(&free_variant)
            .copied()
            .unwrap_or(false)
    }
}

static MODELS_DEV_CATALOG: StdRwLock<Option<Arc<ModelsDevCatalog>>> = StdRwLock::new(None);

/// Initialize the global Models.dev catalog. Does a **blocking** initial fetch
/// in a multi-threaded tokio runtime so the catalog is populated before config
/// validation runs. Spawns background refresh for periodic updates. Subsequent
/// calls are no-ops.
pub fn init_models_dev_catalog(http_client: HttpClient) {
    let mut guard = MODELS_DEV_CATALOG
        .write()
        .unwrap_or_else(|e| e.into_inner());
    if guard.is_some() {
        return;
    }
    let catalog = Arc::new(ModelsDevCatalog::new(http_client));

    // Blocking initial fetch so vision data is available synchronously at
    // config-validation time. Only in multi-threaded runtime (production);
    // tests use `init_models_dev_catalog_for_tests` with mock data.
    if let Ok(handle) = tokio::runtime::Handle::try_current()
        && handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
    {
        tokio::task::block_in_place(|| handle.block_on(catalog.refresh()));
    }

    Arc::clone(&catalog).spawn_background_refresh();
    *guard = Some(catalog);
}

/// Synchronous lookup: does `model_id` support image input per Models.dev?
/// Returns `false` if the catalog is not yet initialized or the model is
/// unknown (safe default: text-only).
#[must_use]
pub fn models_dev_supports_image(model_id: &str) -> bool {
    MODELS_DEV_CATALOG
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|catalog| catalog.supports_image(model_id))
        .unwrap_or(false)
}

/// Force a refresh of the global Models.dev catalog. Used by smoke tests to
/// ensure the catalog is populated before asserting on vision support.
pub async fn refresh_models_dev_catalog() {
    // Clone the Arc out of the guard and drop the guard before awaiting so
    // the `StdRwLock` is never held across an await point (deadlock risk).
    let catalog = MODELS_DEV_CATALOG
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(Arc::clone);
    if let Some(catalog) = catalog {
        catalog.refresh().await;
    }
}

/// Test-only: initialize the global catalog with pre-populated mock data.
/// No network fetch, no background refresh. Replaces any existing catalog
/// so tests can set up known vision state regardless of call order.
#[cfg(test)]
pub(crate) fn init_models_dev_catalog_for_tests(image_support: HashMap<String, bool>) {
    let catalog = Arc::new(ModelsDevCatalog {
        http_client: HttpClient::new(),
        state: StdRwLock::new(ModelsDevState {
            image_support,
            fetched_at: Some(Utc::now()),
        }),
    });
    *MODELS_DEV_CATALOG
        .write()
        .unwrap_or_else(|e| e.into_inner()) = Some(catalog);
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
    let config = OpenCodeGoDiscoveryConfig::new(
        DEFAULT_MODELS_URL,
        Duration::from_secs(DEFAULT_MODEL_DISCOVERY_TTL_SECS),
        protocol_overrides.clone(),
    );
    normalize_models_for_config(raw_models, source, fetched_at, &config)
}

pub fn normalize_models_for_config(
    raw_models: Vec<RawOpenCodeGoModel>,
    source: DiscoverySource,
    fetched_at: DateTime<Utc>,
    config: &OpenCodeGoDiscoveryConfig,
) -> Vec<DiscoveredOpenCodeGoModel> {
    raw_models
        .into_iter()
        .filter(|model| model_matches_filter(model, config.filter))
        .filter_map(|model| {
            let model_id = raw_model_id_for_prefix(&model.id, &config.model_prefix);
            (!model_id.is_empty())
                .then(|| discovered_model_for_config(&model, model_id, source, fetched_at, config))
        })
        .collect()
}

pub fn infer_protocol(
    model_id: &str,
    protocol_overrides: &BTreeMap<String, ModelProtocol>,
) -> ModelProtocol {
    infer_protocol_for_prefix(model_id, OPENCODE_GO_PROVIDER_ID, protocol_overrides)
}

pub fn infer_protocol_for_prefix(
    model_id: &str,
    model_prefix: &str,
    protocol_overrides: &BTreeMap<String, ModelProtocol>,
) -> ModelProtocol {
    let model_id = raw_model_id_for_prefix(model_id, model_prefix);
    if let Some(protocol) = protocol_overrides
        .get(&model_id)
        .or_else(|| protocol_overrides.get(&qualified_model_id_for_prefix(&model_id, model_prefix)))
    {
        return *protocol;
    }

    let lower = model_id.to_ascii_lowercase();
    if lower.starts_with("glm-")
        || lower.starts_with("kimi-")
        || lower.starts_with("deepseek-")
        || lower.starts_with("mimo-v2.5")
        || lower.starts_with("nemotron-")
    {
        return ModelProtocol::OpenAiChatCompletions;
    }
    if lower.starts_with("minimax-") || lower.starts_with("qwen") {
        return ModelProtocol::AnthropicMessages;
    }
    ModelProtocol::Unknown
}

fn infer_protocol_for_config(model_id: &str, config: &OpenCodeGoDiscoveryConfig) -> ModelProtocol {
    let inferred =
        infer_protocol_for_prefix(model_id, &config.model_prefix, &config.protocol_overrides);
    if inferred == ModelProtocol::Unknown {
        config.default_protocol.unwrap_or(inferred)
    } else {
        inferred
    }
}

#[must_use]
pub fn raw_model_id(model_id: &str) -> String {
    let without_go = raw_model_id_for_prefix(model_id, OPENCODE_GO_PROVIDER_ID);
    raw_model_id_for_prefix(&without_go, OPENCODE_ZEN_PROVIDER_ID)
}

#[must_use]
pub fn raw_model_id_for_prefix(model_id: &str, model_prefix: &str) -> String {
    let trimmed = model_id.trim();
    let prefix = format!("{}/", model_prefix.trim().trim_end_matches('/'));
    trimmed.strip_prefix(&prefix).unwrap_or(trimmed).to_string()
}

#[must_use]
pub fn qualified_model_id(model_id: &str) -> String {
    qualified_model_id_for_prefix(model_id, OPENCODE_GO_PROVIDER_ID)
}

#[must_use]
pub fn qualified_model_id_for_prefix(model_id: &str, model_prefix: &str) -> String {
    format!(
        "{}/{}",
        model_prefix.trim().trim_end_matches('/'),
        raw_model_id_for_prefix(model_id, model_prefix)
    )
}

fn discovered_model_for_config(
    raw_model: &RawOpenCodeGoModel,
    model_id: String,
    source: DiscoverySource,
    fetched_at: DateTime<Utc>,
    config: &OpenCodeGoDiscoveryConfig,
) -> DiscoveredOpenCodeGoModel {
    let qualified_id = qualified_model_id_for_prefix(&model_id, &config.model_prefix);
    DiscoveredOpenCodeGoModel {
        provider_id: config.provider_id.clone(),
        model_id: model_id.clone(),
        qualified_id: qualified_id.clone(),
        display_name: qualified_id,
        protocol: infer_protocol_for_config(&model_id, config),
        supports_image_input: supports_image_input(raw_model, &model_id, config),
        source,
        fetched_at,
    }
}

fn supports_image_input(
    model: &RawOpenCodeGoModel,
    model_id: &str,
    config: &OpenCodeGoDiscoveryConfig,
) -> bool {
    explicit_image_input_support(model)
        .or(config.default_image_input)
        .unwrap_or_else(|| models_dev_supports_image(model_id))
}

#[must_use]
pub fn supports_image_input_for_model_id(model_id: &str) -> bool {
    models_dev_supports_image(&raw_model_id(model_id))
}

fn explicit_image_input_support(model: &RawOpenCodeGoModel) -> Option<bool> {
    let mut has_input_metadata = false;
    let mut has_image_input = false;

    if let Some(modalities) = &model.modalities {
        record_modalities(
            &modalities.input,
            &mut has_input_metadata,
            &mut has_image_input,
        );
    }
    record_modalities(
        &model.input_modalities,
        &mut has_input_metadata,
        &mut has_image_input,
    );
    if let Some(architecture) = &model.architecture {
        record_modalities(
            &architecture.input_modalities,
            &mut has_input_metadata,
            &mut has_image_input,
        );
        record_modality_string(
            architecture.modality.as_deref(),
            &mut has_input_metadata,
            &mut has_image_input,
        );
    }
    record_modality_string(
        model.modality.as_deref(),
        &mut has_input_metadata,
        &mut has_image_input,
    );

    has_input_metadata.then_some(has_image_input)
}

fn record_modalities(
    modalities: &[String],
    has_input_metadata: &mut bool,
    has_image_input: &mut bool,
) {
    if modalities.is_empty() {
        return;
    }
    *has_input_metadata = true;
    *has_image_input |= modalities
        .iter()
        .any(|value| modality_value_is_image(value));
}

fn record_modality_string(
    value: Option<&str>,
    has_input_metadata: &mut bool,
    has_image_input: &mut bool,
) {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return;
    };
    *has_input_metadata = true;
    let input_side = value.split("->").next().unwrap_or(value);
    *has_image_input |= modality_value_is_image(input_side);
}

fn modality_value_is_image(value: &str) -> bool {
    value
        .trim()
        .to_ascii_lowercase()
        .split(|ch: char| ch == '+' || ch == ',' || ch == '/' || ch.is_whitespace())
        .any(|token| {
            matches!(
                token,
                "image" | "images" | "vision" | "image_url" | "input_image"
            )
        })
}

fn model_matches_filter(model: &RawOpenCodeGoModel, filter: ModelDiscoveryFilter) -> bool {
    match filter {
        ModelDiscoveryFilter::All => true,
        ModelDiscoveryFilter::FreeOnly => is_free_model(model),
    }
}

fn is_free_model(model: &RawOpenCodeGoModel) -> bool {
    [
        Some(model.id.as_str()),
        model.name.as_deref(),
        model.display_name.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| FREE_MODEL_RE.is_match(value))
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

    fn raw_model(id: &str) -> RawOpenCodeGoModel {
        RawOpenCodeGoModel {
            id: id.to_string(),
            object: "model".to_string(),
            name: None,
            display_name: None,
            created: None,
            owned_by: None,
            modalities: None,
            input_modalities: Vec::new(),
            architecture: None,
            modality: None,
        }
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
                created: Some(1780207178),
                owned_by: Some("opencode".to_string()),
                ..raw_model("kimi-k2.6")
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
    fn image_support_uses_explicit_modalities_before_default_fallback() {
        let config = OpenCodeGoDiscoveryConfig::new_for_provider(
            OPENCODE_GO_PROVIDER_ID,
            OPENCODE_GO_PROVIDER_ID,
            DEFAULT_MODELS_URL.to_string(),
            Duration::from_secs(60),
            BTreeMap::new(),
            None,
            Some(true),
            ModelDiscoveryFilter::All,
        );
        let models = normalize_models_for_config(
            vec![
                raw_model("no-metadata-1"),
                raw_model("no-metadata-2"),
                RawOpenCodeGoModel {
                    modalities: Some(RawOpenCodeGoModelModalities {
                        input: vec!["text".to_string()],
                        output: Vec::new(),
                    }),
                    ..raw_model("explicit-text-only")
                },
                RawOpenCodeGoModel {
                    modalities: Some(RawOpenCodeGoModelModalities {
                        input: vec!["text".to_string(), "image".to_string()],
                        output: Vec::new(),
                    }),
                    ..raw_model("explicit-image")
                },
            ],
            DiscoverySource::Network,
            Utc::now(),
            &config,
        );

        // No explicit modalities → falls back to default_image_input=true
        assert!(model_by_id(&models, "no-metadata-1").supports_image_input);
        // Explicit text-only → overrides default
        assert!(!model_by_id(&models, "explicit-text-only").supports_image_input);
        // Explicit image → overrides default
        assert!(model_by_id(&models, "explicit-image").supports_image_input);
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

    #[test]
    fn zen_discovery_keeps_only_free_models() {
        let config = OpenCodeGoDiscoveryConfig::new_zen(
            "https://opencode.ai/zen/v1/models",
            Duration::from_secs(60),
            BTreeMap::new(),
        );

        let models = normalize_models_for_config(
            vec![
                RawOpenCodeGoModel {
                    name: Some("DeepSeek V4 Flash FREE".to_string()),
                    ..raw_model("deepseek-v4-flash-free")
                },
                RawOpenCodeGoModel {
                    name: Some("Big Pickle".to_string()),
                    ..raw_model("big-pickle")
                },
                RawOpenCodeGoModel {
                    name: Some("GPT 5.4".to_string()),
                    ..raw_model("gpt-5.4")
                },
            ],
            DiscoverySource::Network,
            Utc::now(),
            &config,
        );

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].provider_id, "opencode-zen");
        assert_eq!(
            models[0].qualified_id,
            "opencode-zen/deepseek-v4-flash-free"
        );
        assert_eq!(models[0].protocol, ModelProtocol::OpenAiChatCompletions);
    }

    #[tokio::test]
    async fn refresh_uses_last_known_good_cache_after_error() {
        let catalog = OpenCodeGoModelCatalog::new(
            HttpClient::new(),
            Some("test-key".to_string()),
            discovery_config(),
        );

        let network = catalog
            .refresh_with_fetcher(|| async { Ok(vec![raw_model("kimi-k2.6")]) })
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
    async fn refresh_returns_empty_when_cache_is_empty_after_error() {
        let catalog = OpenCodeGoModelCatalog::new(
            HttpClient::new(),
            Some("test-key".to_string()),
            discovery_config(),
        );

        let models = catalog
            .refresh_with_fetcher(|| async {
                Err(OpenCodeGoDiscoveryError::Network("offline".to_string()))
            })
            .await;

        assert!(models.is_empty());
    }

    #[tokio::test]
    async fn smoke_opencode_go_models_report_vision_via_models_dev() {
        if !matches!(
            std::env::var("RUN_OPENCODE_GO_DISCOVERY_SMOKE").as_deref(),
            Ok("1")
        ) {
            return;
        }

        let api_key = match std::env::var("OPENCODE_GO_API_KEY")
            .or_else(|_| std::env::var("OPENCODE_API_KEY"))
        {
            Ok(value) if !value.trim().is_empty() && value.trim() != "dummy" => value,
            _ => panic!("set OPENCODE_GO_API_KEY or OPENCODE_API_KEY for smoke test"),
        };

        // Initialize and populate Models.dev catalog for vision lookup
        init_models_dev_catalog(HttpClient::new());
        refresh_models_dev_catalog().await;

        let catalog =
            OpenCodeGoModelCatalog::new(HttpClient::new(), Some(api_key), discovery_config());
        let models = catalog.refresh().await;
        assert!(
            !models.is_empty(),
            "OpenCode /models returned no usable models"
        );
        // kimi-k2.5 has vision per Models.dev (exact ID match)
        assert!(model_by_id(&models, "kimi-k2.5").supports_image_input);
        // deepseek-v4-flash is text-only per Models.dev
        assert!(!model_by_id(&models, "deepseek-v4-flash").supports_image_input);
    }

    fn model_by_id<'a>(
        models: &'a [DiscoveredOpenCodeGoModel],
        model_id: &str,
    ) -> &'a DiscoveredOpenCodeGoModel {
        let Some(model) = models.iter().find(|model| model.model_id == model_id) else {
            panic!("model {model_id} should be discovered");
        };
        model
    }
}
