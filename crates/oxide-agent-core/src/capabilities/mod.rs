//! Capability module manifest primitives.
//!
//! Compile-time modules expose deterministic manifests before runtime startup.

mod compiled;
mod manifest;
mod module;
mod registry;

pub use compiled::{compiled_capability_manifest, compiled_modules, compiled_profile_name};
pub use manifest::{
    CapabilityManifestEntry, CompiledCapabilityManifest, EnabledCapabilityManifest, ManifestError,
    ModuleManifestEntry,
};
pub use module::{
    CapabilityId, CapabilityKind, CapabilityModule, CapabilityRequirement, ModuleConfigProperty,
    ModuleConfigValueKind, ModuleId, StaticCapabilityModule,
};
pub use registry::ModuleRegistry;
