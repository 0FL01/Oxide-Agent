# Goal: Browser Live — direct control by the main agent

Date started: 2026-06-17
Status: active
Codex goal: not set
Source spec: user request to rewrite the v3 OTS browser-live plan to give the vision-enabled main agent direct control over browser tools instead of routing through the MiMo decision layer
Goal doc owner: Codex
Last updated: 2026-06-17 23:15

## Objective

Replace the `browser_live` MiMo decision loop with a thin, stateful driver. The vision-enabled main agent receives a screenshot from `browser_observe` and directly calls `browser_execute` with concrete `BrowserAction`s. Add `browser_extract` for deterministic network-data extraction. Remove obsolete MiMo, decision, and prompt-layer code.

Done when every required Completion Audit item is verified by its listed evidence and the OTS full-flow E2E passes through the main agent without the legacy `browser_step`.

## Scope

In scope:
- `crates/oxide-agent-core/src/agent/tool_runtime/` — add image attachment support to `ToolOutput`.
- `crates/oxide-agent-core/src/agent/runner/` — include tool-returned images in the main agent message context.
- `crates/oxide-agent-core/src/agent/providers/browser_live/` — rewrite `tools.rs`, `types.rs`, `actions.rs`, `session.rs`, `prompt.rs`, `policy.rs`, `verification.rs`, `parser.rs`, `test_support.rs`.
- `docker/chrome-agent-sidecar.py` — add `/extract` endpoint or equivalent for network-body extraction.
- Removal of MiMo/Decision modules (`mimo.rs`, `decision.rs`, `model_routes.rs` browser-live-specific parts if any) and `BrowserDecision*` types.

Out of scope:
- Changes to other providers (LLM, SSH, sandbox, etc.).
- Re-architecture of the sidecar CDP protocol itself; the sidecar stays the Chrome bridge.
- Support for non-vision main-agent models. Direct-control mode is gated behind a vision-capable model.
- Web UI or Telegram transport handlers beyond what naturally falls out of `NetworkItem` changes.
- New deployment manifests, Compose changes, or infrastructure wiring.

## Missing Inputs

- None. The direct-control approach is chosen by the user.

## Repository Context

- Relevant entry points:
  - `crates/oxide-agent-core/src/agent/providers/browser_live/` owns the whole provider.
  - `crates/oxide-agent-core/src/agent/tool_runtime/` owns tool registration and execution.
  - `crates/oxide-agent-core/src/agent/runner/` owns message assembly and the main agent loop.
  - `docker/chrome-agent-sidecar.py` is the sidecar REST/CDP driver.
- Existing conventions:
  - Capability-module feature gating for `browser_live`.
  - `serde` types in `types.rs`, tool handlers in `tools.rs`, action mapping in `actions.rs`.
  - `tool_runtime` uses `ToolOutput` as text-only result.
  - Sidecar endpoints are `/sessions/{id}/...` and return JSON.
- Dependencies or runtime assumptions:
  - Main agent must be a vision model (e.g. Claude 4/GPT-4o/Gemini).
  - Sidecar is built from `docker/chrome-agent-sidecar.py` and running.
- Validation infrastructure:
  - `cargo fmt`, `cargo clippy`.
  - `cargo test -p oxide-agent-core`.
  - `python3 -m py_compile docker/chrome-agent-sidecar.py`.
  - `chrome-agent-sidecar --self-test`.
  - Live REST E2E against `https://ots.bash.md/`.

## Completion Audit

### G1: Tool runtime returns image attachments
- Source: user request and direct-control design.
- Acceptance: a tool can attach a screenshot image to its result, and the runner can include it in the next main-agent turn.
- Evidence required: unit test + `cargo test` pass.
- Status: pending
- Evidence collected:

### G2: `browser_observe` returns compact state + screenshot
- Source: direct-control design.
- Acceptance: `browser_observe` returns URL, title, loading state, network/console summaries, and an image attachment; the main agent sees the screenshot.
- Evidence required: live REST test + `cargo test` pass.
- Status: pending
- Evidence collected:

### G3: `browser_execute` replaces `browser_step`
- Source: direct-control design.
- Acceptance: `browser_execute` accepts a single `BrowserAction` and returns a concrete result/observation; `browser_step` is removed; no MiMo decision layer remains.
- Evidence required: unit tests + `cargo test` pass.
- Status: pending
- Evidence collected:

### G4: `browser_extract` reads network response bodies
- Source: v3 OTS requirement (extract `secret_id` from `POST /api/create`).
- Acceptance: `browser_extract` can pull a field from a captured network response by URL pattern, HTTP method, and JSON pointer.
- Evidence required: live REST test + `cargo test` pass.
- Status: pending
- Evidence collected:

### G5: Main-agent prompt/tool schema updated for direct control
- Source: direct-control design.
- Acceptance: main-agent system prompt or tool descriptions expose the available `BrowserAction` kinds and the intended workflow (observe → execute → extract).
- Evidence required: prompt diff + E2E behavior.
- Status: pending
- Evidence collected:

### G6: Policy and safety preserved at tool level
- Source: architectural invariants in `AGENTS.md`.
- Acceptance: `BrowserAction` is validated before execution; sensitive actions are gated; recovery is still possible (thin verification layer) without the MiMo loop.
- Evidence required: unit tests + `cargo test` pass.
- Status: pending
- Evidence collected:

### G7: Full OTS E2E via the main agent succeeds
- Source: user request and v3 test report.
- Acceptance: the main agent can create a secret, extract the share link, open it, reveal it, and recover the original text without calling `browser_step`.
- Evidence required: live REST E2E transcript.
- Status: pending
- Evidence collected:

### Q1: Code quality and static checks
- Source: `AGENTS.md` development practices.
- Acceptance: `cargo fmt`, `cargo clippy` pass; no new warnings; dead code removed.
- Evidence required: command output.
- Status: pending
- Evidence collected:

### Q2: Tests remain green
- Source: `AGENTS.md` testing guidance.
- Acceptance: `cargo test -p oxide-agent-core` and sidecar self-test pass.
- Evidence required: command output.
- Status: pending
- Evidence collected:

### N1: Non-vision models are not supported in direct-control mode
- Source: direct-control design.
- Must preserve: the code may fail gracefully or require a vision model; no fallback MiMo logic is retained.
- Evidence required: code review.
- Status: pending
- Evidence collected:

### N2: No changes to other providers
- Source: scope.
- Must preserve: LLM, SSH, sandbox, reminders, etc. remain untouched.
- Evidence required: `git diff` shows only browser_live/tool_runtime/runner changes.
- Status: pending
- Evidence collected:

## Implementation Plan

### CP-1: Image attachments in tool runtime
- Audit IDs: G1, Q1, Q2.
- Expected changes:
  - Extend `ToolOutput` in `crates/oxide-agent-core/src/agent/tool_runtime/` with an image/bytes variant.
  - Update runner message assembly to include image attachments in the main-agent context.
  - Add unit tests for image attachment serialization.
- Validation:
  - `cargo test -p oxide-agent-core --lib -- agent::tool_runtime`
  - `cargo fmt`, `cargo clippy`
- Exit condition: a tool can return an image and the runner can forward it.

### CP-2: `browser_observe` returns compact state + screenshot
- Audit IDs: G2, Q1, Q2.
- Expected changes:
  - Remove `browser_step` from the public tool set temporarily (or keep it but mark deprecated).
  - Refactor `browser_observe` to build a compact observation and attach the latest screenshot.
  - Update `BrowserObservation` payload and `session.rs` screenshot storage.
- Validation:
  - `cargo test -p oxide-agent-core --lib -- agent::providers::browser_live`
  - Live REST test: call `/sessions/{id}/observe` and confirm the screenshot is present.
- Exit condition: REST observe returns correct URL + screenshot attachment.

### CP-3: `browser_execute` replaces `browser_step`
- Audit IDs: G3, Q1, Q2, G6.
- Expected changes:
  - Add `BrowserExecuteRequest`/`Response` in `types.rs`.
  - Add `browser_execute` tool handler.
  - Remove `BrowserDecision`, `BrowserDecisionAction`, `BrowserMimoDecider`, and related prompt/parser logic.
  - Keep `BrowserAction` mapping in `actions.rs`.
  - Update `test_support.rs` and all unit tests.
- Validation:
  - `cargo test -p oxide-agent-core --lib -- agent::providers::browser_live`
  - Live REST test: fill input and click button through `browser_execute`.
- Exit condition: `browser_execute` works and `browser_step` is gone.

### CP-4: `browser_extract` for network bodies
- Audit IDs: G4, Q1, Q2.
- Expected changes:
  - Add `BrowserExtractRequest`/`Response` in `types.rs`.
  - Add `browser_extract` tool handler.
  - Add sidecar endpoint `/sessions/{id}/extract` or reuse `/debug/network` with query parameters.
