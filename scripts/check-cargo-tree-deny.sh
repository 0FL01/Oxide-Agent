#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'USAGE'
Usage:
  scripts/check-cargo-tree-deny.sh <feature-or-profile> [crate ...]

Examples:
  scripts/check-cargo-tree-deny.sh profile-no-sandbox
  scripts/check-cargo-tree-deny.sh profile-search-only
  scripts/check-cargo-tree-deny.sh profile-web-embedded-opencode-local
  scripts/check-cargo-tree-deny.sh llm-opencode-go

The first argument is passed to cargo as --features for oxide-agent-core.
Additional arguments override the deny list. Without overrides, the script uses
the repository deny list for known profiles/features.
USAGE
}

if [[ $# -lt 1 || "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    usage
    exit 2
fi

features="$1"
shift

deny=("$@")
if [[ ${#deny[@]} -eq 0 ]]; then
    case "$features" in
        profile-no-sandbox)
            deny=(bollard rmcp bincode serde_bytes tar tavily)
            ;;
        sandbox-backend-sandboxd-client)
            deny=(bollard tar http-body-util)
            ;;
        sandbox-backend-bwrap)
            deny=(bollard tar http-body-util bincode serde_bytes)
            ;;
        profile-host-bwrap)
            # The host profile intentionally includes reqwest-backed LLM/search/media
            # modules, and reqwest pulls http-body-util transitively. The backend-only
            # check above still proves bwrap itself does not enable Docker's direct
            # http-body-util dependency path.
            deny=(bollard tar bincode serde_bytes)
            ;;
        profile-search-only)
            deny=(bollard rmcp bincode serde_bytes tar zai-rs async-openai claudius)
            ;;
        profile-lite)
            deny=(bollard rmcp bincode serde_bytes tar zai-rs async-openai claudius tavily)
            ;;
        profile-embedded-opencode-local)
            deny=(rmcp zai-rs async-openai claudius)
            ;;
        profile-web-embedded-opencode-local)
            deny=(rmcp zai-rs async-openai claudius duckduckgo scraper)
            ;;
        profile-media-enabled)
            deny=(bollard rmcp bincode serde_bytes tar zai-rs async-openai claudius tavily htmd)
            ;;
        llm-opencode-go)
            deny=(zai-rs async-openai claudius aws-sdk-s3 aws-config aws-credential-types aws-types bollard rmcp htmd tavily)
            ;;
        *)
            echo "No default deny list for feature/profile '$features'." >&2
            echo "Pass explicit crate names to check." >&2
            exit 2
            ;;
    esac
fi

tmp_tree="$(mktemp)"
trap 'rm -f "$tmp_tree"' EXIT

cargo tree -p oxide-agent-core --no-default-features --features "$features" >"$tmp_tree"

failed=0
for crate in "${deny[@]}"; do
    if grep -Eq "(^|[[:space:]])${crate//+/\\+} v[0-9]" "$tmp_tree"; then
        echo "dependency leak: '$crate' is present for features '$features'" >&2
        failed=1
    fi
done

if [[ "$failed" -ne 0 ]]; then
    echo "cargo tree deny check failed for features '$features'" >&2
    exit 1
fi

echo "cargo tree deny check passed for features '$features'"
