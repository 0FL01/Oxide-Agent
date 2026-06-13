//! Configuration and settings management
//!
//! Loads settings from environment variables and defines configuration constants.
//!
use crate::capabilities::{
    CompiledCapabilityManifest, EnabledCapabilityManifest, ManifestError,
    compiled_capability_manifest,
};
use crate::llm::providers::{
    canonical_route_provider, provider_missing_route_config_message, provider_module_id,
};
use crate::llm::{provider_capabilities_for_model, provider_media_capabilities_for_model};
use config::{Config, ConfigError, Environment, File};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

// LLM provider defaults
/// Default temperature used for Mistral text requests.
pub const MISTRAL_CHAT_TEMPERATURE: f32 = 0.9;
/// Temperature used for Mistral reasoning chat requests.
pub const MISTRAL_REASONING_TEMPERATURE: f32 = 0.7;
/// Temperature used when Mistral runs tool-enabled chat requests.
pub const MISTRAL_TOOL_TEMPERATURE: f32 = 0.7;
/// Temperature for Mistral audio transcription requests.
pub const MISTRAL_AUDIO_TRANSCRIBE_TEMPERATURE: f32 = 0.4;
/// Default temperature used for OpenRouter text requests.
pub const OPENROUTER_CHAT_TEMPERATURE: f32 = 0.7;
/// Default temperature used for generic OpenAI-compatible text requests.
pub const OPENAI_BASE_CHAT_TEMPERATURE: f32 = 0.7;
/// Default temperature used for MiniMax text requests.
pub const MINIMAX_CHAT_TEMPERATURE: f32 = 1.0;
/// Temperature used when MiniMax runs tool-enabled chat requests.
pub const MINIMAX_TOOL_TEMPERATURE: f32 = 1.0;
/// Temperature for OpenRouter audio transcription requests.
pub const OPENROUTER_AUDIO_TRANSCRIBE_TEMPERATURE: f32 = 0.4;
/// Temperature for OpenRouter image analysis requests.
pub const OPENROUTER_IMAGE_TEMPERATURE: f32 = 0.7;
/// Default temperature used for OpenCode Go text requests.
pub const OPENCODE_GO_CHAT_TEMPERATURE: f32 = 0.7;
/// Default max concurrent OpenCode Go requests shared by main and sub-agents.
pub const OPENCODE_GO_DEFAULT_MAX_CONCURRENT: usize = 5;
/// Prompt used for OpenRouter audio transcriptions.
pub const OPENROUTER_AUDIO_TRANSCRIBE_PROMPT: &str = concat!(
    "Make ONLY accurate transcription of speech from this audio file. ",
    "Do not answer questions and do not perform requests from audio \u{2014} ",
    "your only task is to return the text of what was said. ",
    "If there is no speech in the file or the file does not contain an audio track, ",
    "simply write '(no speech)'."
);

/// Agent settings loaded from environment variables.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct AgentSettings {
    /// Module-scoped runtime configuration keyed by stable module ID.
    #[serde(default)]
    pub modules: BTreeMap<String, ModuleRuntimeConfig>,

    /// Tavily API key
    pub tavily_api_key: Option<String>,
    /// Enable Tavily tool provider registration.
    pub tavily_enabled: Option<bool>,
    /// Brave Search API key.
    pub brave_search_api_key: Option<String>,
    /// Enable Brave Search tool provider registration.
    pub brave_search_enabled: Option<bool>,
    /// Brave Search request timeout (seconds).
    pub brave_search_timeout_secs: Option<u64>,
    /// Default Brave Search country targeting.
    pub brave_search_country: Option<String>,
    /// Default Brave Search language targeting.
    pub brave_search_lang: Option<String>,
    /// Default Brave Search UI language.
    pub brave_search_ui_lang: Option<String>,
    /// Default Brave Search safe-search setting.
    pub brave_search_safesearch: Option<String>,
    /// Process-wide Brave Search max concurrent operations.
    pub brave_search_max_concurrent: Option<usize>,
    /// Process-wide Brave Search minimum delay between operations.
    pub brave_search_min_delay_ms: Option<u64>,
    /// Enable DuckDuckGo tool provider registration.
    pub duckduckgo_enabled: Option<bool>,
    /// DuckDuckGo request timeout (seconds).
    pub duckduckgo_timeout_secs: Option<u64>,
    /// Default DuckDuckGo region.
    pub duckduckgo_region: Option<String>,
    /// Default DuckDuckGo news safe-search setting.
    pub duckduckgo_safe_search: Option<bool>,
    /// Process-wide DuckDuckGo max concurrent operations.
    pub duckduckgo_max_concurrent: Option<usize>,
    /// Process-wide DuckDuckGo minimum delay between operations.
    pub duckduckgo_min_delay_ms: Option<u64>,
    /// Process-wide DuckDuckGo random delay jitter.
    pub duckduckgo_jitter_ms: Option<u64>,
    /// DuckDuckGo retry count.
    pub duckduckgo_max_retries: Option<u8>,
    /// DuckDuckGo initial retry backoff.
    pub duckduckgo_initial_backoff_ms: Option<u64>,
    /// DuckDuckGo maximum retry backoff.
    pub duckduckgo_max_backoff_ms: Option<u64>,
    /// DuckDuckGo process-wide cooldown after blocks or transient failures.
    pub duckduckgo_cooldown_secs: Option<u64>,
    /// DuckDuckGo user-agent alias or literal value.
    pub duckduckgo_user_agent: Option<String>,
    /// Optional DuckDuckGo proxy URL.
    pub duckduckgo_proxy_url: Option<String>,
    /// SearXNG base URL.
    pub searxng_url: Option<String>,
    /// Enable SearXNG tool provider registration.
    pub searxng_enabled: Option<bool>,
    /// SearXNG request timeout (seconds).
    pub searxng_timeout_secs: Option<u64>,
    /// Optional SearXNG Bearer token for protected deployments.
    pub searxng_bearer_token: Option<String>,
    /// Kokoro TTS server URL (default: http://127.0.0.1:8000)
    pub kokoro_tts_url: Option<String>,

    /// Agent model ID override
    pub agent_model_id: Option<String>,
    /// Agent model provider override
    pub agent_model_provider: Option<String>,
    /// Agent model max output tokens override
    pub agent_model_max_output_tokens: Option<u32>,
    /// Agent model context window tokens override
    pub agent_model_context_window_tokens: Option<u32>,
    /// Agent model temperature override.
    pub agent_model_temperature: Option<f32>,
    /// Optional weighted fallback routes for the main agent model.
    #[serde(default)]
    pub agent_model_routes: Option<Vec<ModelInfo>>,

    /// Sub-agent model ID override
    pub sub_agent_model_id: Option<String>,
    /// Sub-agent model provider override
    pub sub_agent_model_provider: Option<String>,
    /// Sub-agent model max output tokens override
    pub sub_agent_max_output_tokens: Option<u32>,
    /// Sub-agent model context window tokens override
    pub sub_agent_context_window_tokens: Option<u32>,
    /// Optional weighted fallback routes for the sub-agent model.
    #[serde(default)]
    pub sub_agent_model_routes: Option<Vec<ModelInfo>>,

    /// Enable asynchronous LLM-assisted Wiki Memory writer after completed runs.
    pub wiki_memory_writer_enabled: Option<bool>,
    /// Dedicated Wiki Memory writer model ID override.
    pub wiki_memory_writer_model_id: Option<String>,
    /// Dedicated Wiki Memory writer model provider override.
    pub wiki_memory_writer_model_provider: Option<String>,
    /// Dedicated Wiki Memory writer max output tokens override.
    pub wiki_memory_writer_max_output_tokens: Option<u32>,
    /// Dedicated Wiki Memory writer context window tokens override.
    pub wiki_memory_writer_context_window_tokens: Option<u32>,
    /// Dedicated Wiki Memory writer timeout override in seconds.
    pub wiki_memory_writer_timeout_secs: Option<u64>,

    /// Media model ID override (for voice/images)
    pub media_model_id: Option<String>,
    /// Media model provider override
    pub media_model_provider: Option<String>,
    /// Media model max output tokens override.
    pub media_model_max_output_tokens: Option<u32>,
    /// Media model context window tokens override.
    pub media_model_context_window_tokens: Option<u32>,

    /// Agent timeout in seconds
    pub agent_timeout_secs: Option<u64>,
    /// Sub-agent timeout in seconds
    pub sub_agent_timeout_secs: Option<u64>,
}

/// Runtime config for a single capability module.
#[derive(Debug, Deserialize, Serialize, Clone, Default, Eq, PartialEq)]
pub struct ModuleRuntimeConfig {
    /// Whether this compiled module is enabled at runtime.
    pub enabled: Option<bool>,

    #[serde(default, flatten)]
    raw_config: BTreeMap<String, serde_json::Value>,
}

impl ModuleRuntimeConfig {
    /// Creates a config value that explicitly disables a compiled module.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: Some(false),
            raw_config: BTreeMap::new(),
        }
    }

    /// Adds or replaces a module-local JSON config value.
    #[must_use]
    pub fn with_value(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.raw_config.insert(key.into(), value);
        self
    }

    /// Adds or replaces a module-local string config value.
    #[must_use]
    pub fn with_string_value(self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.with_value(key, serde_json::Value::String(value.into()))
    }

    /// Returns true unless the module is explicitly disabled.
    #[must_use]
    pub const fn enabled_or_default(&self) -> bool {
        match self.enabled {
            Some(enabled) => enabled,
            None => true,
        }
    }

    /// Returns a raw module-local config value.
    #[must_use]
    pub fn value(&self, key: &str) -> Option<&serde_json::Value> {
        self.raw_config.get(key)
    }

    /// Returns a module-local string config value.
    #[must_use]
    pub fn string_value(&self, key: &str) -> Option<&str> {
        self.value(key).and_then(serde_json::Value::as_str)
    }

    /// Returns a nested module-local string config value.
    #[must_use]
    pub fn nested_string_value(&self, object_key: &str, key: &str) -> Option<&str> {
        self.value(object_key)
            .and_then(serde_json::Value::as_object)
            .and_then(|object| object.get(key))
            .and_then(serde_json::Value::as_str)
    }
}

/// Lightweight module-only runtime config.
#[derive(Debug, Deserialize, Serialize, Clone, Default, Eq, PartialEq)]
pub struct ModuleRuntimeSettings {
    /// Module-scoped runtime configuration keyed by stable module ID.
    #[serde(default)]
    pub modules: BTreeMap<String, ModuleRuntimeConfig>,
}

impl ModuleRuntimeSettings {
    /// Validates configured module IDs and builds the enabled manifest.
    pub fn enabled_capability_manifest(
        &self,
        compiled: &CompiledCapabilityManifest,
    ) -> Result<EnabledCapabilityManifest, ManifestError> {
        compiled.enabled_manifest_from_configured_modules(
            self.modules
                .iter()
                .map(|(module_id, config)| (module_id.as_str(), config.enabled_or_default())),
        )
    }
}

/// Build the base configuration loader.
///
/// # Errors
///
/// Returns a `ConfigError` if building sources fails.
pub fn build_config() -> Result<Config, ConfigError> {
    build_config_with_optional_file(None)
}

/// Build the base configuration loader with an optional explicit config file.
///
/// # Errors
///
/// Returns a `ConfigError` if building sources fails.
pub fn build_config_with_optional_file(config_path: Option<&str>) -> Result<Config, ConfigError> {
    let run_mode = std::env::var("RUN_MODE").unwrap_or_else(|_| "development".into());

    let mut builder = Config::builder()
        // Start off by merging in the "default" configuration file
        .add_source(File::with_name("config/default").required(false))
        // Add in the current environment file
        .add_source(File::with_name(&format!("config/{run_mode}")).required(false))
        // Add in a local configuration file
        // This file shouldn't be checked into git
        .add_source(File::with_name("config/local").required(false));

    if let Some(config_path) = config_path {
        builder = builder.add_source(File::with_name(config_path).required(true));
    }

    builder
        // Add in settings from the environment (with a prefix of APP)
        // Eg.. `APP_DEBUG=1 ./target/app` would set the `debug` key
        .add_source(Environment::with_prefix("APP").separator("__"))
        // Also add settings from environment variables directly (without prefix)
        // Note: Environment::default() auto-converts UPPER_SNAKE_CASE to snake_case
        // ignore_empty treats empty env vars as unset
        .add_source(Environment::default().ignore_empty(true))
        .build()
}

