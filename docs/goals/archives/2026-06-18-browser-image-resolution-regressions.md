# Goal: Fix browser image resolution + MiMo empty content 400 regression

Date started: 2026-06-18
Status: active
Codex goal: (to be set)
Source spec: live test regression after Postgres BYTEA goal (CP5 stopped writing screenshots to disk)
Goal doc owner: Codex
Last updated: 2026-06-18 18:00

## Objective

Two regressions introduced by the Postgres BYTEA screenshot goal must be fixed fundamentally:

1. **Image resolution regression:** Browser screenshots saved to Postgres BYTEA are never sent to the LLM as vision input. After checkpoint save/load, inline `data` bytes are lost (`#[serde(skip)]`), and the filesystem fallback fails because CP5 stopped writing to disk. The runner has no Postgres lookup key and no `StorageProvider` handle.

2. **MiMo 400 "text is not set" regression:** `assistant_message()` serializes `"content": ""` for tool-only assistant messages (empty content + tool_calls). MiMo expects `"content": null`. This causes a 400 error on the iteration after a tool-only assistant response.

Done when both regressions are fixed, verified by live test (browser tool sends screenshot to LLM, MiMo no longer 400s on tool-only iterations), and all gates pass.

## Scope

In scope:
- `crates/oxide-agent-core/src/agent/memory.rs` — `AgentMessageAttachment` struct
- `crates/oxide-agent-core/src/agent/tool_runtime/output.rs` — `ToolOutputImageAttachment` struct
- `crates/oxide-agent-core/src/agent/providers/browser_live/tools.rs` — `screenshot_image_attachment`
- `crates/oxide-agent-core/src/agent/runner/tools.rs` — `apply_runtime_tool_output` (attachment creation)
- `crates/oxide-agent-core/src/agent/runner/llm_calls.rs` — `attach_native_image_parts_from_refs` (3-tier resolution)
- `crates/oxide-agent-core/src/agent/runner/types.rs` — `AgentRunnerContext` (add storage + user_id)
- `crates/oxide-agent-core/src/agent/executor/types.rs` — `PreparedExecution::build_runner_context` (thread storage)
- `crates/oxide-agent-core/src/agent/executor/execution.rs` — pass storage to PreparedExecution
- `crates/oxide-agent-core/src/agent/executor.rs` — `AgentExecutor` (storage already exists, thread to PreparedExecution)
- `crates/oxide-agent-core/src/llm/providers/chat_completions/request.rs` — `assistant_message()` (null content fix)
- Transport wiring: web + telegram session.rs (already call `.with_storage()`)
- Tests for both fixes

Out of scope:
- Changing `BrowserSidecar` trait or client (N1 from previous goal still applies)
- Adding new crates
- Changing Postgres schema
- Changing artifact URI scheme
- Changing the checkpoint persistence format (only adding a new serializable field)
- Telegram transport (browser tools are web-only in practice; storage already wired)
- Non-browser image attachments (filesystem tools still work via sandbox path)

## Repository Context

- `AgentExecutor.storage: Option<Arc<dyn StorageProvider>>` already exists (executor.rs:181), set by transport via `.with_storage()`
- `AgentRunnerContext` (runner/types.rs:127) has `memory_scope: Option<AgentMemoryScope>` which contains `user_id: i64`
- `AgentMessageAttachment.data: Option<Vec<u8>>` is `#[serde(skip, default)]` — persists as `null` in checkpoint JSON
- `ToolOutputImageAttachment` is NOT serialized (lives only in `ToolOutput`, transient)
- `attach_native_image_parts_from_refs` (llm_calls.rs:108-199) does 2-tier: inline `data` → `read_native_image_file(sandbox_path)`
- `assistant_message()` (request.rs:429-467) always sends `"content": msg.content` as String — never null
- `AgentMemoryScope` (session.rs:79) has `user_id: i64` — runner context already has this via `memory_scope`
- `load_browser_artifact(user_id: i64, artifact_uri: &str)` exists in storage trait (CP6 of previous goal)
- Postgres: `browser_artifacts` table with `artifact_uri` PK, `data BYTEA`, `user_id BIGINT`

## Completion Audit

- G1: `AgentMessageAttachment` has serializable `artifact_uri: Option<String>` field
  - Source: live test log "Skipping native image attachment... File not found"
  - Acceptance: field persists through checkpoint save/load; after reload, `artifact_uri` is `Some(uri)` for browser screenshots
  - Evidence required: `cargo test` on memory.rs serialization test; `git grep artifact_uri` in memory.rs
  - Status: pending
  - Evidence collected:

