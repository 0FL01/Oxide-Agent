# Goal: Web UI CSS v2 Slice Plan

Date started: 2026-06-06
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-06-web-ui-css-v2-slice-plan.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update this document after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: User request to split `crates/oxide-agent-web-ui/src/styles.css` for maintainability, with `Редизайн (v2)` as the current base and `v1` as MVP legacy.
Goal doc owner: Codex
Last updated: 2026-06-06 22:30

## Objective

Convert the monolithic web UI stylesheet into maintainable, v2-first CSS slices without changing the visual design, class contracts, Trunk entrypoint behavior, or web UI runtime semantics.

Done when `crates/oxide-agent-web-ui/src/styles.css` is a small stylesheet entrypoint, focused CSS slices live under `crates/oxide-agent-web-ui/src/styles/`, the old `v1 base + v2 override` cascade has been collapsed into canonical v2 rules, and every Completion Audit item is verified by its listed evidence.

## Scope

In scope:
- `crates/oxide-agent-web-ui/src/styles.css`.
- New focused CSS files under `crates/oxide-agent-web-ui/src/styles/`.
- `crates/oxide-agent-web-ui/index.html` only if Trunk requires an entrypoint adjustment after validation.
- This goal document and checkpoint progress evidence.

Out of scope:
- Changing Leptos component structure or Rust behavior.
- Changing existing CSS class names used by `crates/oxide-agent-web-ui/src/**/*.rs`.
- New UI features, visual redesigns, themes, CSS modules, preprocessors, style frameworks, or new build dependencies.
- Backend web routes, contracts, SSE behavior, auth, storage, runtime/core/provider code, or Telegram transport code.
- Direct Google Gemini provider work or unrelated cleanup.

## Missing Inputs

- None required for the planning checkpoint.

## Repository Context

