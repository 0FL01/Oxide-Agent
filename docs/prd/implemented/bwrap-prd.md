# PRD: Bubblewrap Sandbox Backend for Oxide Agent

## 1. Summary

This PRD defines a new Bubblewrap-based sandbox backend for Oxide Agent. The backend must allow `SANDBOX_BACKEND=bwrap` to run agent tools without Docker daemon access, Docker socket access, Docker API usage, or `bollard` in bwrap-only builds.

The design is intentionally additive. The existing Docker and Docker Compose workflow remains supported, including the current broker/sandboxd mode. Bubblewrap is introduced as a separate backend with a different runtime model: an unpacked immutable base rootfs directory plus per-scope persistent writable system overlay and persistent `/workspace`, where each command starts a fresh `bwrap` process.

Primary product decision for MVP:

- Keep Docker backend unchanged by default for current Docker Compose deployment.
- Add `BwrapSandboxManager` as a sibling backend, not a replacement.
- Treat bwrap “image” as an Oxide-owned rootfs manifest, not a Docker image and not an OCI runtime layer graph.
- Keep the shared image rootfs immutable and never mutate it during agent execution.
- Make the effective sandbox root writable through a per-scope overlay by default so `apt`, `apk`, and system-level writes can persist for that agent/scope.
- Make `/workspace` a separate writable persistent project/data mount.
- Restrict file tools to `/workspace` only.
- Do not bind mount the repository root, host home, `.git`, `.env`, SSH keys, or Docker socket into bwrap.
- Do not claim Docker bridge networking, cgroup limits, long-running containers, Docker logs, or Docker image parity in MVP.
- Do not let one scope’s package installs or system mutations affect the shared image rootfs or another scope.

## 2. Goals

The feature must deliver the following behavior.

- Add a selectable sandbox backend:
  - `SANDBOX_BACKEND=bwrap`
  - `SANDBOX_BACKEND=docker`
  - `SANDBOX_BACKEND=broker`
- Preserve the current Docker-based modes:
  - direct Docker backend via `sandbox-backend-docker-direct`
  - Docker broker/client mode via `sandbox-backend-sandboxd-client` and `sandbox-daemon`
- Allow bwrap backend to run as a precompiled standalone host binary without Docker daemon, Docker socket, Docker API, or `bollard`.
- Support a bwrap-only Cargo build where `cargo tree` does not contain `bollard`.
- Support a Debian 13 / trixie sandbox rootfs as the primary rootfs target.
- Support Alpine rootfs as an optional target.
- Support Debian 13 host and Alpine-based host as first-class runtime targets.
- Support persistent per-scope `/workspace` state.
- Support persistent per-scope system/package state so commands can run `apt install`, `apk add`, edit files under `/etc`, `/usr`, `/var`, etc., without mutating the shared image rootfs.
- Make state location configurable and safe for both repo-local development and service deployment.
- Update runtime capability metadata so sandbox tools can use either Docker, sandboxd-client, or bwrap when compiled.
- Provide clear, actionable errors for missing `bwrap`, missing rootfs, invalid config, unsupported network mode, and feature/config mismatches.
- Document Docker Compose + bwrap as an explicit compatibility mode with elevated namespace/seccomp requirements, not as the safest/default deployment path.

## 3. Non-Goals

MVP explicitly does not include:

- Replacing the Docker backend globally.
- Removing `sandboxd` or Docker Compose broker mode.
- Implementing a full Docker replacement platform.
- Implementing an OCI runtime.
- Pulling Docker images in bwrap mode.
- Building Docker images in bwrap mode.
- Supporting Docker image names as bwrap images.
- Supporting Docker bridge network parity.
- Supporting automatic Docker-like memory/CPU cgroup parity.
- Mounting host home by default.
- Mounting arbitrary host paths by default.
- Binding repo root into the sandbox by default.
- Mutating the shared image rootfs in `BWRAP_IMAGE_STORE`.
- Cross-architecture emulation.
- QEMU/binfmt setup.
- Privileged Docker Compose as a production requirement.
- Stack logs parity for bwrap in MVP.
- Sharing installed system packages globally across scopes by mutating the base image at runtime.
- Rootless slirp networking in MVP. This is v2 if a separate networking helper is intentionally selected.

## 4. Current Architecture Recon

### Repository and workspace layout

The uploaded repository root is a Rust workspace with these relevant files and directories:

- `Cargo.toml`
- `Cargo.lock`
- `crates/oxide-agent-core/`
- `crates/oxide-agent-runtime/`
- `crates/oxide-agent-sandboxd/`
- `crates/oxide-agent-telegram-bot/`
- `docker-compose.yml`
- `docker/compose.dev.yml`
- `docker/compose.full.yml`
- `docker/compose.embedded-opencode-local.yml`
- `docker/compose.media.yml`
- `docker/compose.search.yml`
- `docker/Dockerfile.app`
- `sandbox/Dockerfile.dev`
- `sandbox/Dockerfile.exec`
- `sandbox/Dockerfile.media`
- `sandbox/Dockerfile.minimal`
- `profiles/*.toml`
- `scripts/check-cargo-tree-deny.sh`
- `scripts/check-compiled-capabilities.sh`
- `scripts/check-compose-profile.sh`
- `scripts/check-runtime-env-surface.sh`
- `.env.example`
- `.gitignore`
- `README.md`

The root `Cargo.toml` is workspace-only. Feature definitions are in package manifests, mainly `crates/oxide-agent-core/Cargo.toml` and `crates/oxide-agent-sandboxd/Cargo.toml`.

### Current sandbox modules

The primary sandbox code lives in `crates/oxide-agent-core/src/sandbox/`:

- `mod.rs`
- `traits.rs`
- `scope.rs`
- `manager.rs`
- `manager_stub.rs`
- `broker.rs`
- `admin.rs`
- `diagnostics.rs`

Current module comments and naming are Docker-centric. `mod.rs` currently describes “Docker sandbox management for Agent Mode” and gates `manager` and `broker` on Docker/sandboxd-related features.

Current gating in `crates/oxide-agent-core/src/sandbox/mod.rs` includes:

- `sandbox-backend-docker-direct`
- `sandbox-backend-sandboxd-client`
- `sandbox-daemon`
- `tool-stack-logs`

When none of those features is enabled, `manager_stub.rs` is exported. The stub error currently says sandbox support is not compiled and recommends enabling `sandbox-backend-docker-direct` or `sandbox-daemon`. This must be updated to include `sandbox-backend-bwrap` after the new feature lands.

### Existing traits and reusable interfaces

`crates/oxide-agent-core/src/sandbox/traits.rs` already contains a useful backend capability layer:

- `SandboxBackendId`
- `SandboxCapability`
- `SandboxBackend`
- `SandboxExec`
- `SandboxFileOps`
- `SandboxLifecycle`
- `SandboxAdmin`
- `SandboxDiagnostics`
- `SandboxFileListing`

These traits are reusable for bwrap with minimal changes. The key methods already align with the desired tool surface:

- `SandboxExec::exec(...)`
- `SandboxFileOps::write_file(...)`
- `SandboxFileOps::read_file(...)`
- `SandboxFileOps::file_size_bytes(...)`
- `SandboxFileOps::list_files(...)`
- `SandboxLifecycle::recreate(...)`
- `SandboxAdmin` inventory and lifecycle methods

`SandboxDiagnostics` is behind `tool-stack-logs` and is tied to stack/Compose log behavior. bwrap should not implement diagnostics in MVP unless a separate bwrap diagnostics design is added.

### Sandbox identity and scope

`crates/oxide-agent-core/src/sandbox/scope.rs` defines `SandboxScope`:

- `owner_id`
- `namespace`
- `chat_id`
- `thread_id`

It provides:

- `SandboxScope::new(owner_id, namespace)`
- `SandboxScope::with_transport_metadata(...)`
- `owner_id()`
- `namespace()`
- `chat_id()`
- `thread_id()`
- `container_name()`
- `docker_labels()`

The current `container_name()` format is stable:

```text
agent-sandbox-u<owner_id>-<fnv64(namespace)>
```

This stable identity can be reused for bwrap scope directories and compatibility records, but the method name is Docker-specific. Implementation should add a backend-neutral helper, for example:

- `SandboxScope::stable_name()`
- `SandboxScope::scope_dir_name()`
- `SandboxScope::sandbox_name()`

Do not store raw `namespace` as a directory name. Use a stable sanitized name based on the existing hash.

### Current Docker manager

`crates/oxide-agent-core/src/sandbox/manager.rs` is the current facade and Docker implementation.

Key public types:

- `ExecResult`
- `SandboxContainerRecord`
- `SandboxManager`

Current `SandboxContainerRecord` is Docker-specific by name and fields:

- `container_id`
- `container_name`
- `image`
- `created_at`
- `state`
- `status`
- `running`
- `user_id`
- `scope`
- `chat_id`
- `thread_id`
- `labels`

This record is used outside Docker-specific code, especially by manager control-plane tools. There is no product migration requirement for this feature. For MVP, avoid broad control-plane refactors:

- return compatibility records from bwrap with `container_id` values like `bwrap:<scope_name>` and labels that include `agent.sandbox_backend=bwrap`;
- keep `SandboxContainerRecord` as a compatibility type to reduce UI and control-plane churn;
- treat a later backend-neutral `SandboxInstanceRecord` as cleanup only, not as a prerequisite for bwrap.

### Current backend selection

`SandboxManager` currently wraps this enum:

```rust
enum SandboxManagerInner {
    Docker(DockerSandboxManager),
    Broker(BrokerSandboxManager),
}
```

The current selection logic is binary and compile-time-biased:

- if both direct Docker and sandboxd client are compiled, `compiled_sandbox_backend_prefers_broker()` checks `sandbox_uses_broker()`
- if only sandboxd client is compiled, broker is used
- otherwise direct Docker is used

The config parser only distinguishes broker from not-broker through `sandbox_uses_broker()`. Adding `bwrap` requires replacing this with an explicit parsed backend enum, for example:

```rust
enum SandboxBackendConfig {
    Docker,
    Broker,
    Bwrap,
}
```

Invalid values must fail fast with a message like:

```text
Invalid SANDBOX_BACKEND='podman'. Valid values: docker, broker, bwrap.
```

Feature/config mismatches must be explicit:

```text
SANDBOX_BACKEND=bwrap was selected, but this binary was not compiled with sandbox-backend-bwrap.
Build with --features sandbox-backend-bwrap or select SANDBOX_BACKEND=docker/broker.
```

### Where `bollard` is used

`bollard` is an optional dependency in `crates/oxide-agent-core/Cargo.toml`:

```toml
bollard = { version = "0.20.2", optional = true }
```

It is enabled by:

```toml
sandbox-backend-docker-direct = [
    "dep:bollard",
    "dep:tar",
    "dep:bytes",
    "dep:http-body-util",
    "sandbox-broker-protocol",
]
```

`tool-stack-logs` currently enables Docker directly:

```toml
tool-stack-logs = ["sandbox-backend-docker-direct"]
```

`sandbox-daemon` also enables direct Docker:

```toml
sandbox-daemon = ["sandbox-backend-docker-direct"]
```

`crates/oxide-agent-core/src/sandbox/manager.rs` uses `bollard` types for Docker operations:

- `bollard::Docker`
- `bollard::exec::{CreateExecOptions, StartExecResults}`
- `bollard::container::LogOutput`
- `bollard::models::{ContainerCreateBody, ContainerSummary, ContainerSummaryStateEnum, HostConfig}`
- `bollard::query_parameters::*`
- Docker logs APIs
- Docker exec APIs
- Docker cp upload/download APIs
- Docker container list/inspect/create/start/remove APIs

A bwrap-only build must not enable any feature that transitively enables `sandbox-backend-docker-direct`, `tool-stack-logs`, or `sandbox-daemon`.

Verification note: the authoring container used for this PRD did not have the `cargo` binary available, so `cargo tree` could not be executed here. The dependency conclusions above are based on `Cargo.toml`, `Cargo.lock`, and `scripts/check-cargo-tree-deny.sh`. The PRD acceptance criteria require running `cargo tree` in CI/dev after implementation.

### Current Docker runtime behavior

The current Docker backend behavior in `DockerSandboxManager` is:

- `new(scope)` connects to Docker via `Docker::connect_with_local_defaults()` and pings Docker.
- `create_sandbox()` creates a persistent Docker container if missing.
- The container image defaults to `SANDBOX_IMAGE=agent-sandbox:latest`.
- The container name comes from `SandboxScope::container_name()`.
- The working directory is `/workspace`.
- The container command is `sleep infinity`.
- Host config includes memory and CPU settings:
  - `SANDBOX_MEMORY_LIMIT`
  - `SANDBOX_CPU_PERIOD`
  - `SANDBOX_CPU_QUOTA`
  - `network_mode: "bridge"`
  - `auto_remove: true`
- `exec_command()` uses Docker exec with `sh -c <cmd>` in `/workspace`.
- `exec_command()` uses `SANDBOX_EXEC_TIMEOUT_SECS`, default 60 seconds.
- Timeout/cancel attempts to run `killall5 -9` in the container.
- `write_file()` uploads through Docker copy/tar APIs.
- `read_file()` downloads through Docker copy/tar APIs.
- `recreate()` removes and creates a new Docker container.
- `destroy()` removes the Docker container.
- stack logs use Docker Compose labels and Docker log APIs.

Docker-specific image-not-found errors currently suggest:

```text
docker compose build sandbox_image
```

or:

```text
docker compose up --build -d
```

The bwrap implementation must add equivalent actionable rootfs errors, not reuse Docker-image wording.

### Current file tools and tool provider layer

The sandbox tool provider is in:

- `crates/oxide-agent-core/src/agent/providers/sandbox.rs`

It exposes these tools:

- `execute_command`
- `write_file`
- `read_file`
- `list_files`
- `recreate_sandbox`

It defines `SandboxRuntime`, which lazily creates a `SandboxManager` and wraps operations in an in-process `tokio::sync::RwLock` named `execution_gate`.

Current concurrency semantics:

- `execute_command`, `write_file`, `read_file`, `file_size_bytes`, and `list_files` take a shared read lock.
- `recreate_sandbox` takes an exclusive write lock.
- This prevents recreate during commands within the same `SandboxRuntime` instance.
- It does not prevent concurrent operations from separate runtimes, processes, or future broker requests.

The bwrap backend needs a per-scope lock outside this in-memory lock.

`list_files()` is currently implemented in the provider by executing a shell command:

```rust
tree -L 3 -h --du <path> 2>/dev/null || find <path> -type f -o -type d | head -100
```

For bwrap, file listing should be implemented directly against the workspace filesystem rather than by running `tree`/`find`, because file tools are restricted to `/workspace` and should not require a command spawn.

Current tool descriptions allow paths “relative to `/workspace` or absolute”. For bwrap MVP, tool descriptions must be tightened:

- relative paths resolve under `/workspace`
- absolute paths are accepted only if they start with `/workspace`
- all other absolute paths are rejected

The same restriction may be applied across all backends for consistency, but at minimum must be enforced by bwrap.

### Current sandbox tools registration

Tool registration happens in:

