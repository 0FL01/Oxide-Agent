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
pub struct CapabilityRequirement {
    capability: CapabilityId,
}

impl CapabilityRequirement {
    /// Creates a hard requirement on a single capability.
    #[must_use]
    pub const fn new(capability: CapabilityId) -> Self {
        Self { capability }
    }

    /// Returns the required capability.
    #[must_use]
    pub const fn capability(self) -> CapabilityId {
        self.capability
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
}
