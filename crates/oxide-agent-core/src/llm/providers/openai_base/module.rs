use std::collections::BTreeMap;
use std::sync::Arc;

use crate::config::AgentSettings;
use crate::llm::LlmProvider;
use crate::llm::capabilities::{MediaCapabilities, ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};

use super::OpenAIBaseProvider;
use super::profile::OpenAICompatibleProfile;

/// Capability module for generic OpenAI-compatible routes.
pub(crate) struct OpenAIBaseProviderModule;

const PROVIDER_PREFIX: &str = "openai-base";
const PROVIDERS_ENV_PREFIX: &str = "OPENAI_BASE_PROVIDERS__";
const LEGACY_ENV_NAMES: &[&str] = &[
    "OPENAI_BASE_API_KEY",
    "OPENAI_BASE_API_BASE",
    "OPENAI_BASE_MODELS_URL",
    "OPENAI_BASE_MODEL_CACHE_TTL_SECS",
];
const MODEL_CACHE_TTL_SECS_DEFAULT: u64 = 30 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenAIBaseEndpointConfig {
    pub(crate) name: String,
    pub(crate) api_base: String,
    pub(crate) api_key: Option<String>,
    pub(crate) models_url: Option<String>,
    pub(crate) model_cache_ttl_secs: u64,
    pub(crate) profile: Option<String>,
}

#[derive(Default)]
struct PartialOpenAIBaseEndpointConfig {
    name: Option<String>,
    api_base: Option<String>,
    api_key: Option<String>,
    models_url: Option<String>,
    model_cache_ttl_secs: Option<u64>,
    profile: Option<String>,
}

impl PartialOpenAIBaseEndpointConfig {
    fn into_endpoint(self) -> Option<OpenAIBaseEndpointConfig> {
        let name = normalize_provider_instance_name(self.name?.as_str())?;
        let api_base = self.api_base?.trim().to_string();
        if api_base.is_empty() {
            return None;
        }
        let api_key = self
            .api_key
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let models_url = self
            .models_url
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        Some(OpenAIBaseEndpointConfig {
            name,
            api_base,
            api_key,
            models_url,
            model_cache_ttl_secs: clamp_model_cache_ttl_secs(
                self.model_cache_ttl_secs
                    .unwrap_or(MODEL_CACHE_TTL_SECS_DEFAULT),
            ),
            profile: self.profile,
        })
    }
}

pub(crate) fn provider_name_for_instance(name: &str) -> Option<String> {
    normalize_provider_instance_name(name).map(|name| format!("{PROVIDER_PREFIX}:{name}"))
}

pub(crate) fn provider_instance_name(provider: &str) -> Option<String> {
    let provider = provider
        .trim()
        .strip_prefix("llm-provider/")
        .unwrap_or(provider.trim())
        .replace('_', "-")
        .to_ascii_lowercase();
    provider
        .strip_prefix("openai-base:")
        .and_then(normalize_provider_instance_name)
}

pub(crate) fn is_legacy_provider_name(provider: &str) -> bool {
    matches!(
        provider
            .trim()
            .strip_prefix("llm-provider/")
            .unwrap_or(provider.trim())
            .replace('_', "-")
            .to_ascii_lowercase()
            .as_str(),
        "openai-base"
    )
}

pub(crate) fn legacy_env_present() -> Option<&'static str> {
    LEGACY_ENV_NAMES.iter().copied().find(|name| {
        std::env::var(name)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
    })
}

pub(crate) fn configured_endpoints() -> Vec<OpenAIBaseEndpointConfig> {
    let mut providers = BTreeMap::<usize, PartialOpenAIBaseEndpointConfig>::new();

    for (key, value) in std::env::vars() {
        if value.trim().is_empty() {
            continue;
        }
        let Some(rest) = key.strip_prefix(PROVIDERS_ENV_PREFIX) else {
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

        let provider = providers.entry(index).or_default();
        match field {
            "NAME" => provider.name = Some(value),
            "API_BASE" => provider.api_base = Some(value),
            "API_KEY" => provider.api_key = Some(value),
            "MODELS_URL" => provider.models_url = Some(value),
            "MODEL_CACHE_TTL_SECS" => {
                provider.model_cache_ttl_secs = value.parse::<u64>().ok();
            }
            "PROFILE" => provider.profile = Some(value),
            _ => {}
        }
    }

    providers
        .into_values()
        .filter_map(PartialOpenAIBaseEndpointConfig::into_endpoint)
        .collect()
}

fn normalize_provider_instance_name(name: &str) -> Option<String> {
    let name = name.trim().replace('_', "-").to_ascii_lowercase();
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        return None;
    }
    Some(name)
}

