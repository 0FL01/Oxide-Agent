# Goal: Browser Live — direct control by the main agent

Date started: 2026-06-17
Status: active
Codex goal: not set
Source spec: user request to rewrite the v3 OTS browser-live plan to give the vision-enabled main agent direct control over browser tools instead of routing through the MiMo decision layer
Goal doc owner: Codex
Last updated: 2026-06-17 23:59

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
- Status: verified
- Evidence collected: `ToolOutput` now carries `image_attachment: Option<ToolOutputImageAttachment>`; `runner/tools.rs` maps it to `AgentMessageAttachment` and stores it in memory; `llm_calls.rs` attaches native image content parts for both user and tool messages; `chat_completions` and `messages` (Anthropic) providers serialize image content parts inside tool-result messages. Unit tests: `tool_output_image_attachment_is_carried_without_bytes`, `typed_runtime_tool_output_image_attachment_is_recorded_in_memory`, `native_image_parts_resolve_for_tool_messages`, `chat_completions_generic_tool_request_includes_image_content_parts`, `prepare_messages_includes_image_blocks_in_tool_results`.

### G2: `browser_observe` returns compact state + screenshot
- Source: direct-control design.
- Acceptance: `browser_observe` returns URL, title, loading state, network/console summaries, and an image attachment; the main agent sees the screenshot.
- Evidence required: live REST test + `cargo test` pass.
- Status: verified
- Evidence collected: `crates/oxide-agent-core/src/agent/providers/browser_live/tools.rs` now returns `ObserveToolResult { payload, image_attachment }` from `BrowserLiveProvider::observe`; the executor attaches the image to `ToolOutput`. The `browser_observe` tool description was updated to say the latest screenshot is attached as a native image for vision models. Unit tests assert the image attachment exists, has a valid MIME type, non-zero size, and the referenced sandbox path exists; another test asserts redacted/empty screenshots are skipped. Validation: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live` 90 pass; live REST test on `http://127.0.0.1:8787` created a session for `https://example.com` and `/sessions/{id}/observe` returned URL/title/loading state; `/sessions/{id}/screenshot/latest?format=binary` returned a 14,846-byte PNG.

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
- Status: verified (for CP-1 and CP-2 scope)
- Evidence collected: `cargo fmt --all -- --check` passes; `cargo clippy -p oxide-agent-core -p oxide-agent-web-contracts -p oxide-agent-web-ui --no-default-features --features profile-full --all-targets -- -D warnings` passes.

### Q2: Tests remain green
- Source: `AGENTS.md` testing guidance.
- Acceptance: `cargo test -p oxide-agent-core` and sidecar self-test pass.
- Evidence required: command output.
- Status: verified (for CP-1 and CP-2 scope)
- Evidence collected: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live` (90 pass); `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib` (1330 pass, 8 ignored, 0 failed); `cargo test -p oxide-agent-web-ui` (11 pass); `cargo test -p oxide-agent-web-contracts` (10 pass); `python3 -m py_compile docker/chrome-agent-sidecar.py` and `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test` pass. Full CP-1 evidence still applies for the tool-runtime tests.

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

- 2026-06-17: CP-1 — image attachments in tool runtime.
  - Changed: `crates/oxide-agent-core/src/agent/tool_runtime/output.rs` added `ToolOutputImageAttachment` and `ToolOutput.image_attachment`; `crates/oxide-agent-core/src/agent/tool_runtime/mod.rs` re-exported the new type; `crates/oxide-agent-core/src/agent/runner/tools.rs` maps the attachment to `AgentMessageAttachment` in memory; `crates/oxide-agent-core/src/agent/memory.rs` added `native_image_attachments()` for user/tool messages; `crates/oxide-agent-core/src/agent/runner/llm_calls.rs` attaches native image content parts for tool messages; `crates/oxide-agent-core/src/llm/providers/chat_completions/request.rs` and `messages/request.rs` serialize image content parts inside tool-result messages; `crates/oxide-agent-core/src/llm/providers/opencode_go.rs` test updated to assert new tool-result image behavior.
  - Evidence: `cargo fmt`, `cargo clippy` pass; targeted tests pass; full core test run has 1328 pass with one unrelated/flaky `wiki_memory` test that passes in isolation.
  - Commands: `cargo fmt --all`, `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --all-targets -- -D warnings`, targeted `cargo test -p oxide-agent-core ...` commands, full `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib`.
  - Audit IDs updated: G1 pending → verified, Q1 pending → verified, Q2 pending → verified.
  - Next: CP-2 — `browser_observe` returns compact state + screenshot.

- 2026-06-17: CP-2 — `browser_observe` returns compact state + screenshot.
  - Changed: `crates/oxide-agent-core/src/agent/providers/browser_live/tools.rs` added `ObserveToolResult` and `screenshot_image_attachment`; `BrowserLiveProvider::observe` returns payload + image attachment; `BrowserLiveToolExecutor::execute` attaches the image to `ToolOutput`; `browser_observe` tool description updated to mention the native image attachment.
  - Evidence: `cargo fmt`, `cargo clippy` pass; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live` 90 pass; full core test run 1330 pass, 8 ignored, 0 failed; `cargo test -p oxide-agent-web-ui` 11 pass; `cargo test -p oxide-agent-web-contracts` 10 pass; sidecar self-test passes; live REST observe on `http://127.0.0.1:8787` returned URL/title/loading state and a 14,846-byte PNG screenshot.
  - Commands: `cargo fmt --all`, `cargo clippy -p oxide-agent-core -p oxide-agent-web-contracts -p oxide-agent-web-ui --no-default-features --features profile-full --all-targets -- -D warnings`, `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live`, `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib`, `cargo test -p oxide-agent-web-ui`, `cargo test -p oxide-agent-web-contracts`, `python3 -m py_compile docker/chrome-agent-sidecar.py`, `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test`, live REST observe.
  - Audit IDs updated: G2 pending → verified, Q1 verified (extended), Q2 verified (extended).
  - Next: CP-3 — `browser_execute` replaces `browser_step`.

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

* Current checkpoint: CP-2 complete.
* What changed: `browser_observe` now returns a compact state payload plus a screenshot image attachment. The provider attaches the latest persisted screenshot to the `ToolOutput` so the vision-enabled main agent sees the page without an extra `browser_screenshot` call.
* What was verified: 90 browser_live tests pass, full core 1330 pass, web-ui 11 pass, web-contracts 10 pass, sidecar self-test passes, and a live REST observe on a real page returned a 14,846-byte PNG screenshot.
* Which audit IDs moved: G2 pending → verified, Q1 verified (extended), Q2 verified (extended).
* What remains: CP-3 through CP-7.
* Whether anything is blocked: not blocked.

## Quality Bar

The goal document is good only if another Codex session can resume from it without relying on hidden conversation memory.
