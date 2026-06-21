//! Feature-gated LLM provider modules and factories.

use std::collections::HashMap;
use std::sync::Arc;

#[cfg(any(
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_opencode_go
))]
use super::super::capabilities::ToolHistoryMode;
use super::super::capabilities::{MediaCapabilities, ProviderCapabilities};
use crate::config::{AgentSettings, ModelInfo};
use crate::llm::LlmProvider;

#[cfg(any(
    oxide_module_llm_provider_openai_chatgpt,
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
use crate::llm::support;

#[cfg(any(
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
use super::chat_completions::{client::ChatCompletionsClient, profile::ChatCompletionsProfile};
#[cfg(any(
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_opencode_go
))]
use super::messages::{MessagesClient, MessagesProfile};

/// Context shared by provider module factories.
pub(crate) struct LlmProviderBuildContext {
    #[cfg(any(
        oxide_module_llm_provider_openai_chatgpt,
        oxide_module_llm_provider_anthropic,
        oxide_module_llm_provider_mistral,
        oxide_module_llm_provider_openai_base,
        oxide_module_llm_provider_opencode_go,
        oxide_module_llm_provider_openrouter
    ))]
    pub(crate) http_client: reqwest::Client,
}

impl LlmProviderBuildContext {
    fn new() -> Self {
        Self {
            #[cfg(any(
                oxide_module_llm_provider_openai_chatgpt,
                oxide_module_llm_provider_anthropic,
                oxide_module_llm_provider_mistral,
                oxide_module_llm_provider_openai_base,
                oxide_module_llm_provider_opencode_go,
                oxide_module_llm_provider_openrouter
            ))]
            http_client: support::http::create_http_client(),
        }
    }
}

/// Internal generic compatible-provider kind. This is intentionally not wired
/// to user config yet; legacy modules remain the stable public surface.
#[cfg(any(
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GenericProviderKind {
    ChatCompletions,
    Messages,
}

#[cfg(any(
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
#[allow(dead_code)]
impl GenericProviderKind {
    pub(crate) fn from_config_value(value: &str) -> Result<Self, String> {
        match value.trim().replace('-', "_").to_ascii_lowercase().as_str() {
            "chat_completions" => Ok(Self::ChatCompletions),
            "messages" => Ok(Self::Messages),
            "chatgpt" | "openai_chatgpt" => {
                Err("ChatGPT/Codex is not a generic compatible provider kind".to_string())
            }
            other => Err(format!("unsupported generic provider kind: {other}")),
        }
    }
}

/// Internal config shape for future compatible endpoint providers.
///
/// This documents the intended fields (`kind`, `endpoint_url`, `api_key`, and
/// optional `profile`) without introducing an untested public settings stanza.
#[cfg(any(
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GenericEndpointProviderConfig {
    pub(crate) name: String,
    pub(crate) kind: GenericProviderKind,
    pub(crate) endpoint_url: String,
    pub(crate) api_key: Option<String>,
    pub(crate) profile: Option<String>,
}

#[cfg(any(
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
#[allow(dead_code)]
impl GenericEndpointProviderConfig {
    pub(crate) fn from_fields(
        name: impl Into<String>,
        kind: &str,
        endpoint_url: impl Into<String>,
        api_key: Option<String>,
        profile: Option<String>,
    ) -> Result<Self, String> {
        Ok(Self {
            name: name.into(),
            kind: GenericProviderKind::from_config_value(kind)?,
            endpoint_url: endpoint_url.into(),
            api_key,
            profile,
        })
    }
}

#[cfg(any(
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) enum GenericEndpointClient {
    #[cfg(any(
        oxide_module_llm_provider_mistral,
        oxide_module_llm_provider_openai_base,
        oxide_module_llm_provider_opencode_go,
        oxide_module_llm_provider_openrouter
    ))]
    ChatCompletions(ChatCompletionsClient),
    #[cfg(any(
        oxide_module_llm_provider_anthropic,
        oxide_module_llm_provider_opencode_go
    ))]
    Messages(MessagesClient),
}

#[cfg(any(
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct GenericEndpointProvider {
    pub(crate) name: String,
    pub(crate) client: GenericEndpointClient,
    pub(crate) capabilities: ProviderCapabilities,
    pub(crate) media_capabilities: MediaCapabilities,
}

