# Goal: Bwrap Sandbox Backend

Date started: 2026-05-25
Status: active
Codex goal: Implement the roadmap described in docs/prd/prd.md for Oxide Agent, with repository-local /goal documentation and commits at each major checkpoint/phase.

## Objective

Implement the PRD-defined Bubblewrap sandbox backend for Oxide Agent so `SANDBOX_BACKEND=bwrap` can run sandbox tools from a bwrap-only host profile without Docker daemon access, Docker socket access, Docker API calls, or `bollard` in the dependency tree. The stopping condition is a compiling implementation with repository docs, rootfs scripts/smoke checks, cargo feature validation, path-safety tests, and no regression to existing Docker/broker workflows.

## Scope

In scope:
- Add `sandbox-backend-bwrap` and `profile-host-bwrap` feature/profile wiring.
- Add explicit sandbox backend selection for `docker`, `broker`, and `bwrap`.
- Implement bwrap scope state, manifest/rootfs validation, per-scope locking, command execution, workspace-only file operations, lifecycle reset/destroy, and admin inventory.
- Add bwrap capability metadata, profile manifest, scripts, `.env.example`, README/docs, and `.gitignore` updates.
- Preserve Docker direct, Docker broker/sandboxd, and stack-logs behavior.

Out of scope:
- Replacing Docker globally or changing the default Docker Compose path.
- Routing bwrap through `sandboxd`.
- Docker image pulling/building in bwrap mode.
- Docker bridge/cgroup/log parity for bwrap.
- Arbitrary host bind mounts, host home/repo binds, Docker socket binds, or rootfs mutation in the shared image store.
- Production rootfs signing beyond checksum/provenance-ready metadata.

## Repository Context

- PRD: `docs/prd/prd.md`.
- Core sandbox facade: `crates/oxide-agent-core/src/sandbox/manager.rs`, `mod.rs`, `traits.rs`, `scope.rs`, `admin.rs`.
- Sandbox tool provider: `crates/oxide-agent-core/src/agent/providers/sandbox.rs`.
- Capability metadata: `crates/oxide-agent-core/src/capabilities/`, `profiles/*.toml`, `scripts/check-*.sh`.
- Config/env: `crates/oxide-agent-core/src/config.rs`, `.env.example`, README/docs.
- Existing feature policy: default features remain empty; `profile-full` keeps current Docker/broker behavior for MVP.
- Scale principle: personal deployment, up to 5 RPS; prefer simple serialized per-scope bwrap operations over complex concurrency.

## Implementation Plan

1. Goal and architecture checkpoint: create this goal document and commit it.
2. Feature/config/capability checkpoint: add bwrap feature/profile, backend selection enum, scope stable naming, compiled capabilities, profile manifest, and cargo-tree deny coverage.
3. Bwrap core checkpoint: implement `crates/oxide-agent-core/src/sandbox/bwrap.rs` with config parsing, manifest validation, state layout, metadata, locking, and command invocation.
4. Tool/lifecycle/admin checkpoint: wire bwrap into `SandboxManager`, implement workspace-only file operations, list/read/write/apply-edit safety, recreate/destroy, and neutral admin inventory where needed.
5. Rootfs and docs checkpoint: add Debian rootfs build script, smoke script/self-test surface, `.gitignore`, `.env.example`, README/deployment docs, and Docker Compose compatibility notes without changing default Compose behavior.
6. Validation checkpoint: run formatting, cargo checks, cargo tree denies, focused tests, and document any environment-only bwrap smoke limitations.

## Validation Contract

- Formatting: `cargo fmt`.
- Lint: `cargo clippy --workspace --no-default-features --features profile-host-bwrap`.
- Bwrap-only checks:
  - `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap`
  - `cargo check -p oxide-agent-core --no-default-features --features 'sandbox-backend-bwrap,tool-sandbox-exec,tool-sandbox-fileops,tool-sandbox-recreate'`
  - `cargo check -p oxide-agent-core --no-default-features --features profile-host-bwrap`
  - `scripts/check-cargo-tree-deny.sh sandbox-backend-bwrap`
  - `scripts/check-cargo-tree-deny.sh profile-host-bwrap`
- Regression checks:
  - `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-docker-direct`
  - `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-sandboxd-client`
  - `cargo check -p oxide-agent-core --no-default-features --features profile-full`
  - `cargo check -p oxide-agent-sandboxd --no-default-features --features profile-full`
