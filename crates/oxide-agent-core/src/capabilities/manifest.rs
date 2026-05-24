//! Deterministic compiled and enabled capability manifests.

use super::{CapabilityId, CapabilityKind, CapabilityModule, CapabilityRequirement, ModuleId};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
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
    /// A compiled module declares a requirement that no compiled capability can satisfy.
    #[error(
        "compiled module {module} has an unsatisfied capability requirement; expected one of {capabilities:?}"
    )]
    UnsatisfiedCompiledCapabilityRequirement {
        /// Module with the unsatisfied requirement.
        module: ModuleId,
        /// Capability IDs that could satisfy the requirement.
        capabilities: Vec<CapabilityId>,
    },
    /// A runtime-enabled module declares a requirement that no enabled capability can satisfy.
    #[error(
        "enabled module {module} has an unsatisfied capability requirement; expected one of {capabilities:?}"
    )]
    UnsatisfiedEnabledCapabilityRequirement {
        /// Module with the unsatisfied requirement.
        module: ModuleId,
        /// Capability IDs that could satisfy the requirement.
        capabilities: Vec<CapabilityId>,
    },
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

        let manifest = Self {
            modules: module_entries,
            capabilities,
        };
        manifest.validate_compiled_requirements()?;
        Ok(manifest)
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

    /// Builds an enabled manifest from runtime module config entries.
    ///
    /// All compiled modules are enabled by default. Config entries may disable
    /// a compiled module with `enabled = false`; entries for non-compiled
    /// modules are hard errors.
    pub fn enabled_manifest_from_configured_modules<I, S>(
        &self,
        module_configs: I,
    ) -> Result<EnabledCapabilityManifest, ManifestError>
    where
        I: IntoIterator<Item = (S, bool)>,
        S: AsRef<str>,
    {
        let mut disabled_modules = BTreeSet::new();

        for (module_id, enabled) in module_configs {
            let module_id = module_id.as_ref();
            if !self.contains_module_id(module_id) {
                return Err(ManifestError::NonCompiledModuleConfig(
                    module_id.to_string(),
                ));
            }
            if !enabled {
                disabled_modules.insert(module_id.to_string());
            }
        }

        let enabled_module_ids = self
            .modules
            .iter()
            .map(ModuleManifestEntry::id)
            .filter(|module_id| !disabled_modules.contains(module_id.as_str()));

        EnabledCapabilityManifest::try_from_compiled_module_ids(self, enabled_module_ids)
    }

    /// Serializes the manifest as pretty JSON for CLI/debug output.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Builds a deterministic JSON Schema for the compiled module config block.
    #[must_use]
    pub fn config_schema(&self) -> Value {
        let mut module_properties = serde_json::Map::new();
        for module in &self.modules {
            module_properties.insert(
                module.id().as_str().to_string(),
                json!({
                    "type": "object",
                    "description": format!(
                        "Runtime config for compiled module {}",
                        module.id().as_str()
                    ),
                    "additionalProperties": true,
                    "properties": {
                        "enabled": {
                            "type": "boolean",
                            "default": true,
                            "description": "Disable this compiled module at runtime when set to false."
                        }
                    },
                    "x-oxide-kind": module.kind(),
                    "x-oxide-cargo-feature": module.cargo_feature(),
                    "x-oxide-provides": module.provides(),
                    "x-oxide-requires": module.requires(),
                    "x-oxide-conflicts": module.conflicts()
                }),
            );
        }

        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "Oxide Agent compiled module configuration",
            "type": "object",
            "additionalProperties": true,
            "properties": {
                "modules": {
                    "type": "object",
                    "description": "Runtime module configuration. Keys must be module IDs compiled into this binary.",
                    "additionalProperties": false,
                    "properties": module_properties
                }
            }
        })
    }

    /// Serializes the compiled module config schema as pretty JSON.
    pub fn config_schema_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.config_schema())
    }

    fn validate_compiled_requirements(&self) -> Result<(), ManifestError> {
        let compiled_capabilities: BTreeSet<CapabilityId> = self
            .capabilities
            .iter()
            .map(CapabilityManifestEntry::id)
            .collect();

        for module in &self.modules {
            for requirement in module.requires() {
                if !requirement_is_satisfied_by(*requirement, &compiled_capabilities) {
                    return Err(ManifestError::UnsatisfiedCompiledCapabilityRequirement {
                        module: module.id(),
                        capabilities: requirement.capability_options(),
                    });
                }
            }
        }

        Ok(())
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
        Self::from_compiled_module_ids(
            compiled,
            compiled.modules.iter().map(ModuleManifestEntry::id),
        )
    }

    /// Creates an enabled manifest from a validated set of compiled module IDs.
    #[must_use]
    pub fn from_compiled_module_ids<I>(compiled: &CompiledCapabilityManifest, module_ids: I) -> Self
    where
        I: IntoIterator<Item = ModuleId>,
    {
        Self::try_from_compiled_module_ids(compiled, module_ids)
            .expect("all compiled module requirements must be satisfied by compiled capabilities")
    }

    /// Creates an enabled manifest from a validated set of compiled module IDs.
    pub fn try_from_compiled_module_ids<I>(
        compiled: &CompiledCapabilityManifest,
        module_ids: I,
    ) -> Result<Self, ManifestError>
    where
        I: IntoIterator<Item = ModuleId>,
    {
        let enabled_modules: BTreeSet<ModuleId> = module_ids.into_iter().collect();

        let modules: Vec<_> = compiled
            .modules
            .iter()
            .map(ModuleManifestEntry::id)
            .filter(|module_id| enabled_modules.contains(module_id))
            .collect();
        let capabilities: Vec<_> = compiled
            .capabilities
            .iter()
            .filter(|entry| enabled_modules.contains(&entry.module()))
            .map(CapabilityManifestEntry::id)
            .collect();
        let enabled_capabilities: BTreeSet<CapabilityId> = capabilities.iter().copied().collect();

        for module in compiled
            .modules
            .iter()
            .filter(|module| enabled_modules.contains(&module.id()))
        {
            for requirement in module.requires() {
                if !requirement_is_satisfied_by(*requirement, &enabled_capabilities) {
                    return Err(ManifestError::UnsatisfiedEnabledCapabilityRequirement {
                        module: module.id(),
                        capabilities: requirement.capability_options(),
                    });
                }
            }
        }

        Ok(Self {
            modules,
            capabilities,
        })
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

fn requirement_is_satisfied_by(
    requirement: CapabilityRequirement,
    capabilities: &BTreeSet<CapabilityId>,
) -> bool {
    match requirement {
        CapabilityRequirement::Capability { capability } => capabilities.contains(&capability),
        CapabilityRequirement::AnyOf {
            capabilities: options,
        } => options
            .iter()
            .any(|capability| capabilities.contains(capability)),
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
    const SANDBOX_BACKEND_A_FILEOPS: &[CapabilityId] =
        &[CapabilityId::new("sandbox-backend/a/fileops")];
    const SANDBOX_FILEOPS_BACKEND_OPTIONS: &[CapabilityId] = &[
        CapabilityId::new("sandbox-backend/a/fileops"),
        CapabilityId::new("sandbox-backend/b/fileops"),
    ];
    const SANDBOX_FILEOPS_REQUIRES: &[CapabilityRequirement] = &[CapabilityRequirement::any_of(
        SANDBOX_FILEOPS_BACKEND_OPTIONS,
    )];

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
    fn config_schema_lists_only_compiled_module_ids() {
        let modules = vec![
            boxed(StaticCapabilityModule::new(
                ModuleId::new("tool/a"),
                CapabilityKind::Tool,
                "tool-a",
                TOOL_A_READ,
            )),
            boxed(StaticCapabilityModule::new(
                ModuleId::new("tool/z"),
                CapabilityKind::Tool,
                "tool-z",
                TOOL_Z_WRITE,
            )),
        ];
        let manifest =
            CompiledCapabilityManifest::from_modules(&modules).expect("manifest should be valid");

        let schema = manifest.config_schema();
        let module_properties = schema["properties"]["modules"]["properties"]
            .as_object()
            .expect("module properties should be an object");

        assert_eq!(
            module_properties.keys().collect::<Vec<_>>(),
            vec!["tool/a", "tool/z"]
        );
        assert_eq!(
            schema["properties"]["modules"]["additionalProperties"],
            false
        );
        assert_eq!(
            module_properties["tool/a"]["properties"]["enabled"]["type"],
            "boolean"
        );
        assert_eq!(
            module_properties["tool/a"]["x-oxide-cargo-feature"],
            "tool-a"
        );
    }

    #[test]
    fn enabled_manifest_uses_all_compiled_minus_disabled_modules() {
        let modules = vec![
            boxed(StaticCapabilityModule::new(
                ModuleId::new("tool/a"),
                CapabilityKind::Tool,
                "tool-a",
                TOOL_A_READ,
            )),
            boxed(StaticCapabilityModule::new(
                ModuleId::new("tool/b"),
                CapabilityKind::Tool,
                "tool-b",
                TOOL_A_WRITE,
            )),
        ];
        let manifest =
            CompiledCapabilityManifest::from_modules(&modules).expect("manifest should be valid");

        let enabled = manifest
            .enabled_manifest_from_configured_modules([("tool/b", false)])
            .expect("disabled compiled module config should validate");

        let module_ids: Vec<_> = enabled
            .modules()
            .iter()
            .map(|module_id| module_id.as_str())
            .collect();
        let capability_ids: Vec<_> = enabled
            .capabilities()
            .iter()
            .map(|capability_id| capability_id.as_str())
            .collect();

        assert_eq!(module_ids, ["tool/a"]);
        assert_eq!(capability_ids, ["tool/a-read"]);
    }

    #[test]
    fn compiled_manifest_requires_declared_capabilities_to_exist() {
        let modules = vec![boxed(
            StaticCapabilityModule::new(
                ModuleId::new("tool/sandbox-fileops"),
                CapabilityKind::SandboxTool,
                "tool-sandbox-fileops",
                TOOL_A_READ,
            )
            .with_requires(SANDBOX_FILEOPS_REQUIRES),
        )];

        let error = CompiledCapabilityManifest::from_modules(&modules)
            .expect_err("missing compiled dependency must fail");

        assert_eq!(
            error,
            ManifestError::UnsatisfiedCompiledCapabilityRequirement {
                module: ModuleId::new("tool/sandbox-fileops"),
                capabilities: SANDBOX_FILEOPS_BACKEND_OPTIONS.to_vec(),
            }
        );
    }

    #[test]
    fn enabled_manifest_requires_declared_capabilities_to_stay_enabled() {
        let modules = vec![
            boxed(
                StaticCapabilityModule::new(
                    ModuleId::new("tool/sandbox-fileops"),
                    CapabilityKind::SandboxTool,
                    "tool-sandbox-fileops",
                    TOOL_A_READ,
                )
                .with_requires(SANDBOX_FILEOPS_REQUIRES),
            ),
            boxed(StaticCapabilityModule::new(
                ModuleId::new("sandbox-backend/a"),
                CapabilityKind::SandboxBackend,
                "sandbox-backend-a",
                SANDBOX_BACKEND_A_FILEOPS,
            )),
        ];
        let manifest =
            CompiledCapabilityManifest::from_modules(&modules).expect("manifest should be valid");

        let error = manifest
            .enabled_manifest_from_configured_modules([("sandbox-backend/a", false)])
            .expect_err("disabled dependency must fail");

        assert_eq!(
            error,
            ManifestError::UnsatisfiedEnabledCapabilityRequirement {
                module: ModuleId::new("tool/sandbox-fileops"),
                capabilities: SANDBOX_FILEOPS_BACKEND_OPTIONS.to_vec(),
            }
        );
    }

    #[cfg(all(
        feature = "tool-sandbox-exec",
        any(
            feature = "sandbox-backend-docker-direct",
            feature = "sandbox-backend-sandboxd-client"
        )
    ))]
    #[test]
    fn compiled_sandbox_exec_declares_exec_backend_requirement() {
        let manifest =
            compiled_capability_manifest().expect("compiled modules must have unique IDs");
        let sandbox_exec = manifest
            .modules()
            .iter()
            .find(|module| module.id().as_str() == "tool/sandbox-exec")
            .expect("sandbox exec module should be compiled");

        let requirement_options: Vec<_> = sandbox_exec
            .requires()
            .iter()
            .flat_map(|requirement| requirement.capability_options())
            .map(|capability| capability.as_str())
            .collect();

        assert_eq!(
            requirement_options,
            [
                "sandbox-backend/docker-direct/exec",
                "sandbox-backend/sandboxd-client/exec"
            ]
        );
    }

    #[cfg(all(
        feature = "tool-ytdlp",
        any(
            feature = "sandbox-backend-docker-direct",
            feature = "sandbox-backend-sandboxd-client"
        )
    ))]
    #[test]
    fn compiled_ytdlp_declares_exec_and_fileops_backend_requirements() {
        let manifest =
            compiled_capability_manifest().expect("compiled modules must have unique IDs");
        let ytdlp = manifest
            .modules()
            .iter()
            .find(|module| module.id().as_str() == "tool/ytdlp")
            .expect("yt-dlp module should be compiled");

        let requirement_options: BTreeSet<_> = ytdlp
            .requires()
            .iter()
            .flat_map(|requirement| requirement.capability_options())
            .map(|capability| capability.as_str())
            .collect();

        assert_eq!(
            requirement_options,
            BTreeSet::from([
                "sandbox-backend/docker-direct/exec",
                "sandbox-backend/docker-direct/fileops",
                "sandbox-backend/sandboxd-client/exec",
                "sandbox-backend/sandboxd-client/fileops",
            ])
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
