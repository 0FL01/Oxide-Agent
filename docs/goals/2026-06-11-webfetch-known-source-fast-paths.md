# Goal: WebFetch Known Source Fast Paths

Date started: 2026-06-11
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-11-webfetch-known-source-fast-paths.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update the doc after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: User request to extend `webfetch_md` fast paths after `a53799f4 feat(webfetch): add fast README fetch paths`
Goal doc owner: Codex
Last updated: 2026-06-12 00:55

## Objective

Extend the lightweight `web_markdown` fast-path source handling so the agent can fetch high-value project/package pages without browser rendering for GitLab, Gitea/Forgejo/Codeberg, Rust package pages (`crates.io` and `docs.rs`), and PyPI package pages.

Done when every required Completion Audit item is verified by its listed evidence and all out-of-scope constraints are preserved.

## Scope

In scope:
- `crates/oxide-agent-core/src/agent/providers/webfetch_md/fetch.rs` -- keep dispatch and fetch orchestration small.
- `crates/oxide-agent-core/src/agent/providers/webfetch_md/known_sources/` -- expected directory-module for known-source handling:
  - `mod.rs` -- shared source-plan enum, `classify()` entry point, and small shared helpers only.
  - `repo_hosts.rs` -- direct README rewrites for GitHub, HuggingFace, GitLab, Gitea/Forgejo/Codeberg.
  - `rust_packages.rs` -- `crates.io` and `docs.rs` URL classification, minimal crates.io JSON parsing/render helpers.
  - `pypi.rs` -- PyPI project URL classification, minimal PyPI JSON parsing/render helpers.
- `crates/oxide-agent-core/src/agent/providers/webfetch_md/mod.rs` -- module declaration only if `known_sources/` is added.
- `crates/oxide-agent-core/src/agent/providers/webfetch_md/tests.rs` -- mapping and minimal local-server tests for API-backed sources.
- `crates/oxide-agent-core/src/agent/prompt/composer.rs` -- only if tool guidance needs a small wording update for the new source set.

Out of scope:
- No new crates or services.
- No browser automation, crawling, caching, indexing, package search, or recursive repository traversal.
- No direct Google Gemini provider work.
- No changes to Crawl4AI behavior except preserving fallback compatibility through existing tool choice guidance.
- No broad generic forge autodetection for arbitrary root URLs; only explicit safe patterns and known hosts.

## Missing Inputs

None. Low-risk defaults are recorded in Decisions and can be revised before implementation review.

## Repository Context

- Current fast-path dispatch lives in `crates/oxide-agent-core/src/agent/providers/webfetch_md/fetch.rs:47`.
- Current known-source mapping for GitHub and HuggingFace lives in `crates/oxide-agent-core/src/agent/providers/webfetch_md/known_sources/repo_hosts.rs` after Checkpoint 1.
- Current known-source tests start at `crates/oxide-agent-core/src/agent/providers/webfetch_md/tests.rs:481`.
- `web_markdown` and `crawl4ai_markdown` are both enabled in the web compose profile after `docker-compose.web.yml:66`.
- Existing validation commands for this area:
  - `cargo fmt --all -- --check`
  - `cargo test -p oxide-agent-core --no-default-features --features tool-webfetch-md --lib webfetch_md`
  - `cargo check -p oxide-agent-core --no-default-features --features "tool-webfetch-md tool-crawl4ai-markdown"`
  - `cargo clippy -p oxide-agent-core --no-default-features --features tool-webfetch-md --all-targets -- -D warnings`
- Workspace-wide `cargo clippy --workspace --all-targets -- -D warnings` currently has unrelated feature-gated Telegram test failures; targeted clippy is the relevant gate for this goal unless those failures are fixed separately.

## Completion Audit

