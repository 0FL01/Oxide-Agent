#!/usr/bin/env bash
set -euo pipefail

suite="trixie"
image_id="debian-13-dev"
output=".oxide/sandbox/images/debian-13-dev"

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/build-bwrap-rootfs-debian.sh [--suite trixie] [--image-id debian-13-dev] [--output .oxide/sandbox/images/debian-13-dev]

Builds an unpacked Debian rootfs for SANDBOX_BACKEND=bwrap. Requires
mmdebstrap on the host. The output directory will contain rootfs/, image.json,
checksums.txt, and provenance.json.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --suite)
      suite="${2:?missing --suite value}"
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

if ! command -v mmdebstrap >/dev/null 2>&1; then
  echo "mmdebstrap is required. On Debian/Ubuntu: sudo apt install mmdebstrap" >&2
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

mkdir -p "$output"

packages=(
  ca-certificates
  curl
  dnsutils
  fd-find
  ffmpeg
  git
  iputils-ping
  jq
  mtr
  net-tools
  nmap
  procps
  python3
  python3-bs4
  python3-httpx
  python3-lxml
  python3-pip
  python3-requests
  ripgrep
  telnet
  traceroute
  tzdata
  unzip
  whois
  yt-dlp
  zip
)

IFS=,
include="${packages[*]}"
unset IFS

mmdebstrap \
  --variant=apt \
  --include="$include" \
  "$suite" \
  "$rootfs"

install -d -m 0755 "$rootfs/proc" "$rootfs/dev" "$rootfs/tmp" "$rootfs/workspace"
chmod 1777 "$rootfs/tmp"

created_at="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
cat >"${output%/}/image.json" <<JSON
{
  "schema_version": 1,
  "id": "$image_id",
  "distro": "debian",
  "suite": "$suite",
  "version": "13",
  "arch": "$arch",
  "rootfs": "rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/workspace",
  "package_manager": "apt",
  "default_env": {
    "HOME": "/workspace",
    "PATH": "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    "LANG": "C.UTF-8",
    "TMPDIR": "/tmp"
  },
  "provenance": {
    "builder": "scripts/build-bwrap-rootfs-debian.sh",
    "source": "debian $suite repositories",
    "created_at": "$created_at"
  }
}
JSON

cat >"${output%/}/provenance.json" <<JSON
{
  "builder": "scripts/build-bwrap-rootfs-debian.sh",
  "source": "debian $suite repositories",
  "created_at": "$created_at",
  "host_arch": "$arch"
}
JSON

(
  cd "$output"
  sha256sum image.json provenance.json >checksums.txt
  find rootfs -xdev -type f -print0 | sort -z | xargs -0 sha256sum >>checksums.txt
)

echo "built bwrap rootfs image at $output"