- Focused tests: path policy, config parsing, state lifecycle, and bwrap smoke tests gated/ignored when `bwrap` or rootfs is absent.
- Done when: all required static checks pass or any host-environment-only smoke gap is documented with exact evidence and a runnable command.

## Decisions

- 2026-05-25: Keep bwrap out of `profile-full` for MVP; add a dedicated `profile-host-bwrap`.
- 2026-05-25: Keep Docker Compose default on broker/sandboxd and document Compose+bwrap as dev/compatibility only.
- 2026-05-25: Serialize all bwrap operations per scope for MVP to avoid overlay/package-manager/workspace corruption.
- 2026-05-25: File tools are workspace-only for bwrap and must reject traversal, unsafe absolute paths, and symlink escapes.
- 2026-05-25: Pin rootfs identity in bwrap scope metadata; changing `BWRAP_IMAGE` affects new scopes unless a recreate/rebase path is explicit.

## Progress Log

- 2026-05-25 19:29 +03: Read `docs/prd/prd.md`, repository instructions, README overview, and initial sandbox/Cargo files. Created active Codex goal and this repo-local goal contract. Next checkpoint: commit goal doc, then start feature/config/capability wiring.
- 2026-05-25 19:45 +03: Added `sandbox-backend-bwrap`, `profile-host-bwrap`, backend parsing for `docker|broker|bwrap`, bwrap module gates, stable scope naming, capability manifest entries, `profiles/host-bwrap.toml`, and cargo-tree deny coverage. Verified `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap`, bwrap+sandbox-tool core check, `profile-host-bwrap` core check, Telegram bot profile check, `scripts/check-cargo-tree-deny.sh sandbox-backend-bwrap`, `scripts/check-cargo-tree-deny.sh profile-host-bwrap`, and `scripts/check-compiled-capabilities.sh host-bwrap`. Next checkpoint: implement bwrap config/state/manifest/locking core.
- 2026-05-25 20:18 +03: Implemented bwrap core manager: env config, image manifest/rootfs validation, scope metadata, per-scope exclusive file lock, fresh bwrap command execution, timeout/cancel handling, output truncation markers, direct workspace read/write/list/size operations, recreate/destroy, uploads/downloads helpers, and bwrap admin inventory records. `SandboxRuntime::list_files` now uses backend file-listing capability so bwrap does not spawn `tree`/`find`. Verified `cargo check -p oxide-agent-core --no-default-features --features 'sandbox-backend-bwrap,tool-sandbox-exec,tool-sandbox-fileops,tool-sandbox-recreate'`, `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-docker-direct`, `cargo check -p oxide-agent-core --no-default-features --features profile-host-bwrap`, cargo-tree denies, and focused bwrap unit tests. Next checkpoint: rootfs/smoke scripts, docs, env examples, and gitignore.
- 2026-05-25 20:28 +03: Added `.oxide/sandbox/` gitignore, `scripts/build-bwrap-rootfs-debian.sh`, `scripts/smoke-bwrap.sh`, `.env.example` bwrap variables, README link, and `docs/bwrap-sandbox.md` with bare-host requirements and Compose compatibility stance. Verified `bash -n` for both scripts. Host has `/usr/bin/bwrap`, but rootfs is not present, so runtime smoke is pending until `scripts/build-bwrap-rootfs-debian.sh` or a prebuilt rootfs is used.

## Risks and Blockers

- Bwrap smoke execution may be limited by the current nested environment, missing `bwrap`, missing user namespace support, or absent rootfs. Mitigation: keep tests gated/ignored and provide scripts with actionable diagnostics.
- Full path race hardening may require adding a small bwrap-scoped dependency such as `cap-std` or `fs2`. Mitigation: keep dependencies optional under `sandbox-backend-bwrap` and prove `bollard` remains absent.
- Existing admin/control-plane APIs use container naming. Mitigation: introduce neutral naming where feasible while preserving serialized compatibility fields for existing clients.
- `http-body-util` cannot be denied at the full `profile-host-bwrap` cargo-tree level because reqwest-backed LLM/search/media modules pull it transitively. The backend-only deny still includes `http-body-util` and proves bwrap itself does not enable the Docker direct dependency path.

## Final Verification

- Pending.