/// Resolve a profile name string (from `OPENAI_BASE_PROVIDERS__N__PROFILE`)
/// to the corresponding `OpenAICompatibleProfile`. Returns `generic()` for
/// `None`, empty, `"generic"`, or any unrecognized value.
fn resolve_profile(profile: &Option<String>) -> OpenAICompatibleProfile {
    match profile.as_deref() {
        Some("mistral") => OpenAICompatibleProfile::mistral(),
        _ => OpenAICompatibleProfile::generic(),
    }
}

#[cfg(feature = "llm-opencode-go")]
fn clamp_model_cache_ttl_secs(value: u64) -> u64 {
    value.clamp(
        crate::llm::providers::opencode_go::discovery::MIN_MODEL_DISCOVERY_TTL_SECS,
        crate::llm::providers::opencode_go::discovery::MAX_MODEL_DISCOVERY_TTL_SECS,
    )
}

#[cfg(not(feature = "llm-opencode-go"))]
const fn clamp_model_cache_ttl_secs(value: u64) -> u64 {
    value
}

#[cfg(feature = "llm-opencode-go")]
pub(crate) fn build_model_catalogs(
    _settings: &AgentSettings,
    http_client: reqwest::Client,
) -> Vec<Arc<crate::llm::providers::opencode_go::discovery::OpenCodeGoModelCatalog>> {
    configured_endpoints()
        .into_iter()
        .map(|endpoint| {
            let catalog = Arc::new(
                crate::llm::providers::opencode_go::discovery::OpenCodeGoModelCatalog::new(
                    http_client.clone(),
                    endpoint.api_key.clone(),
                    openai_base_discovery_config(&endpoint),
                ),
            );
            Arc::clone(&catalog).spawn_background_refresh();
            catalog
        })
        .collect()
}

#[cfg(feature = "llm-opencode-go")]
fn openai_base_discovery_config(
    endpoint: &OpenAIBaseEndpointConfig,
) -> crate::llm::providers::opencode_go::discovery::OpenCodeGoDiscoveryConfig {
    let provider_id = format!("{PROVIDER_PREFIX}:{}", endpoint.name);
    crate::llm::providers::opencode_go::discovery::OpenCodeGoDiscoveryConfig::new_openai_base_for_provider(
        provider_id,
        endpoint
            .models_url
            .clone()
            .unwrap_or_else(|| models_url_from_api_base(&endpoint.api_base)),
        std::time::Duration::from_secs(endpoint.model_cache_ttl_secs),
    )
}

#[cfg(feature = "llm-opencode-go")]
fn models_url_from_api_base(api_base: &str) -> String {
    let chat_url = super::chat_completions_url(api_base);
    chat_url
        .trim_end_matches("/chat/completions")
        .trim_end_matches('/')
        .to_string()
        + "/models"
}

impl LlmProviderModule for OpenAIBaseProviderModule {
    fn provider_id(&self) -> &'static str {
        "llm-provider/openai-base"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }

    fn build_provider(
        &self,
        _settings: &AgentSettings,
        _ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>> {
        None
    }

    fn build_providers(
        &self,
        _settings: &AgentSettings,
        ctx: &LlmProviderBuildContext,
    ) -> Vec<(String, Arc<dyn LlmProvider>)> {
        configured_endpoints()
            .into_iter()
            .filter_map(|endpoint| {
                let provider_name = provider_name_for_instance(&endpoint.name)?;
                let profile = resolve_profile(&endpoint.profile);
                Some((
                    provider_name,
                    Arc::new(super::OpenAIBaseProvider::new_with_client_and_profile(
                        endpoint.api_key,
                        endpoint.api_base,
                        ctx.http_client.clone(),
                        profile,
                    )) as Arc<dyn LlmProvider>,
                ))
            })
            .collect()
    }

    fn missing_route_config_message(
        &self,
        provider_name: &str,
        _settings: &AgentSettings,
    ) -> Option<String> {
        if let Some(env_name) = legacy_env_present() {
            return Some(format!(
                "Critical: {env_name} is deprecated. Use OPENAI_BASE_PROVIDERS__N__NAME and OPENAI_BASE_PROVIDERS__N__API_BASE."
            ));
        }
        if is_legacy_provider_name(provider_name) {
            return Some(
                "Critical: openai-base routes must use an explicit provider instance such as openai-base:local".to_string(),
            );
        }
        let instance = provider_instance_name(provider_name)?;
        configured_endpoints()
            .into_iter()
            .all(|endpoint| endpoint.name != instance)
            .then(|| format!("Critical: unknown OpenAI Base provider instance '{instance}'"))
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::BestEffort, true, true)
    }

    fn media_capabilities(&self) -> MediaCapabilities {
        MediaCapabilities::new(false, true, false)
    }
}

