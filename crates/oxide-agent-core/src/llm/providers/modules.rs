//! Feature-gated LLM provider modules and factories.

use std::collections::HashMap;
use std::sync::Arc;

use super::super::capabilities::{MediaCapabilities, ProviderCapabilities};
use crate::config::AgentSettings;
use crate::config::ModelInfo;
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
pub(crate) fn provider_media_capabilities(provider_name: &str) -> Option<MediaCapabilities> {
    find_provider_module(provider_name).map(|module| module.media_capabilities())
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
    #[cfg(feature = "llm-groq")]
    modules.push(Box::new(super::groq::GroqProviderModule));
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
    #[cfg(feature = "llm-openrouter")]
    modules.push(Box::new(super::openrouter::OpenRouterProviderModule));

    modules
}

#[cfg(test)]
mod tests {
    use super::{
        build_configured_providers, provider_capabilities, provider_capabilities_for_model,
        provider_key, provider_media_capabilities, provider_missing_route_config_message,
        provider_module_id,
    };
    use crate::config::{AgentSettings, ModuleRuntimeConfig};

    #[test]
    fn provider_key_is_case_insensitive() {
        assert_eq!(provider_key("OpenCode-Go"), "opencode-go");
    }

    #[cfg(feature = "llm-opencode-go")]
    #[test]
    fn opencode_go_module_registers_provider_id_and_aliases() {
        let settings = AgentSettings {
            opencode_go_api_key: Some("test-opencode-key".to_string()),
            ..AgentSettings::default()
        };

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
        let mut settings = AgentSettings {
            opencode_go_api_key: Some("test-opencode-key".to_string()),
            ..AgentSettings::default()
        };
        settings.modules.insert(
            "llm-provider/opencode-go".to_string(),
            ModuleRuntimeConfig {
                enabled: Some(false),
            },
        );

        let providers = build_configured_providers(&settings);

        assert!(!providers.contains_key("llm-provider/opencode-go"));
        assert!(!providers.contains_key("opencode-go"));
        assert!(!providers.contains_key("opencode_go"));
    }

    #[cfg(feature = "llm-opencode-go")]
    #[test]
    fn opencode_go_module_owns_missing_route_config_message() {
        let settings = AgentSettings::default();

        assert_eq!(
            provider_missing_route_config_message("opencode_go", &settings),
            Some("Critical: OPENCODE_GO_API_KEY is required for configured OpenCode Go routes")
        );

        let settings = AgentSettings {
            opencode_go_api_key: Some("test-opencode-key".to_string()),
            ..AgentSettings::default()
        };

        assert_eq!(
            provider_missing_route_config_message("opencode_go", &settings),
            None
        );
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
    fn opencode_go_module_owns_model_specific_structured_output() {
        let route = crate::config::ModelInfo {
            id: "opencode-go/deepseek-v4-flash".to_string(),
            provider: "llm-provider/opencode-go".to_string(),
            max_output_tokens: 4096,
            context_window_tokens: 128_000,
            weight: 1,
        };

        let capabilities =
            provider_capabilities_for_model(&route).expect("provider id should resolve");

        assert!(capabilities.supports_structured_output);
    }

    #[cfg(feature = "llm-zai")]
    #[test]
    fn zai_module_owns_missing_route_config_message() {
        let settings = AgentSettings::default();

        assert_eq!(
            provider_missing_route_config_message("zai", &settings),
            Some("Critical: ZAI_API_KEY is required for configured ZAI routes")
        );

        let settings = AgentSettings {
            zai_api_key: Some("test-zai-key".to_string()),
            ..AgentSettings::default()
        };

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
    fn openrouter_module_owns_media_capabilities() {
        let capabilities = provider_media_capabilities("llm-provider/openrouter")
            .expect("provider should resolve");

        assert!(capabilities.supports_audio_transcription);
        assert!(capabilities.supports_image_understanding);
        assert!(capabilities.supports_video_understanding);
    }

    #[cfg(feature = "llm-groq")]
    #[test]
    fn groq_module_registers_provider_id_and_aliases() {
        let settings = AgentSettings {
            groq_api_key: Some("test-groq-key".to_string()),
            ..AgentSettings::default()
        };

        let providers = build_configured_providers(&settings);

        assert!(providers.contains_key("llm-provider/groq"));
        assert!(providers.contains_key("groq"));
        assert_eq!(provider_module_id("groq"), Some("llm-provider/groq"));
    }

    #[cfg(feature = "llm-groq")]
    #[test]
    fn groq_module_owns_base_capabilities() {
        let capabilities =
            provider_capabilities("llm-provider/groq").expect("provider should resolve");

        assert_eq!(capabilities.tool_history_label(), "best_effort");
        assert!(!capabilities.supports_tool_calling);
        assert!(capabilities.supports_structured_output);
    }

    #[cfg(feature = "llm-minimax")]
    #[test]
    fn minimax_module_registers_provider_id_and_aliases() {
        let settings = AgentSettings {
            minimax_api_key: Some("test-minimax-key".to_string()),
            ..AgentSettings::default()
        };

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
        let settings = AgentSettings {
            mistral_api_key: Some("test-mistral-key".to_string()),
            ..AgentSettings::default()
        };

        let providers = build_configured_providers(&settings);

        assert!(providers.contains_key("llm-provider/mistral"));
        assert!(providers.contains_key("mistral"));
        assert_eq!(provider_module_id("mistral"), Some("llm-provider/mistral"));
    }

    #[cfg(feature = "llm-mistral")]
    #[test]
    fn mistral_module_owns_media_capabilities() {
        let capabilities =
            provider_media_capabilities("llm-provider/mistral").expect("provider should resolve");

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
