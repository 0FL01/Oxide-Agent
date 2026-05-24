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
config="$(docker compose -f "${compose_file}" config)"

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