#[cfg(any(
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
#[allow(dead_code)]
pub(crate) fn build_generic_endpoint_provider(
    config: &GenericEndpointProviderConfig,
    ctx: &LlmProviderBuildContext,
) -> Result<GenericEndpointProvider, String> {
    match config.kind {
        GenericProviderKind::ChatCompletions => {
            build_generic_chat_completions_provider(config, ctx)
        }
        GenericProviderKind::Messages => build_generic_messages_provider(config, ctx),
    }
}

#[cfg(any(
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
#[allow(dead_code)]
fn build_generic_chat_completions_provider(
    config: &GenericEndpointProviderConfig,
    ctx: &LlmProviderBuildContext,
) -> Result<GenericEndpointProvider, String> {
    let profile = resolve_generic_chat_completions_profile(config.profile.as_deref())?;
    let endpoint = profile.endpoint_for(&config.endpoint_url);
    let client = ChatCompletionsClient::new(
        ctx.http_client.clone(),
        endpoint,
        config.api_key.clone(),
        "",
        profile,
    );

    Ok(GenericEndpointProvider {
        name: config.name.clone(),
        client: GenericEndpointClient::ChatCompletions(client),
        capabilities: profile.capabilities,
        media_capabilities: profile.media_capabilities,
    })
}

#[cfg(all(
    any(
        oxide_module_llm_provider_anthropic,
        oxide_module_llm_provider_mistral,
        oxide_module_llm_provider_openai_base,
        oxide_module_llm_provider_opencode_go,
        oxide_module_llm_provider_openrouter
    ),
    not(any(
        oxide_module_llm_provider_mistral,
        oxide_module_llm_provider_openai_base,
        oxide_module_llm_provider_opencode_go,
        oxide_module_llm_provider_openrouter
    ))
))]
#[allow(dead_code)]
fn build_generic_chat_completions_provider(
    _config: &GenericEndpointProviderConfig,
    _ctx: &LlmProviderBuildContext,
) -> Result<GenericEndpointProvider, String> {
    Err("generic chat_completions providers are not compiled in this build".to_string())
}

#[cfg(any(
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_opencode_go
))]
#[allow(dead_code)]
fn build_generic_messages_provider(
    config: &GenericEndpointProviderConfig,
    ctx: &LlmProviderBuildContext,
) -> Result<GenericEndpointProvider, String> {
    let profile = resolve_generic_messages_profile(config.profile.as_deref())?;
    let endpoint = profile.endpoint_for(&config.endpoint_url);
    let client = MessagesClient::new(
        ctx.http_client.clone(),
        endpoint,
        config.api_key.clone().unwrap_or_default(),
        profile,
    );

    Ok(GenericEndpointProvider {
        name: config.name.clone(),
        client: GenericEndpointClient::Messages(client),
        capabilities: ProviderCapabilities::new(ToolHistoryMode::Strict, true, false),
        media_capabilities: MediaCapabilities::new(false, false, false),
    })
}

#[cfg(all(
    any(
        oxide_module_llm_provider_anthropic,
        oxide_module_llm_provider_mistral,
        oxide_module_llm_provider_openai_base,
        oxide_module_llm_provider_opencode_go,
        oxide_module_llm_provider_openrouter
    ),
    not(any(
        oxide_module_llm_provider_anthropic,
        oxide_module_llm_provider_opencode_go
    ))
))]
#[allow(dead_code)]
fn build_generic_messages_provider(
    _config: &GenericEndpointProviderConfig,
    _ctx: &LlmProviderBuildContext,
) -> Result<GenericEndpointProvider, String> {
    Err("generic messages providers are not compiled in this build".to_string())
}

#[cfg(any(
    oxide_module_llm_provider_mistral,
    oxide_module_llm_provider_openai_base,
    oxide_module_llm_provider_opencode_go,
    oxide_module_llm_provider_openrouter
))]
#[allow(dead_code)]
fn resolve_generic_chat_completions_profile(
    profile: Option<&str>,
) -> Result<ChatCompletionsProfile, String> {
    match profile
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.replace('-', "_").to_ascii_lowercase())
        .as_deref()
    {
        None | Some("generic") => Ok(ChatCompletionsProfile::generic()),
        Some("mistral") => Ok(ChatCompletionsProfile::mistral()),
        Some("zai") => Ok(ChatCompletionsProfile::zai()),
        Some("openrouter") => Ok(ChatCompletionsProfile::openrouter()),
        Some("opencode_go") => Ok(ChatCompletionsProfile::opencode_go()),
        Some("opencode_zen") => Ok(ChatCompletionsProfile::opencode_zen()),
        Some("chatgpt" | "openai_chatgpt") => {
            Err("ChatGPT/Codex is not a Chat Completions profile".to_string())
        }
        Some(other) => Err(format!("unsupported chat_completions profile: {other}")),
    }
}