- The current stylesheet is `3,223` lines and is loaded from `crates/oxide-agent-web-ui/index.html:11` via `<link data-trunk rel="css" href="src/styles.css" />`.
- The first half is MVP/v1-era base styling, starting with design tokens at `crates/oxide-agent-web-ui/src/styles.css:1` and continuing through responsive rules at `crates/oxide-agent-web-ui/src/styles.css:1817`.
- The current v2 design starts at `crates/oxide-agent-web-ui/src/styles.css:1840` with `FRONT TEMPLATE REDESIGN OVERRIDE`; this must become canonical rather than remaining an override layer.
- Major v2 areas are shell/sidebar/topbar at `crates/oxide-agent-web-ui/src/styles.css:1966`, chat workspace at `crates/oxide-agent-web-ui/src/styles.css:2193`, messages at `crates/oxide-agent-web-ui/src/styles.css:2295`, composer at `crates/oxide-agent-web-ui/src/styles.css:2564`, activity drawer at `crates/oxide-agent-web-ui/src/styles.css:2732`, activity/tool cards at `crates/oxide-agent-web-ui/src/styles.css:2844`, markdown at `crates/oxide-agent-web-ui/src/styles.css:3084`, and responsive rules at `crates/oxide-agent-web-ui/src/styles.css:3179`.
- v1-only rules still contain useful coverage for settings pages, metrics groups, and code-copy helpers at `crates/oxide-agent-web-ui/src/styles.css:1588`, `crates/oxide-agent-web-ui/src/styles.css:1728`, and `crates/oxide-agent-web-ui/src/styles.css:1755`; these must be audited before deletion.
- CSS class consumers include sidebar markup in `crates/oxide-agent-web-ui/src/sessions.rs:63`, workspace/composer markup in `crates/oxide-agent-web-ui/src/tasks/workspace.rs:682`, activity drawer markup in `crates/oxide-agent-web-ui/src/tasks/activity.rs:101`, tool-card markup in `crates/oxide-agent-web-ui/src/tasks/tool_cards.rs:1125`, markdown output wrapper in `crates/oxide-agent-web-ui/src/markdown.rs:73`, and auth/settings markup in `crates/oxide-agent-web-ui/src/auth.rs:90` and `crates/oxide-agent-web-ui/src/auth.rs:275`.
- Web UI validation should prefer wasm compilation and Trunk build checks because `oxide-agent-web-ui` is a Leptos CSR crate: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown` and `env -u NO_COLOR trunk build --release` from `crates/oxide-agent-web-ui/`.

## Completion Audit

- G1: Stylesheet entrypoint is small and slice-based
  - Source: User request to split `styles.css` into slices for maintainability.
  - Acceptance: `crates/oxide-agent-web-ui/src/styles.css` contains only ordered imports and short entrypoint comments, or an equally small Trunk-compatible entrypoint; focused slice files live under `crates/oxide-agent-web-ui/src/styles/`.
  - Evidence required: `wc -l crates/oxide-agent-web-ui/src/styles.css`, `find crates/oxide-agent-web-ui/src/styles -maxdepth 1 -type f | sort`, and `env -u NO_COLOR trunk build --release`.
  - Status: in_progress
  - Evidence collected: Checkpoint 1 scaffold created with `crates/oxide-agent-web-ui/src/styles.css` reduced to 3 lines and temporary ordered slices `crates/oxide-agent-web-ui/src/styles/00-v1-base.css` and `crates/oxide-agent-web-ui/src/styles/10-v2-current.css`; checkpoint 2 expanded the entrypoint to 6 ordered imports and added focused base slices `00-tokens.css`, `01-reset.css`, and `02-primitives.css`; checkpoint 3 expanded the entrypoint to 7 ordered imports and added `03-shell.css`; final component slices still pending.

- G2: v2 is the canonical base, not an override block
  - Source: User clarified that `Редизайн (v2)` is the current base and `v1` was MVP.
  - Acceptance: The `FRONT TEMPLATE REDESIGN OVERRIDE` model is removed; duplicate selectors are resolved to one canonical v2 rule per slice unless a deliberate documented fallback is required.
  - Evidence required: grep for `FRONT TEMPLATE REDESIGN OVERRIDE`, duplicate-selector review for high-risk selectors, and focused diff review.
  - Status: in_progress
  - Evidence collected: Checkpoint 2 removed `FRONT TEMPLATE REDESIGN OVERRIDE` from `crates/oxide-agent-web-ui/src/styles/10-v2-current.css`; v2 token/reset/primitive rules now live in base slices before component layers. Checkpoint 3 collapsed shell/sidebar/session-nav/topbar duplicate selectors into canonical `crates/oxide-agent-web-ui/src/styles/03-shell.css`; responsive media-query duplicates remain intentionally deferred to checkpoint 7.

- G3: Component ownership is clear
  - Source: Maintainability goal from the user request.
  - Acceptance: A class family lives in one slice by component/area: tokens/reset/primitives, shell, chat, composer, activity/tool cards, markdown/code, pages, metrics, responsive.
  - Evidence required: file list review plus spot grep of representative classes (`.composer`, `.activity-drawer`, `.tool-card`, `.markdown-content`, `.settings-page`).
  - Status: in_progress
  - Evidence collected: Checkpoint 2 established base ownership with `00-tokens.css`, `01-reset.css`, and `02-primitives.css`; checkpoint 3 established shell/navigation ownership with `03-shell.css`; chat/activity/markdown/page ownership remains pending.

- G4: v1-only useful coverage is preserved or intentionally removed
  - Source: Recon found v1-only settings, metrics, and code-copy styles not fully represented in v2.
  - Acceptance: Settings/auth/not-found, metrics panel groups, code-copy helpers, status/error/notice helpers, and any class still emitted by Rust components remain styled; removed rules are verified unused.
  - Evidence required: grep for class consumers in `crates/oxide-agent-web-ui/src/**/*.rs`, diff review, and build validation.
  - Status: in_progress
  - Evidence collected: Checkpoint 3 preserved legacy status badge coverage while moving shell ownership; grep confirmed active shell class consumers in `components.rs` and `sessions.rs`. Settings, metrics, code-copy, and page coverage remain pending for later checkpoints.

- Q1: No visual redesign or class-contract changes
  - Source: User asked for maintainability slicing, not a new redesign.
  - Acceptance: Existing class names, visible strings, component markup, layout intent, and v2 visual treatment are preserved unless a user-approved follow-up explicitly changes them.
  - Evidence required: CSS-only diff review and `git diff --name-only` showing no Rust component changes except an entrypoint adjustment if required.
  - Status: in_progress
  - Evidence collected: Checkpoints 1-3 changed only CSS slices and this goal document; no Rust component markup or class strings were changed. Final visual/class-contract audit remains pending until all slices are extracted.

- Q2: No over-engineering or new dependencies
  - Source: Repository implementation bias and user request scope.
  - Acceptance: No new crates, JS tooling, CSS frameworks, preprocessors, CSS modules, design-token generators, or broad abstraction layers.
  - Evidence required: `git diff -- Cargo.toml crates/oxide-agent-web-ui/Cargo.toml package.json pnpm-lock.yaml`, plus full diff review.
  - Status: in_progress
  - Evidence collected: Checkpoints 1-3 used plain CSS `@import` slices only. `git diff -- Cargo.toml crates/oxide-agent-web-ui/Cargo.toml package.json pnpm-lock.yaml` produced no output after checkpoints 2 and 3.

- V1: Web UI CSS build remains valid
  - Source: Trunk stylesheet entrypoint at `crates/oxide-agent-web-ui/index.html:11`.
  - Acceptance: Trunk can build the sliced stylesheet and generated app assets without CSS import/path failures.
  - Evidence required: `env -u NO_COLOR trunk build --release` from `crates/oxide-agent-web-ui/`.
  - Status: verified
  - Evidence collected: `env -u NO_COLOR trunk build --release` from `crates/oxide-agent-web-ui/` succeeded on 2026-06-06 after the import-based scaffold split, after checkpoint 2 base-slice promotion, and after checkpoint 3 shell extraction.

- V2: Rust-side web UI still compiles
  - Source: CSS class consumers live in Leptos components under `crates/oxide-agent-web-ui/src/`.
  - Acceptance: No accidental Rust compile regressions while touching the UI crate.
  - Evidence required: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown` from repo root.
  - Status: verified
  - Evidence collected: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown` succeeded on 2026-06-06 after the scaffold split, after checkpoint 2 base-slice promotion, and after checkpoint 3 shell extraction.

- N1: No unrelated product behavior changes
  - Source: Scope boundaries and repository guardrails.
  - Must preserve: backend APIs, SSE streams, auth/session/task behavior, storage, provider/runtime/core code, and Telegram transport behavior.
  - Evidence required: `git diff --name-only` and final diff audit.
  - Status: in_progress
  - Evidence collected: Checkpoints 1-3 changed only web UI stylesheet slices and this goal document. Final `git diff --name-only` audit remains pending until completion.

## Implementation Plan

1. Create a lossless slice scaffold while preserving cascade order
   - Audit IDs: G1, Q1, Q2, V1, V2, N1.
   - Expected changes: create `crates/oxide-agent-web-ui/src/styles/`; move the existing content into ordered coarse files without semantic edits; keep `crates/oxide-agent-web-ui/src/styles.css` as the Trunk entrypoint; preserve the existing rule order exactly enough that rendered CSS remains equivalent.
   - Suggested initial files: `00-v1-base.css` and `10-v2-current.css` if a purely mechanical first move is safest, or the final slice filenames if order can be preserved without risk.
   - Validation: `env -u NO_COLOR trunk build --release`; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `git diff --check`.
   - Exit condition: stylesheet is physically split, the app still builds, and no behavior/visual intent has changed.

2. Promote v2 tokens, reset, and primitives to canonical base
   - Audit IDs: G2, G3, Q1, Q2, V1.
   - Expected changes: create final base slices `00-tokens.css`, `01-reset.css`, and `02-primitives.css`; keep the final v2 variable set from `crates/oxide-agent-web-ui/src/styles.css:1841`; preserve compatibility aliases such as `--fg-muted`, `--danger`, `--accent-dim`, and status aliases while Rust/CSS consumers still use them.
   - Validation: grep for variable usage across slices; Trunk build; focused diff review for `button`, `.button`, `.btn-primary`, `.btn-danger`, `input`, `select`, `textarea`, `.status-badge`, `.status-chip`, `.error-text`, `.notice`, `.muted`.
   - Exit condition: base styling is v2-first and no longer depends on a late override block.

3. Extract shell and navigation slices
   - Audit IDs: G2, G3, G4, Q1, V1, V2.
   - Expected changes: create `03-shell.css` for `.app-layout`, `.workspace-main`, `.sidebar`, `.sidebar-*`, `.session-*`, `.topbar`, `.topnav`, `.brand`, `.user-pill`, and shell-level empty/loading states where appropriate.
   - Validation: grep consumers in `sessions.rs`, `app.rs`, and `tasks/workspace.rs`; Trunk build; wasm check.
   - Exit condition: shell/navigation ownership is clear and duplicate v1/v2 selectors for shell classes are collapsed to canonical v2 rules.

4. Extract chat, message, and composer slices
   - Audit IDs: G2, G3, Q1, V1, V2.
   - Expected changes: create `04-chat.css` for `.session-workspace`, `.chat-wrapper`, `.results-panel`, `.task-card`, `.message`, `.user-message`, `.assistant-message`, attachments, collapsible message controls, message actions, `.thinking-button`, `.welcome-view`, and `.empty-state`; create `05-composer.css` for `.composer`, `.composer-inner`, composer textarea, footer/actions, attach file input, profile/effort selects, and composer notices.
   - Validation: grep consumers in `tasks/workspace.rs` and `tasks/task_card.rs`; Trunk build; wasm check; review mobile composer behavior.
   - Exit condition: chat and composer rules are isolated and preserve current v2 layout.

5. Extract activity, tool-card, todos, context, and metrics slices
   - Audit IDs: G2, G3, G4, Q1, V1, V2.
   - Expected changes: create `06-activity.css` for `.activity-*`, `.agent-activity`, `.agent-event-*`, `.tool-*`, `.todos-*`, `.todo-*`, `.context-card-*`, reasoning cards, and search-result classes; create `07-metrics.css` for `.events-panel`, `.metrics-*`, `.sse-*`, `.progress-*`, `.event-*`, and panel header/content classes.
   - Validation: grep consumers in `tasks/activity.rs` and `tasks/tool_cards.rs`; ensure metrics/sidebar panel classes from v1-only coverage remain styled; Trunk build; wasm check.
   - Exit condition: operational/activity UI classes have one obvious stylesheet home.

6. Extract markdown, code, and page slices
   - Audit IDs: G3, G4, Q1, V1, V2.
   - Expected changes: create `08-markdown.css` for `.markdown-content`, tables, headings, inline/block code, `pre`, `.code-block`, and `.code-copy-button`; create `09-pages.css` for `.auth-*`, `.settings-*`, `.settings-panel`, `.not-found`, `.section-header`, `.panel`, `.meta-list`, `.model-*`, and `.agent-profile-*`.
   - Validation: grep consumers in `markdown.rs`, `auth.rs`, `app.rs`; Trunk build; wasm check.
   - Exit condition: markdown rendering and standalone pages keep their current styling without relying on stale v1 positioning.

7. Move responsive rules last and run final audit
   - Audit IDs: G1-G4, Q1-Q2, V1-V2, N1.
   - Expected changes: create `10-responsive.css`; move and deduplicate all media queries from `crates/oxide-agent-web-ui/src/styles.css:1817` and `crates/oxide-agent-web-ui/src/styles.css:3179`; remove temporary coarse files if checkpoint 1 used them; update this goal document with evidence.
   - Validation: `cargo fmt` if any Rust/HTML changed; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `env -u NO_COLOR trunk build --release`; `git diff --check`; `git diff --name-only`; representative grep checks from Completion Audit.
   - Exit condition: every Completion Audit item is verified and the final CSS tree is ready for checkpoint commit.

## Target Slice Layout

```text
crates/oxide-agent-web-ui/src/styles.css
crates/oxide-agent-web-ui/src/styles/
  00-tokens.css
  01-reset.css
  02-primitives.css
  03-shell.css
  04-chat.css
  05-composer.css
  06-activity.css
  07-metrics.css
  08-markdown.css
  09-pages.css
  10-responsive.css
