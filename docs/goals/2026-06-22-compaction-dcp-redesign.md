# Goal: DCP-style unified compaction redesign

Date started: 2026-06-22
Status: active
Codex goal: Implement `docs/goals/2026-06-22-compaction-dcp-redesign.md` until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals.
Source spec: User request to replace the current compaction system with a unified DCP-inspired design; RECON over current Oxide compaction and `.donor/opencode-dynamic-context-pruning`.
Goal doc owner: Codex
Last updated: 2026-06-22 00:30

## Objective

Replace Oxide Agent's current replacement-based compaction with one unified compaction architecture inspired by DCP: raw transcript is preserved, compaction state is tracked explicitly, and every manual/automatic/context-limit/model-downshift path renders the same compacted model context through one engine.

Done when every required Completion Audit item is verified by its listed evidence, the old replacement pipeline is removed completely with no compatibility shim or tail path, and all compaction triggers use the unified engine.

## Scope

In scope:
- `crates/oxide-agent-core/src/agent/compaction/` — redesign around state, refs, selection, engine, rendering, strategy, policy, and events.
- `crates/oxide-agent-core/src/agent/memory.rs` — persist raw transcript plus `CompactionState`; preserve backward-compatible deserialization.
- `crates/oxide-agent-core/src/agent/runner/` — use rendered compacted context before LLM calls; route all compaction triggers through one engine.
- `crates/oxide-agent-core/src/agent/providers/compression.rs` — replace no-arg scheduling tool with structured range/message compression.
- `crates/oxide-agent-core/src/storage/` — only if atomic persistence of compaction state requires storage-facade changes.
- Transport progress/event mappings in Telegram/Web only where event schema semantics must change.
- Runtime context/tool-output admission gates that prevent oversized input, document, file-read, or tool-output payloads from entering hot memory inline.
- An explicit trigger matrix defining when compaction/admission runs and which component has authority for each trigger.
- Tests and docs covering the new contract.

Out of scope:
- Direct TypeScript/AGPL code import from DCP.
- DCP TUI, OpenCode plugin SDK, file-based persistence, host-permission model, auto-update, or toast/command subsystem.
- New crates, services, queues, caches, storage backends, or model providers unless a verified contract makes them unavoidable.
- Direct Gemini provider; Gemini remains OpenRouter-routed per repo invariant.
- Wiki memory redesign, browser-live redesign, sandbox redesign, or transport UI redesign beyond compaction event compatibility.

## Missing Inputs

- User review of this goal plan.
  - Impact: implementation should not start until the phase/checkpoint structure is accepted or edited.
  - Low-risk assumption: DCP is a conceptual donor only; Oxide will reimplement the model in Rust with stricter structured contracts.
  - User/external action needed: approve, reject, or edit this goal document.

## Repository Context

- Current compaction entry points:
  - `crates/oxide-agent-core/src/agent/compaction/controller.rs` — `CompactionController` side-LLM summary + memory replacement.
  - `crates/oxide-agent-core/src/agent/compaction/history.rs` — `build_compacted_history` deterministic replacement builder.
  - `crates/oxide-agent-core/src/agent/runner/runtime_compaction.rs` — runtime orchestration for pre-sampling, context-limit, model-downshift, manual checkpoints.
  - `crates/oxide-agent-core/src/agent/executor/compaction.rs` — transport-triggered manual compaction path with duplicated event emission.
  - `crates/oxide-agent-core/src/agent/providers/compression.rs` — current `compress` tool is a no-arg scheduler.
- Current defects found during RECON:
  - Runtime compaction mutates/replaces `AgentMemory` instead of rendering an overlay.
  - Manual/runtime event emission is duplicated.
  - `run_iteration_compaction` is a no-op stub.
  - `RuntimeCompactionSkipped` is defined/mapped but never emitted.
  - `CompactionPolicy::default()` is scattered and not config-backed.
  - Archive/externalization types exist but are not wired into compaction.
  - `build_replacement` is called twice in `controller.rs` for one compaction.