#[cfg(any(
    oxide_module_llm_provider_anthropic,
    oxide_module_llm_provider_opencode_go
))]
#[allow(dead_code)]
fn resolve_generic_messages_profile(profile: Option<&str>) -> Result<MessagesProfile, String> {
    match profile
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.replace('-', "_").to_ascii_lowercase())
        .as_deref()
    {
        None | Some("anthropic") | Some("messages") => Ok(MessagesProfile::anthropic()),
        Some("opencode_go") => Ok(MessagesProfile::opencode_go()),
        Some("chatgpt" | "openai_chatgpt") => {
            Err("ChatGPT/Codex is not a Messages profile".to_string())
        }
        Some(other) => Err(format!("unsupported messages profile: {other}")),
    }
}

/// Provider module descriptor and factory.
pub(crate) trait LlmProviderModule: Send + Sync {
    /// Stable provider module ID from the compiled capability manifest.
    fn provider_id(&self) -> &'static str;

    /// Runtime provider aliases owned by the module.
    fn aliases(&self) -> &'static [&'static str];

    /// Builds a configured provider instance when required credentials are present.
    fn build_provider(
        &self,
        settings: &AgentSettings,
        ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>>;

    /// Builds named runtime provider instances. Most modules expose one instance
    /// under their provider ID and aliases; OpenAI Base exposes named instances.
    fn build_providers(
        &self,
        settings: &AgentSettings,
        ctx: &LlmProviderBuildContext,
    ) -> Vec<(String, Arc<dyn LlmProvider>)> {
        let Some(provider) = self.build_provider(settings, ctx) else {
            return Vec::new();
        };
        let mut providers = Vec::with_capacity(self.aliases().len() + 1);
        providers.push((self.provider_id().to_string(), Arc::clone(&provider)));
        for alias in self.aliases() {
            providers.push(((*alias).to_string(), Arc::clone(&provider)));
        }
        providers
    }

    /// Returns a startup config error when this provider is routed but incomplete.
    fn missing_route_config_message(
        &self,
        _provider_name: &str,
        _settings: &AgentSettings,
    ) -> Option<String> {
        None
    }

    /// Base request capabilities for this provider.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Media modality support for this provider.
    fn media_capabilities(&self) -> MediaCapabilities {
        MediaCapabilities::new(false, false, false)
    }

    /// Media modality support for a concrete model route.
    fn media_capabilities_for_model(&self, _model_info: &ModelInfo) -> MediaCapabilities {
        self.media_capabilities()
    }

    /// Request capabilities for a concrete model route.
    fn capabilities_for_model(&self, _model_info: &ModelInfo) -> ProviderCapabilities {
        self.capabilities()
    }
}

/// Normalizes provider lookup keys.
#[must_use]
pub(crate) fn provider_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Builds all configured providers from compiled and enabled provider modules.
#[must_use]
pub(crate) fn build_configured_providers(
    settings: &AgentSettings,
) -> HashMap<String, Arc<dyn LlmProvider>> {
    let ctx = LlmProviderBuildContext::new();
    let mut providers = HashMap::new();

    for module in compiled_provider_modules() {
        if !settings.is_module_enabled(module.provider_id()) {
            continue;
        }

        for (name, provider) in module.build_providers(settings, &ctx) {
            insert_provider(&mut providers, &name, provider);
        }
    }

    providers
}

/// Returns request capabilities for a compiled provider module.
#[must_use]
pub(crate) fn provider_capabilities(provider_name: &str) -> Option<ProviderCapabilities> {
    find_provider_module(provider_name).map(|module| module.capabilities())
}

/// Resolves a provider alias or module ID to the compiled provider module ID.
#[must_use]
pub(crate) fn provider_module_id(provider_name: &str) -> Option<&'static str> {
    #[cfg(oxide_module_llm_provider_openai_base)]
    if super::openai_base::module::provider_instance_name(provider_name).is_some()
        || super::openai_base::module::is_legacy_provider_name(provider_name)
    {
        return Some("llm-provider/openai-base");
    }

    find_provider_module(provider_name).map(|module| module.provider_id())
}

