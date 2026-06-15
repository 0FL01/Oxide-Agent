//! Feature-gated compiled capability module list.

use super::{
    CapabilityId, CapabilityModule, CapabilityRequirement, CompiledCapabilityManifest,
    ManifestError, ModuleConfigProperty,
};

macro_rules! push_module {
    ($modules:ident, $feature:literal, $id:literal, $kind:ident, [$($capability:literal),+ $(,)?]) => {
        #[cfg(feature = $feature)]
        {
            const PROVIDES: &[$crate::capabilities::CapabilityId] =
                &[$($crate::capabilities::CapabilityId::new($capability)),+];
            $modules.push(Box::new($crate::capabilities::StaticCapabilityModule::new(
                $crate::capabilities::ModuleId::new($id),
                $crate::capabilities::CapabilityKind::$kind,
                $feature,
                PROVIDES,
            )));
        }
    };
}

macro_rules! push_module_with_requires {
    ($modules:ident, $feature:literal, $id:literal, $kind:ident, [$($capability:literal),+ $(,)?], $requires:expr) => {
        #[cfg(feature = $feature)]
        {
            const PROVIDES: &[$crate::capabilities::CapabilityId] =
                &[$($crate::capabilities::CapabilityId::new($capability)),+];
            $modules.push(Box::new(
                $crate::capabilities::StaticCapabilityModule::new(
                    $crate::capabilities::ModuleId::new($id),
                    $crate::capabilities::CapabilityKind::$kind,
                    $feature,
                    PROVIDES,
                )
                .with_requires($requires),
            ));
        }
    };
}

macro_rules! push_module_with_config {
    ($modules:ident, $feature:literal, $id:literal, $kind:ident, [$($capability:literal),+ $(,)?], $config_properties:expr) => {
        #[cfg(feature = $feature)]
        {
            const PROVIDES: &[$crate::capabilities::CapabilityId] =
                &[$($crate::capabilities::CapabilityId::new($capability)),+];
            $modules.push(Box::new(
                $crate::capabilities::StaticCapabilityModule::new(
                    $crate::capabilities::ModuleId::new($id),
                    $crate::capabilities::CapabilityKind::$kind,
                    $feature,
                    PROVIDES,
                )
                .with_config_properties($config_properties),
            ));
        }
    };
}

#[allow(dead_code)]
const SANDBOX_FILEOPS_BACKEND_CAPABILITIES: &[CapabilityId] = &[
    CapabilityId::new("sandbox-backend/docker-direct/fileops"),
    CapabilityId::new("sandbox-backend/sandboxd-client/fileops"),
];
#[allow(dead_code)]
const SANDBOX_EXEC_BACKEND_CAPABILITIES: &[CapabilityId] = &[
    CapabilityId::new("sandbox-backend/docker-direct/exec"),
    CapabilityId::new("sandbox-backend/sandboxd-client/exec"),
];
#[allow(dead_code)]
const SANDBOX_LIFECYCLE_BACKEND_CAPABILITIES: &[CapabilityId] = &[
    CapabilityId::new("sandbox-backend/docker-direct/lifecycle"),
    CapabilityId::new("sandbox-backend/sandboxd-client/lifecycle"),
];
#[allow(dead_code)]
const SANDBOX_DIAGNOSTICS_BACKEND_CAPABILITIES: &[CapabilityId] = &[
    CapabilityId::new("sandbox-backend/docker-direct/diagnostics"),
    CapabilityId::new("sandbox-backend/sandboxd-client/diagnostics"),
];

#[allow(dead_code)]
const SANDBOX_FILEOPS_BACKEND_REQUIREMENT: &[CapabilityRequirement] =
    &[CapabilityRequirement::any_of(
        SANDBOX_FILEOPS_BACKEND_CAPABILITIES,
    )];
#[allow(dead_code)]
const SANDBOX_EXEC_BACKEND_REQUIREMENT: &[CapabilityRequirement] =
    &[CapabilityRequirement::any_of(
        SANDBOX_EXEC_BACKEND_CAPABILITIES,
    )];
