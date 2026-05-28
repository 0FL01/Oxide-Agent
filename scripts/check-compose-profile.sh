#!/usr/bin/env bash
set -euo pipefail

profile="${1:?usage: scripts/check-compose-profile.sh <embedded-opencode-local|search|media|dev|full|root-full>}"

if [[ -f Dockerfile ]]; then
  echo "root Dockerfile must stay removed; use docker/Dockerfile.app with explicit profile args" >&2
  exit 1
fi

if [[ -f sandbox/Dockerfile.sandbox ]]; then
  echo "legacy fat sandbox/Dockerfile.sandbox must stay removed; use explicit sandbox image variants" >&2
  exit 1
fi

case "${profile}" in
  root-full) compose_file="docker-compose.yml" ;;
  *) compose_file="docker/compose.${profile}.yml" ;;
esac

if [[ ! -f "${compose_file}" ]]; then
  echo "compose profile '${profile}' not found at ${compose_file}" >&2
  exit 1
fi

services="$(docker compose -f "${compose_file}" config --services | sort)"
config="$(docker compose -f "${compose_file}" config --no-env-resolution)"
config_json="$(docker compose -f "${compose_file}" config --no-env-resolution --format json)"

has_service() {
  grep -qx "$1" <<<"${services}"
}

require_service() {
  if ! has_service "$1"; then
    echo "expected service '$1' in ${compose_file}" >&2
    echo "${services}" >&2
    exit 1
  fi
}

forbid_service() {
  if has_service "$1"; then
    echo "unexpected service '$1' in ${compose_file}" >&2
    echo "${services}" >&2
    exit 1
  fi
}

forbid_config_text() {
  if grep -q "$1" <<<"${config}"; then
    echo "unexpected '$1' in ${compose_file}" >&2
    exit 1
  fi
}

require_config_text() {
  if ! grep -q "$1" <<<"${config}"; then
    echo "expected '$1' in ${compose_file}" >&2
    exit 1
  fi
}