- `crates/oxide-agent-core/src/agent/tool_runtime/modules.rs`
- `crates/oxide-agent-core/src/agent/executor/registry.rs`

Relevant feature gates:

- `tool-sandbox-exec`
- `tool-sandbox-fileops`
- `tool-sandbox-recreate`

The tool module layer already depends on backend capabilities rather than concrete Docker APIs. This is reusable if compiled capability metadata is updated to include bwrap.

### Other code paths using sandbox runtime

These providers/modules reference sandbox runtime or sandbox file/exec capabilities and should be smoke-tested with bwrap:

- `crates/oxide-agent-core/src/agent/preprocessor.rs`
- `crates/oxide-agent-core/src/agent/providers/ytdlp.rs`
- `crates/oxide-agent-core/src/agent/providers/filehoster.rs`
- `crates/oxide-agent-core/src/agent/providers/media_file.rs`
- `crates/oxide-agent-core/src/agent/providers/browser_use/mod.rs`
- `crates/oxide-agent-core/src/agent/providers/tts/provider.rs`
- `crates/oxide-agent-core/src/agent/providers/silero_tts/provider.rs`
- `crates/oxide-agent-core/src/agent/providers/delegation.rs`

These paths can usually reuse `SandboxRuntime` unchanged if the `SandboxManager` facade remains compatible.

### Broker/daemon mode

`crates/oxide-agent-core/src/sandbox/broker.rs` implements the Unix socket broker protocol and server/client.

Important findings:

- `SandboxBrokerClient` proxies sandbox operations over a Unix socket.
- `SandboxBrokerServer` is compiled only with `sandbox-backend-docker-direct`.
- Server request handlers construct `DockerSandboxManager` directly.
- Broker server methods call Docker backend methods directly.
- The broker is not backend-independent today.
- The broker socket permissions are set to `0o666` in `SandboxBrokerServer::bind(...)`.

The current `oxide-agent-sandboxd` service in `crates/oxide-agent-sandboxd/src/main.rs` imports and runs `SandboxBrokerServer`. Its Cargo feature model is:

```toml
sandbox-daemon = ["oxide-agent-core/sandbox-daemon"]
profile-full = ["sandbox-daemon"]
```

Because `sandbox-daemon = ["sandbox-backend-docker-direct"]`, sandboxd currently implies Docker/Bollard.

MVP decision:

- Do not route bwrap through `sandboxd` in MVP.
- Treat `sandboxd` as the Docker privilege-separation daemon it currently is.
- Add bwrap as in-process host backend for standalone binaries.
- A backend-independent broker is v2 if needed.

### Current environment variables

Sandbox config is in `crates/oxide-agent-core/src/config.rs`.

Existing sandbox env vars:

- `SANDBOX_IMAGE`, default `agent-sandbox:latest`
- `SANDBOX_BACKEND`, default `docker`
- `SANDBOXD_SOCKET`, default `/run/sandboxd/sandboxd.sock`
- `STACK_LOGS_PROJECT`, optional

Existing non-env constants:

- `SANDBOX_MEMORY_LIMIT`, default 1 GiB
- `SANDBOX_CPU_PERIOD`, default `100_000`
- `SANDBOX_CPU_QUOTA`, default `200_000`
- `SANDBOX_EXEC_TIMEOUT_SECS`, default 60 seconds

`.env.example` currently does not document bwrap variables. It also does not prominently document all sandbox variables. This must be updated.

### Current Dockerfiles and sandbox images

The current sandbox image definitions are Dockerfiles under `sandbox/`:

- `sandbox/Dockerfile.dev`
- `sandbox/Dockerfile.exec`
- `sandbox/Dockerfile.media`
- `sandbox/Dockerfile.minimal`

All current sandbox Dockerfiles are based on:

```dockerfile
FROM debian:trixie-slim
```

`Dockerfile.dev` includes packages such as:

- `ca-certificates`
- `curl`
- `dnsutils`
- `fd-find`
- `ffmpeg`
- `git`
- `iputils-ping`
- `jq`
- `mtr`
- `net-tools`
- `nmap`
- `procps`
- `python3`
- `python3-pip`
- `ripgrep`
- `telnet`
- `traceroute`
- `tzdata`
- `unzip`
- `whois`
- `zip`

It also installs Python packages:

- `beautifulsoup4`
- `httpx`
- `lxml`
- `requests`
- `yt-dlp`

The bwrap Debian rootfs should start by matching `sandbox/Dockerfile.dev` unless a smaller bwrap variant is intentionally chosen.

`docker/Dockerfile.app` builds the agent runtime image on `debian:trixie-slim`, creates user `oxide` with UID/GID `10001`, and does not install `bubblewrap` today.

### Current Docker Compose topology

The default root `docker-compose.yml` includes:

- `sandbox_image`
- `oxide_agent`
- `sandboxd`

Current default behavior:

- `oxide_agent` runs with `SANDBOX_BACKEND=broker`.
- `oxide_agent` mounts `sandboxd-run:/run/sandboxd`.
- `oxide_agent` does not mount the Docker socket.
- `sandboxd` runs as root.
- `sandboxd` mounts `/var/run/docker.sock:/var/run/docker.sock`.
- `sandboxd` uses host networking.
- `sandbox_image` builds `sandbox/Dockerfile.dev`.

This is a good security separation for Docker mode: the main bot container does not need direct Docker socket access. Bwrap mode must not break this default path.

### Current capability model

Capability metadata lives in:

- `crates/oxide-agent-core/src/capabilities/compiled.rs`
- `crates/oxide-agent-core/src/capabilities/manifest.rs`
- `profiles/*.toml`
- `scripts/check-compiled-capabilities.sh`

Current sandbox backend capability arrays only contain:

- `sandbox-backend/docker-direct/fileops`
- `sandbox-backend/sandboxd-client/fileops`
- `sandbox-backend/docker-direct/exec`
- `sandbox-backend/sandboxd-client/exec`
- `sandbox-backend/docker-direct/lifecycle`
- `sandbox-backend/sandboxd-client/lifecycle`
- `sandbox-backend/docker-direct/diagnostics`
- `sandbox-backend/sandboxd-client/diagnostics`

`tool/stack-logs` requires diagnostics. bwrap should not provide diagnostics in MVP.

The new backend must add capabilities:

- `sandbox-backend/bwrap`
- `sandbox-backend/bwrap/fileops`
- `sandbox-backend/bwrap/exec`
- `sandbox-backend/bwrap/lifecycle`
- optional later: `sandbox-backend/bwrap/admin`

For MVP, admin can either be part of the same backend compatibility path or introduced as an internal implementation behind existing `SandboxAdminRuntime`. Do not add bwrap diagnostics until there is an actual diagnostics API.

### Current `.gitignore`

`.gitignore` currently ignores `.env`, config, logs, `target`, `.embeddings_cache`, `.claude`, and related local files. It does not ignore `.oxide/` or `.oxide/sandbox/`.

The feature must update `.gitignore` to prevent rootfs/state from being committed:

```gitignore
.oxide/sandbox/
```

A broader `.oxide/` ignore can be considered, but the repository already uses `.oxide/tool-artifacts` as an execution context in tests, so the implementation should check whether any tracked `.oxide` files are intended before ignoring the entire directory.

### Likely breakpoints

Adding `Bwrap` is likely to break or require updates in these areas:

- `SandboxManagerInner` exhaustive matches in `manager.rs`.
- `compiled_sandbox_backend_prefers_broker()` and backend selection logic.
- `manager_stub.rs` error messages.
- `sandbox/mod.rs` feature gates.
- `sandbox/broker.rs` if someone expects broker to handle all backends.
- `crates/oxide-agent-core/src/capabilities/compiled.rs` capability arrays and modules.
- `crates/oxide-agent-core/src/capabilities/manifest.rs` tests that assume only Docker and sandboxd-client backends.
- `profiles/*.toml` module lists.
- `scripts/check-cargo-tree-deny.sh` deny lists.
- `scripts/check-compiled-capabilities.sh` allowed profiles and requirements.
- `scripts/check-runtime-env-surface.sh` expected env vars.
- `README.md` infrastructure and deployment docs.
- `.env.example` sandbox env docs.
- `docker/Dockerfile.app` if Docker Compose + bwrap compatibility image is added.
- `docker/compose.*.yml` only if adding an explicit bwrap compatibility override.
- Manager control-plane tool descriptions in `crates/oxide-agent-core/src/agent/providers/manager_control_plane/sandboxes.rs`, because they say “Docker container name”.
- Sandbox tool descriptions in `crates/oxide-agent-core/src/agent/providers/sandbox.rs`, because they say “bash” and allow absolute paths.

### Parts that can be reused without changes or with minimal changes

- `SandboxScope` stable identity hash, with a backend-neutral name helper added.
- `ExecResult`.
- `SandboxBackend`, `SandboxExec`, `SandboxFileOps`, `SandboxLifecycle` traits.
- `SandboxRuntime` lazy initialization and in-process recreate lock.
- `execute_command`, `write_file`, `read_file`, `list_files`, `recreate_sandbox` tool registration model.
- Most provider modules that use `SandboxRuntime` through traits.
- Docker Compose default path, if bwrap is not added to the default compose file.
- Existing Docker backend implementation, if feature gates remain isolated.

## 5. Bubblewrap Recon

### What bwrap is

`bubblewrap` / `bwrap` is a low-level Linux sandboxing tool. It constructs a new mount namespace and optional user, PID, IPC, network, UTS, and cgroup namespaces, then executes a process inside that namespace.

Upstream sources used for this recon:

- `https://github.com/containers/bubblewrap`
- `https://man.archlinux.org/man/bwrap.1.en`
- `https://docs.docker.com/engine/security/seccomp/`
- Debian package metadata for `bubblewrap` in trixie.
- Alpine package/wiki metadata for `bubblewrap`.

Upstream explicitly says bwrap is not a complete ready-made sandbox policy. The program building the bwrap command line is responsible for defining the security model. For Oxide Agent, that means all filesystem, env, network, resource, and path policy must live in Oxide Agent code/config.

### What bwrap is not

bwrap is not:

- Docker.
- Docker Compose.
- A Docker daemon replacement.
- A Docker API replacement.
- An OCI runtime by itself.
- A Docker image manager.
- A Docker layer graph manager.
- A network bridge manager.
- A cgroup resource runtime by itself.
- A log aggregation system.
- A process supervisor for persistent containers.
- A package manager.
- A rootfs builder.

### Mount namespace and root filesystem behavior

bwrap creates a new mount namespace. It starts from an empty root on a tmpfs that is not visible from the host. The caller then supplies explicit mount instructions to construct what the sandbox sees.

Relevant filesystem options:

- `--ro-bind SRC DEST`: bind mount host path read-only.
- `--bind SRC DEST`: bind mount host path read-write.
- `--tmpfs DEST`: mount a tmpfs at `DEST`.
- `--proc DEST`: mount procfs at `DEST`.
- `--dev DEST`: mount a minimal `/dev` at `DEST`.
- `--overlay-src SRC`: add lower/source layer for later overlay option.
- `--overlay RWSRC WORKDIR DEST`: overlay layers with writes to `RWSRC`.
- `--tmp-overlay DEST`: overlay with writes going to tmpfs, not persisted.
- `--ro-overlay DEST`: read-only overlay.

MVP `overlay-rw` mode should use:

- `--overlay-src "$ROOTFS"`
- `--overlay "$ROOT_UPPER" "$ROOT_WORK" /`
- `--proc /proc`
- `--dev /dev`
- `--tmpfs /tmp`
- `--bind "$WORKSPACE" /workspace`

MVP `ro` mode should replace the overlay options with `--ro-bind "$ROOTFS" /`.

The rootfs must already include mount-point directories or bwrap must be invoked in an order that can create them. Implementation should validate these paths exist in the image rootfs:

- `/bin/sh` or manifest `default_shell`
- `/proc`
- `/dev`
- `/tmp`
- `/workspace`

If a rootfs lacks `/workspace`, image build scripts should create it before publishing the rootfs.

### Namespace options relevant to Oxide

MVP should use these by default:

- `--unshare-user`
- `--uid 0`
- `--gid 0`
- `--unshare-pid`
- `--unshare-ipc`
- `--unshare-uts`
- `--unshare-cgroup-try`
- `--die-with-parent`
- `--new-session`
- `--clearenv`
- `--setenv`
- `--chdir /workspace`

Network mode controls `--unshare-net`:

- `BWRAP_NET=host`: do not pass `--unshare-net`.
- `BWRAP_NET=none`: pass `--unshare-net`.

If `--unshare-net` is used, the sandbox has its own network namespace with loopback only. This is safer but breaks tools that require network access.

### User namespace behavior

bwrap uses user namespaces for unprivileged operation. `--uid` and `--gid` require `--unshare-user`. The typical bwrap model allows the sandbox process to appear as UID 0 inside the user namespace while mapping back to the invoking unprivileged host user outside.

MVP should run as root inside the sandbox namespace for compatibility with package-style tools, but root inside this user namespace is not host root. Do not interpret UID 0 inside the bwrap sandbox as permission to write host paths. Host write permissions are still bounded by the invoking agent user and the explicit writable mounts.

### Nested user namespace restriction

`--disable-userns` prevents the sandboxed process from creating further user namespaces. It requires `--unshare-user`. MVP should enable this by default through:

```env
BWRAP_DISABLE_NESTED_USERNS=true
```

Implementation must detect if the installed bwrap supports `--disable-userns`. If unsupported, either:

- fail closed when `BWRAP_DISABLE_NESTED_USERNS=true`, or
- allow an explicit override `BWRAP_DISABLE_NESTED_USERNS=unsupported-ok` only for dev.

Recommended MVP behavior is fail closed for production profiles.

### Environment behavior

Use:

- `--clearenv`
- `--setenv HOME /workspace`
- `--setenv PATH <manifest path>`
- `--setenv LANG C.UTF-8`
- optionally `--setenv LC_ALL C.UTF-8`
- optionally `--setenv TMPDIR /tmp`

Do not pass host env by default. In particular, never pass:

- `OPENAI_API_KEY`
- `ANTHROPIC_API_KEY`
- `TELEGRAM_TOKEN`
- `OXIDE_R2_*`
- `AWS_*`
- `SSH_AUTH_SOCK`
- `DOCKER_HOST`
- `HOME` from host
- `XDG_CONFIG_HOME` from host
- `.env` contents

If env allowlisting is ever added, it must be explicit and secret-aware.

### Process/session behavior

Use:

- `--unshare-pid`
- `--die-with-parent`
- `--new-session`

`--new-session` is important because upstream warns that without seccomp filtering of terminal ioctls, `--new-session` protects against a known class of out-of-sandbox terminal injection behavior.

The Rust side must still implement:

- command timeout
- cancellation
- process group kill
- stdout/stderr capture limit
- wait and reap behavior

bwrap reports the wrapped command exit status to the parent. Oxide should preserve non-zero exit codes in `ExecResult.exit_code` instead of treating them as infrastructure failures.

### Seccomp considerations

bwrap supports `--seccomp FD`, but MVP should not introduce a custom seccomp profile unless the team commits to maintaining it. The MVP baseline is namespace/mount isolation plus no-new-privs behavior from bwrap.

If a future v2 adds seccomp, it must be tested against:

