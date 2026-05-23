//! Deterministic compiled and enabled capability manifests.

use super::{CapabilityId, CapabilityKind, CapabilityModule, CapabilityRequirement, ModuleId};
use serde::Serialize;
use std::collections::BTreeMap;
use thiserror::Error;

/// Error returned while constructing a capability manifest.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum ManifestError {
    /// More than one module declared the same module ID.
    #[error("duplicate module id: {0}")]
    DuplicateModuleId(ModuleId),
    /// More than one module declared the same capability ID.
    #[error(
        "duplicate capability id: {capability} provided by {first_module} and {second_module}"
    )]
    DuplicateCapabilityId {
        /// Duplicate capability.
        capability: CapabilityId,
        /// First module that provided the capability.
        first_module: ModuleId,
        /// Second module that provided the capability.
        second_module: ModuleId,
    },
    /// Runtime configuration referenced a module that is not compiled into this binary.
    #[error("module config references a non-compiled or unknown module id: {0}")]
    NonCompiledModuleConfig(String),
}

/// Manifest entry for one compiled module.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModuleManifestEntry {
    id: ModuleId,
    kind: CapabilityKind,
    cargo_feature: &'static str,
    provides: Vec<CapabilityId>,
    requires: Vec<CapabilityRequirement>,
    conflicts: Vec<CapabilityId>,
}

impl ModuleManifestEntry {
    /// Stable module ID.
    #[must_use]
    pub const fn id(&self) -> ModuleId {
        self.id
    }

    /// Module kind.
    #[must_use]
    pub const fn kind(&self) -> CapabilityKind {
        self.kind
    }

    /// Cargo feature that compiled this module.
    #[must_use]
    pub const fn cargo_feature(&self) -> &'static str {
        self.cargo_feature
    }

    /// Capabilities exported by this module.
    #[must_use]
    pub fn provides(&self) -> &[CapabilityId] {
        &self.provides
    }

    /// Capabilities required by this module.
    #[must_use]
    pub fn requires(&self) -> &[CapabilityRequirement] {
        &self.requires
    }

    /// Capabilities that conflict with this module.
    #[must_use]
    pub fn conflicts(&self) -> &[CapabilityId] {
        &self.conflicts
    }

    fn from_module(module: &dyn CapabilityModule) -> Self {
        let mut provides = module.provides().to_vec();
        provides.sort_unstable();
        provides.dedup();

        let mut requires = module.requires().to_vec();
        requires.sort_unstable();
        requires.dedup();

        let mut conflicts = module.conflicts().to_vec();
        conflicts.sort_unstable();
        conflicts.dedup();

        Self {
            id: module.id(),
            kind: module.kind(),
            cargo_feature: module.cargo_feature(),
            provides,
            requires,
            conflicts,
        }
    }
}

/// Manifest entry for one compiled capability.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CapabilityManifestEntry {
    id: CapabilityId,
    module: ModuleId,
}

impl CapabilityManifestEntry {
    /// Stable capability ID.
    #[must_use]
    pub const fn id(&self) -> CapabilityId {
        self.id
    }

    /// Module that provides the capability.
    #[must_use]
    pub const fn module(&self) -> ModuleId {
        self.module
    }
}

/// Deterministic manifest for all modules compiled into the binary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CompiledCapabilityManifest {
    modules: Vec<ModuleManifestEntry>,
    capabilities: Vec<CapabilityManifestEntry>,
}

impl CompiledCapabilityManifest {
    /// Builds and validates a deterministic compiled manifest.
    pub fn from_modules(modules: &[Box<dyn CapabilityModule>]) -> Result<Self, ManifestError> {
        let mut module_entries = Vec::with_capacity(modules.len());
        let mut seen_modules = BTreeMap::new();
        let mut seen_capabilities: BTreeMap<CapabilityId, ModuleId> = BTreeMap::new();

        for module in modules {
            let module_id = module.id();
            if seen_modules.insert(module_id, ()).is_some() {
                return Err(ManifestError::DuplicateModuleId(module_id));
            }

            for capability in module.provides() {
                if let Some(first_module) = seen_capabilities.insert(*capability, module_id) {
                    return Err(ManifestError::DuplicateCapabilityId {
                        capability: *capability,
                        first_module,
                        second_module: module_id,
                    });
                }
            }

            module_entries.push(ModuleManifestEntry::from_module(module.as_ref()));
        }

        module_entries.sort_by_key(ModuleManifestEntry::id);

        let capabilities = seen_capabilities
            .into_iter()
            .map(|(id, module)| CapabilityManifestEntry { id, module })
            .collect();

        Ok(Self {
            modules: module_entries,
            capabilities,
        })
    }

    /// Compiled module entries sorted by module ID.
    #[must_use]
    pub fn modules(&self) -> &[ModuleManifestEntry] {
        &self.modules
    }

    /// Compiled capability entries sorted by capability ID.
    #[must_use]
    pub fn capabilities(&self) -> &[CapabilityManifestEntry] {
        &self.capabilities
    }

    /// Returns whether the module ID is compiled into this manifest.
    #[must_use]
    pub fn contains_module_id(&self, module_id: &str) -> bool {
        self.modules
            .iter()
            .any(|entry| entry.id().as_str() == module_id)
    }

