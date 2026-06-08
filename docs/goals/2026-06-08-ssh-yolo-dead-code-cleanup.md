# Goal: SSH YOLO mode and dead-code cleanup

Date started: 2026-06-08
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-08-ssh-yolo-dead-code-cleanup.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update the doc after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: user request after RECON
Goal doc owner: Codex
Last updated: 2026-06-08

## Objective

Remove the inactive SSH approval pipeline and confirmed dead/unnecessary code found during RECON while preserving ordinary YOLO SSH execution through existing topic infra bindings and allowed tool modes.

Done when every required Completion Audit item is verified by its listed evidence and all out-of-scope constraints are preserved.

## Scope

In scope:
- `crates/oxide-agent-core/src/agent/providers/ssh_mcp.rs`
- `crates/oxide-agent-core/src/agent/providers/ssh_mcp_stub.rs`
- `crates/oxide-agent-core/src/agent/providers/mod.rs`
- `crates/oxide-agent-core/src/agent/executor/`
- `crates/oxide-agent-core/src/storage/control_plane.rs`
- `crates/oxide-agent-core/src/agent/providers/manager_control_plane/`
- Confirmed RECON lint/dead-code cleanup in core, web transport tests, and wasm UI when it is low-risk and directly verified by clippy.
- Docs/config references that describe SSH approval as active behavior.

Out of scope:
- Replacing SSH approval with a new approval system.
- Changing SSH transport/protocol, MCP integration, secret-ref handling, or topic infra binding semantics.
- Removing or restricting ordinary SSH tools: `exec`, `sudo_exec`, `ssh_read_file`, `ssh_apply_file_edit`, `ssh_send_file_to_user`, `check_process`.
- Broad provider feature-gating refactors unrelated to SSH approval.
- Public API cleanup that would require transport redesign unless directly needed to remove approval.

## Missing Inputs

None. User decision is explicit: remove SSH approval and keep YOLO SSH.

## Repository Context

- Relevant entry points: `ssh_mcp.rs`, `ssh_mcp_stub.rs`, `agent/executor/execution.rs`, `storage/control_plane.rs`, `manager_control_plane` infra tools.
- Existing convention: smallest working change, no new crates, feature-gated profiles, clippy/fmt required before finishing.
- Current RECON evidence: SSH approval heuristics are explicitly disabled with `APPROVAL DISABLED` comments and `#[allow(dead_code)]`; approval config is stored but not enforced.
- Validation infrastructure: scoped `cargo check`, scoped `cargo clippy -- -D warnings`, `cargo fmt --all -- --check`.
- Risky areas: storage/API structs may deserialize persisted JSON. Removing fields must not break existing records; unknown serde fields are acceptable only if confirmed in code.

## Completion Audit

- G1: SSH approval runtime pipeline removed
  - Source: user request: "SSH approval - выкинуть из кода, оставить обычный yolo ssh"
  - Acceptance: no production approval registry, pending approval queue, approval token, approval replay injection, or approval system prompt remains in SSH execution paths.
  - Evidence required: `rg -n "SshApproval|approval_request|approval_token|inject_approval|APPROVAL DISABLED|is_dangerous_command|is_sensitive_path" crates/oxide-agent-core/src` shows no live approval pipeline symbols, or only documented migration-safe compatibility if explicitly justified.
  - Status: verified
  - Evidence collected: 2026-06-08 Checkpoint 1 removed approval registry/request/token/replay helpers, executor pending/resume APIs, transport approval callbacks/keyboards, approval progress events, and approval replay memory kind. `rg -n "SshApproval|approval_request|approval_token|inject_approval|APPROVAL DISABLED|is_dangerous_command|is_sensitive_path|WaitingForApproval|ApprovalReplay|approval_replay|ssh_approval|agent:ssh|YOLO_APPROVAL" crates/oxide-agent-core/src crates/oxide-agent-transport-telegram/src crates/oxide-agent-transport-web/src` returned no matches.

- G2: YOLO SSH behavior preserved
  - Source: user request and AGENTS SSH invariants
  - Acceptance: existing SSH tools still register/compile under `integration-ssh-mcp`; allowed tool modes and secret refs remain the only access controls.
  - Evidence required: `cargo check -p oxide-agent-core --no-default-features --features integration-ssh-mcp,manager-control-plane`, plus grep/read evidence that `allowed_tool_modes` enforcement remains.
  - Status: verified
  - Evidence collected: 2026-06-08 `cargo check -p oxide-agent-core --no-default-features --features integration-ssh-mcp,manager-control-plane` passed. `ssh_mcp.rs` still registers ordinary SSH tools and keeps `allowed_tool_modes` enforcement in `SshRuntimeToolExecutor::ensure_mode_allowed`.