/// Returns the canonical runtime provider key for route configuration.
#[must_use]
pub(crate) fn canonical_route_provider(provider_name: &str) -> Option<String> {
    #[cfg(oxide_module_llm_provider_openai_base)]
    if let Some(instance) = super::openai_base::module::provider_instance_name(provider_name) {
        return Some(format!("openai-base:{instance}"));
    }

    #[cfg(oxide_module_llm_provider_openai_base)]
    if super::openai_base::module::is_legacy_provider_name(provider_name) {
        return None;
    }

    provider_module_id(provider_name).map(ToString::to_string)
}

/// Returns the provider-owned startup config error for a routed provider.
#[must_use]
pub(crate) fn provider_missing_route_config_message(
    provider_name: &str,
    settings: &AgentSettings,
) -> Option<String> {
    find_provider_module(provider_name)
        .and_then(|module| module.missing_route_config_message(provider_name, settings))
}

/// Returns media capabilities for a compiled provider module.
#[must_use]
#[allow(dead_code)]
pub(crate) fn provider_media_capabilities(provider_name: &str) -> Option<MediaCapabilities> {
    find_provider_module(provider_name).map(|module| module.media_capabilities())
}

/// Returns media capabilities for a concrete route handled by a compiled provider module.
#[must_use]
pub(crate) fn provider_media_capabilities_for_model(
    model_info: &ModelInfo,
) -> Option<MediaCapabilities> {
    find_provider_module(&model_info.provider)
        .map(|module| module.media_capabilities_for_model(model_info))
}

/// Returns request capabilities for a concrete route handled by a compiled provider module.
#[must_use]
pub(crate) fn provider_capabilities_for_model(
    model_info: &ModelInfo,
) -> Option<ProviderCapabilities> {
    find_provider_module(&model_info.provider)
        .map(|module| module.capabilities_for_model(model_info))
}

fn find_provider_module(provider_name: &str) -> Option<Box<dyn LlmProviderModule>> {
    #[cfg(oxide_module_llm_provider_openai_base)]
    if super::openai_base::module::provider_instance_name(provider_name).is_some()
        || super::openai_base::module::is_legacy_provider_name(provider_name)
    {
        return Some(Box::new(super::openai_base::OpenAIBaseProviderModule));
    }

    let provider_key = provider_key(provider_name);
    compiled_provider_modules()
        .into_iter()
        .find(|module| module_matches_key(module.as_ref(), &provider_key))
}

fn module_matches_key(module: &dyn LlmProviderModule, expected_key: &str) -> bool {
    provider_key(module.provider_id()) == expected_key
        || module
            .aliases()
            .iter()
            .any(|alias| provider_key(alias) == expected_key)
}

fn insert_provider(
    providers: &mut HashMap<String, Arc<dyn LlmProvider>>,
    name: &str,
    provider: Arc<dyn LlmProvider>,
) {
    providers.insert(provider_key(name), provider);
}

fn compiled_provider_modules() -> Vec<Box<dyn LlmProviderModule>> {
    let mut modules: Vec<Box<dyn LlmProviderModule>> = Vec::new();
    let _ = &mut modules;

    #[cfg(oxide_module_llm_provider_openai_chatgpt)]
    modules.push(Box::new(super::chatgpt::ChatGptProviderModule));
    #[cfg(oxide_module_llm_provider_mistral)]
    modules.push(Box::new(super::openai_base::MistralProviderModule));
    #[cfg(oxide_module_llm_provider_anthropic)]
    modules.push(Box::new(super::anthropic::AnthropicProviderModule));
    #[cfg(oxide_module_llm_provider_openai_base)]
    modules.push(Box::new(super::openai_base::OpenAIBaseProviderModule));
    #[cfg(oxide_module_llm_provider_opencode_go)]
    modules.push(Box::new(super::opencode_go::OpenCodeGoProviderModule));
    #[cfg(oxide_module_llm_provider_opencode_go)]
    modules.push(Box::new(super::opencode_go::OpenCodeZenProviderModule));
    #[cfg(oxide_module_llm_provider_openrouter)]
    modules.push(Box::new(super::openrouter::OpenRouterProviderModule));

    modules
}

