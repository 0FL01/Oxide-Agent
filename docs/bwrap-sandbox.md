# Bubblewrap Sandbox Backend

`SANDBOX_BACKEND=bwrap` runs sandbox tools with `bubblewrap` on a bare Linux host. It is additive: the default Docker Compose deployment still uses `SANDBOX_BACKEND=broker` and `oxide-agent-sandboxd`.

## Build

Use the dedicated host profile so Docker/Bollard are not pulled into the bwrap binary:

```bash
cargo build -p oxide-agent-telegram-bot \
  --no-default-features \
  --features profile-host-bwrap
```

Dependency boundary checks:

```bash
scripts/check-cargo-tree-deny.sh sandbox-backend-bwrap
scripts/check-cargo-tree-deny.sh profile-host-bwrap
```

## Host Requirements

Debian 13 / trixie:

```bash
sudo apt update
sudo apt install bubblewrap ca-certificates tar xz-utils
sudo apt install mmdebstrap # only when building the rootfs locally
```

Alpine:

```bash
apk add bubblewrap ca-certificates tar xz
```

The host kernel must allow user, mount, PID, IPC, UTS, and optionally network namespaces. Overlay mode also requires overlayfs support.

## Rootfs

Build the first Debian image locally:

```bash
scripts/build-bwrap-rootfs-debian.sh \
  --suite trixie \
  --image-id debian-13-dev \
  --output .oxide/sandbox/images/debian-13-dev
```

Expected layout:

```text
.oxide/sandbox/images/debian-13-dev/
  image.json
  checksums.txt
  provenance.json
  rootfs/
```

The rootfs is the immutable lower layer. Runtime package installs and system writes go into the per-scope overlay under `BWRAP_STATE_DIR`.

Import a prebuilt, checksum-verified rootfs tarball when local `mmdebstrap` is unavailable or when preparing an Alpine host:

```bash
scripts/import-bwrap-rootfs-tar.sh \
  --tarball /path/to/debian-13-dev-rootfs.tar.zst \
  --sha256 <expected-sha256> \
  --image-id debian-13-dev \
  --output .oxide/sandbox/images/debian-13-dev \
  --distro debian \
  --suite trixie \
  --version 13 \
  --package-manager apt
```

The import path still does not require Docker. The checksum is mandatory; keep tarball signing/provenance outside the repository and record the expected checksum from the trusted build pipeline or release artifact.

When `mmdebstrap` or a prebuilt Debian rootfs is unavailable, build a smoke-only rootfs from host binaries:

```bash
scripts/build-bwrap-rootfs-host-smoke.sh
```

This creates `.oxide/sandbox/images/host-smoke-dev/` with just enough files for bwrap runtime checks. It is intentionally not a production image, does not provide package-manager parity, and does not certify Debian 13 rootfs behavior.

## Configuration

Development example:

```env
SANDBOX_BACKEND=bwrap
BWRAP_BIN=bwrap
BWRAP_IMAGE=debian-13-dev
BWRAP_IMAGE_STORE=.oxide/sandbox/images
BWRAP_STATE_DIR=.oxide/sandbox/scopes
BWRAP_LOCK_DIR=.oxide/sandbox/locks
BWRAP_NET=host
BWRAP_ROOT_MODE=overlay-rw
BWRAP_ROOT_UPPER_DIR=.oxide/sandbox/root-upper
BWRAP_COMMAND_TIMEOUT_SECS=60
BWRAP_RECREATE_LOCK_TIMEOUT_SECS=65
BWRAP_MAX_OUTPUT_BYTES=16777216
BWRAP_MAX_READ_FILE_BYTES=52428800
BWRAP_ALLOW_OVERLAY=true
BWRAP_RESOLV_CONF=auto
BWRAP_DISABLE_NESTED_USERNS=true
```

Service-style paths should be absolute:

```env
BWRAP_IMAGE_STORE=/opt/oxide-agent/bwrap-images
BWRAP_STATE_DIR=/var/lib/oxide-agent/sandbox/scopes
BWRAP_LOCK_DIR=/var/lib/oxide-agent/sandbox/locks
BWRAP_ROOT_UPPER_DIR=/var/lib/oxide-agent/sandbox/root-upper
```

All bwrap operations for the same scope use an exclusive filesystem lock so package-manager writes and overlay state are serialized. `BWRAP_RECREATE_LOCK_TIMEOUT_SECS` controls how long an operation waits for that lock; by default it is `BWRAP_COMMAND_TIMEOUT_SECS + 5`.

`BWRAP_ROOT_UPPER_DIR` is optional. When set, each scope stores persistent system overlay writes under `<BWRAP_ROOT_UPPER_DIR>/<scope>/upper` and per-command overlay workdirs under `<BWRAP_ROOT_UPPER_DIR>/<scope>/work`, keeping both on the same filesystem. The path must be a real directory or absent; it must not be a symlink or live inside the shared rootfs image.

## Smoke Test

Run:

```bash
SANDBOX_BACKEND=bwrap \
BWRAP_IMAGE=debian-13-dev \
BWRAP_NET=host \
scripts/smoke-bwrap.sh debian-13-dev
```

