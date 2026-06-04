# Goal: Native Multimodal Web Image Attachments

Date started: 2026-06-04
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-04-native-multimodal-web-images.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: User request after RECON: native vision for compatible selected models, `describe_image_file` fallback for text-only models, and blast-radius-safe `Message` content-parts design.
Goal doc owner: Codex
Last updated: 2026-06-04 23:44 +0300

## Objective

Add true native image context for web agent chat when the selected model supports image input, while preserving the current sandbox attachment path and `describe_image_file` fallback for text-only models.

Done when image attachments sent through the web transport can reach an approved vision-capable selected model as native provider content parts, text-only routes still receive a text projection plus sandbox path, tool-call history remains valid, and every required Completion Audit item is verified by its listed evidence.

## Scope

In scope:
- `crates/oxide-agent-core/src/llm/types.rs` message content-part metadata, added without removing the current text projection.
- `crates/oxide-agent-core/src/agent/memory.rs` persisted user attachment refs, added with backward-compatible serde defaults.
- Agent executor/runner request assembly under `crates/oxide-agent-core/src/agent/executor/` and `crates/oxide-agent-core/src/agent/runner/`.
- OpenCode Go native image request serialization under `crates/oxide-agent-core/src/llm/providers/opencode_go.rs`.
- Web contracts, routes, runtime task request plumbing, and UI model capability display under `crates/oxide-agent-web-contracts/`, `crates/oxide-agent-transport-web/`, and `crates/oxide-agent-web-ui/`.
- Focused tests and goal-document evidence updates.

Out of scope:
- Removing or weakening `describe_image_file`.
- Storing raw image bytes or base64 data in `AgentMemory` or R2 memory JSON.
- Making all providers multimodal in the first rollout.
- Direct Google Gemini provider work. Gemini-family models remain OpenRouter routes only.
- New crates, queues, storage backends, services, broad framework abstractions, or HA/observability work.

## Missing Inputs

- None required.

## Repository Context

- Canonical LLM `Message` is currently text-only: `crates/oxide-agent-core/src/llm/types.rs:7` with `content: String` at `crates/oxide-agent-core/src/llm/types.rs:11`.
- Persisted `AgentMessage` is also text-only today: `crates/oxide-agent-core/src/agent/memory.rs:31` with `content: String` at `crates/oxide-agent-core/src/agent/memory.rs:38`.
- Memory is checkpointed as JSON through `save_agent_memory_for_flow`: `crates/oxide-agent-transport-web/src/session.rs:903`, `crates/oxide-agent-core/src/storage/r2_memory.rs:68`.
- Token accounting and compaction operate on string projections: `crates/oxide-agent-core/src/agent/compaction/budget.rs:24`, `crates/oxide-agent-core/src/agent/memory.rs:751`, `crates/oxide-agent-core/src/agent/compaction/history.rs:382`.
- Tool result encoding expects string tool outputs and must stay unchanged for this goal: `crates/oxide-agent-core/src/llm/providers/tool_result_encoder.rs:47`.
- Web upload bytes are available only during multipart staging: `crates/oxide-agent-transport-web/src/server/session_routes.rs:115`; after upload, `TaskAttachment` keeps metadata and `sandbox_path` only: `crates/oxide-agent-web-contracts/src/tasks.rs:82`.
- Web task execution currently passes only a `String`: `crates/oxide-agent-transport-web/src/server/task_executor.rs:53`, `crates/oxide-agent-core/src/agent/executor/execution.rs:723`.
- OpenCode Go discovery now has `supports_image_input` on provider-local discovered models: `crates/oxide-agent-core/src/llm/providers/opencode_go/discovery.rs:116`.
- OpenCode Go has a dedicated image analysis body using `image_url` parts: `crates/oxide-agent-core/src/llm/providers/opencode_go.rs:852`; normal chat history still serializes `msg.content` as plain text around `crates/oxide-agent-core/src/llm/providers/opencode_go.rs:1019`.

## Completion Audit

