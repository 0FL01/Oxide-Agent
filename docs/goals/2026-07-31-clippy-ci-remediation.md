# Goal: Restore the workspace Clippy CI gate

Status: active
Source: User instruction and `.github/workflows/ci.yml` Clippy job
Last updated: 2026-07-31

## Objective

Make the exact CI Clippy command pass on the current stable Rust toolchain, record that command as a pre-commit gate in `AGENTS.md`, then commit and push the complete fix.

## Execution Directive

Complete the frozen Required Outcomes using the listed Change Envelope and Primary Evidence. Work on the smallest unresolved outcome. Do not add requirements from reviews, tests, tools, speculative risks, or optional source text. Finish when every required outcome is resolved and affected constraints remain satisfied.

## Frozen Contract

### Required Outcomes

- R1: The workspace Clippy CI gate passes.
  - Source: User-provided CI failures and explicit command.
  - Acceptance: `cargo clippy --workspace --all-targets --all-features -- -D warnings` exits successfully on stable Rust 1.97.
  - Primary evidence: The exact command's successful local exit.
  - Status: verified
  - Evidence: `cargo clippy --workspace --all-targets --all-features -- -D warnings` passed on Rust 1.97.1 after the final source edit.
- R2: Repository instructions prohibit commits before the exact Clippy gate passes.
  - Source: Explicit user instruction.
  - Acceptance: `AGENTS.md` names the exact command and says not to commit until it passes.
  - Primary evidence: Diff review of `AGENTS.md`.
  - Status: verified
  - Evidence: `AGENTS.md` Format and lint directive updated on 2026-07-31.
- R3: The verified changes are committed and pushed.
  - Source: Explicit user instruction.
  - Acceptance: One commit containing the complete remediation is present on `origin/main`.
  - Primary evidence: Successful `git push origin main` and clean synchronized status.
  - Status: pending
  - Evidence:

### Constraints

- C1: Do not commit before the exact R1 command passes.
- C2: Preserve behavior; lint remediations are syntax-level equivalents.
- C3: `cargo fmt --all -- --check` must pass before commit.

### Non-goals

- Refactoring unrelated runtime paths.
- Adding dependencies, compatibility paths, migrations, or new tests for syntax-only lint fixes.

## Change Envelope

- Target: Rust 1.97 Clippy findings reachable from the exact workspace CI command.
- Expected paths, symbols, and direct consumers: reported lint sites in web and Telegram transports; additional sites only when surfaced by the same gate; `AGENTS.md`; this goal document.
- Allowed and forbidden artifacts: existing Rust source and repository guidance may change; no dependencies, schemas, configuration surfaces, services, or abstractions.
- User or harness budget: the exact gate must pass before commit; iterate on concrete findings from that gate only.

## Current Checkpoint

- Closes: R3.
- Smallest next action: review the bounded diff, commit it, and push `main`.
- Expected evidence: successful push and synchronized clean status.
- Stop or replan if: review finds an out-of-envelope change or the remote rejects the push.

## Current State

- Resolved: R1 and R2; RECON, lint remediation, and validation are complete.
- Last relevant evidence: The exact Rust 1.97.1 Clippy gate and `cargo fmt --all -- --check` pass; the required web UI wasm check also passes.
- Blocker: None.
- Next: Review, commit, and push.

## Material Decisions

- 2026-07-31: Use the valid CI spelling `--workspace` for the user's requested pre-commit command.
- 2026-07-31: Limit iteration to diagnostics produced by the exact mandatory gate.

## Checkpoint History

- 2026-07-31: RECON confirmed `.github/workflows/ci.yml` runs the exact all-features gate and local Rust 1.94 could not detect Rust 1.97 lints; updated the local stable toolchain to 1.97.1.
- 2026-07-31: Fixed the eight user-reported diagnostics; the exact gate progressed through the transports and surfaced one additional web UI sort diagnostic.
- 2026-07-31: Fixed the final web UI diagnostic; the exact Clippy gate, formatter check, and web UI wasm check passed.

## Completion

- Resolved outcomes:
- Commands and artifacts:
- Constraint and diff-scope check:
- Final status: active
