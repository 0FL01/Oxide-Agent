//! Feature-gated LLM provider modules and factories.

use std::collections::HashMap;
use std::sync::Arc;

use super::super::capabilities::{MediaCapabilities, ProviderCapabilities};
use crate::config::{AgentSettings, ModelInfo};
use crate::llm::LlmProvider;

#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-zai",
    feature = "llm-nvidia",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
use crate::llm::support;

/// Context shared by provider module factories.
pub(crate) struct LlmProviderBuildContext {
    #[cfg(any(
        feature = "llm-chatgpt",
        feature = "llm-mistral",
        feature = "llm-zai",
        feature = "llm-nvidia",
        feature = "llm-opencode-go",
        feature = "llm-openrouter"
    ))]
    pub(crate) http_client: reqwest::Client,
}

impl LlmProviderBuildContext {
    fn new() -> Self {
        Self {
            #[cfg(any(
                feature = "llm-chatgpt",
                feature = "llm-mistral",
                feature = "llm-zai",
                feature = "llm-nvidia",
                feature = "llm-opencode-go",
                feature = "llm-openrouter"
            ))]
            http_client: support::http::create_http_client(),
        }
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

    /// Returns a startup config error when this provider is routed but incomplete.
    fn missing_route_config_message(&self, _settings: &AgentSettings) -> Option<&'static str> {
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

        if let Some(provider) = module.build_provider(settings, &ctx) {
            insert_provider_aliases(&mut providers, module.as_ref(), provider);
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
    find_provider_module(provider_name).map(|module| module.provider_id())
}

/// Returns the provider-owned startup config error for a routed provider.
#[must_use]
pub(crate) fn provider_missing_route_config_message(
    provider_name: &str,
    settings: &AgentSettings,
) -> Option<&'static str> {
    find_provider_module(provider_name)
        .and_then(|module| module.missing_route_config_message(settings))
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

