#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/check-profile-size-budget.sh <embedded-opencode-local|full> [binary|image|all]

Checks configured release binary and Docker image size budgets for selected
modular architecture profiles. Budgets can be overridden with the env vars
printed on failure.
USAGE
}

if [[ $# -lt 1 || $# -gt 2 || "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 2
fi

profile="$1"
mode="${2:-binary}"

case "${mode}" in
  binary | image | all) ;;
  *)
    echo "unknown size budget mode '${mode}'" >&2
    usage
    exit 2
    ;;
esac

declare -a packages
declare -a binaries
entrypoint_binary="oxide-agent-telegram-bot"
runtime_apt_packages=""

case "${profile}" in
  embedded-opencode-local)
    cargo_features="oxide-agent-telegram-bot/profile-embedded-opencode-local"
    packages=(oxide-agent-telegram-bot)
    binaries=(oxide-agent-telegram-bot)
    ;;
  full)
    cargo_features="oxide-agent-telegram-bot/profile-full,oxide-agent-sandboxd/profile-full"
    packages=(oxide-agent-telegram-bot oxide-agent-sandboxd)
    binaries=(oxide-agent-telegram-bot oxide-agent-sandboxd)
    ;;
  *)
    echo "unknown size budget profile '${profile}'" >&2
    usage
    exit 2
    ;;
esac

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

image_env_name() {
  printf 'OXIDE_SIZE_BUDGET_%s_IMAGE_%s_BYTES' "${profile_env}" "$1"
}

default_image_budget() {
  case "${profile}:$1" in
    embedded-opencode-local:UNCOMPRESSED) echo 450000000 ;;
    embedded-opencode-local:COMPRESSED) echo 220000000 ;;
    full:UNCOMPRESSED) echo 650000000 ;;
    full:COMPRESSED) echo 320000000 ;;
    *) echo 650000000 ;;
  esac
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
    --build-arg RUNTIME_APT_PACKAGES="${runtime_apt_packages}" \
    --build-arg ENTRYPOINT_BINARY="${entrypoint_binary}" \
    -t "${tag}" \
    .

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
  all)
    check_binary_budgets
    check_image_budgets
    ;;
esac