```

Temporary coarse files are allowed only during checkpoint 1 if they make the first move lossless. They must be removed before completion.

## Validation Contract

- Static checks:
  - `git diff --check`
  - `cargo fmt --check` only if Rust, `Cargo.toml`, or generated formatting-sensitive files are touched; otherwise `cargo fmt` is not required for CSS-only changes.
- Web UI compile/build:
  - `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`
  - `env -u NO_COLOR trunk build --release` from `crates/oxide-agent-web-ui/`
- Artifact/review checks:
  - `wc -l crates/oxide-agent-web-ui/src/styles.css`
  - `find crates/oxide-agent-web-ui/src/styles -maxdepth 1 -type f | sort`
  - `rg "FRONT TEMPLATE REDESIGN OVERRIDE" crates/oxide-agent-web-ui/src/styles.css crates/oxide-agent-web-ui/src/styles`
  - Representative class-owner greps for `.composer`, `.activity-drawer`, `.tool-card`, `.markdown-content`, `.settings-page`, `.metrics-group`.
- Done when: every Completion Audit item is verified, the final slice layout exists, no stale v2 override block remains, and diff review confirms CSS-only maintainability changes within scope.

## Decisions

- 2026-06-06: Use `docs/goals/2026-06-06-web-ui-css-v2-slice-plan.md` because this repo stores durable goal docs under `docs/goals/`.
- 2026-06-06: Treat the redesign/v2 rules as the source of truth. v1/MVP rules are candidates for deletion only after confirming they are duplicated or unused.
- 2026-06-06: Prefer plain CSS slices and the existing Trunk stylesheet entrypoint. Do not add CSS modules, preprocessors, design-token generators, JS tooling, or new dependencies.
- 2026-06-06: First implementation step is a lossless scaffold split that preserves cascade order before deduplicating v1/v2 rules. This minimizes visual-regression risk and validates Trunk import behavior early.
- 2026-06-06: Use temporary checkpoint-1 coarse slices `00-v1-base.css` and `10-v2-current.css` to preserve exact original cascade before later v2-first deduplication.
- 2026-06-06: Promote base styles by ownership first, not by aggressive selector merging. Keep legacy base coverage inside the relevant base slice before the current v2 rule subset to minimize visual-regression risk while removing the late `FRONT TEMPLATE REDESIGN OVERRIDE` block.
- 2026-06-06: For shell/navigation, merge the computed legacy-plus-v2 result into one canonical shell slice instead of keeping adjacent v1/v2 override blocks. Leave responsive media queries for checkpoint 7, where all responsive rules are moved and deduplicated together.

## Progress Log

- 2026-06-06: Goal document created from stylesheet recon.
  - Changed: Added this goal contract, Completion Audit, target slice layout, and checkpoint plan.
  - Evidence: Existing docs convention found under `docs/goals/`; stylesheet entrypoint confirmed at `crates/oxide-agent-web-ui/index.html:11`; monolithic stylesheet section boundaries mapped, including v1 base at `crates/oxide-agent-web-ui/src/styles.css:1` and v2 override base at `crates/oxide-agent-web-ui/src/styles.css:1840`.
  - Commands: `git status --short`; `git diff --check`.
  - Audit IDs updated: none.
  - Next: Checkpoint 1 — create a lossless slice scaffold while preserving cascade order.

- 2026-06-06 19:06: Checkpoint 1 lossless CSS slice scaffold.
  - Changed: Reduced `crates/oxide-agent-web-ui/src/styles.css` to an ordered import entrypoint; split the original stylesheet into `crates/oxide-agent-web-ui/src/styles/00-v1-base.css` and `crates/oxide-agent-web-ui/src/styles/10-v2-current.css` at the v2 boundary without semantic CSS edits.
  - Evidence: `wc -l crates/oxide-agent-web-ui/src/styles.css` reports 3 lines; `find crates/oxide-agent-web-ui/src/styles -maxdepth 1 -type f | sort` lists `00-v1-base.css` and `10-v2-current.css`; concatenating the two slices matches `HEAD:crates/oxide-agent-web-ui/src/styles.css`; Trunk and wasm checks passed.
  - Commands: `git diff --check`; `env -u NO_COLOR trunk build --release`; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `python` content-equivalence check against `git show HEAD:crates/oxide-agent-web-ui/src/styles.css`.
  - Audit IDs updated: G1 in progress; V1 verified; V2 verified; Q1, Q2, and N1 preserved by CSS-only scaffold diff.
  - Next: Checkpoint 2 — promote v2 tokens, reset, and primitives to canonical base, replacing the temporary override-layer model incrementally.

- 2026-06-06 19:20: Checkpoint 2 v2 base-slice promotion.
  - Changed: Replaced the temporary `00-v1-base.css` import with base slices `00-tokens.css`, `01-reset.css`, `02-primitives.css`, moved remaining v1 component/page coverage to `03-v1-legacy.css`, and trimmed `10-v2-current.css` to the current v2 component layer.
  - Evidence: `wc -l crates/oxide-agent-web-ui/src/styles.css` reports 6 lines; `find crates/oxide-agent-web-ui/src/styles -maxdepth 1 -type f | sort` lists the three base slices plus `03-v1-legacy.css` and `10-v2-current.css`; `rg "FRONT TEMPLATE REDESIGN OVERRIDE" crates/oxide-agent-web-ui/src/styles.css crates/oxide-agent-web-ui/src/styles` returns no matches; compatibility token grep confirms `--fg-muted`, `--danger`, `--accent-dim`, `--border-hover`, `--input-min-height`, and `--drawer-width` remain defined/used; Trunk and wasm checks passed.
  - Commands: `git diff --check`; `env -u NO_COLOR trunk build --release`; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `git diff -- Cargo.toml crates/oxide-agent-web-ui/Cargo.toml package.json pnpm-lock.yaml`; focused `rg` checks for compatibility tokens and primitive selectors.
  - Audit IDs updated: G1 in progress; G2 in progress; G3 in progress; V1 verified; V2 verified; Q1, Q2, and N1 preserved by CSS-only/dependency-free diff.
  - Next: Checkpoint 3 — extract shell and navigation ownership from `03-v1-legacy.css` and `10-v2-current.css` into `03-shell.css`.

- 2026-06-06 22:30: Checkpoint 3 shell/navigation extraction.
  - Changed: Added `crates/oxide-agent-web-ui/src/styles/03-shell.css`; moved and collapsed canonical shell/sidebar/session navigation/topbar/status badge rules out of `03-v1-legacy.css` and the leading shell block of `10-v2-current.css`; updated `crates/oxide-agent-web-ui/src/styles.css` to import the shell slice after base primitives.
  - Evidence: `wc -l crates/oxide-agent-web-ui/src/styles.css` reports 7 lines; `find crates/oxide-agent-web-ui/src/styles -maxdepth 1 -type f | sort` lists `03-shell.css`; anchored shell-selector grep found no non-responsive shell selector ownership left in `03-v1-legacy.css` or `10-v2-current.css`; consumer grep confirms active shell classes in `components.rs` and `sessions.rs`; Trunk and wasm checks passed.
  - Commands: `git diff --check`; `env -u NO_COLOR trunk build --release`; `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`; `git diff -- Cargo.toml crates/oxide-agent-web-ui/Cargo.toml package.json pnpm-lock.yaml`; focused `rg` checks for shell selectors and shell class consumers.
  - Audit IDs updated: G1 in progress; G2 in progress; G3 in progress; G4 in progress; V1 verified; V2 verified; Q1, Q2, and N1 preserved by CSS-only/dependency-free diff.
  - Next: Checkpoint 4 — extract chat, message, and composer ownership into `04-chat.css` and `05-composer.css`.

## Risks and Blockers

- Trunk/local CSS import behavior may differ from browser-relative expectations.
  - Impact: A sliced entrypoint may fail to bundle or may reference missing CSS files at runtime.
  - Evidence: `crates/oxide-agent-web-ui/index.html:11` currently points at a single CSS asset.
  - Mitigation: Validate the first scaffold with `env -u NO_COLOR trunk build --release` before any semantic deduplication; if needed, keep one Trunk entrypoint and adjust only the entrypoint mechanism.
  - Audit IDs affected: G1, V1.

- Removing v1 rules too early could drop styling for less-used pages or panels.
  - Impact: Settings, metrics, code-copy, auth, or utility states may regress even if the main chat view looks correct.
  - Evidence: Useful v1-only areas exist at `crates/oxide-agent-web-ui/src/styles.css:1588`, `crates/oxide-agent-web-ui/src/styles.css:1728`, and `crates/oxide-agent-web-ui/src/styles.css:1755`.
  - Mitigation: Preserve v1-only coverage until consumer greps and visual ownership are reviewed in checkpoints 5 and 6.
  - Audit IDs affected: G4, Q1.

- Duplicate selectors may hide accidental behavior changes during deduplication.
  - Impact: A canonical v2 rule may miss a legacy property that still affects layout or accessibility.
  - Evidence: The current file relies on late v2 overrides after line `1840` rather than a single canonical definition.
  - Mitigation: Deduplicate by slice, not globally; review high-risk selectors in each checkpoint; keep validation small and frequent.
  - Audit IDs affected: G2, G3, Q1.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