#[allow(dead_code)]
const SANDBOX_LIFECYCLE_BACKEND_REQUIREMENT: &[CapabilityRequirement] =
    &[CapabilityRequirement::any_of(
        SANDBOX_LIFECYCLE_BACKEND_CAPABILITIES,
    )];
#[allow(dead_code)]
const SANDBOX_DIAGNOSTICS_BACKEND_REQUIREMENT: &[CapabilityRequirement] =
    &[CapabilityRequirement::any_of(
        SANDBOX_DIAGNOSTICS_BACKEND_CAPABILITIES,
    )];
#[allow(dead_code)]
const SANDBOX_EXEC_AND_FILEOPS_BACKEND_REQUIREMENT: &[CapabilityRequirement] = &[
    CapabilityRequirement::any_of(SANDBOX_EXEC_BACKEND_CAPABILITIES),
    CapabilityRequirement::any_of(SANDBOX_FILEOPS_BACKEND_CAPABILITIES),
];
#[allow(dead_code)]
const SANDBOX_DOCKER_BACKEND_REQUIREMENT: &[CapabilityRequirement] = &[CapabilityRequirement::new(
    CapabilityId::new("sandbox-backend/docker-direct"),
)];

#[allow(dead_code)]
const CHATGPT_CONFIG_PROPERTIES: &[ModuleConfigProperty] =
    &[
        ModuleConfigProperty::string("auth_path", "Path to the ChatGPT/Codex OAuth auth record.")
            .with_env("CHATGPT_AUTH_PATH"),
    ];
#[allow(dead_code)]
const MISTRAL_CONFIG_PROPERTIES: &[ModuleConfigProperty] =
    &[ModuleConfigProperty::string("api_key", "Mistral API key.")
        .with_env("MISTRAL_API_KEY")
        .secret()];
#[allow(dead_code)]
const ANTHROPIC_CONFIG_PROPERTIES: &[ModuleConfigProperty] =
    &[
        ModuleConfigProperty::string("api_key", "Anthropic API key.")
            .with_env("ANTHROPIC_API_KEY")
            .secret(),
    ];
