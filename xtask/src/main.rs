use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

const REGISTRY_PATH: &str = "crates/oxide-agent-core/module_registry.toml";
const CORE_CARGO_PATH: &str = "crates/oxide-agent-core/Cargo.toml";
const COMPILED_RS_PATH: &str = "crates/oxide-agent-core/src/capabilities/compiled.rs";

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    match (args.next().as_deref(), args.next().as_deref(), args.next()) {
        (Some("module-registry"), Some("check"), None) => module_registry_check(),
        _ => Err("usage: cargo run -p xtask -- module-registry check".to_string()),
    }
}

fn module_registry_check() -> Result<(), String> {
    let root = workspace_root()?;
    let registry = parse_registry(&read_to_string(&root, REGISTRY_PATH)?)?;
    let cargo_features = parse_cargo_features(&read_to_string(&root, CORE_CARGO_PATH)?)?;
    let compiled_modules = parse_compiled_modules(&read_to_string(&root, COMPILED_RS_PATH)?)?;

    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    check_duplicate_registry_ids(&registry, &mut errors);
    check_registry_features_exist(&registry, &cargo_features, &mut errors);
    check_compiled_modules(&registry, &compiled_modules, &mut errors);
    check_profiles(&root, &registry, &mut errors, &mut warnings)?;

    for warning in &warnings {
        println!("warning: {warning}");
    }

    if errors.is_empty() {
        println!(
            "module registry check passed: {} modules, {} Cargo features, {} compiled declarations",
            registry.modules.len(),
            cargo_features.len(),
            compiled_modules.len()
        );
        return Ok(());
    }

    for error in errors {
        eprintln!("registry drift: {error}");
    }
    Err("module registry check failed".to_string())
}

fn workspace_root() -> Result<PathBuf, String> {
    env::current_dir().map_err(|err| format!("read current directory: {err}"))
}

fn read_to_string(root: &Path, relative: &str) -> Result<String, String> {
    let path = root.join(relative);
    fs::read_to_string(&path).map_err(|err| format!("read {}: {err}", path.display()))
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct ModuleKey {
    cargo_feature: String,
    id: String,
    kind: String,
}

#[derive(Debug)]
struct RegistryModule {
    key: ModuleKey,
    profile_id: String,
    profiles: BTreeSet<String>,
}

#[derive(Debug, Default)]
struct Registry {
    modules: Vec<RegistryModule>,
}

fn parse_registry(input: &str) -> Result<Registry, String> {
    let mut registry = Registry::default();
    let mut current: Option<RegistryModule> = None;

    for raw_line in input.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if line == "[[modules]]" {
            if let Some(module) = current.take() {
                registry.modules.push(module);
            }
            current = Some(RegistryModule {
                key: ModuleKey {
                    cargo_feature: String::new(),
                    id: String::new(),
                    kind: String::new(),
                },
                profile_id: String::new(),
                profiles: BTreeSet::new(),
            });
            continue;
        }
        let Some(module) = current.as_mut() else {
            continue;
        };
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        match name.trim() {
            "id" => module.key.id = parse_string(value)?,
            "profile_id" => module.profile_id = parse_string(value)?,
            "cargo_feature" => module.key.cargo_feature = parse_string(value)?,
            "kind" => module.key.kind = parse_string(value)?,
            "profiles" => module.profiles = parse_string_array(value)?.into_iter().collect(),
            _ => {}
        }
    }

    if let Some(module) = current.take() {
        registry.modules.push(module);
    }

    for module in &registry.modules {
        if module.key.id.is_empty()
            || module.key.cargo_feature.is_empty()
            || module.key.kind.is_empty()
        {
            return Err(format!("incomplete module declaration: {module:?}"));
        }
    }

    Ok(registry)
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(idx) => &line[..idx],
        None => line,
    }
}

fn parse_string(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("expected quoted string, got `{trimmed}`"))
}

fn parse_string_array(value: &str) -> Result<Vec<String>, String> {
    let trimmed = value.trim();
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(|| format!("expected string array, got `{trimmed}`"))?;
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    inner.split(',').map(parse_string).collect()
}

fn parse_cargo_features(input: &str) -> Result<BTreeSet<String>, String> {
    let mut features = BTreeSet::new();
    let mut in_features = false;

    for raw_line in input.lines() {
        let line = strip_comment(raw_line).trim();
        if line == "[features]" {
            in_features = true;
            continue;
        }
        if in_features && line.starts_with('[') {
            break;
        }
        if !in_features || line.is_empty() || line.starts_with('"') || line.starts_with(']') {
            continue;
        }
        if let Some((name, _)) = line.split_once('=') {
            features.insert(name.trim().to_string());
        }
    }

    if features.is_empty() {
        return Err("no Cargo features parsed from core Cargo.toml".to_string());
    }
    Ok(features)
}

