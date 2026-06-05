# Goal: Native Vision Cleanup

Date started: 2026-06-05
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-05-native-vision-cleanup.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: User request after native vision RECON: keep only native vision plus `describe_image_file`, remove tails/over-engineering, and simplify the implementation.
Goal doc owner: Codex
Last updated: 2026-06-05

## Objective

Simplify the image/vision implementation so the only remaining image-understanding paths are native image parts for approved web/OpenCode Go vision routes and the explicit `describe_image_file` tool for sandbox files or URLs.

Done when automatic image pre-description through the preprocessor is removed, Telegram photos are staged as sandbox files instead of raw image preprocessor input, provider image stubs are reduced to a trait default, OpenCode Go native image support policy is consistent across runtime and UI/API labels, and every Completion Audit item is verified by its listed evidence.

## Scope

In scope:
- Legacy image arms in `crates/oxide-agent-core/src/agent/preprocessor.rs`.
- Telegram photo extraction and preprocessing under `crates/oxide-agent-transport-telegram/src/bot/agent/` and `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/`.
- `LlmProvider::analyze_image` default behavior and stub-only provider implementations under `crates/oxide-agent-core/src/llm/providers/`.
- OpenCode Go image-capability helper usage in discovery/runtime/web route labels.
- Focused tests and this goal document.

Out of scope:
- Removing or weakening `describe_image_file`.
- Removing native web attachment refs, transient `MessageContentPart::Image`, or OpenCode Go native serialization.
- Adding Telegram native attachment plumbing in this cleanup pass.
- Broad native multimodal rollout to all providers.
- Removing audio/video media tools or unrelated media workflows.
- New crates, services, queues, storage backends, or broad abstractions.

## Missing Inputs

- None required.

## Repository Context

- Native web image refs enter through `crates/oxide-agent-transport-web/src/server/task_routes.rs:121` and are persisted as safe attachment refs.
- Native image parts are resolved per route in `crates/oxide-agent-core/src/agent/runner/llm_calls.rs:55` and gated at `crates/oxide-agent-core/src/agent/runner/llm_calls.rs:93`.
- OpenCode Go serializes native user image parts in `crates/oxide-agent-core/src/llm/providers/opencode_go.rs:1115`.
- The explicit fallback tool is `describe_image_file` in `crates/oxide-agent-core/src/agent/providers/media_file.rs:166` and `crates/oxide-agent-core/src/agent/providers/media_file.rs:358`.
- The legacy automatic image-description path is `Preprocessor::describe_image` at `crates/oxide-agent-core/src/agent/preprocessor.rs:148` plus `AgentInput::Image` / `AgentInput::ImageWithText` at `crates/oxide-agent-core/src/agent/preprocessor.rs:533` and `crates/oxide-agent-core/src/agent/preprocessor.rs:549`.
- Telegram currently feeds photos into that legacy path from `crates/oxide-agent-transport-telegram/src/bot/agent/media.rs:84` and `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/input.rs:310`.
- Stub-only `analyze_image` implementations exist because the trait currently has no default at `crates/oxide-agent-core/src/llm/provider.rs:40`.
- Existing native vision implementation goal is `docs/goals/2026-06-04-native-multimodal-web-images.md`.

## Completion Audit

- G1: Legacy automatic image preprocessor path is removed
  - Source: User requirement: keep only native vision plus describe vision tool.
  - Acceptance: `Preprocessor::describe_image`, `AgentInput::Image`, `AgentInput::ImageWithText`, their `preprocess_input` arms, and old image-preprocessor tests are removed. Voice, video, and document preprocessing remain intact.
  - Evidence required: diff review plus focused `oxide-agent-core` tests/checks showing preprocessor still handles voice/video/document paths.
  - Status: pending
  - Evidence collected:

- G2: Telegram photos no longer use raw image pre-description
  - Source: RECON finding that Telegram is the active caller of the legacy image preprocessor path.
  - Acceptance: Telegram photo handling stages photos as sandbox document/uploaded-file input with visible sandbox path text; it does not call `AgentInput::Image` or auto-describe images before agent execution.
  - Evidence required: diff review plus `cargo check -p oxide-agent-transport-telegram --no-default-features --features profile-embedded-opencode-local` or documented feature incompatibility with the smallest equivalent check.
  - Status: pending
  - Evidence collected:

- G3: Provider image-analysis stubs are collapsed into a trait default
  - Source: RECON finding that unsupported providers duplicate boilerplate `analyze_image` errors.
  - Acceptance: `LlmProvider::analyze_image` has a default unsupported implementation analogous to `analyze_video`; stub-only implementations in ChatGPT, MiniMax, NVIDIA, and Mistral are removed; real implementations in OpenCode Go, OpenRouter, and ZAI remain.
  - Evidence required: diff review plus focused core checks proving media tool dispatch still compiles.
  - Status: pending
  - Evidence collected:

- G4: OpenCode Go native image support policy is authoritative and consistent
  - Source: RECON finding that discovered `supports_image_input` and runtime route gating can drift.
  - Acceptance: Runtime native-image gate and UI/API image labels use one OpenCode Go app-supported policy, or the field semantics are made explicit as app-supported native image input. No new broad metadata plumbing is added to `ModelInfo`.
  - Evidence required: diff review plus focused tests/checks for OpenCode Go discovery/module behavior and web model-route mapping.
  - Status: pending
  - Evidence collected:

- Q1: Native web vision and `describe_image_file` remain functional
  - Source: User non-negotiable scope: keep native vision plus describe vision tool.
  - Acceptance: Web image attachment refs, transient native image parts, OpenCode Go serialization, and `describe_image_file` registration/handler remain present and tested.
  - Evidence required: `cargo test -p oxide-agent-core --lib --no-default-features --features profile-web-embedded-opencode-local native_image_parts`; `cargo test -p oxide-agent-core --lib --no-default-features --features profile-web-embedded-opencode-local media_file`; OpenCode Go content-part serialization tests/checks.
  - Status: pending
  - Evidence collected:

- Q2: No unrelated media or provider expansion
  - Source: Over-engineering guardrail from `AGENTS.md` and user cleanup request.
  - Acceptance: Audio/video tools are not removed as part of image cleanup; no new providers receive native image serialization; no new dependencies or services are added.
  - Evidence required: `Cargo.toml` diff review and provider diff review.
  - Status: pending
  - Evidence collected:

- N1: No Telegram native attachment plumbing in this pass
  - Source: RECON decision that adding Telegram native attachments is a larger transport API change.
  - Must preserve: Telegram image cleanup is limited to sandbox-file staging and `describe_image_file` fallback.
  - Evidence required: diff review.
  - Status: pending
  - Evidence collected:

## Implementation Plan

1. Add default unsupported `LlmProvider::analyze_image` and remove stub-only provider implementations
   - Audit IDs: G3, Q1, Q2.
   - Expected changes: add a default `analyze_image` method to `crates/oxide-agent-core/src/llm/provider.rs`; delete Mistral image placeholder/delegation and remove ChatGPT/MiniMax/NVIDIA stub overrides; keep OpenCode Go/OpenRouter/ZAI real implementations.
   - Validation: `cargo check -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local`; focused media-file tests if touched by dispatch changes.
   - Exit condition: unsupported providers compile through the trait default and `describe_image_file` still dispatches to real image-capable providers.

2. Remove legacy image preprocessor variants and auto-description
   - Audit IDs: G1, Q1, Q2.
   - Expected changes: remove `Preprocessor::describe_image`, `AgentInput::Image`, `AgentInput::ImageWithText`, image arms in `preprocess_input`, and obsolete image preprocessor tests; keep voice/video/document behavior.
   - Validation: focused preprocessor/core tests and `cargo check -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local`.
   - Exit condition: no code path can auto-describe an inline image through `Preprocessor`; explicit `describe_image_file` remains available.

3. Convert Telegram photos to sandbox-file input
   - Audit IDs: G2, N1, Q1.
   - Expected changes: make Telegram photo extraction always stage photos as document/uploaded-file input equivalent to the current binary-preserving branch; update comments/user-facing errors that mention image pre-description.
   - Validation: `cargo check -p oxide-agent-transport-telegram --no-default-features --features profile-embedded-opencode-local` or documented smallest equivalent if profile features conflict.
   - Exit condition: Telegram photos produce visible sandbox paths and rely on `describe_image_file` instead of preprocessor image description.

