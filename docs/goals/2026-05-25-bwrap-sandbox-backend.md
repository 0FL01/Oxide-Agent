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
- 2026-05-25 20:05 +03: Added backend-neutral `SandboxInstanceRecord` projection and moved manager/control-plane topic sandbox inventory to `instance_*` fields while preserving `container_*` compatibility aliases. Added `instance_name` input alias for get/delete tools, bwrap rootfs/state/workspace labels, and focused manager sandbox tests. Verified `cargo test -p oxide-agent-core --no-default-features --features manager-control-plane manager_control_plane::tests::sandboxes --lib`, `cargo check -p oxide-agent-core --no-default-features --features 'sandbox-backend-bwrap,manager-control-plane'`, `cargo clippy --workspace --no-default-features --features profile-host-bwrap`, and `cargo check -p oxide-agent-core --no-default-features --features profile-full`.
- 2026-05-25 20:05 +03: Expanded `scripts/smoke-bwrap.sh` evidence to record `environment_kind` (`bare-host`, `docker-container`, or `kubernetes-container`) and verify that `/var/run/docker.sock` and `/run/sandboxd` are absent inside the bwrap sandbox. Updated `docs/bwrap-sandbox.md` with the smoke result contract.
- 2026-05-25 20:10 +03: Added a hermetic bwrap state lifecycle unit test using a fake executable and minimal fake rootfs. The test covers create, metadata record shape, workspace write/read/list/size, recreate workspace wipe, and destroy without requiring a real bwrap-capable host. Verified `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap bwrap::tests --lib` and `cargo fmt`.
- 2026-05-25 20:13 +03: Tightened sandbox tool descriptions to be backend-neutral: `execute_command` now describes shell execution in `/workspace` rather than Bash, file tools state that relative paths resolve under `/workspace` and absolute paths must start with `/workspace/`, and lifecycle wording uses sandbox instance instead of container. Added focused provider tests and verified `cargo test -p oxide-agent-core --no-default-features --features 'tool-sandbox-exec,tool-sandbox-fileops,tool-sandbox-recreate,sandbox-backend-bwrap' agent::providers::sandbox::tests --lib`, `cargo check` for the same feature set, and `cargo fmt --check`.
- 2026-05-25 20:15 +03: Added `host-bwrap` to the modular registry snapshot guard, generated the dedicated host-bwrap snapshot, and asserted that the profile enables `sandbox-backend/bwrap` while keeping Docker direct and sandboxd-client backends absent. Verified `INSTA_UPDATE=always scripts/check-registry-snapshots.sh host-bwrap`, `scripts/check-registry-snapshots.sh host-bwrap`, and `cargo fmt --check`.
- 2026-05-25 20:16 +03: Polished remaining public sandbox wording in README and sandbox admin trait docs from Docker/Bash-specific phrasing to backend-neutral sandbox instance/shell phrasing. Updated `scripts/check-runtime-env-surface.sh` to skip missing optional paths so the guard stays quiet when `README-ru.md` is absent. Verified `scripts/check-runtime-env-surface.sh`, `cargo check -p oxide-agent-core --no-default-features --features 'tool-sandbox-exec,tool-sandbox-fileops,tool-sandbox-recreate,sandbox-backend-bwrap'`, and `cargo fmt --check`.
- 2026-05-25 20:21 +03: Expanded bwrap test coverage with hermetic symlink escape tests for parent and final symlinks across write/read/list paths. Added ignored Rust smoke tests for real bwrap/rootfs execution covering `/workspace` working directory, workspace persistence, overlay-rw `/etc` persistence, Docker socket/sandboxd absence, and read-only root system-write rejection. Documented the ignored test command in `docs/bwrap-sandbox.md`. Verified `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap bwrap::tests --lib`, `cargo test -p oxide-agent-core --no-default-features --features 'sandbox-backend-bwrap,tool-sandbox-exec,tool-sandbox-fileops,tool-sandbox-recreate' bwrap_smoke --lib -- --ignored --list`, `cargo check -p oxide-agent-core --no-default-features --features 'sandbox-backend-bwrap,tool-sandbox-exec,tool-sandbox-fileops,tool-sandbox-recreate'`, and shell syntax for both bwrap scripts. Runtime smoke remains blocked by absent `.oxide/sandbox/images/debian-13-dev/rootfs` and absent `mmdebstrap`; `bwrap` itself is present.
- 2026-05-25 20:23 +03: Added a platform certification matrix to `docs/bwrap-sandbox.md` covering Debian host + Debian rootfs, Debian host + Alpine rootfs, Alpine host + Debian rootfs, Alpine host + Alpine rootfs, and Docker Compose + bwrap. Documented expected smoke JSON shape, Alpine host setup notes, and marked Alpine rootfs paths optional pending with the concrete reason that the MVP ships only the Debian 13 rootfs builder and still needs separate Alpine checksum/provenance plus BusyBox/GNU tool smoke coverage.
- 2026-05-25 20:27 +03: Added unit coverage for sandbox backend config parsing and feature/config mismatch reporting. Parser tests cover `docker`, `broker`, `bwrap`, case/trim handling, invalid backend errors, env parsing, and broker mode compatibility. Backend selection test now runs outside Docker-only tests and verifies that selecting an uncompiled backend reports the selected backend and compiled backend list. Verified `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap sandbox_backend --lib`, `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap selected_backend_reports_feature_config_mismatch --lib`, and `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap`.
- 2026-05-25 20:29 +03: Added hermetic bwrap error-path tests for missing `BWRAP_BIN`, missing `BWRAP_ROOTFS`, unsupported `BWRAP_ROOT_MODE=tmp-overlay`, and invalid image manifests with absolute rootfs or non-`/workspace` default workdir. Verified `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap bwrap::tests --lib`, `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap`, and `cargo fmt --check`.
- 2026-05-25 20:31 +03: Added hermetic bwrap exec behavior coverage using a fake executable. The test proves non-zero exit codes are preserved, stdout/stderr are truncated with diagnostic markers according to `BWRAP_MAX_OUTPUT_BYTES`, and command timeout returns an actionable timeout error. Verified `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap bwrap_exec_preserves_nonzero_exit_truncates_output_and_times_out --lib`, full `bwrap::tests`, `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap`, and `cargo fmt --check`.
- 2026-05-25 20:37 +03: Added `scripts/build-bwrap-rootfs-host-smoke.sh`, a dev-only host-derived rootfs builder for environments without `mmdebstrap` or a prebuilt Debian rootfs. Built `.oxide/sandbox/images/host-smoke-dev`, ran `scripts/smoke-bwrap.sh host-smoke-dev` successfully, and ran the ignored Rust bwrap smoke tests successfully against the host-smoke rootfs. This proves runtime bwrap create/exec/workspace persistence, overlay-rw system persistence, read-only root rejection, and Docker socket/sandboxd absence in the current environment; it does not certify Debian 13 rootfs package-manager behavior.
- 2026-05-25 20:40 +03: Added a multi-backend stack-logs regression test proving `SANDBOX_BACKEND=bwrap` returns the explicit unsupported Docker/Compose diagnostics error for both stack-log source listing and fetching. Verified `cargo test -p oxide-agent-core --no-default-features --features 'sandbox-backend-bwrap,tool-stack-logs' stack_logs_report_explicit_unsupported_error_under_bwrap --lib`.
- 2026-05-25 20:46 +03: Improved bwrap configuration diagnostics. A bwrap-only binary whose implicit default selects an uncompiled backend now tells the operator to set the single compiled backend, for example `SANDBOX_BACKEND=bwrap`; missing bwrap image/rootfs errors now tell operators that `SANDBOX_IMAGE` is Docker-only and ignored by `SANDBOX_BACKEND=bwrap`. Verified focused config tests, `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap`, and `cargo fmt --check`.
- 2026-05-25 20:49 +03: Added hermetic bwrap invocation-argument coverage. The test now asserts `BWRAP_NET=host` leaves networking shared, `BWRAP_NET=none` adds `--unshare-net`, `overlay-rw` uses bwrap overlay arguments, `ro` uses `--ro-bind`, commands chdir to `/workspace`, workspace is the only writable host bind, and Docker socket/sandboxd paths are not bound. Verified the focused invocation test, full `bwrap::tests`, `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap`, and `cargo fmt --check`.
- 2026-05-25 20:51 +03: Tightened bwrap image manifest validation so `rootfs` must be relative and cannot contain path components that escape the image directory. Added a regression test for `../rootfs` manifests. Verified full `bwrap::tests`, `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap`, and `cargo fmt --check`.
- 2026-05-25 20:56 +03: Implemented bounded bwrap scope-lock waiting with `BWRAP_RECREATE_LOCK_TIMEOUT_SECS`, defaulting to `BWRAP_COMMAND_TIMEOUT_SECS + 5`. Lock acquisition now uses nonblocking retries and returns an actionable timeout instead of waiting forever when another command/recreate/destroy owns the same scope. Updated `.env.example`, `docs/bwrap-sandbox.md`, and tests for the default and zero-value rejection. Verified full `bwrap::tests`, `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap`, and `cargo fmt --check`.
- 2026-05-25 20:59 +03: Hardened bwrap image-store rootfs resolution against symlink escapes. `BWRAP_IMAGE` now canonicalizes the selected image directory and rejects a manifest `rootfs` path whose canonical directory resolves outside that image directory, while direct `BWRAP_ROOTFS` remains an explicit development override. Added a regression test with an image-local `rootfs` symlink pointing outside the image directory. Verified focused config errors, full `bwrap::tests`, `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap`, `cargo fmt --check`, and host-smoke runtime smoke.
- 2026-05-25 21:02 +03: Expanded bwrap output truncation diagnostics. Tool output now reports both captured and original byte counts when stdout or stderr exceeds `BWRAP_MAX_OUTPUT_BYTES`, instead of only reporting the configured cap. Updated the exec-limit test to assert the exact truncation metadata. Verified the focused exec-limit test, full `bwrap::tests`, `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap`, `cargo clippy --workspace --no-default-features --features profile-host-bwrap`, and `cargo fmt --check`.
- 2026-05-25 21:04 +03: Made invalid bwrap state and lock directory errors name the exact config keys. `BWRAP_STATE_DIR` and `BWRAP_LOCK_DIR` now fail fast when they point to files instead of directories, with focused tests proving actionable error text. Verified the focused state/lock-dir test, full `bwrap::tests`, `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap`, `cargo clippy --workspace --no-default-features --features profile-host-bwrap`, and `cargo fmt --check`.
- 2026-05-25 21:10 +03: Exposed bwrap image manifest metadata in runtime records. New scope metadata and sandbox records now include the selected image manifest path, manifest SHA-256, and package manager when available, and status text reports `package_manager`, `manifest`, `rootfs`, root mode, and network mode. Existing metadata remains readable through optional fields. Verified focused metadata coverage, full `bwrap::tests`, `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap`, `cargo clippy --workspace --no-default-features --features profile-host-bwrap`, and `cargo fmt --check`.
- 2026-05-25 21:13 +03: Added `scripts/import-bwrap-rootfs-tar.sh` for checksum-required prebuilt rootfs imports. This supports Debian rootfs delivery on hosts without `mmdebstrap`, including Alpine hosts that should consume a verified prebuilt Debian rootfs rather than build one locally. Documented the import command in `docs/bwrap-sandbox.md`. Verified `bash -n scripts/import-bwrap-rootfs-tar.sh` and a functional local import smoke using a minimal tarball with mandatory SHA-256 validation.
- 2026-05-25 21:18 +03: Implemented `BWRAP_ROOT_UPPER_DIR` as an optional parent for per-scope persistent system overlay state. When set, bwrap stores upper/work under `<BWRAP_ROOT_UPPER_DIR>/<scope>/upper` and `<BWRAP_ROOT_UPPER_DIR>/<scope>/work` so both stay on the same filesystem. Added validation for file paths, direct symlinks, and paths under the shared rootfs image, plus docs and `.env.example` coverage. Verified focused override tests, full `bwrap::tests`, `cargo clippy --workspace --no-default-features --features profile-host-bwrap`, and `cargo fmt --check`.
- 2026-05-25 21:22 +03: Fixed bwrap lifecycle cleanup for `BWRAP_ROOT_UPPER_DIR`. `destroy()` and admin `delete_sandbox_by_name()` now remove the external per-scope overlay directory as well as the normal scope state directory, preventing package/system overlay state from surviving sandbox deletion. Extended the upperdir override test to cover both destroy and delete cleanup. Verified focused override tests, full `bwrap::tests`, `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap`, `cargo clippy --workspace --no-default-features --features profile-host-bwrap`, and `cargo fmt --check`.
- 2026-05-25 21:25 +03: Hardened direct `BWRAP_ROOTFS` overrides by rejecting symlink paths before canonicalization. This keeps the explicit development override from bypassing rootfs symlink policy while still allowing normal absolute rootfs directories. Added focused coverage in the actionable config-errors test. Verified focused config errors, full `bwrap::tests`, `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap`, `cargo clippy --workspace --no-default-features --features profile-host-bwrap`, and `cargo fmt --check`.