#[allow(dead_code)]
const OPENAI_BASE_CONFIG_PROPERTIES: &[ModuleConfigProperty] = &[
    ModuleConfigProperty::string(
        "providers",
        "OpenAI-compatible provider instances configured via OPENAI_BASE_PROVIDERS__N__* env vars.",
    )
    .with_env("OPENAI_BASE_PROVIDERS__N__NAME"),
    ModuleConfigProperty::string(
        "providers.N.api_base",
        "OpenAI-compatible API base URL or chat completions endpoint for provider instance N.",
    )
    .with_env("OPENAI_BASE_PROVIDERS__N__API_BASE"),
    ModuleConfigProperty::string(
        "providers.N.api_key",
        "Optional bearer token for OpenAI-compatible provider instance N.",
    )
    .with_env("OPENAI_BASE_PROVIDERS__N__API_KEY")
    .secret(),
    ModuleConfigProperty::string(
        "providers.N.models_url",
        "Optional OpenAI-compatible model discovery endpoint for provider instance N. Defaults to api_base /models.",
    )
    .with_env("OPENAI_BASE_PROVIDERS__N__MODELS_URL"),
    ModuleConfigProperty::string(
        "providers.N.model_cache_ttl_secs",
        "OpenAI Base model discovery cache TTL for provider instance N.",
    )
    .with_env("OPENAI_BASE_PROVIDERS__N__MODEL_CACHE_TTL_SECS")
    .with_default("1800"),
    ModuleConfigProperty::string(
        "providers.N.profile",
        "Behavioral profile for provider instance N: 'generic' (default), 'mistral', or 'zai'. Controls tool-call ID mapping, message layout, response parsing, temperatures, streaming, reasoning, and audio transcription.",
    )
    .with_env("OPENAI_BASE_PROVIDERS__N__PROFILE"),
];
#[allow(dead_code)]
const OPENCODE_GO_CONFIG_PROPERTIES: &[ModuleConfigProperty] = &[
    ModuleConfigProperty::string(
        "api_key",
        "OpenCode Go API key. Also accepts OPENCODE_ZEN_API_KEY and legacy OPENCODE_GO_API_KEY.",
    )
    .with_env("OPENCODE_API_KEY")
    .secret(),
    ModuleConfigProperty::string("api_base", "OpenCode Go Chat Completions endpoint.")
        .with_env("OPENCODE_GO_API_BASE")
        .with_default("https://opencode.ai/zen/go/v1/chat/completions"),
    ModuleConfigProperty::string(
        "messages_api_base",
        "OpenCode Go Anthropic Messages endpoint.",
    )
    .with_env("OPENCODE_GO_MESSAGES_API_BASE")
    .with_default("https://opencode.ai/zen/go/v1/messages"),
    ModuleConfigProperty::string("models_url", "OpenCode Go model discovery endpoint.")
        .with_env("OPENCODE_GO_MODELS_URL")
        .with_default("https://opencode.ai/zen/go/v1/models"),
    ModuleConfigProperty::string(
        "model_cache_ttl_secs",
        "OpenCode Go model discovery cache TTL.",
    )
    .with_env("OPENCODE_GO_MODEL_CACHE_TTL_SECS")
    .with_default("1800"),
];
#[allow(dead_code)]
const OPENCODE_ZEN_CONFIG_PROPERTIES: &[ModuleConfigProperty] = &[
    ModuleConfigProperty::string(
        "api_key",
        "OpenCode Zen API key. Also accepts OPENCODE_API_KEY and OPENCODE_GO_API_KEY.",
    )
    .with_env("OPENCODE_ZEN_API_KEY")
    .secret(),
    ModuleConfigProperty::string("api_base", "OpenCode Zen Chat Completions endpoint.")
        .with_env("OPENCODE_ZEN_API_BASE")
        .with_default("https://opencode.ai/zen/v1/chat/completions"),
    ModuleConfigProperty::string(
        "messages_api_base",
        "OpenCode Zen Anthropic Messages endpoint.",
    )
    .with_env("OPENCODE_ZEN_MESSAGES_API_BASE")
    .with_default("https://opencode.ai/zen/v1/messages"),
    ModuleConfigProperty::string("models_url", "OpenCode Zen free model discovery endpoint.")
        .with_env("OPENCODE_ZEN_MODELS_URL")
        .with_default("https://opencode.ai/zen/v1/models"),
    ModuleConfigProperty::string(
        "model_cache_ttl_secs",
        "OpenCode Zen model discovery cache TTL.",
    )
    .with_env("OPENCODE_ZEN_MODEL_CACHE_TTL_SECS")
    .with_default("1800"),
];
#[allow(dead_code)]
const OPENROUTER_CONFIG_PROPERTIES: &[ModuleConfigProperty] =
    &[
        ModuleConfigProperty::string("api_key", "OpenRouter API key.")
            .with_env("OPENROUTER_API_KEY")
            .secret(),
    ];
#[allow(dead_code)]
const SQLX_STORAGE_CONFIG_PROPERTIES: &[ModuleConfigProperty] = &[
    ModuleConfigProperty::string(
        "database_url",
        "Postgres connection URL. Also accepts DATABASE_URL as a runtime fallback.",
    )
    .with_env("OXIDE_DATABASE_URL")
    .secret(),
    ModuleConfigProperty::string("max_connections", "Maximum Postgres pool connections.")
        .with_env("OXIDE_DATABASE_MAX_CONNECTIONS")
        .with_default("5"),
    ModuleConfigProperty::string(
        "connect_timeout_secs",
        "Postgres pool connection/acquire timeout in seconds.",
    )
    .with_env("OXIDE_DATABASE_CONNECT_TIMEOUT_SECS")
    .with_default("10"),
    ModuleConfigProperty::string(
        "migrate_on_startup",
        "Run SQLx migrations during storage startup when set to true.",
    )
    .with_env("OXIDE_DATABASE_MIGRATE_ON_STARTUP")
    .with_default("false"),
    ModuleConfigProperty::string("migrations_dir", "Runtime path to SQLx migration files.")
        .with_env("OXIDE_DATABASE_MIGRATIONS_DIR")
        .with_default("migrations"),
];