- G1: Known-source logic is sliced by domain
  - Source: user-approved slicing review after Checkpoint 0
  - Acceptance: `fetch.rs` contains no host-specific matchers except calling `known_sources::classify()` and executing returned plans; direct repository hosts live in `known_sources/repo_hosts.rs`; `crates.io`/`docs.rs` logic lives in `known_sources/rust_packages.rs`; PyPI logic lives in `known_sources/pypi.rs`; no single known-source slice exceeds ~220 lines excluding tests unless justified in Decisions
  - Evidence required: code inspection with file paths and line ranges; `cargo check` passes
  - Status: verified
  - Evidence collected: Checkpoint 1 moved existing GitHub/HuggingFace matching to `known_sources/repo_hosts.rs`; `fetch.rs` now calls `known_sources::classify()`. Checkpoint 3 added `known_sources/rust_packages.rs` for `crates.io`/`docs.rs`. Checkpoint 4 added `known_sources/pypi.rs` for PyPI classification, JSON parsing, and rendering; `pypi.rs` is 163 lines and no known-source slice exceeds the goal limit.

- G2: GitLab fast path
  - Source: user request: "gitlab"
  - Acceptance: `gitlab.com/<group...>/<project>` maps to `/-/raw/HEAD/README.md`; `/-/blob/<branch>/.../README.md` maps to `/-/raw/<branch>/.../README.md`; nested groups are supported; non-README pages are ignored
  - Evidence required: unit tests for root, nested group, blob, and negative cases; targeted webfetch tests pass
  - Status: verified
  - Evidence collected: Checkpoint 2 added GitLab classification in `known_sources/repo_hosts.rs:93-130` and tests for root, nested group root, blob README, and non-README negatives in `tests.rs:535-568` and `tests.rs:614-615`; targeted webfetch tests passed 34/34.

- G3: Gitea/Forgejo/Codeberg fast path
  - Source: user request: "Gitea / Forgejo / Codeberg"
  - Acceptance: known hosts (`codeberg.org`, `gitea.com`, and any explicitly chosen Forgejo/Gitea host) map root `owner/repo` to `/raw/branch/HEAD/README.md`; explicit `/src/branch/<branch>/.../README.md` maps to `/raw/branch/<branch>/.../README.md`; generic self-hosted support is limited to explicit `/src/branch/.../README.md` patterns
  - Evidence required: unit tests for Codeberg root, Codeberg `src/branch`, generic self-hosted `src/branch`, and negative cases; targeted webfetch tests pass
  - Status: verified
  - Evidence collected: Checkpoint 2 added known-host root and explicit `src/branch` rewrites in `known_sources/repo_hosts.rs:132-163`; tests cover Codeberg root, Gitea `src/branch`, generic self-hosted `src/branch`, arbitrary-root negative, and non-README negatives in `tests.rs:571-607` and `tests.rs:616-619`; targeted webfetch tests passed 34/34.

- G4: Rust package fast path through `crates.io`
  - Source: user request: "Rust"
  - Acceptance: `https://crates.io/crates/<crate>` fetches `https://crates.io/api/v1/crates/<crate>`, extracts a concrete version, then fetches `https://crates.io/api/v1/crates/<crate>/<version>/readme`; output identifies the original source URL, selected version, mode, and README content
  - Evidence required: mapping/unit tests plus local-server async test for metadata + README flow; targeted webfetch tests pass
  - Status: verified
  - Evidence collected: Checkpoint 3 added `CrateReadme` source plans in `known_sources/mod.rs:14-20`, crates.io classification and minimal metadata parsing/rendering in `known_sources/rust_packages.rs:7-162`, and fetch orchestration in `fetch.rs:121-199`; tests cover mapping and local metadata+README server flow in `tests.rs:630-706`; targeted webfetch tests passed 37/37.

