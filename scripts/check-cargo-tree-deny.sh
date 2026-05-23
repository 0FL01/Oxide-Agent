#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat >&2 <<'USAGE'
Usage:
  scripts/check-cargo-tree-deny.sh <feature-or-profile> [crate ...]

Examples:
  scripts/check-cargo-tree-deny.sh profile-no-sandbox
  scripts/check-cargo-tree-deny.sh profile-search-only
  scripts/check-cargo-tree-deny.sh llm-opencode-go

The first argument is passed to cargo as --features for oxide-agent-core.
Additional arguments override the deny list. Without overrides, the script uses
the PRD Milestone 1 deny list for known profiles/features.
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
            deny=(bollard rmcp bincode serde_bytes tar)
            ;;
        profile-search-only)
            deny=(bollard rmcp bincode serde_bytes tar gemini-rust zai-rs async-openai claudius)
            ;;
        profile-lite | profile-embedded-opencode-local)
            deny=(bollard rmcp bincode serde_bytes tar gemini-rust zai-rs async-openai claudius)
            ;;
        llm-opencode-go)
            deny=(gemini-rust zai-rs async-openai claudius aws-sdk-s3 aws-config aws-credential-types aws-types bollard rmcp)
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
