# Goal: Browser Live RECON hardening

Date started: 2026-06-18
Status: active
Codex goal: Iterate on Browser Live according to the RECON plan: verify live contracts, fix SPA input, fresh navigation, strict action schema, DOM extraction, docs, validate every checkpoint, and create a separate git commit after each completed checkpoint.
Source spec: user-provided v6 OTS evidence report and RECON review request
Goal doc owner: Codex
Last updated: 2026-06-18 02:05

## Objective

Harden Browser Live after the v6 OTS automation evidence so SPA interaction failures become architecturally impossible instead of documented workarounds. Browser Live must provide reliable semantic input, sidecar-owned fresh navigation, strict action schemas, deterministic DOM value extraction, accurate documentation, and validated live evidence.

Done when every required Completion Audit item is verified by its listed evidence, every checkpoint has an individual git commit, and the final OTS browser flow proves create → extract share URL → fresh reveal → second-consumption verification without relying on ad-hoc page JavaScript hacks for normal DOM values.

## Scope

In scope:
- `docker/chrome-agent-sidecar.py` — sidecar live contract, SPA-safe input dispatch, fresh navigation behavior, DOM snapshot/extraction internals.
- `crates/oxide-agent-core/src/agent/providers/browser_live/types.rs` — typed request/response/action contracts.
- `crates/oxide-agent-core/src/agent/providers/browser_live/actions.rs` — action planning and timeout/schema semantics.
- `crates/oxide-agent-core/src/agent/providers/browser_live/tools.rs` — tool schemas, extraction behavior, post-action observations, tests.
- `crates/oxide-agent-core/src/agent/providers/browser_live/test_support.rs` — fake sidecar support for contract tests.
- `crates/oxide-agent-core/src/agent/prompt/composer.rs` — only if prompt guidance must reflect a changed tool contract.
- `docs/browser-live.md` and this goal doc.

Out of scope:
- Reintroducing MiMo, `browser_step`, parser/recovery decision loops, or non-vision fallback control.
- New crates, services, queues, storage backends, or browser automation engines.
- Changes to unrelated providers, transports, storage, SSH, sandbox, reminders, or web UI unless a type contract change proves a direct compile-time dependency.
- Teaching the model workaround recipes as the primary fix when the tool/sidecar can own the contract.

## Missing Inputs

- None for CP-0: the docker `oxide_chrome_agent_sidecar` container is running and accepts authenticated local REST probes from inside the container.

## Repository Context

- Relevant entry points:
  - `docker/chrome-agent-sidecar.py` REST adapter over `chrome-agent --json pipe`.
  - `crates/oxide-agent-core/src/agent/providers/browser_live/client.rs` typed sidecar trait/client.
  - `crates/oxide-agent-core/src/agent/providers/browser_live/tools.rs` public browser tools and tool JSON schemas.
  - `crates/oxide-agent-core/src/agent/providers/browser_live/types.rs` serde action/request/response contracts.
  - `docs/browser-live.md` user-facing setup/usage documentation.
- Existing conventions:
  - Browser Live is feature-gated under `tool-browser-live` and registered through capability modules.
  - Rust contracts are explicit serde structs/enums; sidecar returns stable JSON envelopes.
  - Checkpoints should be small and committed separately.
- Dependencies or runtime assumptions:
  - Live validation needs the docker `chrome-agent-sidecar` service with `chrome-agent` available in the container.
  - Local host currently may not have `chrome-agent`; local py_compile/unit tests remain useful but insufficient for live audit items.
- Validation infrastructure:
  - `python3 -m py_compile docker/chrome-agent-sidecar.py`
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib`
  - `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test`
  - Live OTS REST/tool run against `https://ots.bash.md/`
- Risky areas:
  - CDP listener threading and response draining.
  - SPA hash routing and one-time secret state caching.
  - Framework-controlled input value tracking (React/Vue/Angular/native setters).
  - Tool schema compatibility for existing `BrowserAction` variants.

## Completion Audit