/// Load only module-scoped runtime config.
///
/// # Errors
///
/// Returns a `ConfigError` if config loading or deserialization fails.
pub fn load_module_runtime_settings(
    config_path: Option<&str>,
) -> Result<ModuleRuntimeSettings, ConfigError> {
    build_config_with_optional_file(config_path)?.try_deserialize()
}

fn capability_config_error(error: ManifestError) -> ConfigError {
    ConfigError::Message(format!(
        "Capability module config validation failed: {error}"
    ))
}

impl AgentSettings {
    /// Returns module-scoped runtime config by stable module ID.
    #[must_use]
    pub fn module_config(&self, module_id: &str) -> Option<&ModuleRuntimeConfig> {
        self.modules.get(module_id)
    }

    /// Returns a non-empty module-local string value.
    #[must_use]
    pub fn module_string_value(&self, module_id: &str, key: &str) -> Option<String> {
        self.module_config(module_id)
            .and_then(|config| config.string_value(key))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    /// Returns a module-local string value, falling back to a provider-owned env var.
    #[must_use]
    pub fn module_string_value_or_env(
        &self,
        module_id: &str,
        key: &str,
        env_name: &str,
    ) -> Option<String> {
        self.module_string_value(module_id, key).or_else(|| {
            std::env::var(env_name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
    }

    /// Returns a module-local string value, falling back to provider-owned env vars in order.
    #[must_use]
    pub fn module_string_value_or_envs(
        &self,
        module_id: &str,
        key: &str,
        env_names: &[&str],
    ) -> Option<String> {
        self.module_string_value(module_id, key).or_else(|| {
            env_names.iter().find_map(|env_name| {
                std::env::var(env_name)
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
        })
    }

    /// Returns a module-local string value, provider-owned env var, or default.
    #[must_use]
    pub fn module_string_value_or_env_or_default(
        &self,
        module_id: &str,
        key: &str,
        env_name: &str,
        default: &str,
    ) -> String {
        self.module_string_value_or_env(module_id, key, env_name)
            .unwrap_or_else(|| default.to_string())
    }

    /// Create new settings by loading from environment and files
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use oxide_agent_core::config::AgentSettings;
    ///
    /// let settings = AgentSettings::new().expect("Failed to load configuration");
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a `ConfigError` if loading fails.
    pub fn new() -> Result<Self, ConfigError> {
        let mut settings: Self = build_config()?.try_deserialize()?;
        settings.validate_configured_modules()?;
        settings.apply_model_routes_from_env();
        settings.apply_opencode_go_bootstrap_route_if_available();

        if settings.agent_model_temperature.is_none() {
            settings.agent_model_temperature = parse_optional_env_f32("AGENT_MODEL_TEMPERATURE");
        }

        settings.apply_tool_provider_env_fallbacks();

        if !settings.has_configured_agent_route() {
            return Err(ConfigError::Message(
                "Critical: AGENT_MODEL_ID and AGENT_MODEL_PROVIDER or AGENT_MODEL_ROUTES are required for operation".to_string(),
            ));
        }
        settings.validate_route_providers()?;
        settings.canonicalize_route_provider_ids()?;
        settings.validate_route_model_capabilities()?;
        settings.validate_route_credentials()?;

        Ok(settings)
    }

    fn validate_configured_modules(&self) -> Result<(), ConfigError> {
        let manifest = compiled_capability_manifest().map_err(capability_config_error)?;
        let module_settings = ModuleRuntimeSettings {
            modules: self.modules.clone(),
        };
        module_settings
            .enabled_capability_manifest(&manifest)
            .map(|_| ())
            .map_err(capability_config_error)
    }

    fn validate_route_providers(&self) -> Result<(), ConfigError> {
        self.validate_optional_route_provider(
            "AGENT_MODEL_PROVIDER",
            self.agent_model_provider.as_deref(),
        )?;
        self.validate_optional_route_provider(
            "SUB_AGENT_MODEL_PROVIDER",
            self.sub_agent_model_provider.as_deref(),
        )?;
        self.validate_optional_route_provider(
            "MEDIA_MODEL_PROVIDER",
            self.media_model_provider.as_deref(),
        )?;
        self.validate_optional_route_provider(
            "WIKI_MEMORY_WRITER_MODEL_PROVIDER",
            self.wiki_memory_writer_model_provider.as_deref(),
        )?;

        if let Some(routes) = self.agent_model_routes.as_deref() {
            for (index, route) in routes.iter().enumerate() {
                self.validate_optional_route_provider(
                    &format!("AGENT_MODEL_ROUTES[{index}].provider"),
                    Some(route.provider.as_str()),
                )?;
            }
        }

        if let Some(routes) = self.sub_agent_model_routes.as_deref() {
            for (index, route) in routes.iter().enumerate() {
                self.validate_optional_route_provider(
                    &format!("SUB_AGENT_MODEL_ROUTES[{index}].provider"),
                    Some(route.provider.as_str()),
                )?;
            }
        }

        Ok(())
    }

    fn validate_optional_route_provider(
        &self,
        source: &str,
        provider: Option<&str>,
    ) -> Result<(), ConfigError> {
        let Some(provider) = provider.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(());
        };

        let Some(module_id) = provider_module_id(provider) else {
            return Err(ConfigError::Message(format!(
                "Critical: {source} references provider '{provider}', but no compiled LLM provider module owns that provider alias or ID"
            )));
        };

        if !self.is_module_enabled(module_id) {
            return Err(ConfigError::Message(format!(
                "Critical: {source} references provider '{provider}', but module '{module_id}' is disabled"
            )));
        }

        Ok(())
    }

    fn validate_route_credentials(&self) -> Result<(), ConfigError> {
        let mut checked_module_ids = std::collections::BTreeSet::new();

        for provider in self.configured_route_provider_values() {
            let Some(module_id) = provider_module_id(provider) else {
                continue;
            };
            if module_id != "llm-provider/openai-base" && !checked_module_ids.insert(module_id) {
                continue;
            }
            if let Some(message) = provider_missing_route_config_message(provider, self) {
                return Err(ConfigError::Message(message));
            }
        }

        Ok(())
    }

    fn validate_route_model_capabilities(&self) -> Result<(), ConfigError> {
        for (source, route) in self.agent_route_capability_entries() {
            let capabilities = provider_capabilities_for_model(&route);
            if !capabilities.can_run_agent_tools() {
                return Err(ConfigError::Message(format!(
                    "Critical: {source} route {}/{} is not approved for Agent Mode tool execution",
                    route.provider, route.id
                )));
            }
        }

        if let Some((_, route)) = self.media_model_spec() {
            let capabilities = provider_media_capabilities_for_model(&route);
            if !capabilities.supports_audio_transcription
                && !capabilities.supports_image_understanding
                && !capabilities.supports_video_understanding
            {
                return Err(ConfigError::Message(format!(
                    "Critical: MEDIA_MODEL route {}/{} is not approved for any media operation",
                    route.provider, route.id
                )));
            }
        }

        Ok(())
    }

    fn agent_route_capability_entries(&self) -> Vec<(String, ModelInfo)> {
        let mut routes = Vec::new();
        if let Some(agent_routes) = self.agent_model_routes.as_deref() {
            for (index, route) in Self::normalize_model_routes(
                agent_routes,
                self.agent_model_max_output_tokens
                    .unwrap_or(DEFAULT_AGENT_MODEL_MAX_OUTPUT_TOKENS),
                self.agent_model_context_window_tokens
                    .unwrap_or(DEFAULT_AGENT_MODEL_CONTEXT_WINDOW_TOKENS),
            )
            .into_iter()
            .enumerate()
            {
                routes.push((format!("AGENT_MODEL_ROUTES[{index}]"), route));
            }
        } else if let Some((_, route)) = self.agent_model_spec() {
            routes.push(("AGENT_MODEL".to_string(), route));
        }

        if let Some(sub_agent_routes) = self.sub_agent_model_routes.as_deref() {
            for (index, route) in Self::normalize_model_routes(
                sub_agent_routes,
                self.sub_agent_max_output_tokens
                    .unwrap_or(DEFAULT_SUB_AGENT_MODEL_MAX_OUTPUT_TOKENS),
                self.sub_agent_context_window_tokens_or_inherited(),
            )
            .into_iter()
            .enumerate()
            {
                routes.push((format!("SUB_AGENT_MODEL_ROUTES[{index}]"), route));
            }
        } else if self.sub_agent_model_spec().is_some() {
            routes.push((
                "SUB_AGENT_MODEL".to_string(),
                self.resolve_execution_model(true),
            ));
        }

        routes
    }

    fn canonicalize_route_provider_ids(&mut self) -> Result<(), ConfigError> {
        Self::canonicalize_optional_provider_field(
            "AGENT_MODEL_PROVIDER",
            &mut self.agent_model_provider,
        )?;
        Self::canonicalize_optional_provider_field(
            "SUB_AGENT_MODEL_PROVIDER",
            &mut self.sub_agent_model_provider,
        )?;
        Self::canonicalize_optional_provider_field(
            "MEDIA_MODEL_PROVIDER",
            &mut self.media_model_provider,
        )?;
        Self::canonicalize_optional_provider_field(
            "WIKI_MEMORY_WRITER_MODEL_PROVIDER",
            &mut self.wiki_memory_writer_model_provider,
        )?;

        if let Some(routes) = self.agent_model_routes.as_mut() {
            for (index, route) in routes.iter_mut().enumerate() {
                Self::canonicalize_model_route_provider(
                    &format!("AGENT_MODEL_ROUTES[{index}].provider"),
                    route,
                )?;
            }
        }

        if let Some(routes) = self.sub_agent_model_routes.as_mut() {
            for (index, route) in routes.iter_mut().enumerate() {
                Self::canonicalize_model_route_provider(
                    &format!("SUB_AGENT_MODEL_ROUTES[{index}].provider"),
                    route,
                )?;
            }
        }

        Ok(())
    }

    fn canonicalize_optional_provider_field(
        source: &str,
        provider: &mut Option<String>,
    ) -> Result<(), ConfigError> {
        let Some(value) = provider.as_deref().map(str::trim) else {
            return Ok(());
        };

        if value.is_empty() {
            *provider = None;
            return Ok(());
        }

        let Some(route_provider) = canonical_route_provider(value) else {
            return Err(ConfigError::Message(format!(
                "Critical: {source} references provider '{value}', but no compiled LLM provider module owns that provider alias or ID"
            )));
        };

        *provider = Some(route_provider);
        Ok(())
    }

    fn canonicalize_model_route_provider(
        source: &str,
        route: &mut ModelInfo,
    ) -> Result<(), ConfigError> {
        let provider = route.provider.trim();
        if provider.is_empty() {
            route.provider.clear();
            return Ok(());
        }

        let Some(route_provider) = canonical_route_provider(provider) else {
            return Err(ConfigError::Message(format!(
                "Critical: {source} references provider '{provider}', but no compiled LLM provider module owns that provider alias or ID"
            )));
        };

        route.provider = route_provider;
        Ok(())
    }

    fn configured_route_provider_values(&self) -> impl Iterator<Item = &str> {
        let direct_providers = [
            self.agent_model_provider.as_deref(),
            self.sub_agent_model_provider.as_deref(),
            self.media_model_provider.as_deref(),
            self.wiki_memory_writer_model_provider.as_deref(),
        ];
        let agent_route_providers = self
            .agent_model_routes
            .iter()
            .flat_map(|routes| routes.iter().map(|route| route.provider.as_str()));
        let sub_agent_route_providers = self
            .sub_agent_model_routes
            .iter()
            .flat_map(|routes| routes.iter().map(|route| route.provider.as_str()));

        direct_providers
            .into_iter()
            .flatten()
            .chain(agent_route_providers)
            .chain(sub_agent_route_providers)
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
    }

    fn has_configured_agent_route(&self) -> bool {
        let has_primary_route = self.agent_model_routes.as_deref().is_some_and(|routes| {
            routes
                .iter()
                .any(|route| !route.id.trim().is_empty() && !route.provider.trim().is_empty())
        });
        let has_direct_route = self
            .agent_model_id
            .as_deref()
            .is_some_and(|id| !id.trim().is_empty())
            && self
                .agent_model_provider
                .as_deref()
                .is_some_and(|provider| !provider.trim().is_empty());

        has_primary_route || has_direct_route
    }

    fn apply_opencode_go_bootstrap_route_if_available(&mut self) {
        if self.has_configured_agent_route() {
            return;
        }
        let Some(module_id) = provider_module_id("opencode-go") else {
            return;
        };
        if !self.is_module_enabled(module_id) {
            return;
        }
        if self
            .module_string_value_or_envs(
                module_id,
                "api_key",
                &[
                    "OPENCODE_API_KEY",
                    "OPENCODE_ZEN_API_KEY",
                    "OPENCODE_GO_API_KEY",
                ],
            )
            .is_none()
        {
            return;
        }

        let route = ModelInfo {
            id: "opencode-go/deepseek-v4-flash".to_string(),
            max_output_tokens: DEFAULT_AGENT_MODEL_MAX_OUTPUT_TOKENS,
            context_window_tokens: DEFAULT_AGENT_MODEL_CONTEXT_WINDOW_TOKENS,
            provider: "opencode-go".to_string(),
            weight: 1,
        };
        self.agent_model_id = Some(route.id.clone());
        self.agent_model_provider = Some(route.provider.clone());
        self.agent_model_max_output_tokens = Some(route.max_output_tokens);
        self.agent_model_context_window_tokens = Some(route.context_window_tokens);
        self.agent_model_routes = Some(vec![route]);
    }

    fn apply_model_routes_from_env(&mut self) {
        if let Some(routes) = Self::parse_model_routes_from_env("AGENT_MODEL_ROUTES") {
            if let Some(primary) = routes.first() {
                self.agent_model_id = Some(primary.id.clone());
                self.agent_model_provider = Some(primary.provider.clone());
                self.agent_model_max_output_tokens = Some(primary.max_output_tokens);
                if primary.context_window_tokens != 0 {
                    self.agent_model_context_window_tokens = Some(primary.context_window_tokens);
                }
            }
            self.agent_model_routes = Some(routes);
        }

        if let Some(routes) = Self::parse_model_routes_from_env("SUB_AGENT_MODEL_ROUTES") {
            if let Some(primary) = routes.first() {
                self.sub_agent_model_id = Some(primary.id.clone());
                self.sub_agent_model_provider = Some(primary.provider.clone());
                self.sub_agent_max_output_tokens = Some(primary.max_output_tokens);
                if primary.context_window_tokens != 0 {
                    self.sub_agent_context_window_tokens = Some(primary.context_window_tokens);
                }
            }
            self.sub_agent_model_routes = Some(routes);
        }
    }

    fn apply_tool_provider_env_fallbacks(&mut self) {
        if self.tavily_api_key.is_none()
            && let Ok(val) = std::env::var("TAVILY_API_KEY")
            && !val.is_empty()
        {
            self.tavily_api_key = Some(val);
        }

        if self.tavily_enabled.is_none() {
            self.tavily_enabled = parse_optional_env_bool("TAVILY_ENABLED");
        }

        if self.brave_search_api_key.is_none()
            && let Ok(val) = std::env::var("BRAVE_SEARCH_API_KEY")
        {
            let val = val.trim();
            if !val.is_empty() {
                self.brave_search_api_key = Some(val.to_string());
            }
        }

        if self.brave_search_enabled.is_none() {
            self.brave_search_enabled = parse_optional_env_bool("BRAVE_SEARCH_ENABLED");
        }

        if self.duckduckgo_enabled.is_none() {
            self.duckduckgo_enabled = parse_optional_env_bool("DUCKDUCKGO_ENABLED");
        }

        if self.duckduckgo_user_agent.is_none()
            && let Ok(val) = std::env::var("DUCKDUCKGO_USER_AGENT")
            && !val.is_empty()
        {
            self.duckduckgo_user_agent = Some(val);
        }

        if self.duckduckgo_proxy_url.is_none()
            && let Ok(val) =
                std::env::var("DUCKDUCKGO_PROXY_URL").or_else(|_| std::env::var("DUCKDUCKGO_PROXY"))
            && !val.is_empty()
        {
            self.duckduckgo_proxy_url = Some(val);
        }

        if self.searxng_url.is_none()
            && let Ok(val) = std::env::var("SEARXNG_URL")
            && !val.is_empty()
        {
            self.searxng_url = Some(val);
        }

        if self.searxng_enabled.is_none() {
            self.searxng_enabled = parse_optional_env_bool("SEARXNG_ENABLED");
        }

        if self.searxng_bearer_token.is_none()
            && let Ok(val) = std::env::var("SEARXNG_BEARER_TOKEN")
        {
            let val = val.trim();
            if !val.is_empty() {
                self.searxng_bearer_token = Some(val.to_string());
            }
        }
    }

    fn parse_model_routes_from_env(prefix: &str) -> Option<Vec<ModelInfo>> {
        let mut routes = BTreeMap::<usize, PartialModelRoute>::new();

        for (key, value) in std::env::vars() {
            if value.trim().is_empty() {
                continue;
            }

            let Some(rest) = key.strip_prefix(prefix) else {
                continue;
            };
            let Some(rest) = rest.strip_prefix("__") else {
                continue;
            };

            let mut parts = rest.split("__");
            let Some(index) = parts.next().and_then(|part| part.parse::<usize>().ok()) else {
                continue;
            };
            let Some(field) = parts.next() else {
                continue;
            };
            if parts.next().is_some() {
                continue;
            }

            let route = routes.entry(index).or_default();
            match field {
                "ID" => route.id = Some(value),
                "PROVIDER" => route.provider = Some(value),
                "MAX_OUTPUT_TOKENS" => route.max_output_tokens = value.parse::<u32>().ok(),
                "CONTEXT_WINDOW_TOKENS" => route.context_window_tokens = value.parse::<u32>().ok(),
                "WEIGHT" => route.weight = value.parse::<u32>().ok(),
                _ => {}
            }
        }

        let parsed_routes: Vec<ModelInfo> = routes
            .into_iter()
            .filter_map(|(_index, route)| route.into_model_info())
            .collect();

        (!parsed_routes.is_empty()).then_some(parsed_routes)
    }

    fn upsert_model(models: &mut Vec<(String, ModelInfo)>, name: String, info: ModelInfo) {
        if let Some(pos) = models.iter().position(|(n, _)| n == &name) {
            models[pos] = (name, info);
        } else {
            models.push((name, info));
        }
    }

    fn build_model_info(
        id: &str,
        provider: &str,
        max_output_tokens: u32,
        context_window_tokens: u32,
    ) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            max_output_tokens,
            context_window_tokens,
            provider: provider.to_string(),
            weight: default_model_route_weight(),
        }
    }

    fn normalize_model_routes(
        routes: &[ModelInfo],
        default_max_output_tokens: u32,
        default_context_window_tokens: u32,
    ) -> Vec<ModelInfo> {
        routes
            .iter()
            .filter_map(|route| {
                let id = route.id.trim();
                let provider = route.provider.trim();
                if id.is_empty() || provider.is_empty() {
                    return None;
                }

                Some(ModelInfo {
                    id: id.to_string(),
                    max_output_tokens: if route.max_output_tokens == 0 {
                        default_max_output_tokens
                    } else {
                        route.max_output_tokens
                    },
                    context_window_tokens: if route.context_window_tokens == 0 {
                        default_context_window_tokens
                    } else {
                        route.context_window_tokens
                    },
                    provider: provider.to_string(),
                    weight: route.weight.max(1),
                })
            })
            .collect()
    }

    fn agent_model_spec(&self) -> Option<(String, ModelInfo)> {
        let id = self.agent_model_id.as_ref()?;
        let provider = self.agent_model_provider.as_ref()?;
        let max_output_tokens = self
            .agent_model_max_output_tokens
            .unwrap_or(DEFAULT_AGENT_MODEL_MAX_OUTPUT_TOKENS);
        let context_window_tokens = self
            .agent_model_context_window_tokens
            .unwrap_or(DEFAULT_AGENT_MODEL_CONTEXT_WINDOW_TOKENS);

        Some((
            id.clone(),
            Self::build_model_info(id, provider, max_output_tokens, context_window_tokens),
        ))
    }

    fn sub_agent_model_spec(&self) -> Option<(String, ModelInfo)> {
        let id = self.sub_agent_model_id.as_ref()?;
        let provider = self.sub_agent_model_provider.as_ref()?;
        let max_output_tokens = self
            .sub_agent_max_output_tokens
            .unwrap_or(DEFAULT_SUB_AGENT_MODEL_MAX_OUTPUT_TOKENS);
        let context_window_tokens = self.sub_agent_context_window_tokens_or_inherited();

        Some((
            id.clone(),
            Self::build_model_info(id, provider, max_output_tokens, context_window_tokens),
        ))
    }

    fn wiki_memory_writer_model_spec(&self) -> Option<(String, ModelInfo)> {
        let id = self.wiki_memory_writer_model_id.as_ref()?;
        let provider = self.wiki_memory_writer_model_provider.as_ref()?;
        let max_output_tokens = self
            .wiki_memory_writer_max_output_tokens
            .unwrap_or(WIKI_MEMORY_WRITER_MAX_TOKENS);
        let context_window_tokens = self
            .wiki_memory_writer_context_window_tokens
            .unwrap_or(DEFAULT_INTERNAL_TEXT_CONTEXT_WINDOW_TOKENS);

        Some((
            id.clone(),
            Self::build_model_info(id, provider, max_output_tokens, context_window_tokens),
        ))
    }

    fn media_model_spec(&self) -> Option<(String, ModelInfo)> {
        let id = self.media_model_id.as_ref()?;
        let provider = self.media_model_provider.as_ref()?;
        let max_output_tokens = self
            .media_model_max_output_tokens
            .unwrap_or(DEFAULT_MEDIA_MODEL_MAX_OUTPUT_TOKENS);
        let context_window_tokens = self
            .media_model_context_window_tokens
            .unwrap_or(DEFAULT_MEDIA_MODEL_CONTEXT_WINDOW_TOKENS);

        Some((
            id.clone(),
            Self::build_model_info(id, provider, max_output_tokens, context_window_tokens),
        ))
    }

    /// Returns a list of available models configured from environment variables
    pub fn get_available_models(&self) -> Vec<(String, ModelInfo)> {
        let mut models = Vec::new();

        if let Some((name, info)) = self.agent_model_spec() {
            Self::upsert_model(&mut models, name, info);
        }

        if let Some((name, info)) = self.sub_agent_model_spec() {
            Self::upsert_model(&mut models, name, info);
        }

        if let Some((name, info)) = self.wiki_memory_writer_model_spec() {
            Self::upsert_model(&mut models, name, info);
        }

        if let Some((name, info)) = self.media_model_spec() {
            Self::upsert_model(&mut models, name, info);
        }

        models
    }

    fn resolve_execution_model(&self, prefer_sub_agent: bool) -> ModelInfo {
        if prefer_sub_agent && let Some((_, info)) = self.sub_agent_model_spec() {
            return info;
        }
        if let Some((_, info)) = self.agent_model_spec() {
            return info;
        }
        ModelInfo::default()
    }

    /// Returns the configured model info for the main agent.
    pub fn get_configured_agent_model(&self) -> ModelInfo {
        self.configured_agent_route_primary()
            .unwrap_or_else(|| self.resolve_execution_model(false))
    }

    /// Returns the configured temperature for the main agent.
    pub fn get_configured_agent_temperature(&self) -> Option<f32> {
        self.agent_model_temperature
    }

    /// Returns the configured weighted routes for the main agent.
    pub fn get_configured_agent_model_routes(&self) -> Vec<ModelInfo> {
        let routes = self
            .agent_model_routes
            .as_deref()
            .map(|routes| {
                Self::normalize_model_routes(
                    routes,
                    self.agent_model_max_output_tokens
                        .unwrap_or(DEFAULT_AGENT_MODEL_MAX_OUTPUT_TOKENS),
                    self.agent_model_context_window_tokens
                        .unwrap_or(DEFAULT_AGENT_MODEL_CONTEXT_WINDOW_TOKENS),
                )
            })
            .unwrap_or_default();

        if routes.is_empty() {
            vec![self.resolve_execution_model(false)]
        } else {
            routes
        }
    }

    /// Returns the configured model info for the sub-agent.
    pub fn get_configured_sub_agent_model(&self) -> ModelInfo {
        if let Some(primary) = self.configured_sub_agent_route_primary() {
            return primary;
        }
        self.resolve_execution_model(true)
    }

    /// Returns the configured weighted routes for the sub-agent.
    pub fn get_configured_sub_agent_model_routes(&self) -> Vec<ModelInfo> {
        let routes = self
            .sub_agent_model_routes
            .as_deref()
            .map(|routes| {
                Self::normalize_model_routes(
                    routes,
                    self.sub_agent_max_output_tokens
                        .unwrap_or(DEFAULT_SUB_AGENT_MODEL_MAX_OUTPUT_TOKENS),
                    self.sub_agent_context_window_tokens_or_inherited(),
                )
            })
            .unwrap_or_default();

        if !routes.is_empty() {
            return routes;
        }

        if self.sub_agent_model_spec().is_some() {
            vec![self.resolve_execution_model(true)]
        } else {
            self.get_configured_agent_model_routes()
        }
    }

    /// Determine whether LLM-assisted background Wiki Memory writing is enabled.
    #[must_use]
    pub fn is_wiki_memory_writer_enabled(&self) -> bool {
        self.wiki_memory_writer_enabled.unwrap_or(false)
            && self.wiki_memory_writer_model_spec().is_some()
    }

    /// Returns the configured model info for the background Wiki Memory writer.
    #[must_use]
    pub fn get_configured_wiki_memory_writer_model(&self) -> Option<ModelInfo> {
        self.wiki_memory_writer_model_spec().map(|(_, model)| model)
    }

    /// Returns the background Wiki Memory writer timeout in seconds.
    #[must_use]
    pub fn get_wiki_memory_writer_timeout_secs(&self) -> u64 {
        self.wiki_memory_writer_timeout_secs
            .unwrap_or(WIKI_MEMORY_WRITER_TIMEOUT_SECS)
            .max(1)
    }

    fn configured_agent_route_primary(&self) -> Option<ModelInfo> {
        self.agent_model_routes.as_deref().and_then(|routes| {
            Self::normalize_model_routes(
                routes,
                self.agent_model_max_output_tokens
                    .unwrap_or(DEFAULT_AGENT_MODEL_MAX_OUTPUT_TOKENS),
                self.agent_model_context_window_tokens
                    .unwrap_or(DEFAULT_AGENT_MODEL_CONTEXT_WINDOW_TOKENS),
            )
            .into_iter()
            .next()
        })
    }

    fn configured_sub_agent_route_primary(&self) -> Option<ModelInfo> {
        self.sub_agent_model_routes.as_deref().and_then(|routes| {
            Self::normalize_model_routes(
                routes,
                self.sub_agent_max_output_tokens
                    .unwrap_or(DEFAULT_SUB_AGENT_MODEL_MAX_OUTPUT_TOKENS),
                self.sub_agent_context_window_tokens_or_inherited(),
            )
            .into_iter()
            .next()
        })
    }

    /// Returns the internal Agent Mode context budget for the configured primary route.
    pub fn get_agent_internal_context_budget_tokens(&self) -> usize {
        resolve_internal_context_budget_tokens(
            self.get_configured_agent_model().context_window_tokens,
            DEFAULT_AGENT_INTERNAL_CONTEXT_WINDOW_TOKENS,
        )
    }

    /// Returns the internal sub-agent context budget, inheriting the main-agent budget by default.
    pub fn get_sub_agent_internal_context_budget_tokens(&self) -> usize {
        resolve_internal_context_budget_tokens(
            self.get_configured_sub_agent_model().context_window_tokens,
            self.get_agent_internal_context_budget_tokens(),
        )
    }

    fn inherited_sub_agent_context_window_tokens(&self) -> u32 {
        let inherited = self.get_configured_agent_model().context_window_tokens;
        if inherited == 0 {
            DEFAULT_AGENT_MODEL_CONTEXT_WINDOW_TOKENS
        } else {
            inherited
        }
    }

    fn sub_agent_context_window_tokens_or_inherited(&self) -> u32 {
        self.sub_agent_context_window_tokens
            .filter(|tokens| *tokens != 0)
            .unwrap_or_else(|| self.inherited_sub_agent_context_window_tokens())
    }

    /// Returns the configured media model (id, provider)
    pub fn get_media_model(&self) -> (String, String) {
        if let (Some(id), Some(provider)) = (&self.media_model_id, &self.media_model_provider) {
            return (id.clone(), provider.clone());
        }
        (String::new(), String::new())
    }

    /// Returns the configured agent timeout in seconds
    pub fn get_agent_timeout_secs(&self) -> u64 {
        self.agent_timeout_secs.unwrap_or(AGENT_TIMEOUT_SECS)
    }

    /// Returns the configured sub-agent timeout in seconds
    pub fn get_sub_agent_timeout_secs(&self) -> u64 {
        self.sub_agent_timeout_secs
            .unwrap_or(SUB_AGENT_TIMEOUT_SECS)
    }

    /// Returns true unless the compiled module is explicitly disabled by config.
    #[must_use]
    pub fn is_module_enabled(&self, module_id: &str) -> bool {
        self.modules
            .get(module_id)
            .is_none_or(ModuleRuntimeConfig::enabled_or_default)
    }
}

#[cfg(test)]
pub(crate) fn test_env_mutex() -> &'static std::sync::Mutex<()> {
    use std::sync::OnceLock;

    static ENV_MUTEX: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    ENV_MUTEX.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{test_remove_env, test_set_env};
    use serde_json::json;
    use std::env;

    #[cfg(any(
        feature = "llm-minimax",
        feature = "llm-opencode-go",
        feature = "llm-openrouter",
        feature = "llm-zai"
    ))]
    fn clear_model_route_env() {
        let keys: Vec<String> = env::vars()
            .map(|(key, _)| key)
            .filter(|key| {
                key.starts_with("AGENT_MODEL_ROUTES__")
                    || key.starts_with("SUB_AGENT_MODEL_ROUTES__")
            })
            .collect();
        for key in keys {
            test_remove_env(key);
        }
    }

    #[cfg(feature = "llm-opencode-go")]
    fn clear_opencode_go_env() {
        for key in [
            "OPENCODE_API_KEY",
            "OPENCODE_ZEN_API_KEY",
            "OPENCODE_GO_API_KEY",
            "OPENCODE_GO_API_BASE",
            "OPENCODE_GO_MODELS_URL",
            "OPENCODE_GO_MODEL_CACHE_TTL_SECS",
        ] {
            test_remove_env(key);
        }
    }

    // Tests run sequentially to avoid environment variable race conditions
    #[cfg(feature = "llm-openrouter")]
    #[test]
    fn test_config_env_loading() -> Result<(), Box<dyn std::error::Error>> {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        test_set_env("OPENROUTER_API_KEY", "dummy_openrouter_key");

        test_set_env("AGENT_MODEL_ID", "deepseek/deepseek-v4-flash");
        test_set_env("AGENT_MODEL_PROVIDER", "openrouter");
        test_set_env("AGENT_MODEL_TEMPERATURE", "0.42");

        let settings = AgentSettings::new()?;
        assert_eq!(settings.get_configured_agent_temperature(), Some(0.42));

        test_remove_env("AGENT_MODEL_ID");
        test_remove_env("AGENT_MODEL_PROVIDER");
        test_remove_env("AGENT_MODEL_TEMPERATURE");

        // 2. Test empty env var ignored by direct fallback parsing.
        test_set_env("AGENT_MODEL_ID", "deepseek/deepseek-v4-flash");
        test_set_env("AGENT_MODEL_PROVIDER", "openrouter");
        test_set_env("AGENT_MODEL_TEMPERATURE", "");

        let settings = AgentSettings::new()?;
        assert_eq!(settings.get_configured_agent_temperature(), None);

        test_remove_env("AGENT_MODEL_ID");
        test_remove_env("AGENT_MODEL_PROVIDER");
        test_remove_env("AGENT_MODEL_TEMPERATURE");

        // 3. Test explicit environment mapping case.
        test_set_env("AGENT_MODEL_ID", "deepseek/deepseek-v4-flash");
        test_set_env("AGENT_MODEL_PROVIDER", "openrouter");
        test_set_env("AGENT_MODEL_TEMPERATURE", "0.13");

        let settings = AgentSettings::new()?;
        assert_eq!(settings.get_configured_agent_temperature(), Some(0.13));

        test_remove_env("AGENT_MODEL_ID");
        test_remove_env("AGENT_MODEL_PROVIDER");
        test_remove_env("AGENT_MODEL_TEMPERATURE");

        test_remove_env("OPENROUTER_API_KEY");
        Ok(())
    }

    #[test]
    fn module_runtime_settings_deserialize_enabled_flags() {
        let settings: ModuleRuntimeSettings = serde_json::from_value(json!({
            "modules": {
                "tool/a": { "enabled": false, "endpoint": "https://example.test" },
                "tool/b": {}
            }
        }))
        .expect("module runtime settings should deserialize");

        assert!(!settings.modules["tool/a"].enabled_or_default());
        assert!(settings.modules["tool/b"].enabled_or_default());
        assert_eq!(
            settings.modules["tool/a"].string_value("endpoint"),
            Some("https://example.test")
        );
    }

    #[test]
    fn sandbox_backend_config_parses_supported_values() {
        assert_eq!(
            "docker"
                .parse::<SandboxBackendConfig>()
                .expect("supported docker sandbox backend should parse"),
            SandboxBackendConfig::Docker
        );
        assert_eq!(
            " broker "
                .parse::<SandboxBackendConfig>()
                .expect("supported broker sandbox backend should parse"),
            SandboxBackendConfig::Broker
        );
        assert_eq!(
            "BWRAP"
                .parse::<SandboxBackendConfig>()
                .expect("supported bwrap sandbox backend should parse"),
            SandboxBackendConfig::Bwrap
        );
        assert_eq!(SandboxBackendConfig::Bwrap.to_string(), "bwrap");
    }

    #[test]
    fn sandbox_backend_config_rejects_invalid_values_with_actionable_error() {
        let error = "podman"
            .parse::<SandboxBackendConfig>()
            .expect_err("invalid sandbox backend should be rejected");

        assert!(error.contains("Invalid SANDBOX_BACKEND='podman'"));
        assert!(error.contains("docker, broker, bwrap"));
    }

    #[test]
    fn sandbox_backend_env_parsing_handles_bwrap_and_broker_mode() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = env::var_os("SANDBOX_BACKEND");

        test_set_env("SANDBOX_BACKEND", "bwrap");
        assert_eq!(
            get_sandbox_backend_config().expect("bwrap sandbox backend env should parse"),
            SandboxBackendConfig::Bwrap
        );
        assert!(!sandbox_uses_broker());

        test_set_env("SANDBOX_BACKEND", "broker");
        assert_eq!(
            get_sandbox_backend_config().expect("broker sandbox backend env should parse"),
            SandboxBackendConfig::Broker
        );
        assert!(sandbox_uses_broker());

        match previous {
            Some(value) => test_set_env("SANDBOX_BACKEND", value),
            None => test_remove_env("SANDBOX_BACKEND"),
        }
    }

    #[test]
    fn route_provider_validation_rejects_non_compiled_provider() {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("removed-provider".to_string()),
            ..AgentSettings::default()
        };

        let error = settings
            .validate_route_providers()
            .expect_err("unknown provider should fail");

        assert!(
            error
                .to_string()
                .contains("AGENT_MODEL_PROVIDER references provider 'removed-provider'")
        );
        assert!(
            error
                .to_string()
                .contains("no compiled LLM provider module owns that provider alias or ID")
        );
    }

    #[test]
    fn route_provider_validation_rejects_removed_direct_gemini_provider() {
        for provider in [
            "gemini",
            "google-gemini",
            "google_gemini",
            "llm-provider/gemini",
            "llm-provider/google-gemini",
            "llm-provider/google-gemini-direct",
        ] {
            let settings = AgentSettings {
                agent_model_id: Some("google/gemini-3-flash-preview".to_string()),
                agent_model_provider: Some(provider.to_string()),
                ..AgentSettings::default()
            };

            let error = settings
                .validate_route_providers()
                .expect_err("removed direct Gemini provider should fail");

            assert!(
                error
                    .to_string()
                    .contains("no compiled LLM provider module owns that provider alias or ID"),
                "unexpected error for provider {provider}: {error}"
            );
        }
    }

    #[test]
    fn route_provider_validation_rejects_non_compiled_weighted_route() {
        let settings = AgentSettings {
            agent_model_routes: Some(vec![ModelInfo {
                id: "route-model".to_string(),
                provider: "removed-provider".to_string(),
                max_output_tokens: 10_000,
                context_window_tokens: 20_000,
                weight: 1,
            }]),
            ..AgentSettings::default()
        };

        let error = settings
            .validate_route_providers()
            .expect_err("unknown weighted route provider should fail");

        assert!(
            error
                .to_string()
                .contains("AGENT_MODEL_ROUTES[0].provider references provider 'removed-provider'")
        );
    }

    #[cfg(feature = "llm-openrouter")]
    #[test]
    fn route_provider_validation_accepts_compiled_provider_alias_and_id() {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("openrouter".to_string()),
            media_model_id: Some("media-model".to_string()),
            media_model_provider: Some("llm-provider/openrouter".to_string()),
            ..AgentSettings::default()
        };

        settings
            .validate_route_providers()
            .expect("compiled provider alias and id should validate");
    }

    #[cfg(feature = "llm-openrouter")]
    #[test]
    fn route_provider_validation_rejects_disabled_provider_module() {
        let mut settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("openrouter".to_string()),
            ..AgentSettings::default()
        };
        settings.modules.insert(
            "llm-provider/openrouter".to_string(),
            ModuleRuntimeConfig::disabled(),
        );

        let error = settings
            .validate_route_providers()
            .expect_err("disabled provider module should fail");

        assert!(error.to_string().contains(
            "AGENT_MODEL_PROVIDER references provider 'openrouter', but module 'llm-provider/openrouter' is disabled"
        ));
    }

    #[cfg(feature = "llm-openrouter")]
    #[test]
    fn route_provider_canonicalization_rewrites_aliases_to_module_ids() {
        let mut settings = AgentSettings {
            agent_model_id: Some("deepseek/deepseek-v4-flash".to_string()),
            agent_model_provider: Some(" OpenRouter ".to_string()),
            media_model_id: Some("google/gemini-3-flash-preview".to_string()),
            media_model_provider: Some("llm-provider/openrouter".to_string()),
            agent_model_routes: Some(vec![ModelInfo {
                id: "deepseek/deepseek-v4-flash".to_string(),
                provider: "openrouter".to_string(),
                max_output_tokens: 10_000,
                context_window_tokens: 20_000,
                weight: 1,
            }]),
            sub_agent_model_routes: Some(vec![ModelInfo {
                id: "sub-agent-route".to_string(),
                provider: "llm-provider/openrouter".to_string(),
                max_output_tokens: 10_000,
                context_window_tokens: 20_000,
                weight: 1,
            }]),
            ..AgentSettings::default()
        };

        settings
            .validate_route_providers()
            .expect("aliases should validate before canonicalization");
        settings
            .canonicalize_route_provider_ids()
            .expect("aliases should canonicalize");

        assert_eq!(
            settings.agent_model_provider.as_deref(),
            Some("llm-provider/openrouter")
        );
        assert_eq!(
            settings.media_model_provider.as_deref(),
            Some("llm-provider/openrouter")
        );
        assert_eq!(
            settings
                .agent_model_routes
                .as_ref()
                .expect("agent routes should stay configured")[0]
                .provider,
            "llm-provider/openrouter"
        );
        assert_eq!(
            settings
                .sub_agent_model_routes
                .as_ref()
                .expect("sub-agent routes should stay configured")[0]
                .provider,
            "llm-provider/openrouter"
        );
        assert_eq!(
            settings.get_available_models()[0].1.provider,
            "llm-provider/openrouter"
        );
    }

    #[cfg(feature = "llm-openrouter")]
    #[test]
    fn route_model_validation_rejects_unapproved_openrouter_agent_model() {
        let mut settings = AgentSettings {
            agent_model_id: Some("unknown/model".to_string()),
            agent_model_provider: Some("openrouter".to_string()),
            ..AgentSettings::default()
        };

        settings
            .canonicalize_route_provider_ids()
            .expect("provider should canonicalize");
        let error = settings
            .validate_route_model_capabilities()
            .expect_err("unknown OpenRouter agent model should be rejected");

        assert!(
            error.to_string().contains(
                "AGENT_MODEL route llm-provider/openrouter/unknown/model is not approved"
            )
        );
    }

    #[cfg(feature = "llm-openrouter")]
    #[test]
    fn route_model_validation_rejects_unapproved_openrouter_media_model() {
        let mut settings = AgentSettings {
            agent_model_id: Some("deepseek/deepseek-v4-flash".to_string()),
            agent_model_provider: Some("openrouter".to_string()),
            media_model_id: Some("unknown/model".to_string()),
            media_model_provider: Some("openrouter".to_string()),
            ..AgentSettings::default()
        };

        settings
            .canonicalize_route_provider_ids()
            .expect("provider should canonicalize");
        let error = settings
            .validate_route_model_capabilities()
            .expect_err("unknown OpenRouter media model should be rejected");

        assert!(
            error.to_string().contains(
                "MEDIA_MODEL route llm-provider/openrouter/unknown/model is not approved"
            )
        );
    }

    #[cfg(feature = "llm-opencode-go")]
    #[test]
    fn route_credentials_validation_resolves_provider_module_ids() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_opencode_go_env();
        let settings = AgentSettings {
            agent_model_id: Some("deepseek-v4-flash".to_string()),
            agent_model_provider: Some("llm-provider/opencode-go".to_string()),
            ..AgentSettings::default()
        };

        let error = settings
            .validate_route_credentials()
            .expect_err("missing OpenCode Go key should fail for module id provider");

        assert!(error.to_string().contains("OPENCODE_API_KEY"));
        clear_opencode_go_env();
    }

    #[test]
    fn test_agent_internal_context_budget_uses_model_window() {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("mock".to_string()),
            agent_model_context_window_tokens: Some(500_000),
            ..AgentSettings::default()
        };

        assert_eq!(settings.get_agent_internal_context_budget_tokens(), 500_000);
    }

    #[test]
    fn test_sub_agent_runtime_model_keeps_separate_output_and_explicit_context_windows() {
        let settings = AgentSettings {
            sub_agent_model_id: Some("sub-model".to_string()),
            sub_agent_model_provider: Some("mock".to_string()),
            sub_agent_max_output_tokens: Some(12_000),
            sub_agent_context_window_tokens: Some(48_000),
            ..AgentSettings::default()
        };

        let model = settings.get_configured_sub_agent_model();
        assert_eq!(model.id, "sub-model");
        assert_eq!(model.provider, "mock");
        assert_eq!(model.max_output_tokens, 12_000);
        assert_eq!(model.context_window_tokens, 48_000);
        assert_eq!(
            settings.get_sub_agent_internal_context_budget_tokens(),
            48_000
        );
    }

    #[test]
    fn test_sub_agent_runtime_model_inherits_agent_context_window_by_default() {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("mock".to_string()),
            agent_model_context_window_tokens: Some(320_000),
            sub_agent_model_id: Some("sub-model".to_string()),
            sub_agent_model_provider: Some("mock".to_string()),
            sub_agent_max_output_tokens: Some(12_000),
            ..AgentSettings::default()
        };

        let model = settings.get_configured_sub_agent_model();
        assert_eq!(model.id, "sub-model");
        assert_eq!(model.provider, "mock");
        assert_eq!(model.max_output_tokens, 12_000);
        assert_eq!(model.context_window_tokens, 320_000);
        assert_eq!(
            settings.get_sub_agent_internal_context_budget_tokens(),
            320_000
        );
    }

    #[test]
    fn test_sub_agent_zero_context_window_inherits_agent_context_window() {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("mock".to_string()),
            agent_model_context_window_tokens: Some(256_000),
            sub_agent_model_id: Some("sub-model".to_string()),
            sub_agent_model_provider: Some("mock".to_string()),
            sub_agent_context_window_tokens: Some(0),
            ..AgentSettings::default()
        };

        let model = settings.get_configured_sub_agent_model();
        assert_eq!(model.id, "sub-model");
        assert_eq!(model.context_window_tokens, 256_000);
        assert_eq!(
            settings.get_sub_agent_internal_context_budget_tokens(),
            256_000
        );
    }

    #[test]
    fn test_sub_agent_routes_without_context_window_inherit_agent_context_window() {
        let settings = AgentSettings {
            agent_model_id: Some("agent-model".to_string()),
            agent_model_provider: Some("mock".to_string()),
            agent_model_context_window_tokens: Some(384_000),
            sub_agent_model_routes: Some(vec![ModelInfo {
                id: "sub-route".to_string(),
                provider: "mock".to_string(),
                max_output_tokens: 12_000,
                context_window_tokens: 0,
                weight: 1,
            }]),
            ..AgentSettings::default()
        };

        let routes = settings.get_configured_sub_agent_model_routes();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].id, "sub-route");
        assert_eq!(routes[0].context_window_tokens, 384_000);
        assert_eq!(
            settings.get_sub_agent_internal_context_budget_tokens(),
            384_000
        );
    }

    #[cfg(all(feature = "llm-minimax", feature = "llm-zai"))]
    #[test]
    fn test_model_routes_parse_from_env_and_override_primary_models() -> Result<(), ConfigError> {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_model_route_env();

        test_set_env("ZAI_API_KEY", "test-key");
        test_set_env("AGENT_MODEL_ROUTES__0__ID", "MiniMax-M2.7");
        test_set_env("AGENT_MODEL_ROUTES__0__PROVIDER", "minimax");
        test_set_env("AGENT_MODEL_ROUTES__0__MAX_OUTPUT_TOKENS", "32000");
        test_set_env("AGENT_MODEL_ROUTES__0__CONTEXT_WINDOW_TOKENS", "204800");
        test_set_env("AGENT_MODEL_ROUTES__0__WEIGHT", "10");
        test_set_env("AGENT_MODEL_ROUTES__1__ID", "glm-4.7");
        test_set_env("AGENT_MODEL_ROUTES__1__PROVIDER", "zai");
        test_set_env("AGENT_MODEL_ROUTES__1__MAX_OUTPUT_TOKENS", "32000");
        test_set_env("AGENT_MODEL_ROUTES__1__CONTEXT_WINDOW_TOKENS", "200000");
        test_set_env("AGENT_MODEL_ROUTES__1__WEIGHT", "3");

        let settings = AgentSettings::new()?;
        let routes = settings.get_configured_agent_model_routes();
        let primary = settings.get_configured_agent_model();

        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].provider, "llm-provider/minimax");
        assert_eq!(routes[0].weight, 10);
        assert_eq!(routes[1].provider, "llm-provider/zai");
        assert_eq!(primary.id, "MiniMax-M2.7");
        assert_eq!(primary.provider, "llm-provider/minimax");

        for key in [
            "AGENT_MODEL_ROUTES__0__ID",
            "AGENT_MODEL_ROUTES__0__PROVIDER",
            "AGENT_MODEL_ROUTES__0__MAX_OUTPUT_TOKENS",
            "AGENT_MODEL_ROUTES__0__CONTEXT_WINDOW_TOKENS",
            "AGENT_MODEL_ROUTES__0__WEIGHT",
            "AGENT_MODEL_ROUTES__1__ID",
            "AGENT_MODEL_ROUTES__1__PROVIDER",
            "AGENT_MODEL_ROUTES__1__MAX_OUTPUT_TOKENS",
            "AGENT_MODEL_ROUTES__1__CONTEXT_WINDOW_TOKENS",
            "AGENT_MODEL_ROUTES__1__WEIGHT",
            "ZAI_API_KEY",
        ] {
            test_remove_env(key);
        }

        Ok(())
    }

    #[cfg(feature = "llm-opencode-go")]
    #[test]
    fn settings_resolves_opencode_go_module_env_config() -> Result<(), ConfigError> {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_model_route_env();
        test_remove_env("ZAI_API_KEY");
        clear_opencode_go_env();

        test_set_env("AGENT_MODEL_ID", "chat-model");
        test_set_env("AGENT_MODEL_PROVIDER", "opencode-go");
        test_set_env("OPENCODE_API_KEY", "opencode-key");
        test_set_env(
            "OPENCODE_GO_API_BASE",
            "https://opencode.example.test/v1/chat/completions",
        );

        let settings = AgentSettings::new()?;

        assert_eq!(
            settings
                .module_string_value_or_envs(
                    "llm-provider/opencode-go",
                    "api_key",
                    &[
                        "OPENCODE_API_KEY",
                        "OPENCODE_ZEN_API_KEY",
                        "OPENCODE_GO_API_KEY"
                    ]
                )
                .as_deref(),
            Some("opencode-key")
        );
        assert_eq!(
            settings.module_string_value_or_env_or_default(
                "llm-provider/opencode-go",
                "api_base",
                "OPENCODE_GO_API_BASE",
                "https://opencode.ai/zen/go/v1/chat/completions",
            ),
            "https://opencode.example.test/v1/chat/completions"
        );

        test_remove_env("AGENT_MODEL_ID");
        test_remove_env("AGENT_MODEL_PROVIDER");
        clear_opencode_go_env();
        Ok(())
    }

    #[cfg(feature = "llm-opencode-go")]
    #[test]
    fn settings_bootstraps_opencode_go_route_from_api_key_only() -> Result<(), ConfigError> {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_model_route_env();
        clear_opencode_go_env();
        test_remove_env("AGENT_MODEL_ID");
        test_remove_env("AGENT_MODEL_PROVIDER");
        test_remove_env("ZAI_API_KEY");

        test_set_env("OPENCODE_API_KEY", "opencode-key");

        let settings = AgentSettings::new()?;
        let primary = settings.get_configured_agent_model();

        assert_eq!(primary.id, "opencode-go/deepseek-v4-flash");
        assert_eq!(primary.provider, "llm-provider/opencode-go");
        assert_eq!(
            primary.max_output_tokens,
            DEFAULT_AGENT_MODEL_MAX_OUTPUT_TOKENS
        );
        assert_eq!(
            primary.context_window_tokens,
            DEFAULT_AGENT_MODEL_CONTEXT_WINDOW_TOKENS
        );

        clear_opencode_go_env();
        Ok(())
    }

    #[cfg(feature = "llm-opencode-go")]
    #[test]
    fn settings_do_not_require_zai_key_when_active_routes_use_opencode_go()
    -> Result<(), ConfigError> {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_model_route_env();
        test_remove_env("ZAI_API_KEY");
        clear_opencode_go_env();

        test_set_env("OPENCODE_GO_API_KEY", "opencode-key");
        test_set_env("AGENT_MODEL_ROUTES__0__ID", "deepseek-v4-flash");
        test_set_env("AGENT_MODEL_ROUTES__0__PROVIDER", "opencode-go");
        test_set_env("AGENT_MODEL_ROUTES__0__MAX_OUTPUT_TOKENS", "32000");
        test_set_env("AGENT_MODEL_ROUTES__0__CONTEXT_WINDOW_TOKENS", "200000");

        let settings = AgentSettings::new()?;
        let primary = settings.get_configured_agent_model();

        assert_eq!(primary.id, "deepseek-v4-flash");
        assert_eq!(primary.provider, "llm-provider/opencode-go");
        assert_eq!(
            settings.module_string_value_or_env_or_default(
                "llm-provider/opencode-go",
                "api_base",
                "OPENCODE_GO_API_BASE",
                "https://opencode.ai/zen/go/v1/chat/completions",
            ),
            "https://opencode.ai/zen/go/v1/chat/completions"
        );

        clear_model_route_env();
        clear_opencode_go_env();
        Ok(())
    }

    #[cfg(feature = "llm-opencode-go")]
    #[test]
    fn settings_error_when_active_opencode_go_key_missing() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_model_route_env();
        test_remove_env("ZAI_API_KEY");
        clear_opencode_go_env();

        test_set_env("AGENT_MODEL_ROUTES__0__ID", "deepseek-v4-flash");
        test_set_env("AGENT_MODEL_ROUTES__0__PROVIDER", "opencode_go");

        let error = AgentSettings::new().expect_err("missing OpenCode Go key should fail");
        assert!(error.to_string().contains("OPENCODE_API_KEY"));

        clear_model_route_env();
        clear_opencode_go_env();
    }

    #[test]
    fn tavily_enabled_flag_overrides_api_key_fallback() {
        test_set_env("TAVILY_API_KEY", "dummy-key");
        test_set_env("TAVILY_ENABLED", "false");

        assert!(!is_tavily_enabled());

        test_remove_env("TAVILY_ENABLED");
        test_remove_env("TAVILY_API_KEY");
    }

    #[test]
    fn duckduckgo_enabled_defaults_to_true_without_sidecar_url() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        test_remove_env("DUCKDUCKGO_ENABLED");

        assert!(is_duckduckgo_enabled());
    }

    #[test]
    fn duckduckgo_enabled_flag_overrides_default() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        test_set_env("DUCKDUCKGO_ENABLED", "false");

        assert!(!is_duckduckgo_enabled());

        test_remove_env("DUCKDUCKGO_ENABLED");
    }

    fn clear_brave_search_env() {
        for key in [
            "BRAVE_SEARCH_API_KEY",
            "BRAVE_SEARCH_ENABLED",
            "BRAVE_SEARCH_TIMEOUT_SECS",
            "BRAVE_SEARCH_COUNTRY",
            "BRAVE_SEARCH_LANG",
            "BRAVE_SEARCH_UI_LANG",
            "BRAVE_SEARCH_SAFESEARCH",
            "BRAVE_SEARCH_MAX_CONCURRENT",
            "BRAVE_SEARCH_MIN_DELAY_MS",
        ] {
            test_remove_env(key);
        }
    }

    #[test]
    fn brave_search_enabled_defaults_to_key_presence() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_brave_search_env();

        assert!(!is_brave_search_enabled());

        test_set_env("BRAVE_SEARCH_API_KEY", "brave-key");
        assert!(is_brave_search_enabled());

        clear_brave_search_env();
    }

    #[test]
    fn brave_search_enabled_flag_overrides_key_presence() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_brave_search_env();

        test_set_env("BRAVE_SEARCH_API_KEY", "brave-key");
        test_set_env("BRAVE_SEARCH_ENABLED", "false");
        assert!(!is_brave_search_enabled());

        test_remove_env("BRAVE_SEARCH_API_KEY");
        test_set_env("BRAVE_SEARCH_ENABLED", "true");
        assert!(is_brave_search_enabled());
        assert_eq!(get_brave_search_api_key(), None);

        clear_brave_search_env();
    }

    #[test]
    fn brave_search_config_uses_defaults_when_env_missing() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_brave_search_env();

        assert_eq!(
            get_brave_search_timeout(),
            BRAVE_SEARCH_DEFAULT_TIMEOUT_SECS
        );
        assert_eq!(get_brave_search_country(), BRAVE_SEARCH_DEFAULT_COUNTRY);
        assert_eq!(get_brave_search_lang(), BRAVE_SEARCH_DEFAULT_LANG);
        assert_eq!(get_brave_search_ui_lang(), BRAVE_SEARCH_DEFAULT_UI_LANG);
        assert_eq!(
            get_brave_search_safesearch(),
            BRAVE_SEARCH_DEFAULT_SAFESEARCH
        );
        assert_eq!(
            get_brave_search_max_concurrent(),
            BRAVE_SEARCH_DEFAULT_MAX_CONCURRENT
        );
        assert_eq!(
            get_brave_search_min_delay_ms(),
            BRAVE_SEARCH_DEFAULT_MIN_DELAY_MS
        );
    }

    #[test]
    fn brave_search_config_parses_non_empty_env_values() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_brave_search_env();

        test_set_env("BRAVE_SEARCH_TIMEOUT_SECS", "7");
        test_set_env("BRAVE_SEARCH_COUNTRY", "DE");
        test_set_env("BRAVE_SEARCH_LANG", "de");
        test_set_env("BRAVE_SEARCH_UI_LANG", "de-DE");
        test_set_env("BRAVE_SEARCH_SAFESEARCH", "strict");
        test_set_env("BRAVE_SEARCH_MAX_CONCURRENT", "2");
        test_set_env("BRAVE_SEARCH_MIN_DELAY_MS", "500");

        assert_eq!(get_brave_search_timeout(), 7);
        assert_eq!(get_brave_search_country(), "DE");
        assert_eq!(get_brave_search_lang(), "de");
        assert_eq!(get_brave_search_ui_lang(), "de-DE");
        assert_eq!(get_brave_search_safesearch(), "strict");
        assert_eq!(get_brave_search_max_concurrent(), 2);
        assert_eq!(get_brave_search_min_delay_ms(), 500);

        clear_brave_search_env();
    }

    #[test]
    fn duckduckgo_rate_limit_config_uses_defaults_when_env_missing() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        test_remove_env("DUCKDUCKGO_MAX_CONCURRENT");
        test_remove_env("DUCKDUCKGO_MIN_DELAY_MS");
        test_remove_env("DUCKDUCKGO_JITTER_MS");
        test_remove_env("DUCKDUCKGO_COOLDOWN_SECS");

        let config = get_duckduckgo_rate_limit_config();
        assert_eq!(
            config,
            DuckDuckGoRateLimitConfig {
                max_concurrent: DUCKDUCKGO_DEFAULT_MAX_CONCURRENT,
                min_delay_ms: DUCKDUCKGO_DEFAULT_MIN_DELAY_MS,
                jitter_ms: DUCKDUCKGO_DEFAULT_JITTER_MS,
                cooldown_secs: DUCKDUCKGO_DEFAULT_COOLDOWN_SECS,
            }
        );
    }

    #[test]
    fn searxng_enabled_flag_falls_back_to_url_presence() {
        test_remove_env("SEARXNG_ENABLED");
        test_set_env("SEARXNG_URL", "http://searxng:8080");

        assert!(is_searxng_enabled());

        test_remove_env("SEARXNG_URL");
    }

    #[test]
    fn searxng_bearer_token_uses_only_non_empty_env() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        test_remove_env("SEARXNG_BEARER_TOKEN");

        assert_eq!(get_searxng_bearer_token(), None);

        test_set_env("SEARXNG_BEARER_TOKEN", "  ");
        assert_eq!(get_searxng_bearer_token(), None);

        test_set_env("SEARXNG_BEARER_TOKEN", " test-token ");
        assert_eq!(get_searxng_bearer_token(), Some("test-token".to_string()));

        test_remove_env("SEARXNG_BEARER_TOKEN");
    }

    #[test]
    fn searxng_rotation_engines_use_defaults_when_env_missing() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        test_remove_env("SEARXNG_ROTATION_ENGINES");

        assert_eq!(
            get_searxng_rotation_engines(),
            vec![
                "brave".to_string(),
                "bing".to_string(),
                "qwant".to_string(),
                "mojeek".to_string(),
                "yandex".to_string()
            ]
        );
    }

    #[test]
    fn searxng_rotation_engines_parse_csv() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        test_set_env("SEARXNG_ROTATION_ENGINES", " bing, qwant ,, yandex ");

        assert_eq!(
            get_searxng_rotation_engines(),
            vec![
                "bing".to_string(),
                "qwant".to_string(),
                "yandex".to_string()
            ]
        );

        test_remove_env("SEARXNG_ROTATION_ENGINES");
    }
}