- Python
- Git
- curl/wget
- ffmpeg
- yt-dlp
- package managers if used inside the rootfs
- Alpine busybox tools
- Debian tools

### Overlay behavior

bwrap overlay options are core to the bwrap backend MVP because the sandbox must support package installation and system file mutation without modifying the shared image rootfs.

MVP root policy:

- `BWRAP_ROOT_MODE=overlay-rw` is the default product mode for bwrap.
- the shared image rootfs is used as an immutable lower layer.
- all system writes go to a per-scope overlay upperdir.
- `apt install`, `apt upgrade`, `apk add`, package database writes, `/etc` edits, `/usr` writes, and `/var` writes persist for that scope only.
- `/workspace` remains a separate persistent bind mount for project/data files.
- `/tmp` remains tmpfs and non-persistent.

Additional supported/optional root policies:

- `BWRAP_ROOT_MODE=ro`: hardening/debug mode; system package installation and system writes fail, but `/workspace` remains writable.
- `BWRAP_ROOT_MODE=tmp-overlay`: optional dev mode; system writes are visible only during one command and disappear after the process exits.

Overlay MVP must handle kernel support, bwrap overlay option availability, workdir/upperdir same-filesystem requirements, cleanup of per-command workdirs, and rootfs tampering risks.

### Running inside Docker containers

Running bwrap inside Docker is tricky because bwrap needs namespace and mount-related syscalls. Docker’s default seccomp profile blocks or restricts important syscalls such as namespace-related `clone`, `unshare`, `mount`, `umount`, `pivot_root`, and `setns`. Docker docs also state the default seccomp profile is the least-privilege default and not recommended to change casually.

Therefore Docker Compose + bwrap is not the safest path and should not become the default deployment. It can be supported only as an explicit dev/compatibility mode with documented tradeoffs.

## 6. Target Runtime Matrix

### Debian host + Debian sandbox rootfs

Expected support level: MVP primary path.

Design:

- Host installs `bubblewrap` from Debian packages.
- Host runs the precompiled Oxide Agent binary or a local cargo-built binary.
- Rootfs is Debian 13 / trixie, unpacked under the image store.
- Rootfs architecture must match host architecture.
- `create_sandbox()` creates per-scope state dirs and metadata.
- `exec_command()` starts a fresh bwrap process with Debian userspace.
- Persistent state is `/workspace` mounted from the scope state dir.

Notes:

- This is not a VM. The host kernel is shared.
- The Debian rootfs supplies userspace tools and libraries.
- The bwrap backend must work without Docker daemon running.

### Debian host + Alpine sandbox rootfs

Expected support level: supported after Alpine rootfs smoke tests.

Design:

- Host installs Debian `bubblewrap`.
- Rootfs is Alpine minirootfs or built Alpine rootfs.
- The sandbox shell defaults to `/bin/sh` from busybox unless manifest selects another shell.
- Commands that assume GNU tools may behave differently.
- Package manager is `apk` if package manager access is intentionally enabled.

Notes:

- Debian host can run Alpine userspace because the kernel ABI is shared.
- No CPU emulation is involved.
- Tests must cover shell/tool differences.

### Alpine host + Debian sandbox rootfs

Expected support level: first-class target for bare binary mode.

Design:

- Host installs `bubblewrap` with `apk add bubblewrap`.
- Agent binary should be a musl build or otherwise compatible with Alpine host.
- Debian rootfs can include glibc inside the sandbox; this does not require host glibc.
- Rootfs delivery should prefer prebuilt verified tarball, because building Debian rootfs on Alpine via `mmdebstrap` is possible but not the simplest runtime story.

Notes:

- Verify Alpine kernel config/user namespace support.
- OpenRC service mode needs explicit state dir ownership.
- `systemd-run` resource-limit integration is not available on typical Alpine/OpenRC hosts.

### Alpine host + Alpine sandbox rootfs

Expected support level: supported after Alpine host and Alpine rootfs smoke tests.

Design:

- Host installs Alpine `bubblewrap`.
- Agent binary is musl-compatible.
- Rootfs is Alpine minirootfs or prepared Alpine dev rootfs.
- Shell is `/bin/sh`; no Bash unless installed.
- Tool availability differs from Debian rootfs.

Notes:

- Useful for small deployments.
- Package and shell differences must be documented in image manifest and tests.

### Docker Compose container + bwrap sandbox inside container

Expected support level: explicit compatibility/dev mode only, not default MVP production path.

Current compose path is Docker backend through `sandboxd`. Keep it.

Bwrap inside Docker likely needs some combination of:

- `bubblewrap` installed in the app container.
- bwrap rootfs mounted into the app container.
- writable bwrap state volume mounted into the app container.
- user namespaces available inside the container.
- mount namespaces available inside the container.
- Docker seccomp override or custom seccomp profile.
- possibly `CAP_SYS_ADMIN`, depending on host/kernel/Docker constraints.
- AppArmor/SELinux adjustments on hosts enforcing profiles.
- careful `/proc` and `/dev` behavior.

Recommended product stance:

- Safe supported path: Docker Compose + existing Docker backend/broker.
- Safe supported bwrap path: bare host binary + bwrap.
- Dev-compatible path: explicit compose override for bwrap with warnings and smoke tests.
- Unsupported unsafe path: `privileged: true` plus broad host binds, host home, repo root, Docker socket, or host root.

### Bare precompiled binary + bwrap sandbox on host

Expected support level: MVP target.

Design:

- Precompiled binary includes `sandbox-backend-bwrap` and sandbox tools.
- Binary does not include `bollard` in bwrap-only profile.
- Host has `bwrap` installed.
- Rootfs is present and verified.
- State dir is writable by the agent user.
- No Docker daemon is required.
- No Docker socket is required.

## 7. Proposed Architecture

### New backend type

Add a bwrap backend implementation in a new module:

```text
crates/oxide-agent-core/src/sandbox/bwrap.rs
```

Primary type:

```rust
pub(crate) struct BwrapSandboxManager {
    config: BwrapSandboxConfig,
    scope: SandboxScope,
    state: BwrapScopeState,
}
```

Supporting types:

```rust
enum SandboxBackendConfig {
    Docker,
    Broker,
    Bwrap,
}

struct BwrapSandboxConfig {
    bwrap_bin: PathBuf,
    image_id: Option<String>,
    image_store: PathBuf,
    rootfs: PathBuf,
    state_dir: PathBuf,
    net: BwrapNetworkMode,
    root_mode: BwrapRootMode,
    command_timeout: Duration,
    max_output_bytes: usize,
    allow_overlay: bool,
    disable_nested_userns: bool,
    default_shell: String,
    default_workdir: String,
    default_env: BTreeMap<String, String>,
}

enum BwrapNetworkMode {
    Host,
    None,
}

enum BwrapRootMode {
    ReadOnly,
    TmpOverlay,
    OverlayRw,
}
```

MVP supports `BwrapRootMode::OverlayRw` and `BwrapRootMode::ReadOnly`.

`OverlayRw` is the default for the bwrap product profile because package installation and system writes are required. `ReadOnly` remains a supported hardening/debug mode. `TmpOverlay` is optional; if not implemented, selecting it must return an actionable unsupported error.

### Module gating

Update `crates/oxide-agent-core/src/sandbox/mod.rs` so the real manager is compiled when any real backend is present:

- `sandbox-backend-docker-direct`
- `sandbox-backend-sandboxd-client`
- `sandbox-backend-bwrap`
- `sandbox-daemon`
- `tool-stack-logs`

Add:

```rust
#[cfg(feature = "sandbox-backend-bwrap")]
pub(crate) mod bwrap;
```

Do not put `bwrap` code inside Docker feature gates.

### Backend selection flow

Replace current broker-vs-docker selection with explicit parse/dispatch:

1. Read `SANDBOX_BACKEND`.
2. Normalize lowercase/trim.
3. Validate one of:
   - `docker`
   - `broker`
   - `bwrap`
4. Check that selected backend is compiled.
5. Construct the selected manager variant.

Example behavior:

- Binary compiled with only `sandbox-backend-bwrap`, env missing:
  - default should be `bwrap` for `profile-host-bwrap`, not global default.
  - Implementation can set a profile-specific default through config/profile, or fail with “set `SANDBOX_BACKEND=bwrap`”.
- Binary compiled with Docker and bwrap, env missing:
  - keep current default `docker` to avoid behavior changes.
- Compose default:
  - `SANDBOX_BACKEND=broker` remains explicit.

Recommended implementation:

- Keep `SANDBOX_BACKEND` global default in `config.rs` as `docker` for backward compatibility.
- New host-bwrap profile docs and `.env.example` set `SANDBOX_BACKEND=bwrap` explicitly.
- If Docker is not compiled and `SANDBOX_BACKEND` is unset, return a message that names compiled backends and asks for explicit selection.

### Manager enum changes

Change:

```rust
enum SandboxManagerInner {
    Docker(DockerSandboxManager),
    Broker(BrokerSandboxManager),
}
```

to:

```rust
enum SandboxManagerInner {
    #[cfg(feature = "sandbox-backend-docker-direct")]
    Docker(DockerSandboxManager),
    #[cfg(feature = "sandbox-backend-sandboxd-client")]
    Broker(BrokerSandboxManager),
    #[cfg(feature = "sandbox-backend-bwrap")]
    Bwrap(BwrapSandboxManager),
}
```

Add dispatch arms for:

- `is_running()`
- `container_id()` or compatibility instance id
- `scope()`
- `create_sandbox()`
- `exec_command()`
- `write_file()`
- `read_file()`
- `upload_file()` if kept as alias
- `download_file()` if kept as alias
- `get_uploads_size()`
- `cleanup_old_downloads()`
- `destroy()`
- `recreate()`
- `file_size_bytes()`
- admin list/inspect/ensure/recreate/delete

`get_uploads_size()` and `cleanup_old_downloads()` are currently Docker-backed helpers around `/workspace/uploads`. For bwrap they can be implemented directly on host workspace paths.

### bwrap lifecycle semantics

`create_sandbox()` in bwrap does not create a long-running container. It must:

- resolve config
- validate `bwrap` binary
- validate rootfs manifest and rootfs path
- validate architecture compatibility
- create state layout for the scope
- create `workspace/`
- create `tmp/` or staging dirs if needed
- create metadata file
- acquire/release a short lifecycle lock
- return success

`exec_command()` in bwrap must:

- ensure sandbox state exists
- acquire a per-scope shared/read command lock
- spawn a fresh `bwrap` process for the command
- capture stdout/stderr with size limits
- enforce timeout/cancellation
- kill the spawned process/process group on timeout/cancel
- return `ExecResult` preserving exit status

`recreate()` in bwrap must:

- acquire an exclusive per-scope lifecycle lock
- block new commands
- wait for active commands to finish or cancel/kill them according to policy
- remove and recreate `workspace/`
- clear temp/staging dirs
- keep or rewrite metadata with incremented generation
- return success

`destroy()` in bwrap must:

- acquire exclusive per-scope lifecycle lock
- block new commands
- kill or wait for active commands according to policy
- remove scope state directory
- remove lock file only if safe
- return success even when scope is already absent, unless permissions prevent cleanup

### Per-scope locking

The current in-memory `SandboxRuntime.execution_gate` is not sufficient across multiple `SandboxRuntime` instances or processes. bwrap must add filesystem-level per-scope locking.

Recommended MVP:

- `locks/<scope-name>.lock` for process-wide coordination.
- A shared/read lock for exec and file operations.
- An exclusive/write lock for recreate and destroy.
- A metadata `generation` number checked before and after lock acquisition.

Rust implementation options:

- Use `fs2` or another safe file-lock crate under `sandbox-backend-bwrap`.
- Avoid `unsafe` because crate lint forbids unsafe code.

When recreate is requested while a command is running:

- MVP behavior: wait up to `BWRAP_RECREATE_LOCK_TIMEOUT_SECS`, default equal to command timeout plus 5 seconds.
- If still busy, return an actionable error:

```text
Cannot recreate bwrap sandbox '<scope>' because commands are still running after 65s. Cancel the active command or retry later.
```

Destroy may use the same behavior, or may kill active commands if called from an admin cleanup path. The policy must be explicit in code and docs.

### Command execution API

MVP command execution should remain shell-compatible with existing tool behavior:

```text
/default_shell -lc <command>
```

The default shell comes from `image.json`, normally `/bin/sh`.

Do not assume Bash. Existing tool description says “bash command”; update to “shell command” or ensure Debian image installs bash and manifest says `/bin/bash`. Recommended MVP: use `/bin/sh` to support both Debian and Alpine.

### File operation API

Bwrap file tools should operate directly on the host workspace path instead of spawning bwrap for file ops.

Rules:

- Accepted user path forms:
  - `foo.txt`
  - `dir/foo.txt`
  - `/workspace/foo.txt`
  - `/workspace/dir/foo.txt`
- Rejected path forms:
  - `/etc/passwd`
  - `/workspace/../etc/passwd`
  - `../secret`
  - paths containing NUL
  - paths resolving through symlinks outside workspace
  - absolute paths not under `/workspace`
- Reads reject directories.
- Writes create parent directories under workspace only.
- Writes must not follow symlinked parent components.
- Writes should reject replacing an existing symlink unless a safe no-follow open confirms the final target remains inside workspace.

Implementation should use file-descriptor-relative APIs where possible, for example via `cap-std` or `rustix` openat-style calls with no-follow semantics. Do not rely only on string prefix checks after `canonicalize()`, because symlink races can still exist.

### Metadata

Each bwrap scope must have metadata:

```json
{
  "schema_version": 1,
  "backend": "bwrap",
  "scope_name": "agent-sandbox-u77-0123456789abcdef",
  "owner_id": 77,
  "namespace": "-100123:69",
  "chat_id": -100123,
  "thread_id": 69,
  "image_id": "debian-13-dev",
  "rootfs": "/abs/path/.oxide/sandbox/images/debian-13-dev/rootfs",
  "workspace": "/abs/path/.oxide/sandbox/scopes/agent-sandbox-u77-0123456789abcdef/workspace",
  "created_at": 1760000000,
  "updated_at": 1760000000,
  "generation": 1,
  "active_commands": []
}
```

Metadata is for inventory and safety, not security truth. Always validate actual paths and locks at runtime.

## 8. Image and Rootfs Model

### Terminology

In bwrap mode, “image” means an Oxide bwrap image:

- unpacked root filesystem directory
- `image.json` manifest metadata
- optional checksum/signature/provenance files
- rootfs path
- default shell
- env defaults
- mount policy defaults
- network policy defaults
- persistence policy defaults

Example:

```text
.oxide/sandbox/images/debian-13-dev/rootfs
```

This is not:

- a Docker image
- an OCI image runtime
- a container layer graph
- a Docker registry reference

Do not use Docker image names for bwrap unless a future explicit importer is implemented.

### Image store

Default repo-local image store:

```text
.oxide/sandbox/images
```

Service-mode image store:

```text
/var/lib/oxide-agent/sandbox/images
```

User-mode image store:

```text
$XDG_STATE_HOME/oxide-agent/sandbox/images
```

Each image directory:

```text
images/
  debian-13-dev/
    rootfs/
    image.json
    checksums.txt
    provenance.json
  alpine-3.23-dev/
    rootfs/
    image.json
    checksums.txt
    provenance.json
```

`checksums.txt` and `provenance.json` are strongly recommended for the initial implementation. Signature verification is recommended before production enablement.

### Debian 13 rootfs lifecycle

Supported ways to create Debian rootfs:

1. Build locally with `mmdebstrap`.
2. Build in CI and publish a tarball.
3. Download prebuilt tarball and verify checksum/signature.
4. Unpack into image store.

Recommended MVP path:

- Add `scripts/build-bwrap-rootfs-debian.sh` for local builds.
- Add CI job later to publish verified tarball.
- Do not require Docker daemon to build or use bwrap rootfs.

Suggested Debian package baseline should match `sandbox/Dockerfile.dev` initially:

- `ca-certificates`
- `curl`
- `dnsutils`
- `fd-find`
- `ffmpeg`
- `git`
- `iputils-ping`
- `jq`
- `mtr`
- `net-tools`
- `nmap`
- `procps`
- `python3`
- `python3-pip`
- `ripgrep`
- `telnet`
- `traceroute`
- `tzdata`
- `unzip`
- `whois`
- `zip`

Suggested Python packages should match `sandbox/Dockerfile.dev`:

- `beautifulsoup4`
- `httpx`
- `lxml`
- `requests`
- `yt-dlp`

Rootfs build scripts must create:

- `/workspace`
- `/tmp`
- `/proc`
- `/dev`

and must set appropriate permissions.

Example local build command shape:

```bash
scripts/build-bwrap-rootfs-debian.sh \
  --suite trixie \
  --image-id debian-13-dev \
  --output .oxide/sandbox/images/debian-13-dev
```

### Alpine rootfs lifecycle

Supported ways to create Alpine rootfs:

1. Download official Alpine minirootfs tarball.
2. Verify checksum/signature.
3. Unpack into image store.
4. Optionally install package baseline using `apk` inside a controlled setup step.

Recommended scripts:

```text
scripts/fetch-bwrap-rootfs-alpine.sh
scripts/build-bwrap-rootfs-alpine.sh
```

For Alpine rootfs, default shell is `/bin/sh`. Bash is not guaranteed.

### Architecture compatibility

Rootfs architecture must match host architecture unless a future explicit QEMU/binfmt design is added.

Examples:

- x86_64 host + x86_64 rootfs: supported.
- aarch64 host + aarch64 rootfs: supported after smoke tests.
- x86_64 host + aarch64 rootfs: unsupported in MVP.
- aarch64 host + x86_64 rootfs: unsupported in MVP.

Reason:

- bwrap is not a VM.
- bwrap is not CPU emulation.
- The host kernel runs the sandboxed process directly.
- Userspace libraries come from the rootfs.

### Cross-distro compatibility

Supported in principle:

- Debian host + Debian rootfs
- Debian host + Alpine rootfs
- Alpine host + Debian rootfs
- Alpine host + Alpine rootfs

Reason:

- The host kernel is shared.
- The rootfs supplies userspace.
- Debian rootfs can carry glibc even when Alpine host uses musl.
- Alpine rootfs can carry musl even when Debian host uses glibc.

### `image.json` manifest schema

Required `image.json` example:

```json
{
  "schema_version": 1,
  "id": "debian-13-dev",
  "distro": "debian",
  "suite": "trixie",
  "version": "13",
  "arch": "x86_64",
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
  "mount_policy": {
    "root_mode": "ro",
    "writable_paths": ["/workspace"],
    "tmpfs_paths": ["/tmp"]
  },
  "network_policy": {
    "default": "host",
    "supported": ["host", "none"]
  },
  "provenance": {
    "builder": "scripts/build-bwrap-rootfs-debian.sh",
    "source": "debian trixie repositories",
    "created_at": "2026-05-25T00:00:00Z"
  }
}
```

Manifest validation must check:

- `schema_version == 1`
- `id` matches directory name or configured image id
- `rootfs` is relative, not absolute
- resolved rootfs stays under image directory
- rootfs exists and is a directory
- `arch` matches host arch
- `default_shell` is absolute and exists inside rootfs
- `default_workdir` is `/workspace` for MVP
- `PATH` is present
- direct writable bind paths are only `/workspace` for MVP; system writes are allowed only through the per-scope root overlay

### Image selection and lookup

Bwrap image selection is explicit and must not reuse Docker image resolution semantics.

Selection precedence:

1. If `BWRAP_ROOTFS` is set, use that rootfs path directly. The path must exist, must be a directory, must not resolve through unsafe symlinks, and should have an adjacent or configured `image.json` manifest. `BWRAP_IMAGE` is then treated only as a logical image id/label unless the implementation requires manifest lookup.
2. Otherwise, resolve `BWRAP_IMAGE` under `BWRAP_IMAGE_STORE`. For example, `BWRAP_IMAGE=debian-13-dev` and `BWRAP_IMAGE_STORE=.oxide/sandbox/images` resolves to `.oxide/sandbox/images/debian-13-dev/`.
3. If neither `BWRAP_ROOTFS` nor `BWRAP_IMAGE` is set, default `BWRAP_IMAGE` to `debian-13-dev`.
4. If the resolved image directory does not contain `image.json` and `rootfs/`, fail with an actionable error.

Example Debian 13 selection:

```env
SANDBOX_BACKEND=bwrap
BWRAP_IMAGE=debian-13-dev
BWRAP_IMAGE_STORE=.oxide/sandbox/images
BWRAP_STATE_DIR=.oxide/sandbox/scopes
BWRAP_NET=host
BWRAP_ROOT_MODE=overlay-rw
```

Expected files:

```text
.oxide/sandbox/images/debian-13-dev/
  image.json
  rootfs/
```

Example Alpine 3.23 selection:

```env
SANDBOX_BACKEND=bwrap
BWRAP_IMAGE=alpine-3.23-dev
BWRAP_IMAGE_STORE=.oxide/sandbox/images
BWRAP_STATE_DIR=.oxide/sandbox/scopes
BWRAP_NET=host
BWRAP_ROOT_MODE=overlay-rw
```

Expected files:

```text
.oxide/sandbox/images/alpine-3.23-dev/
  image.json
  rootfs/
```

Direct rootfs override is allowed for development and binary-adjacent deployments:

```env
SANDBOX_BACKEND=bwrap
BWRAP_IMAGE=debian-13-dev
BWRAP_ROOTFS=./sandbox-state/images/debian-13-dev/rootfs
BWRAP_STATE_DIR=./sandbox-state/scopes
BWRAP_LOCK_DIR=./sandbox-state/locks
```

Implementation must report the selected image id, rootfs path, manifest path, package manager, and network mode in debug/status output so operators can tell which rootfs is actually being used.

### How to add Debian 13 and Alpine 3.23 images

To add an image, the implementation should not register Rust code for every distro. The image is plugged in by adding an image directory under `BWRAP_IMAGE_STORE` and selecting it with `BWRAP_IMAGE`.

Debian 13 image directory:

```text
.oxide/sandbox/images/debian-13-dev/
  rootfs/
  image.json
  checksums.txt
  provenance.json
```

Minimum Debian `image.json` values:

```json
{
  "schema_version": 1,
  "id": "debian-13-dev",
  "distro": "debian",
  "suite": "trixie",
  "version": "13",
  "arch": "x86_64",
  "rootfs": "rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/workspace",
  "package_manager": "apt"
}
```

Alpine 3.23 image directory:

```text
.oxide/sandbox/images/alpine-3.23-dev/
  rootfs/
  image.json
  checksums.txt
  provenance.json
```

Minimum Alpine `image.json` values:

```json
{
  "schema_version": 1,
  "id": "alpine-3.23-dev",
  "distro": "alpine",
  "version": "3.23",
  "arch": "x86_64",
  "rootfs": "rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/workspace",
  "package_manager": "apk"
}
```

The selected rootfs must contain the shell declared in the manifest. The implementation must not assume Bash for Alpine.

The image selection contract is data-driven:

- `BWRAP_IMAGE=debian-13-dev` selects `<BWRAP_IMAGE_STORE>/debian-13-dev/image.json` and the manifest's `rootfs` field.
- `BWRAP_IMAGE=alpine-3.23-dev` selects `<BWRAP_IMAGE_STORE>/alpine-3.23-dev/image.json` and the manifest's `rootfs` field.
- `BWRAP_ROOTFS=/path/to/rootfs` bypasses image-store lookup for development, but should still load an adjacent or explicitly configured manifest when possible.
- Optional alias files such as `<image-store>/aliases/debian-current -> ../debian-13-dev` are allowed only if resolved paths stay inside the image store and metadata records the resolved image id.


### Package installation model for bwrap

MVP package installation policy:

- Runtime system package installation is required for the bwrap backend.
- The shared image rootfs in `BWRAP_IMAGE_STORE` must remain immutable during normal agent execution.
- Runtime `apt install`, `apt upgrade`, `apk add`, package database writes, cache writes, and system file edits must persist in the per-scope root overlay, not in the shared image rootfs.
- Each agent/scope gets its own writable system layer under `BWRAP_STATE_DIR`.
- Package changes are isolated by scope. Installing `ffmpeg` in scope A must not make it available in scope B unless both scopes intentionally use the same scope state.

Effective root model:

```text
image lowerdir:  <image-store>/<image-id>/rootfs          # immutable, shared
scope upperdir:  <state>/scopes/<scope>/system/upper     # persistent, per scope
scope workdir:   <state>/scopes/<scope>/system/work/<id> # empty per command, same filesystem as upper
sandbox root:    /                                      # overlay view
workspace bind:  <state>/scopes/<scope>/workspace -> /workspace
```

Debian behavior:

- `apt`, `dpkg`, `/var/lib/apt`, `/var/lib/dpkg`, `/etc/apt`, `/usr`, `/bin`, `/lib*`, and related writes go to the scope overlay upperdir.
- Example agent command:

```bash
apt-get update && apt-get install -y jq ffmpeg python3-venv
```

Alpine behavior:

- `apk`, `/lib/apk/db`, `/etc/apk`, `/usr`, `/bin`, `/lib`, and related writes go to the scope overlay upperdir.
- Example agent command:

```bash
apk add --no-cache jq ffmpeg python3 py3-pip
```

Python/application dependencies are supported in two ways:

1. Workspace-local dependency state, recommended for project dependencies:

```bash
python3 -m venv /workspace/.venv
. /workspace/.venv/bin/activate
pip install requests beautifulsoup4 httpx
```

2. System/site package state, allowed when the scope needs OS-level persistence:

```bash
pip install yt-dlp
python3 -m pip install --upgrade pip
```

If `pip` writes into system site-packages, those writes must land in the per-scope overlay and persist for that scope only.

Network behavior matters:

- `BWRAP_NET=host` allows `apt`, `apk`, `pip`, `curl`, `git`, and language package managers to download packages, subject to host firewall/DNS.
- `BWRAP_NET=none` prevents downloads from inside sandbox commands. Workspace-local wheels, source trees, vendored packages, or pre-populated package caches can still be used.

DNS/package-manager requirements:

- Rootfs images should include valid package manager repository config (`/etc/apt/sources.list` or `/etc/apt/sources.list.d/*` for Debian; `/etc/apk/repositories` for Alpine).
- Rootfs images should include CA certificates.
- DNS must be explicitly handled. Preferred MVP: generate or bind a safe read-only resolver file into `/etc/resolv.conf` when `BWRAP_NET=host`, controlled by `BWRAP_RESOLV_CONF=auto|none|<path>`.
- Do not bind the entire host `/etc`.

Concurrency rule:

- In `overlay-rw` mode, commands in the same scope must be serialized by default.
- Reason: two concurrent bwrap processes must not mount/write the same overlay upperdir and package manager database at the same time.
- Cross-scope concurrency is allowed because each scope has its own upperdir/workspace.

Reset behavior:

- Normal command execution preserves both system overlay and `/workspace`.
- `recreate_sandbox()` resets both the system overlay and `/workspace` for that scope.
- `destroy()` removes the whole scope state.
- The shared image store is never removed by scope recreate/destroy.

Read-only mode:

- `BWRAP_ROOT_MODE=ro` remains valid for locked-down deployments and tests.
- In `ro` mode, `apt install`, `apk add`, and system writes are expected to fail, while `/workspace` remains writable.

MVP acceptance rule: `SANDBOX_BACKEND=bwrap` must support installing packages inside the sandbox with Debian `apt` and Alpine `apk` when `BWRAP_ROOT_MODE=overlay-rw` and network/package repositories are configured.

## 9. Filesystem and Persistence Model

### State location precedence

Recommended precedence:

1. Explicit env/config:
   - `BWRAP_ROOTFS`
   - `BWRAP_IMAGE_STORE`
   - `BWRAP_STATE_DIR`
2. Repo-local state when running from a repo checkout and writable:
   - `.oxide/sandbox`
3. XDG state dir for user-mode host binary:
   - `$XDG_STATE_HOME/oxide-agent/sandbox`
   - fallback `$HOME/.local/state/oxide-agent/sandbox`
4. Service state dir when configured or detected as service mode:
   - `/var/lib/oxide-agent/sandbox`
5. Binary-adjacent state:
   - not used implicitly in MVP
   - allowed only when explicitly set by `BWRAP_STATE_DIR`

Rationale:

- Repo-local state is convenient for development.
- XDG state is correct for unprivileged standalone binary runs.
- `/var/lib/oxide-agent/sandbox` is correct for service deployment.
- Binary-adjacent writes are surprising and unsafe for packaged binaries.

### Suggested state layout

```text
.oxide/
  sandbox/
    images/
      debian-13-dev/
        rootfs/
        image.json
        checksums.txt
        provenance.json
      alpine-3.23-dev/
        rootfs/
        image.json
        checksums.txt
        provenance.json
    scopes/
      agent-sandbox-u77-0123456789abcdef/
        system/
          upper/
            # persistent root overlay writes: apt/apk/db/cache/etc/usr/var changes
          work/
            # per-command empty overlayfs workdirs, same filesystem as upper
        workspace/
          # persistent files visible as /workspace
        tmp/
        metadata.json
        active/
    locks/
      agent-sandbox-u77-0123456789abcdef.lock
```

`system/upper` is persistent per scope. `system/work/<exec-id>` must be created empty for each command, used as overlayfs workdir, and removed after the command exits. It must live on the same filesystem as `system/upper`.

Do not place `system/upper` or `system/work` inside the image rootfs, repo root, host home, or any shared path outside the scope state directory.

### Persistent vs temporary

Persistent:

- `scopes/<scope>/system/upper/` for package installs and system/rootfs mutations
- `scopes/<scope>/workspace/` for project/data files
- `scopes/<scope>/metadata.json`

Temporary:

- bwrap `/tmp` tmpfs inside each command
- per-command overlay workdirs under `scopes/<scope>/system/work/<exec-id>/`
- host staging files under `scopes/<scope>/tmp/`
- active command pid/status files under `scopes/<scope>/active/`

Deleted by `recreate()`:

- `system/upper/`
- `system/work/`
- `workspace/`
- `tmp/`
- active command state after commands are stopped
- generation-specific transient metadata

Preserved by `recreate()`:

- scope directory
- `metadata.json`, rewritten with incremented generation
- image/rootfs store
- lock file

Reset by `recreate()`:

- `/workspace` contents
- per-scope system overlay upperdir
- per-scope overlay workdirs
- active command metadata

Deleted by `destroy()`:

- entire `scopes/<scope>/` directory
- transient lock state if safe

Not deleted by `destroy()`:

- image store
- other scopes
- repo files
- config files

### Workspace-only file tools

MVP file tools are restricted to `/workspace`.

Path policy:

- `foo.txt` maps to `<scope>/workspace/foo.txt`.
- `/workspace/foo.txt` maps to `<scope>/workspace/foo.txt`.
- `/tmp/foo.txt` is rejected for file tools, even though `/tmp` exists during command execution.
- `/etc/passwd` is rejected.
- `../../x` is rejected.
- symlink escapes are rejected.

Reason:

- This keeps tool behavior simple and safe.
- Commands can still create temporary files in `/tmp` during execution, but those are not persistent and are not exposed through file tools.

### Avoid accidental host access

Do not bind mount:

- repo root
- host home
- `.git`
- `.env`
- `config/`
- SSH keys
- cloud credentials
- Docker socket
- `/var/run/docker.sock`
- `/run/sandboxd`
- `/run/user/*`
- host `/tmp`

The only writable host paths mounted into bwrap by default are:

- the per-scope system overlay upperdir/workdir used to make `/` writable inside the sandbox;
- the per-scope workspace mounted at `/workspace`.

The shared image rootfs is mounted only as an immutable lower layer and must never receive package-manager writes.

### `.gitignore` update

Add:

```gitignore
.oxide/sandbox/
```

Optional broader ignore after review:

```gitignore
.oxide/
!.oxide/.gitkeep
```

The implementation must not commit rootfs tarballs, unpacked rootfs directories, scope workspaces, or lock files.

## 10. Network Model

### Supported MVP modes

MVP supports exactly two network modes:

- `BWRAP_NET=host`
- `BWRAP_NET=none`

Invalid values fail fast.

### `host` mode

Behavior:

- Do not pass `--unshare-net`.
- The sandbox process uses the agent process network namespace.
- DNS, git, curl, pip, apt, and other network tools can work if rootfs has config and host network allows it.

Security tradeoff:

- Less isolation.
- Network exfiltration is possible from sandboxed commands.

Default recommendation:

- Use `host` as MVP default for compatibility with current agent tool expectations and Docker bridge availability.
- Production deployments that need stronger isolation should set `BWRAP_NET=none` or use Docker backend with network policies.

### `none` mode

Behavior:

- Pass `--unshare-net`.
- The sandbox gets a new network namespace.
- Only loopback is available.

Security benefit:

- Blocks most network exfiltration.

Compatibility cost:

- Breaks git/curl/wget/pip/apt/yt-dlp and other network-dependent commands.

### Not in MVP

Do not claim support for:

- Docker bridge networking.
- Docker Compose service DNS.
- NAT setup.
- port publishing.
- slirp networking.
- rootless user-mode network helper.

V2 can consider a slirp-like helper, but that is an explicit additional dependency and design.

## 11. Resource Limits Model

### Baseline statement

bwrap is not a cgroup runtime by itself. It does not automatically provide Docker-like memory, CPU, pids, blkio, or network resource limits.

The current Docker backend uses Docker host config for memory and CPU. The bwrap MVP cannot claim parity unless separate cgroup/systemd integration is implemented.

### MVP resource controls

MVP must implement:

- command timeout in Rust
- cancellation handling
- process/process-group kill on timeout or cancellation
- stdout size cap
- stderr size cap
- combined output cap or independent stream caps
- file read size cap for `read_file`
- optional max write size guard for `write_file`

Recommended defaults:

- `BWRAP_COMMAND_TIMEOUT_SECS=60`
- `BWRAP_MAX_OUTPUT_BYTES=16777216` per stream or combined, final value to be confirmed
- `BWRAP_MAX_READ_FILE_BYTES=52428800`, matching the current Docker download safety intent

Output truncation must be explicit in tool output. Infrastructure logs should include original size and captured size.

### Process killing

Use bwrap with `--new-session` and spawn as a process group/session where possible. On timeout/cancel:

- send termination to the child process group
- wait short grace period
- send kill to process group
- reap process
- clean active command metadata

Do not leave orphaned long-running processes.

### Shell `ulimit` wrapper

MVP may wrap command execution with shell limits if safe and portable enough:

```sh
ulimit -n 1024
ulimit -f 1048576
exec /bin/sh -lc "$COMMAND"
```

But shell `ulimit` is not memory/cpu cgroup parity and behaves differently across shells. Treat it as best-effort only.

### V2 resource controls

V2 options:

- `systemd-run --user --scope` for bare host with systemd.
- systemd service-level `MemoryMax`, `CPUQuota`, `TasksMax` around the agent process.
- cgroup v2 integration per command.
- pids.max per command.
- IO limits if needed.
- Compose-level limits when the agent itself runs in Docker.

### Docker Compose container-level limits

If bwrap runs inside the `oxide_agent` Docker container, Docker can limit the `oxide_agent` container as a whole. That does not provide per-sandbox-command isolation and does not equal Docker backend per-container sandbox limits.

## 12. Docker Compose Compatibility

### Existing Docker Compose path

Keep the current default path:

- `oxide_agent` uses `SANDBOX_BACKEND=broker`.
- `sandboxd` owns Docker socket access.
- `sandbox_image` builds the Debian sandbox Docker image.
- Docker/Bollard remains available in full compose profile.

This path should remain the safest supported Docker Compose deployment.

### Docker Compose + bwrap support level

Docker Compose + bwrap is a separate compatibility mode. Do not merge it into default `docker-compose.yml` until it is smoke-tested on target hosts.

Add an optional override only after dedicated compose+bwrap smoke testing, for example:

```text
docker/compose.bwrap-dev.yml
```

This override may:

- install `bubblewrap` in app image or use a bwrap-enabled app image variant
- mount an image store volume
- mount a state volume
- set `SANDBOX_BACKEND=bwrap`
- set `BWRAP_IMAGE_STORE=/var/lib/oxide-agent/sandbox/images`
- set `BWRAP_STATE_DIR=/var/lib/oxide-agent/sandbox/scopes`
- set `BWRAP_ROOTFS=/var/lib/oxide-agent/sandbox/images/debian-13-dev/rootfs`

### Why this is tricky

bwrap needs namespace/mount operations. Docker default seccomp blocks or restricts namespace and mount syscalls. Depending on kernel, Docker version, AppArmor/SELinux, and container settings, bwrap may fail with errors like:

```text
No permissions to create new namespace
```

or:

```text
Operation not permitted
```

Potential requirements include:

- custom Docker seccomp profile
- `security_opt: ["seccomp=unconfined"]` for dev-only testing
- AppArmor adjustment
- SELinux label adjustment on SELinux hosts
- `cap_add: ["SYS_ADMIN"]` in some environments
- user namespace support
- mount namespace support
- careful `/proc` behavior
- careful `/dev` behavior

`privileged: true` may make bwrap work, but it is too broad for production and should be discouraged.

### Recommendation

Safest supported path:

- For Docker Compose deployments, keep current Docker backend with `sandboxd`.

Safest bwrap path:

- Run Oxide Agent as a bare host binary with `SANDBOX_BACKEND=bwrap`.

Dev-compatible path:

- Provide a separate compose override for bwrap with clear warnings.
- Use only on developer machines or controlled test environments.
- Run smoke tests before claiming support.

Unsupported/pathologically unsafe path:

- `privileged: true`
- bind mounting `/`
- bind mounting host home
- bind mounting repo root writable
- bind mounting Docker socket into bwrap sandbox
- disabling all security profiles without compensating controls

## 13. Bare Binary Deployment

### General host binary requirements

For `SANDBOX_BACKEND=bwrap`, the host must provide:

- Oxide Agent binary compiled with `sandbox-backend-bwrap`.
- `bwrap` executable available in `PATH` or configured with `BWRAP_BIN`.
- rootfs image unpacked and verified.
- writable state dir for the agent user.
- user namespaces available.
- mount namespaces available.
- enough filesystem permissions to create workspace, locks, metadata.

No Docker daemon is required.

No Docker socket is required.

No Docker API is used.

### Service layout

Recommended service state:

```text
/var/lib/oxide-agent/sandbox/
  images/
  scopes/
  locks/
```

Recommended service user:

```text
oxide-agent
```

Recommended ownership:

```text
oxide-agent:oxide-agent /var/lib/oxide-agent/sandbox
```

### Upgrade behavior

Rootfs upgrades must be explicit.

Recommended behavior:

- Image IDs are immutable enough for reproducibility, e.g. `debian-13-dev-2026-05-25`.
- `BWRAP_IMAGE=debian-13-dev` may point to a symlink/alias only if metadata records the resolved image ID.
- Existing scope metadata records the image ID/rootfs used when created.
- On image upgrade, existing scopes keep their workspace and start using the newly configured rootfs only after validation.
- If this behavior is considered risky, add `BWRAP_PIN_IMAGE_PER_SCOPE=true` in v2.

### Logs

bwrap command output is captured into tool outputs. Infrastructure logs should include:

- scope name
- backend `bwrap`
- image id
- rootfs path
- command duration
- exit status
- timeout/cancel state
- output truncation state

Do not log full commands at info level if they may contain secrets. Use debug level with caution.

## 14. Debian 13 Host Requirements

### Install packages

Runtime requirement:

```bash
sudo apt update
sudo apt install bubblewrap ca-certificates tar xz-utils
```

Rootfs build-time requirement if building locally:

```bash
sudo apt install mmdebstrap
```

`mmdebstrap` is build-time only. It should not be required to run Oxide Agent if a prebuilt rootfs tarball is delivered.

### User namespace availability

Debian official kernels normally support user namespaces. The installer or startup check should still validate:

```bash
bwrap --version
bwrap --unshare-user --uid 0 --gid 0 --unshare-pid --proc /proc --dev /dev --tmpfs /tmp /bin/sh -c 'true'
```

That command uses host `/bin/sh`; final Oxide tests must use the configured rootfs.

### State directory

Development mode:

```bash
mkdir -p .oxide/sandbox/images .oxide/sandbox/scopes .oxide/sandbox/locks
```

Service mode:

```bash
sudo install -d -o oxide-agent -g oxide-agent -m 0750 /var/lib/oxide-agent/sandbox
sudo install -d -o oxide-agent -g oxide-agent -m 0750 /var/lib/oxide-agent/sandbox/images
sudo install -d -o oxide-agent -g oxide-agent -m 0750 /var/lib/oxide-agent/sandbox/scopes
sudo install -d -o oxide-agent -g oxide-agent -m 0750 /var/lib/oxide-agent/sandbox/locks
```

### systemd service considerations

A systemd unit may use:

```ini
User=oxide-agent
Group=oxide-agent
StateDirectory=oxide-agent
WorkingDirectory=/var/lib/oxide-agent
Environment=SANDBOX_BACKEND=bwrap
Environment=BWRAP_STATE_DIR=/var/lib/oxide-agent/sandbox/scopes
Environment=BWRAP_IMAGE_STORE=/var/lib/oxide-agent/sandbox/images
```

Be careful with systemd hardening options. Some namespace restrictions can break bwrap. Validate before enabling:

- `RestrictNamespaces=`
- `PrivateUsers=`
- `SystemCallFilter=`
- `NoNewPrivileges=`
- `ProtectHome=`
- `ProtectSystem=`

Do not add hardening that prevents user/mount/pid namespaces until tested.

### Smoke test

Example smoke test after implementation:

```bash
SANDBOX_BACKEND=bwrap \
BWRAP_ROOTFS=.oxide/sandbox/images/debian-13-dev/rootfs \
BWRAP_STATE_DIR=.oxide/sandbox/scopes \
BWRAP_IMAGE_STORE=.oxide/sandbox/images \
./oxide-agent-telegram-bot --sandbox-smoke 'cat /etc/os-release && echo ok > /workspace/ok.txt'
```

If the app has no CLI smoke command, add one or provide an integration test binary/script.

## 15. Alpine Host Requirements

### Install packages

Runtime:

```bash
apk add bubblewrap ca-certificates tar xz
```

If using static bwrap package where preferred:

```bash
apk add bubblewrap-static
```

Package naming must be verified for the specific Alpine release and architecture in CI docs.

### Agent binary

The precompiled host binary for Alpine should be built for musl, for example:

```text
x86_64-unknown-linux-musl
```

Do not ship a glibc-linked host binary and assume it runs on Alpine.

A Debian rootfs inside bwrap is still OK because Debian glibc lives inside the sandbox rootfs. The agent binary itself runs on the Alpine host and should not depend on host glibc.

### Kernel requirements

Verify Alpine host has:

- user namespace support
- mount namespace support
- PID namespace support
- IPC namespace support
- UTS namespace support
- network namespace support if `BWRAP_NET=none`
- overlayfs support for default `BWRAP_ROOT_MODE=overlay-rw`

### Service management

Alpine often uses OpenRC, not systemd.

OpenRC service must set:

- environment variables
- working directory
- service user/group
- state dir creation/ownership
- restart policy

Example state setup:

```bash
addgroup -S oxide-agent
adduser -S -G oxide-agent -h /var/lib/oxide-agent oxide-agent
install -d -o oxide-agent -g oxide-agent -m 0750 /var/lib/oxide-agent/sandbox
install -d -o oxide-agent -g oxide-agent -m 0750 /var/lib/oxide-agent/sandbox/images
install -d -o oxide-agent -g oxide-agent -m 0750 /var/lib/oxide-agent/sandbox/scopes
install -d -o oxide-agent -g oxide-agent -m 0750 /var/lib/oxide-agent/sandbox/locks
```

### Rootfs delivery

For Alpine host + Debian sandbox rootfs, prefer prebuilt Debian rootfs tarball over requiring local `mmdebstrap`.

Reason:

- Alpine host is a runtime target, not necessarily a Debian rootfs build machine.
- `mmdebstrap` workflows and dependencies are more natural on Debian/Ubuntu.
- CI-produced tarballs with checksums/signatures are simpler for Alpine operators.

### Shell/tool caveats

Alpine rootfs uses BusyBox by default:

- `/bin/sh` is available.
- `bash` is not available unless installed.
- GNU `stat`, `find`, `grep`, and other tools can differ from Debian.

Avoid hardcoding GNU-specific shell behavior in bwrap backend. Rust-side file ops should avoid shelling out for path safety.

## 16. Configuration

### Existing variables to keep

`SANDBOX_BACKEND`

- Purpose: select sandbox backend.
- Default today: `docker`.
- Valid values after feature: `docker`, `broker`, `bwrap`.
- Required: no, but bwrap host profile should set it explicitly.
- Invalid behavior: fail fast with valid values and compiled backend list.

`SANDBOX_IMAGE`

- Purpose: Docker image reference for Docker backend.
- Default: `agent-sandbox:latest`.
- Used by: Docker backend and sandboxd-client.
- Bwrap behavior: ignored unless a future compatibility alias maps it; do not use as bwrap image ID in MVP.
- Invalid behavior in bwrap mode: if set but `BWRAP_IMAGE`/`BWRAP_ROOTFS` is missing, warn that `SANDBOX_IMAGE` is Docker-only.