- G2: `ToolOutputImageAttachment` has `artifact_uri: Option<String>` field
  - Source: same as G1 — upstream of AgentMessageAttachment
  - Acceptance: field set when browser provider creates attachment; flows through to AgentMessageAttachment
  - Evidence required: `git grep artifact_uri` in output.rs and tools.rs
  - Status: pending
  - Evidence collected:

- G3: Browser provider passes `frame.artifact.uri` into attachment
  - Source: `screenshot_image_attachment` in tools.rs:83-113 currently has no artifact_uri
  - Acceptance: `screenshot_image_attachment` accepts `artifact_uri: Option<&str>`, sets it on `ToolOutputImageAttachment`
  - Evidence required: code review of tools.rs; browser_live test
  - Status: pending
  - Evidence collected:

- G4: Runner resolves image bytes via 3-tier fallback: `data` → `load_browser_artifact(user_id, artifact_uri)` → `read_native_image_file(sandbox_path)`
  - Source: llm_calls.rs:150-158 currently only does 2-tier
  - Acceptance: when `data` is None and `artifact_uri` is Some, calls `storage.load_browser_artifact(user_id, artifact_uri)` to get bytes from Postgres; only falls back to filesystem if storage lookup also fails
  - Evidence required: code review of llm_calls.rs; unit test with mock storage
  - Status: pending
  - Evidence collected:

- G5: `AgentRunnerContext` has access to `StorageProvider` + `user_id`
  - Source: runner/types.rs:127 has no storage field
  - Acceptance: `AgentRunnerContext` has `storage: Option<Arc<dyn StorageProvider>>`; threaded from `AgentExecutor.storage` via `PreparedExecution::build_runner_context`; `user_id` extracted from `memory_scope.user_id`
  - Evidence required: code review of types.rs + executor/types.rs; `cargo check` passes
  - Status: pending
  - Evidence collected:

- G6: `assistant_message()` sends `content: null` for tool-only messages (empty content + has tool_calls)
  - Source: request.rs:432 `"content": msg.content` always String; MiMo 400 "text is not set"
  - Acceptance: when `msg.content.is_empty()` and `msg.tool_calls` is non-empty, JSON has `"content": null` instead of `"content": ""`
  - Evidence required: code review of request.rs; unit test asserting null content for empty tool-only assistant message
  - Status: pending
  - Evidence collected:

- Q1: No new crates added
  - Source: previous goal N2
  - Acceptance: `git diff -- Cargo.toml` shows no new dependencies
  - Evidence required: `git diff` check
  - Status: pending
  - Evidence collected:

- Q2: `cargo fmt --all -- --check` passes
  - Source: AGENTS.md CI requirement
  - Acceptance: zero diff
  - Evidence required: command output
  - Status: pending
  - Evidence collected:

- Q3: `cargo clippy --workspace --all-targets -- -D warnings` passes (or scoped profiles)
  - Source: AGENTS.md CI requirement
  - Acceptance: zero warnings
  - Evidence required: command output on profile-full
  - Status: pending
  - Evidence collected:

- Q4: All existing tests pass
  - Source: AGENTS.md
  - Acceptance: `cargo test -p oxide-agent-core` (profile-full) + `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local` + `cargo test -p oxide-agent-web-ui` all green
  - Evidence required: test output
  - Status: pending
  - Evidence collected:

- V1: Live test — browser tool sends screenshot to LLM as vision input
  - Source: user request "скинь мне ссылку" test
  - Acceptance: no "Skipping native image attachment" warning in logs; LLM receives screenshot as image_url part; MiMo no longer 400s
  - Evidence required: docker compose up + live browser task + log inspection
  - Status: pending
  - Evidence collected:

- N1: `BrowserSidecar` trait and client.rs unchanged
  - Source: previous goal N1
  - Acceptance: `git diff -- crates/oxide-agent-core/src/agent/providers/browser_live/client.rs` shows no changes
  - Evidence required: git diff
  - Status: pending
  - Evidence collected:

- N2: No Postgres schema changes
  - Source: previous goal already applied migration 0008+0009
  - Acceptance: no new migration files
  - Evidence required: `git diff -- migrations/`
  - Status: pending
  - Evidence collected:

- N3: Filesystem-only image attachments (non-browser) still work
  - Source: existing filesystem tools create attachments without artifact_uri
  - Acceptance: attachments with `artifact_uri: None` and `data: None` still try `read_native_image_file(sandbox_path)` — 3rd tier unchanged
  - Evidence required: code review; existing tests pass
  - Status: pending
  - Evidence collected:

## Implementation Plan