check_structural_topology() {
  COMPOSE_CONFIG_JSON="${config_json}" python3 - "${profile}" "${compose_file}" <<'PY'
import json
import os
import pathlib
import sys
import tomllib

profile = sys.argv[1]
compose_file = sys.argv[2]
config = json.loads(os.environ["COMPOSE_CONFIG_JSON"])
services = config.get("services", {})
declared_volumes = set((config.get("volumes") or {}).keys())

profile_to_defaults = {
    "embedded-opencode-local": "embedded-opencode-local",
    "search": "search-only",
    "media": "media-enabled",
    "dev": "full",
    "full": "full",
    "root-full": "full",
}
defaults_profile = profile_to_defaults[profile]
profile_path = pathlib.Path("profiles") / f"{defaults_profile}.toml"
with profile_path.open("rb") as fh:
    profile_doc = tomllib.load(fh)
module_ids = set((profile_doc.get("modules") or {}).keys())

uses_sandboxd = "sandbox-daemon/sandboxd" in module_ids
uses_searxng = "tool/searxng" in module_ids
uses_browser_use = False  # Browser Use bridge is intentionally dormant until a cost-effective vision model is selected.
uses_ssh_mcp = "integration/ssh-mcp" in module_ids

expected_cargo_features = [
    f"oxide-agent-telegram-bot/profile-{defaults_profile}",
]
if uses_sandboxd:
    expected_cargo_features.append("oxide-agent-sandboxd/profile-full")
expected_cargo_features_text = ",".join(expected_cargo_features)

expected_packages = ["oxide-agent-telegram-bot"]
expected_binaries = ["oxide-agent-telegram-bot"]
if uses_sandboxd:
    expected_packages.append("oxide-agent-sandboxd")
    expected_binaries.append("oxide-agent-sandboxd")
expected_packages_text = " ".join(expected_packages)
expected_binaries_text = " ".join(expected_binaries)

mcp_binary_by_module = [
    ("integration/ssh-mcp", "ssh-mcp"),
    ("integration/mcp-jira", "jira-mcp"),
    ("integration/mcp-mattermost", "mattermost-mcp"),
]
expected_mcp_binaries = [
    binary for module_id, binary in mcp_binary_by_module if module_id in module_ids
]
expected_mcp_binaries_text = " ".join(expected_mcp_binaries)
expected_runtime_apt_packages = "openssh-client" if uses_ssh_mcp else ""


def fail(message: str) -> None:
    print(f"compose profile '{profile}' check failed for {compose_file}: {message}", file=sys.stderr)
    sys.exit(1)


docker_socket_mounts = []
for service_name, service in services.items():
    for volume in service.get("volumes") or []:
        if not isinstance(volume, dict):
            continue
        source = volume.get("source")
        target = volume.get("target")
        if source == "/var/run/docker.sock" or target == "/var/run/docker.sock":
            docker_socket_mounts.append((service_name, source, target))

if profile in {"embedded-opencode-local", "search", "media"}:
    if docker_socket_mounts:
        fail(f"Docker socket must be absent from minimal profiles; mounts={docker_socket_mounts}")
else:
    expected = [("sandboxd", "/var/run/docker.sock", "/var/run/docker.sock")]
    if docker_socket_mounts != expected:
        fail(f"Docker socket must be mounted only into sandboxd; mounts={docker_socket_mounts}")

module_volume_owners = {
    "browser-use-data": "browser_use",
    "sandboxd-run": "sandboxd",
}
for volume_name, owner_service in module_volume_owners.items():
    if volume_name in declared_volumes and owner_service not in services:
        fail(f"volume {volume_name!r} declared without selected service {owner_service!r}")

if "sandboxd" in services and "sandboxd-run" not in declared_volumes:
    fail("sandboxd service requires sandboxd-run volume")

if "browser_use" in services and "browser-use-data" not in declared_volumes:
    fail("browser_use service requires browser-use-data volume")

if ("sandboxd" in services) != uses_sandboxd:
    fail(
        "sandboxd service selection does not match profile modules; "
        f"service_present={'sandboxd' in services}; module_selected={uses_sandboxd}"
    )

if ("sandbox_image" in services) != uses_sandboxd:
    fail(
        "sandbox image service selection does not match sandbox daemon module; "
        f"service_present={'sandbox_image' in services}; module_selected={uses_sandboxd}"
    )

if ("searxng" in services) != uses_searxng:
    fail(
        "searxng service selection does not match tool/searxng module; "
        f"service_present={'searxng' in services}; module_selected={uses_searxng}"
    )

if "browser_use" in services and not uses_browser_use:
    fail("browser_use service must stay absent while the bridge is intentionally dormant")

oxide_agent = services.get("oxide_agent")
if not oxide_agent:
    fail("oxide_agent service is required")

def normalized_args(service_name: str) -> dict[str, str]:
    service = services[service_name]
    build = service.get("build")
    if not build:
        fail(f"{service_name} must declare a profile-aware app build")
    dockerfile = build.get("dockerfile")
    if dockerfile != "docker/Dockerfile.app":
        fail(f"{service_name} must build docker/Dockerfile.app, got {dockerfile!r}")
    return {str(key): str(value) for key, value in (build.get("args") or {}).items()}


def assert_app_build_args(service_name: str) -> None:
    args = normalized_args(service_name)
    expected = {
        "CARGO_FEATURES": expected_cargo_features_text,
        "PACKAGES": expected_packages_text,
        "BINARIES": expected_binaries_text,
        "ENTRYPOINT_BINARY": "oxide-agent-telegram-bot",
    }
    if expected_mcp_binaries_text:
        expected["MCP_BINARIES"] = expected_mcp_binaries_text
    if expected_runtime_apt_packages:
        expected["RUNTIME_APT_PACKAGES"] = expected_runtime_apt_packages

    for key, expected_value in expected.items():
        if args.get(key) != expected_value:
            fail(
                f"{service_name} build arg {key} mismatch: "
                f"actual={args.get(key)!r}; expected={expected_value!r}"
            )

    forbidden_when_empty = {
        "MCP_BINARIES": expected_mcp_binaries_text,
        "RUNTIME_APT_PACKAGES": expected_runtime_apt_packages,
    }
    for key, expected_value in forbidden_when_empty.items():
        if not expected_value and args.get(key, "") not in {"", None}:
            fail(f"{service_name} build arg {key} must be absent or empty for this profile")


assert_app_build_args("oxide_agent")

if "oxide_web" in services:
    args = normalized_args("oxide_web")
    expected = {
        "CARGO_FEATURES": "oxide-agent-transport-web/profile-web-embedded-opencode-local",
        "PACKAGES": "oxide-agent-transport-web",
        "BINARIES": "oxide-agent-web-console",
        "ENTRYPOINT_BINARY": "oxide-agent-web-console",
        "BUILD_WEB_UI": "true",
    }
    for key, expected_value in expected.items():
        if args.get(key) != expected_value:
            fail(
                f"oxide_web build arg {key} mismatch: "
                f"actual={args.get(key)!r}; expected={expected_value!r}"
            )
    if args.get("MCP_BINARIES", "") not in {"", None}:
        fail("oxide_web build arg MCP_BINARIES must be empty")
    if args.get("RUNTIME_APT_PACKAGES", "") not in {"", None}:
        fail("oxide_web build arg RUNTIME_APT_PACKAGES must be empty")

if "sandboxd" in services:
    sandboxd = services["sandboxd"]
    build = sandboxd.get("build")
    if build:
        assert_app_build_args("sandboxd")
    elif sandboxd.get("image") != oxide_agent.get("image"):
        fail("sandboxd without its own build must reuse the oxide_agent image")
    command = sandboxd.get("command") or []
    if "./oxide-agent-sandboxd" not in command:
        fail("sandboxd service must run the oxide-agent-sandboxd binary")

if not uses_sandboxd and expected_mcp_binaries:
    fail("MCP binaries require the full sandbox-capable app image profile")

print(f"compose profile '{profile}' structural topology check passed")
PY
}

check_structural_topology

case "${profile}" in
  embedded-opencode-local | search | media)
    require_service oxide_agent
    forbid_service sandboxd
    forbid_service sandbox_image
    forbid_service searxng
    forbid_service browser_use
    forbid_config_text "MCP_BINARIES"
    forbid_config_text "ssh-mcp"
    forbid_config_text "jira-mcp"
    forbid_config_text "mattermost-mcp"
    forbid_config_text "/var/run/docker.sock"
    forbid_config_text "sandboxd-run"
    forbid_config_text "browser-use-data"
    ;;
  dev | full | root-full)
    require_service oxide_agent
    require_service sandboxd
    require_service sandbox_image
    require_service searxng
    if [[ "${profile}" == "root-full" ]]; then
      require_service oxide_web
    fi
    require_config_text "sandbox/Dockerfile.dev"
    forbid_config_text "sandbox/Dockerfile.sandbox"
    if ! grep -q "/var/run/docker.sock" <<<"${config}"; then
      echo "full compose must mount Docker socket only into sandboxd" >&2
      exit 1
    fi
    require_config_text "MCP_BINARIES"
    require_config_text "ssh-mcp"
    require_config_text "jira-mcp"
    require_config_text "mattermost-mcp"
    ;;
  *)
    echo "unknown compose profile '${profile}'" >&2
    exit 1
    ;;
esac

echo "compose profile '${profile}' check passed"