- G3: Approval config/API surface pruned
  - Source: user request to remove approval, RECON finding that `approval_required_modes` is not enforced.
  - Acceptance: manager-control-plane no longer creates, updates, displays, or documents approval-required modes as active behavior. Storage compatibility is either removed safely or explicitly retained only as ignored legacy input.
  - Evidence required: `rg -n "approval_required_modes|approval required|approval-required" crates/oxide-agent-core docs config profiles README.md` has no active behavior references except compatibility notes if needed.
  - Status: verified
  - Evidence collected: 2026-06-08 Checkpoint 2 removed `approval_required_modes` from topic infra records/options, SQLx read/write paths, manager-control-plane tool schemas/args/responses, README security docs, and tests. Persisted SQL column remains ignored for compatibility; serde ignores legacy JSON fields because the storage record does not deny unknown fields. Targeted `rg` now only matches this goal document.

- G4: Confirmed dead code and lint blockers removed
  - Source: RECON report
  - Acceptance: remove low-risk dead code and fix current clippy blockers without unrelated refactors.
  - Evidence required: targeted clippy commands in Validation Contract pass or remaining failures are documented as out-of-scope with exact evidence.
  - Status: pending
  - Evidence collected:

- Q1: No new abstractions or dependencies
  - Source: AGENTS.md implementation bias
  - Acceptance: no new crates, services, queues, traits, approval replacements, or feature frameworks.
  - Evidence required: no Cargo dependency additions; diff review confirms deletion/simplification only.
  - Status: pending
  - Evidence collected:

- Q2: Architecture invariants preserved
  - Source: AGENTS.md architectural invariants
  - Acceptance: core/runtime remain transport-agnostic; teloxide stays transport-only; direct Gemini provider remains absent; SQLx durable storage invariant unaffected.
  - Evidence required: diff review and scoped checks.
  - Status: in_progress
  - Evidence collected: 2026-06-08 Checkpoint 2 changed only core storage/control-plane/docs surfaces and kept SQLx compatibility by ignoring the legacy column instead of adding a new storage backend or migration requirement.

- V1: Formatting and lint validation completed
  - Source: AGENTS.md format/lint rules
  - Acceptance: relevant check/clippy/fmt commands pass before completion.
  - Evidence required: command output summaries recorded in Progress Log and Final Verification.
  - Status: pending
  - Evidence collected:

- N1: No SSH behavior expansion
  - Source: user request to keep ordinary YOLO SSH, not redesign it
  - Must preserve: no new prompts, approval UX, queues, tokens, or operator confirmation flow.
  - Evidence required: diff review shows only removal/simplification around approval.
  - Status: verified
  - Evidence collected: 2026-06-08 diff removes approval prompts/queues/tokens/callbacks and does not add replacement approval UX or controls.

## Implementation Plan

### Checkpoint 0: goal contract
- Audit IDs: planning only
- Expected changes: create this goal document with objective, scope, audit ledger, checkpoints, and validation contract.
- Validation: review `git diff`, commit docs-only checkpoint.
- Exit condition: goal doc committed and next implementation step identified.

### Checkpoint 1: remove SSH approval execution plumbing
- Audit IDs: G1, G2, N1
- Expected changes: delete approval registry/request/token/replay/system-prompt helpers from real and stub SSH providers; remove executor pending-approval plumbing; delete approval-only tests; keep normal SSH tool execution and preflight/status code.
- Validation:
  - `cargo check -p oxide-agent-core --no-default-features --features integration-ssh-mcp,manager-control-plane`
  - `cargo clippy -p oxide-agent-core --no-default-features --features integration-ssh-mcp,manager-control-plane --all-targets -- -D warnings`
  - targeted `rg` for approval symbols.
- Exit condition: SSH approval symbols are gone from live code and SSH feature build remains green.

### Checkpoint 2: prune approval config/storage/control-plane surface
- Audit IDs: G3, G2, Q2
- Expected changes: remove `approval_required_modes` from active topic infra creation/update/display paths; inspect serde/storage compatibility before deleting stored fields; update tests and docs/config references.
- Validation:
  - `cargo check -p oxide-agent-core --no-default-features --features manager-control-plane,integration-ssh-mcp,storage-sqlx`
  - `rg -n "approval_required_modes|approval required|approval-required" crates/oxide-agent-core docs config profiles README.md`
- Exit condition: no active approval config remains; persisted-data compatibility decision is recorded.

### Checkpoint 3: remove confirmed dead code and lint blockers
- Audit IDs: G4, Q1, V1
- Expected changes: fix current clippy blockers from RECON: unused imports, `vec_init_then_push`, `unwrap_err`, `await_holding_lock`, no-feature test helper dead code, web transport test unused imports, wasm UI simple lint fixes. Avoid broad public API refactors unless required by compilation.
- Validation:
  - `cargo clippy -p oxide-agent-core --no-default-features --all-targets -- -D warnings`
  - `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --all-targets -- -D warnings`
  - `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --all-targets -- -D warnings`
  - `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown -- -D warnings`
- Exit condition: targeted clippy blockers are green or any remaining unrelated blocker is documented with exact evidence.

### Checkpoint 4: final validation and audit
- Audit IDs: G1, G2, G3, G4, Q1, Q2, V1, N1
- Expected changes: update this goal doc with evidence, final verification, and commit summary.
- Validation:
  - `cargo fmt --all -- --check`
  - repeat any targeted checks affected by final diff.