### G1: Live contracts verified before design/code
- Source: RECON CP-0 and P0.5 verification requirement.
- Acceptance: current live behavior is captured with raw payloads for `type_text` vs `fill`, `force_reload`, `about:blank` recovery, and `browser_extract` DOM value semantics before depending on those contracts.
- Evidence required: commands/scripts used, raw response summaries with fields/status/timings, and classification of unavailable runtime if sidecar cannot run.
- Status: verified
- Evidence collected: CP-0 live probes ran inside `oxide_chrome_agent_sidecar` against `https://ots.bash.md/` without printing the bearer token. Health returned `{"ok":true,"chrome_agent_available":true,"chrome_agent_status":"stopped","auth_configured":true,...}` and `chrome-agent-sidecar --self-test` returned `chrome-agent-sidecar self-test ok`. `type_text` on `#createSecretData` returned `ok:true`, `status:"executed"`, then JS state reported `{"value":"cp0 compact type_text ...","disabled":false}`; submit created a share URL (`share_present:true`, `share_hash_len:59`). `fill` returned the same successful shape and created a share URL (`share_present:true`, `share_hash_len:59`). `navigate` to the generated share URL with `force_reload:true` returned `ok:true`, `navigation.status:"loaded"`, `force_reload:true`, final URL preserved the redacted OTS hash, DOM snapshot length 9, and waiting for `button.btn-success` succeeded. Native sidecar `/goto` to `about:blank` returned `ok:false`, `status:"blocked"`; subsequent native `/goto` back to `https://ots.bash.md/` returned `ok:true`, DOM snapshot length 12, and waiting for `#createSecretData` succeeded. Current `browser_extract` DOM baseline is code-owned rather than a sidecar REST endpoint: `extract_from_dom` builds an `execute_javascript` expression and currently echoes requested `attribute` while returning all properties; existing test `browser_extract_dom_returns_js_result_elements` verifies returned `value`/`innerText` but not exact attribute selection.

### G2: SPA-safe semantic input
- Source: v6 problem: `type_text` returns technical success but does not reliably trigger SPA state updates.
- Acceptance: `fill` and `type_text` share one sidecar-owned semantic value-setting primitive that uses native value setters where required, dispatches framework-visible events, returns final value diagnostics, and works on OTS without extra JS hacks.
- Evidence required: sidecar/unit tests plus live OTS run where `type_text` alone enables submit and creates a secret.
- Status: verified
- Evidence collected: CP-1 replaced the split chrome-agent `fill` + sidecar dispatch sequence with one sidecar-owned semantic input eval for both `fill` and `type_text`. The primitive uses native value setters for `HTMLInputElement`, `HTMLTextAreaElement`, and `HTMLSelectElement`, dispatches `focus`/`focusin`/`beforeinput`/`input`/`change`/`keyup`, fails on final value mismatch, and returns non-value JSON diagnostics through `action_result.result`. Contract checks confirmed both public actions translate to exactly one eval command with native setter/event markers and do not echo `final_value`. Live OTS probe on the refreshed `oxide_chrome_agent_sidecar` returned `ok:true`: `type_text` diagnostic `{action:"type_text", tag:"textarea", value_matches:true, value_length:42, expected_length:42}`, JS state after input `{value_matches:true, disabled:false}`, submit succeeded, share URL present with hash length 59 and post DOM length 12; `fill` returned the same diagnostic shape with value length 37, submit succeeded, share URL present with hash length 59 and post DOM length 12.

