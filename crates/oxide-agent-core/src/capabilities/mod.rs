//! Capability module manifest primitives.
//!
//! This module is the first stable surface for the PRD capability architecture:
//! compile-time modules expose deterministic manifests before runtime
//! registration is migrated to the unified registry.

mod compiled;
mod manifest;
mod module;
mod registry;

pub use compiled::{compiled_capability_manifest, compiled_modules};
pub use manifest::{
    CapabilityManifestEntry, CompiledCapabilityManifest, EnabledCapabilityManifest, ManifestError,
    ModuleManifestEntry,
};
pub use module::{
    CapabilityId, CapabilityKind, CapabilityModule, CapabilityRequirement, ModuleConfigProperty,
    ModuleConfigValueKind, ModuleId, StaticCapabilityModule,
};
pub use registry::ModuleRegistry;
