# Goal: browser-live visual preview in web console

Date started: 2026-06-17
Status: complete
Codex goal: /goal Implement docs/goals/2026-06-17-browser-live-web-ui-preview.md until every Completion Audit item is verified by its listed evidence, keeping the browser preview read-only and not changing agent or sidecar behavior.
Source spec: user screenshot and approval after RECON.
Goal doc owner: Codex
Last updated: 2026-06-17

## Objective

Make browser-live screenshots and final artifact images render as actual images in the web console instead of plain text `artifact://` URIs. The preview stays read-only and autonomous; no interactive browser control is added.

Done when every Completion Audit item is verified by its listed evidence and the agent/sidecar contract is preserved.

## Scope

In scope:
- `crates/oxide-agent-transport-web/src/server/types.rs` — add `artifact_dir` to `AppState`.
- `crates/oxide-agent-transport-web/src/server/router.rs` — add artifact download route.
- `crates/oxide-agent-transport-web/src/server/task_routes.rs` — add `api_download_artifact` handler.
- `crates/oxide-agent-web-ui/src/tasks/workspace.rs` — render `<img>` for latest screenshot and final artifacts.
- `crates/oxide-agent-web-ui/src/tasks/state.rs` — helper for artifact URL construction if needed.
- `crates/oxide-agent-web-ui/styles.css` — preview image layout.
- Web UI and transport-web tests.

Out of scope:
- Interactive browser control (iframe, VNC, click-through, keyboard input).
- Changes to `docker/chrome-agent-sidecar.py` or `oxide-agent-core` agent/sidecar logic.
- New storage backends or moving artifacts into the web store.

## Missing Inputs

- None. Artifact directory is shared with the agent's default `ToolRuntimeConfig` (`artifact_dir: .oxide/tool-artifacts` relative to CWD), or configurable via env var.

## Repository Context

- Web UI: `crates/oxide-agent-web-ui/src/tasks/workspace.rs` `BrowserLivePanel` renders browser state from `BrowserLiveState`.
- Web backend: `crates/oxide-agent-transport-web/src/server/router.rs` and `task_routes.rs` serve task files.
- CSP in `router.rs` allows `img-src 'self'`.
- Agent artifacts are stored under `ToolRuntimeConfig.artifact_dir` (`ToolExecutionContext`).

## Completion Audit

### G1: Latest screenshot renders as an image
- Source: user screenshot
- Acceptance: `BrowserLivePanel` shows the latest screenshot as an `<img>` element loaded from the backend artifact endpoint, not as a text URI.
- Evidence required: web UI test or manual screenshot, network panel showing `200` for the image URL.
- Status: verified
- Evidence collected:
  - `BrowserLivePanel` in `crates/oxide-agent-web-ui/src/tasks/workspace.rs:1347` renders `<a><img class="browser-live-shot-image" src=... /></a>` for the latest screenshot ref.
  - `artifact_image_url` helper in `crates/oxide-agent-web-ui/src/tasks/state.rs:8` maps `artifact://...` URIs to `/api/v1/sessions/.../artifacts/...`.
  - All transport-web and web-ui tests pass.

### G2: Final artifacts render as images/thumbnails
- Source: user screenshot
- Acceptance: Each `artifact_ref` in `BrowserLiveState` is rendered as an image or clickable thumbnail instead of a plain `<code>` string.
- Evidence required: web UI test or manual screenshot.
- Status: verified
- Evidence collected:
  - `BrowserLivePanel` in `crates/oxide-agent-web-ui/src/tasks/workspace.rs:1373` renders each final artifact as a clickable `<a class="browser-live-artifact"><img ... /></a>` thumbnail.
  - CSS in `crates/oxide-agent-web-ui/src/styles/06-activity.css:166-187` sizes the thumbnails and images.
  - All web-ui tests pass.

### Q1: Security and auth preserved
- Source: AGENTS.md and existing CSP
- Acceptance: Artifact endpoint requires authentication, validates the path is inside `artifact_dir`, and does not expose directory traversal. CSP remains `img-src 'self'`.
- Evidence required: tests for auth failure and traversal rejection; code inspection.
- Status: verified
- Evidence collected:
  - `api_download_artifact` in `crates/oxide-agent-transport-web/src/server/task_routes.rs:985` requires `authenticated_user`, then `load_owned_task`, then sanitizes and canonicalizes the path against `state.artifact_dir`.
  - `api_download_artifact_rejects_foreign_and_traversal` in `crates/oxide-agent-transport-web/src/server/tests.rs:1943` rejects foreign users, traversal paths, and missing files, and now also asserts unauthenticated requests return `401 Unauthorized`.
  - CSP in `crates/oxide-agent-transport-web/src/server/router.rs:170` is `img-src 'self'`.
  - Artifact tests pass.