/// Returns the deterministic list of modules compiled into this build.
#[must_use]
pub fn compiled_modules() -> Vec<Box<dyn CapabilityModule>> {
    let mut modules: Vec<Box<dyn CapabilityModule>> = Vec::new();
    push_declared_modules(&mut modules);
    modules
}

/// Builds the deterministic compiled capability manifest for this build.
pub fn compiled_capability_manifest() -> Result<CompiledCapabilityManifest, ManifestError> {
    let modules = compiled_modules();
    CompiledCapabilityManifest::from_modules(&modules)
}

/// Returns the selected named profile when exactly one profile feature is active.
#[must_use]
pub fn compiled_profile_name() -> Option<&'static str> {
    let active_profile_count = cfg!(feature = "profile-embedded-opencode-local") as usize
        + cfg!(feature = "profile-web-embedded-opencode-local") as usize
        + cfg!(feature = "profile-lite") as usize
        + cfg!(feature = "profile-search-only") as usize
        + cfg!(feature = "profile-no-sandbox") as usize
        + cfg!(feature = "profile-media-enabled") as usize
        + cfg!(feature = "profile-full") as usize;

    if active_profile_count != 1 {
        return None;
    }

    if cfg!(feature = "profile-embedded-opencode-local") {
        Some("embedded-opencode-local")
    } else if cfg!(feature = "profile-web-embedded-opencode-local") {
        Some("web-embedded-opencode-local")
    } else if cfg!(feature = "profile-lite") {
        Some("lite")
    } else if cfg!(feature = "profile-search-only") {
        Some("search-only")
    } else if cfg!(feature = "profile-no-sandbox") {
        Some("no-sandbox")
    } else if cfg!(feature = "profile-media-enabled") {
        Some("media-enabled")
    } else {
        Some("full")
    }
}

fn push_declared_modules(modules: &mut Vec<Box<dyn CapabilityModule>>) {
    push_transport_and_storage_modules(modules);
    push_llm_modules(modules);
    push_tool_modules(modules);
    push_runtime_and_integration_modules(modules);
}

fn push_transport_and_storage_modules(modules: &mut Vec<Box<dyn CapabilityModule>>) {
    let _ = &modules;
    push_module!(
        modules,
        "transport-telegram",
        "transport/telegram",
        Transport,
        ["transport/telegram"]
    );
    push_module!(
        modules,
        "transport-web",
        "transport/web",
        Transport,
        ["transport/web"]
    );
    push_module_with_config!(
        modules,
        "storage-sqlx",
        "storage/sqlx",
        StorageBackend,
        ["storage/sqlx"],
        SQLX_STORAGE_CONFIG_PROPERTIES
    );
}

fn push_llm_modules(modules: &mut Vec<Box<dyn CapabilityModule>>) {
    let _ = &modules;
    push_module_with_config!(
        modules,
        "llm-chatgpt",
        "llm-provider/openai-chatgpt",
        LlmProvider,
        ["llm-provider/openai-chatgpt"],
        CHATGPT_CONFIG_PROPERTIES
    );
    push_module_with_config!(
        modules,
        "llm-mistral",
        "llm-provider/mistral",
        LlmProvider,
        ["llm-provider/mistral"],
        MISTRAL_CONFIG_PROPERTIES
    );
    push_module_with_config!(
        modules,
        "llm-minimax",
        "llm-provider/anthropic",
        LlmProvider,
        ["llm-provider/anthropic"],
        ANTHROPIC_CONFIG_PROPERTIES
    );
    push_module_with_config!(
        modules,
        "llm-openai-base",
        "llm-provider/openai-base",
        LlmProvider,
        ["llm-provider/openai-base"],
        OPENAI_BASE_CONFIG_PROPERTIES
    );
    push_module_with_config!(
        modules,
        "llm-opencode-go",
        "llm-provider/opencode-go",
        LlmProvider,
        ["llm-provider/opencode-go"],
        OPENCODE_GO_CONFIG_PROPERTIES
    );
    push_module_with_config!(
        modules,
        "llm-opencode-go",
        "llm-provider/opencode-zen",
        LlmProvider,
        ["llm-provider/opencode-zen"],
        OPENCODE_ZEN_CONFIG_PROPERTIES
    );
    push_module_with_config!(
        modules,
        "llm-openrouter",
        "llm-provider/openrouter",
        LlmProvider,
        ["llm-provider/openrouter"],
        OPENROUTER_CONFIG_PROPERTIES
    );
}