### G3: Fresh navigation contract for hash SPAs
- Source: v6 problem: same-hash SPA navigation caches state; `location.reload()`/`about:blank` break DOM.
- Acceptance: `navigate { force_reload: true }` means sidecar-owned fresh document navigation preserving the full target URL/hash; it must not rely on page JS `window.location.reload(true)` as the correctness mechanism and must fail structurally if freshness cannot be guaranteed.
- Evidence required: code review of sidecar fresh navigation path, unit/fake tests for `force_reload`, and live OTS reveal in a fresh browser context after previous SPA state.
- Status: verified
- Evidence collected: CP-2 replaced the `force_reload` hash-SPA path in `docker/chrome-agent-sidecar.py` with sidecar-owned browser-process restart: `chrome-agent close` without `--purge` drops the page JS heap/in-memory SPA state while preserving profile data, the old pipe/listener is closed, a new pipe is installed for the same sidecar session, and the normal `goto` command opens the exact target URL including the hash. Code search/import contract check confirmed no `window.location.reload` / `location.reload(` remains in the sidecar source. Live chrome-agent contract probe with a temporary browser showed initial URL `https://ots.bash.md/#cp2-fragment-check`, marker `"stale"`, then `chrome-agent close` returned `ok:true`, reopening the same browser name and navigating to the same URL preserved the hash while marker became `null`. Live OTS sidecar probe on refreshed `oxide_chrome_agent_sidecar` set a marker before creating a secret, then `POST /sessions/{id}/goto` with `force_reload:true` to the share URL returned `ok:true`, `navigation.status:"loaded"`, final hash length 59, DOM snapshot length 9, marker `null`, reveal button present, and browser reveal recovered the original secret (`recovered_matches:true`).

### G4: Strict `BrowserAction` tool schema
- Source: v6 problem: `wait` requires `timeout_ms`, but loose schema permits/encourages wrong `ms` field.
- Acceptance: `browser_execute` exposes a strict nested `oneOf`/equivalent schema for all public `BrowserAction` variants with required fields, ranges, and `additionalProperties:false`; no `ms` alias is added.
- Evidence required: unit tests inspecting tool schema for representative variants and absence of legacy/alias fields.
- Status: verified
- Evidence collected: CP-3 replaced the prose-only `browser_execute.action` schema with a strict `oneOf` schema covering all public `BrowserAction` variants. Each variant requires a literal `kind`, declares its required fields, bounds numeric fields, and sets `additionalProperties:false`; `wait` exposes only `timeout_ms` with range `1..60000` and has no `ms` property or alias. `script.steps` now has an item schema for direct action steps. Rust deserialization for `BrowserAction` now uses `deny_unknown_fields`, so providers that do not enforce the schema still cannot silently drop unknown nested fields. Tests inspect representative schema variants (`wait`, `navigate`, `fill`, `script`) and verify that `BrowserAction` rejects `wait.ms`/unexpected fields.

### G5: Deterministic DOM value/attribute extraction
- Source: v6 problem: share URL requires JS `querySelector(...).value` workaround.
- Acceptance: normal DOM values and attributes are extracted through `browser_extract`/typed action contract, with `attribute:"value"` returning the requested property deterministically and OTS share URL extracted without ad-hoc JavaScript.
- Evidence required: unit tests for DOM extraction shape/attribute selection and live OTS share URL extraction through `browser_extract`.
- Status: pending
- Evidence collected:

### G6: Post-action observations remain current and diagnosable
- Source: v6 improvement and remaining low priority note that `execute_javascript` DOM mutations may not refresh snapshots.
- Acceptance: every state-changing successful action returns a fresh post-observation with DOM snapshot or structured DOM snapshot error; read-only actions clearly report result-only behavior.
- Evidence required: unit tests for `execute_javascript`/script post-observation behavior and code review of action category handling.
- Status: pending
- Evidence collected:

### G7: Browser-based one-time verification
- Source: v6 problem: one-time verification uses direct API because browser navigation reuses SPA state.
- Acceptance: final validation proves first browser reveal returns the secret and a second browser fresh-context attempt reports consumed/missing state; direct API verification may be corroborating but not the only browser-live evidence.
- Evidence required: live OTS transcript with share URL redacted only if necessary, first reveal value match, second attempt consumed/error state, and network/console summaries.
- Status: pending
- Evidence collected: CP-2 provided partial browser evidence for the first fresh reveal: after create/extract via the sidecar, same-session `force_reload:true` reopened a fresh browser process at the share URL with no stale JS marker and the reveal action recovered the original secret. The final second-consumption browser attempt and network/console summaries remain for CP-6.