- G1: Web model capability is surfaced to selection UI and API clients
  - Source: User requirement: auto fetch confirmation that selected model supports photo input.
  - Acceptance: OpenCode discovery image capability is carried through `DiscoveredLlmModel` and `ModelRouteView`; web model route response exposes a boolean such as `supports_image_input`; existing clients remain serde-compatible.
  - Evidence required: diff review plus `cargo check -p oxide-agent-web-contracts` and `cargo check -p oxide-agent-transport-web --bin oxide-agent-web-console --no-default-features --features profile-web-embedded-opencode-local`.
  - Status: verified
  - Evidence collected: `supports_image_input` now flows through core discovery metadata, web API DTOs, and the model settings UI. Verified by `cargo check -p oxide-agent-web-contracts`, `cargo test -p oxide-agent-web-contracts`, `cargo check -p oxide-agent-web-ui`, and `cargo check -p oxide-agent-transport-web --bin oxide-agent-web-console --no-default-features --features profile-web-embedded-opencode-local` on 2026-06-04.

- G2: Core message model supports native image context additively
  - Source: Blast-radius decision from RECON.
  - Acceptance: `content: String` remains the canonical text projection; new content-part or attachment-ref metadata is optional, serde-defaulted, and limited to user image context. Old memory JSON deserializes without migration.
  - Evidence required: focused unit tests for old JSON deserialization and text-only message behavior.
  - Status: verified
  - Evidence collected: `Message.content` and `AgentMessage.content` remain string text projections; additive user-only media refs/content parts were added with serde-safe defaults and transient `Message` parts skipped from serialization. Verified by focused core tests on 2026-06-04.

- G3: Executor and runner can carry web image refs without raw-byte persistence
  - Source: Web attachment flow RECON.
  - Acceptance: New/resume task input can include attachment refs; persisted memory stores only safe metadata and sandbox paths; before each provider request, eligible image refs are resolved from the session sandbox into transient provider content parts.
  - Evidence required: unit/integration tests showing refs persist, bytes are not serialized, and missing sandbox files degrade to text-only instead of failing the whole task.
  - Status: in_progress
  - Evidence collected: Checkpoint 3 added `AgentUserInput` for attachment-aware new/resume turns, while preserving existing text-only wrappers. Checkpoint 4 wires web image `TaskAttachment` metadata into those inputs while keeping the text projection with visible sandbox paths. Native sandbox-byte resolution remains pending for checkpoint 5.

- G4: OpenCode Go selected vision models receive native image parts in agent chat
  - Source: User example: MiMo v2.5 supports images.
  - Acceptance: For image-capable OpenCode Go OpenAI Chat Completions routes, user image parts serialize as `content: [{type:text}, {type:image_url,...}]` in normal `chat_with_tools` requests, not only in `analyze_image`.
  - Evidence required: provider request-body unit test covering image-capable route and text-only route.
  - Status: pending
  - Evidence collected:

- G5: Text-only selected models keep the `describe_image_file` fallback path
  - Source: User asked whether the image-description tool remains for blind models.
  - Acceptance: Text-only routes receive the existing text projection with sandbox paths; `describe_image_file` remains registered/usable and still uses configured `MEDIA_MODEL` for image understanding.
  - Evidence required: diff review plus a focused test or fixture proving image refs are stripped/degraded for text-only routes while attachment path text remains.
  - Status: pending
  - Evidence collected:

- G6: Web task/create/resume/version behavior remains compatible
  - Source: Existing web transport architecture.
  - Acceptance: Existing upload endpoint, task DTOs, persisted user-message events, SSE, task lifecycle, and version/edit flows keep working for text-only and non-image attachments.
  - Evidence required: focused web transport checks and existing e2e tests when touched.
  - Status: in_progress
  - Evidence collected: Checkpoint 4 keeps `TaskAttachment` DTOs, upload staging, persisted user-message events, previews, and visible `build_task_execution_input()` path text unchanged; only runtime execution input now carries image refs for core memory.

- Q1: Tool-call history integrity is preserved
  - Source: `AGENTS.md` invariant: preserve history repair and `tool_call_id` integrity before LLM calls.
  - Acceptance: Media parts are never attached to assistant tool-call messages or tool-result messages; `ToolResultEncoder` stays string-based; strict tool history repair still passes.
  - Evidence required: focused diff review and existing tool-history tests for touched areas.
  - Status: pending
  - Evidence collected: Checkpoint 2 helpers attach native parts/refs only to user messages; assistant/tool helper paths remain string-only and `ToolResultEncoder` was not changed. Checkpoint 3 keeps attachment-aware input limited to user task/runtime-context messages.

