# Goal: Non-SSH RECON cleanup

Date started: 2026-06-08
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-08-non-ssh-recon-cleanup.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update the doc after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: user request after RECON
Goal doc owner: Codex
Last updated: 2026-06-08

## Objective

Clean up confirmed non-SSH RECON findings that are stale, misleading, or dead, without broad refactors or new dependencies.

Done when every required Completion Audit item is verified by its listed evidence and all out-of-scope constraints are preserved.

## Scope

In scope:
- Profile/config/docs drift found by RECON: embedded sandbox backend mismatch, stale SSH provider wording, and missing compiled module entries in `profiles/*.toml`.
- Telegram callback surface that is parsed/routed but no longer emitted by current keyboards.
- Goal documentation and validation evidence for each checkpoint.

Out of scope:
- Reintroducing SSH approval or changing YOLO SSH behavior.
- Broad provider module feature-gating refactors.
- Web UI legacy CSS cleanup.
- WebFetch/Crawl4AI deduplication.
- Historical PRD/goal rewrites unless an active doc is misleading current users.

## Missing Inputs

None. User approved continuing RECON-cleanup as a separate phase.

## Repository Context

- Relevant entry points: `crates/oxide-agent-core/Cargo.toml`, `profiles/*.toml`, `README.md`, `docker-compose.telegram.yml`, `crates/oxide-agent-transport-telegram/src/bot/`.
- Existing conventions: default branch `dev`, no new dependencies unless required, smallest maintainable changes, `cargo fmt --all -- --check` and scoped clippy/check validation.
- Risky areas: profile TOMLs are module config examples; compiled modules are enabled by default but configured non-compiled module IDs fail validation.

## Completion Audit

- G1: Embedded Telegram compose sandbox backend is compile/runtime consistent
  - Source: RECON finding that `docker-compose.telegram.yml` uses `SANDBOX_BACKEND=broker` while embedded profile lacks `sandbox-backend-sandboxd-client`.
  - Acceptance: `profile-embedded-opencode-local` compiles the sandboxd client module used by `SANDBOX_BACKEND=broker`, and its profile TOML lists the client module instead of stale daemon-only wiring.
  - Evidence required: `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local` passes; targeted `rg` confirms embedded profile includes `sandbox-backend/sandboxd-client` and no longer includes `sandbox-daemon/sandboxd`.
  - Status: verified
  - Evidence collected: 2026-06-08 Checkpoint 1 added `sandbox-backend-sandboxd-client` to `profile-embedded-opencode-local`, replaced the embedded profile TOML daemon entry with `sandbox-backend/sandboxd-client`, and kept `SANDBOX_BACKEND=broker` aligned with the existing `sandboxd` service/socket compose wiring. Embedded core and bot cargo checks passed.

- G2: Active docs no longer describe SSH approval as current behavior
  - Source: RECON finding at `README.md:520`.
  - Acceptance: active README wording describes SSH MCP as YOLO/full-permission infrastructure, not an approval flow.
  - Evidence required: `rg -n "approval flow" README.md` returns no matches.
  - Status: verified
  - Evidence collected: 2026-06-08 Checkpoint 1 changed active README provider text to YOLO full-permission mode. `rg -n "approval flow" README.md` returned no matches.

- G3: Profile TOMLs list compiled Brave Search module where corresponding Cargo profiles compile it
  - Source: RECON profile TOML drift for `tool-brave-search`.
  - Acceptance: `profiles/full.toml`, `profiles/embedded-opencode-local.toml`, `profiles/search-only.toml`, and `profiles/host-bwrap.toml` include `"tool/brave-search" = { enabled = true }`.
  - Evidence required: targeted `rg`/diff review and profile check command pass.
  - Status: verified
  - Evidence collected: 2026-06-08 Checkpoint 1 added `"tool/brave-search" = { enabled = true }` to `profiles/full.toml`, `profiles/embedded-opencode-local.toml`, `profiles/search-only.toml`, and `profiles/host-bwrap.toml`.

- G4: Dead Telegram callback surface removed
  - Source: RECON finding that un-emitted callback/menu actions remain parsed and routed.
  - Acceptance: unused `agent:clear`, `agent:compact`, `agent:recreate`, `agent:exit`, `menu:agent`, `menu:clear`, and `menu:back` parser/routing surface is removed or explicitly justified for compatibility.
  - Evidence required: Telegram transport check/clippy/test compile and targeted callback `rg`.
  - Status: in_progress
  - Evidence collected: 2026-06-08 Checkpoint 1 changed only profile/config/docs target files plus this goal doc; no dependencies, services, or abstractions were added.

- Q1: No broad refactors or new dependencies
  - Source: AGENTS.md implementation bias.
  - Acceptance: changes are local to documented cleanup targets; no new crates, services, or abstractions.
  - Evidence required: diff/Cargo review.
  - Status: in_progress
  - Evidence collected: 2026-06-08 Checkpoint 1 passed embedded core/bot checks, targeted `rg`, and `cargo fmt --all -- --check`.