- G5: `docs.rs` fast path through `crates.io` README API
  - Source: user request: "docs.rs"
  - Acceptance: common docs.rs URLs (`/crate/<crate>/<version>`, `/<crate>`, `/<crate>/latest/...`, `/<crate>/<version>/...`) resolve to the same crates.io README flow; explicit non-latest version skips version discovery when safe; package docs pages remain source URL in output
  - Evidence required: mapping tests for URL variants and local-server coverage shared with G4; targeted webfetch tests pass
  - Status: verified
  - Evidence collected: Checkpoint 3 added docs.rs URL classification for root, `latest`, explicit version, and `/crate/<crate>/<version>` routes in `known_sources/rust_packages.rs:89-108`; mapping tests assert version discovery vs explicit version handling in `tests.rs:644-667`; local-server coverage is shared with the crates.io README flow; targeted webfetch tests passed 37/37.

- G6: PyPI package fast path
  - Source: user request: "PyPI"
  - Acceptance: `https://pypi.org/project/<package>/` fetches `https://pypi.org/pypi/<package>/json`, renders `info.description` plus key metadata as Markdown, and falls back to normal fetch when JSON parse or description is unusable
  - Evidence required: mapping/unit tests plus local-server async test for JSON-to-Markdown rendering; targeted webfetch tests pass
  - Status: verified
  - Evidence collected: Checkpoint 4 added `PypiProject` source plans in `known_sources/mod.rs:22-27`, PyPI URL classification and minimal JSON parsing/rendering in `known_sources/pypi.rs:7-163`, and fetch orchestration in `fetch.rs:123-134` and `fetch.rs:212-242`; tests cover mapping and local JSON-to-Markdown rendering in `tests.rs:669-681` and `tests.rs:722-763`; targeted webfetch tests passed 39/39.

- Q1: Preserve lightweight fallback behavior
  - Source: project principle and user goal to speed fetch tools without losing Crawl4AI fallback
  - Acceptance: every fast path failure logs a warning and falls back to the original URL fetch; normal `web_markdown` anti-bot failure behavior remains unchanged
  - Evidence required: code inspection and existing anti-bot tests still pass
  - Status: in_progress
  - Evidence collected: Checkpoint 1 preserved existing fallback flow; Checkpoint 2 only extended known-source classification; Checkpoint 3 keeps the existing known-source failure warning/fallback in `fetch.rs:42-58` and routes crates/docs failures through that same path. Checkpoint 4 routes PyPI JSON failures through the same warning/fallback path; targeted webfetch tests passed 39/39.

- Q2: No new dependencies or over-engineering
  - Source: `AGENTS.md` project rules
  - Acceptance: no `Cargo.toml` changes; no new services/caches/queues; JSON parsing uses existing `serde_json`
  - Evidence required: `git diff -- Cargo.toml` empty; code inspection
  - Status: in_progress
  - Evidence collected: Checkpoint 1 added only local Rust modules; Checkpoint 2 changed only `known_sources/repo_hosts.rs`, tests, and this goal doc; Checkpoint 3 used existing `serde_json` and added no dependency changes. Checkpoint 4 added only local Rust code/tests/docs; `git diff -- Cargo.toml` is empty.

- Q3: Output remains agent-readable and source-transparent
  - Source: previous implementation contract from `a53799f4`
  - Acceptance: fast-path outputs include enough metadata to identify `URL`, `Source-URL`, `Mode`, and source-specific metadata such as version where relevant
  - Evidence required: async tests assert output fields for crates.io/PyPI and code inspection for direct README paths
  - Status: in_progress
  - Evidence collected: Checkpoint 3 crate README output includes `URL`, `Source-URL`, `Mode`, `Crate`, `Version`, `Content-Type`, `Fetched-Bytes`, and `Truncated`; async test asserts source URL, mode, crate, version, and README content in `tests.rs:713-719`. Checkpoint 4 PyPI output includes `URL`, `Source-URL`, `Mode`, `Package`, `Version`, `Description-Content-Type`, `Content-Type`, `Fetched-Bytes`, `Truncated`, optional summary/project URL, and description; async test asserts these fields in `tests.rs:754-762`.