- DCP donor concepts to reimplement conceptually:
  - `CompressionBlock` graph with active/consumed/parent/effective message/tool ids.
  - Stable visible refs (`mNNNN`, `bN`).
  - In-flight compacted rendering instead of destructive transcript mutation.
  - Range/message compression selection.
  - Deduplication and purge-error strategies.
  - Context-limit, turn, and iteration nudges.
- Existing validation infrastructure:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - `cargo build --release --no-default-features --features profile-full` for final full binary confidence when practical.
  - If web UI changes: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`.

## Contract Boundary Analysis

### LLM -> `compress` tool

- Sender: model-visible assistant.
- Receiver: Oxide compression tool/engine.
- Old unreliable contract: LLM calls `compress` with no selection and runner later decides whether/how to compact.
- New reliable contract: LLM may reference only renderer-injected visible refs (`mNNNN`, `bN`) plus its own structured summary content. It must not provide internal message UUIDs, tool ids, storage keys, or downstream state guesses.
- Receiver-owned responsibilities: resolve refs to raw messages/blocks, validate range order/non-overlap, validate block references, preserve tool-call pairing, and atomically update `CompactionState`.

### Runner/hooks/transport -> compaction engine

- Sender: runtime trigger source (`manual`, `pre_sampling`, `context_limit`, `model_downshift`, hook/tool request).
- Receiver: `CompactionEngine`.
- New reliable contract: sender supplies intent, reason, route, scope, and current memory snapshot. It does not choose raw messages to delete.
- Receiver-owned responsibilities: choose or validate selection, create/update blocks, render compacted context, emit normalized events.

### Compaction engine -> memory/storage

- Sender: `CompactionEngine`.
- Receiver: `AgentMemory` and persistence/checkpoint layer.
- New reliable contract: engine commits one atomic state transition containing raw transcript preservation plus compaction overlay updates.
- Receiver-owned responsibilities: serialize/deserialize backward-compatibly, checkpoint the raw transcript and compaction state together, reject invalid tool history before model calls.

### Ingress/tool output -> hot memory

- Sender: transport/preprocessor/runtime inbox/tool runtime.
- Receiver: `ContextAdmission` before `AgentMemory` mutation.
- Old unreliable contract: upstream payloads are appended directly, and later compaction/provider overflow handling may try to recover.
- New reliable contract: upstream sends payload bytes/text plus metadata (`new_task`, `runtime_context`, `document`, `tool_output`, source path/tool name, attachments/artifact refs if any). It does not need to know route budget, prompt/tool-schema overhead, or provider tokenization.
- Receiver-owned responsibilities: decide `inline`, `archive_manifest`, `archive_plus_chunk_summary`, or `controlled_pause/reject` before hot-memory mutation. Oversized raw payloads must never become inline hot-memory messages.

### Runner -> LLM provider under emergency pressure

- Sender: runner/render pipeline.
- Receiver: LLM provider.
- New reliable contract: runner sends only a pre-budgeted `RenderedModelContext`; `rendered_tokens + hard_reserve <= route_context_window` is a hard preflight gate.
- Receiver-owned responsibilities: provider may still return a typed context-limit error for tokenizer drift, but this is a last-resort fallback, not the primary overflow detector.

## Compaction Trigger Matrix

- Context admission trigger:
  - Initiator: `ContextAdmission`.
  - Condition: new user task, runtime context, document/preprocessor output, file read, command output, or any tool output is about to enter hot memory.
  - Action: inline only if the rendered budget can safely contain it; otherwise archive/reference raw content, insert a bounded manifest, optionally create bounded chunk summaries, or controlled-pause/reject if safe continuation is impossible.

- Pre-LLM budget trigger:
  - Initiator: runner render/budget gate.
  - Condition: before every provider call, `system + tools + wiki/context + rendered memory + reserves` approaches or exceeds the active route window.
  - Action: create/update compaction blocks through `CompactionEngine`; sending an oversized request is forbidden.

- Agent-requested trigger:
  - Initiator: LLM via structured `compress` tool.
  - Condition: agent asks to compress visible refs such as `mNNNN`/`bN`.
  - Action: engine validates refs, boundaries, summary parts, and tool-call safety; it creates a block or rejects without mutation.

- User/manual trigger:
  - Initiator: user/transport manual compaction request.
  - Condition: explicit manual compact/checkpoint action.
  - Action: same engine path as every other trigger; no separate manual replacement pipeline.

- Model-downshift trigger:
  - Initiator: model routing/failover.
  - Condition: next route has a smaller context window than the current rendered context can fit.
  - Action: force compaction to the next route budget before calling the smaller model.

- Typed provider-overflow trigger:
  - Initiator: provider typed context-limit error.
  - Condition: provider rejects a request despite local preflight, due to tokenizer drift or provider-side accounting.
  - Action: emergency shrink through the same engine, bounded retry, then controlled stop if overflow repeats.

## Completion Audit

### Functional requirements (G*)

- G1: Raw transcript is preserved and compaction becomes an overlay/render concern.
  - Source: User request “текущую систему выкинуть, нужна единая логика compaction”; RECON finding that current compaction replaces `AgentMemory`.
  - Acceptance: compaction does not destructively remove source messages from persisted raw memory; model-facing history is produced by a renderer from raw messages + compaction state.
  - Evidence required: tests showing raw messages remain after compaction while rendered context omits compacted ranges; storage round-trip preserves both raw messages and compaction state.
  - Status: pending
  - Evidence collected:

- G2: One `CompactionEngine` is the only runtime mutation authority.
  - Source: RECON finding multiple compaction entry paths and duplicated event emitters.
  - Acceptance: manual transport, `compress` tool, pre-sampling/pre-LLM budget checks, context-limit retry, model-downshift, and context-admission emergency paths all call the same engine API for compaction state changes.
  - Evidence required: grep/call-site audit plus integration tests for each trigger.
  - Status: pending
  - Evidence collected:

- G3: DCP-style block graph is implemented with nesting/consumption semantics.
  - Source: DCP `CompressionBlock` and `PruneMessagesState` model.
  - Acceptance: active blocks, consumed blocks, parent blocks, direct/effective message refs, and direct/effective tool ids are tracked deterministically; recompression consumes prior blocks without duplicate model-visible summaries.
  - Evidence required: unit tests for block creation, nested block consumption, deactivation, reactivation/sync if applicable, and effective id propagation.
  - Status: pending
  - Evidence collected:

- G4: Stable visible refs are renderer-owned and tool-resolved.
  - Source: DCP `mNNNN`/`bN`; П0 contract rule that LLM must not provide unknown downstream ids.
  - Acceptance: renderer injects refs; `compress` accepts only visible refs; engine resolves refs internally and rejects invented/stale refs with structured tool errors.
  - Evidence required: tests for ref allocation, parsing, stale/missing refs, block refs, and capacity/error behavior.
  - Status: pending
  - Evidence collected:

- G5: Summary nesting uses structured data, not regex/string matching over LLM output.
  - Source: П0 ban on regex/string-match over LLM output; DCP placeholder mechanism identified as valuable but not directly acceptable.
  - Acceptance: `compress` schema represents summary as typed parts (`text`, `block_ref`, etc.) or an equivalent structured contract; engine validates required block refs exactly once with no regex-dependent semantic parsing.
  - Evidence required: schema tests and validation tests for missing, duplicate, unknown, and extra block refs.
  - Status: pending
  - Evidence collected:

- G6: Tool-call history remains provider-valid after rendering.
  - Source: existing invariant in `repair_agent_message_history_runtime`; compaction must not create orphaned tool results or partial tool-call batches.
  - Acceptance: rendered model context never contains orphaned tool results, partial completed tool batches, or invalid terminal open tool batches.
  - Evidence required: unit/property tests over rendered histories plus integration coverage for read/write/tool-heavy histories.
  - Status: pending
  - Evidence collected:

- G7: DCP-inspired pruning strategies are unified with rendering.
  - Source: DCP `deduplicate` and `purgeErrors`; current Oxide read-file dedup is local to `history.rs`.
  - Acceptance: duplicate/superseded tool outputs and old errored tool inputs are pruned by strategy state during rendering, with protected tools/files respected.
  - Evidence required: tests for duplicate tool signature grouping, write/edit intervention, purge-error age threshold, protected tool/file bypass, and token accounting.
  - Status: pending
  - Evidence collected:

- G8: Budget and nudge policy is centralized and route-aware.
  - Source: RECON finding scattered `CompactionPolicy::default()`; DCP min/max and per-model overrides.
  - Acceptance: budget thresholds, turn protection, nudge frequency, protected tools/files, and per-model overrides come from one policy object/config path; runner/hooks/tools do not instantiate independent defaults.
  - Evidence required: grep audit for scattered default policy use; policy unit tests; token snapshot tests for route-specific thresholds.
  - Status: pending
  - Evidence collected:

- G9: Old replacement pipeline is deleted completely.
  - Source: user request to throw away current system.
  - Acceptance: `build_compacted_history`, `replace_compacted_history`, `CompactionController`, duplicated event emitters, no-op stubs, and stale archive/externalization pieces are removed or fully rewritten into the new engine/render/admission architecture. No compatibility shim, production fallback, or test-only old replacement helper remains.
  - Evidence required: call-site grep audit proving old symbols are absent; deleted/replaced tests; runtime integration tests passing through new engine.
  - Status: pending
  - Evidence collected:

- G10: Emergency context-bomb admission prevents oversized input from entering hot memory inline.
  - Source: User review focus on recommended A+B+C+typed-D scheme; current RECON found new task/runtime context/tool output can be appended before safe admission.
  - Acceptance: every new external or tool-produced payload passes through `ContextAdmission` before `AgentMemory` mutation. If it cannot fit the current rendered budget, raw content is losslessly archived or referenced, and hot memory receives only a bounded manifest/summary descriptor.
  - Evidence required: tests for oversized new task, runtime context injection, document/preprocessor output, and large tool/file-read output showing raw payload is not inline in memory while artifact refs/manifests are preserved.
  - Status: pending
  - Evidence collected:

- G11: Optional chunked emergency summarization is bounded and never receives the whole bomb in one prompt.
  - Source: Recommended scheme B; П0.5/П0 requirement that emergency summarizer not rely on unsafe oversized calls.
  - Acceptance: emergency summarization runs only over bounded chunks or artifact ranges, emits chunk summaries plus a summary-of-summaries block, and degrades to manifest-only when summarization is unavailable.
  - Evidence required: tests for chunk sizing, summary-of-summaries creation, summarizer failure fallback to manifest-only, and preservation of artifact refs for later targeted retrieval.
  - Status: pending
  - Evidence collected:

- G12: Controlled pause/reject is the terminal fallback when safe continuation is impossible.
  - Source: Recommended scheme C.
  - Acceptance: if raw payload cannot be archived/referenced, no bounded manifest can fit, or the task requires exact full-content reasoning unavailable through chunks/ranges, the agent stops or asks the user with exact size/budget/reason instead of crashing or sending an oversized provider request.
  - Evidence required: tests for archive failure, no retrieval tool, manifest-over-budget, and exact-analysis-required cases producing structured blocker/pause output.
  - Status: pending
  - Evidence collected:

- G13: Provider context-limit fallback is typed and bounded.
  - Source: Recommended scheme typed D; current RECON found string matching in `llm_error_suggests_context_overflow`.
  - Acceptance: provider overflow handling uses typed `LlmError` classification/capability metadata rather than substring matching; retry count is bounded; retry invokes the same render shrink/emergency compaction path.
  - Evidence required: grep proving overflow substring classifier is removed from production flow; tests for typed overflow -> emergency shrink -> retry and repeated overflow -> controlled stop.
  - Status: pending
  - Evidence collected:

- G14: Compaction trigger conditions and initiators are explicit and exhaustive.
  - Source: User question “compaction то как срабатывает? при каких условиях, по чьей инициативе?”
  - Acceptance: the implementation has a closed trigger matrix covering context admission, pre-LLM budget checks, agent-requested `compress`, user/manual compaction, model downshift, and typed provider overflow; no hidden/scattered compaction trigger mutates state outside the matrix.
  - Evidence required: design doc trigger matrix; grep/call-site audit for compaction entry points; integration tests or targeted unit tests for each trigger.
  - Status: pending
  - Evidence collected:

### Quality requirements (Q*)

- Q1: П0-compliant root redesign, no symptom patches.
  - Source: AGENTS.md П0.
  - Acceptance: no workaround that merely validates/synchronizes old destructive replacement behavior; architecture makes transcript loss and id hallucination impossible by contract.
  - Evidence required: design doc section mapping old failure classes to new impossible states; code review checklist before implementation.
  - Status: pending
  - Evidence collected:

- Q2: П0.5 verification precedes code touching external/uncontrolled contracts.
  - Source: AGENTS.md П0.5.
  - Acceptance: before storage/schema/provider-contract changes, verification skeleton records commands/queries and actual observed outputs.
  - Evidence required: checked-in or goal-doc-linked verification notes for SQLx serialization/backward compatibility and provider-render constraints.
  - Status: pending
  - Evidence collected:

- Q3: П0.6 blast radius is checked after each implementation checkpoint.
  - Source: AGENTS.md П0.6.
  - Acceptance: each checkpoint records affected symbols/call-sites and validation/classification of any failures.
  - Evidence required: progress log entries with grep/call-site audits and monorepo-wide gates or justified narrower pre-commit gates.
  - Status: pending
  - Evidence collected:

- Q4: No direct AGPL code import.
  - Source: DCP donor license and implementation constraint.
  - Acceptance: implementation is original Rust design using concepts only; no copied TS code or prompt text verbatim unless license decision is explicitly made.
  - Evidence required: diff review; decisions log records conceptual reimplementation.
  - Status: pending
  - Evidence collected:

- Q5: Repository invariants remain intact.
  - Source: AGENTS.md architecture invariants.
  - Acceptance: core/runtime stay transport-agnostic; teloxide remains transport-only; module registry remains source of truth; no new crates/services without verified need.
  - Evidence required: Cargo diff review, dependency grep, module-registry check if module/profile changes occur.
  - Status: pending
  - Evidence collected:

- Q6: Runtime mine safety preserves progress without treating untrusted content as instructions.
  - Source: User question about agent hitting a “mine” while reading a file.
  - Acceptance: large or prompt-injection-like file/tool content is represented as untrusted data in manifests/chunks. Agent may continue with previews, targeted range reads, searches, or chunk summaries; it stops only when safe continuation is impossible.
  - Evidence required: tests/documented cases for huge file read, injected instruction inside file content, and continuation via range/search/chunk summary.
  - Status: pending
  - Evidence collected:

### Validation requirements (V*)

- V1: Core unit/integration tests for compaction pass.
  - Evidence required: targeted `cargo test -p oxide-agent-core ...` commands covering new compaction modules.
  - Status: pending
  - Evidence collected:

- V2: Workspace gates pass before completion.
  - Evidence required: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.
  - Status: pending
  - Evidence collected:

- V3: Transport/web compatibility is verified if event contracts change.
  - Evidence required: affected transport tests; if web UI touched, `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`.
  - Status: pending
  - Evidence collected:

### Non-goals (N*)

- N1: Do not import DCP implementation directly.
  - Must preserve: Rust-native implementation and licensing safety.
  - Evidence required: diff review.
  - Status: pending
  - Evidence collected:

- N2: Do not redesign unrelated subsystems.
  - Must preserve: wiki memory, browser live, sandbox, manager control plane, and LLM providers except compaction integration points.
  - Evidence required: git diff scope review.
  - Status: pending
  - Evidence collected:

## Implementation Plan

### Phase 0 — Review gate and verification skeleton

- Audit IDs: Q1, Q2, Q4, Q5
- Expected changes:
  - Finalize this goal doc after user review.
  - Create/update `docs/compaction-redesign.md` with verified current contracts and target architecture.
  - Record П0.5 checks for storage serialization, rendered provider history validity, and existing event consumers.
- Validation:
  - Documentation review only.
  - Commands/queries for later verification are listed before design is treated as final.
- Exit condition:
  - User approves or edits this goal.
  - Design doc has explicit contract boundaries and verification skeleton.

### Phase 1 — Raw transcript / rendered context split

- Audit IDs: G1, G6, Q1, Q3
- Expected changes:
  - Introduce `RenderedModelContext` or equivalent runner boundary.
  - Make token budget/snapshots reason about rendered context, not raw memory alone.
  - Keep renderer initially identity-equivalent except for explicit metadata plumbing.
- Validation:
  - Tests proving rendered output equals current output before blocks exist.
  - Call-site audit of `AgentMemory::token_count()` and `convert_memory_to_messages` usage.
- Exit condition:
  - Runner LLM calls consume rendered context through one boundary.
  - No behavior change when `CompactionState` is empty.

### Phase 2 — Persistent `CompactionState` and stable refs

- Audit IDs: G1, G3, G4, Q2
- Expected changes:
  - Add backward-compatible `CompactionState` to `AgentMemory` or a tightly coupled persisted session structure.
  - Implement message refs and block refs.
  - Add state sync for reset/load/rollback-like memory changes.
- Validation:
  - Serialization/deserialization tests from old memory JSON with absent compaction state.
  - Ref allocation/parsing/stale-ref tests.
  - Storage round-trip test if storage facade is touched.
- Exit condition:
  - Raw memory and empty/non-empty compaction state persist atomically.

### Phase 3 — Block graph and selection engine

- Audit IDs: G2, G3, G4, G5, G6
- Expected changes:
  - Implement range/message selection over refs.
  - Implement structured summary AST and block-ref validation.
  - Implement block creation, activation, consumption, parent/effective id propagation.
  - Reject unsafe selections before mutation.
- Validation:
  - Unit tests for non-overlap, missing/stale refs, nested block consumption, duplicate/extra block refs, tool-batch safety.
- Exit condition:
  - `CompactionEngine::apply_*` can create and recompress blocks without rendering changes enabled globally.

### Phase 4 — Compacted renderer and pruning strategies

- Audit IDs: G1, G6, G7, G8
- Expected changes:
  - Renderer injects active block summaries at anchors and skips covered raw messages.
  - Implement dedup and purge-error strategy state.
  - Preserve protected recent turns, protected tools/files, and valid tool-call batches.
  - Centralize policy/config access.
- Validation:
  - Renderer tests for compacted ranges, nested blocks, protected content, duplicate tools, write/edit intervention, old errored tools, and token deltas.
  - Grep audit for scattered `CompactionPolicy::default()`.
- Exit condition:
  - Model-facing context is compacted by renderer; raw transcript remains intact.

### Phase 5 — Emergency admission and runtime mine safety

- Audit IDs: G10, G11, G12, G13, Q6
- Expected changes:
  - Add `ContextAdmission` before new-task append, runtime-context drain, document/preprocessor payload insertion, and tool-output memory writes.
  - Implement A+B+C+typed-D scheme:
    - A: lossless archive/reference plus bounded manifest for any oversized payload.
    - B: optional bounded chunked summary and summary-of-summaries block when useful/available.
    - C: controlled pause/reject with exact reason when safe continuation is impossible.
    - typed D: provider context-limit fallback as a last-resort typed retry path.
  - Ensure large file reads/tool outputs become manifests with path/source/size/artifact ref/head-tail preview and retrieval instructions, not inline bombs.
  - Mark external file/tool content as untrusted data so prompt-injection text from files cannot become system/user instructions.
- Validation:
  - Tests for oversized user task, runtime context bomb, huge document/preprocessor output, huge file read/tool output, chunked summary success/failure, no-archive fallback, and typed provider overflow retry.
  - Render-budget tests proving no emergency path sends an oversized rendered request.
- Exit condition:
  - Runtime can encounter an oversized file/tool output and continue with manifest/chunk/range retrieval, or pause with a precise blocker if safe continuation is impossible.

### Phase 6 — Replace `compress` tool contract

- Audit IDs: G2, G4, G5, G7
- Expected changes:
  - Replace current no-arg scheduler with structured range/message compression schema.
  - Tool returns block ids/stats and structured validation errors.
  - Runner applies tool results through the same engine, not a separate path.
- Validation:
  - Tool schema tests.
  - Tool execution tests for valid compression and invalid refs/summary parts.
  - Integration test: model calls `compress`, next LLM request sees compacted rendered context.
- Exit condition:
  - Agent-facing compaction is real, structured, and engine-backed.

### Phase 7 — Migrate automatic/runtime triggers

- Audit IDs: G2, G8, G9, G14
- Expected changes:
  - Pre-sampling/pre-LLM budget threshold, context-limit retry, model downshift, hook/manual request, transport manual compaction, agent-requested `compress`, and context-admission emergency compaction all call the same engine.
  - Remove duplicate event emitters; normalize started/completed/failed/skipped semantics.
  - Decide whether `RuntimeCompactionSkipped` is emitted or removed from the public contract.
- Validation:
  - Integration tests for every trigger.
  - Web/Telegram progress tests updated if event contract changes.
  - Call-site grep proving one mutation authority.
- Exit condition:
  - All trigger-matrix entries produce blocks/rendered compaction or controlled pauses through unified engine/admission paths.

### Phase 8 — Delete old replacement pipeline and update docs

- Audit IDs: G9, N1, N2, V1, V2, V3
- Expected changes:
  - Delete `CompactionController`, `build_compacted_history`, `replace_compacted_history`, stale archive/externalization pieces, no-op stubs, and old replacement tests/helpers instead of leaving compatibility tails.
  - Update `docs/context-window-tracking.md` or add new compaction architecture docs.
  - Migrate old compaction tests to new semantics.
- Validation:
  - Grep/call-site audit for deleted old symbols.
  - Full workspace gates.
  - Diff scope review for non-goals.
- Exit condition:
  - Old replacement system is absent from production and test/helper paths; all audit items have evidence.

## Validation Contract

- Static checks:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
- Tests:
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full` for focused core coverage when profile-gated tests require it.
  - `cargo test --workspace` before completion.