#[cfg(test)]
#[cfg_attr(
    not(any(
        oxide_module_llm_provider_openai_chatgpt,
        oxide_module_llm_provider_mistral,
        oxide_module_llm_provider_anthropic,
        oxide_module_llm_provider_opencode_go,
        oxide_module_llm_provider_openrouter
    )),
    allow(dead_code, unused_imports)
)]
mod tests {
    #[cfg(oxide_module_llm_provider_openai_base)]
    use super::canonical_route_provider;
    use super::{
        build_configured_providers, provider_capabilities, provider_capabilities_for_model,
        provider_key, provider_missing_route_config_message, provider_module_id,
    };
    use crate::config::{AgentSettings, ModuleRuntimeConfig, test_env_mutex};
    use crate::testing::{test_remove_env, test_set_env};

    fn settings_with_provider_key(module_id: &str, api_key: &str) -> AgentSettings {
        let mut settings = AgentSettings::default();
        settings.modules.insert(
            module_id.to_string(),
            ModuleRuntimeConfig::default().with_string_value("api_key", api_key),
        );
        settings
    }

    #[test]
    fn provider_key_is_case_insensitive() {
        assert_eq!(provider_key("OpenCode-Go"), "opencode-go");
    }

    #[cfg(oxide_module_llm_provider_openai_base)]
    #[test]
    fn openai_base_registers_named_env_provider_instances_only() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        test_remove_env("OPENAI_BASE_API_BASE");
        test_remove_env("OPENAI_BASE_API_KEY");
        test_set_env("OPENAI_BASE_PROVIDERS__0__NAME", "local");
        test_set_env(
            "OPENAI_BASE_PROVIDERS__0__API_BASE",
            "http://127.0.0.1:8080/v1",
        );
        test_set_env("OPENAI_BASE_PROVIDERS__1__NAME", "groq");
        test_set_env(
            "OPENAI_BASE_PROVIDERS__1__API_BASE",
            "https://api.groq.com/openai/v1",
        );

        let providers = build_configured_providers(&AgentSettings::default());

        assert!(providers.contains_key("openai-base:local"));
        assert!(providers.contains_key("openai-base:groq"));
        assert!(!providers.contains_key("openai-base"));
        assert_eq!(
            canonical_route_provider("llm-provider/openai-base:Groq"),
            Some("openai-base:groq".to_string())
        );
        assert_eq!(canonical_route_provider("openai-base"), None);