### Q1: Static checks and tests pass at each code checkpoint
- Source: repo development practices.
- Acceptance: relevant targeted checks pass before each checkpoint commit; final gate includes broad static/test commands listed in Validation Contract.
- Evidence required: command outputs per checkpoint.
- Status: in_progress
- Evidence collected: CP-0 docs-only checkpoint ran `python3 -m py_compile docker/chrome-agent-sidecar.py` (pass) and `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live --quiet` (65 passed, 0 failed). CP-1 ran `python3 -m py_compile docker/chrome-agent-sidecar.py` (pass), a temp-dir Python import/contract check for `action_to_pipe_cmd` (pass after documenting the local `/var/lib/oxide-browser` permission issue), `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live --quiet` (65 passed, 0 failed), `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test` (pass), and the live OTS semantic input probe (pass). CP-2 ran `python3 -m py_compile docker/chrome-agent-sidecar.py` (pass), a temp-dir Python import/source contract check asserting no reload JS remains and hash-navigation classification works (pass), `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live --quiet` (65 passed, 0 failed), refreshed the live sidecar with an executable script, `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test` (pass), a raw chrome-agent close/reopen contract probe (pass), and a live OTS force-reload reveal probe (pass). CP-3 ran `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live --quiet` (67 passed, 0 failed), `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib --quiet` (1305 passed, 8 ignored, 0 failed), and `cargo fmt --all -- --check` (pass after applying `cargo fmt --all`).

### Q2: One commit per completed checkpoint
- Source: user instruction.
- Acceptance: every completed checkpoint has a separate git commit after validation and goal doc update.
- Evidence required: commit hashes recorded in Progress Log.
- Status: in_progress
- Evidence collected: CP-0 committed as `62ba7a7d docs(browser-live): add recon hardening goal`. CP-1 committed as `b951bc92 fix(browser-live): unify semantic input actions`. CP-2 committed as `0b103b89 fix(browser-live): make force reload fresh`. CP-3 committed as `036c5485 fix(browser-live): enforce strict action schema`.

### N1: Browser Live direct-control architecture preserved
- Source: existing completed direct-control goal and current repo invariants.
- Must preserve: no `browser_step`, no MiMo decision layer, no internal vision fallback, no broad transport/provider rewrites.
- Evidence required: code search and diff review before final completion.
- Status: pending
- Evidence collected:

## Implementation Plan

### CP-0: Verification skeleton and live contract baseline
- Audit IDs: G1, Q2.
- Expected changes:
  - Create this goal doc.
  - Check sidecar/docker availability.
  - Run or prepare live contract probes for `type_text`, `fill`, `force_reload`, `about:blank`, and DOM extraction.
  - Record raw facts before code design.