- V1: Validation completed for affected areas
  - Source: AGENTS.md validation guidance.
  - Acceptance: checkpoint-specific check/clippy/fmt commands pass before completion.
  - Evidence required: Progress Log records exact commands.
  - Status: pending
  - Evidence collected:

## Implementation Plan

### Checkpoint 1: profile/config/docs correctness
- Audit IDs: G1, G2, G3, Q1, V1
- Expected changes: add embedded sandboxd client feature/module, remove stale embedded daemon module entry, fix README SSH wording, add missing Brave Search module entries to profile TOMLs.
- Validation:
  - `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local`
  - `cargo check -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local`
  - `cargo fmt --all -- --check`
  - targeted `rg` checks for stale profile/doc strings.
- Exit condition: profile/config/docs drift is fixed and committed with goal evidence.

### Checkpoint 2: Telegram callback dead-code cleanup
- Audit IDs: G4, Q1, V1
- Expected changes: remove un-emitted callback constants/parser arms/menu handler chain, simplify ignored inline keyboard function args, update tests.
- Validation:
  - `cargo check -p oxide-agent-transport-telegram --no-default-features --features profile-full`
  - `cargo clippy -p oxide-agent-transport-telegram --no-default-features --features profile-full --all-targets -- -D warnings`
  - `cargo test -p oxide-agent-transport-telegram --no-default-features --features profile-full --no-run`
  - targeted callback `rg`.
- Exit condition: dead callback surface is removed or remaining compatibility surface is documented with evidence.

### Checkpoint 3: final validation and audit
- Audit IDs: G1, G2, G3, G4, Q1, V1
- Expected changes: update this goal doc with final evidence and completion status.
- Validation: repeat affected checks and `cargo fmt --all -- --check`.
- Exit condition: every Completion Audit item is verified or explicitly blocked with evidence.

## Validation Contract

- Static checks:
  - `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local`
  - `cargo check -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local`
  - `cargo check -p oxide-agent-transport-telegram --no-default-features --features profile-full`
- Lint:
  - `cargo clippy -p oxide-agent-transport-telegram --no-default-features --features profile-full --all-targets -- -D warnings`
- Tests:
  - `cargo test -p oxide-agent-transport-telegram --no-default-features --features profile-full --no-run`
- Formatting: `cargo fmt --all -- --check`
- Artifact verification: targeted `rg` searches for stale profile/doc/callback strings.
- Done when: all audit IDs are verified and the goal doc contains current evidence.

## Decisions

- 2026-06-08: Keep this goal separate from the completed SSH cleanup goal. Reason: remaining findings are non-SSH profile/docs and Telegram callback cleanup.
- 2026-06-08: Fix embedded compose by compiling the broker client instead of changing `SANDBOX_BACKEND=broker`. Reason: compose already runs `sandboxd` and mounts its socket.

## Progress Log

- 2026-06-08: goal doc created
  - Changed: `docs/goals/2026-06-08-non-ssh-recon-cleanup.md`
  - Evidence: RECON identified low-risk follow-up checkpoints for profile/config/docs and Telegram callback cleanup.
  - Commands: `git status --short`, `git log --oneline -5`, targeted reads of affected files.
  - Audit IDs updated: none yet.
  - Next: Checkpoint 1 — profile/config/docs correctness.

- 2026-06-08: Checkpoint 1 implemented
  - Changed: embedded Cargo profile now compiles `sandbox-backend-sandboxd-client`; embedded profile TOML uses `sandbox-backend/sandboxd-client` instead of stale `sandbox-daemon/sandboxd`; Brave Search module entries were added to profile TOMLs whose Cargo profiles already compile it; README SSH wording now says YOLO full-permission mode.
  - Evidence: targeted profile/doc `rg` showed no `approval flow` in README, embedded profile has `sandbox-backend/sandboxd-client`, and Brave Search entries exist in the four target profile TOMLs.
  - Commands: `cargo check -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local`; `cargo check -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-embedded-opencode-local`; `cargo fmt --all -- --check`; targeted `rg` checks for profile/doc strings.
  - Audit IDs updated: G1, G2, G3 verified; Q1 and V1 evidence added.
  - Next: Checkpoint 2 — Telegram callback dead-code cleanup.

## Risks and Blockers

- Old persisted Telegram inline keyboards may contain removed callback data after Checkpoint 2.
  - Impact: tapping old buttons may no-op instead of executing stale actions.
  - Evidence: current keyboards no longer emit those callback strings.
  - Mitigation: acceptable if handler falls through without error; document exact behavior during Checkpoint 2.
  - Audit IDs affected: G4.

## Final Verification

Filled only when complete.