`SANDBOXD_SOCKET`

- Purpose: Unix socket path for Docker sandbox broker.
- Default: `/run/sandboxd/sandboxd.sock`.
- Bwrap behavior: ignored in MVP.

`STACK_LOGS_PROJECT`

- Purpose: Docker Compose stack log discovery.
- Bwrap behavior: unsupported in MVP.

### New bwrap variables

`BWRAP_BIN`

- Purpose: path/name of bwrap executable.
- Default: `bwrap`.
- Valid values: executable path or binary name found in `PATH`.
- Required: no.
- Invalid behavior: actionable error:

```text
Bwrap backend selected, but BWRAP_BIN='bwrap' was not found or is not executable. Install bubblewrap or set BWRAP_BIN=/path/to/bwrap.
```

`BWRAP_IMAGE`

- Purpose: image ID under `BWRAP_IMAGE_STORE`.
- Default: `debian-13-dev`.
- Valid values: directory name containing `image.json` and rootfs.
- Required: no if `BWRAP_ROOTFS` is set; otherwise yes by default.
- Invalid behavior: error names expected image path and suggests image build/fetch script.

`BWRAP_IMAGE_STORE`

- Purpose: directory containing bwrap image directories.
- Default: resolved by state precedence, usually `.oxide/sandbox/images` in repo-local mode.
- Valid values: readable directory.
- Required: no if `BWRAP_ROOTFS` points directly to rootfs.
- Invalid behavior: error with path and creation/build instructions.

`BWRAP_ROOTFS`

- Purpose: explicit rootfs path. Overrides `BWRAP_IMAGE` lookup.
- Default: unset.
- Valid values: existing rootfs directory.
- Required: no if image store/image ID is valid.
- Invalid behavior: fail if missing, not directory, unsafe symlink, wrong arch, or missing manifest when manifest is required.

`BWRAP_STATE_DIR`

- Purpose: parent directory for bwrap scope state.
- Default: resolved by state precedence, usually `.oxide/sandbox/scopes` in repo-local mode.
- Valid values: writable directory or creatable parent.
- Required: no.
- Invalid behavior: fail with permissions/path message.

`BWRAP_LOCK_DIR`

- Purpose: optional explicit lock dir.
- Default: sibling of scopes, usually `.oxide/sandbox/locks`.
- Valid values: writable directory.
- Required: no.
- Invalid behavior: fail with permissions/path message.

`BWRAP_NET`

- Purpose: network namespace policy.
- Default: `host` for compatibility.
- Valid values: `host`, `none`.
- Required: no.
- Invalid behavior: fail with valid values.

`BWRAP_ROOT_MODE`

- Purpose: effective rootfs write policy.
- Default: `overlay-rw` for bwrap profile.
- Valid values for MVP: `overlay-rw`, `ro`.
- Optional/dev value: `tmp-overlay` if implemented.
- Required: no.
- Invalid behavior: fail with valid values. If `overlay-rw` is selected but host/bwrap/kernel support is missing, fail with an actionable overlay support error.

`BWRAP_ROOT_UPPER_DIR`

- Purpose: optional override for the persistent per-scope system upperdir parent. Usually not needed.
- Default: `<BWRAP_STATE_DIR>/<scope>/system/upper`.
- Valid values: writable directory on the same filesystem as the workdir.
- Required: no.
- Invalid behavior: fail if not writable, unsafe symlink, or incompatible with overlay workdir.

`BWRAP_RESOLV_CONF`

- Purpose: DNS config for package managers when `BWRAP_NET=host`.
- Default: `auto`.
- Valid values: `auto`, `none`, or path to a resolver file to bind read-only at `/etc/resolv.conf`.
- Required: no.
- Invalid behavior: fail if path is selected and missing/unsafe. Never bind the whole host `/etc`.

`BWRAP_COMMAND_TIMEOUT_SECS`

- Purpose: per-command timeout.
- Default: use existing `SANDBOX_EXEC_TIMEOUT_SECS` value, currently 60, unless set.
- Valid values: positive integer.
- Required: no.
- Invalid behavior: fail or fall back with warning; recommended fail for explicit invalid value.

`BWRAP_MAX_OUTPUT_BYTES`

- Purpose: stdout/stderr capture cap.
- Default: proposed 16 MiB per stream, final value to confirm.
- Valid values: positive integer.
- Required: no.
- Invalid behavior: fail for invalid explicit value.

`BWRAP_MAX_READ_FILE_BYTES`

- Purpose: max bytes returned by `read_file`.
- Default: proposed 50 MiB.
- Valid values: positive integer.
- Required: no.
- Invalid behavior: fail for invalid explicit value.

`BWRAP_ALLOW_OVERLAY`

- Purpose: safety gate for writable root overlay support in deployments that want to force read-only mode.
- Default: `true` for bwrap profile.
- Valid values: `true`, `false`, `1`, `0`.
- Required: no.
- Invalid behavior: fail. If set to `false`, only `BWRAP_ROOT_MODE=ro` is allowed.

`BWRAP_DISABLE_NESTED_USERNS`

- Purpose: pass `--disable-userns` to prevent nested user namespace creation inside sandbox.
- Default: `true`.
- Valid values: `true`, `false`, `1`, `0`.
- Required: no.
- Invalid behavior: fail. If true but installed bwrap does not support it, fail closed unless an explicit dev override is added.

`BWRAP_RECREATE_LOCK_TIMEOUT_SECS`

- Purpose: how long recreate/destroy waits for active commands.
- Default: `BWRAP_COMMAND_TIMEOUT_SECS + 5`.
- Valid values: positive integer.
- Required: no.
- Invalid behavior: fail.

### Example `.env`

```env
SANDBOX_BACKEND=bwrap
BWRAP_BIN=bwrap
BWRAP_IMAGE=debian-13-dev
BWRAP_IMAGE_STORE=.oxide/sandbox/images
BWRAP_STATE_DIR=.oxide/sandbox/scopes
BWRAP_ROOTFS=.oxide/sandbox/images/debian-13-dev/rootfs
BWRAP_NET=host
BWRAP_ROOT_MODE=overlay-rw
BWRAP_COMMAND_TIMEOUT_SECS=60
BWRAP_MAX_OUTPUT_BYTES=16777216
BWRAP_ALLOW_OVERLAY=true
BWRAP_RESOLV_CONF=auto
BWRAP_DISABLE_NESTED_USERNS=true
```

## 17. Security Model and Risk Register

### Security baseline

Bwrap is a primitive. Oxide Agent must define the policy.

MVP security posture:

- shared image rootfs immutable
- effective `/` writable only through per-scope overlay upperdir
- `/workspace` writable as separate per-scope project/data state
- `/tmp` tmpfs
- clear env
- no host home mount
- no repo root mount
- no Docker socket mount
- no `.git` access
- no `.env` access
- no secrets passed through env
- no arbitrary host path mounts
- user namespace enabled
- PID namespace enabled
- IPC namespace enabled
- UTS namespace enabled
- nested user namespace disabled by default
- network host or none explicitly configured
- command timeout and output caps
- per-scope locks
- verified rootfs manifest/checksum before production use

### Threat: path traversal

Risk:

- User passes `../..` or absolute host path to file tools.

Mitigation:

- Normalize all tool paths through one bwrap workspace path resolver.
- Reject `..`, NUL bytes, Windows prefixes, and unsupported absolute paths.
- Accept only relative paths or absolute `/workspace/...` paths.
- Unit-test resolver extensively.

Status: must fix in MVP.

### Threat: symlink escape

Risk:

- File tools follow symlinks from workspace to host paths.

Mitigation:

- Reject symlinked path components.
- Use file-descriptor-relative open APIs with no-follow semantics where possible.
- Do not rely only on `canonicalize()` prefix checks.
- Add tests with symlinks pointing outside workspace.

Status: must fix in MVP.

### Threat: host secret leakage

Risk:

- Sandbox command reads `.env`, config files, SSH keys, cloud credentials, or host home.

Mitigation:

- Do not bind host home.
- Do not bind repo root.
- Do not bind config.
- Use `--clearenv`.
- Allowlist only safe env from image manifest.
- Never mount Docker socket.

Status: must fix in MVP.

### Threat: accidental bind mount of repo root

Risk:

- Developer binds entire repo for convenience and exposes `.git`, `.env`, source, agent binary, or secrets.

Mitigation:

- Explicitly disallow repo root bind by default.
- Do not offer `BWRAP_BIND_REPO_ROOT` in MVP.
- If arbitrary bind mounts are ever added, require explicit allowlist and read-only default.

Status: not allowed in MVP.

### Threat: Docker socket exposure

Risk:

- Mounting `/var/run/docker.sock` into bwrap gives sandbox effective host control.

Mitigation:

- Never mount Docker socket in bwrap mode.
- Add smoke test that `/var/run/docker.sock` is absent.
- Docs must state this clearly.

Status: must fix in MVP.

### Threat: network exfiltration

Risk:

- `BWRAP_NET=host` allows commands to exfiltrate data.

Mitigation:

- Support `BWRAP_NET=none`.
- Default `host` only for compatibility, documented as accepted risk.
- Allow production deployments to set `none`.
- Do not pass secrets into sandbox.

Status: accepted risk for `host`; mitigated by `none` option.

### Threat: nested user namespaces

Risk:

- Process creates further namespaces to rearrange filesystem isolation.

Mitigation:

- `BWRAP_DISABLE_NESTED_USERNS=true` default.
- Pass `--disable-userns` when supported.
- Fail closed if unavailable in production mode.

Status: must fix in MVP.

### Threat: privilege escalation through writable mounts

Risk:

- Writable rootfs or host paths allow persistent compromise or host writes.

Mitigation:

- Shared image rootfs immutable.
- Effective root writes are confined to the per-scope overlay upperdir.
- `/workspace` is the only direct project/data writable bind mount.
- No arbitrary host binds.
- No host home/repo/config binds.
- Serialize commands per scope in `overlay-rw` mode to avoid overlay/package database corruption.

Status: must fix in MVP.

### Threat: `/proc` leakage

Risk:

- `/proc` exposes host or process info.

Mitigation:

- Use `--unshare-pid` with `--proc /proc`.
- Do not bind host `/proc`.
- Validate bwrap behavior inside Docker separately.

Status: must fix in MVP.

### Threat: `/dev` exposure

Risk:

- Broad `/dev` access exposes host devices.

Mitigation:

- Use `--dev /dev`, not `--dev-bind /dev /dev`.
- Do not mount GPU, audio, block devices, or host device nodes in MVP.

Status: must fix in MVP.

### Threat: setuid binaries inside rootfs

Risk:

- setuid binaries could elevate inside namespace or exploit kernel bugs.

Mitigation:

- bwrap uses no-new-privs behavior.
- Rootfs build scripts should strip or audit setuid binaries where practical.
- The shared image rootfs is immutable, but overlay-rw mode can persist new setuid files inside the scope overlay. bwrap/no-new-privs and user namespace boundaries mitigate host impact; production images should still strip/audit setuid binaries and tests should verify no host privilege gain path is exposed.

Status: mitigated; audit rootfs before enabling production rootfs artifacts.

### Threat: Linux capabilities

Risk:

- Capabilities inside user namespace may allow mount namespace manipulation.

Mitigation:

- Use `--disable-userns`.
- Keep writable mounts minimal.
- Do not bind sensitive host filesystems.
- No `--cap-add` concept in bwrap; avoid Docker container capabilities unless in dev compatibility mode.

Status: mitigated.

### Threat: running inside Docker with privileged mode

Risk:

- `privileged: true` weakens container boundary and can undermine bwrap isolation.

Mitigation:

- Do not use privileged Compose as supported production mode.
- Provide dev-only override with warnings if needed.
- Keep Docker Compose default on existing Docker backend/broker.

Status: not allowed for production.

### Threat: malicious output flooding

Risk:

- Command writes unlimited stdout/stderr causing memory/disk pressure.

Mitigation:

- Stream output with caps.
- Truncate after `BWRAP_MAX_OUTPUT_BYTES`.
- Kill or continue-with-truncation; recommended MVP is kill when combined cap is exceeded unless product chooses otherwise.

Status: must fix in MVP.

### Threat: long-running process/fork bomb

Risk:

- Commands do not terminate or fork heavily.

Mitigation:

- Timeout and cancellation.
- Process group kill.
- Optional shell `ulimit -u` if available.
- V2 cgroup pids.max.

Status: timeout must fix in MVP; fork bomb fully mitigated only with cgroup v2.

### Threat: leftover processes after timeout

Risk:

- Child processes survive after command timeout.

Mitigation:

- `--new-session`.
- Process group tracking/kill.
- `--die-with-parent`.
- Active command metadata cleanup.

Status: must fix in MVP.

### Threat: concurrent state corruption

Risk:

- Recreate/delete while exec or file write is active.

Mitigation:

- Per-scope filesystem locks.
- In-process RwLock already exists but is not enough.
- Generation metadata.

Status: must fix in MVP.

### Threat: rootfs tampering

Risk:

- Rootfs contents modified by attacker, poisoning future commands.

Mitigation:

- Shared image rootfs immutable at runtime; only per-scope overlay is writable.
- Verify checksum/signature/provenance before use.
- Store images outside writable workspace.
- File permissions should prevent agent command from writing image store.

Status: checksum in MVP; signature before production recommended.

### Threat: poisoned image store

Risk:

- Attacker writes malicious `image.json` or rootfs.

Mitigation:

- Image store not mounted into sandbox.
- State/workspace separate from image store.
- Production image store owned by service user/root with controlled permissions.
- Verify manifest and checksum.

Status: must fix before production.

### Threat: supply chain of rootfs tarballs

Risk:

- Downloaded rootfs tarball is malicious or corrupted.

Mitigation:

- Pin URLs.
- Verify SHA256.
- Prefer signed releases.
- Record provenance.
- CI-generated artifacts should have checksums/signatures.

Status: checksum MVP; signatures recommended.

### Threat: gitignored local state accidentally committed

Risk:

- Rootfs or workspace data committed to repo.

Mitigation:

- Add `.oxide/sandbox/` to `.gitignore`.
- Keep rootfs tarballs out of repo.

Status: must fix in MVP.

### Threat: sandbox modifying agent binary or repo

Risk:

- Writable bind mount allows command to alter source or binary.

Mitigation:

- Do not bind repo root or binary dir.
- Only `/workspace` writable.
- Binary-adjacent state not used implicitly.

Status: must fix in MVP.

## 18. Tool/API Behavior

### `execute_command`

Docker behavior today:

- Ensures container exists.
- Runs Docker exec in `/workspace`.
- Returns stdout/stderr/exit code.

Bwrap behavior:

- Ensures scope state exists.
- Starts fresh bwrap process for every command.
- Uses rootfs manifest shell, default `/bin/sh`.
- Working directory is `/workspace`.
- Captures stdout/stderr with limits.
- Returns non-zero command exit as normal `ExecResult` failure status, not infrastructure error.

Error examples:

```text
Bwrap backend selected, but rootfs not found at .oxide/sandbox/images/debian-13-dev/rootfs. Run scripts/build-bwrap-rootfs-debian.sh or set BWRAP_ROOTFS.
```

```text
Bwrap command timed out after 60s and the process group was killed.
```

### `write_file`

Docker behavior today:

- Uploads through Docker copy/tar to arbitrary provided container path.

Bwrap behavior:

- Writes directly to scope workspace on host.
- Creates parent dirs inside workspace.
- Rejects paths outside `/workspace`.
- Rejects path traversal and symlink escapes.
- Returns bytes written.

### `read_file`

Docker behavior today:

- Downloads through Docker copy/tar.

Bwrap behavior:

- Reads directly from scope workspace on host.
- Rejects paths outside `/workspace`.
- Rejects directories.
- Enforces max read size.
- Preserves binary detection at tool output layer.

### `list_files`

Docker behavior today:

- Runs `tree` or `find` in container.

Bwrap behavior:

- Lists workspace files directly in Rust.
- Enforces workspace-only path policy.
- Returns text compatible with current `SandboxFileListing`.
- Does not require `tree` to exist in rootfs.

### `recreate_sandbox`

Docker behavior today:

- Removes and recreates Docker container, wiping workspace state.

Bwrap behavior:

- Acquires exclusive scope lock.
- Waits for active commands or fails/terminates according to policy.
- Deletes and recreates `workspace/`.
- Clears temp state.
- Rewrites metadata with new generation.
- Does not delete image store.

Tool message can remain conceptually similar but should avoid “container” wording:

```text
Sandbox recreated successfully. Previous workspace contents were removed.
```

### `destroy_sandbox` / admin delete

Bwrap behavior:

- Deletes per-scope state dir.
- Does not delete rootfs image.
- Returns success when already absent.
- Admin deletion by name uses the same stable scope name compatibility as Docker container name.

### Stack logs / container logs

Bwrap MVP does not support Docker Compose stack logs.

Behavior under bwrap:

- `tool-stack-logs` should not compile into bwrap-only profile.
- If compiled in a multi-backend binary but `SANDBOX_BACKEND=bwrap`, stack logs should return an explicit unsupported error:

```text
Stack logs are Docker/Compose diagnostics and are not supported by SANDBOX_BACKEND=bwrap.
```

### Manager control-plane sandbox tools

`crates/oxide-agent-core/src/agent/providers/manager_control_plane/sandboxes.rs` currently says “Docker container name” and serializes `TopicSandboxInventoryRecord` fields named `container_id` and `container_name`.

MVP compatibility decision:

- Keep JSON fields for now to avoid breaking clients.
- For bwrap records:
  - `container_id`: `bwrap:<scope_name>`
  - `container_name`: existing stable scope name
  - `image`: bwrap image ID or rootfs path
  - `state`: `ready`, `running`, `missing`, or `error`
  - `status`: human-readable bwrap status
  - `running`: true when active command metadata exists
  - labels include `agent.sandbox=true`, `agent.sandbox_backend=bwrap`, `agent.user_id`, `agent.scope`, optional chat/thread labels

Docs and tool descriptions should be updated from “container” to “sandbox instance” where possible.

## 19. Cargo Features and Build Profiles

### Existing relevant features

In `crates/oxide-agent-core/Cargo.toml`:

- `sandbox-backend-docker-direct`
- `sandbox-backend-sandboxd-client`
- `sandbox-broker-protocol`
- `sandbox-daemon`
- `tool-sandbox-fileops`
- `tool-sandbox-exec`
- `tool-sandbox-recreate`
- `tool-stack-logs`

Current implications:

- `sandbox-backend-docker-direct` pulls `bollard`, `tar`, `bytes`, `http-body-util`, and broker protocol.
- `sandbox-backend-sandboxd-client` pulls broker protocol but not `bollard`.
- `sandbox-daemon` pulls direct Docker.
- `tool-stack-logs` pulls direct Docker.

### New feature

Add:

```toml
sandbox-backend-bwrap = [
    # Keep empty if using only existing deps,
    # or include optional small deps required for safe locking/path handling.
]
```

If new dependencies are needed, keep them bwrap-scoped:

```toml
fs2 = { version = "...", optional = true }
cap-std = { version = "...", optional = true }
# or rustix with fs features if chosen

sandbox-backend-bwrap = [
    "dep:fs2",
    "dep:cap-std",
]
```

Do not include:

- `dep:bollard`
- `dep:tar` unless needed independently for rootfs unpack scripts inside Rust, which MVP should avoid
- `sandbox-broker-protocol`
- `sandbox-daemon`

### New profile feature

Add a bwrap host profile in `crates/oxide-agent-core/Cargo.toml`:

```toml
profile-host-bwrap = [
    "transport-telegram",
    "storage-s3-r2",
    "llm-opencode-go",
    "tool-todos",
    "tool-compression",
    "tool-delegation",
    "tool-agents-md",
    "tool-reminder",
    "tool-wiki-memory",
    "tool-webfetch-md",
    "tool-file-delivery",
    "tool-media-audio",
    "tool-media-image",
    "tool-media-video",
    "tool-tavily",
    "tool-sandbox-fileops",
    "tool-sandbox-exec",
    "tool-sandbox-recreate",
    "sandbox-backend-bwrap"
]
```

Do not include:

- `sandbox-backend-docker-direct`
- `sandbox-backend-sandboxd-client`
- `sandbox-daemon`
- `tool-stack-logs`

Add `profiles/host-bwrap.toml` with module:

```toml
profile = "host-bwrap"
cargo_features = ["profile-host-bwrap"]

[modules]
"sandbox-backend/bwrap" = { enabled = true }
"tool/sandbox-exec" = { enabled = true }
"tool/sandbox-fileops" = { enabled = true }
"tool/sandbox-recreate" = { enabled = true }
# plus the selected transport/storage/LLM/tool modules for this product profile
```

### Optional multi-backend dev profile

Add only if needed:

```toml
profile-dev-all-sandboxes = [
    "profile-embedded-opencode-local",
    "sandbox-backend-bwrap"
]
```

This profile may include both Docker and bwrap for local development/testing. It must not become the production default by accident.

### Existing profiles policy

`profile-full`

- Keep current Docker/broker behavior unchanged for Docker Compose.
- Do not add bwrap in the first implementation unless there is a product decision to ship all backends in full.
- If bwrap is added to full later, ensure default `SANDBOX_BACKEND=broker` in Compose remains explicit.

`profile-embedded-opencode-local`

- Currently includes direct Docker backend and sandbox tools.
- Keep unchanged for compatibility.
- Add separate bwrap profile rather than silently changing this one.

`profile-no-sandbox`, `profile-lite`, `profile-search-only`, `profile-media-enabled`

- Keep without sandbox backend/tools unless product owner intentionally changes them.

### `oxide-agent-sandboxd`

Keep `oxide-agent-sandboxd` Docker-only in MVP.

Do not add bwrap to `sandbox-daemon` until a backend-independent broker design exists.

### Cargo tree acceptance

Must pass after implementation:

```bash
cargo tree -p oxide-agent-core \
  --no-default-features \
  --features sandbox-backend-bwrap | grep -E '(^| )bollard v' && exit 1 || true
```

And for the host profile:

```bash
cargo tree -p oxide-agent-core \
  --no-default-features \
  --features profile-host-bwrap | grep -E '(^| )bollard v' && exit 1 || true
```

Update `scripts/check-cargo-tree-deny.sh` with:

```bash
profile-host-bwrap)
    deny=(bollard tar http-body-util bincode serde_bytes)
    ;;
sandbox-backend-bwrap)
    deny=(bollard tar http-body-util bincode serde_bytes)
    ;;
```

If bwrap implementation intentionally uses `tar` in Rust for rootfs unpacking, adjust this, but MVP should keep rootfs lifecycle in scripts and not require `tar` crate.

### Capability metadata updates

Update `crates/oxide-agent-core/src/capabilities/compiled.rs`:

- Add bwrap to fileops backend capability array.
- Add bwrap to exec backend capability array.
- Add bwrap to lifecycle backend capability array.
- Do not add bwrap to diagnostics backend capability array.
- Push a new runtime module `sandbox-backend/bwrap` under `sandbox-backend-bwrap`.

Required module capabilities:

- `sandbox-backend/bwrap`
- `sandbox-backend/bwrap/fileops`
- `sandbox-backend/bwrap/exec`
- `sandbox-backend/bwrap/lifecycle`

Update manifest tests in `crates/oxide-agent-core/src/capabilities/manifest.rs` to expect bwrap as a valid backend when feature-gated.

Update capability scripts:

- `scripts/check-compiled-capabilities.sh`
- `scripts/check-registry-snapshots.sh`
- `scripts/check-profile-size-budget.sh` if new profile has budget.
- `profiles/host-bwrap.toml` must match compiled manifest.

## 20. Testing Plan

### Unit tests: path safety

Add tests for a shared resolver, for example in `crates/oxide-agent-core/src/sandbox/bwrap.rs` or `path_policy.rs`:

- `foo.txt` resolves inside workspace.
- `dir/foo.txt` resolves inside workspace.
- `/workspace/foo.txt` resolves inside workspace.
- `/workspace/dir/foo.txt` resolves inside workspace.
- `..` is rejected.
- `../x` is rejected.
- `/workspace/../x` is rejected.
- `/etc/passwd` is rejected.
- empty path behavior is explicit.
- path with NUL is rejected.
- path with symlink parent escaping workspace is rejected.
- final symlink escape is rejected for reads.
- final symlink is not followed for writes.
- Unicode and spaces are preserved safely.

### Unit tests: config parsing

- `SANDBOX_BACKEND=bwrap` parses to bwrap.
- Invalid backend returns valid values.
- `BWRAP_NET=host` valid.
- `BWRAP_NET=none` valid.
- invalid `BWRAP_NET` rejected.
- `BWRAP_ROOT_MODE=overlay-rw` valid.
- `BWRAP_ROOT_MODE=ro` valid.
- `BWRAP_ALLOW_OVERLAY=false` rejects `overlay-rw`.
- missing overlay support returns actionable error.
- missing bwrap binary error is actionable.
- missing rootfs error is actionable.
- invalid image manifest rejected.
- arch mismatch rejected.

### Unit tests: state lifecycle

- create sandbox state creates scope dirs.
- create writes metadata.
- create is idempotent.
- recreate deletes workspace contents.
- recreate increments generation.
- recreate does not delete image store.
- destroy removes scope state.
- destroy absent scope is OK.
- list user sandboxes returns bwrap compatibility records.
- inspect by name returns only user-owned scope.
- delete by name refuses wrong owner.

### Integration tests: bwrap command execution

Mark as ignored or gated when rootfs/bwrap is absent:

```bash
SANDBOX_BACKEND=bwrap \
BWRAP_ROOTFS=.oxide/sandbox/images/debian-13-dev/rootfs \
BWRAP_STATE_DIR=.oxide/sandbox/scopes \
cargo test -p oxide-agent-core \
  --no-default-features \
  --features 'sandbox-backend-bwrap,tool-sandbox-exec,tool-sandbox-fileops,tool-sandbox-recreate' \
  bwrap_smoke -- --ignored
```

Test cases:

- command success: `echo ok`
- command failure: `exit 7` returns exit code 7
- stdout capture
- stderr capture
- large stdout truncation
- timeout kills process
- cancellation kills process
- environment isolation: secret host env absent
- `pwd` is `/workspace`
- `/tmp` exists and is tmpfs per process
- `/workspace` persists across commands
- overlay-rw mode: writing `/etc/oxide-test` succeeds and persists across commands in the same scope
- ro mode: writing `/etc/oxide-test` fails
- no access to host repo root
- no access to `.env`
- overlay-rw mode: `apt-get update && apt-get install -y jq` succeeds in Debian rootfs when network/repositories are configured
- overlay-rw mode: installed `jq` remains available in a later command in the same scope
- overlay-rw mode: installed `jq` is not available in a different fresh scope unless present in the base image
- overlay-rw mode: concurrent commands in the same scope are serialized
- `BWRAP_NET=none`: package downloads fail cleanly without corrupting package database

### Debian rootfs smoke test

```bash
SANDBOX_BACKEND=bwrap \
BWRAP_IMAGE=debian-13-dev \
BWRAP_NET=host \
./scripts/smoke-bwrap.sh debian-13-dev
```

Expected:

- `cat /etc/os-release` shows Debian/trixie.
- `python3 --version` works for dev image.
- `curl --version` works for dev image.
- `git --version` works.
- write/read `/workspace/hello.txt` works.

### Alpine rootfs smoke test

```bash
SANDBOX_BACKEND=bwrap \
BWRAP_IMAGE=alpine-3.23-dev \
BWRAP_NET=host \
./scripts/smoke-bwrap.sh alpine-3.23-dev
```

Expected:

- `cat /etc/os-release` shows Alpine.
- `/bin/sh -c 'echo ok'` works.
- BusyBox caveats documented.
- Workspace persistence works.

### Network tests

Host mode:

```bash
BWRAP_NET=host execute_command 'getent hosts debian.org || true'
```

None mode:

```bash
BWRAP_NET=none execute_command 'wget -qO- https://example.com'
```

Expected for none mode:

- command fails due no external network.
- error is command failure, not infrastructure failure.

### Concurrent exec behavior

- Start long-running command in scope.
- Start another command in same scope if allowed by policy.
- Trigger recreate while command runs.
- Confirm recreate waits/fails/kills according to spec.
- Confirm workspace state is not corrupted.

MVP concurrency recommendation:

- Allow concurrent exec only if workspace consistency risk is accepted.
- Serialize recreate/destroy against all exec/fileops.
- If concurrent writes are not safe, serialize all bwrap operations per scope using exclusive lock. This is simpler and safer for MVP, but can reduce parallelism.

Final implementation must choose explicitly. Recommended MVP: serialize all operations per scope with one exclusive lock, then relax to shared/exclusive later if needed.

### Cargo feature tests

Must pass:

```bash
cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap
cargo check -p oxide-agent-core --no-default-features --features 'sandbox-backend-bwrap,tool-sandbox-exec,tool-sandbox-fileops,tool-sandbox-recreate'
cargo check -p oxide-agent-core --no-default-features --features profile-host-bwrap
cargo check -p oxide-agent-core --no-default-features --features profile-full
cargo check -p oxide-agent-sandboxd --no-default-features --features profile-full
```

Must prove no `bollard` in bwrap-only:

```bash
scripts/check-cargo-tree-deny.sh sandbox-backend-bwrap
scripts/check-cargo-tree-deny.sh profile-host-bwrap
```

Docker still compiles:

```bash
cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-docker-direct
cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-sandboxd-client
cargo check -p oxide-agent-core --no-default-features --features profile-full
```

### Docker Compose default smoke test

Current workflow must continue:

```bash
docker compose up --build -d
```

Expected:

- `sandbox_image` builds.
- `sandboxd` starts.
- `oxide_agent` connects to `/run/sandboxd/sandboxd.sock`.
- sandbox tools work through Docker broker.
- stack logs still work in full profile.

### Docker Compose + bwrap smoke test

Only after explicit override exists:

```bash
docker compose -f docker-compose.yml -f docker/compose.bwrap-dev.yml up --build -d
```