- Validation:
  - `python3 -m py_compile docker/chrome-agent-sidecar.py`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live`
  - live sidecar probe commands when runtime is available.
- Exit condition: live baseline facts or exact environment blocker are documented, and CP-0 is committed.

### CP-1: Unified SPA-safe input primitive
- Audit IDs: G2, Q1, Q2.
- Expected changes:
  - Replace divergent input JS with one sidecar primitive used by `fill` and `type_text`.
  - Use native element value setters and framework-visible event dispatch.
  - Return structured input diagnostics in action results where possible.
  - Add tests covering generated behavior and Rust/fake expectations.
- Validation:
  - `python3 -m py_compile docker/chrome-agent-sidecar.py`
  - targeted browser_live Rust tests
  - live OTS `type_text` create-secret check.
- Exit condition: `type_text` and `fill` have the same reliable semantic input behavior.

### CP-2: Sidecar-owned fresh navigation
- Audit IDs: G3, G7, Q1, Q2.
- Expected changes:
  - Redefine/implement `force_reload` as fresh document navigation owned by sidecar, not page JS reload.
  - Preserve full target hash and return structured failure if freshness cannot be guaranteed.
  - Add tests for `force_reload` propagation and failure/success shape.
- Validation:
  - sidecar py_compile/self-test
  - targeted Rust tests
  - live OTS fresh reveal after previous SPA state.
- Exit condition: same-session or sidecar-owned fresh-context hash navigation no longer needs `about:blank` or `location.reload()` workarounds.

### CP-3: Strict public action schema
- Audit IDs: G4, Q1, Q2.
- Expected changes:
  - Replace prose-only nested `action` schema with exact per-variant schema.
  - Include strict `wait.timeout_ms`, `navigate.force_reload`, `fill/type_text.selector/value`, and script step schemas.
  - Add schema inspection tests.
- Validation:
  - targeted Rust tests.
  - review generated tool definition payload.
- Exit condition: wrong fields such as `ms` are rejected by schema rather than handled as runtime surprises.

### CP-4: Deterministic DOM extraction
- Audit IDs: G5, Q1, Q2.
- Expected changes:
  - Make `browser_extract` honor requested DOM `attribute`/property exactly.
  - Return matched values with selector/tag/count diagnostics.
  - Update tests and documentation guidance.
- Validation:
  - targeted Rust tests.
  - live OTS share URL extraction via `browser_extract` with `attribute:"value"`.
- Exit condition: OTS share URL is available without raw `execute_javascript` querySelector hacks.

### CP-5: Observation freshness and diagnostics
- Audit IDs: G6, Q1, Q2.
- Expected changes:
  - Make action categories explicit.
  - Ensure state-changing actions return fresh post-observations or structured DOM snapshot errors.
  - Add tests for `execute_javascript` mutation behavior.
- Validation:
  - targeted Rust tests.
  - sidecar py_compile/self-test.
- Exit condition: post-action state is current or diagnostically failed, never silently stale/empty.

### CP-6: Docs and final OTS E2E
- Audit IDs: G7, Q1, Q2, N1.
- Expected changes:
  - Update `docs/browser-live.md` to current direct-control tools and SPA semantics.
  - Run final OTS browser E2E: create, extract, fresh reveal, second consumption.
  - Run final broad gate and code search for non-goals.
- Validation:
  - Final Validation Contract.
- Exit condition: every audit item is verified and final checkpoint committed.

## Validation Contract

- Static checks:
  - `python3 -m py_compile docker/chrome-agent-sidecar.py`
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
- Tests:
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib`
- Runtime/manual verification:
  - `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test`
  - live OTS create → extract share URL → fresh reveal → second consumption check.
- Artifact verification:
  - Goal doc progress log records commands, evidence, and commit hash per checkpoint.
  - Final diff/code search confirms non-goals preserved.
- Done when: all Completion Audit statuses are `verified`, no required live evidence is missing, and the session goal can be closed with concrete evidence.

## Decisions

- 2026-06-18: Do not add compatibility aliases like `wait.ms`; wrong nested action fields are a schema-contract bug and should be rejected by strict schema.
- 2026-06-18: Make strictness two-layered for BrowserAction inputs: public tool schema prevents malformed LLM calls where the provider enforces schemas, and Rust `deny_unknown_fields` prevents silent drift where the provider does not.
- 2026-06-18: Treat `force_reload` as an intent for fresh document state owned by sidecar, not permission for the LLM or page JS to choose `reload()`/`about:blank` workarounds.
- 2026-06-18: Keep `fill` and `type_text` public, but make them share the same semantic value-setting primitive so SPA correctness cannot diverge by action name.
- 2026-06-18: Implement `force_reload` as browser-process replacement without profile purge. This is stronger than same-document reload for SPA memory freshness, preserves cookies/local storage better than purge/new session, and keeps the target URL/hash owned by the sidecar rather than by page JavaScript.

## Progress Log

- 2026-06-18 00:00: CP-0 started — goal contract created.
  - Changed: added this goal doc with audit IDs, checkpoint plan, validation contract, and checkpoint commit policy.
  - Evidence: pending CP-0 runtime probes.
  - Commands: pending.
  - Audit IDs updated: G1, Q2 in progress.
  - Next: check sidecar/docker availability and run live contract probes or record exact environment blocker.