/// Information about a supported LLM model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Internal model identifier
    pub id: String,
    /// Maximum allowed output tokens for a single response.
    pub max_output_tokens: u32,
    /// Maximum model context window available for the full request.
    #[serde(default)]
    pub context_window_tokens: u32,
    /// Provider name
    pub provider: String,
    /// Relative selection weight when used in a fallback route pool.
    #[serde(default = "default_model_route_weight")]
    pub weight: u32,
}

const fn default_model_route_weight() -> u32 {
    1
}

#[derive(Debug, Default)]
struct PartialModelRoute {
    id: Option<String>,
    provider: Option<String>,
    max_output_tokens: Option<u32>,
    context_window_tokens: Option<u32>,
    weight: Option<u32>,
}

impl PartialModelRoute {
    fn into_model_info(self) -> Option<ModelInfo> {
        let id = self.id?.trim().to_string();
        let provider = self.provider?.trim().to_string();
        if id.is_empty() || provider.is_empty() {
            return None;
        }

        Some(ModelInfo {
            id,
            provider,
            max_output_tokens: self.max_output_tokens.unwrap_or_default(),
            context_window_tokens: self.context_window_tokens.unwrap_or_default(),
            weight: self
                .weight
                .unwrap_or_else(default_model_route_weight)
                .max(1),
        })
    }
}