/// Backward-compatible Mistral route registration.
///
/// Creates an [`OpenAIBaseProvider`] with a Mistral-specific profile when
/// `MISTRAL_API_KEY` is set. Module ID is `"llm-provider/mistral"`.
pub(crate) struct MistralProviderModule;

const MISTRAL_API_KEY_CONFIG_KEY: &str = "api_key";
const MISTRAL_API_KEY_ENV: &str = "MISTRAL_API_KEY";
const MISTRAL_API_BASE: &str = "https://api.mistral.ai/v1";

impl LlmProviderModule for MistralProviderModule {
    fn provider_id(&self) -> &'static str {
        "llm-provider/mistral"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["mistral"]
    }

    fn build_provider(
        &self,
        settings: &AgentSettings,
        ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>> {
        settings
            .module_string_value_or_env(
                self.provider_id(),
                MISTRAL_API_KEY_CONFIG_KEY,
                MISTRAL_API_KEY_ENV,
            )
            .map(|api_key| {
                Arc::new(OpenAIBaseProvider::new_with_client_and_profile(
                    Some(api_key),
                    MISTRAL_API_BASE.to_string(),
                    ctx.http_client.clone(),
                    OpenAICompatibleProfile::mistral(),
                )) as Arc<dyn LlmProvider>
            })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::Strict, true, true)
    }

    fn media_capabilities(&self) -> MediaCapabilities {
        MediaCapabilities::new(true, false, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{test_remove_env, test_set_env};

    #[test]
    fn resolve_profile_none_is_generic() {
        assert_eq!(resolve_profile(&None), OpenAICompatibleProfile::generic());
    }

    #[test]
    fn resolve_profile_generic_string_is_generic() {
        assert_eq!(
            resolve_profile(&Some("generic".to_string())),
            OpenAICompatibleProfile::generic()
        );
    }

    #[test]
    fn resolve_profile_mistral_string() {
        assert_eq!(
            resolve_profile(&Some("mistral".to_string())),
            OpenAICompatibleProfile::mistral()
        );
    }

    #[test]
    fn resolve_profile_unknown_falls_back_to_generic() {
        assert_eq!(
            resolve_profile(&Some("deepseek".to_string())),
            OpenAICompatibleProfile::generic()
        );
    }

    #[test]
    fn configured_endpoints_parses_profile() {
        let _guard = crate::config::test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        test_set_env("OPENAI_BASE_PROVIDERS__0__NAME", "test-profile");
        test_set_env("OPENAI_BASE_PROVIDERS__0__API_BASE", "http://localhost/v1");
        test_set_env("OPENAI_BASE_PROVIDERS__0__PROFILE", "mistral");

        let endpoints = configured_endpoints();
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].profile, Some("mistral".to_string()));

        test_remove_env("OPENAI_BASE_PROVIDERS__0__NAME");
        test_remove_env("OPENAI_BASE_PROVIDERS__0__API_BASE");
        test_remove_env("OPENAI_BASE_PROVIDERS__0__PROFILE");
    }

    #[test]
    fn configured_endpoints_absent_profile_is_none() {
        let _guard = crate::config::test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        test_set_env("OPENAI_BASE_PROVIDERS__0__NAME", "no-profile");
        test_set_env("OPENAI_BASE_PROVIDERS__0__API_BASE", "http://localhost/v1");

        let endpoints = configured_endpoints();
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].profile, None);

        test_remove_env("OPENAI_BASE_PROVIDERS__0__NAME");
        test_remove_env("OPENAI_BASE_PROVIDERS__0__API_BASE");
    }
}
