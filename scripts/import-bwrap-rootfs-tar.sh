#!/usr/bin/env bash
set -euo pipefail

tarball=""
sha256=""
image_id="debian-13-dev"
output=".oxide/sandbox/images/debian-13-dev"
distro="debian"
suite="trixie"
version="13"
package_manager="apt"
strip_components="0"

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/import-bwrap-rootfs-tar.sh \
    --tarball /path/to/rootfs.tar.zst \
    --sha256 <expected-sha256> \
    [--image-id debian-13-dev] \
    [--output .oxide/sandbox/images/debian-13-dev] \
    [--distro debian] [--suite trixie] [--version 13] \
    [--package-manager apt] [--strip-components 0]

Imports a prebuilt rootfs tarball for SANDBOX_BACKEND=bwrap without Docker.
The tarball checksum is mandatory. The output directory will contain rootfs/,
image.json, checksums.txt, and provenance.json.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tarball)
      tarball="${2:?missing --tarball value}"
      shift 2
      ;;
    --sha256)
      sha256="${2:?missing --sha256 value}"
      shift 2
      ;;
    --image-id)
      image_id="${2:?missing --image-id value}"
      shift 2
      ;;
    --output)
      output="${2:?missing --output value}"
      shift 2
      ;;
    --distro)
      distro="${2:?missing --distro value}"
      shift 2
      ;;
    --suite)
      suite="${2:?missing --suite value}"
      shift 2
      ;;
    --version)
      version="${2:?missing --version value}"
      shift 2
      ;;
    --package-manager)
      package_manager="${2:?missing --package-manager value}"
      shift 2
      ;;
    --strip-components)
      strip_components="${2:?missing --strip-components value}"
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

if [[ -z "$tarball" || -z "$sha256" ]]; then
  usage
  exit 2
fi

if [[ ! -f "$tarball" ]]; then
  echo "rootfs tarball not found: $tarball" >&2
  exit 1
fi

case "$strip_components" in
  '' | *[!0-9]*)
    echo "--strip-components must be a non-negative integer" >&2
    exit 2
    ;;
esac

actual_sha256="$(sha256sum "$tarball" | awk '{print $1}')"
if [[ "$actual_sha256" != "$sha256" ]]; then
  echo "checksum mismatch for $tarball" >&2
  echo "expected: $sha256" >&2
  echo "actual:   $actual_sha256" >&2
  exit 1
fi

arch="$(uname -m)"
case "$arch" in
  x86_64 | aarch64) ;;
  *)
    echo "unsupported host arch for MVP bwrap rootfs: $arch" >&2
    exit 1
    ;;
esac

rootfs="${output%/}/rootfs"
if [[ -e "$rootfs" ]]; then
  echo "refusing to overwrite existing rootfs: $rootfs" >&2
  exit 1
fi

mkdir -p "$rootfs"

tar_args=(
  --extract
  --file "$tarball"
  --directory "$rootfs"
  --numeric-owner
)
if [[ "$strip_components" != "0" ]]; then
  tar_args+=(--strip-components "$strip_components")
fi
tar "${tar_args[@]}"

install -d -m 0755 "$rootfs/proc" "$rootfs/dev" "$rootfs/workspace"
install -d -m 1777 "$rootfs/tmp"

shell_path="$rootfs/bin/sh"
if [[ ! -f "$shell_path" ]]; then
  echo "imported rootfs is missing /bin/sh" >&2
  exit 1
fi

created_at="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
cat >"${output%/}/image.json" <<JSON
{
  "schema_version": 1,
  "id": "$image_id",
  "distro": "$distro",
  "suite": "$suite",
  "version": "$version",
  "arch": "$arch",
  "rootfs": "rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/workspace",
  "package_manager": "$package_manager",
  "default_env": {
    "HOME": "/workspace",
    "PATH": "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    "LANG": "C.UTF-8",
    "TMPDIR": "/tmp"
  },
  "provenance": {
    "builder": "scripts/import-bwrap-rootfs-tar.sh",
    "source": "$tarball",
    "source_sha256": "$sha256",
    "created_at": "$created_at"
  }
}
JSON

cat >"${output%/}/provenance.json" <<JSON
{
  "builder": "scripts/import-bwrap-rootfs-tar.sh",
  "source": "$tarball",
  "source_sha256": "$sha256",
  "created_at": "$created_at",
  "host_arch": "$arch"
}
JSON

(
  cd "$output"
  sha256sum image.json provenance.json >checksums.txt
  find rootfs -xdev -type f -print0 | sort -z | xargs -0 sha256sum >>checksums.txt
)

echo "imported bwrap rootfs image at $output"