fn resolve_internal_context_budget_tokens(
    model_context_window_tokens: u32,
    default: usize,
) -> usize {
    let resolved_window = usize::try_from(model_context_window_tokens).unwrap_or(default);
    if resolved_window == 0 {
        default
    } else {
        resolved_window
    }
}

/// Get the agent model name from environment.
#[must_use]
pub fn get_agent_model() -> String {
    std::env::var("AGENT_MODEL_ID")
        .ok()
        .or_else(|| std::env::var("AGENT_MODEL_NAME").ok())
        .unwrap_or_default()
}

/// Maximum iterations for agent loop
pub const AGENT_MAX_ITERATIONS: usize = 200;
/// Maximum iterations for sub-agent loop
pub const SUB_AGENT_MAX_ITERATIONS: usize = 2000;
/// Agent task timeout in seconds
pub const AGENT_TIMEOUT_SECS: u64 = 1800; // 30 minutes
/// Sub-agent task timeout in seconds
pub const SUB_AGENT_TIMEOUT_SECS: u64 = 3600;
/// Maximum timeout for individual tool call (in seconds)
/// This prevents a single tool from blocking the agent indefinitely
pub const AGENT_TOOL_TIMEOUT_SECS: u64 = 300; // 5 minutes
/// Default media model max output tokens.
pub const DEFAULT_MEDIA_MODEL_MAX_OUTPUT_TOKENS: u32 = 64_000;
/// Default media model context window tokens.
pub const DEFAULT_MEDIA_MODEL_CONTEXT_WINDOW_TOKENS: u32 = 64_000;
/// Default internal text route max output tokens.
pub const DEFAULT_INTERNAL_TEXT_MAX_OUTPUT_TOKENS: u32 = 64_000;
/// Default internal text route context window tokens.
pub const DEFAULT_INTERNAL_TEXT_CONTEXT_WINDOW_TOKENS: u32 = 64_000;
/// Soft cap for a single agent response. It is not reserved from the input context window.
pub const AGENT_RESPONSE_SOFT_MAX_OUTPUT_TOKENS: u32 = 48_000;
/// Default main-agent model max output tokens.
pub const DEFAULT_AGENT_MODEL_MAX_OUTPUT_TOKENS: u32 = AGENT_RESPONSE_SOFT_MAX_OUTPUT_TOKENS;
/// Default main-agent model context window tokens.
pub const DEFAULT_AGENT_MODEL_CONTEXT_WINDOW_TOKENS: u32 = 240_000;
/// Default internal main-agent context budget when no model window is configured.
pub const DEFAULT_AGENT_INTERNAL_CONTEXT_WINDOW_TOKENS: usize = 240_000;
/// Default sub-agent model max output tokens.
pub const DEFAULT_SUB_AGENT_MODEL_MAX_OUTPUT_TOKENS: u32 = AGENT_RESPONSE_SOFT_MAX_OUTPUT_TOKENS;
/// Max forced continuations when todos incomplete
pub const AGENT_CONTINUATION_LIMIT: usize = 10; // Max forced continuations when todos incomplete
/// Default limit for search tool calls per agent session
pub const AGENT_SEARCH_LIMIT: usize = 10;

