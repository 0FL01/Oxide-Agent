#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/check-compiled-capabilities.sh <embedded-opencode-local|lite|search-only|no-sandbox|media-enabled|full>

Runs the Telegram app capability CLI for a profile and verifies the compiled
capability manifest stays deterministic and aligned with the PRD profile
contract.
USAGE
}

if [[ $# -ne 1 || "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 2
fi

profile="$1"
case "${profile}" in
  embedded-opencode-local | lite | search-only | no-sandbox | media-enabled | full)
    cargo_feature="profile-${profile}"
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
  -p oxide-agent-telegram-bot \
  --bin oxide-agent-telegram-bot \
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

with open(manifest_path, "r", encoding="utf-8") as fh:
    manifest = json.load(fh)

modules = manifest.get("modules", [])
capabilities = manifest.get("capabilities", [])
module_ids = [module["id"] for module in modules]
capability_ids = [capability["id"] for capability in capabilities]


def fail(message: str) -> None:
    print(f"compiled capability check failed for {profile}: {message}", file=sys.stderr)
    sys.exit(1)


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

profile_path = pathlib.Path("profiles") / f"{profile}.toml"
if not profile_path.exists():
    fail(f"profile defaults file is missing: {profile_path}")
with profile_path.open("rb") as fh:
    profile_doc = tomllib.load(fh)
if profile_doc.get("profile") != profile:
    fail(f"profile defaults file has wrong profile name: {profile_doc.get('profile')!r}")
if profile_doc.get("cargo_features") != [f"profile-{profile}"]:
    fail(f"profile defaults file has wrong cargo_features: {profile_doc.get('cargo_features')!r}")
profile_module_ids = set(profile_doc.get("modules", {}).keys())
if profile_module_ids != module_set:
    missing = sorted(module_set - profile_module_ids)
    unexpected = sorted(profile_module_ids - module_set)
    fail(
        "profile defaults module IDs drifted from compiled manifest; "
        f"missing={missing}; unexpected={unexpected}"
    )

common_forbidden_ids = {
    "llm-provider/gemini",
    "llm-provider/google-gemini",
    "llm-provider/google-gemini-direct",
}

profile_requirements = {
    "embedded-opencode-local": {
        "exact_modules": {
            "llm-provider/opencode-go",
            "storage/r2",
            "tool/agents-md",
            "tool/reminder",
            "tool/todos",
            "tool/webfetch-md",
            "tool/wiki-memory",
            "transport/telegram",
        },
        "required_capabilities": {
            "llm-provider/opencode-go",
            "storage/r2",
            "tool/agents-md",
            "tool/reminder",
            "tool/todos",
            "tool/webfetch-md",
            "tool/wiki-memory",
            "transport/telegram",
        },
        "forbidden_modules": common_forbidden_ids,
        "forbidden_prefixes": (
            "integration/",
            "sandbox-backend/",
            "sandbox-daemon/",
            "tool/browser-use",
            "tool/file-delivery",
            "tool/media-",
            "tool/sandbox-",
            "tool/searxng",
            "tool/stack-logs",
            "tool/tavily",
            "tool/tts-",
            "tool/ytdlp",
        ),
    },
    "lite": {
        "exact_modules": {
            "llm-provider/opencode-go",
            "storage/r2",
            "tool/reminder",
            "tool/todos",
            "tool/webfetch-md",
            "transport/telegram",
        },
        "required_capabilities": {
            "llm-provider/opencode-go",
            "storage/r2",
            "tool/reminder",
            "tool/todos",
            "tool/webfetch-md",
            "transport/telegram",
        },
        "forbidden_modules": common_forbidden_ids,
        "forbidden_prefixes": (
            "integration/",
            "sandbox-backend/",
            "sandbox-daemon/",
            "tool/agents-md",
            "tool/browser-use",
            "tool/file-delivery",
            "tool/media-",
            "tool/sandbox-",
            "tool/searxng",
            "tool/stack-logs",
            "tool/tavily",
            "tool/tts-",
            "tool/wiki-memory",
            "tool/ytdlp",
        ),
    },
    "search-only": {
        "required_modules": {
            "llm-provider/opencode-go",
            "storage/r2",
            "tool/tavily",
            "tool/webfetch-md",
            "transport/telegram",
        },
        "required_capabilities": {
            "tool/tavily-extract",
            "tool/tavily-search",
            "tool/webfetch-md",
        },
        "forbidden_modules": common_forbidden_ids,
        "forbidden_prefixes": (
            "integration/",
            "sandbox-backend/",
            "sandbox-daemon/",
            "tool/browser-use",
            "tool/media-",
            "tool/sandbox-",
            "tool/searxng",
            "tool/stack-logs",
            "tool/tts-",
            "tool/ytdlp",
        ),
    },
    "no-sandbox": {
        "exact_modules": {
            "llm-provider/opencode-go",
            "storage/r2",
            "tool/reminder",
            "tool/todos",
            "tool/webfetch-md",
            "tool/wiki-memory",
            "transport/telegram",
        },
        "required_capabilities": {
            "llm-provider/opencode-go",
            "storage/r2",
            "tool/reminder",
            "tool/todos",
            "tool/webfetch-md",
            "tool/wiki-memory",
            "transport/telegram",
        },
        "forbidden_modules": common_forbidden_ids,
        "forbidden_prefixes": (
            "integration/",
            "sandbox-backend/",
            "sandbox-daemon/",
            "tool/agents-md",
            "tool/browser-use",
            "tool/file-delivery",
            "tool/media-",
            "tool/sandbox-",
            "tool/searxng",
            "tool/stack-logs",
            "tool/tavily",
            "tool/tts-",
            "tool/ytdlp",
        ),
    },
    "media-enabled": {
        "required_modules": {
            "llm-provider/opencode-go",
            "storage/r2",
            "tool/file-delivery",
            "tool/media-audio",
            "tool/media-image",
            "tool/media-video",
            "transport/telegram",
        },
        "required_capabilities": {
            "tool/file-delivery",
            "tool/media-audio-transcription",
            "tool/media-image-description",
            "tool/media-video-description",
        },
        "forbidden_modules": common_forbidden_ids,
        "forbidden_prefixes": (
            "integration/",
            "sandbox-backend/",
            "sandbox-daemon/",
            "tool/browser-use",
            "tool/sandbox-",
            "tool/searxng",
            "tool/stack-logs",
            "tool/tavily",
            "tool/tts-",
            "tool/webfetch-md",
            "tool/ytdlp",
        ),
    },
    "full": {
        "required_modules": {
            "integration/mcp-jira",
            "integration/mcp-mattermost",
            "integration/ssh-mcp",
            "llm-provider/groq",
            "llm-provider/minimax",
            "llm-provider/mistral",
            "llm-provider/nvidia",
            "llm-provider/openai-chatgpt",
            "llm-provider/opencode-go",
            "llm-provider/openrouter",
            "llm-provider/zai",
            "manager/control-plane",
            "sandbox-backend/docker-direct",
            "sandbox-backend/sandboxd-client",
            "sandbox-daemon/sandboxd",
            "storage/r2",
            "tool/browser-use",
            "tool/searxng",
            "tool/stack-logs",
            "tool/ytdlp",
            "transport/telegram",
            "transport/web",
        },
        "required_capabilities": {
            "manager/control-plane",
            "sandbox-backend/docker-direct/exec",
            "sandbox-backend/sandboxd-client/exec",
            "tool/browser-use",
            "tool/searxng-search",
            "tool/stack-logs",
            "tool/ytdlp-download",
        },
        "forbidden_modules": common_forbidden_ids,
        "forbidden_prefixes": (),
    },
}

rules = profile_requirements[profile]

exact_modules = rules.get("exact_modules")
if exact_modules is not None and module_set != exact_modules:
    missing = sorted(exact_modules - module_set)
    unexpected = sorted(module_set - exact_modules)
    fail(f"module set mismatch; missing={missing}; unexpected={unexpected}")

missing_modules = sorted(rules.get("required_modules", set()) - module_set)
if missing_modules:
    fail(f"missing required modules: {missing_modules}")

missing_capabilities = sorted(rules.get("required_capabilities", set()) - capability_set)
if missing_capabilities:
    fail(f"missing required capabilities: {missing_capabilities}")

forbidden_modules = sorted(rules.get("forbidden_modules", set()) & module_set)
if forbidden_modules:
    fail(f"forbidden modules present: {forbidden_modules}")

forbidden_capabilities = sorted(rules.get("forbidden_modules", set()) & capability_set)
if forbidden_capabilities:
    fail(f"forbidden capabilities present: {forbidden_capabilities}")

for prefix in rules.get("forbidden_prefixes", ()):
    forbidden_by_prefix = sorted(module_id for module_id in module_set if module_id.startswith(prefix))
    if forbidden_by_prefix:
        fail(f"forbidden module prefix {prefix!r} present: {forbidden_by_prefix}")
    forbidden_capabilities_by_prefix = sorted(
        capability_id for capability_id in capability_set if capability_id.startswith(prefix)
    )
    if forbidden_capabilities_by_prefix:
        fail(f"forbidden capability prefix {prefix!r} present: {forbidden_capabilities_by_prefix}")

print(f"compiled capability check passed for {profile}: {len(module_ids)} modules, {len(capability_ids)} capabilities")
PY

cargo run -q \
  -p oxide-agent-telegram-bot \
  --bin oxide-agent-telegram-bot \
  --no-default-features \
  --features "${cargo_feature}" \
  -- config schema --compiled --json >"${tmp_config_schema}"

python3 - "${profile}" "${tmp_manifest}" "${tmp_config_schema}" <<'PY'
import json
import sys

profile = sys.argv[1]
manifest_path = sys.argv[2]
schema_path = sys.argv[3]

with open(manifest_path, "r", encoding="utf-8") as fh:
    manifest = json.load(fh)
with open(schema_path, "r", encoding="utf-8") as fh:
    schema = json.load(fh)

module_ids = [module["id"] for module in manifest.get("modules", [])]
module_schema = (
    schema.get("properties", {})
    .get("modules", {})
)
schema_modules = module_schema.get("properties", {})


def fail(message: str) -> None:
    print(f"config schema check failed for {profile}: {message}", file=sys.stderr)
    sys.exit(1)


if module_schema.get("additionalProperties") is not False:
    fail("modules schema must reject non-compiled module IDs")

if list(schema_modules.keys()) != module_ids:
    fail(
        "schema module IDs do not match compiled manifest; "
        f"schema={list(schema_modules.keys())}; manifest={module_ids}"
    )

for module in manifest.get("modules", []):
    module_id = module["id"]
    entry = schema_modules[module_id]
    enabled_schema = entry.get("properties", {}).get("enabled", {})
    if enabled_schema.get("type") != "boolean":
        fail(f"{module_id} missing boolean enabled flag schema")
    if entry.get("x-oxide-cargo-feature") != module["cargo_feature"]:
        fail(f"{module_id} cargo feature metadata mismatch")
    if entry.get("x-oxide-provides") != module["provides"]:
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
    if module_id.startswith("llm-provider/") and module_id != "llm-provider/openai-chatgpt":
        if not any(prop["name"] == "api_key" for prop in declared_properties):
            fail(f"{module_id} provider schema must declare module-owned api_key")
    if module_id == "llm-provider/openai-chatgpt":
        if not any(prop["name"] == "auth_path" for prop in declared_properties):
            fail("llm-provider/openai-chatgpt schema must declare module-owned auth_path")

print(f"config schema check passed for {profile}: {len(schema_modules)} module schemas")
PY

cargo run -q \
  -p oxide-agent-telegram-bot \
  --bin oxide-agent-telegram-bot \
  --no-default-features \
  --features "${cargo_feature}" \
  -- config example --profile "${profile}" --json >"${tmp_config_example}"

python3 - "${profile}" "${tmp_manifest}" "${tmp_config_example}" <<'PY'
import json
import sys

profile = sys.argv[1]
manifest_path = sys.argv[2]
example_path = sys.argv[3]

with open(manifest_path, "r", encoding="utf-8") as fh:
    manifest = json.load(fh)
with open(example_path, "r", encoding="utf-8") as fh:
    example = json.load(fh)

module_ids = [module["id"] for module in manifest.get("modules", [])]
example_modules = example.get("modules", {})


def fail(message: str) -> None:
    print(f"config example check failed for {profile}: {message}", file=sys.stderr)
    sys.exit(1)


if example.get("profile") != profile:
    fail(f"example profile mismatch: {example.get('profile')!r}")
if list(example_modules.keys()) != module_ids:
    fail(
        "example module IDs do not match compiled manifest; "
        f"example={list(example_modules.keys())}; manifest={module_ids}"
    )

for module in manifest.get("modules", []):
    module_id = module["id"]
    config = example_modules[module_id]
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

cat >"${tmp_enabled_config}" <<'YAML'
modules:
  transport/telegram:
    enabled: false
YAML

cargo run -q \
  -p oxide-agent-telegram-bot \
  --bin oxide-agent-telegram-bot \
  --no-default-features \
  --features "${cargo_feature}" \
  -- capabilities --enabled --config "${tmp_enabled_config}" --json >"${tmp_enabled_manifest}"

python3 - "${profile}" "${tmp_enabled_manifest}" <<'PY'
import json
import sys

profile = sys.argv[1]
manifest_path = sys.argv[2]

with open(manifest_path, "r", encoding="utf-8") as fh:
    manifest = json.load(fh)

module_ids = set(manifest.get("modules", []))
capability_ids = set(manifest.get("capabilities", []))

if "transport/telegram" in module_ids or "transport/telegram" in capability_ids:
    print(
        f"enabled capability check failed for {profile}: disabled transport/telegram is still present",
        file=sys.stderr,
    )
    sys.exit(1)

print(f"enabled capability check passed for {profile}: disabled transport/telegram removed")
PY

cat >"${tmp_invalid_config}" <<'YAML'
modules:
  tool/not-compiled:
    enabled: false
YAML

if cargo run -q \
  -p oxide-agent-telegram-bot \
  --bin oxide-agent-telegram-bot \
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
