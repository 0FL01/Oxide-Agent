#!/usr/bin/env bash
set -euo pipefail

image_id="${1:-${BWRAP_IMAGE:-debian-13-dev}}"
image_store="${BWRAP_IMAGE_STORE:-.oxide/sandbox/images}"
state_root="${BWRAP_STATE_DIR:-.oxide/sandbox/scopes}"
lock_root="${BWRAP_LOCK_DIR:-.oxide/sandbox/locks}"
bwrap_bin="${BWRAP_BIN:-bwrap}"
net="${BWRAP_NET:-host}"
root_mode="${BWRAP_ROOT_MODE:-overlay-rw}"
package_tests="${BWRAP_SMOKE_PACKAGE_TESTS:-auto}"

case "$package_tests" in
  auto | required | skip) ;;
  *)
    echo "invalid BWRAP_SMOKE_PACKAGE_TESTS='$package_tests'. Valid values: auto, required, skip." >&2
    exit 2
    ;;
esac

if ! command -v "$bwrap_bin" >/dev/null 2>&1; then
  echo "bwrap not found. Install bubblewrap or set BWRAP_BIN=/path/to/bwrap" >&2
  exit 1
fi

if [[ -n "${BWRAP_ROOTFS:-}" ]]; then
  rootfs="$BWRAP_ROOTFS"
  manifest_path="$(dirname "$rootfs")/image.json"
else
  rootfs="${image_store%/}/${image_id}/rootfs"
  manifest_path="${image_store%/}/${image_id}/image.json"
fi

if [[ ! -d "$rootfs" ]]; then
  echo "rootfs not found: $rootfs" >&2
  echo "Run scripts/build-bwrap-rootfs-debian.sh or set BWRAP_ROOTFS." >&2
  exit 1
fi

for required in bin/sh proc dev tmp workspace; do
  if [[ ! -e "${rootfs%/}/$required" ]]; then
    echo "rootfs is missing /$required: $rootfs" >&2
    exit 1
  fi
done

package_manager=""
if [[ -f "$manifest_path" ]]; then
  package_manager="$(sed -n 's/.*"package_manager"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$manifest_path" | head -n1)"
fi

scope="smoke-${image_id}-$$"
scope_dir="${state_root%/}/$scope"
workspace="$scope_dir/workspace"
upper="$scope_dir/system/upper"
work_root="$scope_dir/system/work"
result_dir=".oxide/sandbox/smoke"
result_file="$result_dir/${scope}.json"

mkdir -p "$workspace" "$upper" "$work_root" "$lock_root" "$result_dir"

base_args=(
  --unshare-user
  --uid 0
  --gid 0
  --unshare-pid
  --unshare-ipc
  --unshare-uts
  --unshare-cgroup-try
  --die-with-parent
  --new-session
  --clearenv
  --setenv HOME /workspace
  --setenv PATH /usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
  --setenv LANG C.UTF-8
  --setenv TMPDIR /tmp
)

if [[ "$net" == "none" ]]; then
  base_args+=(--unshare-net)
elif [[ "$net" != "host" ]]; then
  echo "invalid BWRAP_NET='$net'. Valid values: host, none." >&2
  exit 1
fi

post_args=(
  --proc /proc
  --dev /dev
  --tmpfs /tmp
  --bind "$workspace" /workspace
  --chdir /workspace
)

if "$bwrap_bin" --help 2>/dev/null | grep -q -- "--disable-userns"; then
  post_args+=(--disable-userns)
fi

build_run_args() {
  local work_dir="$1"
  run_args=("${base_args[@]}")
  if [[ "$root_mode" == "overlay-rw" ]]; then
    run_args+=(--overlay-src "$rootfs" --overlay "$upper" "$work_dir" /)
  elif [[ "$root_mode" == "ro" ]]; then
    run_args+=(--ro-bind "$rootfs" /)
  else
    echo "invalid BWRAP_ROOT_MODE='$root_mode'. Valid values: overlay-rw, ro." >&2
    exit 1
  fi
  run_args+=("${post_args[@]}")
}

