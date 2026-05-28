#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/check-compiled-capabilities.sh <embedded-opencode-local|web-embedded-opencode-local|lite|search-only|no-sandbox|media-enabled|host-bwrap|full>

Runs the Telegram app capability CLI for a profile and verifies that the
compiled manifest, profile defaults, config schema, and config example stay
aligned. The source of truth is the Rust compiled manifest plus profiles/*.toml;
this script intentionally avoids profile-specific hardcoded module lists.
USAGE
}

if [[ $# -ne 1 || "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 2
fi

profile="$1"
case "${profile}" in
  embedded-opencode-local | lite | search-only | no-sandbox | media-enabled | host-bwrap | full)
    cargo_feature="profile-${profile}"
    cargo_package="oxide-agent-telegram-bot"
    cargo_bin="oxide-agent-telegram-bot"
    transport_module_id="transport/telegram"
    ;;
  web-embedded-opencode-local)
    cargo_feature="profile-web-embedded-opencode-local"
    cargo_package="oxide-agent-transport-web"
    cargo_bin="oxide-agent-web-console"
    transport_module_id="transport/web"
    ;;
  *)
    echo "unknown capability profile '${profile}'" >&2
    usage
    exit 2
    ;;
esac

tmp_manifest="$(mktemp)"
tmp_config_schema="$(mktemp)"
tmp_config_example="$(mktemp)"
tmp_enabled_config="$(mktemp --suffix=.yaml)"
tmp_enabled_manifest="$(mktemp)"
tmp_invalid_config="$(mktemp --suffix=.yaml)"
tmp_invalid_stderr="$(mktemp)"
trap 'rm -f "${tmp_manifest}" "${tmp_config_schema}" "${tmp_config_example}" "${tmp_enabled_config}" "${tmp_enabled_manifest}" "${tmp_invalid_config}" "${tmp_invalid_stderr}"' EXIT

cargo run -q \
  -p "${cargo_package}" \
  --bin "${cargo_bin}" \
  --no-default-features \
  --features "${cargo_feature}" \
  -- capabilities --compiled --json >"${tmp_manifest}"

python3 - "${profile}" "${tmp_manifest}" <<'PY'
import json
import pathlib
import sys
import tomllib

profile = sys.argv[1]
manifest_path = sys.argv[2]


def fail(message: str) -> None:
    print(f"compiled capability check failed for {profile}: {message}", file=sys.stderr)
    sys.exit(1)


with open(manifest_path, "r", encoding="utf-8") as fh:
    manifest = json.load(fh)

modules = manifest.get("modules", [])
capabilities = manifest.get("capabilities", [])
module_ids = [module.get("id") for module in modules]
capability_ids = [capability.get("id") for capability in capabilities]

if not modules:
    fail("manifest has no modules")
if any(not isinstance(module_id, str) or not module_id for module_id in module_ids):
    fail(f"invalid module ids: {module_ids!r}")
if any(not isinstance(capability_id, str) or not capability_id for capability_id in capability_ids):
    fail(f"invalid capability ids: {capability_ids!r}")
if module_ids != sorted(module_ids):
    fail("module ids are not sorted deterministically")
if capability_ids != sorted(capability_ids):
    fail("capability ids are not sorted deterministically")
if len(module_ids) != len(set(module_ids)):
    fail("duplicate module id detected")
if len(capability_ids) != len(set(capability_ids)):
    fail("duplicate capability id detected")

module_set = set(module_ids)
capability_set = set(capability_ids)

for module in modules:
    module_id = module["id"]
    cargo_feature = module.get("cargo_feature")
    provides = module.get("provides", [])
    if not isinstance(cargo_feature, str) or not cargo_feature:
        fail(f"{module_id} missing cargo_feature metadata")
    if not isinstance(provides, list):
        fail(f"{module_id} has non-list provides metadata")
    missing_provides = sorted(set(provides) - capability_set)
    if missing_provides:
        fail(f"{module_id} provides capabilities absent from manifest: {missing_provides}")
    for prop in module.get("config_properties", []):
        if not isinstance(prop.get("name"), str) or not prop["name"]:
            fail(f"{module_id} has config property without a name: {prop!r}")

profile_path = pathlib.Path("profiles") / f"{profile}.toml"
if not profile_path.exists():
    fail(f"profile defaults file is missing: {profile_path}")
with profile_path.open("rb") as fh:
    profile_doc = tomllib.load(fh)
if profile_doc.get("profile") != profile:
    fail(f"profile defaults file has wrong profile name: {profile_doc.get('profile')!r}")
if profile_doc.get("cargo_features") != [f"profile-{profile}"]:
    fail(f"profile defaults file has wrong cargo_features: {profile_doc.get('cargo_features')!r}")
profile_module_ids = set((profile_doc.get("modules") or {}).keys())
if profile_module_ids != module_set:
    missing = sorted(module_set - profile_module_ids)
    unexpected = sorted(profile_module_ids - module_set)
    fail(
        "profile defaults module IDs drifted from compiled manifest; "
        f"missing={missing}; unexpected={unexpected}"
    )

removed_direct_gemini_ids = {
    "llm-provider/gemini",
    "llm-provider/google-gemini",
    "llm-provider/google-gemini-direct",
}
present_removed = sorted((module_set | capability_set) & removed_direct_gemini_ids)
if present_removed:
    fail(f"removed direct Gemini provider ids are present: {present_removed}")

print(
    f"compiled capability check passed for {profile}: "
    f"{len(module_ids)} modules, {len(capability_ids)} capabilities"
)
PY

cargo run -q \
  -p "${cargo_package}" \
  --bin "${cargo_bin}" \
  --no-default-features \
  --features "${cargo_feature}" \
  -- config schema --compiled --json >"${tmp_config_schema}"

python3 - "${profile}" "${tmp_manifest}" "${tmp_config_schema}" <<'PY'
import json
import sys

profile = sys.argv[1]
manifest_path = sys.argv[2]
schema_path = sys.argv[3]


def fail(message: str) -> None:
    print(f"config schema check failed for {profile}: {message}", file=sys.stderr)
    sys.exit(1)


with open(manifest_path, "r", encoding="utf-8") as fh:
    manifest = json.load(fh)
with open(schema_path, "r", encoding="utf-8") as fh:
    schema = json.load(fh)

module_ids = [module["id"] for module in manifest.get("modules", [])]
module_schema = schema.get("properties", {}).get("modules", {})
schema_modules = module_schema.get("properties", {})

if module_schema.get("additionalProperties") is not False:
    fail("modules schema must reject non-compiled module IDs")
if list(schema_modules.keys()) != module_ids:
    fail(
        "schema module IDs do not match compiled manifest; "
        f"schema={list(schema_modules.keys())}; manifest={module_ids}"
    )

for module in manifest.get("modules", []):
    module_id = module["id"]
    entry = schema_modules.get(module_id)
    if entry is None:
        fail(f"missing schema entry for {module_id}")

    enabled_schema = entry.get("properties", {}).get("enabled", {})
    if enabled_schema.get("type") != "boolean":
        fail(f"{module_id} missing boolean enabled flag schema")
    if entry.get("x-oxide-cargo-feature") != module.get("cargo_feature"):
        fail(f"{module_id} cargo feature metadata mismatch")
    if entry.get("x-oxide-provides") != module.get("provides"):
        fail(f"{module_id} provided capability metadata mismatch")

    declared_properties = module.get("config_properties", [])
    if entry.get("x-oxide-config-properties") != declared_properties:
        fail(f"{module_id} config property metadata mismatch")

    schema_properties = entry.get("properties", {})
    for prop in declared_properties:
        name = prop["name"]
        prop_schema = schema_properties.get(name)
        if prop_schema is None:
            fail(f"{module_id} missing declared config property schema: {name}")
        if prop_schema.get("type") != "string":
            fail(f"{module_id}.{name} must be a string config property")
        if prop.get("env") and prop_schema.get("x-oxide-env") != prop["env"]:
            fail(f"{module_id}.{name} env metadata mismatch")
        if prop.get("secret") and prop_schema.get("x-oxide-secret") is not True:
            fail(f"{module_id}.{name} secret metadata mismatch")
        if prop.get("default_value") is not None and prop_schema.get("default") != prop["default_value"]:
            fail(f"{module_id}.{name} default metadata mismatch")

print(f"config schema check passed for {profile}: {len(schema_modules)} module schemas")
PY

cargo run -q \
  -p "${cargo_package}" \
  --bin "${cargo_bin}" \
  --no-default-features \
  --features "${cargo_feature}" \
  -- config example --profile "${profile}" --json >"${tmp_config_example}"

python3 - "${profile}" "${tmp_manifest}" "${tmp_config_example}" <<'PY'
import json
import sys

profile = sys.argv[1]
manifest_path = sys.argv[2]
example_path = sys.argv[3]


def fail(message: str) -> None:
    print(f"config example check failed for {profile}: {message}", file=sys.stderr)
    sys.exit(1)


with open(manifest_path, "r", encoding="utf-8") as fh:
    manifest = json.load(fh)
with open(example_path, "r", encoding="utf-8") as fh:
    example = json.load(fh)

module_ids = [module["id"] for module in manifest.get("modules", [])]
example_modules = example.get("modules", {})

if example.get("profile") != profile:
    fail(f"example profile mismatch: {example.get('profile')!r}")
if list(example_modules.keys()) != module_ids:
    fail(
        "example module IDs do not match compiled manifest; "
        f"example={list(example_modules.keys())}; manifest={module_ids}"
    )

for module in manifest.get("modules", []):
    module_id = module["id"]
    config = example_modules.get(module_id)
    if config is None:
        fail(f"missing config example entry for {module_id}")
    if config.get("enabled") is not True:
        fail(f"{module_id} example must enable compiled module by default")
    for prop in module.get("config_properties", []):
        name = prop["name"]
        if prop.get("secret") and name in config:
            fail(f"{module_id}.{name} secret value must not be emitted in config examples")
        if not prop.get("secret") and prop.get("default_value") is not None:
            if config.get(name) != prop["default_value"]:
                fail(f"{module_id}.{name} default value missing from config example")

print(f"config example check passed for {profile}: {len(example_modules)} module configs")
PY

cat >"${tmp_enabled_config}" <<YAML
modules:
  ${transport_module_id}:
    enabled: false
YAML

cargo run -q \
  -p "${cargo_package}" \
  --bin "${cargo_bin}" \
  --no-default-features \
  --features "${cargo_feature}" \
  -- capabilities --enabled --config "${tmp_enabled_config}" --json >"${tmp_enabled_manifest}"

python3 - "${profile}" "${transport_module_id}" "${tmp_enabled_manifest}" <<'PY'
import json
import sys

profile = sys.argv[1]
transport_module_id = sys.argv[2]
manifest_path = sys.argv[3]

with open(manifest_path, "r", encoding="utf-8") as fh:
    manifest = json.load(fh)

module_ids = set(manifest.get("modules", []))
capability_ids = set(manifest.get("capabilities", []))

if transport_module_id in module_ids or transport_module_id in capability_ids:
    print(
        f"enabled capability check failed for {profile}: disabled {transport_module_id} is still present",
        file=sys.stderr,
    )
    sys.exit(1)

print(f"enabled capability check passed for {profile}: disabled {transport_module_id} removed")
PY

cat >"${tmp_invalid_config}" <<'YAML'
modules:
  tool/not-compiled:
    enabled: false
YAML

if cargo run -q \
  -p "${cargo_package}" \
  --bin "${cargo_bin}" \
  --no-default-features \
  --features "${cargo_feature}" \
  -- capabilities --enabled --config "${tmp_invalid_config}" --json 2>"${tmp_invalid_stderr}"; then
  echo "enabled capability check failed for ${profile}: non-compiled module config unexpectedly succeeded" >&2
  exit 1
fi

if ! grep -q "non-compiled or unknown module id: tool/not-compiled" "${tmp_invalid_stderr}"; then
  echo "enabled capability check failed for ${profile}: unexpected error for non-compiled module config" >&2
  cat "${tmp_invalid_stderr}" >&2
  exit 1
fi

echo "enabled capability validation check passed for ${profile}: non-compiled module config rejected"