fn push_tool_modules(modules: &mut Vec<Box<dyn CapabilityModule>>) {
    let _ = &modules;
    push_module!(modules, "tool-todos", "tool/todos", Tool, ["tool/todos"]);
    push_module!(
        modules,
        "tool-compression",
        "tool/compression",
        Tool,
        ["tool/compression"]
    );
    push_module!(
        modules,
        "tool-delegation",
        "tool/delegation",
        Tool,
        ["tool/delegation"]
    );
    push_module!(
        modules,
        "tool-agents-md",
        "tool/agents-md",
        Tool,
        ["tool/agents-md"]
    );
    push_module!(
        modules,
        "tool-reminder",
        "tool/reminder",
        Reminder,
        ["tool/reminder"]
    );
    push_module!(
        modules,
        "tool-wiki-memory",
        "tool/wiki-memory",
        Memory,
        ["tool/wiki-memory"]
    );
    push_module!(
        modules,
        "tool-webfetch-md",
        "tool/webfetch-md",
        Search,
        ["tool/webfetch-md"]
    );
    push_module!(
        modules,
        "tool-webfetch-md",
        "tool/web-crawler",
        Search,
        ["tool/web-crawler"]
    );
    push_module!(
        modules,
        "tool-tavily",
        "tool/tavily",
        Search,
        ["tool/tavily-search", "tool/tavily-extract"]
    );
    push_module!(
        modules,
        "tool-brave-search",
        "tool/brave-search",
        Search,
        ["tool/brave-search"]
    );
    push_module!(
        modules,
        "tool-crw",
        "tool/crw",
        Search,
        ["tool/crw-search", "tool/crw-scrape"]
    );
    push_module_with_requires!(
        modules,
        "tool-sandbox-fileops",
        "tool/sandbox-fileops",
        SandboxTool,
        ["tool/sandbox-fileops", "tool/sandbox-list-files"],
        SANDBOX_FILEOPS_BACKEND_REQUIREMENT
    );
    push_module_with_requires!(
        modules,
        "tool-sandbox-exec",
        "tool/sandbox-exec",
        SandboxTool,
        ["tool/sandbox-exec"],
        SANDBOX_EXEC_BACKEND_REQUIREMENT
    );
    push_module_with_requires!(
        modules,
        "tool-sandbox-recreate",
        "tool/sandbox-recreate",
        SandboxTool,
        ["tool/sandbox-recreate"],
        SANDBOX_LIFECYCLE_BACKEND_REQUIREMENT
    );
    push_module!(
        modules,
        "tool-file-delivery",
        "tool/file-delivery",
        FileDelivery,
        ["tool/file-delivery"]
    );
    push_module!(
        modules,
        "tool-media-audio",
        "tool/media-audio",
        Media,
        ["tool/media-audio-transcription"]
    );
    push_module!(
        modules,
        "tool-media-image",
        "tool/media-image",
        Media,
        ["tool/media-image-description"]
    );
    push_module!(
        modules,
        "tool-media-video",
        "tool/media-video",
        Media,
        ["tool/media-video-description"]
    );
    push_module_with_requires!(
        modules,
        "tool-ytdlp",
        "tool/ytdlp",
        Media,
        [
            "tool/ytdlp-metadata",
            "tool/ytdlp-transcript",
            "tool/ytdlp-download"
        ],
        SANDBOX_EXEC_AND_FILEOPS_BACKEND_REQUIREMENT
    );
    push_module!(
        modules,
        "tool-tts-kokoro",
        "tool/tts-kokoro",
        Media,
        ["tool/tts-kokoro"]
    );
    push_module!(
        modules,
        "tool-tts-silero",
        "tool/tts-silero",
        Media,
        ["tool/tts-silero"]
    );
    push_module_with_requires!(
        modules,
        "tool-stack-logs",
        "tool/stack-logs",
        Diagnostics,
        ["tool/stack-logs"],
        SANDBOX_DIAGNOSTICS_BACKEND_REQUIREMENT
    );
}