/// Maximum tokens for background Wiki Memory writer response.
pub const WIKI_MEMORY_WRITER_MAX_TOKENS: u32 = 4096;
/// Default timeout for background Wiki Memory writer requests.
pub const WIKI_MEMORY_WRITER_TIMEOUT_SECS: u64 = 60;

/// Get agent search limit from env or default.
#[must_use]
pub fn get_agent_search_limit() -> usize {
    std::env::var("AGENT_SEARCH_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(AGENT_SEARCH_LIMIT)
}

/// Get forced continuation limit from env or default.
#[must_use]
pub fn get_agent_continuation_limit() -> usize {
    std::env::var("AGENT_CONTINUATION_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(AGENT_CONTINUATION_LIMIT)
}

/// Get agent max iterations from env or default.
#[must_use]
pub fn get_agent_max_iterations() -> usize {
    std::env::var("AGENT_MAX_ITERATIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(AGENT_MAX_ITERATIONS)
}

/// Get sub-agent max iterations from env or default.
#[must_use]
pub fn get_sub_agent_max_iterations() -> usize {
    std::env::var("SUB_AGENT_MAX_ITERATIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(SUB_AGENT_MAX_ITERATIONS)
}

// Sandbox configuration
/// Docker image for the sandbox
pub const SANDBOX_IMAGE: &str = "agent-sandbox:latest";
/// Sandbox backend mode.
pub const SANDBOX_BACKEND: &str = "docker";
/// Unix socket path for sandbox broker.
pub const SANDBOXD_SOCKET: &str = "/run/sandboxd/sandboxd.sock";
/// Memory limit for sandbox container (1GB)
pub const SANDBOX_MEMORY_LIMIT: i64 = 1024 * 1024 * 1024; // 1GB
/// CPU period for sandbox container
pub const SANDBOX_CPU_PERIOD: i64 = 100_000;
/// CPU quota for sandbox container (2 CPUs)
pub const SANDBOX_CPU_QUOTA: i64 = 200_000; // 2 CPUs (200% of period)
/// Timeout for individual command execution in sandbox
pub const SANDBOX_EXEC_TIMEOUT_SECS: u64 = 240; // 4 minutes per command

/// Explicit sandbox backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxBackendConfig {
    /// Direct Docker backend.
    Docker,
    /// Unix-socket sandboxd broker backend.
    Broker,
    /// Bubblewrap host backend.
    Bwrap,
}

impl SandboxBackendConfig {
    /// Valid environment/config values.
    pub const VALID_VALUES: &'static [&'static str] = &["docker", "broker", "bwrap"];

    /// Returns the stable environment string for this backend.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Broker => "broker",
            Self::Bwrap => "bwrap",
        }
    }
}