fn parse_compiled_modules(input: &str) -> Result<BTreeSet<ModuleKey>, String> {
    let mut modules = BTreeSet::new();
    let mut cursor = input
        .find("fn push_transport_and_storage_modules")
        .unwrap_or(0);

    while let Some(relative) = input[cursor..].find("push_module") {
        let start = cursor + relative;
        let Some(paren_relative) = input[start..].find('(') else {
            break;
        };
        let open = start + paren_relative;
        let Some(close) = find_matching_paren(input, open) else {
            return Err("unterminated push_module call in compiled.rs".to_string());
        };
        let args = &input[open + 1..close];
        let strings = quoted_strings(args);
        if strings.len() >= 2 {
            let kind = parse_kind_arg(args).ok_or_else(|| {
                format!("could not parse module kind near compiled.rs byte offset {start}")
            })?;
            modules.insert(ModuleKey {
                cargo_feature: strings[0].clone(),
                id: strings[1].clone(),
                kind,
            });
        }
        cursor = close + 1;
    }

    if modules.is_empty() {
        return Err("no compiled module declarations parsed".to_string());
    }
    Ok(modules)
}

fn find_matching_paren(input: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut previous_escape = false;

    for (idx, ch) in input[open..].char_indices() {
        if in_string {
            if ch == '"' && !previous_escape {
                in_string = false;
            }
            previous_escape = ch == '\\' && !previous_escape;
            continue;
        }
        match ch {
            '"' => in_string = true,
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(open + idx);
                }
            }
            _ => {}
        }
        previous_escape = false;
    }
    None
}

fn quoted_strings(input: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut previous_escape = false;

    for ch in input.chars() {
        if in_string {
            if ch == '"' && !previous_escape {
                values.push(current.clone());
                current.clear();
                in_string = false;
            } else {
                current.push(ch);
            }
            previous_escape = ch == '\\' && !previous_escape;
        } else if ch == '"' {
            in_string = true;
            previous_escape = false;
        }
    }

    values
}

fn parse_kind_arg(args: &str) -> Option<String> {
    let mut parts = args.split(',').map(str::trim);
    parts.next()?;
    parts.next()?;
    parts.next()?;
    let kind = parts.next()?;
    Some(
        kind.strip_prefix("ModuleKind::")
            .unwrap_or(kind)
            .to_string(),
    )
}

fn check_duplicate_registry_ids(registry: &Registry, errors: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    for module in &registry.modules {
        if !seen.insert(module.key.id.clone()) {
            errors.push(format!("duplicate registry module id `{}`", module.key.id));
        }
    }
}

fn check_registry_features_exist(
    registry: &Registry,
    cargo_features: &BTreeSet<String>,
    errors: &mut Vec<String>,
) {
    for module in &registry.modules {
        if !cargo_features.contains(&module.key.cargo_feature) {
            errors.push(format!(
                "registry module `{}` references missing Cargo feature `{}`",
                module.key.id, module.key.cargo_feature
            ));
        }
    }
}

fn check_compiled_modules(
    registry: &Registry,
    compiled_modules: &BTreeSet<ModuleKey>,
    errors: &mut Vec<String>,
) {
    let registry_modules = registry
        .modules
        .iter()
        .map(|module| module.key.clone())
        .collect::<BTreeSet<_>>();

    for missing in registry_modules.difference(compiled_modules) {
        errors.push(format!(
            "compiled.rs is missing registry module {missing:?}"
        ));
    }
    for extra in compiled_modules.difference(&registry_modules) {
        errors.push(format!(
            "compiled.rs has undeclared registry module {extra:?}"
        ));
    }
}

fn check_profiles(
    root: &Path,
    registry: &Registry,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let mut expected_by_profile: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for module in &registry.modules {
        for profile in &module.profiles {
            expected_by_profile
                .entry(profile.clone())
                .or_default()
                .insert(module.profile_module_id().to_string());
        }
    }

    for (profile, expected) in expected_by_profile {
        let path = root.join("profiles").join(format!("{profile}.toml"));
        let actual = parse_profile_modules(
            &fs::read_to_string(&path)
                .map_err(|err| format!("read runtime profile {}: {err}", path.display()))?,
        )?;

        for missing in expected.difference(&actual) {
            if is_known_browser_live_profile_drift(&profile, missing) {
                warnings.push(format!(
                    "known Browser Live runtime-profile drift: `{missing}` missing from profiles/{profile}.toml"
                ));
            } else {
                errors.push(format!(
                    "profiles/{profile}.toml is missing registry module `{missing}`"
                ));
            }
        }
        for extra in actual.difference(&expected) {
            errors.push(format!(
                "profiles/{profile}.toml has module `{extra}` not enabled by registry"
            ));
        }
    }

    Ok(())
}

impl RegistryModule {
    fn profile_module_id(&self) -> &str {
        if self.profile_id.is_empty() {
            &self.key.id
        } else {
            &self.profile_id
        }
    }
}

fn parse_profile_modules(input: &str) -> Result<BTreeSet<String>, String> {
    let mut modules = BTreeSet::new();
    for raw_line in input.lines() {
        let line = strip_comment(raw_line).trim();
        if !line.starts_with('"') {
            continue;
        }
        let Some(end) = line[1..].find('"') else {
            return Err(format!("invalid quoted module line `{line}`"));
        };
        modules.insert(line[1..=end].to_string());
    }
    Ok(modules)
}

fn is_known_browser_live_profile_drift(profile: &str, module_id: &str) -> bool {
    module_id == "tool/browser-live" && matches!(profile, "full" | "web-embedded-opencode-local")
}