## Risks and Blockers

- Bwrap smoke execution may be limited by the current nested environment, missing `bwrap`, missing user namespace support, or absent rootfs. Mitigation: keep tests gated/ignored and provide scripts with actionable diagnostics.
- Full path race hardening may require adding a small bwrap-scoped dependency such as `cap-std` or `fs2`. Mitigation: keep dependencies optional under `sandbox-backend-bwrap` and prove `bollard` remains absent.
- Existing admin/control-plane APIs use container naming. Mitigation: introduce neutral naming where feasible while preserving serialized compatibility fields for existing clients.
- `http-body-util` cannot be denied at the full `profile-host-bwrap` cargo-tree level because reqwest-backed LLM/search/media modules pull it transitively. The backend-only deny still includes `http-body-util` and proves bwrap itself does not enable the Docker direct dependency path.

## Final Verification

- `cargo fmt --check` passed.
- `cargo clippy --workspace --no-default-features --features profile-host-bwrap` passed.
- `cargo check -p oxide-agent-core --no-default-features --features 'sandbox-backend-bwrap,tool-sandbox-exec,tool-sandbox-fileops,tool-sandbox-recreate'` passed.
- `cargo check -p oxide-agent-core --no-default-features --features profile-host-bwrap` passed.
- `cargo check -p oxide-agent-core --no-default-features --features profile-full` passed.
- `cargo check -p oxide-agent-core --no-default-features --features sandbox-backend-docker-direct` passed.
- `cargo check -p oxide-agent-sandboxd --no-default-features --features profile-full` passed.
- `scripts/check-cargo-tree-deny.sh sandbox-backend-bwrap` passed.
- `scripts/check-cargo-tree-deny.sh profile-host-bwrap` passed.
- `scripts/check-compiled-capabilities.sh host-bwrap` passed.
- `scripts/check-runtime-env-surface.sh` passed.
- `scripts/check-registry-snapshots.sh host-bwrap` passed.
- `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap bwrap::tests --lib` passed.
- `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap sandbox_backend --lib` passed.
- `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap selected_backend_reports_feature_config_mismatch --lib` passed.
- `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap bwrap_config_errors_are_actionable --lib` passed.
- `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap bwrap_invocation_args_encode_network_root_modes_and_bind_policy --lib` passed.
- `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap bwrap_manifest_validation_rejects_unsafe_values --lib` passed.
- `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap bwrap_lock_timeout_defaults_to_command_timeout_plus_five_and_rejects_zero --lib` passed.
- `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap bwrap_state_and_lock_dir_errors_name_config_keys --lib` passed.
- `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap bwrap_metadata_reports_manifest_path_package_manager_and_sha --lib` passed.
- `cargo test -p oxide-agent-core --no-default-features --features sandbox-backend-bwrap bwrap_root_upper_dir_override_is_per_scope_and_rejects_unsafe_paths --lib` passed.
- `cargo test -p oxide-agent-core --no-default-features --features 'tool-sandbox-exec,tool-sandbox-fileops,tool-sandbox-recreate,sandbox-backend-bwrap' agent::providers::sandbox::tests --lib` passed.
- `cargo test -p oxide-agent-core --no-default-features --features 'sandbox-backend-bwrap,tool-sandbox-exec,tool-sandbox-fileops,tool-sandbox-recreate' bwrap_smoke --lib -- --ignored --list` passed and lists two ignored runtime smoke tests.
- `cargo test -p oxide-agent-core --no-default-features --features 'sandbox-backend-bwrap,tool-stack-logs' stack_logs_report_explicit_unsupported_error_under_bwrap --lib` passed.
- `bash -n scripts/build-bwrap-rootfs-debian.sh && bash -n scripts/smoke-bwrap.sh` passed.
- Hermetic bwrap state lifecycle unit test passed with a fake rootfs/bwrap executable.
- Runtime bwrap smoke passed in this environment with the dev-only `host-smoke-dev` rootfs built by `scripts/build-bwrap-rootfs-host-smoke.sh`.
- Debian 13 rootfs smoke was not executed because this environment has `/usr/bin/bwrap` but does not have `mmdebstrap`, `debootstrap`, or `.oxide/sandbox/images/debian-13-dev/rootfs`. Run `scripts/build-bwrap-rootfs-debian.sh` on a host with `mmdebstrap` or provide a prebuilt Debian rootfs, then run `scripts/smoke-bwrap.sh debian-13-dev`.
