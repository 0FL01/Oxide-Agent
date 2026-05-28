#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/check-profile-size-budget.sh <embedded-opencode-local|web-embedded-opencode-local|full> [binary|image|metrics|all]

Checks configured release binary, Docker image, and profile metric budgets for
selected modular architecture profiles. Budgets can be overridden with the env
vars printed on failure.
USAGE
}

if [[ $# -lt 1 || $# -gt 2 || "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 2
fi

profile="$1"
mode="${2:-binary}"

case "${mode}" in
  binary | image | metrics | all) ;;
  *)
    echo "unknown size budget mode '${mode}'" >&2
    usage
    exit 2
    ;;
esac

declare -a packages
declare -a binaries
declare -a high_risk_dependencies
entrypoint_binary="oxide-agent-telegram-bot"
runtime_apt_packages=""
mcp_binaries=""
build_web_ui="false"

case "${profile}" in
  embedded-opencode-local)
    cargo_features="oxide-agent-telegram-bot/profile-embedded-opencode-local"
    manifest_features="oxide-agent-telegram-bot/profile-embedded-opencode-local"
    packages=(oxide-agent-telegram-bot)
    binaries=(oxide-agent-telegram-bot)
    ;;
  web-embedded-opencode-local)
    cargo_features="oxide-agent-transport-web/profile-web-embedded-opencode-local"
    manifest_features="oxide-agent-transport-web/profile-web-embedded-opencode-local"
    packages=(oxide-agent-transport-web)
    binaries=(oxide-agent-web-console)
    entrypoint_binary="oxide-agent-web-console"
    build_web_ui="true"
    ;;
  full)
    cargo_features="oxide-agent-telegram-bot/profile-full,oxide-agent-sandboxd/profile-full"
    manifest_features="oxide-agent-telegram-bot/profile-full"
    packages=(oxide-agent-telegram-bot oxide-agent-sandboxd)
    binaries=(oxide-agent-telegram-bot oxide-agent-sandboxd)
    mcp_binaries="ssh-mcp jira-mcp mattermost-mcp"
    ;;
  *)
    echo "unknown size budget profile '${profile}'" >&2
    usage
    exit 2
    ;;
esac

high_risk_dependencies=(
  aws-sdk-s3
  aws-config
  aws-credential-types
  aws-types
  bollard
  rmcp
  bincode
  serde_bytes
  tar
  zai-rs
  async-openai
  claudius
  htmd
  reqwest
  teloxide
)

profile_env="${profile^^}"
profile_env="${profile_env//-/_}"

join_by_space() {
  local joined=""
  local item
  for item in "$@"; do
    if [[ -z "${joined}" ]]; then
      joined="${item}"
    else
      joined="${joined} ${item}"
    fi
  done
  printf '%s' "${joined}"
}

human_bytes() {
  python3 - "$1" <<'PY'
import sys

value = int(sys.argv[1])
units = ("B", "KiB", "MiB", "GiB")
size = float(value)
for unit in units:
    if size < 1024 or unit == units[-1]:
        print(f"{size:.1f} {unit}" if unit != "B" else f"{value} B")
        break
    size /= 1024
PY
}

binary_env_name() {
  local binary_env="${1^^}"
  binary_env="${binary_env//-/_}"
  printf 'OXIDE_SIZE_BUDGET_%s_%s_BYTES' "${profile_env}" "${binary_env}"
}

default_binary_budget() {
  case "${profile}:$1" in
    embedded-opencode-local:oxide-agent-telegram-bot) echo 70000000 ;;
    web-embedded-opencode-local:oxide-agent-web-console) echo 70000000 ;;
    full:oxide-agent-telegram-bot) echo 90000000 ;;
    full:oxide-agent-sandboxd) echo 20000000 ;;
    *) echo 90000000 ;;
  esac
}

check_budget() {
  local label="$1"
  local actual="$2"
  local budget="$3"
  local env_name="$4"

  if (( actual > budget )); then
    echo "size budget exceeded for ${label}: actual $(human_bytes "${actual}") > budget $(human_bytes "${budget}")" >&2
    echo "override with ${env_name}=${actual} after intentionally accepting the new baseline" >&2
    exit 1
  fi

  echo "size budget passed for ${label}: actual $(human_bytes "${actual}") <= budget $(human_bytes "${budget}")"
}

check_count_budget() {
  local label="$1"
  local actual="$2"
  local budget="$3"
  local env_name="$4"

  if (( actual > budget )); then
    echo "count budget exceeded for ${label}: actual ${actual} > budget ${budget}" >&2
    echo "override with ${env_name}=${actual} after intentionally accepting the new baseline" >&2
    exit 1
  fi

  echo "count budget passed for ${label}: actual ${actual} <= budget ${budget}"
}