- Q2: No raw media bloat in memory, compaction, or storage
  - Source: RECON blast-radius and storage constraints.
  - Acceptance: Raw image bytes/base64 are transient only; token accounting and compaction use text projection plus bounded placeholders; R2 memory snapshots do not include raw media payloads.
  - Evidence required: serialization test and diff review.
  - Status: pending
  - Evidence collected: Checkpoint 2 stores only file metadata and sandbox paths in `AgentMessage`; `Message` native parts are skipped by serde; token counting remains based on `content` text projection. Checkpoints 3-4 carry only `AgentMessageAttachment` refs through executor inputs, runtime context, and web task execution.

- N1: No broad provider rollout in the first implementation
  - Source: Over-engineering guardrail.
  - Must preserve: Native provider serialization starts with OpenCode Go; other providers degrade to text-only unless explicitly added in a later checkpoint.
  - Evidence required: diff review.
  - Status: pending
  - Evidence collected:

- N2: No new dependencies or direct Gemini provider
  - Source: `AGENTS.md` architecture invariants.
  - Must preserve: No new crates/services/storage backends; Gemini remains OpenRouter-only.
  - Evidence required: `Cargo.toml` diff review.
  - Status: pending
  - Evidence collected: No `Cargo.toml` changes in checkpoints 1-4.

## Implementation Plan

1. Surface image capability in model-route contracts and UI
   - Audit IDs: G1, N2.
   - Expected changes: add `supports_image_input` to `DiscoveredLlmModel` and `ModelRouteView`; map it in web model routes; show a compact UI badge/status for vision-capable models.
   - Validation: `cargo check -p oxide-agent-web-contracts`; `cargo check -p oxide-agent-transport-web --bin oxide-agent-web-console --no-default-features --features profile-web-embedded-opencode-local`; UI check if UI files change.
   - Exit condition: `/api/v1/model-routes` exposes capability without changing selection semantics.

2. Add additive core media refs and text projection helpers
   - Audit IDs: G2, Q1, Q2, N2.
   - Expected changes: optional user-attachment refs on `AgentMessage`; optional transient content parts on `Message`; helpers for `text_projection()` and provider text-only degradation.
   - Validation: old JSON deserialization test, token-count tests, and focused `oxide-agent-core` tests.
   - Exit condition: existing text-only sessions behave identically and old memory snapshots deserialize.

3. Introduce attachment-aware execution input without breaking old APIs
   - Audit IDs: G3, G5, Q1, Q2.
   - Expected changes: add an internal `AgentUserInput` or equivalent; keep `execute_with_options(&str, ...)` as a wrapper; add attachment-aware new/resume paths.
   - Validation: executor tests for new task/resume with and without attachments.
   - Exit condition: transports can opt into attachments while existing callers keep compiling.

4. Wire web attachments into core user messages
   - Audit IDs: G3, G5, G6, Q2.
   - Expected changes: pass validated web `TaskAttachment` refs into attachment-aware execution; preserve `build_task_execution_input()` text fallback and persisted event display; do not store upload bytes in contracts.
   - Validation: web transport checks and e2e task tests for text-only attachments.
   - Exit condition: web image attachments produce user messages with safe refs and unchanged visible attachment paths.

5. Resolve image refs into native provider parts for supported selected routes
   - Audit IDs: G3, G4, G5, Q1, Q2, N1.
   - Expected changes: before provider request, resolve eligible image refs from the session sandbox into transient bytes only when the active route supports image input; otherwise strip to text projection.
   - Validation: tests for supported route native parts, unsupported route degradation, missing file degradation, and no media on tool messages.
   - Exit condition: native parts are present only for approved user image messages on approved routes.

6. Roll out OpenCode Go native image serialization in agent chat
   - Audit IDs: G4, G5, Q1, N1.
   - Expected changes: update OpenCode Go normal chat builders to serialize user image parts for OpenAI Chat Completions; preserve tool call/result wire format; leave Anthropic/unknown protocols text-only unless explicitly proven safe.
   - Validation: `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go opencode_go --lib`; `cargo clippy -p oxide-agent-core --no-default-features --features llm-opencode-go --lib`.
   - Exit condition: `mimo-v2.5` selected route can receive native image context in `chat_with_tools` request bodies.