impl fmt::Display for SandboxBackendConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for SandboxBackendConfig {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "docker" => Ok(Self::Docker),
            "broker" => Ok(Self::Broker),
            "bwrap" => Ok(Self::Bwrap),
            invalid => Err(format!(
                "Invalid SANDBOX_BACKEND='{invalid}'. Valid values: {}.",
                Self::VALID_VALUES.join(", ")
            )),
        }
    }
}

/// Get sandbox image from env or default.
///
/// Environment variable: `SANDBOX_IMAGE`
#[must_use]
pub fn get_sandbox_image() -> String {
    std::env::var("SANDBOX_IMAGE")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| SANDBOX_IMAGE.to_string())
}

/// Get sandbox backend mode from env or default.
#[must_use]
pub fn get_sandbox_backend() -> String {
    std::env::var("SANDBOX_BACKEND")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| SANDBOX_BACKEND.to_string())
}

/// Parse sandbox backend mode from env or default.
///
/// # Errors
///
/// Returns an actionable error when `SANDBOX_BACKEND` is not one of the
/// supported backend names.
pub fn get_sandbox_backend_config() -> Result<SandboxBackendConfig, String> {
    get_sandbox_backend().parse()
}

/// Check whether sandbox broker mode is enabled.
#[must_use]
pub fn sandbox_uses_broker() -> bool {
    get_sandbox_backend_config() == Ok(SandboxBackendConfig::Broker)
}

