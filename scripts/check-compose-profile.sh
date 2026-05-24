#!/usr/bin/env bash
set -euo pipefail

profile="${1:?usage: scripts/check-compose-profile.sh <embedded-opencode-local|search|media|dev|full|root-full>}"

if [[ -f Dockerfile ]]; then
  echo "root Dockerfile must stay removed; use docker/Dockerfile.app with explicit profile args" >&2
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
import sys

profile = sys.argv[1]
compose_file = sys.argv[2]
config = json.loads(os.environ["COMPOSE_CONFIG_JSON"])
services = config.get("services", {})
declared_volumes = set((config.get("volumes") or {}).keys())


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