check_binary_budgets() {
  local package_args=()
  local package
  for package in "${packages[@]}"; do
    package_args+=("-p" "${package}")
  done

  cargo build --release --no-default-features "${package_args[@]}" --features "${cargo_features}"

  local binary env_name default_budget budget actual path
  for binary in "${binaries[@]}"; do
    path="target/release/${binary}"
    if [[ ! -x "${path}" ]]; then
      echo "expected release binary '${path}' to exist after build" >&2
      exit 1
    fi

    actual="$(stat -c '%s' "${path}")"
    env_name="$(binary_env_name "${binary}")"
    default_budget="$(default_binary_budget "${binary}")"
    budget="${!env_name:-${default_budget}}"
    check_budget "${profile}/${binary}" "${actual}" "${budget}" "${env_name}"
  done
}

metric_env_name() {
  local metric_env="${1^^}"
  metric_env="${metric_env//-/_}"
  printf 'OXIDE_SIZE_BUDGET_%s_%s' "${profile_env}" "${metric_env}"
}

default_metric_budget() {
  case "${profile}:$1" in
    embedded-opencode-local:MODULES) echo 19 ;;
    embedded-opencode-local:CAPABILITIES) echo 25 ;;
    embedded-opencode-local:DEPENDENCIES) echo 400 ;;
    embedded-opencode-local:HIGH_RISK_DEPENDENCIES) echo 11 ;;
    web-embedded-opencode-local:MODULES) echo 19 ;;
    web-embedded-opencode-local:CAPABILITIES) echo 25 ;;
    web-embedded-opencode-local:DEPENDENCIES) echo 400 ;;
    web-embedded-opencode-local:HIGH_RISK_DEPENDENCIES) echo 11 ;;
    full:MODULES) echo 39 ;;
    full:CAPABILITIES) echo 53 ;;
    full:DEPENDENCIES) echo 470 ;;
    full:HIGH_RISK_DEPENDENCIES) echo 15 ;;
    *) echo 500 ;;
  esac
}

check_profile_metric_budgets() {
  local tmp_manifest tmp_tree
  tmp_manifest="$(mktemp)"
  tmp_tree="$(mktemp)"
  trap 'rm -f "${tmp_manifest}" "${tmp_tree}"' RETURN

  cargo run -q \
    -p oxide-agent-telegram-bot \
    --bin oxide-agent-telegram-bot \
    --no-default-features \
    --features "${manifest_features}" \
    -- capabilities --compiled --json >"${tmp_manifest}"

  local module_count capability_count
  read -r module_count capability_count < <(
    python3 - "${tmp_manifest}" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as fh:
    manifest = json.load(fh)
print(len(manifest["modules"]), len(manifest["capabilities"]))
PY
  )

  cargo tree --workspace --no-default-features --features "${cargo_features}" --prefix none >"${tmp_tree}"

  local dependency_count high_risk_count
  dependency_count="$(
    sed -n 's/^\([A-Za-z0-9_.-][A-Za-z0-9_.-]*\) v[0-9].*/\1/p' "${tmp_tree}" \
      | sort -u \
      | wc -l \
      | tr -d '[:space:]'
  )"

  high_risk_count=0
  local dependency
  for dependency in "${high_risk_dependencies[@]}"; do
    if grep -Eq "^${dependency//+/\\+} v[0-9]" "${tmp_tree}"; then
      high_risk_count=$((high_risk_count + 1))
    fi
  done

  local env_name budget
  env_name="$(metric_env_name MODULES)"
  budget="${!env_name:-$(default_metric_budget MODULES)}"
  check_count_budget "${profile}/compiled-modules" "${module_count}" "${budget}" "${env_name}"

  env_name="$(metric_env_name CAPABILITIES)"
  budget="${!env_name:-$(default_metric_budget CAPABILITIES)}"
  check_count_budget "${profile}/compiled-capabilities" "${capability_count}" "${budget}" "${env_name}"

  env_name="$(metric_env_name DEPENDENCIES)"
  budget="${!env_name:-$(default_metric_budget DEPENDENCIES)}"
  check_count_budget "${profile}/unique-dependencies" "${dependency_count}" "${budget}" "${env_name}"

  env_name="$(metric_env_name HIGH_RISK_DEPENDENCIES)"
  budget="${!env_name:-$(default_metric_budget HIGH_RISK_DEPENDENCIES)}"
  check_count_budget "${profile}/high-risk-dependencies" "${high_risk_count}" "${budget}" "${env_name}"
}