Expected:

- mode is marked dev/compat.
- startup verifies `bwrap` inside app container.
- rootfs mounted/present.
- one command executes.
- shared image rootfs immutable.
- `/workspace` persists on configured volume.

If this cannot be made to work without elevated settings, document it directly and keep unsupported until requirements are met.

### Bare Debian 13 smoke test

- Install bwrap.
- Fetch/build Debian rootfs.
- Run bwrap profile binary.
- Execute command.
- Verify persistent workspace.
- Verify no Docker daemon/socket needed.

### Bare Alpine host smoke test

- Install bwrap via apk.
- Run musl binary.
- Use Debian rootfs tarball.
- Execute command.
- Verify persistent workspace.
- Repeat with Alpine rootfs if supported.

## 21. Acceptance Criteria

### MVP acceptance in the current development environment

The following criteria are release-blocking for the implementation agent working inside the current development sandbox.

- `SANDBOX_BACKEND=bwrap` can be selected through config/env without requiring Docker daemon, Docker socket, Docker API, or `bollard`.
- The bwrap-only build profile does not include `bollard` in `cargo tree`.
- Docker backend still compiles and existing Docker/Docker Compose workflow is not broken.
- Bwrap backend can create a scope state directory under configured `BWRAP_STATE_DIR`.
- Bwrap backend can resolve an image through `BWRAP_IMAGE` and `BWRAP_IMAGE_STORE`, or through explicit `BWRAP_ROOTFS`.
- Bwrap backend persists `/workspace` across command executions within the same scope.
- Bwrap backend supports per-scope writable system state through `BWRAP_ROOT_MODE=overlay-rw`, or fails with an actionable error if overlay mode is unavailable in the current nested sandbox.
- `exec_command` runs through a fresh `bwrap` process per command.
- `write_file`, `read_file`, `list_files`, and `apply_file_edit` work against `/workspace`.
- `apply_file_edit` preserves read-snapshot guard semantics, expected replacement count validation, and SHA-256 before/after reporting.
- File tools cannot escape `/workspace` through `..`, absolute host paths, or symlink traversal.
- Per-scope operations are serialized so package-manager writes, `write_file`, `apply_file_edit`, `recreate_sandbox`, and `destroy_sandbox` cannot corrupt shared scope state.
- Missing `bwrap`, missing rootfs, unsupported overlay mode, invalid image manifest, and invalid state directory permissions produce actionable errors.
- `BWRAP_NET=host` and `BWRAP_NET=none` are represented in config and in bwrap invocation construction.
- Output capture enforces `BWRAP_MAX_OUTPUT_BYTES` and reports truncation metadata.
- Command timeout kills or terminates the bwrap process tree and reports timeout status.
- `recreate_sandbox` resets the per-scope workspace and writable system overlay according to the PRD.
- `destroy_sandbox` removes the per-scope state directory and does not delete shared image rootfs data.
- Repo-local sandbox state is ignored by git.
- A self-test command or script exists for the current development environment and records whether the environment is nested bwrap, Docker, or bare host.

### Platform certification criteria

The following criteria are required before declaring a platform officially supported, but they are not required to be executed by an implementation agent running inside a nested Fedora+bwrap development sandbox.

- Bare binary on Debian 13 is documented with install requirements, rootfs setup, state directory setup, and smoke-test commands.
- Bare binary on Alpine host is documented with install requirements, rootfs setup, state directory setup, and smoke-test commands.
- Debian host + Debian rootfs has a smoke-test script and expected output.
- Debian host + Alpine rootfs has a smoke-test script and expected output, or is explicitly marked optional pending with a concrete reason.
- Alpine host + Debian rootfs has a smoke-test script and expected output.
- Alpine host + Alpine rootfs has a smoke-test script and expected output, or is explicitly marked optional pending with a concrete reason.
- Docker Compose + bwrap has documented requirements and is marked experimental/dev-only until manually smoke-tested on target hosts.
- If Docker Compose + bwrap requires `privileged: true` on a target host, that path is marked unsafe/dev-only and is not considered a normal supported deployment mode.

### Platform certification evidence

Each platform smoke test should produce a small machine-readable result file, for example:

```json
{
  "platform": "debian-13-host",
  "rootfs": "debian-13-dev",
  "backend": "bwrap",
  "nested": false,
  "bwrap_version": "0.11.x",
  "root_mode": "overlay-rw",
  "network_mode": "host",
  "tests": {
    "create_scope": "pass",
    "exec_command": "pass",
    "workspace_persistence": "pass",
    "apt_or_apk_install": "pass",
    "pip_install": "pass",
    "apply_file_edit": "pass",
    "path_escape_rejected": "pass",
    "timeout": "pass",
    "destroy_scope": "pass"
  }
}

## 22. Product and Architecture Decisions

These questions are intentionally resolved here instead of being left open. The goal is to keep the bwrap backend implementation narrow, testable, and safe without blocking Docker/Docker Compose users.

### Build profiles

Decision: keep `sandbox-backend-bwrap` out of `profile-full` for MVP.

MVP should introduce a dedicated host profile, for example `profile-host-bwrap` or equivalent, that enables `sandbox-backend-bwrap` without Docker direct dependencies. `profile-full` may include bwrap later only after bwrap has stable CI coverage on Debian 13 and Alpine hosts and after `cargo tree` confirms that the intended bwrap-only profile does not include `bollard`.

Rationale: `profile-full` is too broad for first integration. Adding bwrap there immediately increases dependency and support surface. The product goal is not to migrate Docker users; it is to add a standalone host backend that can run without Docker daemon, Docker socket, Docker API, or `bollard`.

### Admin inventory naming

Decision: introduce `SandboxInstanceRecord` before exposing bwrap admin inventory.

`SandboxContainerRecord` may remain as a Docker compatibility type internally, but new admin-facing inventory should use backend-neutral naming. For bwrap, the runtime object is not a container: it is a scope directory plus metadata, workspace, writable system overlay, locks, and optionally active process records.

Required shape:

- `SandboxInstanceRecord`
- `backend: docker | sandboxd | bwrap`
- `scope_id`
- `state_dir`
- `workspace_dir`
- `image_id`
- `rootfs_path`
- `root_mode`
- `network_mode`
- `created_at`
- `last_used_at`
- `status`
- backend-specific metadata under an explicit nested field

Rationale: using `ContainerRecord` for bwrap leaks Docker semantics into the new backend and will make future admin tooling confusing.

### Debian image package set

Decision: create bwrap image variants, but ship only `debian-13-dev` first.

`debian-13-dev` should be derived from the practical package set in `sandbox/Dockerfile.dev`, adjusted for rootfs/bootstrap constraints. It should be the first supported and smoke-tested image.

Do not block MVP on `exec`, `media`, and `minimal` variants. Define the naming convention now, but implement additional variants after the first Debian rootfs is stable:

- `debian-13-dev`
- `debian-13-exec`
- `debian-13-media`
- `debian-13-minimal`
- `alpine-3.23-dev`

Rationale: one high-quality Debian rootfs with package installation support is more valuable than several half-tested variants. Variant support should be an image lifecycle feature, not a blocker for the backend.

### Rootfs signing

Decision: use release checksums for MVP; design manifests so `cosign` can be added later.

MVP should publish:

- rootfs tarball
- `SHA256SUMS`
- `image.json`
- optionally `SHA256SUMS.txt` generated by CI

The `image.json` schema should reserve fields for future signing and provenance:

- `sha256`
- `source`
- `created_at`
- `builder`
- `provenance`
- `signature`
- `signature_type`

Post-MVP should prefer `cosign` for signing release blobs/artifacts. Do not use GPG as the default product path unless the project already has release-key management in place. Do not introduce minisign unless the project wants a deliberately smaller non-Sigstore signing workflow.

Rationale: release checksums are enough for local MVP smoke testing, while the manifest remains compatible with stronger signing later.

### Rootfs pinning per scope

Decision: pin rootfs image identity per scope.

When a scope is created, persist the selected image identity in `metadata.json`:

- `image_id`
- `image_manifest_sha256`
- `rootfs_path`
- `rootfs_sha256`, if available
- `root_mode`

Existing scopes must continue using their pinned image metadata unless the user explicitly runs a recreate/rebase operation.

Changing `BWRAP_IMAGE` affects only newly created scopes by default.

Rationale: package installs are persisted in the per-scope system overlay. Letting old scopes silently follow a new base rootfs can break package databases, dynamic linker expectations, Python environments, and agent-created files.

### Network default

Decision: default `BWRAP_NET=host` for the developer/agent profile; require `BWRAP_NET=none` for hardened production profiles.

The default host-bwrap developer profile should use `host` because package managers and language tooling commonly need network access:

- `apt-get update`
- `apt-get install`
- `apk add`
- `pip install`
- `git clone`
- `curl`

Production or restricted deployments should explicitly set:

```env
BWRAP_NET=none

## 23. Appendix: Candidate bwrap Invocation

Candidate MVP invocation shape:

```bash
bwrap \
  --unshare-user \
  --uid 0 \
  --gid 0 \
  --unshare-pid \
  --unshare-ipc \
  --unshare-uts \
  --unshare-cgroup-try \
  --die-with-parent \
  --new-session \
  --clearenv \
  --setenv HOME /workspace \
  --setenv PATH /usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  --setenv LANG C.UTF-8 \
  --setenv TMPDIR /tmp \
  --overlay-src "$ROOTFS" \
  --overlay "$ROOT_UPPER" "$ROOT_WORK" / \
  --proc /proc \
  --dev /dev \
  --tmpfs /tmp \
  --bind "$WORKSPACE" /workspace \
  --chdir /workspace \
  --disable-userns \
  /bin/sh -lc "$COMMAND"
```

When `BWRAP_NET=none`, add:

```bash
--unshare-net
```

When `BWRAP_NET=host`, do not add `--unshare-net`.

Implementation details:

- Build bwrap args with `std::process::Command` / `tokio::process::Command`, not shell string concatenation.
- Only the inner user command is passed to `/bin/sh -lc`.
- Use manifest `default_shell`; do not hardcode `/bin/sh` without validation.
- Validate `--disable-userns` support if enabled.
- Validate rootfs has mount points and shell.
- Capture stdout/stderr with streaming caps.
- Kill process group on timeout/cancel.
- In `BWRAP_NET=host`, provide `/etc/resolv.conf` through `BWRAP_RESOLV_CONF=auto|none|<path>` so `apt`, `apk`, `pip`, and `curl` can resolve package repositories without binding the whole host `/etc`.
- In `BWRAP_ROOT_MODE=ro`, replace the overlay arguments with `--ro-bind "$ROOTFS" /` and keep `/workspace` writable.
- In `BWRAP_ROOT_MODE=overlay-rw`, create a fresh empty `ROOT_WORK` directory for each command on the same filesystem as `ROOT_UPPER`, and serialize commands per scope.
- Do not use `--dev-bind /dev /dev`; use `--dev /dev`.
- Do not bind `/sys` in MVP unless a specific tool requires it and the risk is accepted.

Possible future additions:

- `--json-status-fd` for structured child status/pid tracking.
- `--seccomp FD` for custom syscall policy.
- `--tmp-overlay /` for temporary writable root experiments if needed.

## 24. Appendix: Suggested State Layout

Development repo-local layout:

```text
.oxide/
  sandbox/
    images/
      debian-13-dev/
        rootfs/
          bin/
          dev/
          etc/
          proc/
          tmp/
          usr/
          var/
          workspace/
        image.json
        checksums.txt
        provenance.json
      alpine-3.23-dev/
        rootfs/
        image.json
        checksums.txt
        provenance.json
    scopes/
      agent-sandbox-u77-0123456789abcdef/
        system/
          upper/
            # persistent root overlay writes: apt/apk/db/cache/etc/usr/var changes
          work/
            # per-command empty overlayfs workdirs, same filesystem as upper
        workspace/
          # persistent files visible as /workspace
        tmp/
          # host-side staging only, not sandbox /tmp
        active/
          # command metadata/pids/status during execution
        metadata.json
    locks/
      agent-sandbox-u77-0123456789abcdef.lock
```

### Deployment-aware storage layout

The bwrap backend must not assume that the repository checkout is present at runtime. Oxide Agent may run in several deployment modes:

- local development from a repository checkout;
- standalone precompiled binary with a `.env` file next to the binary;
- system service;
- Docker Compose deployment;
- Docker Compose image that already contains prebuilt bwrap rootfs assets.

Because of this, storage must be split into two concepts:

1. **Image store** — unpacked bwrap rootfs images and immutable image metadata.
2. **Runtime state** — per-agent/per-scope writable sandbox state, overlays, workspaces, locks, and metadata.

The image store may be read-only after installation. Runtime state must be writable by the Oxide Agent process.

Recommended variables:

```env
OXIDE_STATE_DIR=/var/lib/oxide-agent
BWRAP_IMAGE_STORE=/opt/oxide-agent/bwrap-images
BWRAP_STATE_DIR=/var/lib/oxide-agent/sandbox/scopes
BWRAP_LOCK_DIR=/var/lib/oxide-agent/sandbox/locks
````

Do not rely on shell-style variable interpolation inside `.env` files unless the config loader explicitly implements and tests it. For MVP, prefer fully resolved absolute paths in production examples.

#### Workspace mount mapping

```text
host:    ${BWRAP_STATE_DIR}/<scope>/workspace
sandbox: /workspace
mode:    read-write
persistence: persistent until recreate/destroy
```

#### Rootfs overlay mapping when `BWRAP_ROOT_MODE=overlay-rw`

```text
lower host: ${BWRAP_IMAGE_STORE}/<image-id>/rootfs
upper host: ${BWRAP_STATE_DIR}/<scope>/system/upper
work host:  ${BWRAP_STATE_DIR}/<scope>/system/work
sandbox:    /
mode:       overlay-rw
persistence: lower is immutable/shared; upper is persistent per scope until recreate/destroy
```

The overlay work directory must be on the same filesystem as the upper directory. Per-scope operations must be serialized in MVP. If the implementation chooses per-command work directories, they must be created empty before each command and removed after command completion.

#### Read-only root mapping when `BWRAP_ROOT_MODE=ro`

```text
host:    ${BWRAP_IMAGE_STORE}/<image-id>/rootfs
sandbox: /
mode:    read-only
persistence: immutable image content only
```

`BWRAP_ROOT_MODE=ro` is valid for restricted execution, but it does not satisfy the package-install use case because `apt`, `apk`, and system-level `pip` installs need writable system paths. The developer/agent profile should default to `overlay-rw`.

#### Tmp mapping

```text
sandbox: /tmp
mode:    tmpfs
persistence: per-command only
```

#### Forbidden default mappings

The bwrap backend must not mount the following host paths into the sandbox by default:

```text
host repo root -> sandbox
host home -> sandbox
host .git -> sandbox
host .env -> sandbox
host config -> sandbox
host /var/run/docker.sock -> sandbox
host /run/sandboxd -> sandbox
host / -> sandbox
```

The only default writable host-backed mount exposed to sandbox tools should be `/workspace`, backed by `${BWRAP_STATE_DIR}/<scope>/workspace`. System writes inside the sandbox must go through the per-scope overlay upperdir, not by binding arbitrary host directories.
