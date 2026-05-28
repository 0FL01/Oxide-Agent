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
    CapabilityId::new("sandbox-backend/bwrap/fileops"),
];
#[allow(dead_code)]
const SANDBOX_EXEC_BACKEND_CAPABILITIES: &[CapabilityId] = &[
    CapabilityId::new("sandbox-backend/docker-direct/exec"),
    CapabilityId::new("sandbox-backend/sandboxd-client/exec"),
    CapabilityId::new("sandbox-backend/bwrap/exec"),
];
#[allow(dead_code)]
const SANDBOX_LIFECYCLE_BACKEND_CAPABILITIES: &[CapabilityId] = &[
    CapabilityId::new("sandbox-backend/docker-direct/lifecycle"),
    CapabilityId::new("sandbox-backend/sandboxd-client/lifecycle"),
    CapabilityId::new("sandbox-backend/bwrap/lifecycle"),
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
const MINIMAX_CONFIG_PROPERTIES: &[ModuleConfigProperty] =
    &[ModuleConfigProperty::string("api_key", "MiniMax API key.")
        .with_env("MINIMAX_API_KEY")
        .secret()];
#[allow(dead_code)]
const ZAI_CONFIG_PROPERTIES: &[ModuleConfigProperty] = &[
    ModuleConfigProperty::string("api_key", "ZAI/Zhipu API key.")
        .with_env("ZAI_API_KEY")
        .secret(),
    ModuleConfigProperty::string("api_base", "ZAI/Zhipu Chat Completions API endpoint.")
        .with_env("ZAI_API_BASE")
        .with_default("https://api.z.ai/api/coding/paas/v4/chat/completions"),
];
#[allow(dead_code)]
const NVIDIA_CONFIG_PROPERTIES: &[ModuleConfigProperty] = &[
    ModuleConfigProperty::string("api_key", "NVIDIA NIM API key.")
        .with_env("NVIDIA_API_KEY")
        .secret(),
    ModuleConfigProperty::string("api_base", "NVIDIA NIM OpenAI-compatible API base URL.")
        .with_env("NVIDIA_API_BASE")
        .with_default("https://integrate.api.nvidia.com/v1"),
];
#[allow(dead_code)]
const OPENCODE_GO_CONFIG_PROPERTIES: &[ModuleConfigProperty] = &[
    ModuleConfigProperty::string("api_key", "OpenCode Go API key.")
        .with_env("OPENCODE_GO_API_KEY")
        .secret(),
    ModuleConfigProperty::string("api_base", "OpenCode Go Chat Completions endpoint.")
        .with_env("OPENCODE_GO_API_BASE")
        .with_default("https://opencode.ai/zen/go/v1/chat/completions"),
];
#[allow(dead_code)]
const OPENROUTER_CONFIG_PROPERTIES: &[ModuleConfigProperty] =
    &[
        ModuleConfigProperty::string("api_key", "OpenRouter API key.")
            .with_env("OPENROUTER_API_KEY")
            .secret(),
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
        + cfg!(feature = "profile-host-bwrap") as usize
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
    } else if cfg!(feature = "profile-host-bwrap") {
        Some("host-bwrap")
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
    push_module!(
        modules,
        "transport-cli",
        "transport/cli",
        Transport,
        ["transport/cli"]
    );
    push_module!(
        modules,
        "transport-http-api",
        "transport/http-api",
        Transport,
        ["transport/http-api"]
    );

    push_module!(
        modules,
        "storage-s3-r2",
        "storage/r2",
        StorageBackend,
        ["storage/r2"]
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
        "llm-provider/minimax",
        LlmProvider,
        ["llm-provider/minimax"],
        MINIMAX_CONFIG_PROPERTIES
    );
    push_module_with_config!(
        modules,
        "llm-zai",
        "llm-provider/zai",
        LlmProvider,
        ["llm-provider/zai"],
        ZAI_CONFIG_PROPERTIES
    );
    push_module_with_config!(
        modules,
        "llm-nvidia",
        "llm-provider/nvidia",
        LlmProvider,
        ["llm-provider/nvidia"],
        NVIDIA_CONFIG_PROPERTIES
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
        "tool-tavily",
        "tool/tavily",
        Search,
        ["tool/tavily-search", "tool/tavily-extract"]
    );
    push_module!(
        modules,
        "tool-searxng",
        "tool/searxng",
        Search,
        ["tool/searxng-search"]
    );
    push_module!(
        modules,
        "tool-browser-use",
        "tool/browser-use",
        Browser,
        ["tool/browser-use"]
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
    push_module!(
        modules,
        "sandbox-backend-bwrap",
        "sandbox-backend/bwrap",
        SandboxBackend,
        [
            "sandbox-backend/bwrap",
            "sandbox-backend/bwrap/fileops",
            "sandbox-backend/bwrap/exec",
            "sandbox-backend/bwrap/lifecycle"
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
    use super::compiled_capability_manifest;
    use crate::capabilities::CapabilityKind;

    #[test]
    fn transient_local_fs_is_not_registered_as_durable_storage_backend() {
        let manifest = compiled_capability_manifest().expect("compiled manifest should be valid");

        assert!(
            manifest
                .modules()
                .iter()
                .all(|module| module.id().as_str() != "storage/local-fs-transient"),
            "storage-local-fs is transient workspace only and must not register a storage backend module"
        );
        assert!(
            manifest
                .capabilities()
                .iter()
                .all(|capability| capability.id().as_str() != "storage/local-fs-transient"),
            "storage-local-fs must not expose a durable storage capability"
        );
    }

    #[cfg(feature = "storage-s3-r2")]
    #[test]
    fn compiled_manifest_exposes_only_r2_as_durable_storage_backend() {
        let manifest = compiled_capability_manifest().expect("compiled manifest should be valid");
        let storage_backend_ids: Vec<_> = manifest
            .modules()
            .iter()
            .filter(|module| module.kind() == CapabilityKind::StorageBackend)
            .map(|module| module.id().as_str())
            .collect();

        assert_eq!(
            storage_backend_ids,
            ["storage/r2"],
            "S3/R2 must stay the single production durable storage backend"
        );
    }

    #[cfg(feature = "llm-openrouter")]
    #[test]
    fn openrouter_module_declares_provider_config_schema() {
        let manifest = compiled_capability_manifest().expect("compiled manifest should be valid");
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