The script writes a JSON result under `.oxide/sandbox/smoke/`. It reports `environment_kind` (`bare-host`, `docker-container`, or `kubernetes-container`), whether the current environment appears nested, and whether basic create/exec/workspace persistence passed. It also checks that the bwrap sandbox did not expose `/var/run/docker.sock` or `/run/sandboxd`.

The Rust integration-style smoke tests are ignored by default and can be run against the same prepared rootfs:

```bash
SANDBOX_BACKEND=bwrap \
BWRAP_ROOTFS=.oxide/sandbox/images/debian-13-dev/rootfs \
cargo test -p oxide-agent-core \
  --no-default-features \
  --features 'sandbox-backend-bwrap,tool-sandbox-exec,tool-sandbox-fileops,tool-sandbox-recreate' \
  bwrap_smoke --lib -- --ignored
```

For the host-derived smoke rootfs, use:

```bash
scripts/build-bwrap-rootfs-host-smoke.sh

BWRAP_IMAGE_STORE=.oxide/sandbox/images \
BWRAP_STATE_DIR=.oxide/sandbox/scopes \
BWRAP_LOCK_DIR=.oxide/sandbox/locks \
BWRAP_NET=host \
BWRAP_ROOT_MODE=overlay-rw \
scripts/smoke-bwrap.sh host-smoke-dev

SANDBOX_BACKEND=bwrap \
BWRAP_ROOTFS=.oxide/sandbox/images/host-smoke-dev/rootfs \
BWRAP_NET=host \
cargo test -p oxide-agent-core \
  --no-default-features \
  --features 'sandbox-backend-bwrap,tool-sandbox-exec,tool-sandbox-fileops,tool-sandbox-recreate' \
  bwrap_smoke --lib -- --ignored
```

## Platform Certification

Current MVP certification status:

| Host | Rootfs | Status | Smoke command |
| --- | --- | --- | --- |
| Debian 13 | Debian 13 `debian-13-dev` | Primary target, ready to smoke after rootfs build | `scripts/smoke-bwrap.sh debian-13-dev` |
| Debian 13 | Alpine | Optional pending | Use `BWRAP_ROOTFS=/path/to/alpine/rootfs scripts/smoke-bwrap.sh alpine-3.23-dev` after an Alpine rootfs builder or verified minirootfs import is added |
| Alpine host | Debian 13 `debian-13-dev` | First-class target, requires prebuilt/copied Debian rootfs | `BWRAP_ROOTFS=/opt/oxide-agent/bwrap-images/debian-13-dev/rootfs scripts/smoke-bwrap.sh debian-13-dev` |
| Alpine host | Alpine | Optional pending | Use `BWRAP_ROOTFS=/path/to/alpine/rootfs scripts/smoke-bwrap.sh alpine-3.23-dev` after Alpine host plus Alpine rootfs smoke coverage exists |
| Docker Compose app container | Any bwrap rootfs | Experimental/dev-only | Requires a future explicit override with namespace/seccomp requirements; keep normal Compose on broker mode |

Alpine rootfs support is intentionally marked optional pending for MVP because the shipped rootfs builder currently targets Debian 13 package parity with `sandbox/Dockerfile.dev`. Alpine minirootfs import needs a separate checksum/provenance path and smoke coverage for BusyBox/GNU tool differences before it should be called supported.

Expected successful smoke result shape:

```json
{
  "backend": "bwrap",
  "environment_kind": "bare-host",
  "root_mode": "overlay-rw",
  "network_mode": "host",
  "exit_status": 0,
  "tests": {
    "create_scope": "pass",
    "exec_command": "pass",
    "workspace_persistence": "pass",
    "docker_socket_absent": "pass",
    "sandboxd_socket_absent": "pass"
  }
}
```

On Alpine hosts, install `bubblewrap ca-certificates tar xz`, place the rootfs/image store and state directories under absolute service paths, and ensure the OpenRC/service user owns `BWRAP_STATE_DIR` and `BWRAP_LOCK_DIR`. Debian rootfs execution on Alpine does not require host glibc because glibc lives inside the rootfs; the agent binary itself must be compatible with the Alpine host.

## Security Notes

- File tools are restricted to `/workspace`.
- Relative paths resolve under `/workspace`; absolute paths must start with `/workspace/`.
- `..`, NUL bytes, non-workspace absolute paths, and symlink escapes are rejected.
- The backend does not mount host home, repo root, `.git`, `.env`, config, SSH keys, Docker socket, or `/run/sandboxd`.
- `BWRAP_NET=host` is the compatibility default and allows network access. Use `BWRAP_NET=none` for restricted deployments.
- `BWRAP_ROOT_MODE=overlay-rw` allows per-scope package installs without mutating the shared image rootfs. `BWRAP_ROOT_MODE=ro` keeps the rootfs read-only but package managers will fail.

## Docker Compose Compatibility

Docker Compose plus bwrap is not the default or safest deployment path. Bwrap needs namespace and mount syscalls that Docker default seccomp/AppArmor profiles often restrict. Keep normal Compose deployments on the existing broker path:

```text
oxide_agent -> /run/sandboxd/sandboxd.sock -> oxide-agent-sandboxd -> Docker socket
```

If a future dev-only Compose override is added, it must document any elevated namespace/seccomp requirements and must not mount host home, the repo root, or the Docker socket into the bwrap sandbox.
