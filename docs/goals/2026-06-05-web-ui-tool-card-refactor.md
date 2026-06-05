# Goal: Web UI Tool Card Refactor

Date started: 2026-06-05
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-05-web-ui-tool-card-refactor.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: User request after web Activity/tool-card RECON: reduce duplication in `crates/oxide-agent-web-ui/src/tasks.rs` by extracting simple universal helpers for web transport tool rendering.
Goal doc owner: Codex
Last updated: 2026-06-05 09:58

## Objective

Reduce duplicated web Activity/tool-card rendering code in `crates/oxide-agent-web-ui/src/tasks.rs` by introducing small shared helpers for common tool-card chrome, raw debug blocks, details wrappers, metadata rows, and stream rendering.

Done when the specialized tool cards preserve their current visual behavior and parsing semantics, repeated markup/status/duration/raw blocks are consolidated into simple helpers, and every Completion Audit item is verified by its listed evidence.

## Scope

In scope:
- `crates/oxide-agent-web-ui/src/tasks.rs` tool/event Activity rendering helpers and specialized tool cards.
- Existing specialized cards: shell, search, web markdown, Crawl4AI markdown, sub-agent spawn/wait, write todos, generic fallback, and reasoning card.
- This goal document and checkpoint progress evidence.

Out of scope:
- Changing backend web transport events, contracts, provider output formats, tool names, route behavior, or persistence.
- Changing CSS unless a helper cannot preserve existing class structure.
- Adding new crates, frontend frameworks, macro systems, builders, or broad generic card registries.
- Redesigning card visuals, copy, `default_open` behavior, preview priority, or parsing logic beyond mechanical preservation.
- Refactoring unrelated web UI routes, auth, SSE, chat messages, markdown renderer, or API client code.

## Missing Inputs

- None required.

## Repository Context

- Web UI Activity tool dispatch is centralized in `crates/oxide-agent-web-ui/src/tasks.rs:1908`.
- Specialized cards currently live in the same file: `ShellToolCard` at `tasks.rs:1980`, `SearchToolCard` at `tasks.rs:2094`, `WebMarkdownToolCard` at `tasks.rs:2328`, `CrawlToolCard` at `tasks.rs:2488`, `SpawnSubAgentsToolCard` at `tasks.rs:2734`, `WaitSubAgentsToolCard` at `tasks.rs:2865`, `WriteTodosToolCard` at `tasks.rs:3003`, and `GenericToolCard` at `tasks.rs:3093`.
- Existing reusable helpers already include `tool_duration_ms` at `tasks.rs:3636`, `format_duration_ms` at `tasks.rs:3646`, `tool_status_icon` at `tasks.rs:3654`, `input_preview_json` at `tasks.rs:3664`, `render_todo_list` at `tasks.rs:3877`, `tool_result_summary` at `tasks.rs:3906`, and `stream_text` at `tasks.rs:3965`.
- Confirmed duplicated UI patterns include tool-card headers, preview blocks, details wrappers, raw debug blocks, metadata rows, and stream/pre wrappers across the specialized cards.
- `crates/oxide-agent-web-ui/Cargo.toml:11` forbids `unwrap_used` and warns on `too_many_lines`; no new dependencies are needed.
- Repo validation for touched web UI code should include native, wasm, clippy, and Trunk checks.

## Completion Audit

- G1: Shared tool outcome/status utilities are used consistently
  - Source: RECON finding that `is_running`, `success`, duration, and icon extraction are repeated across tool cards.
  - Acceptance: Repeated manual `is_running`/`success`/duration/icon logic is reduced by small helpers or reuse of existing helpers, without changing status class, labels, duration visibility, or failure behavior for any card.
  - Evidence required: focused diff review plus `cargo check -p oxide-agent-web-ui`.
  - Status: verified
  - Evidence collected: Checkpoint 1 added `ToolOutcome`, `tool_duration_label`, `raw_output_preview`, `tool_url_from_structured_or_input`, and `active_count_label` in `crates/oxide-agent-web-ui/src/tasks.rs`; focused diff review confirmed card-specific preview/default-open/parsing remained local; `cargo check -p oxide-agent-web-ui` passed.