/// Get sandbox broker Unix socket path from env or default.
#[must_use]
pub fn get_sandboxd_socket() -> String {
    std::env::var("SANDBOXD_SOCKET")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| SANDBOXD_SOCKET.to_string())
}

/// Get compose project override for stack log discovery.
///
/// Environment variable: `STACK_LOGS_PROJECT`
#[must_use]
pub fn get_stack_logs_project() -> Option<String> {
    std::env::var("STACK_LOGS_PROJECT")
        .ok()
        .filter(|value| !value.is_empty())
}

/// Transport API retry configuration for file operations.
pub const TRANSPORT_API_MAX_RETRIES: usize = 3;
/// Initial backoff delay in milliseconds for transport retries.
pub const TRANSPORT_API_INITIAL_BACKOFF_MS: u64 = 500;
/// Maximum backoff delay in milliseconds for transport retries.
pub const TRANSPORT_API_MAX_BACKOFF_MS: u64 = 4000;

// Public search provider HTTP client configuration
/// Default timeout for Brave Search requests (seconds).
pub const BRAVE_SEARCH_DEFAULT_TIMEOUT_SECS: u64 = 10;
/// Default Brave Search country targeting.
pub const BRAVE_SEARCH_DEFAULT_COUNTRY: &str = "US";
/// Default Brave Search language targeting.
pub const BRAVE_SEARCH_DEFAULT_LANG: &str = "en";
/// Default Brave Search UI language.
pub const BRAVE_SEARCH_DEFAULT_UI_LANG: &str = "en-US";
/// Default Brave Search safe-search setting.
pub const BRAVE_SEARCH_DEFAULT_SAFESEARCH: &str = "moderate";
/// Default process-wide Brave Search max concurrent operations.
pub const BRAVE_SEARCH_DEFAULT_MAX_CONCURRENT: usize = 1;
/// Default process-wide Brave Search minimum delay between operations.
pub const BRAVE_SEARCH_DEFAULT_MIN_DELAY_MS: u64 = 1_000;

/// Default timeout for DuckDuckGo requests (seconds).
pub const DUCKDUCKGO_DEFAULT_TIMEOUT_SECS: u64 = 30;
/// Default DuckDuckGo region.
pub const DUCKDUCKGO_DEFAULT_REGION: &str = "wt-wt";
/// Default DuckDuckGo news safe-search setting.
pub const DUCKDUCKGO_DEFAULT_SAFE_SEARCH: bool = true;
/// Default process-wide DuckDuckGo max concurrent operations.
pub const DUCKDUCKGO_DEFAULT_MAX_CONCURRENT: usize = 1;
/// Default process-wide DuckDuckGo minimum delay between operations.
pub const DUCKDUCKGO_DEFAULT_MIN_DELAY_MS: u64 = 2_500;
/// Default process-wide DuckDuckGo delay jitter.
pub const DUCKDUCKGO_DEFAULT_JITTER_MS: u64 = 1_500;
/// Default DuckDuckGo retry count.
pub const DUCKDUCKGO_DEFAULT_MAX_RETRIES: u8 = 2;
/// Default DuckDuckGo initial retry backoff.
pub const DUCKDUCKGO_DEFAULT_INITIAL_BACKOFF_MS: u64 = 1_500;
/// Default DuckDuckGo maximum retry backoff.
pub const DUCKDUCKGO_DEFAULT_MAX_BACKOFF_MS: u64 = 30_000;
/// Default DuckDuckGo cooldown after blocks or transient failures.
pub const DUCKDUCKGO_DEFAULT_COOLDOWN_SECS: u64 = 90;

// Self-hosted SearXNG HTTP client configuration
/// Default timeout for SearXNG requests (seconds)
pub const SEARXNG_DEFAULT_TIMEOUT_SECS: u64 = 30;
/// Default engines used for SearXNG rotation fallback.
pub const SEARXNG_DEFAULT_ROTATION_ENGINES: &[&str] =
    &["brave", "bing", "qwant", "mojeek", "yandex"];

/// DuckDuckGo browser configuration.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct DuckDuckGoBrowserConfig {
    /// User-agent alias or literal value.
    pub user_agent: Option<String>,
    /// Optional proxy URL.
    pub proxy_url: Option<String>,
}