fn push_runtime_and_integration_modules(modules: &mut Vec<Box<dyn CapabilityModule>>) {
    let _ = &modules;
    push_module!(
        modules,
        "sandbox-backend-docker-direct",
        "sandbox-backend/docker-direct",
        SandboxBackend,
        [
            "sandbox-backend/docker-direct",
            "sandbox-backend/docker-direct/fileops",
            "sandbox-backend/docker-direct/exec",
            "sandbox-backend/docker-direct/lifecycle",
            "sandbox-backend/docker-direct/diagnostics"
        ]
    );
    push_module!(
        modules,
        "sandbox-backend-sandboxd-client",
        "sandbox-backend/sandboxd-client",
        SandboxBackend,
        [
            "sandbox-backend/sandboxd-client",
            "sandbox-backend/sandboxd-client/fileops",
            "sandbox-backend/sandboxd-client/exec",
            "sandbox-backend/sandboxd-client/lifecycle",
            "sandbox-backend/sandboxd-client/diagnostics"
        ]
    );
    push_module_with_requires!(
        modules,
        "sandbox-daemon",
        "sandbox-daemon/sandboxd",
        Service,
        ["sandbox-daemon/sandboxd"],
        SANDBOX_DOCKER_BACKEND_REQUIREMENT
    );

    push_module!(
        modules,
        "integration-mcp-jira",
        "integration/mcp-jira",
        McpIntegration,
        ["integration/mcp-jira"]
    );
    push_module!(
        modules,
        "integration-mcp-mattermost",
        "integration/mcp-mattermost",
        McpIntegration,
        ["integration/mcp-mattermost"]
    );
    push_module!(
        modules,
        "integration-ssh-mcp",
        "integration/ssh-mcp",
        McpIntegration,
        ["integration/ssh-mcp"]
    );
    push_module!(
        modules,
        "manager-control-plane",
        "manager/control-plane",
        Manager,
        [
            "manager/control-plane",
            "manager/topic-sandbox-admin",
            "manager/agent-profile-admin"
        ]
    );
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "storage-sqlx")]
    #[test]
    fn compiled_manifest_exposes_compiled_durable_storage_backends() {
        let manifest =
            super::compiled_capability_manifest().expect("compiled manifest should be valid");
        let storage_backend_ids: Vec<_> = manifest
            .modules()
            .iter()
            .filter(|module| module.kind() == crate::capabilities::CapabilityKind::StorageBackend)
            .map(|module| module.id().as_str())
            .collect();
        let expected = vec!["storage/sqlx"];

        assert_eq!(
            storage_backend_ids, expected,
            "compiled durable storage backend modules must match active storage features"
        );
    }

    #[cfg(feature = "llm-openrouter")]
    #[test]
    fn openrouter_module_declares_provider_config_schema() {
        let manifest =
            super::compiled_capability_manifest().expect("compiled manifest should be valid");
        let schema = manifest.config_schema();
        let openrouter = &schema["properties"]["modules"]["properties"]["llm-provider/openrouter"];

        assert_eq!(openrouter["additionalProperties"], false);
        assert_eq!(openrouter["properties"]["api_key"]["type"], "string");
        assert_eq!(
            openrouter["properties"]["api_key"]["x-oxide-env"],
            "OPENROUTER_API_KEY"
        );
        assert_eq!(openrouter["properties"]["api_key"]["x-oxide-secret"], true);
        let properties = openrouter["properties"]
            .as_object()
            .expect("OpenRouter config properties should be an object");
        assert_eq!(properties.len(), 2);
        assert!(properties.contains_key("enabled"));
        assert!(properties.contains_key("api_key"));
    }
}