- G2: Repeated visual chrome is extracted into simple helpers
  - Source: RECON finding that headers, previews, details wrappers, raw blocks, metadata rows, and stream wrappers are duplicated.
  - Acceptance: Introduce simple local helpers/components for repeated visual fragments such as header/meta, preview, details shell, raw output details, key/value row, pre stream, and markdown stream. Helpers preserve existing HTML classes and do not introduce a large generic builder/spec framework.
  - Evidence required: focused diff review plus native and wasm web-ui checks.
  - Status: verified
  - Evidence collected: Checkpoint 2 added local visual primitives in `crates/oxide-agent-web-ui/src/tasks.rs`: `ToolHeaderMeta`, `tool_card_header`, `tool_preview`, `ToolDetails`, `tool_query_row`, `tool_command`, `tool_pre_stream`, `tool_markdown_stream`, and `tool_raw_details`. Focused diff review confirmed preserved classes: `tool-card-header`, `tool-status-icon`, `tool-name`, `tool-meta`, `tool-preview`, `tool-card-body`, `tool-card-expand`, `tool-query`, `tool-stream`, `tool-stream-label`, `tool-stream-pre`, `tool-stream-content`, and `tool-raw-details`; native and wasm checks passed.

- G3: Specialized parsing and UX semantics remain local and unchanged
  - Source: RECON risk assessment that one generic `ToolCardSpec` would overfit because parsing, preview priority, and `default_open` differ per card.
  - Acceptance: Search result parsing, WebMarkdown parsing, Crawl4AI success/failure parsing, sub-agent parsing, todo parsing, preview priority, and `default_open` rules remain card-specific unless a helper is proven trivial and behavior-preserving.
  - Evidence required: diff review of each specialized card and manual behavior checklist recorded in progress log.
  - Status: verified
  - Evidence collected: Checkpoint 1 moved only shared outcome/duration/raw-output/URL-fallback/active-count extraction; specialized parsing, preview priority, and `default_open` branches stayed in each card. Checkpoint 2 moved only repeated markup fragments; search/WebMarkdown/Crawl/sub-agent/todo parsing and preview/default-open expressions remained local. Checkpoint 3 applied the remaining safe primitives to `ReasoningEventCard` only through class-preserving helper variants and reviewed all card groups; parsing, preview priority, and `default_open` stayed local.

- G4: Tool-card rendering remains compatible with recent web UI polish
  - Source: Recent commits `377255ce`, `79c1919c`, and `acac93d7` added rich tool cards, inline todos, reasoning preview, and compact failed Crawl cards.
  - Acceptance: The refactor preserves current cards for `web_markdown`, `searxng_search`, `spawn_sub_agents`, `wait_sub_agents`, `write_todos`, Reasoning/CoT, and failed `crawl4ai_markdown` compact display.
  - Evidence required: diff review against the listed behaviors plus `env -u NO_COLOR trunk build --release`.
  - Status: verified
  - Evidence collected: Checkpoint 2 preserved the existing specialized branches for `web_markdown`, `searxng_search`, `spawn_sub_agents`, `wait_sub_agents`, `write_todos`, and compact failed `crawl4ai_markdown`. Checkpoint 3 preserved Reasoning/CoT classes while moving its header, preview, details shell, and reasoning stream to shared primitive variants; `env -u NO_COLOR trunk build --release` passed.

- Q1: UI-only blast radius is preserved
  - Source: User requested refactor in `crates/oxide-agent-web-ui/src/tasks.rs`; AGENTS over-engineering guardrails require smallest maintainable change.
  - Acceptance: No backend/core/contracts/provider files change; no output formats, tool names, routes, or transport events change.
  - Evidence required: `git diff --stat` and file list review.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 implementation changed only `crates/oxide-agent-web-ui/src/tasks.rs`; this goal doc was updated for evidence. Checkpoint 2 changed only `crates/oxide-agent-web-ui/src/tasks.rs` plus this goal doc. Checkpoint 3 changed only `crates/oxide-agent-web-ui/src/tasks.rs` plus this goal doc.

- Q2: No new dependencies or broad abstractions
  - Source: `AGENTS.md` implementation bias and RECON recommendation.
  - Acceptance: No `Cargo.toml` changes; no new crates; no broad card registry/builder/macro framework; helpers stay local and boring.
  - Evidence required: `Cargo.toml` diff review and implementation diff review.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 added local data helpers only; no `Cargo.toml` changes and no card registry/builder/macro framework. Checkpoint 2 added small local Leptos helpers/components only; no dependency or `Cargo.toml` changes. Checkpoint 3 added class-preserving helper variants only; no dependency or `Cargo.toml` changes.