        test_remove_env("OPENAI_BASE_PROVIDERS__0__NAME");
        test_remove_env("OPENAI_BASE_PROVIDERS__0__API_BASE");
        test_remove_env("OPENAI_BASE_PROVIDERS__1__NAME");
        test_remove_env("OPENAI_BASE_PROVIDERS__1__API_BASE");
    }

    #[cfg(oxide_module_llm_provider_openai_base)]
    #[test]
    fn openai_base_legacy_env_returns_migration_error() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        test_set_env("OPENAI_BASE_API_BASE", "https://api.openai.com/v1");

        assert_eq!(
            provider_missing_route_config_message("openai-base:openai", &AgentSettings::default()),
            Some(
                "Critical: OPENAI_BASE_API_BASE is deprecated. Use OPENAI_BASE_PROVIDERS__N__NAME and OPENAI_BASE_PROVIDERS__N__API_BASE."
                    .to_string()
            )
        );

        test_remove_env("OPENAI_BASE_API_BASE");
    }

    #[cfg(oxide_module_llm_provider_openai_base)]
    #[test]
    fn openai_base_profile_env_selects_mistral_profile() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        test_remove_env("OPENAI_BASE_API_BASE");
        test_set_env("OPENAI_BASE_PROVIDERS__0__NAME", "custom-mistral");
        test_set_env(
            "OPENAI_BASE_PROVIDERS__0__API_BASE",
            "https://api.mistral.ai/v1",
        );
        test_set_env("OPENAI_BASE_PROVIDERS__0__PROFILE", "mistral");

        let providers = build_configured_providers(&AgentSettings::default());

        assert!(providers.contains_key("openai-base:custom-mistral"));

        test_remove_env("OPENAI_BASE_PROVIDERS__0__NAME");
        test_remove_env("OPENAI_BASE_PROVIDERS__0__API_BASE");
        test_remove_env("OPENAI_BASE_PROVIDERS__0__PROFILE");
    }

    #[cfg(oxide_module_llm_provider_opencode_go)]
    #[test]
    fn opencode_go_module_registers_provider_id_and_aliases() {
        let settings = settings_with_provider_key("llm-provider/opencode-go", "test-opencode-key");

        let providers = build_configured_providers(&settings);

        assert!(providers.contains_key("llm-provider/opencode-go"));
        assert!(providers.contains_key("opencode-go"));
        assert!(providers.contains_key("opencode_go"));
        assert_eq!(
            provider_module_id("opencode_go"),
            Some("llm-provider/opencode-go")
        );
        assert_eq!(
            provider_module_id("llm-provider/opencode-go"),
            Some("llm-provider/opencode-go")
        );
    }

    #[cfg(oxide_module_llm_provider_opencode_go)]
    #[test]
    fn disabled_opencode_go_module_registers_no_aliases() {
        let mut settings =
            settings_with_provider_key("llm-provider/opencode-go", "test-opencode-key");
        settings.modules.insert(
            "llm-provider/opencode-go".to_string(),
            ModuleRuntimeConfig::disabled(),
        );

        let providers = build_configured_providers(&settings);

        assert!(!providers.contains_key("llm-provider/opencode-go"));
        assert!(!providers.contains_key("opencode-go"));
        assert!(!providers.contains_key("opencode_go"));
    }

    #[cfg(oxide_module_llm_provider_opencode_go)]
    #[test]
    fn opencode_go_module_owns_missing_route_config_message() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_api_key = std::env::var("OPENCODE_GO_API_KEY").ok();
        let previous_primary_api_key = std::env::var("OPENCODE_API_KEY").ok();
        let previous_zen_api_key = std::env::var("OPENCODE_ZEN_API_KEY").ok();
        test_remove_env("OPENCODE_GO_API_KEY");
        test_remove_env("OPENCODE_API_KEY");
        test_remove_env("OPENCODE_ZEN_API_KEY");

        let settings = AgentSettings::default();

        assert_eq!(
            provider_missing_route_config_message("opencode_go", &settings),
            Some(
                "Critical: OPENCODE_API_KEY, OPENCODE_ZEN_API_KEY, or OPENCODE_GO_API_KEY is required for configured OpenCode Go routes".to_string()
            )
        );

        let settings = settings_with_provider_key("llm-provider/opencode-go", "test-opencode-key");

        assert_eq!(
            provider_missing_route_config_message("opencode_go", &settings),
            None
        );

        if let Some(api_key) = previous_api_key {
            test_set_env("OPENCODE_GO_API_KEY", api_key);
        }
        if let Some(api_key) = previous_primary_api_key {
            test_set_env("OPENCODE_API_KEY", api_key);
        }
        if let Some(api_key) = previous_zen_api_key {
            test_set_env("OPENCODE_ZEN_API_KEY", api_key);
        }
    }

    #[cfg(oxide_module_llm_provider_opencode_go)]
    #[test]
    fn opencode_go_capabilities_resolve_provider_id_and_aliases() {
        let provider_id =
            provider_capabilities("llm-provider/opencode-go").expect("provider id should resolve");
        let alias = provider_capabilities("opencode_go").expect("alias should resolve");

        assert_eq!(provider_id.tool_history_label(), "strict");
        assert_eq!(alias.tool_history_label(), "strict");
        assert!(provider_id.supports_tool_calling);
        assert!(alias.supports_tool_calling);
    }

    #[cfg(oxide_module_llm_provider_opencode_go)]
    #[test]
    fn opencode_zen_module_registers_provider_id_and_aliases() {
        let settings = settings_with_provider_key("llm-provider/opencode-zen", "test-opencode-key");

        let providers = build_configured_providers(&settings);

        assert!(providers.contains_key("llm-provider/opencode-zen"));
        assert!(providers.contains_key("opencode-zen"));
        assert!(providers.contains_key("opencode_zen"));
        assert_eq!(
            provider_module_id("opencode_zen"),
            Some("llm-provider/opencode-zen")
        );
    }

    #[cfg(oxide_module_llm_provider_opencode_go)]
    #[test]
    fn opencode_zen_module_accepts_go_key_env_alias() {
        let _guard = test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_go_key = std::env::var("OPENCODE_GO_API_KEY").ok();
        let previous_primary_key = std::env::var("OPENCODE_API_KEY").ok();
        let previous_zen_key = std::env::var("OPENCODE_ZEN_API_KEY").ok();
        test_set_env("OPENCODE_GO_API_KEY", "test-opencode-go-key");
        test_remove_env("OPENCODE_API_KEY");
        test_remove_env("OPENCODE_ZEN_API_KEY");

        let providers = build_configured_providers(&AgentSettings::default());

        assert!(providers.contains_key("opencode-zen"));

        if let Some(api_key) = previous_go_key {
            test_set_env("OPENCODE_GO_API_KEY", api_key);
        } else {
            test_remove_env("OPENCODE_GO_API_KEY");
        }
        if let Some(api_key) = previous_primary_key {
            test_set_env("OPENCODE_API_KEY", api_key);
        }
        if let Some(api_key) = previous_zen_key {
            test_set_env("OPENCODE_ZEN_API_KEY", api_key);
        }
    }

    #[cfg(oxide_module_llm_provider_opencode_go)]
    #[test]
    fn opencode_go_module_all_models_use_native_tools_without_structured_output() {
        let route = crate::config::ModelInfo {
            id: "opencode-go/deepseek-v4-flash".to_string(),
            provider: "llm-provider/opencode-go".to_string(),
            max_output_tokens: 4096,
            context_window_tokens: 128_000,
            weight: 1,
        };

        let capabilities =
            provider_capabilities_for_model(&route).expect("provider id should resolve");

        assert!(capabilities.supports_tool_calling);
        assert!(!capabilities.supports_structured_output);
    }

    #[cfg(oxide_module_llm_provider_openrouter)]
    #[test]
    fn openrouter_module_owns_model_specific_media_capabilities() {
        for model_id in [
            "google/gemini-2.0-flash",
            "google/gemini-2.5-flash-lite",
            "google/gemini-3-flash-preview",
            "google/gemini-3-pro-preview",
            "google/gemini-3.1-flash-lite",
            "google/gemini-3.1-flash-lite-preview",
        ] {
            let route = crate::config::ModelInfo {
                id: model_id.to_string(),
                provider: "llm-provider/openrouter".to_string(),
                max_output_tokens: 4096,
                context_window_tokens: 128_000,
                weight: 1,
            };

            let capabilities = super::provider_media_capabilities_for_model(&route)
                .expect("provider should resolve");

            assert!(capabilities.supports_audio_transcription, "{model_id}");
            assert!(capabilities.supports_image_understanding, "{model_id}");
            assert!(capabilities.supports_video_understanding, "{model_id}");
        }
    }

    #[cfg(oxide_module_llm_provider_anthropic)]
    #[test]
    fn anthropic_module_registers_provider_id_and_aliases() {
        let settings = settings_with_provider_key("llm-provider/anthropic", "test-anthropic-key");

        let providers = build_configured_providers(&settings);

        assert!(providers.contains_key("llm-provider/anthropic"));
        assert!(providers.contains_key("anthropic"));
        assert_eq!(
            provider_module_id("anthropic"),
            Some("llm-provider/anthropic")
        );
    }

    #[cfg(oxide_module_llm_provider_anthropic)]
    #[test]
    fn anthropic_module_owns_base_capabilities() {
        let capabilities =
            provider_capabilities("llm-provider/anthropic").expect("provider should resolve");

        assert_eq!(capabilities.tool_history_label(), "strict");
        assert!(capabilities.supports_tool_calling);
        assert!(!capabilities.supports_structured_output);
    }

    #[cfg(all(
        oxide_module_llm_provider_openai_base,
        oxide_module_llm_provider_anthropic
    ))]
    #[test]
    fn generic_chat_completions_provider_builds_from_kind_endpoint_profile() {
        let ctx = super::LlmProviderBuildContext::new();
        let config = super::GenericEndpointProviderConfig::from_fields(
            "custom-openrouter",
            "chat_completions",
            "https://openrouter.ai/api/v1/chat/completions",
            Some(" token ".to_string()),
            Some("openrouter".to_string()),
        )
        .expect("generic chat completions config should parse");

        let provider = super::build_generic_endpoint_provider(&config, &ctx)
            .expect("generic chat completions provider should build");

        assert_eq!(provider.name, "custom-openrouter");
        assert_eq!(provider.capabilities.tool_history_label(), "best_effort");
        assert!(!provider.capabilities.supports_tool_calling);
        match provider.client {
            super::GenericEndpointClient::ChatCompletions(client) => {
                assert_eq!(
                    client.endpoint(),
                    "https://openrouter.ai/api/v1/chat/completions"
                );
                assert_eq!(client.profile().label, "openrouter");
                assert_eq!(client.auth_header().as_deref(), Some("Bearer token"));
                assert_eq!(client.extra_headers().len(), 3);
            }
            super::GenericEndpointClient::Messages(_) => panic!("expected chat completions client"),
        }
    }

    #[cfg(all(
        oxide_module_llm_provider_openai_base,
        oxide_module_llm_provider_anthropic
    ))]
    #[test]
    fn generic_messages_provider_builds_from_kind_endpoint_profile() {
        let ctx = super::LlmProviderBuildContext::new();
        let config = super::GenericEndpointProviderConfig::from_fields(
            "custom-anthropic",
            "messages",
            "https://api.anthropic.com",
            Some(" key ".to_string()),
            Some("anthropic".to_string()),
        )
        .expect("generic messages config should parse");

        let provider = super::build_generic_endpoint_provider(&config, &ctx)
            .expect("generic messages provider should build");

        assert_eq!(provider.name, "custom-anthropic");
        assert_eq!(provider.capabilities.tool_history_label(), "strict");
        assert!(provider.capabilities.supports_tool_calling);
        assert!(!provider.capabilities.supports_structured_output);
        assert!(!provider.media_capabilities.supports_image_understanding);
        match provider.client {
            super::GenericEndpointClient::Messages(client) => {
                assert_eq!(client.endpoint(), "https://api.anthropic.com/v1/messages");
                assert_eq!(client.profile().label, "Anthropic");
                assert_eq!(client.api_key(), " key ");
                assert!(client.profile().auth_header(client.api_key()).is_none());
            }
            super::GenericEndpointClient::ChatCompletions(_) => panic!("expected messages client"),
        }
    }

    #[cfg(all(
        oxide_module_llm_provider_openai_chatgpt,
        oxide_module_llm_provider_mistral,
        oxide_module_llm_provider_anthropic,
        oxide_module_llm_provider_opencode_go,
        oxide_module_llm_provider_openrouter
    ))]
    #[test]
    fn legacy_aliases_still_build_same_provider_modules() {
        assert_eq!(provider_module_id("mistral"), Some("llm-provider/mistral"));
        assert_eq!(
            provider_module_id("openrouter"),
            Some("llm-provider/openrouter")
        );
        assert_eq!(
            provider_module_id("anthropic"),
            Some("llm-provider/anthropic")
        );
        assert_eq!(
            provider_module_id("opencode_go"),
            Some("llm-provider/opencode-go")
        );
        assert_eq!(
            provider_module_id("opencode_zen"),
            Some("llm-provider/opencode-zen")
        );
        assert_eq!(
            provider_module_id("chatgpt"),
            Some("llm-provider/openai-chatgpt")
        );
        assert_eq!(canonical_route_provider("openai-base"), None);
        assert!(super::GenericProviderKind::from_config_value("chatgpt").is_err());
    }

    #[cfg(oxide_module_llm_provider_mistral)]
    #[test]
    fn mistral_module_registers_provider_id_and_aliases() {
        let settings = settings_with_provider_key("llm-provider/mistral", "test-mistral-key");

        let providers = build_configured_providers(&settings);

        assert!(providers.contains_key("llm-provider/mistral"));
        assert!(providers.contains_key("mistral"));
        assert_eq!(provider_module_id("mistral"), Some("llm-provider/mistral"));
    }

    #[cfg(oxide_module_llm_provider_mistral)]
    #[test]
    fn mistral_module_owns_media_capabilities() {
        let capabilities = super::provider_media_capabilities("llm-provider/mistral")
            .expect("provider should resolve");

        assert!(capabilities.supports_audio_transcription);
        assert!(!capabilities.supports_image_understanding);
        assert!(!capabilities.supports_video_understanding);
    }

    #[cfg(oxide_module_llm_provider_openai_chatgpt)]
    #[test]
    fn chatgpt_module_owns_aliases_and_base_capabilities() {
        let capabilities =
            provider_capabilities("llm-provider/openai-chatgpt").expect("provider should resolve");

        assert_eq!(
            provider_module_id("chatgpt"),
            Some("llm-provider/openai-chatgpt")
        );
        assert_eq!(
            provider_module_id("openai-chatgpt"),
            Some("llm-provider/openai-chatgpt")
        );
        assert_eq!(capabilities.tool_history_label(), "best_effort");
        assert!(capabilities.supports_tool_calling);
        assert!(!capabilities.supports_structured_output);
    }
}
