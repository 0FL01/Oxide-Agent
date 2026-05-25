//! Minimal capability registry scaffold.

use super::{CompiledCapabilityManifest, EnabledCapabilityManifest};

/// Registry shell that keeps compiled and enabled capability manifests together.
///
/// Runtime registries for tools, LLM providers, storage, sandbox, and transports
/// will move behind this type in later checkpoints.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleRegistry {
    compiled: CompiledCapabilityManifest,
    enabled: EnabledCapabilityManifest,
}

impl ModuleRegistry {
    /// Creates a registry scaffold from compiled and enabled manifests.
    #[must_use]
    pub const fn new(
        compiled: CompiledCapabilityManifest,
        enabled: EnabledCapabilityManifest,
    ) -> Self {
        Self { compiled, enabled }
    }

    /// Creates a registry where every compiled module is enabled.
    #[must_use]
    pub fn all_compiled_enabled(compiled: CompiledCapabilityManifest) -> Self {
        let enabled = EnabledCapabilityManifest::all_compiled(&compiled);
        Self { compiled, enabled }
    }

    /// Compiled capability manifest.
    #[must_use]
    pub const fn compiled(&self) -> &CompiledCapabilityManifest {
        &self.compiled
    }

    /// Enabled capability manifest.
    #[must_use]
    pub const fn enabled(&self) -> &EnabledCapabilityManifest {
        &self.enabled
    }
}