- Q4: Slice boundaries stay locally understandable
  - Source: project anti-overengineering rules and user concern about one-file "каша"
  - Acceptance: no generic provider trait, registry framework, macros, or router abstraction; each known-source module exposes small plain functions and focused data structs/enums only when needed by `fetch.rs`
  - Evidence required: code inspection and absence of new dependencies/framework-style abstractions
  - Status: in_progress
  - Evidence collected: Checkpoint 1 introduced plain `known_sources/mod.rs` and `known_sources/repo_hosts.rs`; Checkpoint 3 added one focused `rust_packages.rs` slice and a simple enum variant. Checkpoint 4 added one focused `pypi.rs` slice and one enum variant, with no traits, macros, registry framework, or new dependency.

- V1: Required validation passes
  - Source: repo validation conventions and previous webfetch checkpoint
  - Acceptance: listed validation commands pass, except documented unrelated workspace-wide clippy failures if re-run
  - Evidence required: command outputs summarized in Progress Log and Final Verification
  - Status: in_progress
  - Evidence collected: Checkpoint 4 validation passed: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-webfetch-md --lib webfetch_md`; `cargo check -p oxide-agent-core --no-default-features --features "tool-webfetch-md tool-crawl4ai-markdown"`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-webfetch-md --all-targets -- -D warnings`; `git diff -- Cargo.toml` empty; `git diff --check` clean.

- N1: No broad arbitrary-host root guessing
  - Source: safety boundary in user-reviewed plan
  - Must preserve: root URL fast paths only for known hosts; arbitrary hosts only get explicit, highly specific raw/blob pattern rewrites when safe
  - Evidence required: negative tests and code inspection
  - Status: verified
  - Evidence collected: Checkpoint 2 limits root rewrites to `codeberg.org` and `gitea.com` while allowing arbitrary hosts only for explicit `/owner/repo/src/branch/<branch>/.../README.md`; negative tests include `https://git.example.test/owner/repo` and non-README explicit paths in `tests.rs:618-619`.

## Implementation Plan

### Checkpoint 0: goal contract and review gate
- Audit IDs: planning only
- Expected changes: create this goal document, commit it, stop for user review before implementation
- Validation: `git diff --check`, inspect doc diff
- Exit condition: committed goal doc with first implementation checkpoint clearly identified