- 2026-06-18 00:03: CP-0 live contract baseline completed.
  - Changed: updated this goal doc with CP-0 evidence.
  - Evidence: sidecar container running; health/self-test pass; live OTS probes show current `type_text` and `fill` both enable submit and create a share URL, `force_reload:true` loads the redacted share URL and exposes the reveal button, native `/goto about:blank` is blocked but native recovery to OTS succeeds, and current `browser_extract` exact-attribute behavior remains a code-level gap for CP-4.
  - Commands: `docker ps --format '{{.Names}}' | sort`; `docker exec oxide_chrome_agent_sidecar sh -lc 'curl -fsS -H "Authorization: Bearer $BROWSER_AGENT_SIDECAR_TOKEN" http://127.0.0.1:8787/healthz && printf "\n" && chrome-agent-sidecar --self-test'`; `docker exec -i oxide_chrome_agent_sidecar python3 - <<'PY' ...`; `python3 -m py_compile docker/chrome-agent-sidecar.py`; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live --quiet`.
  - Audit IDs updated: G1 verified; Q1/Q2 in progress.
  - Next: review diff, commit CP-0, then start CP-1 input primitive hardening.

- 2026-06-18 00:05: CP-0 committed.
  - Changed: committed the goal doc and CP-0 baseline evidence.
  - Evidence: git commit `62ba7a7d docs(browser-live): add recon hardening goal`.
  - Commands: `git status --short`; `git diff -- docs/goals/2026-06-18-browser-live-recon-hardening.md`; `git add docs/goals/2026-06-18-browser-live-recon-hardening.md`; `git commit -m ...`.
  - Audit IDs updated: Q2 in progress.
  - Next: CP-1 semantic input hardening.

- 2026-06-18 01:05: CP-1 semantic input primitive completed.
  - Changed: `docker/chrome-agent-sidecar.py` now maps both `fill` and `type_text` to one sidecar semantic input eval using native element setters and framework-visible events; action results now include input diagnostics and fail on final value mismatch; sidecar self-test asserts the shared input contract.
  - Evidence: git commit `b951bc92 fix(browser-live): unify semantic input actions`. Live OTS probe on refreshed sidecar showed `type_text` alone set `#createSecretData`, enabled submit, created a share URL (`share_hash_len:59`), and returned diagnostic `value_matches:true` without `final_value`; `fill` showed the same behavior and diagnostic shape. The live refresh initially exposed a deployment-mode issue: `docker cp` wrote the script without executable permission and `tini` logged `exec /usr/local/bin/chrome-agent-sidecar failed: Permission denied`; this was repaired by copying a `0755` temp executable and confirmed by healthcheck/hash match before live validation.
  - Commands: `python3 -m py_compile docker/chrome-agent-sidecar.py`; `BROWSER_AGENT_ARTIFACT_DIR=/home/stfu/ai/Oxide-Agent/target/sidecar-artifacts-test BROWSER_AGENT_PROFILE_DIR=/home/stfu/ai/Oxide-Agent/target/sidecar-profiles-test python3 - <<'PY' ...`; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live --quiet`; `install -m 0755 docker/chrome-agent-sidecar.py target/chrome-agent-sidecar-live`; `docker cp target/chrome-agent-sidecar-live oxide_chrome_agent_sidecar:/usr/local/bin/chrome-agent-sidecar`; `docker restart oxide_chrome_agent_sidecar`; `docker exec oxide_chrome_agent_sidecar sh -lc 'ls -l /usr/local/bin/chrome-agent-sidecar && sha256sum /usr/local/bin/chrome-agent-sidecar && curl -fsS -H "Authorization: Bearer $BROWSER_AGENT_SIDECAR_TOKEN" http://127.0.0.1:8787/healthz'`; `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test`; `docker exec -i oxide_chrome_agent_sidecar python3 - <<'PY' ...`.
  - Audit IDs updated: G2 verified; Q1/Q2 in progress.
  - Next: review CP-1 diff, commit, then start CP-2 sidecar-owned fresh navigation.

- 2026-06-18 01:35: CP-2 sidecar-owned fresh navigation completed.
  - Changed: `docker/chrome-agent-sidecar.py` now handles `force_reload:true` by closing the managed browser without profile purge, closing the stale pipe/listener, creating a fresh pipe for the same sidecar session, and using normal `goto` for the exact target URL. Same-origin hash navigation without `force_reload` remains a lightweight hash update; the old `window.location.reload(true)` path was removed. Sidecar self-test now verifies close/reopen freshness by proving a page JS marker is gone while a hash URL is preserved.
  - Evidence: git commit `0b103b89 fix(browser-live): make force reload fresh`. Raw chrome-agent contract probe returned `close_stdout:{"ok":true,...}`, second `goto` preserved `https://ots.bash.md/#cp2-fragment-check`, and marker changed from `"stale"` to `null`. Live sidecar script SHA256 was `ed801dc26b3432bdd1b44ca602226d2cc831ac04da29d2afaccb33f3bf51570a`; live OTS probe set marker `"stale"`, created a share URL (`share_hash_len:59`), navigated to it with `force_reload:true`, got `status:"loaded"`, DOM snapshot length 9, marker `null`, reveal button present, and recovered the original secret (`recovered_matches:true`).
  - Commands: `docker exec oxide_chrome_agent_sidecar sh -lc 'chrome-agent --help | sed -n "1,160p"'`; `docker exec oxide_chrome_agent_sidecar sh -lc 'chrome-agent close --help | sed -n "1,120p"'`; `docker exec -i oxide_chrome_agent_sidecar python3 - <<'PY' ...` (raw close/reopen contract); `python3 -m py_compile docker/chrome-agent-sidecar.py`; `BROWSER_AGENT_ARTIFACT_DIR=/home/stfu/ai/Oxide-Agent/target/sidecar-artifacts-test BROWSER_AGENT_PROFILE_DIR=/home/stfu/ai/Oxide-Agent/target/sidecar-profiles-test python3 - <<'PY' ...`; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live --quiet`; `install -m 0755 docker/chrome-agent-sidecar.py target/chrome-agent-sidecar-live`; `docker cp target/chrome-agent-sidecar-live oxide_chrome_agent_sidecar:/usr/local/bin/chrome-agent-sidecar`; `docker restart oxide_chrome_agent_sidecar`; `docker exec oxide_chrome_agent_sidecar chrome-agent-sidecar --self-test`; `docker exec -i oxide_chrome_agent_sidecar python3 - <<'PY' ...` (live OTS force-reload reveal).
  - Audit IDs updated: G3 verified; G7 partial evidence; Q1/Q2 in progress.
  - Next: review CP-2 diff/status, commit checkpoint, then start CP-3 strict public action schema.

- 2026-06-18 02:05: CP-3 strict public action schema completed.
  - Changed: `browser_execute.action` now exposes a strict per-variant `oneOf` schema with literal `kind` values, required fields, numeric bounds, and `additionalProperties:false`; `BrowserAction` deserialization now rejects unknown variant fields so bad aliases are not silently ignored.
  - Evidence: git commit `036c5485 fix(browser-live): enforce strict action schema`. Schema tests inspect `wait`, `navigate`, `fill`, and `script` variants; deserialization tests reject `wait.ms` and unexpected fields.
  - Commands: `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib -- agent::providers::browser_live --quiet`; `cargo test -p oxide-agent-core --no-default-features --features profile-full --lib --quiet`; `cargo fmt --all`; `cargo fmt --all -- --check`; `git diff -- crates/oxide-agent-core/src/agent/providers/browser_live/types.rs crates/oxide-agent-core/src/agent/providers/browser_live/tools.rs`; `git status --short`.
  - Audit IDs updated: G4 verified; Q1/Q2 in progress.
  - Next: commit this ledger update, then start CP-4 deterministic DOM extraction.

## Risks and Blockers

- Live sidecar availability is confirmed for CP-0 but may still be transient in later checkpoints.
  - Impact: G2/G3/G5/G7 final live evidence depends on the docker sidecar and public OTS target remaining reachable.
  - Evidence: CP-0 health/self-test/live probes passed from `oxide_chrome_agent_sidecar`.
  - Mitigation or requested decision: rerun targeted live probes at each affected checkpoint; if runtime becomes unavailable, record exact command output and do not mark live audit items verified.
  - Audit IDs affected: G2, G3, G5, G7.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