- Exit condition: every Completion Audit item is verified or explicitly blocked with evidence.

## Validation Contract

- Static checks:
  - `cargo check -p oxide-agent-core --no-default-features --features integration-ssh-mcp,manager-control-plane`
  - `cargo check -p oxide-agent-core --no-default-features --features manager-control-plane,integration-ssh-mcp,storage-sqlx`
- Lint:
  - `cargo clippy -p oxide-agent-core --no-default-features --all-targets -- -D warnings`
  - `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --all-targets -- -D warnings`
  - `cargo clippy -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --all-targets -- -D warnings`
  - `cargo clippy -p oxide-agent-web-ui --target wasm32-unknown-unknown -- -D warnings`
- Formatting: `cargo fmt --all -- --check`
- Artifact verification: targeted `rg` searches for approval symbols and config references.
- Done when: all audit IDs are verified and the goal doc contains current evidence.

## Decisions

- 2026-06-08: Remove SSH approval instead of repairing it. Reason: user explicitly requested YOLO SSH and RECON showed the approval path is disabled/dead.
- 2026-06-08: Do not replace approval with a new abstraction. Reason: personal-use scale and AGENTS.md forbid unnecessary complexity.
- 2026-06-08: Dead-code cleanup is limited to confirmed RECON findings and clippy blockers. Reason: avoid broad public API churn.
- 2026-06-08: Keep the legacy SQLx `topic_infra_configs.approval_required_modes` column ignored instead of requiring an immediate migration. Reason: code no longer reads/writes it, fresh inserts use the existing default, and old databases remain compatible.

## Progress Log

- 2026-06-08: goal doc created
  - Changed: `docs/goals/2026-06-08-ssh-yolo-dead-code-cleanup.md`
  - Evidence: RECON completed; user selected removal of SSH approval and preservation of YOLO SSH.
  - Commands: `git status --short`, `git log --oneline -5`, read existing goal docs and AGENTS.md.
  - Audit IDs updated: none yet.
  - Next: Checkpoint 1 — remove SSH approval execution plumbing.

- 2026-06-08: Checkpoint 1 implemented
  - Changed: removed SSH approval registry/replay plumbing from core providers, executor, progress, Telegram callbacks/views/task delivery, and web task handling; kept topic infra preflight and ordinary SSH tool paths.
  - Evidence: approval-symbol `rg` returned no matches; `allowed_tool_modes` remains enforced in `ssh_mcp.rs`; no Cargo dependency changes.
  - Commands: `cargo check -p oxide-agent-core --no-default-features --features integration-ssh-mcp,manager-control-plane`; `cargo clippy -p oxide-agent-core --no-default-features --features integration-ssh-mcp,manager-control-plane --all-targets -- -D warnings`; `cargo check -p oxide-agent-transport-telegram --no-default-features --features profile-embedded-opencode-local`; `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`; `cargo fmt --all -- --check`.
  - Audit IDs updated: G1, G2, N1 verified.
  - Next: Checkpoint 2 — prune active `approval_required_modes` config/storage/control-plane surface.

- 2026-06-08: Checkpoint 2 implemented
  - Changed: removed active `approval_required_modes` config/API/storage surface from topic infra records/options, SQLx row mapping/upserts, manager-control-plane infra/provision tools, tests, README, and stale PRD schema snippet.
  - Evidence: targeted `rg` for `approval_required_modes|approval required|approval-required` across `crates/oxide-agent-core docs config profiles README.md` only matches this goal doc; SQLx legacy column is ignored by code and kept compatible through existing column default plus serde unknown-field tolerance for old JSON snapshots.
  - Commands: `cargo check -p oxide-agent-core --no-default-features --features manager-control-plane,integration-ssh-mcp,storage-sqlx`; `cargo clippy -p oxide-agent-core --no-default-features --features manager-control-plane,integration-ssh-mcp,storage-sqlx --all-targets -- -D warnings`; `cargo fmt --all -- --check`.
  - Audit IDs updated: G3 verified; Q2 evidence added.
  - Next: Checkpoint 3 — remove confirmed lint blockers and low-risk dead code from RECON.

## Risks and Blockers

- Storage compatibility for `approval_required_modes`
  - Impact: deleting a field without checking serde/storage use could break old topic infra records or tests.
  - Evidence: resolved in Checkpoint 2 by removing the Rust field and SQL read/write bindings while leaving the old SQLx column ignored for existing databases.
  - Mitigation: no active mitigation needed unless a future migration intentionally drops the legacy column.
  - Audit IDs affected: G3 verified.

- Clippy may reveal additional failures after first blockers are fixed
  - Impact: `-D warnings` can stop early and mask later issues.
  - Evidence: current RECON clippy already stops at first failure sets.
  - Mitigation: rerun targeted clippy after each checkpoint and record newly surfaced issues.
  - Audit IDs affected: G4, V1.

## Final Verification

Filled only when complete.