fn insert_provider_aliases(
    providers: &mut HashMap<String, Arc<dyn LlmProvider>>,
    module: &dyn LlmProviderModule,
    provider: Arc<dyn LlmProvider>,
) {
    insert_provider(providers, module.provider_id(), Arc::clone(&provider));
    for alias in module.aliases() {
        insert_provider(providers, alias, Arc::clone(&provider));
    }
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

    #[cfg(feature = "llm-chatgpt")]
    modules.push(Box::new(super::chatgpt::ChatGptProviderModule));
    #[cfg(feature = "llm-mistral")]
    modules.push(Box::new(super::mistral::MistralProviderModule));
    #[cfg(feature = "llm-minimax")]
    modules.push(Box::new(super::minimax::MiniMaxProviderModule));
    #[cfg(feature = "llm-zai")]
    modules.push(Box::new(super::zai::ZaiProviderModule));
    #[cfg(feature = "llm-nvidia")]
    modules.push(Box::new(super::nvidia::NvidiaProviderModule));
    #[cfg(feature = "llm-opencode-go")]
    modules.push(Box::new(super::opencode_go::OpenCodeGoProviderModule));
    #[cfg(feature = "llm-opencode-go")]
    modules.push(Box::new(super::opencode_go::OpenCodeZenProviderModule));
    #[cfg(feature = "llm-openrouter")]
    modules.push(Box::new(super::openrouter::OpenRouterProviderModule));

    modules
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn removed_direct_gemini_provider_aliases_are_absent() {
        for provider in [
            "gemini",
            "google-gemini",
            "google_gemini",
            "llm-provider/gemini",
            "llm-provider/google-gemini",
            "llm-provider/google-gemini-direct",
        ] {
            assert_eq!(
                provider_module_id(provider),
                None,
                "direct Gemini provider alias must stay absent: {provider}"
            );
        }
    }

    #[cfg(feature = "llm-opencode-go")]
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

    #[cfg(feature = "llm-opencode-go")]
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

    #[cfg(feature = "llm-opencode-go")]
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
                "Critical: OPENCODE_API_KEY, OPENCODE_ZEN_API_KEY, or OPENCODE_GO_API_KEY is required for configured OpenCode Go routes"
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

    #[cfg(feature = "llm-opencode-go")]
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

    #[cfg(feature = "llm-opencode-go")]
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

    #[cfg(feature = "llm-opencode-go")]
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

    #[cfg(feature = "llm-opencode-go")]
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

    #[cfg(feature = "llm-zai")]
    #[test]
    fn zai_module_owns_missing_route_config_message() {
        let settings = AgentSettings::default();

        assert_eq!(
            provider_missing_route_config_message("zai", &settings),
            Some("Critical: ZAI_API_KEY is required for configured ZAI routes")
        );

        let settings = settings_with_provider_key("llm-provider/zai", "test-zai-key");

        assert_eq!(
            provider_missing_route_config_message("llm-provider/zai", &settings),
            None
        );
    }

    #[cfg(feature = "llm-zai")]
    #[test]
    fn zai_module_owns_model_specific_structured_output() {
        let route = crate::config::ModelInfo {
            id: "GLM-4.7".to_string(),
            provider: "llm-provider/zai".to_string(),
            max_output_tokens: 4096,
            context_window_tokens: 128_000,
            weight: 1,
        };

        let capabilities =
            provider_capabilities_for_model(&route).expect("provider id should resolve");

        assert!(capabilities.supports_structured_output);
    }

    #[cfg(feature = "llm-nvidia")]
    #[test]
    fn nvidia_module_owns_model_specific_capabilities() {
        let route = crate::config::ModelInfo {
            id: "deepseek-ai/deepseek-r1".to_string(),
            provider: "llm-provider/nvidia".to_string(),
            max_output_tokens: 4096,
            context_window_tokens: 128_000,
            weight: 1,
        };

        let capabilities =
            provider_capabilities_for_model(&route).expect("provider id should resolve");

        assert!(!capabilities.supports_tool_calling);
        assert!(!capabilities.supports_structured_output);
    }

    #[cfg(feature = "llm-openrouter")]
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

    #[cfg(feature = "llm-minimax")]
    #[test]
    fn minimax_module_registers_provider_id_and_aliases() {
        let settings = settings_with_provider_key("llm-provider/minimax", "test-minimax-key");

        let providers = build_configured_providers(&settings);

        assert!(providers.contains_key("llm-provider/minimax"));
        assert!(providers.contains_key("minimax"));
        assert_eq!(provider_module_id("minimax"), Some("llm-provider/minimax"));
    }

    #[cfg(feature = "llm-minimax")]
    #[test]
    fn minimax_module_owns_base_capabilities() {
        let capabilities =
            provider_capabilities("llm-provider/minimax").expect("provider should resolve");

        assert_eq!(capabilities.tool_history_label(), "strict");
        assert!(capabilities.supports_tool_calling);
        assert!(!capabilities.supports_structured_output);
    }

    #[cfg(feature = "llm-mistral")]
    #[test]
    fn mistral_module_registers_provider_id_and_aliases() {
        let settings = settings_with_provider_key("llm-provider/mistral", "test-mistral-key");

        let providers = build_configured_providers(&settings);

        assert!(providers.contains_key("llm-provider/mistral"));
        assert!(providers.contains_key("mistral"));
        assert_eq!(provider_module_id("mistral"), Some("llm-provider/mistral"));
    }

    #[cfg(feature = "llm-mistral")]
    #[test]
    fn mistral_module_owns_media_capabilities() {
        let capabilities = super::provider_media_capabilities("llm-provider/mistral")
            .expect("provider should resolve");

        assert!(capabilities.supports_audio_transcription);
        assert!(!capabilities.supports_image_understanding);
        assert!(!capabilities.supports_video_understanding);
    }

    #[cfg(feature = "llm-chatgpt")]
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