image_env_name() {
  printf 'OXIDE_SIZE_BUDGET_%s_IMAGE_%s_BYTES' "${profile_env}" "$1"
}

default_image_budget() {
  case "${profile}:$1" in
    embedded-opencode-local:UNCOMPRESSED) echo 450000000 ;;
    embedded-opencode-local:COMPRESSED) echo 220000000 ;;
    web-embedded-opencode-local:UNCOMPRESSED) echo 450000000 ;;
    web-embedded-opencode-local:COMPRESSED) echo 220000000 ;;
    full:UNCOMPRESSED) echo 650000000 ;;
    full:COMPRESSED) echo 320000000 ;;
    *) echo 650000000 ;;
  esac
}

check_image_contents() {
  local tag="$1"
  local -a expected_names
  expected_names=("${binaries[@]}")
  if [[ -n "${mcp_binaries}" ]]; then
    local -a mcp_binary_names
    read -r -a mcp_binary_names <<<"${mcp_binaries}"
    expected_names+=("${mcp_binary_names[@]}")
  fi

  local expected actual
  expected="$(printf '%s\n' "${expected_names[@]}" | sort | xargs)"
  actual="$(
    docker run --rm --entrypoint /bin/sh "${tag}" -c \
      "find /app -maxdepth 1 -type f -perm /111 -printf '%f\n' | sort | xargs"
  )"
  if [[ "${actual}" != "${expected}" ]]; then
    echo "image content check failed for ${profile}: executable /app files '${actual}' != expected '${expected}'" >&2
    exit 1
  fi

  docker run --rm --entrypoint /bin/sh "${tag}" -c \
    "test ! -e /app/skills && test ! -e /app/sandbox && test ! -e /app/config"

  docker run --rm --entrypoint /bin/sh "${tag}" -c \
    "/app/${entrypoint_binary} capabilities --compiled --json >/tmp/compiled-capabilities.json && grep -q '\"modules\"' /tmp/compiled-capabilities.json && grep -q '\"capabilities\"' /tmp/compiled-capabilities.json"

  docker run --rm --entrypoint /bin/sh "${tag}" -c \
    "/app/${entrypoint_binary} config example --profile '${profile}' --json >/tmp/config-example.json && grep -q '\"modules\"' /tmp/config-example.json"

  if [[ "${profile}" == "embedded-opencode-local" ]]; then
    docker run --rm --entrypoint /bin/sh "${tag}" -c '
      for tool in ssh scp ffmpeg python3 yt-dlp nmap mtr chromium chromium-browser google-chrome firefox; do
        if command -v "${tool}" >/dev/null 2>&1; then
          echo "unexpected runtime command in embedded image: ${tool}" >&2
          exit 1
        fi
      done
    '
  fi

  echo "image content check passed for ${profile}: ${expected}"
}

check_image_budgets() {
  local tag="oxide-agent:size-${profile}"
  local package_arg binary_arg uncompressed compressed env_name budget
  package_arg="$(join_by_space "${packages[@]}")"
  binary_arg="$(join_by_space "${binaries[@]}")"

  docker build \
    -f docker/Dockerfile.app \
    --build-arg CARGO_FEATURES="${cargo_features}" \
    --build-arg PACKAGES="${package_arg}" \
    --build-arg BINARIES="${binary_arg}" \
    --build-arg MCP_BINARIES="${mcp_binaries}" \
    --build-arg RUNTIME_APT_PACKAGES="${runtime_apt_packages}" \
    --build-arg ENTRYPOINT_BINARY="${entrypoint_binary}" \
    --build-arg BUILD_WEB_UI="${build_web_ui}" \
    -t "${tag}" \
    .

  check_image_contents "${tag}"

  uncompressed="$(docker image inspect "${tag}" --format '{{.Size}}')"
  env_name="$(image_env_name UNCOMPRESSED)"
  budget="${!env_name:-$(default_image_budget UNCOMPRESSED)}"
  check_budget "${profile}/docker-image-uncompressed" "${uncompressed}" "${budget}" "${env_name}"

  compressed="$(docker save "${tag}" | gzip -c | wc -c | tr -d '[:space:]')"
  env_name="$(image_env_name COMPRESSED)"
  budget="${!env_name:-$(default_image_budget COMPRESSED)}"
  check_budget "${profile}/docker-image-compressed" "${compressed}" "${budget}" "${env_name}"
}

case "${mode}" in
  binary)
    check_binary_budgets
    ;;
  image)
    check_image_budgets
    ;;
  metrics)
    check_profile_metric_budgets
    ;;
  all)
    check_binary_budgets
    check_profile_metric_budgets
    check_image_budgets
    ;;
esac