1. CP1 — Add `artifact_uri` to attachment structs
   - Audit IDs: G1, G2
   - Expected changes:
     - `AgentMessageAttachment` (memory.rs): add `#[serde(default, skip_serializing_if = "Option::is_none")] pub artifact_uri: Option<String>`
     - `ToolOutputImageAttachment` (output.rs): add `pub artifact_uri: Option<String>`
     - Update both `image()` and `image_with_data()` constructors to accept/set `artifact_uri`
     - Update all constructor call sites
   - Validation: `cargo check -p oxide-agent-core`; serialization test (artifact_uri persists)
   - Exit condition: compiles, existing tests pass

2. CP2 — Browser provider passes artifact_uri into attachment
   - Audit IDs: G3
   - Expected changes:
     - `screenshot_image_attachment` (tools.rs:83-113): accept `artifact_uri: Option<&str>`, pass to `ToolOutputImageAttachment`
     - Call sites in `observe` and `execute`: pass `Some(&frame.artifact.uri)`
     - `apply_runtime_tool_output` (runner/tools.rs:408-427): pass `artifact_uri` from `ToolOutputImageAttachment` to `AgentMessageAttachment`
   - Validation: `cargo check`; browser_live tests pass
   - Exit condition: compiles, `artifact_uri` flows from frame.artifact.uri to AgentMessageAttachment

3. CP3 — Runner 3-tier image resolution + storage threading
   - Audit IDs: G4, G5
   - Expected changes:
     - `AgentRunnerContext` (runner/types.rs): add `storage: Option<Arc<dyn StorageProvider>>` field
     - `PreparedExecution::build_runner_context` (executor/types.rs): thread `storage` from executor
     - `execution.rs`: pass `self.storage` to `PreparedExecution`
     - `attach_native_image_parts_from_refs` (llm_calls.rs:108-199): add 3rd tier — when `data` is None and `artifact_uri` is Some, call `storage.load_browser_artifact(user_id, artifact_uri)`. Get `user_id` from `ctx.memory_scope.user_id`.
     - Unit test: mock storage returns bytes for artifact_uri
   - Validation: `cargo test -p oxide-agent-core`; unit test for 3-tier resolution
   - Exit condition: compiles, 3-tier test passes

4. CP4 — Fix assistant_message empty content → null
   - Audit IDs: G6
   - Expected changes:
     - `assistant_message()` (request.rs:429-467): when `msg.content.is_empty()` and `msg.tool_calls` is non-empty (or resolved to non-empty), set `"content": null` instead of `"content": ""`
     - Unit test: empty content + tool_calls → content is null; non-empty content → content is string; empty content + no tool_calls → content is "" (unchanged)
   - Validation: `cargo test` on chat_completions request tests
   - Exit condition: compiles, unit test passes

5. CP5 — Final verification + live test
   - Audit IDs: Q1, Q2, Q3, Q4, V1, N1, N2, N3
   - Expected changes:
     - Docker rebuild + live browser task test
     - Log inspection: no "Skipping" warning, no 400 error
     - Full gate run: fmt + clippy + test
   - Validation: all gates green; live test passes
   - Exit condition: all audit items verified

## Validation Contract

- Static checks: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`
- Tests: `cargo test -p oxide-agent-core --no-default-features --features profile-full`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`, `cargo test -p oxide-agent-web-ui`
- Runtime/manual verification: `docker compose -f docker-compose.web.yml up --build`, run browser task ("перейди на https://ots.bash.md/, напиши hello world, submit, share"), inspect logs for: (1) no "Skipping native image attachment" warnings, (2) no 400 "text is not set" errors, (3) screenshot appears in LLM context
- Done when: V1 verified by live test + all Q/N items verified by commands

## Decisions

- 2026-06-18: `artifact_uri` is serializable (not `#[serde(skip)]`) because it must survive checkpoint save/load. It's a small String — no storage cost concern.
- 2026-06-18: 3-tier resolution order: inline `data` (fastest, in-memory) → Postgres `load_browser_artifact` (durable, for post-checkpoint) → filesystem `read_native_image_file` (legacy fallback). This is the natural order — inline first, then durable, then legacy.
- 2026-06-18: `content: null` for empty tool-only assistant messages follows the OpenAI API spec (content should be null when tool_calls is present and no text). MiMo expects this.
- 2026-06-18: `user_id` comes from `AgentRunnerContext.memory_scope.user_id` — already available, no new field needed for user_id.

## Progress Log

(updated after each checkpoint)

## Risks and Blockers

- Risk: `AgentRunnerContext` is constructed in multiple places (executor + tests). Adding a field may break test constructors.
  - Mitigation: make field `Option`, default to `None` in test helpers.
- Risk: `assistant_message` null content may break other providers that expect String content.
  - Mitigation: only set null when content is empty AND tool_calls is non-empty. Non-empty content always stays as String. Empty content without tool_calls stays as "" (existing behavior).

## Final Verification

(filled when complete)
