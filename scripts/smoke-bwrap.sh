#!/usr/bin/env bash
set -euo pipefail

image_id="${1:-${BWRAP_IMAGE:-debian-13-dev}}"
image_store="${BWRAP_IMAGE_STORE:-.oxide/sandbox/images}"
state_root="${BWRAP_STATE_DIR:-.oxide/sandbox/scopes}"
lock_root="${BWRAP_LOCK_DIR:-.oxide/sandbox/locks}"
bwrap_bin="${BWRAP_BIN:-bwrap}"
net="${BWRAP_NET:-host}"
root_mode="${BWRAP_ROOT_MODE:-overlay-rw}"

if ! command -v "$bwrap_bin" >/dev/null 2>&1; then
  echo "bwrap not found. Install bubblewrap or set BWRAP_BIN=/path/to/bwrap" >&2
  exit 1
fi

if [[ -n "${BWRAP_ROOTFS:-}" ]]; then
  rootfs="$BWRAP_ROOTFS"
else
  rootfs="${image_store%/}/${image_id}/rootfs"
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

scope="smoke-${image_id}-$$"
scope_dir="${state_root%/}/$scope"
workspace="$scope_dir/workspace"
upper="$scope_dir/system/upper"
work="$scope_dir/system/work/current"
result_dir=".oxide/sandbox/smoke"
result_file="$result_dir/${scope}.json"

mkdir -p "$workspace" "$upper" "$work" "$lock_root" "$result_dir"

args=(
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
  args+=(--unshare-net)
elif [[ "$net" != "host" ]]; then
  echo "invalid BWRAP_NET='$net'. Valid values: host, none." >&2
  exit 1
fi

if [[ "$root_mode" == "overlay-rw" ]]; then
  args+=(--overlay-src "$rootfs" --overlay "$upper" "$work" /)
elif [[ "$root_mode" == "ro" ]]; then
  args+=(--ro-bind "$rootfs" /)
else
  echo "invalid BWRAP_ROOT_MODE='$root_mode'. Valid values: overlay-rw, ro." >&2
  exit 1
fi

args+=(
  --proc /proc
  --dev /dev
  --tmpfs /tmp
  --bind "$workspace" /workspace
  --chdir /workspace
)

if "$bwrap_bin" --help 2>/dev/null | grep -q -- "--disable-userns"; then
  args+=(--disable-userns)
fi

set +e
"$bwrap_bin" "${args[@]}" /bin/sh -lc 'cat /etc/os-release >/workspace/os-release.txt && echo ok >/workspace/ok.txt && pwd >/workspace/pwd.txt && { test ! -S /var/run/docker.sock && echo pass || echo fail; } >/workspace/docker-socket-absent.txt && { test ! -e /run/sandboxd && echo pass || echo fail; } >/workspace/sandboxd-absent.txt'
status=$?
set -e

tests_create_scope=pass
tests_exec_command=fail
tests_workspace_persistence=fail
tests_docker_socket_absent=fail
tests_sandboxd_absent=fail
if [[ "$status" -eq 0 ]]; then
  tests_exec_command=pass
fi
if [[ -f "$workspace/ok.txt" && "$(cat "$workspace/ok.txt")" == "ok" ]]; then
  tests_workspace_persistence=pass
fi
if [[ -f "$workspace/docker-socket-absent.txt" && "$(cat "$workspace/docker-socket-absent.txt")" == "pass" ]]; then
  tests_docker_socket_absent=pass
fi
if [[ -f "$workspace/sandboxd-absent.txt" && "$(cat "$workspace/sandboxd-absent.txt")" == "pass" ]]; then
  tests_sandboxd_absent=pass
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
  "exit_status": $status,
  "tests": {
    "create_scope": "$tests_create_scope",
    "exec_command": "$tests_exec_command",
    "workspace_persistence": "$tests_workspace_persistence",
    "docker_socket_absent": "$tests_docker_socket_absent",
    "sandboxd_socket_absent": "$tests_sandboxd_absent"
  }
}
JSON

cat "$result_file"
exit "$status"