- Build/check:
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - `cargo build --release --no-default-features --features profile-full` when final binary confidence is practical.
- Transport/UI:
  - If web UI touched: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`.
- Artifact verification:
  - Grep/call-site audit for old compaction symbols and scattered policy defaults.
  - Diff review proving DCP code was not copied directly.
- Done when:
  - Every G/Q/V/N item is verified with current evidence.

## Decisions

- 2026-06-22: Use DCP as conceptual donor only. Reason: DCP's block graph/render overlay solves the root class, but direct code import is unsuitable due to language, SDK, persistence, UI, and license constraints.
- 2026-06-22: Replace DCP regex placeholder summaries with structured summary parts. Reason: regex/string matching over LLM output violates П0; the receiver must validate typed references, not parse prose semantics.
- 2026-06-22: Store compaction state with raw memory. Reason: transcript and compaction overlay must checkpoint atomically to avoid drift/race between rendered state and persisted messages.
- 2026-06-22: Make renderer the only model-facing compaction boundary. Reason: destructive memory replacement is the root defect; rendered context can shrink without losing source transcript.
- 2026-06-22: Emergency compaction is admission-first, not provider-error-first. Reason: oversized user/runtime/tool payloads must be externalized or summarized before hot-memory mutation; provider overflow is only a typed drift fallback.
- 2026-06-22: No old compaction compatibility shim is allowed. Reason: user explicitly requested replace without tails; leaving a non-authoritative old path would preserve the broken contract and violate П0.

## Progress Log

- 2026-06-22 00:00: Goal plan created after RECON.
  - Changed: added `docs/goals/2026-06-22-compaction-dcp-redesign.md`.
  - Evidence: current compaction and DCP donor mapped in conversation RECON; no code changed.
  - Commands: preflight reads of `README.md`, `Cargo.toml`, existing goal docs, context-window docs, and hooks docs.
  - Audit IDs updated: none verified yet; all implementation audit items pending review.
  - Next: user review; then Phase 0 design/verification skeleton.

- 2026-06-22 00:20: Emergency context-bomb/runtime mine design added after user review question.
  - Changed: added ingress/tool-output admission contract, G10-G13, Q6, and emergency Phase 5 for A+B+C+typed-D handling.
  - Evidence: current RECON showed new task/runtime context/tool output can be appended before safe admission; recommended design requires archive/manifest, optional chunked summary, controlled pause, and typed provider overflow fallback.
  - Commands: targeted reads of the goal doc and prior RECON of `executor/execution.rs`, `runner/execution.rs`, `runner/runtime_compaction.rs`, `memory.rs`, and `session.rs`.
  - Audit IDs updated: G10, G11, G12, G13, Q6 added as pending.
  - Next: review emergency phase ordering and decide exact artifact/retrieval contract during Phase 0.

- 2026-06-22 00:30: Trigger matrix and full old-system deletion requirement added.
  - Changed: added explicit compaction trigger matrix, G14, and changed G9/objective/Phase 8 from non-authoritative compatibility language to full deletion/no-tail language.
  - Evidence: user asked whether trigger conditions/initiators are documented and whether replacement is without old-system tails.
  - Commands: targeted goal-doc reads before edit; whitespace/diff checks after edit.
  - Audit IDs updated: G9 strengthened; G14 added as pending.
  - Next: user approval; then Phase 0 verification skeleton before implementation.

## Risks and Blockers

- Risk: Existing e2e tests assume destructive summary-boundary behavior.
  - Impact: tests must be migrated to rendered-overlay semantics, not patched around old expectations.
  - Evidence: RECON found extensive `transport-web/tests/e2e/compaction_regression_tests.rs` coverage of current behavior.
  - Mitigation: keep old tests as behavior inventory, rewrite assertions around raw preservation + rendered compaction.
  - Audit IDs affected: G1, G6, G9, V1, V2.

- Risk: Storage serialization change can break existing persisted sessions.
  - Impact: users may have memory checkpoints without compaction state.
  - Evidence: `AgentMemory` is serialized through SQLx storage facade.
  - Mitigation: `serde(default)` and old-json round-trip tests before rollout.
  - Audit IDs affected: G1, Q2.

- Risk: Renderer can accidentally break provider tool-call pairing.
  - Impact: model calls fail with repairable or unrecoverable history errors.
  - Evidence: current code has explicit repair/validation paths for tool history.
  - Mitigation: renderer-level validation plus tests over completed and terminal open tool batches.
  - Audit IDs affected: G6.

- Risk: Runtime file/tool “mines” can still enter hot memory if admission is applied only to user input.
  - Impact: agent may overflow, obey untrusted prompt-injection-like file content, or crash/retry late at provider boundary.
  - Evidence: current tool-result memory writing happens in runner/tool output paths; user explicitly asked about agent hitting a file mine during runtime.
  - Mitigation: place `ContextAdmission` before every hot-memory mutation path for external/tool payloads, not only at transport ingress.
  - Audit IDs affected: G10, G11, G12, G13, Q6.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
