//! Capability module trait and identifier types.

use serde::Serialize;
use std::fmt;

/// Stable unique identifier for a compiled module.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ModuleId(&'static str);

impl ModuleId {
    /// Creates a module identifier from a stable static string.
    #[must_use]
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    /// Returns the raw identifier string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

/// Stable unique identifier for a behavior exposed by a module.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct CapabilityId(&'static str);

impl CapabilityId {
    /// Creates a capability identifier from a stable static string.
    #[must_use]
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    /// Returns the raw identifier string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

/// High-level category of a capability module.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityKind {
    /// User-facing or agent-facing tool module.
    Tool,
    /// LLM provider module.
    LlmProvider,
    /// Storage backend module.
    StorageBackend,
    /// Sandbox backend module.
    SandboxBackend,
    /// Sandbox tool module.
    SandboxTool,
    /// Transport module.
    Transport,
    /// MCP integration module.
    McpIntegration,
    /// Search integration module.
    Search,
    /// Browser automation module.
    Browser,
    /// Media processing module.
    Media,
    /// Memory or wiki context module.
    Memory,
    /// Reminder or scheduled task module.
    Reminder,
    /// File delivery module.
    FileDelivery,
    /// Diagnostic or operational tool module.
    Diagnostics,
    /// Manager control-plane module.
    Manager,
    /// Sidecar service module.
    Service,
}

/// Required capability edge declared by a module.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CapabilityRequirement {
    /// A hard requirement on one exact capability.
    Capability {
        /// Required capability.
        capability: CapabilityId,
    },
    /// A requirement that can be satisfied by any one of several capabilities.
    AnyOf {
        /// Capabilities that can satisfy the requirement.
        capabilities: &'static [CapabilityId],
    },
}

impl CapabilityRequirement {
    /// Creates a hard requirement on a single capability.
    #[must_use]
    pub const fn new(capability: CapabilityId) -> Self {
        Self::Capability { capability }
    }

    /// Creates a requirement that can be satisfied by any listed capability.
    #[must_use]
    pub const fn any_of(capabilities: &'static [CapabilityId]) -> Self {
        Self::AnyOf { capabilities }
    }

    /// Returns the required capabilities in deterministic order.
    #[must_use]
    pub fn capability_options(self) -> Vec<CapabilityId> {
        match self {
            Self::Capability { capability } => vec![capability],
            Self::AnyOf { capabilities } => capabilities.to_vec(),
        }
    }
}

/// JSON-compatible scalar type for a module-local config property.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModuleConfigValueKind {
    /// UTF-8 string config value.
    String,
}

impl ModuleConfigValueKind {
    /// Returns the JSON Schema type name.
    #[must_use]
    pub const fn json_schema_type(self) -> &'static str {
        match self {
            Self::String => "string",
        }
    }
}

/// Module-owned config property exposed in generated config schemas.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ModuleConfigProperty {
    name: &'static str,
    value_kind: ModuleConfigValueKind,
    description: &'static str,
    env: Option<&'static str>,
    secret: bool,
    default_value: Option<&'static str>,
}

impl ModuleConfigProperty {
    /// Creates a string property descriptor.
    #[must_use]
    pub const fn string(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            value_kind: ModuleConfigValueKind::String,
            description,
            env: None,
            secret: false,
            default_value: None,
        }
    }

    /// Attaches the provider-owned environment variable fallback.
    #[must_use]
    pub const fn with_env(mut self, env: &'static str) -> Self {
        self.env = Some(env);
        self
    }

    /// Marks the property as secret-bearing.
    #[must_use]
    pub const fn secret(mut self) -> Self {
        self.secret = true;
        self
    }

    /// Attaches the default value used when module config and env are both absent.
    #[must_use]
    pub const fn with_default(mut self, default_value: &'static str) -> Self {
        self.default_value = Some(default_value);
        self
    }

    /// Property name inside the module config object.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// JSON-compatible scalar type.
    #[must_use]
    pub const fn value_kind(self) -> ModuleConfigValueKind {
        self.value_kind
    }

    /// Human-readable property description.
    #[must_use]
    pub const fn description(self) -> &'static str {
        self.description
    }

    /// Provider-owned environment variable fallback, if any.
    #[must_use]
    pub const fn env(self) -> Option<&'static str> {
        self.env
    }

    /// Whether this property carries a secret value.
    #[must_use]
    pub const fn is_secret(self) -> bool {
        self.secret
    }

    /// Default value used when module config and env are both absent.
    #[must_use]
    pub const fn default_value(self) -> Option<&'static str> {
        self.default_value
    }
}

/// Compile-time module descriptor used to build manifests and registries.
pub trait CapabilityModule: Send + Sync {
    /// Stable module identifier.
    fn id(&self) -> ModuleId;

    /// Module category.
    fn kind(&self) -> CapabilityKind;

    /// Atomic Cargo feature that compiles this module.
    fn cargo_feature(&self) -> &'static str;

    /// Capabilities exported by this module.
    fn provides(&self) -> &'static [CapabilityId];

    /// Capabilities required by this module.
    fn requires(&self) -> &'static [CapabilityRequirement] {
        &[]
    }

    /// Capabilities that conflict with this module.
    fn conflicts(&self) -> &'static [CapabilityId] {
        &[]
    }

    /// Module-owned runtime config properties for generated schemas.
    fn config_properties(&self) -> &'static [ModuleConfigProperty] {
        &[]
    }
}

/// Static module descriptor for modules that do not need custom behavior yet.
#[derive(Clone, Copy, Debug)]
pub struct StaticCapabilityModule {
    id: ModuleId,
    kind: CapabilityKind,
    cargo_feature: &'static str,
    provides: &'static [CapabilityId],
    requires: &'static [CapabilityRequirement],
    conflicts: &'static [CapabilityId],
    config_properties: &'static [ModuleConfigProperty],
}

impl StaticCapabilityModule {
    /// Creates a static module descriptor.
    #[must_use]
    pub const fn new(
        id: ModuleId,
        kind: CapabilityKind,
        cargo_feature: &'static str,
        provides: &'static [CapabilityId],
    ) -> Self {
        Self {
            id,
            kind,
            cargo_feature,
            provides,
            requires: &[],
            conflicts: &[],
            config_properties: &[],
        }
    }

    /// Adds hard capability requirements to the descriptor.
    #[must_use]
    pub const fn with_requires(mut self, requires: &'static [CapabilityRequirement]) -> Self {
        self.requires = requires;
        self
    }

    /// Adds capability conflicts to the descriptor.
    #[must_use]
    pub const fn with_conflicts(mut self, conflicts: &'static [CapabilityId]) -> Self {
        self.conflicts = conflicts;
        self
    }

    /// Adds module-owned config properties to the descriptor.
    #[must_use]
    pub const fn with_config_properties(
        mut self,
        config_properties: &'static [ModuleConfigProperty],
    ) -> Self {
        self.config_properties = config_properties;
        self
    }
}

impl CapabilityModule for StaticCapabilityModule {
    fn id(&self) -> ModuleId {
        self.id
    }

    fn kind(&self) -> CapabilityKind {
        self.kind
    }

    fn cargo_feature(&self) -> &'static str {
        self.cargo_feature
    }

    fn provides(&self) -> &'static [CapabilityId] {
        self.provides
    }

    fn requires(&self) -> &'static [CapabilityRequirement] {
        self.requires
    }

    fn conflicts(&self) -> &'static [CapabilityId] {
        self.conflicts
    }

    fn config_properties(&self) -> &'static [ModuleConfigProperty] {
        self.config_properties
    }
}