7. End-to-end verification and docs update
   - Audit IDs: G1-G6, Q1-Q2, N1-N2.
   - Expected changes: update this document with evidence; add short user-facing docs if runtime configuration or fallback behavior needs explanation.
   - Validation: profile checks, focused tests, and a manual web run if environment is available.
   - Exit condition: Completion Audit is fully verified or remaining blockers are documented with exact evidence.

## Validation Contract

- Static checks:
  - `cargo fmt --check`
  - `cargo check -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local`
  - `cargo check -p oxide-agent-transport-web --bin oxide-agent-web-console --no-default-features --features profile-web-embedded-opencode-local`
  - `cargo check -p oxide-agent-web-contracts`
- Focused tests:
  - `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go opencode_go --lib`
  - Additional focused tests for message serialization, memory serde compatibility, executor input, and web attachment flow as checkpoints add them.
- Runtime/manual verification:
  - With a vision-capable selected OpenCode Go model, upload a small JPEG/PNG in web chat and confirm the provider request includes a native image part.
  - With a text-only selected model, upload the same image and confirm the prompt still exposes the sandbox path and `describe_image_file` can be used.
- Done when: every Completion Audit item is verified, and no out-of-scope constraint was violated.

## Decisions

- 2026-06-04: Use `docs/goals/` because existing durable goal docs live there.
- 2026-06-04: Do not replace `Message.content: String`; keep it as the stable text projection and add optional media metadata. This reduces blast radius for compaction, storage, cache ordering, and tool history.
- 2026-06-04: Store only attachment refs in memory. Raw image bytes are resolved transiently from sandbox paths immediately before provider requests.
- 2026-06-04: Keep `describe_image_file` as the fallback for blind/text-only selected models and for re-analysis later in the task.
- 2026-06-04: First native provider rollout is OpenCode Go only, because MiMo v2.5 support is discovered and the provider already has an image-analysis request body.

## Progress Log

- 2026-06-04 23:05 +0300: Goal document created from RECON.
  - Changed: Added this goal contract and checkpoint plan.
  - Evidence: Existing docs convention found under `docs/goals/`; RECON identified core message, storage, web attachment, and provider blast radius.
  - Commands: `git status --short`; `git log --oneline -5`; diff review of current OpenCode Go vision changes; `git diff --check`; `cargo fmt --check`; `cargo test -p oxide-agent-core --no-default-features --features llm-opencode-go opencode_go --lib` (65 passed); `cargo clippy -p oxide-agent-core --no-default-features --features llm-opencode-go --lib`; `cargo check -p oxide-agent-transport-web --bin oxide-agent-web-console --no-default-features --features profile-web-embedded-opencode-local`.
  - Audit IDs updated: none.
  - Next: Checkpoint 1, surface image capability through web model-route contracts/API/UI.

- 2026-06-04 23:14 +0300: Checkpoint 1 completed.
  - Changed: Added `supports_image_input` to shared discovered model metadata and `ModelRouteView`; mapped it in web model routes; showed image support in the web model selector option label and metadata row.
  - Evidence: G1 verified. Existing route payloads without the new field deserialize with `supports_image_input = false`; no Cargo dependency changes were made.
  - Commands: `cargo fmt`; `cargo check -p oxide-agent-web-contracts`; `cargo test -p oxide-agent-web-contracts`; `cargo check -p oxide-agent-web-ui`; `cargo check -p oxide-agent-transport-web --bin oxide-agent-web-console --no-default-features --features profile-web-embedded-opencode-local`; `cargo fmt --check`; `git diff --check`.
  - Audit IDs updated: G1 verified; N2 preserved for this checkpoint.
  - Next: Checkpoint 2, add additive core media refs and text projection helpers without replacing `Message.content`.