- V1: Web UI validation passes after each meaningful checkpoint
  - Source: Repo validation practice and prior web UI checks.
  - Acceptance: Required commands pass for the checkpoint scope or a blocker records exact failure output and smallest next action.
  - Evidence required: `cargo fmt`, `cargo check -p oxide-agent-web-ui`, `cargo check --target wasm32-unknown-unknown -p oxide-agent-web-ui`, `cargo clippy -p oxide-agent-web-ui`, and `env -u NO_COLOR trunk build --release` before final completion.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 passed `cargo fmt`, `cargo check -p oxide-agent-web-ui`, `cargo check --target wasm32-unknown-unknown -p oxide-agent-web-ui`, and `cargo clippy -p oxide-agent-web-ui`. Checkpoint 2 passed `cargo fmt`, `cargo check -p oxide-agent-web-ui`, `cargo check --target wasm32-unknown-unknown -p oxide-agent-web-ui`, `cargo clippy -p oxide-agent-web-ui`, and `git diff --check`. Checkpoint 3 passed `cargo fmt`, `cargo check -p oxide-agent-web-ui`, `cargo check --target wasm32-unknown-unknown -p oxide-agent-web-ui`, `cargo clippy -p oxide-agent-web-ui`, `env -u NO_COLOR trunk build --release`, and `git diff --check`.

- N1: No visual redesign hidden inside refactor
  - Source: User goal is deduplication/refactor, not another UI redesign pass.
  - Must preserve: Existing card text, CSS class names, collapsed/expanded defaults, and specialized previews unless explicitly called out in this document and approved later.
  - Evidence required: diff review and behavior checklist.
  - Status: verified
  - Evidence collected: Checkpoint 1 did not change visible strings, CSS classes, or details/default-open expressions; it only replaced repeated data extraction with helpers. Checkpoint 2 preserved visible card names, meta text, preview choices, details labels, CSS classes, and local `default_open` expressions while moving repeated markup into helpers. Checkpoint 3 preserved Reasoning/CoT visible strings (`Thinking`, `CoT`, `truncated`, `redacted`, `details`, `reasoning`), classes, and closed details behavior while applying helper variants.

## Implementation Plan

1. Normalize shared tool outcome helpers
   - Audit IDs: G1, G3, Q1, Q2, N1.
   - Expected changes: add/reuse helpers for success/running state, duration labels, status icons, raw output extraction, active-count labels, and URL extraction from structured payload/input preview where already duplicated.
   - Validation: `cargo fmt`; `cargo check -p oxide-agent-web-ui`; focused diff review that no card behavior changes.
   - Exit condition: cards compile using shared status/duration/raw helpers while visual output remains equivalent.

2. Extract repeated visual primitives
   - Audit IDs: G2, G3, Q1, Q2, N1.
   - Expected changes: introduce small local rendering helpers/components for `ToolPreview`, `ToolDetails`, `ToolRawDetails`, metadata key/value rows, pre streams, and markdown streams while preserving current class names.
   - Validation: native and wasm web-ui checks.
   - Exit condition: duplicated markup is reduced and specialized parsing/preview/default-open logic remains local.

3. Apply primitives to specialized cards in safe groups
   - Audit IDs: G2, G3, G4, N1.
   - Expected changes: first update `GenericToolCard` and `ShellToolCard`; then `SearchToolCard`; then `WebMarkdownToolCard` and `CrawlToolCard`; then sub-agent/todos cards; finally reuse only safe pieces in `ReasoningEventCard`.
   - Validation: checkpoint-by-checkpoint diff review plus `cargo check -p oxide-agent-web-ui` after each group if changes are non-trivial.
   - Exit condition: all specialized cards use shared visual primitives where sensible, with behavior checklist preserved.

4. Final verification and audit update
   - Audit IDs: G1-G4, Q1-Q2, V1, N1.
   - Expected changes: update this goal document with evidence, command results, and any accepted deviations.
   - Validation: `cargo fmt`; `cargo check -p oxide-agent-web-ui`; `cargo check --target wasm32-unknown-unknown -p oxide-agent-web-ui`; `cargo clippy -p oxide-agent-web-ui`; `env -u NO_COLOR trunk build --release`; `git diff --check`.
   - Exit condition: every Completion Audit item is verified and the refactor is ready to commit.

## Validation Contract

- Static checks:
  - `cargo check -p oxide-agent-web-ui`
  - `cargo check --target wasm32-unknown-unknown -p oxide-agent-web-ui`
- Lints/format:
  - `cargo fmt`
  - `cargo clippy -p oxide-agent-web-ui`
  - `git diff --check`
- Runtime/build artifact:
  - `env -u NO_COLOR trunk build --release`
- Done when: every Completion Audit item is verified, no out-of-scope files are changed, and the final diff shows a local helper refactor without visual redesign.

## Decisions