### Checkpoint 1: create known_sources directory and move existing GitHub/HuggingFace
- Audit IDs: G1, Q1, Q2, Q4, N1
- Expected changes: add `known_sources/mod.rs` and `known_sources/repo_hosts.rs`; move existing GitHub/HuggingFace classifier from `fetch.rs`; make `fetch.rs` call `known_sources::classify()`; keep behavior identical; update imports/tests only as needed; do not add new source support yet
- Validation: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-webfetch-md --lib webfetch_md`; `cargo check -p oxide-agent-core --no-default-features --features "tool-webfetch-md tool-crawl4ai-markdown"`
- Exit condition: GitHub/HuggingFace tests still pass and `fetch.rs` is smaller/cleaner before adding new sources

### Checkpoint 2: direct forge README fast paths
- Audit IDs: G2, G3, Q1, Q2, N1
- Expected changes: add GitLab, Codeberg/Gitea/Forgejo direct README/source rewrites and tests in `known_sources/repo_hosts.rs`
- Validation: targeted webfetch tests, targeted check, targeted clippy
- Exit condition: direct forge mapping and negative tests pass

### Checkpoint 3: crates.io and docs.rs API-backed README fast paths
- Audit IDs: G4, G5, Q1, Q2, Q3
- Expected changes: implement `known_sources/rust_packages.rs`; introduce source plan variant for crates.io README flow; parse minimal JSON metadata; render source-transparent output
- Validation: mapping tests, local-server async test, targeted webfetch tests/check/clippy
- Exit condition: crates.io/docs.rs flows are verified without live network dependency in tests

### Checkpoint 4: PyPI API-backed project description fast path
- Audit IDs: G6, Q1, Q2, Q3
- Expected changes: implement `known_sources/pypi.rs`; introduce PyPI source plan variant; parse minimal JSON; render Markdown metadata and description; fallback on unusable JSON/description
- Validation: mapping tests, local-server async test, targeted webfetch tests/check/clippy
- Exit condition: PyPI flow is verified without live network dependency in tests

### Checkpoint 5: final guidance and audit
- Audit IDs: V1 and all open items
- Expected changes: small prompt guidance update only if needed; update Completion Audit and Final Verification
- Validation: full goal validation contract
- Exit condition: every Completion Audit item is verified or explicitly user-dropped

## Validation Contract

- Static checks:
  - `cargo fmt --all -- --check`
  - `cargo check -p oxide-agent-core --no-default-features --features "tool-webfetch-md tool-crawl4ai-markdown"`
  - `cargo clippy -p oxide-agent-core --no-default-features --features tool-webfetch-md --all-targets -- -D warnings`
- Tests:
  - `cargo test -p oxide-agent-core --no-default-features --features tool-webfetch-md --lib webfetch_md`
- Artifact verification:
  - `git diff -- Cargo.toml` shows no dependency changes
  - code references for source mapping, output rendering, and fallback behavior
- Done when: every Completion Audit item is verified with current evidence.

## Decisions

- 2026-06-11: Use a focused `known_sources/` directory-module because adding GitLab/Gitea/crates/docs.rs/PyPI directly to `fetch.rs` would make the fetch orchestration file a per-site matcher, while one monolithic `known_sources.rs` would become a mixed host/API parser.
- 2026-06-11: Use crates.io README API for both `crates.io` and `docs.rs` because it returns README content directly and avoids docs.rs HTML/source pages.
- 2026-06-11: Use PyPI JSON API and render `info.description` locally because PyPI project pages are HTML shells while the JSON API exposes project metadata and long description directly.
- 2026-06-11: Avoid arbitrary-host root guessing for self-hosted forges; only explicit forge path patterns are safe enough without configuration.
- 2026-06-11: Keep known-source slices boring and explicit: no traits, registry framework, macros, or generic router until real duplication proves they are needed.

## Progress Log

- 2026-06-11 00:00: Checkpoint 0 drafted
  - Changed: created goal contract for webfetch known-source fast paths
  - Evidence: pending commit after review of doc diff
  - Commands: `git status --short && git log --oneline -5`; read `docs/goals`; inspected current `fetch.rs` and tests; web probes verified GitLab raw, Codeberg raw, and crates.io README API patterns
  - Audit IDs updated: none, planning checkpoint only
  - Next: commit this goal document and stop for user review before Checkpoint 1

- 2026-06-11 00:15: Slicing plan tightened after user review
  - Changed: replaced monolithic `known_sources.rs` plan with `known_sources/` directory-module boundaries; made G1 stricter; added Q4 anti-overengineering slice-boundary requirement; clarified Checkpoints 1-4 ownership by slice
  - Evidence: doc diff reviewed before commit
  - Commands: `git status --short && git log --oneline -5`; read current goal document
  - Audit IDs updated: G1, Q4, checkpoint plan
  - Next: commit goal update and wait for implementation approval

- 2026-06-11 00:35: Checkpoint 1 implemented
  - Changed: added `known_sources/mod.rs` and `known_sources/repo_hosts.rs`; moved existing GitHub/HuggingFace classifier out of `fetch.rs`; updated tests to call `known_sources::classify()`
  - Evidence: `fetch.rs` line count reduced to 285; `known_sources/mod.rs` is 45 lines; `known_sources/repo_hosts.rs` is 94 lines
  - Commands: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-webfetch-md --lib webfetch_md`; `cargo check -p oxide-agent-core --no-default-features --features "tool-webfetch-md tool-crawl4ai-markdown"`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-webfetch-md --all-targets -- -D warnings`
  - Audit IDs updated: G1(in_progress), Q1(in_progress), Q2(in_progress), Q4(in_progress)
  - Next: Checkpoint 2 direct forge README fast paths

- 2026-06-12 00:04: Checkpoint 2 implemented
  - Changed: added GitLab root/blob README rewrites; added Codeberg/Gitea known-host root rewrites; added explicit Gitea/Forgejo `src/branch` README rewrites for any host without arbitrary root guessing
  - Evidence: `known_sources/repo_hosts.rs` is 168 lines; tests cover GitLab root/nested/blob, Codeberg root, Gitea explicit source path, generic self-hosted explicit source path, and negative cases
  - Commands: `cargo fmt --all`; `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-webfetch-md --lib webfetch_md`; `cargo check -p oxide-agent-core --no-default-features --features "tool-webfetch-md tool-crawl4ai-markdown"`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-webfetch-md --all-targets -- -D warnings`; `git diff -- Cargo.toml`
  - Audit IDs updated: G2(verified), G3(verified), Q1(in_progress), Q2(in_progress), V1(in_progress), N1(verified)
  - Next: Checkpoint 3 crates.io and docs.rs API-backed README fast paths