- Validation:
  - Live REST test: extract `secret_id` from `POST /api/create`.
  - `cargo test` pass.
- Exit condition: `browser_extract` returns the expected field.

### CP-5: Main-agent prompt/tool schema
- Audit IDs: G5, G7.
- Expected changes:
  - Update main-agent system prompt or tool descriptions to expose `BrowserAction` kinds.
  - Document the intended workflow: observe → execute → extract.
  - Remove the legacy browser-live MiMo prompt from `prompt.rs`.
- Validation:
  - Review tool definitions in `capabilities --enabled --json`.
  - Live REST E2E: main agent can complete the OTS flow.
- Exit condition: prompt/tool descriptions align with the new API.

### CP-6: Policy and verification
- Audit IDs: G6, Q1, Q2.
- Expected changes:
  - Move `BrowserDecision` policy checks to `BrowserAction` validation.
  - Keep a thin `verification.rs` that returns success/failure/needs_retry to the main agent.
  - Ensure sensitive-action gating still works.
- Validation:
  - `cargo test` + live REST test of a disallowed action.
- Exit condition: policy still protects without MiMo.

### CP-7: Full OTS E2E
- Audit IDs: G7, Q1, Q2, N2.
- Expected changes:
  - Remove or update the temporary E2E script.
  - Run the full flow via main-agent calls.
- Validation:
  - Live REST E2E against `https://ots.bash.md/`.
  - `git diff` review confirms only targeted files changed.
- Exit condition: recovered secret matches injected secret.

## Validation Contract

- Static checks:
  - `cargo fmt --all -- --check`
  - `cargo clippy -p oxide-agent-core -p oxide-agent-web-contracts -p oxide-agent-web-ui --no-default-features --features profile-full --all-targets -- -D warnings`
  - `python3 -m py_compile docker/chrome-agent-sidecar.py`
- Tests:
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live`
  - `cargo test -p oxide-agent-web-ui`
  - `cargo test -p oxide-agent-web-contracts`
  - `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test`
- Runtime/manual verification:
  - Live REST E2E: create secret → extract share link → open → reveal → recover original text.
- Artifact verification:
  - `git diff` scope limited to browser_live/tool_runtime/runner + sidecar.
- Done when: every Completion Audit item is verified and E2E succeeds.

## Decisions

- 2026-06-17: Direct main-agent control chosen over MiMo fix. Reason: user explicitly requested it; it removes the intermediary class of problems and simplifies the control contract.
- 2026-06-17: Non-vision models are out of scope for direct-control mode. Reason: the whole point is that the main agent sees the screenshot.
- 2026-06-17: Sidecar remains the Chrome bridge. Reason: only the decision layer moves, not the browser automation backend.

## Progress Log

- 2026-06-17: Goal created and plan approved.
  - Changed: `docs/goals/2026-06-17-browser-live-direct-control.md` created.
  - Evidence: user approved direct-control plan.
  - Commands: none yet.
  - Audit IDs updated: all pending.
  - Next: CP-1 — image attachments in tool runtime.

## Risks and Blockers

- Main agent must be a vision model.
  - Impact: direct-control mode fails or is unavailable with text-only models.
  - Mitigation: gating in config/tool descriptions; no MiMo fallback kept.
- Image attachments may bloat the main-agent context and increase token cost.
  - Impact: higher latency/cost.
  - Mitigation: `browser_observe` returns image only when needed; retained-artifact deduplication stays.
- Recovery logic moves to the main agent.
  - Impact: model may loop or miss a failed action.
  - Mitigation: thin verification layer returns explicit `needs_retry`/`success`/`failed`; tool-level safety stop.
- `BrowserAction` schema may need to be more expressive without MiMo.
  - Impact: more types to maintain.
  - Mitigation: keep schema minimal; add actions only when the main agent needs them.
- Dead-code removal may break existing consumers or tests.
  - Impact: compile failures outside browser_live.
  - Mitigation: check `git grep` for all references to `BrowserDecision`, `browser_step`, `MiMo`, etc. across the repo.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:

## User-Facing Progress Updates

* Current checkpoint: CP-1 pending.
* What changed: goal document created; direct-control plan approved.
* What was verified: plan reviewed against repository conventions.
* Which audit IDs moved: none yet.
* What remains: CP-1 through CP-7.
* Whether anything is blocked: not blocked.

## Quality Bar

The goal document is good only if another Codex session can resume from it without relying on hidden conversation memory.
