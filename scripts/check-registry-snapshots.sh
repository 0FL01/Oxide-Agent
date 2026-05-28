#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: scripts/check-registry-snapshots.sh <profile>

Runs deterministic modular registry snapshot tests for a profile.

Profiles:
  embedded-opencode-local
  web-embedded-opencode-local
  lite
  search-only
  no-sandbox
  media-enabled
  host-bwrap
  full
USAGE
}

profile="${1:-}"
if [[ -z "${profile}" ]]; then
  usage
  exit 2
fi

case "${profile}" in
  embedded-opencode-local)
    features="profile-embedded-opencode-local"
    ;;
  web-embedded-opencode-local)
    features="profile-web-embedded-opencode-local"
    ;;
  lite)
    features="profile-lite"
    ;;
  search-only)
    features="profile-search-only"
    ;;
  no-sandbox)
    features="profile-no-sandbox"
    ;;
  media-enabled)
    features="profile-media-enabled"
    ;;
  host-bwrap)
    features="profile-host-bwrap"
    ;;
  full)
    features="profile-full"
    ;;
  *)
    echo "unknown registry snapshot profile '${profile}'" >&2
    usage
    exit 2
    ;;
esac

INSTA_UPDATE="${INSTA_UPDATE:-no}" cargo test \
  -p oxide-agent-core \
  --test modular_registry_snapshots \
  --no-default-features \
  --features "${features}" \
  modular_registry_snapshot_covers_manifest_and_tool_lists \
  -- --exact
