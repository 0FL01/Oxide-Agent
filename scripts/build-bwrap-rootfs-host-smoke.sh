#!/usr/bin/env bash
set -euo pipefail

image_id="host-smoke-dev"
output=".oxide/sandbox/images/host-smoke-dev"

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/build-bwrap-rootfs-host-smoke.sh [--image-id host-smoke-dev] [--output .oxide/sandbox/images/host-smoke-dev]

Builds a tiny host-derived rootfs for local Bubblewrap smoke tests when
mmdebstrap/debootstrap are unavailable. This is not a production image and does
not certify Debian 13 rootfs behavior. It copies only /bin/sh, /bin/cat, their
runtime libraries, and minimal metadata required by scripts/smoke-bwrap.sh.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --image-id)
      image_id="${2:?missing --image-id value}"
      shift 2
      ;;
    --output)
      output="${2:?missing --output value}"
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
done

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "host-smoke bwrap rootfs can only be built on Linux hosts" >&2
  exit 1
fi

rootfs="${output%/}/rootfs"
if [[ -e "$rootfs" ]]; then
  echo "refusing to overwrite existing rootfs: $rootfs" >&2
  exit 1
fi

required_bins=(/bin/sh /bin/cat)
for bin in "${required_bins[@]}"; do
  if [[ ! -x "$bin" ]]; then
    echo "required executable not found: $bin" >&2
    exit 1
  fi
done

mkdir -p "$rootfs/bin" "$rootfs/etc" "$rootfs/proc" "$rootfs/dev" "$rootfs/tmp" "$rootfs/workspace"
chmod 1777 "$rootfs/tmp"

copy_path() {
  local source="$1"
  local dest="$rootfs$source"
  mkdir -p "$(dirname "$dest")"
  cp -L "$source" "$dest"
}

for bin in "${required_bins[@]}"; do
  copy_path "$bin"
done

for bin in "${required_bins[@]}"; do
  ldd "$bin" \
    | awk '{ for (i = 1; i <= NF; i++) if (substr($i, 1, 1) == "/") print $i }'
done | sort -u | while read -r lib; do
  copy_path "$lib"
done

created_at="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"

cat >"$rootfs/etc/os-release" <<EOF
NAME="Oxide host-derived bwrap smoke rootfs"
ID=oxide-host-smoke
PRETTY_NAME="Oxide host-derived bwrap smoke rootfs"
EOF

cat >"${output%/}/image.json" <<JSON
{
  "schema_version": 1,
  "id": "$image_id",
  "distro": "host-derived-smoke",
  "arch": "$(uname -m)",
  "rootfs": "rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/workspace",
  "package_manager": null,
  "default_env": {
    "HOME": "/workspace",
    "PATH": "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    "LANG": "C.UTF-8",
    "TMPDIR": "/tmp"
  },
  "provenance": {
    "builder": "scripts/build-bwrap-rootfs-host-smoke.sh",
    "source": "host executables and libraries",
    "created_at": "$created_at"
  }
}
JSON

cat >"${output%/}/provenance.json" <<JSON
{
  "builder": "scripts/build-bwrap-rootfs-host-smoke.sh",
  "source": "host executables and libraries",
  "created_at": "$created_at",
  "host_arch": "$(uname -m)",
  "host_os_release": "/etc/os-release"
}
JSON

(
  cd "$output"
  sha256sum image.json provenance.json >checksums.txt
  find rootfs -xdev -type f -print0 | sort -z | xargs -0 sha256sum >>checksums.txt
)

echo "built host-derived bwrap smoke rootfs at $output"