run_bwrap_command() {
  local name="$1"
  local command="$2"
  local work_dir="${work_root%/}/$name"
  local restore_errexit=false
  case "$-" in
    *e*) restore_errexit=true ;;
  esac
  rm -rf "$work_dir"
  mkdir -p "$work_dir"
  build_run_args "$work_dir"
  set +e
  "$bwrap_bin" "${run_args[@]}" /bin/sh -lc "$command"
  local status=$?
  rm -rf "$work_dir"
  if [[ "$restore_errexit" == "true" ]]; then
    set -e
  fi
  return "$status"
}

run_bwrap_timeout_command() {
  local name="$1"
  local command="$2"
  if ! command -v timeout >/dev/null 2>&1; then
    return 125
  fi
  local restore_errexit=false
  case "$-" in
    *e*) restore_errexit=true ;;
  esac
  local work_dir="${work_root%/}/$name"
  rm -rf "$work_dir"
  mkdir -p "$work_dir"
  build_run_args "$work_dir"
  set +e
  timeout -k 1 1 "$bwrap_bin" "${run_args[@]}" /bin/sh -lc "$command"
  local status=$?
  rm -rf "$work_dir"
  if [[ "$restore_errexit" == "true" ]]; then
    set -e
  fi
  return "$status"
}

tests_create_scope=pass
tests_exec_command=fail
tests_workspace_persistence=fail
tests_system_overlay_persistence=skipped
tests_docker_socket_absent=fail
tests_sandboxd_absent=fail
tests_apt_or_apk_install=skipped
tests_pip_install=skipped
tests_apply_file_edit=fail
tests_path_escape_rejected=covered_by_rust_tests
tests_timeout=fail
tests_destroy_scope=fail
overall_status=0

basic_command='cat /etc/os-release >/workspace/os-release.txt && echo ok >/workspace/ok.txt && pwd >/workspace/pwd.txt && { test ! -S /var/run/docker.sock && echo pass || echo fail; } >/workspace/docker-socket-absent.txt && { test ! -e /run/sandboxd && echo pass || echo fail; } >/workspace/sandboxd-absent.txt'
if [[ "$root_mode" == "overlay-rw" ]]; then
  basic_command="$basic_command && printf system >/etc/oxide-bwrap-smoke"
fi

if run_bwrap_command basic "$basic_command"; then
  tests_exec_command=pass
else
  overall_status=1
fi

if run_bwrap_command persistence 'test "$(cat /workspace/ok.txt)" = ok'; then
  tests_workspace_persistence=pass
else
  overall_status=1
fi

if [[ "$root_mode" == "overlay-rw" ]]; then
  if run_bwrap_command system-persistence 'test "$(cat /etc/oxide-bwrap-smoke)" = system'; then
    tests_system_overlay_persistence=pass
  else
    overall_status=1
    tests_system_overlay_persistence=fail
  fi
fi

if [[ -f "$workspace/docker-socket-absent.txt" && "$(cat "$workspace/docker-socket-absent.txt")" == "pass" ]]; then
  tests_docker_socket_absent=pass
else
  overall_status=1
fi
if [[ -f "$workspace/sandboxd-absent.txt" && "$(cat "$workspace/sandboxd-absent.txt")" == "pass" ]]; then
  tests_sandboxd_absent=pass
else
  overall_status=1
fi

set +e
run_bwrap_command apply-edit 'for tool in sha256sum awk grep wc sed; do command -v "$tool" >/dev/null 2>&1 || exit 125; done && printf "alpha\nbeta\n" >/workspace/edit.txt && before="$(sha256sum /workspace/edit.txt | awk '"'"'{print $1}'"'"')" && matches="$(grep -o beta /workspace/edit.txt | wc -l)" && test "$matches" -eq 1 && sed "s/beta/gamma/" /workspace/edit.txt >/workspace/edit.txt.tmp && mv /workspace/edit.txt.tmp /workspace/edit.txt && after="$(sha256sum /workspace/edit.txt | awk '"'"'{print $1}'"'"')" && test "$before" != "$after" && test "$(sed -n 2p /workspace/edit.txt)" = gamma'
apply_edit_status=$?
set -e
if [[ "$apply_edit_status" -eq 0 ]]; then
  tests_apply_file_edit=pass
elif [[ "$apply_edit_status" -eq 125 ]]; then
  tests_apply_file_edit=skipped