    /// Validates runtime module config keys against the compiled manifest.
    ///
    /// This is the reusable primitive that the later config resolver will call
    /// after parsing a `modules:` section. It intentionally does not select or
    /// enable modules; it only rejects config for modules absent from this
    /// binary.
    pub fn validate_configured_module_ids<I, S>(&self, module_ids: I) -> Result<(), ManifestError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for module_id in module_ids {
            let module_id = module_id.as_ref();
            if !self.contains_module_id(module_id) {
                return Err(ManifestError::NonCompiledModuleConfig(
                    module_id.to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Serializes the manifest as pretty JSON for CLI/debug output.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// Deterministic manifest for modules enabled by runtime configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EnabledCapabilityManifest {
    modules: Vec<ModuleId>,
    capabilities: Vec<CapabilityId>,
}

impl EnabledCapabilityManifest {
    /// Creates an enabled manifest where every compiled module is enabled.
    #[must_use]
    pub fn all_compiled(compiled: &CompiledCapabilityManifest) -> Self {
        Self {
            modules: compiled
                .modules
                .iter()
                .map(ModuleManifestEntry::id)
                .collect(),
            capabilities: compiled
                .capabilities
                .iter()
                .map(CapabilityManifestEntry::id)
                .collect(),
        }
    }

    /// Enabled module IDs sorted by module ID.
    #[must_use]
    pub fn modules(&self) -> &[ModuleId] {
        &self.modules
    }

    /// Enabled capability IDs sorted by capability ID.
    #[must_use]
    pub fn capabilities(&self) -> &[CapabilityId] {
        &self.capabilities
    }

    /// Serializes the manifest as pretty JSON for CLI/debug output.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::{compiled_capability_manifest, StaticCapabilityModule};

    const TOOL_Z_WRITE: &[CapabilityId] = &[CapabilityId::new("tool/z-write")];
    const TOOL_A_WRITE_READ: &[CapabilityId] = &[
        CapabilityId::new("tool/a-write"),
        CapabilityId::new("tool/a-read"),
    ];
    const TOOL_A_READ: &[CapabilityId] = &[CapabilityId::new("tool/a-read")];
    const TOOL_A_WRITE: &[CapabilityId] = &[CapabilityId::new("tool/a-write")];
    const TOOL_SHARED: &[CapabilityId] = &[CapabilityId::new("tool/shared")];

    fn boxed(module: StaticCapabilityModule) -> Box<dyn CapabilityModule> {
        Box::new(module)
    }

    #[test]
    fn manifest_orders_modules_and_capabilities_deterministically() {
        let modules = vec![
            boxed(StaticCapabilityModule::new(
                ModuleId::new("tool/z"),
                CapabilityKind::Tool,
                "tool-z",
                TOOL_Z_WRITE,
            )),
            boxed(StaticCapabilityModule::new(
                ModuleId::new("tool/a"),
                CapabilityKind::Tool,
                "tool-a",
                TOOL_A_WRITE_READ,
            )),
        ];

        let manifest =
            CompiledCapabilityManifest::from_modules(&modules).expect("manifest should be valid");

        let module_ids: Vec<_> = manifest
            .modules()
            .iter()
            .map(|entry| entry.id().as_str())
            .collect();
        let capability_ids: Vec<_> = manifest
            .capabilities()
            .iter()
            .map(|entry| entry.id().as_str())
            .collect();

        assert_eq!(module_ids, ["tool/a", "tool/z"]);
        assert_eq!(
            capability_ids,
            ["tool/a-read", "tool/a-write", "tool/z-write"]
        );
    }

    #[test]
    fn duplicate_module_ids_fail() {
        let modules = vec![
            boxed(StaticCapabilityModule::new(
                ModuleId::new("tool/a"),
                CapabilityKind::Tool,
                "tool-a",
                TOOL_A_READ,
            )),
            boxed(StaticCapabilityModule::new(
                ModuleId::new("tool/a"),
                CapabilityKind::Tool,
                "tool-a-copy",
                TOOL_A_WRITE,
            )),
        ];

        let error = CompiledCapabilityManifest::from_modules(&modules)
            .expect_err("duplicate module ids must fail");

        assert_eq!(
            error,
            ManifestError::DuplicateModuleId(ModuleId::new("tool/a"))
        );
    }

    #[test]
    fn duplicate_capability_ids_fail() {
        let modules = vec![
            boxed(StaticCapabilityModule::new(
                ModuleId::new("tool/a"),
                CapabilityKind::Tool,
                "tool-a",
                TOOL_SHARED,
            )),
            boxed(StaticCapabilityModule::new(
                ModuleId::new("tool/b"),
                CapabilityKind::Tool,
                "tool-b",
                TOOL_SHARED,
            )),
        ];

        let error = CompiledCapabilityManifest::from_modules(&modules)
            .expect_err("duplicate capability ids must fail");

        assert_eq!(
            error,
            ManifestError::DuplicateCapabilityId {
                capability: CapabilityId::new("tool/shared"),
                first_module: ModuleId::new("tool/a"),
                second_module: ModuleId::new("tool/b"),
            }
        );
    }

    #[test]
    fn configured_module_ids_must_be_compiled() {
        let modules = vec![boxed(StaticCapabilityModule::new(
            ModuleId::new("tool/a"),
            CapabilityKind::Tool,
            "tool-a",
            TOOL_A_READ,
        ))];
        let manifest =
            CompiledCapabilityManifest::from_modules(&modules).expect("manifest should be valid");

        manifest
            .validate_configured_module_ids(["tool/a"])
            .expect("compiled module config should validate");

        let error = manifest
            .validate_configured_module_ids(["tool/missing"])
            .expect_err("non-compiled module config should fail");

        assert_eq!(
            error,
            ManifestError::NonCompiledModuleConfig("tool/missing".to_string())
        );
    }

    #[test]
    fn compiled_manifest_is_valid_for_selected_features() {
        let manifest =
            compiled_capability_manifest().expect("compiled modules must have unique IDs");

        let json = manifest
            .to_json_pretty()
            .expect("compiled manifest should serialize");

        assert!(json.contains("\"modules\""));
        assert!(json.contains("\"capabilities\""));
    }
}