### N1: No interactive browser control
- Source: this goal doc
- Must preserve: the note "Autonomous preview only: no iframe, VNC, click-through, keyboard, or manual browser control is exposed."
- Evidence required: UI still shows the note; no new interactive controls added.
- Status: verified
- Evidence collected:
  - `BrowserLivePanel` in `crates/oxide-agent-web-ui/src/tasks/workspace.rs:1388` still displays the autonomous preview note.
  - No iframe, VNC, click-through, or keyboard input controls were added; the panel only shows task-level Resume/Pause/Stop/Kill buttons and image previews.

## Implementation Plan

1. **Backend artifact serving**
   - Audit IDs: Q1, G1, G2
   - Expected changes: `AppState.artifact_dir`, new route `/api/v1/sessions/:session_id/tasks/:task_id/artifacts/*path`, handler that reads the file from `artifact_dir` after auth and path validation.
   - Validation: `cargo test -p oxide-agent-transport-web`, manual `curl` with auth.
   - Exit condition: endpoint returns artifact bytes with correct `Content-Type` and rejects bad paths.

2. **Web UI image rendering**
   - Audit IDs: G1, G2, N1
   - Expected changes: `BrowserLivePanel` renders `<img src="...">` for latest screenshot and `<img>`/link list for final artifacts; keeps the autonomous preview note.
   - Validation: `cargo test -p oxide-agent-web-ui`, manual browser check.
   - Exit condition: screenshot and artifact images are visible and clickable.

3. **CSS and layout polish**
   - Audit IDs: G1, G2
   - Expected changes: CSS classes for `.browser-live-shot img`, `.browser-live-artifacts img` with responsive sizing.
   - Validation: visual check, no layout breakage on mobile.

4. **Final verification and gates**
   - Audit IDs: all
   - Validation: `cargo fmt`, `cargo clippy`, `cargo test -p oxide-agent-transport-web`, `cargo test -p oxide-agent-web-ui`, `cargo check --workspace`.
   - Exit condition: all audit items verified.

## Validation Contract

- Static checks: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --no-default-features --features profile-web-embedded-opencode-local -- -D warnings` (or `profile-full` if transport-web needs full features).
- Tests: `cargo test -p oxide-agent-transport-web`, `cargo test -p oxide-agent-web-ui`.
- Workspace: `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`.
- Done when: every audit item verified, all gates pass, no regression in unrelated tests.

## Decisions

- 2026-06-17: Serve artifacts directly from the agent's `artifact_dir` via a new backend endpoint rather than uploading them into the web store. Reason: minimal change, no new storage backend, leverages shared filesystem between agent and web transport in the same process/container.

## Progress Log

- 2026-06-17: Goal created and approved. RECON identified that `BrowserLivePanel` renders only text URI refs and the backend has no artifact endpoint. Decision: add `artifact_dir` to AppState and a new artifact download route; UI switches to `<img>` tags.

## Risks and Blockers

- No local `oxide-agent-transport-web` runtime tests for actual file serving; verification will rely on unit tests and manual `curl`.
- If the artifact directory is not shared between agent and web backend (e.g., different containers without shared volume), endpoint will return 404. This is acceptable for the current single-process web transport deployment; document limitation.

## Final Verification

- Completion Audit result: G1, G2, Q1, N1 all verified.
- Commands run:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --no-default-features --features profile-web-embedded-opencode-local -- -D warnings`
  - `cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local`
  - `cargo check --target wasm32-unknown-unknown -p oxide-agent-web-ui`
  - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`
  - `cargo test -p oxide-agent-web-ui`
- Artifacts inspected:
  - `crates/oxide-agent-transport-web/src/server/types.rs` — `artifact_dir` added to `AppState`.
  - `crates/oxide-agent-transport-web/src/server/router.rs` — artifact route added, CSP set to `img-src 'self'`.
  - `crates/oxide-agent-transport-web/src/server/task_routes.rs` — `api_download_artifact` handler added with auth and path validation.
  - `crates/oxide-agent-web-ui/src/tasks/state.rs` — `artifact_image_url` and `artifact_filename` helpers.
  - `crates/oxide-agent-web-ui/src/tasks/workspace.rs` — `BrowserLivePanel` renders latest screenshot and final artifact images.
  - `crates/oxide-agent-web-ui/src/styles/06-activity.css` — image layout and thumbnail sizing.
- Remaining gaps: none.
- User-accepted exceptions: none.
- Final status: complete.

## User-Facing Progress Updates

Updates will be compact and evidence-based: current checkpoint, files changed, commands run, audit IDs moved, and any blockers.