- 2026-06-04 23:26 +0300: Checkpoint 2 completed.
  - Changed: Added user-only `AgentMessageAttachment` refs, transient `MessageContentPart` image parts, text projection helpers, and text-only degradation helpers without replacing string content.
  - Evidence: G2 verified. Attachment refs serialize without raw bytes/base64 fields, old memory JSON without attachment refs deserializes, native parts are skipped from `Message` serialization, and attachment refs do not affect token counts.
  - Commands: `cargo fmt`; `cargo test -p oxide-agent-core --lib --no-default-features --features profile-web-embedded-opencode-local content_parts`; `cargo test -p oxide-agent-core --lib --no-default-features --features profile-web-embedded-opencode-local attachment_refs`; `cargo check -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local`; `cargo check -p oxide-agent-transport-web --bin oxide-agent-web-console --no-default-features --features profile-web-embedded-opencode-local`; `cargo fmt --check`; `git diff --check`.
  - Audit IDs updated: G2 verified; Q1, Q2 evidence added for this checkpoint; N2 preserved.
  - Next: Checkpoint 3, introduce attachment-aware execution input while keeping existing `execute_with_options(&str, ...)` wrappers.

- 2026-06-04 23:36 +0300: Checkpoint 3 completed.
  - Changed: Added `AgentUserInput`, attachment-aware execute/resume paths, runtime-context attachment refs, and focused executor tests for new task and resume flows.
  - Evidence: Existing text APIs remain wrappers; user task/runtime-context entries persist only safe attachment refs; no provider-native raw bytes are introduced in memory.
  - Commands: `cargo fmt`; `cargo test -p oxide-agent-core --lib --no-default-features --features profile-web-embedded-opencode-local agent::executor::tests::resume`; `cargo check -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local`; `cargo check -p oxide-agent-runtime`; `cargo check -p oxide-agent-transport-web --bin oxide-agent-web-console --no-default-features --features profile-web-embedded-opencode-local`; `cargo fmt --check`; `git diff --check`.
  - Audit IDs updated: G3 in progress; Q1, Q2 evidence extended; G5 fallback path preserved structurally.
  - Next: Checkpoint 4, wire validated web `TaskAttachment` refs into the attachment-aware core execution input while keeping visible sandbox path text.

- 2026-06-04 23:44 +0300: Checkpoint 4 completed.
  - Changed: Web task create, resume, and version flows now build `AgentUserInput` from validated attachments; image attachments become safe core refs, while all attachments remain visible in the existing text projection and persisted web events.
  - Evidence: The new web helper maps only `image/*` attachments to `AgentMessageAttachment::Image`, keeps non-image attachments text-only, and preserves sandbox path text for fallback/tool use.
  - Commands: `cargo fmt`; `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local build_task_agent_user_input_preserves_text_and_maps_image_refs`; `cargo check -p oxide-agent-transport-web --bin oxide-agent-web-console --no-default-features --features profile-web-embedded-opencode-local`; `cargo check -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local`; `cargo fmt --check`; `git diff --check`.
  - Audit IDs updated: G3 evidence extended; G6 in progress; Q2 evidence extended; G5 fallback text path preserved.
  - Next: Checkpoint 5, resolve image refs from the session sandbox into transient provider content parts only for image-capable selected routes, with text-only and missing-file degradation.

## Risks and Blockers

- Persisted memory compatibility can break if `AgentMessage` is changed without serde defaults.
  - Impact: Existing R2 sessions fail to load.
  - Evidence: `AgentMemory` serializes/deserializes the full message vector as JSON.
  - Mitigation: Add optional fields with `#[serde(default)]`; test old JSON.
  - Audit IDs affected: G2, Q2.

- Tool-call history can break if media parts are attached to assistant/tool messages.
  - Impact: Strict providers reject requests or history repair drops valid tool results.
  - Evidence: `ToolResultEncoder` and history repair are string/tool-id based.
  - Mitigation: Restrict media parts to user messages; leave tool result content string-only.
  - Audit IDs affected: Q1, G4.

- Sandbox file refs may be stale after sandbox recreation.
  - Impact: Native image resolution can fail on later turns.
  - Evidence: Web uploads are sandbox-local and current prompt warns they are lost if sandbox is destroyed.
  - Mitigation: Degrade to text-only path with the existing sandbox warning; do not fail the whole task.
  - Audit IDs affected: G3, G5.

- Provider capability may differ from discovery fallback for future OpenCode models.
  - Impact: Image requests can fail at runtime for misreported models.
  - Evidence: Current discovery supports explicit modalities when provided and fallback only for known MiMo IDs.
  - Mitigation: Keep fallback narrow; on provider error, report clear failure and preserve text/tool fallback.
  - Audit IDs affected: G1, G4, G5.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