- 2026-06-12 00:38: Checkpoint 3 implemented
  - Changed: added `CrateReadme` known-source plan; added `known_sources/rust_packages.rs` for crates.io/docs.rs classification, minimal crates.io metadata version selection, README API URL construction, and source-transparent rendering; added local-server async coverage for metadata+README fetch flow
  - Evidence: `rust_packages.rs` is 162 lines; tests cover crates.io mapping, docs.rs root/latest/explicit version mapping, and local metadata+README fetch output fields
  - Commands: `cargo fmt --all`; `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-webfetch-md --lib webfetch_md`; `cargo check -p oxide-agent-core --no-default-features --features "tool-webfetch-md tool-crawl4ai-markdown"`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-webfetch-md --all-targets -- -D warnings`; `git diff -- Cargo.toml`
  - Audit IDs updated: G4(verified), G5(verified), Q1(in_progress), Q2(in_progress), Q3(in_progress), Q4(in_progress), V1(in_progress)
  - Next: Checkpoint 4 PyPI API-backed project description fast path

- 2026-06-12 00:55: Checkpoint 4 implemented
  - Changed: added `PypiProject` known-source plan; added `known_sources/pypi.rs` for PyPI project URL classification, minimal JSON parsing, and source-transparent project description rendering; added local-server async coverage for JSON-to-Markdown output
  - Evidence: `pypi.rs` is 163 lines; tests cover PyPI mapping, local JSON fetch output fields, and negative non-project PyPI URLs
  - Commands: `cargo fmt --all`; `cargo fmt --all -- --check`; `cargo test -p oxide-agent-core --no-default-features --features tool-webfetch-md --lib webfetch_md`; `cargo check -p oxide-agent-core --no-default-features --features "tool-webfetch-md tool-crawl4ai-markdown"`; `cargo clippy -p oxide-agent-core --no-default-features --features tool-webfetch-md --all-targets -- -D warnings`; `git diff -- Cargo.toml`; `git diff --check`
  - Audit IDs updated: G1(verified), G6(verified), Q1(in_progress), Q2(in_progress), Q3(in_progress), Q4(in_progress), V1(in_progress)
  - Next: Checkpoint 5 final guidance and audit

## Risks and Blockers

- Some package APIs can return large JSON or HTML-converted descriptions.
  - Impact: output could exceed useful size or include package-page noise.
  - Evidence: PyPI and crates.io API responses can be large.
  - Mitigation: reuse existing response byte limits and `MAX_OUTPUT_CHARS` truncation; parse only required JSON fields.
  - Audit IDs affected: G4, G5, G6, Q3

- Default branch discovery for direct forge root URLs is not uniform across hosts.
  - Impact: `HEAD` may not work everywhere.
  - Evidence: GitLab and Codeberg probes accepted `HEAD` for tested raw paths; arbitrary hosts are not guaranteed.
  - Mitigation: root fast paths only for known hosts; failures fall back to normal fetch.
  - Audit IDs affected: G2, G3, Q1

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
