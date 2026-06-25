use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

const REGISTRY_PATH: &str = "crates/oxide-agent-core/module_registry.toml";
const CORE_CARGO_PATH: &str = "crates/oxide-agent-core/Cargo.toml";
const COMPILED_RS_PATH: &str = "crates/oxide-agent-core/src/capabilities/compiled.rs";

const PROFILE_ORDER: &[&str] = &[
    "full",
    "embedded-opencode-local",
    "web-embedded-opencode-local",
    "search-only",
];

const FORWARDING_CRATES: &[(&str, &str)] = &[
    (
        "crates/oxide-agent-transport-telegram/Cargo.toml",
        "transport/telegram",
    ),
    (
        "crates/oxide-agent-transport-web/Cargo.toml",
        "transport/web",
    ),
    (
        "crates/oxide-agent-telegram-bot/Cargo.toml",
        "transport/telegram",
    ),
];

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
        (Some("module-registry"), Some("generate"), None) => module_registry_generate(),
        _ => Err("usage: cargo run -p xtask -- module-registry <check|generate>".to_string()),
    }
}

// ---------------------------------------------------------------------------
// check
// ---------------------------------------------------------------------------

fn module_registry_check() -> Result<(), String> {
    let root = workspace_root()?;
    let registry = parse_registry(&read_to_string(&root, REGISTRY_PATH)?)?;
    let cargo_features = parse_cargo_feature_names(&read_to_string(&root, CORE_CARGO_PATH)?)?;
    let compiled_modules = parse_compiled_modules(&read_to_string(&root, COMPILED_RS_PATH)?)?;

    let mut errors = Vec::new();

    check_duplicate_registry_ids(&registry, &mut errors);
    check_registry_features_exist(&registry, &cargo_features, &mut errors);
    check_compiled_modules(&registry, &compiled_modules, &mut errors);
    check_profile_coverage(&registry, &mut errors);
    check_core_profile_section(&root, &registry, &mut errors)?;
    check_forwarding(&root, &registry, &mut errors)?;
    check_profile_tomls(&root, &registry, &mut errors)?;

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

// ---------------------------------------------------------------------------
// generate
// ---------------------------------------------------------------------------

fn module_registry_generate() -> Result<(), String> {
    let root = workspace_root()?;
    let registry = parse_registry(&read_to_string(&root, REGISTRY_PATH)?)?;

    let compositions = compute_profile_compositions(&registry);
    let body = render_profile_section(&compositions);

    let content = read_to_string(&root, CORE_CARGO_PATH)?;
    let updated = replace_marked_section(&content, "profiles", &format!("{body}\n"))?;

    fs::write(root.join(CORE_CARGO_PATH), updated)
        .map_err(|err| format!("write {}: {err}", CORE_CARGO_PATH))?;

    generate_profile_tomls(&root, &registry)?;

    println!(
        "generated profile section for {} profiles in {} and {} profile TOMLs in profiles/",
        compositions.len(),
        CORE_CARGO_PATH,
        PROFILE_ORDER.len()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

fn workspace_root() -> Result<PathBuf, String> {
    env::current_dir().map_err(|err| format!("read current directory: {err}"))
}

fn read_to_string(root: &Path, relative: &str) -> Result<String, String> {
    let path = root.join(relative);
    fs::read_to_string(&path).map_err(|err| format!("read {}: {err}", path.display()))
}

// ---------------------------------------------------------------------------
// registry model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Default)]
struct ModuleKey {
    cargo_feature: String,
    id: String,
    kind: String,
}

#[derive(Debug, Default)]
struct RegistryModule {
    key: ModuleKey,
    profile_id: String,
    profiles: BTreeSet<String>,
    provides: Vec<String>,
    requires: Vec<String>,
}

#[derive(Debug, Clone)]
struct CompiledModule {
    key: ModuleKey,
    provides: Vec<String>,
    has_requires: bool,
}

#[derive(Debug, Default)]
struct Registry {
    modules: Vec<RegistryModule>,
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

// ---------------------------------------------------------------------------
// registry parsing
// ---------------------------------------------------------------------------

fn parse_registry(input: &str) -> Result<Registry, String> {
    let mut registry = Registry::default();
    let mut current: Option<RegistryModule> = None;
    let lines: Vec<&str> = input.lines().collect();
    let mut idx = 0;

    while idx < lines.len() {
        let line = strip_comment(lines[idx]).trim();
        idx += 1;

        if line.is_empty() {
            continue;
        }
        if line == "[[modules]]" {
            if let Some(module) = current.take() {
                registry.modules.push(module);
            }
            current = Some(RegistryModule::default());
            continue;
        }
        let Some(module) = current.as_mut() else {
            continue;
        };
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };

        let name = name.trim();
        let mut full_value = value.trim().to_string();

        while !brackets_balanced(&full_value) && idx < lines.len() {
            full_value.push('\n');
            full_value.push_str(strip_comment(lines[idx]).trim());
            idx += 1;
        }

        match name {
            "id" => module.key.id = parse_string(&full_value)?,
            "profile_id" => module.profile_id = parse_string(&full_value)?,
            "cargo_feature" => module.key.cargo_feature = parse_string(&full_value)?,
            "kind" => module.key.kind = parse_string(&full_value)?,
            "profiles" => module.profiles = parse_string_array(&full_value)?.into_iter().collect(),
            "provides" => module.provides = parse_string_array(&full_value)?,
            "requires" => module.requires = parse_string_array(&full_value)?,
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

// ---------------------------------------------------------------------------
// Cargo.toml parsing
// ---------------------------------------------------------------------------

fn parse_cargo_feature_names(input: &str) -> Result<BTreeSet<String>, String> {
    let features = parse_cargo_features_with_deps(input)?;
    Ok(features.keys().cloned().collect())
}

fn parse_cargo_features_with_deps(input: &str) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut features: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut in_features = false;
    let lines: Vec<&str> = input.lines().collect();
    let mut idx = 0;

    while idx < lines.len() {
        let line = strip_comment(lines[idx]).trim();
        idx += 1;

        if line == "[features]" {
            in_features = true;
            continue;
        }
        if in_features && line.starts_with('[') {
            break;
        }
        if !in_features || line.is_empty() {
            continue;
        }

        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim().to_string();
        let mut full_value = value.trim().to_string();

        while !brackets_balanced(&full_value) && idx < lines.len() {
            full_value.push('\n');
            full_value.push_str(strip_comment(lines[idx]).trim());
            idx += 1;
        }

        let deps = quoted_strings(&full_value);
        features.insert(name, deps);
    }

    if features.is_empty() {
        return Err("no Cargo features parsed from Cargo.toml".to_string());
    }
    Ok(features)
}

fn brackets_balanced(s: &str) -> bool {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut prev_escape = false;
    for ch in s.chars() {
        if in_string {
            if ch == '"' && !prev_escape {
                in_string = false;
            }
            prev_escape = ch == '\\' && !prev_escape;
        } else {
            match ch {
                '"' => in_string = true,
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
        }
    }
    depth == 0
}

// ---------------------------------------------------------------------------
// compiled.rs parsing
// ---------------------------------------------------------------------------

fn parse_compiled_modules(input: &str) -> Result<Vec<CompiledModule>, String> {
    let mut modules = Vec::new();
    let mut cursor = input
        .find("fn push_transport_and_storage_modules")
        .unwrap_or(0);

    while let Some(relative) = input[cursor..].find("push_module") {
        let start = cursor + relative;

        let after = &input[start + "push_module".len()..];
        let has_requires = after.starts_with("_with_requires");

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
            let provides = strings[2..].to_vec();
            modules.push(CompiledModule {
                key: ModuleKey {
                    cargo_feature: strings[0].clone(),
                    id: strings[1].clone(),
                    kind,
                },
                provides,
                has_requires,
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

// ---------------------------------------------------------------------------
// profile composition
// ---------------------------------------------------------------------------

fn compute_profile_compositions(registry: &Registry) -> Vec<(String, Vec<String>)> {
    let mut result = Vec::new();
    for profile_name in PROFILE_ORDER {
        let mut features: Vec<String> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for module in &registry.modules {
            if module.profiles.contains(*profile_name)
                && seen.insert(module.key.cargo_feature.clone())
            {
                features.push(module.key.cargo_feature.clone());
            }
        }
        result.push((profile_name.to_string(), features));
    }
    result
}

fn render_profile_section(compositions: &[(String, Vec<String>)]) -> String {
    let mut blocks = Vec::new();
    for (profile, features) in compositions {
        let mut block = format!("profile-{profile} = [\n");
        for (idx, feature) in features.iter().enumerate() {
            let comma = if idx + 1 < features.len() { "," } else { "" };
            block.push_str(&format!("    \"{feature}\"{comma}\n"));
        }
        block.push(']');
        blocks.push(block);
    }
    blocks.join("\n")
}

// ---------------------------------------------------------------------------
// marked section helpers
// ---------------------------------------------------------------------------

fn extract_marked_section(content: &str, section_name: &str) -> Result<String, String> {
    let begin_marker = format!("# BEGIN OXIDE-REGISTRY: {section_name}\n");
    let end_marker = format!("# END OXIDE-REGISTRY: {section_name}");

    let begin_pos = content
        .find(&begin_marker)
        .ok_or_else(|| format!("missing BEGIN marker for section `{section_name}`"))?;
    let body_start = begin_pos + begin_marker.len();

    let end_pos = content[body_start..]
        .find(&end_marker)
        .ok_or_else(|| format!("missing END marker for section `{section_name}`"))?;
    let body_end = body_start + end_pos;

    Ok(content[body_start..body_end].to_string())
}

fn replace_marked_section(
    content: &str,
    section_name: &str,
    new_body: &str,
) -> Result<String, String> {
    let begin_marker = format!("# BEGIN OXIDE-REGISTRY: {section_name}\n");
    let end_marker = format!("# END OXIDE-REGISTRY: {section_name}");

    let begin_pos = content
        .find(&begin_marker)
        .ok_or_else(|| format!("missing BEGIN marker for section `{section_name}`"))?;
    let body_start = begin_pos + begin_marker.len();

    let end_pos = content[body_start..]
        .find(&end_marker)
        .ok_or_else(|| format!("missing END marker for section `{section_name}`"))?;
    let body_end = body_start + end_pos;

    let mut result = String::with_capacity(content.len());
    result.push_str(&content[..body_start]);
    result.push_str(new_body);
    result.push_str(&content[body_end..]);
    Ok(result)
}

// ---------------------------------------------------------------------------
// checks
// ---------------------------------------------------------------------------

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
    compiled: &[CompiledModule],
    errors: &mut Vec<String>,
) {
    let compiled_map: BTreeMap<&ModuleKey, &CompiledModule> =
        compiled.iter().map(|m| (&m.key, m)).collect();

    let registry_keys: BTreeSet<&ModuleKey> = registry.modules.iter().map(|m| &m.key).collect();
    let compiled_keys: BTreeSet<&ModuleKey> = compiled.iter().map(|m| &m.key).collect();

    for missing in registry_keys.difference(&compiled_keys) {
        errors.push(format!(
            "compiled.rs is missing registry module {missing:?}"
        ));
    }
    for extra in compiled_keys.difference(&registry_keys) {
        errors.push(format!(
            "compiled.rs has undeclared registry module {extra:?}"
        ));
    }

    for reg_module in &registry.modules {
        let Some(comp_module) = compiled_map.get(&reg_module.key) else {
            continue;
        };

        if reg_module.provides != comp_module.provides {
            errors.push(format!(
                "provides mismatch for module `{}`: registry={:?} compiled={:?}",
                reg_module.key.id, reg_module.provides, comp_module.provides
            ));
        }

        let registry_has_requires = !reg_module.requires.is_empty();
        if registry_has_requires != comp_module.has_requires {
            errors.push(format!(
                "requires mismatch for module `{}`: registry_requires={} compiled_uses_push_module_with_requires={}",
                reg_module.key.id, registry_has_requires, comp_module.has_requires
            ));
        }
    }
}

fn check_profile_coverage(registry: &Registry, errors: &mut Vec<String>) {
    let known: BTreeSet<String> = PROFILE_ORDER.iter().map(|s| s.to_string()).collect();
    let registry_profiles: BTreeSet<String> = registry
        .modules
        .iter()
        .flat_map(|m| m.profiles.iter().cloned())
        .collect();
    for missing in registry_profiles.difference(&known) {
        errors.push(format!(
            "registry profile `{missing}` not in xtask PROFILE_ORDER"
        ));
    }
}

fn check_core_profile_section(
    root: &Path,
    registry: &Registry,
    errors: &mut Vec<String>,
) -> Result<(), String> {
    let content = read_to_string(root, CORE_CARGO_PATH)?;
    let current = extract_marked_section(&content, "profiles")?;
    let compositions = compute_profile_compositions(registry);
    let expected = format!("{}\n", render_profile_section(&compositions));

    if current != expected {
        errors.push(
            "core Cargo.toml profile section is stale; run `cargo run -p xtask -- module-registry generate`"
                .to_string(),
        );
    }
    Ok(())
}

fn check_forwarding(
    root: &Path,
    registry: &Registry,
    errors: &mut Vec<String>,
) -> Result<(), String> {
    let mut profiles_by_transport: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for module in &registry.modules {
        if module.key.id.starts_with("transport/") {
            for profile in &module.profiles {
                profiles_by_transport
                    .entry(module.key.id.clone())
                    .or_default()
                    .insert(profile.clone());
            }
        }
    }

    for (cargo_path, transport_id) in FORWARDING_CRATES {
        let content = read_to_string(root, cargo_path)?;
        let features = parse_cargo_features_with_deps(&content)?;

        let expected_profiles = profiles_by_transport
            .get(*transport_id)
            .cloned()
            .unwrap_or_default();

        let actual_profiles: BTreeSet<String> = features
            .keys()
            .filter(|name| name.starts_with("profile-"))
            .map(|name| name.trim_start_matches("profile-").to_string())
            .collect();

        for missing in expected_profiles.difference(&actual_profiles) {
            errors.push(format!(
                "{cargo_path} is missing profile feature `profile-{missing}` for transport `{transport_id}`"
            ));
        }

        for extra in actual_profiles.difference(&expected_profiles) {
            errors.push(format!(
                "{cargo_path} has profile feature `profile-{extra}` not expected for transport `{transport_id}`"
            ));
        }

        for profile in &expected_profiles {
            let feature_name = format!("profile-{profile}");
            if let Some(deps) = features.get(&feature_name) {
                let core_forward = format!("oxide-agent-core/profile-{profile}");
                if !deps.contains(&core_forward) {
                    errors.push(format!(
                        "{cargo_path} feature `{feature_name}` does not forward to `{core_forward}`"
                    ));
                }
            }
        }
    }

    Ok(())
}
fn check_profile_tomls(
    root: &Path,
    registry: &Registry,
    errors: &mut Vec<String>,
) -> Result<(), String> {
    for profile_name in PROFILE_ORDER {
        let path = root.join("profiles").join(format!("{profile_name}.toml"));
        let current =
            fs::read_to_string(&path).map_err(|err| format!("read {}: {err}", path.display()))?;
        let expected = render_profile_toml(profile_name, registry);
        if current != expected {
            errors.push(format!(
                "profiles/{profile_name}.toml is stale; run `cargo run -p xtask -- module-registry generate`"
            ));
        }
    }
    Ok(())
}

fn generate_profile_tomls(root: &Path, registry: &Registry) -> Result<(), String> {
    for profile_name in PROFILE_ORDER {
        let content = render_profile_toml(profile_name, registry);
        let path = root.join("profiles").join(format!("{profile_name}.toml"));
        fs::write(&path, content).map_err(|err| format!("write {}: {err}", path.display()))?;
    }
    Ok(())
}

fn render_profile_toml(profile_name: &str, registry: &Registry) -> String {
    let mut module_ids: BTreeSet<String> = BTreeSet::new();
    for module in &registry.modules {
        if module.profiles.contains(profile_name) {
            module_ids.insert(module.profile_module_id().to_string());
        }
    }

    let mut output = format!("profile = \"{profile_name}\"\n");
    output.push_str(&format!("cargo_features = [\"profile-{profile_name}\"]\n"));
    output.push('\n');
    output.push_str("[modules]\n");
    for module_id in &module_ids {
        output.push_str(&format!("\"{module_id}\" = {{ enabled = true }}\n"));
    }
    output
}