else
  overall_status=1
fi

set +e
run_bwrap_timeout_command timeout 'while :; do :; done'
timeout_status=$?
set -e
if [[ "$timeout_status" -eq 124 || "$timeout_status" -eq 137 || "$timeout_status" -eq 143 ]]; then
  tests_timeout=pass
elif [[ "$timeout_status" -eq 125 ]]; then
  tests_timeout=skipped
else
  overall_status=1
fi

should_run_package_tests=false
if [[ "$package_tests" == "required" ]]; then
  should_run_package_tests=true
elif [[ "$package_tests" == "auto" && "$root_mode" == "overlay-rw" && "$net" == "host" && ( "$package_manager" == "apt" || "$package_manager" == "apk" ) ]]; then
  should_run_package_tests=true
fi

if [[ "$should_run_package_tests" == "true" ]]; then
  if [[ "$package_manager" == "apt" ]]; then
    package_command='export DEBIAN_FRONTEND=noninteractive; apt-get -o APT::Sandbox::User=root update && apt-get -o APT::Sandbox::User=root install -y bc && test "$(printf "2+2\n" | bc)" = 4 && bc --version >/workspace/package-tool.txt'
  elif [[ "$package_manager" == "apk" ]]; then
    package_command='apk add --no-cache bc && test "$(printf "2+2\n" | bc)" = 4 && bc --version >/workspace/package-tool.txt'
  else
    package_command='exit 42'
  fi

  if run_bwrap_command package-install "$package_command"; then
    tests_apt_or_apk_install=pass
  else
    tests_apt_or_apk_install=fail
    overall_status=1
  fi

  if run_bwrap_command pip-install 'if command -v python3 >/dev/null 2>&1 && python3 -m pip --version >/dev/null 2>&1; then rm -rf /workspace/pip-smoke && python3 -m pip install --no-cache-dir --target /workspace/pip-smoke idna && PYTHONPATH=/workspace/pip-smoke python3 -c "import idna; print(idna.__version__)" >/workspace/pip-smoke.txt; else exit 42; fi'; then
    tests_pip_install=pass
  else
    tests_pip_install=fail
    overall_status=1
  fi
elif [[ "$package_tests" == "required" ]]; then
  tests_apt_or_apk_install=fail
  tests_pip_install=fail
  overall_status=1
fi

nested=false
environment_kind="bare-host"
if [[ -f /.dockerenv ]] || grep -qaE '(docker|containerd|kubepods)' /proc/1/cgroup 2>/dev/null; then
  nested=true
  environment_kind="docker-container"
fi
if grep -qaE 'kubepods' /proc/1/cgroup 2>/dev/null; then
  environment_kind="kubernetes-container"
fi

rm -rf "$scope_dir"
if [[ ! -e "$scope_dir" ]]; then
  tests_destroy_scope=pass
else
  overall_status=1
fi

cat >"$result_file" <<JSON
{
  "platform": "$(uname -s)-$(uname -m)",
  "rootfs": "$image_id",
  "backend": "bwrap",
  "environment_kind": "$environment_kind",
  "nested": $nested,
  "bwrap_version": "$("$bwrap_bin" --version 2>/dev/null | head -n1)",
  "root_mode": "$root_mode",
  "network_mode": "$net",
  "package_manager": "${package_manager:-unknown}",
  "package_tests": "$package_tests",
  "exit_status": $overall_status,
  "tests": {
    "create_scope": "$tests_create_scope",
    "exec_command": "$tests_exec_command",
    "workspace_persistence": "$tests_workspace_persistence",
    "system_overlay_persistence": "$tests_system_overlay_persistence",
    "apt_or_apk_install": "$tests_apt_or_apk_install",
    "pip_install": "$tests_pip_install",
    "apply_file_edit": "$tests_apply_file_edit",
    "path_escape_rejected": "$tests_path_escape_rejected",
    "timeout": "$tests_timeout",
    "docker_socket_absent": "$tests_docker_socket_absent",
    "sandboxd_socket_absent": "$tests_sandboxd_absent",
    "destroy_scope": "$tests_destroy_scope"
  }
}
JSON

cat "$result_file"
exit "$overall_status"