4. Align OpenCode Go image-capability policy
   - Audit IDs: G4, Q1, Q2.
   - Expected changes: centralize or clarify the OpenCode Go app-supported native image helper so runtime and UI/API labels do not drift; avoid adding discovered metadata to `ModelInfo`.
   - Validation: focused OpenCode Go discovery/module tests plus web model-route check if mapping changes.
   - Exit condition: a model shown as image-capable by the app is also eligible for native image parts at runtime, and unsupported models are labelled consistently.

5. Final cleanup, docs, and validation
   - Audit IDs: G1-G4, Q1-Q2, N1.
   - Expected changes: update this document and, if needed, close stale status in `docs/goals/2026-06-04-native-multimodal-web-images.md` without changing its original evidence claims.
   - Validation: full command set in the Validation Contract.
   - Exit condition: Completion Audit is fully verified or any remaining blocker has exact command/error evidence and a smallest next action.

## Validation Contract

- Static checks:
  - `cargo fmt --check`
  - `cargo check -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local`
  - `cargo check -p oxide-agent-transport-web --bin oxide-agent-web-console --no-default-features --features profile-web-embedded-opencode-local`
  - `cargo check -p oxide-agent-transport-telegram --no-default-features --features profile-embedded-opencode-local`
- Focused tests:
  - `cargo test -p oxide-agent-core --lib --no-default-features --features profile-web-embedded-opencode-local native_image_parts`
  - `cargo test -p oxide-agent-core --lib --no-default-features --features profile-web-embedded-opencode-local content_parts`
  - `cargo test -p oxide-agent-core --lib --no-default-features --features profile-web-embedded-opencode-local media_file`
  - OpenCode Go serialization/discovery tests affected by capability-policy changes.
- Artifact verification:
  - Diff review confirms no raw image bytes/base64 are introduced into memory/storage.
  - Diff review confirms no new crates/services/providers are added.
- Done when: every Completion Audit item is verified, native web vision and `describe_image_file` remain available, and legacy automatic image pre-description is gone.

## Decisions

- 2026-06-05: Use a new goal doc instead of rewriting `docs/goals/2026-06-04-native-multimodal-web-images.md` because this is a distinct cleanup objective after implementation RECON.
- 2026-06-05: Keep Telegram cleanup simple: stage photos as sandbox files and rely on `describe_image_file`; native Telegram attachments are explicitly out of scope.
- 2026-06-05: Do not remove audio/video media tools as part of image cleanup; they are separate feature-gated workflows.
- 2026-06-05: Prefer a trait default for unsupported image analysis over per-provider unsupported stubs.

## Progress Log

- 2026-06-05: Goal document created from native vision RECON.
  - Changed: Added cleanup objective, audit ledger, checkpoint plan, validation contract, and first-step ordering.
  - Evidence: RECON identified the legacy preprocessor image path, Telegram caller, provider stub boilerplate, and OpenCode Go capability-policy mismatch.
  - Commands: `git status --short && git log -3 --oneline`.
  - Audit IDs updated: none.
  - Next: Checkpoint 1, add default unsupported `LlmProvider::analyze_image` and remove stub-only provider implementations.

## Risks and Blockers

- Telegram feature-profile check may fail for unrelated profile wiring.
  - Impact: G2 validation could need a narrower package/feature command.
  - Evidence: Not observed yet.
  - Mitigation or requested decision: If the planned check fails outside touched code, document the exact error and run the smallest equivalent Telegram/core check.
  - Audit IDs affected: G2.

- Capability label semantics can drift if discovery metadata and runtime policy remain independent.
  - Impact: UI could mark a model image-capable while runtime sends text-only.
  - Evidence: RECON found discovered `supports_image_input` and static runtime policy are separate.
  - Mitigation or requested decision: Keep one OpenCode Go app-supported helper as the source of truth for labels and runtime native-image gating.
  - Audit IDs affected: G4.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