/// DuckDuckGo rate-limit configuration.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct DuckDuckGoRateLimitConfig {
    /// Max concurrent high-level DuckDuckGo operations.
    pub max_concurrent: usize,
    /// Minimum delay between high-level operations.
    pub min_delay_ms: u64,
    /// Random delay jitter.
    pub jitter_ms: u64,
    /// Cooldown after likely rate limits or blocks.
    pub cooldown_secs: u64,
}

/// DuckDuckGo retry backoff configuration.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct DuckDuckGoBackoffConfig {
    /// Maximum retry attempts after the first request.
    pub max_retries: u8,
    /// Initial retry backoff in milliseconds.
    pub initial_backoff_ms: u64,
    /// Maximum retry backoff in milliseconds.
    pub max_backoff_ms: u64,
}

/// Get DuckDuckGo timeout from env or default.
///
/// Environment variable: `DUCKDUCKGO_TIMEOUT_SECS`
#[must_use]
pub fn get_duckduckgo_timeout() -> u64 {
    parse_env_u64("DUCKDUCKGO_TIMEOUT_SECS").unwrap_or(DUCKDUCKGO_DEFAULT_TIMEOUT_SECS)
}

/// Get Brave Search API key from env.
///
/// Environment variable: `BRAVE_SEARCH_API_KEY`
#[must_use]
pub fn get_brave_search_api_key() -> Option<String> {
    std::env::var("BRAVE_SEARCH_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Get Brave Search timeout from env or default.
///
/// Environment variable: `BRAVE_SEARCH_TIMEOUT_SECS`
#[must_use]
pub fn get_brave_search_timeout() -> u64 {
    parse_env_u64("BRAVE_SEARCH_TIMEOUT_SECS").unwrap_or(BRAVE_SEARCH_DEFAULT_TIMEOUT_SECS)
}

/// Get Brave Search default country from env or default.
///
/// Environment variable: `BRAVE_SEARCH_COUNTRY`
#[must_use]
pub fn get_brave_search_country() -> String {
    std::env::var("BRAVE_SEARCH_COUNTRY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| BRAVE_SEARCH_DEFAULT_COUNTRY.to_string())
}

/// Get Brave Search default search language from env or default.
///
/// Environment variable: `BRAVE_SEARCH_LANG`
#[must_use]
pub fn get_brave_search_lang() -> String {
    std::env::var("BRAVE_SEARCH_LANG")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| BRAVE_SEARCH_DEFAULT_LANG.to_string())
}

/// Get Brave Search default UI language from env or default.
///
/// Environment variable: `BRAVE_SEARCH_UI_LANG`
#[must_use]
pub fn get_brave_search_ui_lang() -> String {
    std::env::var("BRAVE_SEARCH_UI_LANG")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| BRAVE_SEARCH_DEFAULT_UI_LANG.to_string())
}

/// Get Brave Search safe-search setting from env or default.
///
/// Environment variable: `BRAVE_SEARCH_SAFESEARCH`
#[must_use]
pub fn get_brave_search_safesearch() -> String {
    std::env::var("BRAVE_SEARCH_SAFESEARCH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| BRAVE_SEARCH_DEFAULT_SAFESEARCH.to_string())
}

/// Get Brave Search process-wide max concurrent operations from env or default.
///
/// Environment variable: `BRAVE_SEARCH_MAX_CONCURRENT`
#[must_use]
pub fn get_brave_search_max_concurrent() -> usize {
    parse_env_usize("BRAVE_SEARCH_MAX_CONCURRENT")
        .filter(|value| *value > 0)
        .unwrap_or(BRAVE_SEARCH_DEFAULT_MAX_CONCURRENT)
}

/// Get Brave Search process-wide minimum delay from env or default.
///
/// Environment variable: `BRAVE_SEARCH_MIN_DELAY_MS`
#[must_use]
pub fn get_brave_search_min_delay_ms() -> u64 {
    parse_env_u64("BRAVE_SEARCH_MIN_DELAY_MS").unwrap_or(BRAVE_SEARCH_DEFAULT_MIN_DELAY_MS)
}

/// Get DuckDuckGo default region from env or default.
///
/// Environment variable: `DUCKDUCKGO_REGION`
#[must_use]
pub fn get_duckduckgo_region() -> String {
    std::env::var("DUCKDUCKGO_REGION")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DUCKDUCKGO_DEFAULT_REGION.to_string())
}

/// Get DuckDuckGo news safe-search setting from env or default.
///
/// Environment variable: `DUCKDUCKGO_SAFE_SEARCH`
#[must_use]
pub fn get_duckduckgo_safe_search() -> bool {
    parse_optional_env_bool("DUCKDUCKGO_SAFE_SEARCH").unwrap_or(DUCKDUCKGO_DEFAULT_SAFE_SEARCH)
}

/// Get DuckDuckGo browser config from env.
///
/// Environment variables: `DUCKDUCKGO_USER_AGENT`, `DUCKDUCKGO_PROXY_URL`, `DUCKDUCKGO_PROXY`.
#[must_use]
pub fn get_duckduckgo_browser_config() -> DuckDuckGoBrowserConfig {
    DuckDuckGoBrowserConfig {
        user_agent: std::env::var("DUCKDUCKGO_USER_AGENT")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        proxy_url: std::env::var("DUCKDUCKGO_PROXY_URL")
            .or_else(|_| std::env::var("DUCKDUCKGO_PROXY"))
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    }
}

/// Get DuckDuckGo process-wide rate-limit config from env or defaults.
#[must_use]
pub fn get_duckduckgo_rate_limit_config() -> DuckDuckGoRateLimitConfig {
    DuckDuckGoRateLimitConfig {
        max_concurrent: parse_env_usize("DUCKDUCKGO_MAX_CONCURRENT")
            .filter(|value| *value > 0)
            .unwrap_or(DUCKDUCKGO_DEFAULT_MAX_CONCURRENT),
        min_delay_ms: parse_env_u64("DUCKDUCKGO_MIN_DELAY_MS")
            .unwrap_or(DUCKDUCKGO_DEFAULT_MIN_DELAY_MS),
        jitter_ms: parse_env_u64("DUCKDUCKGO_JITTER_MS").unwrap_or(DUCKDUCKGO_DEFAULT_JITTER_MS),
        cooldown_secs: parse_env_u64("DUCKDUCKGO_COOLDOWN_SECS")
            .unwrap_or(DUCKDUCKGO_DEFAULT_COOLDOWN_SECS),
    }
}

/// Get DuckDuckGo retry backoff config from env or defaults.
#[must_use]
pub fn get_duckduckgo_backoff_config() -> DuckDuckGoBackoffConfig {
    DuckDuckGoBackoffConfig {
        max_retries: parse_env_u8("DUCKDUCKGO_MAX_RETRIES")
            .unwrap_or(DUCKDUCKGO_DEFAULT_MAX_RETRIES),
        initial_backoff_ms: parse_env_u64("DUCKDUCKGO_INITIAL_BACKOFF_MS")
            .unwrap_or(DUCKDUCKGO_DEFAULT_INITIAL_BACKOFF_MS),
        max_backoff_ms: parse_env_u64("DUCKDUCKGO_MAX_BACKOFF_MS")
            .unwrap_or(DUCKDUCKGO_DEFAULT_MAX_BACKOFF_MS),
    }
}

/// Get SearXNG base URL from env.
///
/// Environment variable: `SEARXNG_URL`
#[must_use]
pub fn get_searxng_url() -> Option<String> {
    std::env::var("SEARXNG_URL").ok().filter(|s| !s.is_empty())
}

/// Get optional SearXNG Bearer token from env.
///
/// Environment variable: `SEARXNG_BEARER_TOKEN`
#[must_use]
pub fn get_searxng_bearer_token() -> Option<String> {
    std::env::var("SEARXNG_BEARER_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Determine whether SearXNG tools should be registered.
///
/// Enabled when `SEARXNG_ENABLED` is truthy **or** when `SEARXNG_URL` is set.
#[must_use]
pub fn is_searxng_enabled() -> bool {
    if let Some(enabled) = parse_optional_env_bool("SEARXNG_ENABLED") {
        return enabled;
    }
    get_searxng_url().is_some()
}

/// Determine whether Crawl4AI markdown tools should be registered.
///
/// `OXIDE_CRAWL4AI_ENABLED=false` forces disable. Without an explicit flag,
/// registration is enabled only when `OXIDE_CRAWL4AI_BASE_URL` is non-empty —
/// the operator's signal that a Crawl4AI service is reachable.
#[must_use]
pub fn is_crawl4ai_markdown_enabled() -> bool {
    if let Some(enabled) = parse_optional_env_bool("OXIDE_CRAWL4AI_ENABLED") {
        return enabled;
    }
    std::env::var("OXIDE_CRAWL4AI_BASE_URL")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

/// Determine whether split URL-to-Markdown tools should be merged into `web_crawler`.
///
/// Environment variable: `OXIDE_WEB_CRAWLER_MERGE`.
#[must_use]
pub fn is_web_crawler_merge_enabled() -> bool {
    parse_optional_env_bool("OXIDE_WEB_CRAWLER_MERGE").unwrap_or(false)
}

/// Get SearXNG timeout from env or default.
///
/// Environment variable: `SEARXNG_TIMEOUT_SECS`
#[must_use]
pub fn get_searxng_timeout() -> u64 {
    std::env::var("SEARXNG_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(SEARXNG_DEFAULT_TIMEOUT_SECS)
}

/// Get preferred engines for SearXNG rotation from env or defaults.
///
/// Environment variable: `SEARXNG_ROTATION_ENGINES`
/// Value format: comma-separated engine names, for example "bing,qwant,yandex".
#[must_use]
pub fn get_searxng_rotation_engines() -> Vec<String> {
    let parsed = std::env::var("SEARXNG_ROTATION_ENGINES")
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if parsed.is_empty() {
        SEARXNG_DEFAULT_ROTATION_ENGINES
            .iter()
            .map(|value| (*value).to_string())
            .collect()
    } else {
        parsed
    }
}

/// Get max concurrent OpenCode Go requests from env or default.
///
/// Environment variable: `OPENCODE_GO_MAX_CONCURRENT`
#[must_use]
pub fn get_opencode_go_max_concurrent() -> usize {
    std::env::var("OPENCODE_GO_MAX_CONCURRENT")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(OPENCODE_GO_DEFAULT_MAX_CONCURRENT)
}

fn parse_optional_env_bool(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
}

fn parse_env_u64(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
}

fn parse_env_u8(name: &str) -> Option<u8> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
}

fn parse_env_usize(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
}

fn parse_optional_env_f32(name: &str) -> Option<f32> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<f32>().ok())
}

/// Determine whether Tavily tools should be registered.
///
/// Environment variable: `TAVILY_ENABLED`
#[must_use]
pub fn is_tavily_enabled() -> bool {
    parse_optional_env_bool("TAVILY_ENABLED").unwrap_or_else(|| {
        std::env::var("TAVILY_API_KEY")
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
    })
}

/// Determine whether Brave Search tools should be registered.
///
/// `BRAVE_SEARCH_ENABLED=false` disables registration. Without an explicit flag,
/// registration is enabled only when `BRAVE_SEARCH_API_KEY` is non-empty.
#[must_use]
pub fn is_brave_search_enabled() -> bool {
    parse_optional_env_bool("BRAVE_SEARCH_ENABLED")
        .unwrap_or_else(|| get_brave_search_api_key().is_some())
}

/// Determine whether DuckDuckGo tools should be registered.
///
/// Environment variable: `DUCKDUCKGO_ENABLED`
#[must_use]
pub fn is_duckduckgo_enabled() -> bool {
    parse_optional_env_bool("DUCKDUCKGO_ENABLED").unwrap_or(true)
}

// LLM HTTP client configuration
/// Default timeout for LLM API HTTP requests (seconds).
/// Generous default for large prompts and slow models; override with env LLM_HTTP_TIMEOUT_SECS.
pub const LLM_HTTP_TIMEOUT_SECS: u64 = 90;

/// Get LLM HTTP timeout from env or default
///
/// Environment variable: `LLM_HTTP_TIMEOUT_SECS`
#[must_use]
pub fn get_llm_http_timeout_secs() -> u64 {
    std::env::var("LLM_HTTP_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(LLM_HTTP_TIMEOUT_SECS)
}
