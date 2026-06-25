//! Build script that emits `oxide_module_*` cfg aliases from the module registry.
//!
//! Each module in `module_registry.toml` gets a cfg flag `oxide_module_<id>`
//! (with `/` and `-` replaced by `_`) that is set when the module's
//! `cargo_feature` is activated. This lets tests express requirements in
//! domain terms (`#[cfg(oxide_module_tool_todos)]`) instead of raw Cargo
//! feature names, and enables `unexpected_cfgs` drift detection when a
//! module is renamed in the registry without updating test gates.
//!
//! The parser is intentionally minimal: it only extracts `id` and
//! `cargo_feature` string fields from `[[modules]]` entries. The full
//! registry is validated by `xtask module-registry check`.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by Cargo"),
    );
    let registry_path = manifest_dir.join("module_registry.toml");

    println!("cargo:rerun-if-changed=module_registry.toml");

    let content = fs::read_to_string(&registry_path)
        .unwrap_or_else(|e| panic!("failed to read module_registry.toml: {e}"));

    for (module_id, cargo_feature) in parse_module_entries(&content) {
        let cfg_name = cfg_name_for_module(&module_id);
        let env_var = env_var_for_feature(&cargo_feature);

        // Declare expected cfg so `unexpected_cfgs` lint knows about it.
        println!("cargo:rustc-check-cfg=cfg({cfg_name})");

        // Emit cfg when the Cargo feature is activated (including transitively
        // through profile features).
        if env::var_os(&env_var).is_some() {
            println!("cargo:rustc-cfg={cfg_name}");
        }
    }
}

/// Extract `(id, cargo_feature)` pairs from `[[modules]]` entries.
///
/// Only parses simple `key = "value"` string fields. Multi-line arrays
/// (`provides`, `requires`, `profiles`) are skipped — they are not needed
/// for cfg emission and are validated by `xtask module-registry check`.
fn parse_module_entries(content: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    let mut current_id: Option<String> = None;
    let mut current_feature: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "[[modules]]" {
            if let (Some(id), Some(feature)) = (current_id.take(), current_feature.take()) {
                entries.push((id, feature));
            }
        } else if let Some(rest) = trimmed.strip_prefix("id = ") {
            current_id = Some(parse_string_value(rest));
        } else if let Some(rest) = trimmed.strip_prefix("cargo_feature = ") {
            current_feature = Some(parse_string_value(rest));
        }
    }
    if let (Some(id), Some(feature)) = (current_id, current_feature) {
        entries.push((id, feature));
    }

    entries
}

/// Extract a quoted string value from a `key = "value"` line.
fn parse_string_value(raw: &str) -> String {
    let trimmed = raw.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed)
        .to_string()
}

/// Normalize a module ID into a cfg flag name: `tool/todos` → `oxide_module_tool_todos`.
fn cfg_name_for_module(module_id: &str) -> String {
    let normalized = module_id.replace(['/', '-'], "_");
    format!("oxide_module_{normalized}")
}

/// Convert a Cargo feature name to the env var Cargo sets: `tool-todos` → `CARGO_FEATURE_TOOL_TODOS`.
fn env_var_for_feature(cargo_feature: &str) -> String {
    format!(
        "CARGO_FEATURE_{}",
        cargo_feature.replace('-', "_").to_uppercase()
    )
}
