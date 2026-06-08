# Goal: WebFetch Markdown Provider Module Slice

Date started: 2026-06-08
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-08-webfetch-md-module-slice.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: User request to split `webfetch_md.rs` (1312 lines) into a modular directory with domain-focused slices.
Goal doc owner: Codex
Last updated: 2026-06-08

## Objective

Split the monolithic `crates/oxide-agent-core/src/agent/providers/webfetch_md.rs` (1312 lines) into a `webfetch_md/` directory with domain-focused submodules, each 30-180 lines. Preserve the public API (`WebFetchMdProvider`), all existing behavior, and all tests. No functional changes.

Done when the single file is replaced by the directory, `cargo check -p oxide-agent-core` passes, all existing webfetch tests pass, and `providers/mod.rs` requires no manual changes beyond automatic directory-module resolution.

## Scope

In scope:
- `crates/oxide-agent-core/src/agent/providers/webfetch_md.rs` -- delete after split.
- `crates/oxide-agent-core/src/agent/providers/webfetch_md/` -- new directory.
- `crates/oxide-agent-core/src/agent/providers/mod.rs` -- automatic resolution only (line 40 `pub mod webfetch_md;` resolves to directory `mod.rs` with zero changes needed).

Out of scope:
- Any behavioral change to webfetch tool logic.
- Changes to any other provider, transport, runner, or web crate.
- New dependencies, abstractions, or traits.
- Direct Google Gemini provider work.

## Missing Inputs

None. The plan was reviewed and approved by the user before goal creation.

## Repository Context

- Target file: `crates/oxide-agent-core/src/agent/providers/webfetch_md.rs` (1312 lines).
- Feature gate: `#[cfg(feature = "tool-webfetch-md")]` on `pub mod webfetch_md` in `providers/mod.rs:40`.
- Public exports: `WebFetchMdProvider` (struct + `new()`, `Default`, `tool_runtime_executors()`), re-exported at `providers/mod.rs:111`.
- External references: `tool_runtime/modules.rs` (line 728-730) registers the provider; `manager_control_plane/agent_controls.rs` has string literal `"webfetch_md"`. None import internal types. Zero blast radius outside `providers/mod.rs`.
- Validation: `cargo check -p oxide-agent-core`, `cargo test -p oxide-agent-core -- webfetch`.
- Existing convention: `manager_control_plane/`, `crawl4ai_markdown/`, `brave_search/`, `silero_tts/`, `duckduckgo/`, `searxng/` are already directory-modules inside `providers/`.
- Test infrastructure: ~478 lines of tests inside `#[cfg(test)] mod tests` at line 834. Covers: runtime executor, HTML conversion, URL validation, SSRF, anti-bot detection, truncation, structured failure payload, Reddit RSS parsing/rendering.

## Completion Audit

- G1: Monolithic file replaced by directory with all domain slices
  - Source: user request, plan approved
  - Acceptance: `webfetch_md.rs` deleted; `webfetch_md/` directory exists with `mod.rs`, `fetch.rs`, `url.rs`, `detect.rs`, `error.rs`, `convert.rs`, `reddit.rs`, `tests.rs`
  - Evidence required: `ls -la` showing directory contents, `wc -l` per file
  - Status: pending
  - Evidence collected:

- G2: Public API preserved
  - Source: `providers/mod.rs:111`
  - Acceptance: `pub use webfetch_md::WebFetchMdProvider;` compiles without changes
  - Evidence required: `cargo check -p oxide-agent-core` passes
  - Status: pending
  - Evidence collected:

- G3: All tests pass
  - Source: existing test suite (line 834-1312)
  - Acceptance: All `webfetch` tests pass from new module layout
  - Evidence required: `cargo test -p oxide-agent-core -- webfetch` passes
  - Status: pending
  - Evidence collected:

- Q1: Each slice under 180 lines (tests.rs excluded)
  - Source: plan agreement
  - Acceptance: `wc -l` per file shows <=180 for non-test slices; `tests.rs` exempt
  - Evidence required: line counts
  - Status: pending
  - Evidence collected:

- Q2: No new dependencies or traits
  - Source: project conventions
  - Acceptance: `Cargo.toml` unchanged
  - Evidence required: `git diff` shows no Cargo.toml changes
  - Status: pending
  - Evidence collected:

- N1: No behavioral changes
  - Source: user request
  - Must preserve: identical tool output, error messages, URL validation, Reddit RSS rendering
  - Evidence required: `git diff --stat` shows only move/rename + visibility adjustments
  - Status: pending
  - Evidence collected:

## Implementation Plan

### Checkpoint 0: folder-ize
- Audit IDs: G1, G2
- Expected changes: `mkdir webfetch_md/`, `mv webfetch_md.rs webfetch_md/mod.rs`, zero code changes
- Validation: `cargo check -p oxide-agent-core`
- Exit condition: compiles, all tests pass