- 2026-06-05: Use `docs/goals/2026-06-05-web-ui-tool-card-refactor.md` because the repo stores durable goal docs under `docs/goals/`.
- 2026-06-05: Keep refactor UI-only and centered on `crates/oxide-agent-web-ui/src/tasks.rs`; backend/core/contracts/provider changes are out of scope.
- 2026-06-05: Do not introduce one generic `ToolCardSpec` or builder. Specialized parsing, preview priority, and `default_open` remain local to avoid over-engineering.
- 2026-06-05: First implementation step is shared tool outcome/helper normalization before extracting visual primitives, because it is the smallest low-risk deduplication pass.

## Progress Log

- 2026-06-05: Goal document created from RECON.
  - Changed: Added this goal contract for `tasks.rs` tool-card helper refactoring.
  - Evidence: Existing docs convention found under `docs/goals/`; current tool-card duplication confirmed around `crates/oxide-agent-web-ui/src/tasks.rs:1908-3188` and helpers around `tasks.rs:3636-3965`.
  - Commands: none.
  - Audit IDs updated: none.
  - Next: Checkpoint 1 — normalize shared tool outcome helpers.

- 2026-06-05: Checkpoint 1 — normalized shared tool outcome helpers.
  - Changed: Added local helpers for tool outcome/status class/icon, duration label, raw output preview, structured/input URL fallback, and sub-agent active-count labels; switched specialized cards to those helpers.
  - Evidence: Focused diff review confirmed specialized parsing, preview priority, `default_open`, labels, and CSS classes stayed local and visually equivalent.
  - Commands: `cargo fmt`; `cargo check -p oxide-agent-web-ui`; `cargo check --target wasm32-unknown-unknown -p oxide-agent-web-ui`; `cargo clippy -p oxide-agent-web-ui`.
  - Audit IDs updated: G1 verified; G3, Q1, Q2, V1, N1 in progress.
  - Next: Checkpoint 2 — extract repeated visual primitives.

- 2026-06-05: Checkpoint 2 — extracted repeated visual primitives.
  - Changed: Added local helper primitives for tool headers/meta, previews, details shells, key/value rows, command rows, pre streams, markdown streams, and raw output details; replaced repeated markup in specialized tool cards with those helpers.
  - Evidence: Focused diff review confirmed specialized parsing, preview priority, `default_open`, card names, labels, and CSS classes stayed behavior-preserving; Reasoning/CoT was intentionally left outside the main tool-card helper extraction.
  - Commands: `cargo fmt`; `cargo check -p oxide-agent-web-ui`; `cargo check --target wasm32-unknown-unknown -p oxide-agent-web-ui`; `cargo clippy -p oxide-agent-web-ui`; `git diff --check`.
  - Audit IDs updated: G2 verified; G3, G4, Q1, Q2, V1, N1 in progress.
  - Next: Checkpoint 3 — apply/review remaining safe primitive usage by card groups and complete the behavior checklist.

- 2026-06-05: Checkpoint 3 — applied remaining safe primitive usage and behavior checklist.
  - Changed: Added class-preserving variants for tool header, preview, and details helpers; moved `ReasoningEventCard` header/preview/details/stream markup to those helpers.
  - Evidence: Behavior checklist preserved `web_markdown`, `searxng_search`, sub-agent spawn/wait, inline `write_todos`, compact failed Crawl, and Reasoning/CoT labels/classes/default-open behavior; the only remaining direct `tool-stream-content` markup is the nested sub-agent status output body, which intentionally does not use the full stream wrapper.
  - Commands: `cargo fmt`; `cargo check -p oxide-agent-web-ui`; `cargo check --target wasm32-unknown-unknown -p oxide-agent-web-ui`; `cargo clippy -p oxide-agent-web-ui`; `env -u NO_COLOR trunk build --release`; `git diff --check`.
  - Audit IDs updated: G3, G4, N1 verified; Q1, Q2, V1 remain in progress for final file-list/diff-check audit.
  - Next: Checkpoint 4 — final verification and audit update.

## Risks and Blockers

- Accidentally changing UI behavior during markup extraction.
  - Impact: Regressions in recently polished Activity cards.
  - Evidence: Recent changes added specialized behavior for todos, reasoning, sub-agents, web markdown, SearXNG, and compact failed Crawl cards.
  - Mitigation: Preserve class names and move only exact repeated fragments; keep parsing, preview priority, and `default_open` local.
  - Audit IDs affected: G3, G4, N1.

- Leptos view type friction when extracting helpers.
  - Impact: Compilation failures or overuse of boxed views.
  - Evidence: Existing cards mix `impl IntoView`, `AnyView`, `view!`, and conditional branches.
  - Mitigation: Prefer small helpers returning `AnyView` only where branches already require it; otherwise use simple functions for data extraction.
  - Audit IDs affected: G2, V1.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