### Checkpoint 1: extract reddit.rs (~160 lines)
- Audit IDs: G1, Q1
- Expected changes: move Reddit RSS types and functions (lines 675-832) to `reddit.rs`, add `mod reddit;` to `mod.rs`, adjust visibility to `pub(super)`
- Validation: `cargo check -p oxide-agent-core`, `cargo test -p oxide-agent-core -- webfetch`
- Exit condition: compiles, tests pass

### Checkpoint 2: extract url.rs (~90 lines)
- Audit IDs: G1, Q1
- Expected changes: move URL validation and SSRF functions (lines 385-475) to `url.rs`, `pub(super)` visibility
- Validation: `cargo check -p oxide-agent-core`, `cargo test -p oxide-agent-core -- webfetch`
- Exit condition: compiles, tests pass

### Checkpoint 3: extract error.rs (~105 lines)
- Audit IDs: G1, Q1
- Expected changes: move error reporting functions (lines 538-641) to `error.rs`, `pub(super)` visibility
- Validation: `cargo check -p oxide-agent-core`, `cargo test -p oxide-agent-core -- webfetch`
- Exit condition: compiles, tests pass

### Checkpoint 4: extract convert.rs (~35 lines)
- Audit IDs: G1, Q1
- Expected changes: move `html_to_markdown()`, `TruncatedOutput`, `truncate_chars()` (lines 643-673) to `convert.rs`, `pub(super)` visibility
- Validation: `cargo check -p oxide-agent-core`, `cargo test -p oxide-agent-core -- webfetch`
- Exit condition: compiles, tests pass

### Checkpoint 5: extract detect.rs (~60 lines)
- Audit IDs: G1, Q1
- Expected changes: move content-type checks and anti-bot detection (lines 477-536) to `detect.rs`, `pub(super)` visibility
- Validation: `cargo check -p oxide-agent-core`, `cargo test -p oxide-agent-core -- webfetch`
- Exit condition: compiles, tests pass

### Checkpoint 6: extract fetch.rs (~130 lines)
- Audit IDs: G1, Q1
- Expected changes: move `fetch_markdown()`, `fetch_reddit_rss()`, `fetch_text()`, `read_limited_body()` to `fetch.rs`, `pub(super)` visibility. Provider impl in `mod.rs` delegates to `fetch::` functions.
- Validation: `cargo check -p oxide-agent-core`, `cargo test -p oxide-agent-core -- webfetch`
- Exit condition: compiles, tests pass

### Checkpoint 7: cleanup mod.rs + extract tests.rs
- Audit IDs: G1, G3, Q1
- Expected changes: move `#[cfg(test)] mod tests` to `tests.rs` file, update imports in tests (`use super::*;` + sub-module imports), verify `mod.rs` contains only provider struct, executor, constants, arg parsing
- Validation: `cargo check -p oxide-agent-core`, `cargo test -p oxide-agent-core -- webfetch`
- Exit condition: compiles, all tests pass, line counts within budget

## Validation Contract

- Static checks: `cargo check -p oxide-agent-core`
- Tests: `cargo test -p oxide-agent-core -- webfetch`
- Artifact verification: `ls -la` and `wc -l` on all slice files
- Done when: all audit items verified, no behavioral diff

## Decisions

- 2026-06-08: Tests as single `tests.rs` file (not `tests/mod.rs`) per user preference
- 2026-06-08: All internal items `pub(super)` visibility -- zero external API change
- 2026-06-08: Extraction order by dependency -- leaf modules first (url, error, convert, detect), then reddit, then fetch (depends on all), then tests last
- 2026-06-08: Checkpoints 1 and 4 merged -- reddit.rs depends on html_to_markdown from convert.rs, so both extracted together

## Progress Log

- 2026-06-08: Goal document created, plan approved by user. Ready for implementation.
  - Changed: goal doc created
  - Evidence: file exists at `docs/goals/2026-06-08-webfetch-md-module-slice.md`
  - Commands: none yet
  - Audit IDs updated: none
  - Next: Checkpoint 0 (folder-ize)

- 2026-06-08: Checkpoints 0-1 complete
  - Changed: folder-ized `webfetch_md.rs` → `webfetch_md/mod.rs`; extracted `convert.rs` (33 lines) and `reddit.rs` (158 lines)
  - Evidence: `cargo check` clean, 19 webfetch tests pass, zero warnings
  - Commands: `cargo check -p oxide-agent-core`, `cargo test -p oxide-agent-core --no-default-features --features profile-lite`
  - Audit IDs updated: G1(partial), G2(partial)
  - Decisions: extracted `convert.rs` together with `reddit.rs` (checkpoint 1+4 merged) because reddit.rs depends on `html_to_markdown` from convert.rs; added `xml_tag_text` to mod.rs top-level imports since `fetch_reddit_rss()` in the provider impl calls it directly
  - Line counts: mod.rs=1139, convert.rs=33, reddit.rs=158
  - Next: Checkpoint 2 (extract url.rs)

## Risks and Blockers

None identified. The refactoring is purely internal with zero blast radius.

## Final Verification

Filled only when complete.
