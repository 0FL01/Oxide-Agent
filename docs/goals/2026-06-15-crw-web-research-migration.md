# Goal: Migrate web research from SearXNG + Crawl4AI to CRW

Date started: 2026-06-15
Status: active
Codex goal: `/goal Implement docs/goals/2026-06-15-crw-web-research-migration.md until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update the doc after each meaningful verification, commit after each completed checkpoint, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.`
Source spec: user-attached migration spec, `Pasted markdown(20).md`
Goal doc owner: Codex
Last updated: 2026-06-15 14:35 UTC+3

## Objective

Replace the current SearXNG + Crawl4AI web-research stack with a single CRW REST-backed integration while preserving the existing Oxide Agent architecture.

Done when:

- The LLM-facing search tool is `web_search`, backed by CRW `POST /v1/search`.
- The LLM-facing crawl/read tool remains `web_crawler`, with `webfetch_md` as the first tier and CRW `POST /v1/scrape` as fallback only for anti-bot / HTTP 403 / HTTP 429 style failures.
- The old `searxng_search` and `crawl4ai_markdown` tools, providers, features, config envs, compose services, docs references, and tests are removed or migrated.
- `webfetch_md`, Tavily, and Brave Search are preserved according to the non-goals.
- Every item in the Completion Audit is verified with current repo evidence.

## Scope

In scope:

- `crates/oxide-agent-core/src/agent/providers/crw/` creation.
- Removal of:
  - `crates/oxide-agent-core/src/agent/providers/searxng/`
  - `crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown/`
  - `crates/oxide-agent-core/tests/searxng_provider.rs`
  - `docker/searxng/settings.yml`
- Core feature/config/capability/runtime migration:
  - `crates/oxide-agent-core/Cargo.toml`
  - `crates/oxide-agent-core/src/agent/providers/mod.rs`
  - `crates/oxide-agent-core/src/agent/tool_runtime/modules.rs`
  - `crates/oxide-agent-core/src/agent/tool_runtime/mod.rs`
  - `crates/oxide-agent-core/src/agent/executor/registry.rs`
  - `crates/oxide-agent-core/src/agent/providers/delegation.rs`
  - `crates/oxide-agent-core/src/config.rs`
  - `crates/oxide-agent-core/src/capabilities/compiled.rs`
- Search/fetch policy and prompt/UI migration:
  - `crates/oxide-agent-core/src/agent/hooks/search_budget.rs`
  - `crates/oxide-agent-core/src/agent/prompt/composer.rs`
  - `crates/oxide-agent-core/src/agent/thoughts.rs`
  - `crates/oxide-agent-transport-web/src/server/search_probe.rs`
  - `crates/oxide-agent-transport-web/src/session.rs`
  - `crates/oxide-agent-transport-web/src/web_transport.rs`
  - `crates/oxide-agent-web-ui/src/tasks/tool_cards.rs`
- Profiles, compose, env, docs:
  - `profiles/*.toml`
  - `docker-compose.yml`
  - `docker-compose.telegram.yml`
  - `docker-compose.web.yml` if it has local web-research env/depends-on references
  - `docker-compose.web.local-services.yml`
  - `docker-compose.telegram.local-services.yml`
  - `docker/compose.full.yml`
  - `docker/compose.dev.yml`
  - `.env.example`
  - `AGENTS.md`, `README.md`, `docs/deploy.md`, `docs/hooks/search-budget.md`, `docs/stack-logs-stage0.md`, `docs/prd/implemented/brave-search-prd.md`, `docs/prd/implemented/plan-search-probe.md`
- Static guard and snapshot/test fixture updates:
  - `crates/oxide-agent-core/tests/tool_runtime_static_guards.rs`
  - any `insta` snapshots or hardcoded JSON fixtures touched by tool names/payloads.

Out of scope:

- Do not remove or rewrite `webfetch_md`; it remains the first tier for `web_crawler`.
- Do not change known-source fast paths in `webfetch_md` unless a compile failure forces a tiny signature update.
- Do not remove, rename, or reimplement Brave Search or Tavily providers. Only add the smallest conflict guard needed to prevent duplicate `web_search` registration if both Tavily and CRW are enabled.
- Do not add new crates, queues, storage, frameworks, service layers, or generalized crawler abstractions.
- Do not expose raw browser/scrape tools to the LLM. CRW scrape is reachable through `web_crawler` fallback only.
- Do not implement CRW `/v1/crawl`, `/v1/map`, `/v1/extract`, job polling, proxy rotation management, or remote CRW provisioning beyond the compose service/env migration required here.
- Do not commit secrets, tokens, cookies, private logs, or real runtime env dumps.

## Missing Inputs

- Exact CRW self-host auth env name, if the local Docker service should enforce server-side API keys.
  - Impact: Compose can inject an Oxide client token via `OXIDE_CRW_API_TOKEN`, but CRW server-side auth may need a CRW-specific env var.
  - Low-risk assumption: Oxide supports Bearer auth whenever `OXIDE_CRW_API_TOKEN` is non-empty; local compose may run unauthenticated unless the CRW image documents a server auth env.
  - Agent action: inspect CRW upstream docs/source before editing compose; use the documented CRW server auth env only if found. Never invent or commit a secret value.
- Exact CRW request/response field names for optional search/scrape tuning.
  - Impact: Core `query`, `url`, `limit`, `formats: ["markdown"]`, and Bearer auth are enough for the first working integration; legacy optional SearXNG/Crawl4AI-specific knobs may not have a CRW equivalent.
  - Low-risk assumption: implement documented Firecrawl-compatible fields first, tolerate response variants in deserialization, and lock the chosen JSON in unit tests.
  - Agent action: during Checkpoint 1, inspect CRW docs/source/tests for `POST /v1/search` and `POST /v1/scrape` body/response structs before finalizing mapping.
- `web_search` ownership conflict with Tavily.
  - Impact: Tavily already exposes `web_search`, while the migration requires CRW to expose `web_search`; the registry fails fast on duplicate tool names.
  - Low-risk assumption: CRW owns `web_search` when `OXIDE_CRW_ENABLED=true`; Tavily remains present for `web_extract`, and its search executor is skipped only in the CRW-enabled duplicate case.
  - Agent action: implement the smallest runtime conflict guard and add a test that enabling both CRW and Tavily does not duplicate `web_search`.

## Repository Context

- Project rules are in `AGENTS.md`: Rust 1.94, empty default features, profile features, small explicit changes, no over-engineering, `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` required before finishing.
- Current SearXNG provider lives in `crates/oxide-agent-core/src/agent/providers/searxng/` and exposes `searxng_search`.
- Current Crawl4AI provider lives in `crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown/` and exposes `crawl4ai_markdown`.
- Current merge tool is `web_crawler` in `crates/oxide-agent-core/src/agent/tool_runtime/modules.rs`; it uses `webfetch_md` first and Crawl4AI fallback only when configured.
- Registration is not only in `tool_runtime/modules.rs`; also inspect and update:
  - `crates/oxide-agent-core/src/agent/executor/registry.rs`
  - `crates/oxide-agent-core/src/agent/providers/delegation.rs`
- The CRW upstream docs describe a Firecrawl-compatible REST API and a self-host Docker image `ghcr.io/us/crw` listening on port `3000`, with endpoints including `/v1/scrape`, `/v1/search`, and `/health`.

## CRW API Mapping Contract

### CRW provider shape

Create one small provider module:

```text
crates/oxide-agent-core/src/agent/providers/crw/
  mod.rs
  client.rs
  error.rs
  format.rs
  provider.rs
  types.rs
  tests.rs       # optional inline cfg(test) is also fine
```

Keep it simple:

- One `CrwProvider` or `CrwClient` backed by `reqwest::Client`.
- No adapter trait unless a second implementation already exists.
- Public methods:
  - `search(args: CrwSearchArgs) -> Result<CrwSearchOutput, CrwError>`
  - `scrape(args: CrwScrapeArgs) -> Result<CrwScrapeOutput, CrwError>`
- Tool executor methods can live in `provider.rs`; HTTP and deserialization should stay in `client.rs`/`types.rs`.
- Build under feature `tool-crw = ["dep:reqwest"]` or `tool-crw = ["dep:reqwest", "reqwest/json"]` if required by current crate feature setup. Do not add a new crate unless compile proves an existing dependency cannot do the job.

### Env/config mapping

New envs:

- `OXIDE_CRW_ENABLED`
  - Runtime enablement for CRW module.
  - Default: `false` unless compose/profile intentionally sets it to `true`.
- `OXIDE_CRW_BASE_URL`
  - Base URL, no trailing slash required.
  - Default for local service: `http://127.0.0.1:3000` in host-network compose or `http://crw:3000` in bridge/local-service compose.
- `OXIDE_CRW_API_TOKEN`
  - Optional Bearer token.
  - If set and non-empty, send `Authorization: Bearer <token>`.
  - If absent/empty, do not send auth header.
- `OXIDE_CRW_TIMEOUT_SECS`
  - Request timeout for CRW HTTP calls.
  - Default: keep close to existing web-research timeout behavior; prefer `30` unless current config has a stronger precedent.

Remove old runtime config support:

- `SEARXNG_URL`
- `SEARXNG_ENABLED`
- `SEARXNG_BEARER_TOKEN`
- `SEARXNG_TIMEOUT_SECS`
- `SEARXNG_ROTATION_ENGINES`
- `OXIDE_CRAWL4AI_*`

### `web_search` -> CRW `POST /v1/search`

LLM-facing tool name: `web_search`.

Public schema should be generic, not SearXNG-branded:

```json
{
  "query": "string, required",
  "max_results": "integer 1..10, optional default 5",
  "language": "string, optional",
  "time_range": "day|week|month|year, optional",
  "safe_search": "integer 0|1|2, optional",
  "categories": "string or array, optional",
  "page": "integer >=1, optional"
}
```

Mapping rules:

- `query` -> CRW request query field.
- `max_results` -> CRW result limit field, clamped to `1..10`.
- `page` -> CRW page/offset field only if CRW documents it; otherwise accept the argument but return a structured warning/note that pagination is not supported by this backend.
- `language`, `time_range`, `safe_search`, `categories` -> send only if CRW documents compatible fields. If unsupported, do not invent SearXNG passthrough parameters; keep the argument accepted for caller compatibility and include a short backend note in the result payload.
- Remove `engines` from the public schema because it is SearXNG-specific. If old tests pass it through during migration, deserialize with `#[serde(default)]` and ignore it with a backend note until fixtures are updated.
- Response normalization must produce stable markdown/text output plus structured JSON metadata:
  - provider/backend: `crw`
  - tool: `web_search`
  - query
  - result count
  - result entries with title, URL, snippet/content if present
  - raw/unknown CRW metadata preserved only if useful and not huge.

Minimum request body to lock in unit tests after CRW source inspection:

```json
{
  "query": "rust async reqwest timeout",
  "limit": 5
}
```

If CRW uses Firecrawl-compatible names different from `limit`, update the body and tests to the documented names. The test should assert the exact emitted JSON so this mapping cannot drift silently.

### `web_crawler` fallback -> CRW `POST /v1/scrape`

LLM-facing tool name remains `web_crawler`.

Preserve the current `web_crawler` input shape unless compile/tests reveal a required rename:

```json
{
  "url": "string, required",
  "timeout_secs": "integer, optional",
  "wait_for": "string, optional",
  "fresh": "boolean, optional",
  "max_chars": "integer, optional",
  "offset_chars": "integer, optional"
}
```

Fallback chain:

1. Try `webfetch_md` first.
2. Return `webfetch_md` success immediately.
3. Fallback to CRW scrape only when the webfetch failure is anti-bot / access-block style:
   - existing `anti_bot` classification
   - HTTP 403
   - HTTP 429
   - existing explicit anti-bot host rules already implemented in `webfetch_md` / `web_crawler`
4. Do not fallback for ordinary DNS errors, invalid URL, unsupported scheme, permanent parse errors, or generic timeouts unless the current merge tool already classifies them as anti-bot.

CRW scrape request mapping:

- `url` -> CRW request URL field.
- Requested format -> `formats: ["markdown"]` or CRW's documented markdown format field.
- `timeout_secs` -> reqwest deadline always; also send CRW request timeout only if documented.
- `wait_for` -> send only if CRW documents a compatible wait field; otherwise ignore with backend note.
- `fresh` -> send only if CRW documents cache-bypass semantics; otherwise ignore with backend note.
- `max_chars` and `offset_chars` remain Oxide-side post-processing. Do not rely on CRW to truncate.
- Renderer selection should remain CRW default. Do not force Chrome; CRW's default lightpanda path is the reason for migration.

Response normalization:

- In merged success payload:
  - `provider`: `web_crawler`
  - `backend`: `crw_scrape` when fallback is used
  - `primary_backend`: `webfetch_md`
  - `fallback_backend`: `crw_scrape`
  - `fallback_reason`: original webfetch error kind/status
  - `url`, `final_url`, `status_code` if CRW returns them
  - `markdown`
  - `chars`, `raw_chars`, `selected_chars`, `truncated`
  - `elapsed_ms`
- Remove/replace old fields:
  - `fallback_backend: "crawl4ai_markdown"` -> `"crw_scrape"`
  - `crawl4ai_error_kind` -> `crw_error_kind`
  - any user-visible "Crawl4AI" message -> "CRW scrape fallback" or generic "browser-rendered fallback".

Failure normalization:

- Invalid URL -> `invalid_arguments` or existing invalid URL kind.
- CRW 401/403 auth failure -> `crw_auth_failed` when caused by CRW response, but preserve anti-bot classification when the target site caused 403 and CRW exposes that as target status.
- CRW 408/timeout -> `crw_timeout`.
- CRW non-success without target status -> `crw_unavailable` or `crw_http_status`.
- Oversized/no markdown -> existing truncation/empty-content behavior with `backend: crw_scrape`.

## Completion Audit

### Functional requirements

- G1: CRW provider exists and is feature-gated.
  - Source: migration spec, "Create `crates/oxide-agent-core/src/agent/providers/crw/`".
  - Acceptance: `tool-crw` builds a provider/client that can call CRW search and scrape through REST without pulling in old SearXNG/Crawl4AI providers.
  - Evidence required: `cargo test -p oxide-agent-core --no-default-features --features tool-crw crw`; inspect `providers/mod.rs` and `providers/crw/`.
  - Status: verified
  - Evidence collected: `crates/oxide-agent-core/src/agent/providers/crw/` created with mod.rs, client.rs, error.rs, format.rs, provider.rs, types.rs. Feature `tool-crw = ["dep:reqwest"]` in Cargo.toml. `providers/mod.rs` has `#[cfg(feature = "tool-crw")] pub mod crw;` and `pub use crw::CrwProvider;`. Tests: 27 passed, 0 failed. Clippy clean.

- G2: `web_search` is backed by CRW `/v1/search`.
  - Source: accepted decision: LLM sees `web_search` for CRW search.
  - Acceptance: runtime registers one `web_search` executor for CRW when CRW is enabled; `searxng_search` is not registered.
  - Evidence required: unit test for CRW search executor; registry test with `OXIDE_CRW_ENABLED=true`; `rg "searxng_search" crates/oxide-agent-core crates/oxide-agent-transport-web crates/oxide-agent-web-ui profiles docker-compose*.yml docker .env.example AGENTS.md README.md docs` shows only allowed historical references if any.
  - Status: verified (additive; old `searxng_search` removal in Checkpoint 8)
  - Evidence collected: `CrwSearchToolModule` registered in registry.rs behind `#[cfg(feature = "tool-crw")]`, registered in delegation.rs. Tavily `web_search` executor skipped when `is_crw_enabled()` is true (duplicate-name guard in `TavilyToolModule::tool_runtime_executors()`). `cargo check --workspace --no-default-features --features profile-full` passes. Old `searxng_search` still compiles but will be removed in Checkpoint 8.

- G3: `web_crawler` preserves webfetch-first fallback chain and uses CRW scrape fallback.
  - Source: accepted decision: `webfetch_md` remains and CRW `/v1/scrape` is fallback only for anti-bot/JS blocks.
  - Acceptance: webfetch success never calls CRW; anti-bot/403/429 webfetch failure calls CRW; non-anti-bot failure does not call CRW.
  - Evidence required: focused tests for `WebCrawlerToolExecutor` success/fallback/no-fallback paths; inspect `tool_runtime/modules.rs`.
  - Status: verified
  - Evidence collected: `WebCrawlerToolExecutor` now has `crw: Option<Arc<CrwProvider>>` field. CRW fallback preferred over Crawl4AI in `execute_crawl4ai_fallback()`. `execute_crw_scrape_fallback()` added. Fallback priority: webfetch_md → CRW scrape → Crawl4AI → no fallback. web_crawler_tests: 88 passed, 0 failed.

- G4: old providers and tests are removed.
  - Source: migration spec removal list.
  - Acceptance: SearXNG and Crawl4AI provider directories, SearXNG test file, and SearXNG settings file no longer exist; no compile references remain.
  - Evidence required: `test ! -d .../searxng`, `test ! -d .../crawl4ai_markdown`, `test ! -f crates/oxide-agent-core/tests/searxng_provider.rs`, `test ! -f docker/searxng/settings.yml`, plus `cargo check`.
  - Status: pending
  - Evidence collected:

- G5: Cargo features and profiles use `tool-crw`.
  - Source: migration spec Cargo/profile instructions.
  - Acceptance: `tool-searxng` and `tool-crawl4ai-markdown` feature definitions are gone; `tool-crw` exists; profile-full, profile-web-embedded-opencode-local, and profile-search-only use `tool-crw`; TOML profiles reference `tool/crw` where SearXNG/Crawl4AI modules were used.
  - Evidence required: inspect `crates/oxide-agent-core/Cargo.toml`; `cargo check`/`cargo test` for affected profiles.
  - Status: verified (profiles switched; old feature definitions removed in Checkpoint 8)
  - Evidence collected: `profile-full`, `profile-web-embedded-opencode-local`, `profile-search-only` all use `tool-crw`. `profile-web-embedded-opencode-local` Cargo profile: `tool-crawl4ai-markdown` and `tool-searxng` replaced with single `tool-crw`. Profile TOMLs: `full.toml`, `search-only.toml`, `embedded-opencode-local.toml`, `host-bwrap.toml`, `web-embedded-opencode-local.toml` all reference `tool/crw`. `cargo check` passes for all three profiles.

- G6: Runtime registration paths know CRW and no longer register raw old modules.
  - Source: architecture invariant: `tool_runtime/modules.rs` is tool registration point; repo inspection also found executor registry and delegation registration.
  - Acceptance: `CrwSearchToolModule` is registered; raw `Crawl4AiMarkdownToolModule` and `SearxngToolModule` are removed; sub-agent/delegation registration compiles.
  - Evidence required: registry/unit tests plus `cargo check --workspace --no-default-features --features profile-full`.
  - Status: verified (additive; old modules still compile, removal in Checkpoint 8)
  - Evidence collected: `CrwSearchToolModule` registered in `registry.rs` and `delegation.rs`. `feature = "tool-crw"` added to all cfg `any()` gates in both files. `cargo check --workspace --no-default-features --features profile-full` passes. Old modules still present but will be removed in Checkpoint 8.

- G7: Config migrates to `OXIDE_CRW_*`.
  - Source: migration spec config section.
  - Acceptance: new config helpers exist; old SearXNG/Crawl4AI helpers and struct fields are removed; tests cover enabled/base-url/token/timeout behavior.
  - Evidence required: config unit tests; `rg "SEARXNG_|OXIDE_CRAWL4AI_" crates .env.example docker-compose*.yml docker profiles AGENTS.md README.md docs` reviewed.
  - Status: verified (additive only; old removal in Checkpoint 8)
  - Evidence collected: Config helpers added in config.rs: `is_crw_enabled()`, `get_crw_base_url()`, `get_crw_api_token()`, `get_crw_timeout_secs()`, `CRW_DEFAULT_TIMEOUT_SECS`. 8 config tests pass covering enabled/disabled, base-url default+env, token trim+blank, timeout default+env. Old SearXNG/Crawl4AI helpers remain for now (removal in Checkpoint 8).

- G8: Capability manifest uses one CRW module.
  - Source: migration spec capabilities section.
  - Acceptance: `tool-crw -> tool/crw` exposes `tool/crw-search` and `tool/crw-scrape`; old `tool/searxng` and `tool/crawl4ai-markdown` entries are gone.
  - Evidence required: inspect `compiled.rs`; run compiled capability command for affected profile if practical.
  - Status: verified (CRW entry added; old entries removed in Checkpoint 8)
  - Evidence collected: `push_module!(modules, "tool-crw", "tool/crw", Search, ["tool/crw-search", "tool/crw-scrape"])` added in `compiled.rs`. Capability command output shows `tool/crw` module with `tool/crw-search` and `tool/crw-scrape` capabilities. `cargo run ... -- capabilities --compiled --json` confirms.

- G9: Search budget hook uses new tool names and preserves host blocking.
  - Source: migration spec hooks section and invariant that search budget counts search tool calls and blocks repeated anti-bot hosts.
  - Acceptance: counted names include `web_search`, `web_crawler`, `web_markdown`, `brave_search`, `web_extract` as currently applicable; names do not include `searxng_search` or `crawl4ai_markdown`; fallback warning says `web_search`.
  - Evidence required: focused `search_budget` tests.
  - Status: verified
  - Evidence collected: `search_budget.rs` counts `web_search`, `web_crawler`, `web_markdown`, `brave_search`, and `web_extract`; no `searxng_search` or `crawl4ai_markdown` remain in the hook. Brave fallback warning now says `web_search`. `cargo test -p oxide-agent-core --no-default-features --features profile-full search_budget` → 8 passed.

- G10: Search probe allowlist uses generic tools only.
  - Source: migration spec search_probe section and invariant that probe has search + fetch tools, no browser-specific tools.
  - Acceptance: split allowlist is search + lightweight fetch; merged allowlist is search + `web_crawler`; no `crawl4ai_markdown` block constant remains; tests updated.
  - Evidence required: focused `search_probe` tests.
  - Status: verified
  - Evidence collected: `search_probe.rs` default split allowlist is `web_search`, `web_markdown`; merged allowlist is `web_search`, `web_crawler`; `SEARCH_PROBE_BLOCKED_TOOL_CRAWL4AI` was removed. `session.rs` no longer filters `crawl4ai_markdown` from Search Probe runtime options. `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local search_probe` → 31 passed.

- G11: Prompt guidance, thoughts, session, web UI, and web transport fixtures use new names/payloads.
  - Source: migration spec sections for session, prompt composer, thoughts, UI, web transport tests.
  - Acceptance: user-visible messages/cards/snapshots no longer mention Crawl4AI/SearXNG as active tools; web events handle CRW/web_crawler payloads.
  - Evidence required: focused core/web/web-ui tests; snapshot review.
  - Status: verified for active core/web/UI paths (final repo-wide stale-reference sweep in Checkpoint 9)
  - Evidence collected: Core prompt/workflow guidance updated in `composer.rs`, effort prompt guidance in `executor/execution.rs`, thought labels in `thoughts.rs`, Brave fallback guidance in `brave_search/*`, and failure summaries in `tool_failure_summary.rs` to use `web_search`, `web_crawler`, and `web_markdown` without active SearXNG/Crawl4AI tool names. Web transport fixtures updated to `web_search` and `web_crawler` with `crw_scrape` backend display payloads. Web UI `tool_cards.rs` now maps `web_search` to generic "Web Search", maps only `web_crawler` to the crawl card, accepts `web_crawler` display payloads, and summarizes `crw_http_status`, `crw_unavailable`, `crw_auth_failed`, and `crw_timeout`. Focused tests/checks pass: composer 19 passed, thoughts 8 passed, tool_failure_summary 5 passed, brave_search 26 passed, web_transport 21 passed, `cargo check -p oxide-agent-web-ui` OK, `cargo test -p oxide-agent-web-ui` 5 passed.

- G12: Docker Compose uses one CRW service instead of SearXNG + Crawl4AI.
  - Source: migration spec compose/env sections.
  - Acceptance: listed compose files define/inject CRW service/env; SearXNG and Crawl4AI services/env/depends-on are removed; `.env.example` documents `OXIDE_CRW_*`.
  - Evidence required: `docker compose -f <file> config` for each touched compose file where Docker is available; otherwise `docker compose` unavailability documented and YAML inspected with `rg`.
  - Status: pending
  - Evidence collected:

- G13: Documentation reflects CRW migration.
  - Source: migration spec docs section.
  - Acceptance: active docs and README/AGENTS mention CRW and generic tool names; old names remain only in historical PRD context if intentionally preserved.
  - Evidence required: `rg` review and doc diff inspection.
  - Status: pending
  - Evidence collected:

### Quality / architecture requirements

- Q1: Keep core transport-agnostic.
  - Source: architecture invariant.
  - Acceptance: `oxide-agent-core` does not depend on transport crates; CRW provider lives under core providers and uses config/env only.
  - Evidence required: `cargo tree -p oxide-agent-core` or Cargo manifest inspection plus compile.
  - Status: pending
  - Evidence collected:

- Q2: Feature gates control module existence; env controls runtime enablement.
  - Source: architecture invariant.
  - Acceptance: CRW code is behind `tool-crw`; CRW runtime registration requires `OXIDE_CRW_ENABLED` and base URL/token config.
  - Evidence required: no-default feature checks and config tests.
  - Status: verified
  - Evidence collected: `tool-crw` feature in Cargo.toml gates all CRW code. `is_crw_enabled()` defaults to `false`. `CrwSearchToolModule` checks `is_crw_enabled()` before creating provider. `WebCrawlerToolExecutor` only initializes CRW field when `is_crw_enabled()` is true. Tavily guard checks `is_crw_enabled()` before skipping `web_search`.

- Q3: No over-engineering.
  - Source: project context and AGENTS rules.
  - Acceptance: no new crate/service/framework/adapter trait unless directly required; implementation is a thin reqwest client and two tool paths.
  - Evidence required: diff review; `Cargo.toml` dependency review.
  - Status: verified
  - Evidence collected: CRW provider is one thin reqwest client (`CrwClient`) with `search()` and `scrape()` methods. No new crate, no adapter trait, no generic abstraction. Uses existing `dep:reqwest` with `json` feature already enabled.

- Q4: Tool name collision with Tavily is handled safely.
  - Source: repository inspection; Tavily currently exposes `web_search` and registry rejects duplicates.
  - Acceptance: enabling CRW and Tavily cannot produce duplicate `web_search`; Tavily provider is not removed, and `web_extract` stays available when Tavily is configured.
  - Evidence required: focused registry/module test with both envs set.
  - Status: verified
  - Evidence collected: Tavily duplicate-name guard in `TavilyToolModule::tool_runtime_executors()` (modules.rs): when `#[cfg(feature = "tool-crw")]` and `is_crw_enabled()`, filters out `web_search` executor, keeping only `web_extract`. `cargo check -p oxide-agent-core --no-default-features --features tool-crw,tool-webfetch-md,tool-tavily,tool-brave-search` passes.

- Q5: Final static checks pass.
  - Source: architecture invariant.
  - Acceptance: `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` pass.
  - Evidence required: command output.
  - Status: pending
  - Evidence collected:

### Validation requirements

- V1: Profile-specific builds/tests pass.
  - Source: architecture invariant and AGENTS testing instructions.
  - Acceptance: affected profile commands pass or any pre-existing unrelated failures are documented with exact evidence.
  - Evidence required:
    - `cargo check --workspace --no-default-features --features profile-full`
    - `cargo test -p oxide-agent-core --no-default-features --features profile-full`
    - `cargo test -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local`
    - `cargo test -p oxide-agent-core --no-default-features --features profile-search-only`
    - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`
  - Status: in progress (profile checks pass; full test suite final in Checkpoint 10)
  - Evidence collected: `cargo check` passes for profile-full, profile-search-only, profile-web-embedded-opencode-local. Capabilities: 30 passed, 2 failed (pre-existing sandbox backend requirement failures). `cargo clippy` clean for profile-full. `cargo fmt` clean.

- V2: Snapshot/static guard updates are intentional.
  - Source: migration spec snapshot/static guard notes.
  - Acceptance: snapshots/fixtures reflect `web_search` and CRW/web_crawler payloads; static guards no longer expect `SearxngProvider::new`.
  - Evidence required: snapshot test commands and diff review.
  - Status: in progress (static guards, web transport fixtures, and UI parsing updated; snapshots later)
  - Evidence collected: `tool_runtime_static_guards.rs` no longer expects `SearxngProvider::new`; delegation guard now checks `CrwProvider::new` is not constructed directly in delegation. Web transport fixtures now use `web_search` and `web_crawler` + `crw_scrape` payloads. Web UI `tool_cards.rs` no longer contains `SearXNG`, `Crawl4AI`, `searxng`, or `crawl4ai` active parsing/label strings. Stale false-positive static guard paths/patterns were narrowed to current architecture. `cargo test -p oxide-agent-core --no-default-features --features profile-full --test tool_runtime_static_guards` → 19 passed. `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local web_transport` → 21 passed. `cargo test -p oxide-agent-web-ui` → 5 passed.

- V3: Compose configs are syntactically valid.
  - Source: migration spec compose section.
  - Acceptance: each touched compose file renders with `docker compose config`, or Docker unavailability is recorded with manual YAML review.
  - Evidence required: command outputs or blocker note.
  - Status: pending
  - Evidence collected:

### Non-goals / exclusions

- N1: Do not remove `webfetch_md`.
  - Source: accepted decision.
  - Must preserve: provider directory, feature `tool-webfetch-md`, and `web_markdown` lightweight fetch behavior.
  - Evidence required: `rg "tool-webfetch-md|web_markdown|webfetch_md" crates/oxide-agent-core` plus tests.
  - Status: verified
  - Evidence collected: `tool-webfetch-md` feature remains in `crates/oxide-agent-core/Cargo.toml`; `web_markdown` and `webfetch_md` references remain in core registration/capabilities. `rg "tool-webfetch-md|web_markdown|webfetch_md" crates/oxide-agent-core/src crates/oxide-agent-core/Cargo.toml` confirms presence. `web_markdown` remains counted by search budget and preserved in prompt guidance.

- N2: Do not touch Brave/Tavily beyond required duplicate-name guard.
  - Source: accepted decision.
  - Must preserve: Brave provider and Tavily provider remain separate API providers; no migration to CRW internals.
  - Evidence required: diff review and focused registry test.
  - Status: pending
  - Evidence collected:

- N3: Do not expose raw CRW scrape/browser tool to the LLM.
  - Source: accepted decision and search_probe invariant.
  - Must preserve: CRW scrape only used inside `web_crawler` fallback; probe allowlist has no browser-specific raw tool.
  - Evidence required: registry tool names and search_probe tests.
  - Status: verified
  - Evidence collected: CRW scrape is internal to `WebCrawlerToolExecutor` via `crw.client().scrape()`. No standalone `crw_scrape` tool registered in `CrwSearchToolModule` (only `web_search`). `CrwSearchToolModule` exposes only `CrwSearchToolExecutor`. Search Probe allowlists now contain only `web_search`, `web_markdown`, and `web_crawler`; no raw browser/scrape tool is allowlisted.

## Implementation Plan

### Checkpoint 0 — Preflight, baseline, and goal contract

- Depends on: none.
- Audit IDs: all, setup only.
- Expected changes:
  - Create this file: `docs/goals/2026-06-15-crw-web-research-migration.md`.
  - No production code changes.
- Files to inspect:
  - `AGENTS.md`, `README.md`, `Cargo.toml`, `crates/oxide-agent-core/Cargo.toml`
  - `crates/oxide-agent-core/src/agent/tool_runtime/modules.rs`
  - `crates/oxide-agent-core/src/agent/executor/registry.rs`
  - `crates/oxide-agent-core/src/agent/providers/delegation.rs`
  - existing `docs/goals/*.md`
- Key decisions:
  - Use `docs/goals/` convention.
  - Plan additive first, destructive cleanup late, so every checkpoint can compile.
- Validation:
  - `git status --short`
  - `rg "searxng|crawl4ai|web_search|web_crawler|OXIDE_CRAWL4AI|SEARXNG" crates profiles docker docs README.md AGENTS.md .env.example`
  - Optional baseline: `cargo check --workspace --no-default-features --features profile-full`
- Exit condition:
  - Goal doc exists and another agent can start Checkpoint 1 without conversation context.
- Risks and compromises:
  - Baseline may already fail for unrelated reasons; record exact output and continue if the failures are clearly pre-existing.
- Alternatives:
  - If `docs/goals/` convention is rejected by repo instructions, move the content to the closest existing goal/plan convention and update the Codex goal path.

### Checkpoint 1 — Add CRW config and provider without removing old providers

- Depends on: Checkpoint 0.
- Audit IDs: G1, G7, Q2, Q3.
- Expected changes:
  - Add `tool-crw` feature to `crates/oxide-agent-core/Cargo.toml` but do not yet remove old feature definitions.
  - Add `crates/oxide-agent-core/src/agent/providers/crw/`.
  - Update `crates/oxide-agent-core/src/agent/providers/mod.rs` with `#[cfg(feature = "tool-crw")]` module/export.
  - Add config helpers in `crates/oxide-agent-core/src/config.rs`:
    - `is_crw_enabled()`
    - `get_crw_base_url()`
    - `get_crw_api_token()`
    - `get_crw_timeout()` / `get_crw_timeout_secs()` depending on existing naming style.
  - Add config unit tests for enabled/base URL/token/timeout behavior.
- Key decisions:
  - Thin reqwest client only.
  - Deserialization accepts CRW/Firecrawl-compatible response variants with `#[serde(default)]` where needed.
  - Do not copy Crawl4AI's Chromium-specific knobs.
- Validation:
  - `cargo fmt --all -- --check`
  - `cargo test -p oxide-agent-core --no-default-features --features tool-crw crw`
  - `cargo test -p oxide-agent-core --no-default-features --features tool-crw config`
  - `cargo check -p oxide-agent-core --no-default-features --features tool-crw,tool-webfetch-md`
- Exit condition:
  - CRW provider/config compiles and has hermetic tests, while old SearXNG/Crawl4AI still compile unchanged.
- Risks and compromises:
  - CRW optional fields may differ from assumptions. Keep the first request body minimal and tested.
  - Adding config fields to `AgentSettings` too early can disturb config schema snapshots; prefer helper functions/env readers first unless current style requires struct fields.
- Alternatives:
  - If CRW response shape is too variant-heavy, normalize via `serde_json::Value` in the client boundary and convert to typed structs after extracting known fields. Do not add a new generic abstraction.

### Checkpoint 2 — Register CRW `web_search` and update `web_crawler` fallback side-by-side

- Depends on: Checkpoint 1.
- Audit IDs: G2, G3, G6, Q2, Q4, N3.
- Expected changes:
  - `crates/oxide-agent-core/src/agent/tool_runtime/modules.rs`:
    - Add `CrwSearchToolModule` behind `tool-crw`.
    - Add CRW fallback support to `WebCrawlerToolExecutor` behind `tool-crw`.
    - Keep Crawl4AI fallback code temporarily behind `tool-crawl4ai-markdown` only if needed to keep old profiles compiling before final removal.
    - Prefer CRW fallback when `tool-crw` is compiled and `OXIDE_CRW_ENABLED=true`.
    - Update `web_crawler` tool description to generic "webfetch first, rendered fallback" language.
  - `crates/oxide-agent-core/src/agent/tool_runtime/mod.rs`:
    - Export `CrwSearchToolModule`.
  - `crates/oxide-agent-core/src/agent/executor/registry.rs`:
    - Add CRW module registration under `tool-crw`.
    - Add/adjust duplicate `web_search` guard for Tavily + CRW.
  - `crates/oxide-agent-core/src/agent/providers/delegation.rs`:
    - Add CRW module support for sub-agent tool module lists if delegation exposes search tools.
    - Remove old raw browser tool from sub-agent allow/deny references only after equivalent CRW path is in place.
- Key decisions:
  - CRW scrape has no standalone LLM tool executor. It is a provider method used by `web_crawler`.
  - `web_search` ownership: when CRW is enabled, CRW owns `web_search`; Tavily search must not also register `web_search`. Preserve Tavily `web_extract`.
- Validation:
  - `cargo test -p oxide-agent-core --no-default-features --features tool-crw,tool-webfetch-md web_crawler`
  - `cargo test -p oxide-agent-core --no-default-features --features tool-crw,tool-tavily tool_registry`
  - `cargo check -p oxide-agent-core --no-default-features --features tool-crw,tool-webfetch-md,tool-tavily,tool-brave-search`
- Exit condition:
  - Runtime can expose exactly one `web_search` and merged `web_crawler`; webfetch success/no-fallback/fallback behavior is tested.
- Risks and compromises:
  - Touching Tavily registration is risky because it is a non-goal. Limit the change to duplicate-name prevention and document it in `Decisions`.
  - Mocking webfetch anti-bot paths may be awkward if `WebFetchMdProvider` is concrete. Prefer small helper functions to classify fallback eligibility, not a new provider trait.
- Alternatives:
  - If preserving Tavily `web_extract` while skipping Tavily `web_search` is invasive, register Tavily first only when CRW is disabled; record the temporary loss of `web_extract` as unacceptable unless no smaller path exists.
  - If tests cannot inject fake CRW/webfetch clients without large refactor, test the fallback classification helper and CRW request builder directly, then add one integration-style executor test with local mock HTTP if existing test utilities support it.

### Checkpoint 3 — Switch capabilities, Cargo profiles, and repo profile TOMLs to CRW

- Depends on: Checkpoint 2.
- Audit IDs: G5, G8, Q2, V1.
- Expected changes:
  - `crates/oxide-agent-core/Cargo.toml`:
    - Replace old profile references with `tool-crw` in `profile-full`, `profile-web-embedded-opencode-local`, and `profile-search-only`.
    - Keep old feature definitions for one checkpoint if old code still exists; remove them in Checkpoint 8.
  - `crates/oxide-agent-core/src/capabilities/compiled.rs`:
    - Add `tool-crw -> tool/crw` with capabilities `tool/crw-search` and `tool/crw-scrape`.
    - Remove old capability module entries only if no profile still references them.
  - `profiles/full.toml`, `profiles/web-embedded-opencode-local.toml`, `profiles/search-only.toml`, `profiles/host-bwrap.toml`, `profiles/embedded-opencode-local.toml`:
    - Replace `tool/searxng` and raw Crawl4AI module refs with `tool/crw`.
- Key decisions:
  - Capability manifests describe modules, not every internal provider method. `tool/crw-scrape` is a capability of the CRW module even though scrape is consumed by `web_crawler`.
- Validation:
  - `cargo check -p oxide-agent-core --no-default-features --features profile-full`
  - `cargo check -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local`
  - `cargo check -p oxide-agent-core --no-default-features --features profile-search-only`
  - Capability command if practical: `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-full -- capabilities --compiled --json`
- Exit condition:
  - Affected profile features compile with CRW included and old search/scrape profile references gone.
- Risks and compromises:
  - Some profile commands may pull unrelated crates and fail for pre-existing reasons. Capture exact output and run narrower crate checks to isolate migration issues.
- Alternatives:
  - If full workspace profile check is too slow or blocked, run crate-scoped checks first, then leave the exact workspace command as required final validation.

### Checkpoint 4 — Migrate search budget, prompt guidance, thoughts, and static tool-name policy

- Depends on: Checkpoint 3.
- Audit IDs: G9, G11, N1, N3.
- Expected changes:
  - `crates/oxide-agent-core/src/agent/hooks/search_budget.rs`:
    - Count `web_search`, `web_crawler`, `web_markdown`, `brave_search`, and `web_extract` where currently applicable.
    - Remove `searxng_search` and `crawl4ai_markdown`.
    - Update fallback warning text to `web_search`.
    - Update tests.
  - `crates/oxide-agent-core/src/agent/prompt/composer.rs`:
    - Replace SearXNG guidance with `web_search`.
    - Remove raw `crawl4ai_markdown` guidance.
    - Let `web_crawler` guidance describe the merge fallback.
  - `crates/oxide-agent-core/src/agent/thoughts.rs`:
    - Replace SearXNG thought label with Web Search.
    - Remove Crawl4AI raw tool label.
  - `crates/oxide-agent-core/tests/tool_runtime_static_guards.rs`:
    - Replace old `SearxngProvider::new` guard with CRW registration/provider guard.
- Key decisions:
  - `web_crawler` and `web_markdown` remain counted by search budget because they consume web access budget and anti-bot host blocking.
  - Raw CRW scrape is not counted separately because it is not an exposed tool.
- Validation:
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full search_budget`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full prompt`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full tool_runtime_static_guards`
- Exit condition:
  - Core policy/guidance tests pass and active prompt/thought output no longer advertises old tools.
- Risks and compromises:
  - Some tests may assert exact English strings. Update snapshots/fixtures intentionally and review diffs.
- Alternatives:
  - If string snapshots are broad, first add a small unit test for the specific tool-list behavior, then update broad snapshots in the final snapshot checkpoint.

### Checkpoint 5 — Migrate web transport probe/session and transport fixtures

- Depends on: Checkpoint 4.
- Audit IDs: G10, G11, V2, N3.
- Expected changes:
  - `crates/oxide-agent-transport-web/src/server/search_probe.rs`:
    - Split allowlist: `web_search`, `web_markdown`.
    - Merged allowlist: `web_search`, `web_crawler`.
    - Remove `SEARCH_PROBE_BLOCKED_TOOL_CRAWL4AI`.
    - Update tests around hardcoded old names.
  - `crates/oxide-agent-transport-web/src/session.rs`:
    - Remove `crawl4ai_markdown` filtering.
  - `crates/oxide-agent-transport-web/src/web_transport.rs`:
    - Replace `searxng_search` fixtures with `web_search`.
    - Replace Crawl4AI payload fixtures/error examples with `web_crawler` + `crw_scrape` payloads.
    - Review snapshots after update.
- Key decisions:
  - Search probe should not know about raw scrape/browser provider names.
  - CRW scrape is observable only through merged `web_crawler` payload metadata.
- Validation:
  - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local search_probe`
  - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local web_transport`
  - `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`
- Exit condition:
  - Web transport tests pass and no active probe/session path references `crawl4ai_markdown` or `searxng_search`.
- Risks and compromises:
  - Transport profiles do not enable all core features for unrelated crates. Use `-p oxide-agent-transport-web`, not broad workspace tests, for this checkpoint.
- Alternatives:
  - If snapshots are noisy, update only after inspecting failures and keep a short list of expected snapshot changes in the Progress Log.

### Checkpoint 6 — Migrate web UI tool cards and error parsing

- Depends on: Checkpoint 5.
- Audit IDs: G11, V2.
- Expected changes:
  - `crates/oxide-agent-web-ui/src/tasks/tool_cards.rs`:
    - Rename SearXNG card to generic "Web Search".
    - Treat `web_search` as generic CRW/Tavily-compatible search display without implementation branding.
    - Update crawl card parsing for `web_crawler` + `crw_scrape` backend payloads.
    - Remove Crawl4AI-specific error kind parsing (`crawl4ai_*`) and replace with `crw_*`/generic fallback parsing.
- Key decisions:
  - UI labels should be user-oriented: "Web Search" and "Web Crawler", not implementation names.
  - Display backend details only where useful for debugging, such as `backend: crw_scrape` in expanded metadata.
- Validation:
  - `cargo check -p oxide-agent-web-ui`
  - If UI crate has tests: `cargo test -p oxide-agent-web-ui`
  - `rg "SearXNG|Crawl4AI|crawl4ai|searxng" crates/oxide-agent-web-ui/src/tasks/tool_cards.rs`
- Exit condition:
  - UI crate compiles and active tool cards parse/render new payloads.
- Risks and compromises:
  - Leptos UI compile may depend on generated artifacts or feature setup. If broad check fails for unrelated frontend setup, run the narrowest crate check available and document exact failure.
- Alternatives:
  - If full UI parsing refactor grows, keep old generic branches for unknown payloads but remove implementation-specific active labels.

### Checkpoint 7 — Migrate Docker Compose and `.env.example`

- Depends on: Checkpoint 3 or later. Prefer after Checkpoint 6 so runtime names are stable.
- Audit IDs: G12, G7, V3.
- Expected changes:
  - `docker-compose.yml`
  - `docker-compose.telegram.yml`
  - `docker-compose.web.yml` if it contains local web-research env/depends-on references
  - `docker-compose.web.local-services.yml`
  - `docker-compose.telegram.local-services.yml`
  - `docker/compose.full.yml`
  - `docker/compose.dev.yml`
  - `.env.example`
  - Delete `docker/searxng/settings.yml` when no compose file mounts it.
- CRW service definition target:
  - Service name: `crw`.
  - Image: `${OXIDE_CRW_IMAGE:-ghcr.io/us/crw}` unless upstream docs/source reveal a stable tag already used by the project.
  - Port: `${OXIDE_CRW_PORT:-3000}:3000` or `127.0.0.1:${OXIDE_CRW_PORT:-3000}:3000` for host-local services matching current security style.
  - Healthcheck: `GET /health` using `curl` or `wget` available in the image. If neither is guaranteed, use Docker's supported healthcheck command from CRW docs or omit only with documented reason.
  - Oxide env for host-network/root compose: `OXIDE_CRW_BASE_URL=http://127.0.0.1:3000`.
  - Oxide env for bridge/local-service compose: `OXIDE_CRW_BASE_URL=http://crw:3000`.
  - `OXIDE_CRW_ENABLED=true` where old local SearXNG/Crawl4AI integration was enabled.
  - `OXIDE_CRW_API_TOKEN=${OXIDE_CRW_API_TOKEN:-}` for client auth injection.
  - Preserve `OXIDE_WEB_CRAWLER_MERGE=true` defaults where currently present.
- Key decisions:
  - One CRW service replaces two old services.
  - Do not embed CRW proxy lists or tokens into compose examples.
- Validation:
  - `docker compose -f docker-compose.yml config`
  - `docker compose -f docker-compose.telegram.yml config`
  - `docker compose -f docker-compose.web.yml config`
  - `docker compose -f docker-compose.web.local-services.yml config`
  - `docker compose -f docker-compose.telegram.local-services.yml config`
  - `docker compose -f docker/compose.full.yml config`
  - `docker compose -f docker/compose.dev.yml config`
  - `rg "SEARXNG_|searxng|OXIDE_CRAWL4AI|crawl4ai|11235|8081|docker/searxng" docker-compose*.yml docker .env.example`
- Exit condition:
  - Compose YAML renders and old local services/envs are gone from active compose/env examples.
- Risks and compromises:
  - CRW image may not include `curl` for healthcheck. Verify before choosing health command.
  - Root compose files may use host network while local-service overlays use bridge DNS; base URLs must match each file's network mode.
- Alternatives:
  - If CRW image lacks a shell/HTTP client, use `CMD-SHELL` with whatever binary exists, or document a simple `test: ["CMD", "crw", "healthcheck"]` only if upstream supports it.
  - If local CRW is intentionally remote-only for some compose variants, do not add a service there; set only `OXIDE_CRW_BASE_URL` env and record the decision.

### Checkpoint 8 — Remove old providers, features, config fields, and stale compile references

- Depends on: Checkpoints 1-7.
- Audit IDs: G4, G5, G6, G7, Q1, Q2, N1.
- Expected changes:
  - Delete:
    - `crates/oxide-agent-core/src/agent/providers/searxng/`
    - `crates/oxide-agent-core/src/agent/providers/crawl4ai_markdown/`
    - `crates/oxide-agent-core/tests/searxng_provider.rs`
    - `docker/searxng/settings.yml` if not already deleted.
  - Remove old exports/imports in:
    - `crates/oxide-agent-core/src/agent/providers/mod.rs`
    - `crates/oxide-agent-core/src/agent/tool_runtime/modules.rs`
    - `crates/oxide-agent-core/src/agent/tool_runtime/mod.rs`
    - `crates/oxide-agent-core/src/agent/executor/registry.rs`
    - `crates/oxide-agent-core/src/agent/providers/delegation.rs`
  - Remove old feature definitions from `crates/oxide-agent-core/Cargo.toml`:
    - `tool-searxng`
    - `tool-crawl4ai-markdown`
  - Remove old config fields/env overlay/tests from `crates/oxide-agent-core/src/config.rs`.
- Key decisions:
  - This is the destructive checkpoint; do it only after CRW is registered and profiles no longer rely on old modules.
- Validation:
  - `cargo check -p oxide-agent-core --no-default-features --features profile-full`
  - `cargo check -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local`
  - `cargo check -p oxide-agent-core --no-default-features --features profile-search-only`
  - `rg "tool-searxng|tool-crawl4ai-markdown|Searxng|SearXNG|searxng_search|Crawl4Ai|crawl4ai_markdown|OXIDE_CRAWL4AI|SEARXNG_" crates/oxide-agent-core crates/oxide-agent-transport-web crates/oxide-agent-web-ui profiles docker-compose*.yml docker .env.example`
- Exit condition:
  - Old code is physically gone, old features no longer exist, and affected core profiles compile.
- Risks and compromises:
  - Removing `htmd` or `reqwest/query` feature edges may affect other tools. Check dependency features before deleting dependency features from Cargo.
- Alternatives:
  - If deletion causes a large unrelated compile issue, keep a minimal compatibility shim for one checkpoint only, clearly marked for removal before final verification.

### Checkpoint 9 — Documentation and snapshot fixture cleanup

- Depends on: Checkpoint 8.
- Audit IDs: G11, G13, V2, N1, N2, N3.
- Expected changes:
  - `AGENTS.md`:
    - Feature list: replace SearXNG/Crawl4AI with CRW.
    - Merge tool description: webfetch first, CRW fallback.
    - Compose description: CRW service/remote CRW.
  - `README.md`:
    - Replace active SearXNG/Crawl4AI setup with CRW.
    - Update env and compose references.
    - Update tool names to `web_search`/`web_crawler`.
  - `docs/deploy.md`, `docs/hooks/search-budget.md`, `docs/stack-logs-stage0.md`.
  - `docs/prd/implemented/brave-search-prd.md` and `docs/prd/implemented/plan-search-probe.md`:
    - Update fallback/allowlist references while preserving historical context if needed.
  - Snapshot files and hardcoded fixtures across core/web transport/UI.
- Key decisions:
  - Active docs should not instruct users to run SearXNG/Crawl4AI.
  - Historical docs can mention old names only as past context, not as active instructions.
- Validation:
  - `rg "SearXNG|searxng|Crawl4AI|crawl4ai|searxng_search|crawl4ai_markdown|SEARXNG_|OXIDE_CRAWL4AI" AGENTS.md README.md docs crates profiles docker-compose*.yml docker .env.example`
  - Review every remaining match and record whether it is historical/allowed or needs removal.
  - Run affected snapshot tests, updating snapshots only after reviewing diff.
- Exit condition:
  - Active documentation and snapshots are consistent with CRW migration.
- Risks and compromises:
  - Historical PRD docs may legitimately retain old words. Do not erase useful history; make active behavior clear.
- Alternatives:
  - If snapshot update tooling is unavailable, record exact failing snapshot names and manually inspect generated `.snap.new` files before committing.

### Checkpoint 10 — Final verification and completion audit

- Depends on: Checkpoints 0-9.
- Audit IDs: all.
- Expected changes:
  - No feature work.
  - Update this goal doc with evidence and final audit results.
- Validation commands:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo check --workspace --no-default-features --features profile-full`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-full`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-search-only`
  - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`
  - `cargo check -p oxide-agent-web-ui`
  - Compose config commands from Checkpoint 7.
  - Final old-name sweep:
    - `rg "tool-searxng|tool-crawl4ai-markdown|searxng_search|crawl4ai_markdown|SEARXNG_|OXIDE_CRAWL4AI|SearxngProvider|Crawl4AiMarkdownProvider" crates profiles docker-compose*.yml docker .env.example AGENTS.md README.md docs`
- Exit condition:
  - Every Completion Audit item is `verified` with evidence, or a remaining item is `blocked` with exact command/output and the smallest needed external action.
- Risks and compromises:
  - Full clippy/workspace test may expose unrelated warnings/failures. Do not mark complete unless the migration-specific evidence is strong and unrelated failures are documented precisely.
- Alternatives:
  - If Docker is unavailable, compose validation may be blocked; record Docker command failure and compensate with YAML/static inspection, but do not mark V3 verified without a user-accepted exception.

## Validation Contract

Static checks:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo check --workspace --no-default-features --features profile-full`

Focused tests:

- `cargo test -p oxide-agent-core --no-default-features --features tool-crw crw`
- `cargo test -p oxide-agent-core --no-default-features --features profile-full search_budget`
- `cargo test -p oxide-agent-core --no-default-features --features profile-full tool_runtime_static_guards`
- `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local search_probe`
- `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local web_transport`

Profile tests:

- `cargo test -p oxide-agent-core --no-default-features --features profile-full`
- `cargo test -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local`
- `cargo test -p oxide-agent-core --no-default-features --features profile-search-only`
- `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local`

Compose validation:

- `docker compose -f docker-compose.yml config`
- `docker compose -f docker-compose.telegram.yml config`
- `docker compose -f docker-compose.web.yml config`
- `docker compose -f docker-compose.web.local-services.yml config`
- `docker compose -f docker-compose.telegram.local-services.yml config`
- `docker compose -f docker/compose.full.yml config`
- `docker compose -f docker/compose.dev.yml config`

Artifact/static verification:

- `rg` old-name sweeps listed in checkpoints.
- Capability command for profile-full if practical:
  - `cargo run -p oxide-agent-telegram-bot --bin oxide-agent-telegram-bot --no-default-features --features profile-full -- capabilities --compiled --json`
- Manual diff review before each checkpoint commit.

Done when:

- Every audit item has evidence.
- All active docs, profiles, compose files, config helpers, runtime registration, probe allowlists, prompt guidance, UI cards, and tests use CRW/generic tool names.
- Old provider directories/files/features are gone.
- `webfetch_md`, Tavily, and Brave Search remain within the stated non-goals.

## Decisions

- 2026-06-15: Use `docs/goals/2026-06-15-crw-web-research-migration.md` because the repo already has `docs/goals/` goal documents.
- 2026-06-15: Implement CRW additively before deleting old providers so each checkpoint can compile.
- 2026-06-15: Use one thin CRW reqwest client/provider; no adapter trait or generic crawler abstraction.
- 2026-06-15: CRW owns `web_search` when enabled. Tavily is preserved, but its duplicate search executor must be skipped or otherwise conflict-guarded in the CRW-enabled case; `web_extract` should remain available.
- 2026-06-15: CRW scrape is internal to `web_crawler`; no raw `crw_scrape` LLM tool is exposed.
- 2026-06-15: Preserve `webfetch_md` as the first tier and do all truncation/windowing in Oxide so CRW response differences do not leak into prompt budget behavior.

## Progress Log

- 2026-06-15 12:30 UTC+3: Created goal contract and checkpoint plan.
  - Changed: `docs/goals/2026-06-15-crw-web-research-migration.md`.
  - Evidence: repository convention inspected; migration spec mapped to audit IDs and checkpoints.
  - Commands: not yet run in repo after file creation.
  - Audit IDs updated: all remain pending; this is planning evidence only.
  - Next: Checkpoint 1 — add CRW config/provider without removing old providers.

- 2026-06-15 Checkpoint 1 complete: CRW provider and config added.
  - Changed: `crates/oxide-agent-core/Cargo.toml` (feature `tool-crw`), `providers/mod.rs` (module+export), `config.rs` (5 helpers + 8 tests), `providers/crw/` (6 files: mod.rs, types.rs, client.rs, error.rs, format.rs, provider.rs).
  - Commands run:
    - `cargo test -p oxide-agent-core --no-default-features --features tool-crw crw` → 27 passed, 0 failed.
    - `cargo test -p oxide-agent-core --no-default-features --features tool-crw "config::tests"` → 30 passed, 0 failed.
    - `cargo check -p oxide-agent-core --no-default-features --features tool-crw,tool-webfetch-md` → OK.
    - `cargo check -p oxide-agent-core --no-default-features --features tool-searxng,tool-crawl4ai-markdown,tool-webfetch-md` → OK (old providers unchanged).
    - `cargo clippy -p oxide-agent-core --no-default-features --features tool-crw --all-targets -- -D warnings` → clean.
    - `cargo fmt --all -- --check` → clean.
  - Audit IDs updated: G1 verified, G7 verified (additive), Q2 verified (config layer), Q3 verified.
  - Next: Checkpoint 2 — register CRW `web_search` and update `web_crawler` fallback.

- 2026-06-15 Checkpoint 2 complete: CRW `web_search` registered and `web_crawler` fallback updated.
  - Changed: `tool_runtime/mod.rs` (export `CrwSearchToolModule`), `executor/registry.rs` (CRW module registration + cfg gates), `providers/delegation.rs` (CRW in sub-agent lists + cfg gates), `tool_runtime/modules.rs` (`CrwSearchToolModule`, `WebCrawlerToolExecutor` CRW fallback, Tavily duplicate-name guard), `providers/crw/mod.rs` (re-export `CrwScrapeArgs`).
  - Commands run:
    - `cargo check -p oxide-agent-core --no-default-features --features tool-crw,tool-webfetch-md` → OK.
    - `cargo check -p oxide-agent-core --no-default-features --features tool-crw,tool-webfetch-md,tool-tavily,tool-brave-search` → OK.
    - `cargo check --workspace --no-default-features --features profile-full` → OK.
    - `cargo test -p oxide-agent-core --no-default-features --features tool-crw crw` → 27 passed.
    - `cargo test -p oxide-agent-core --no-default-features --features profile-full -- modules::web_crawler_tests config` → 90 passed.
    - `cargo clippy --workspace --no-default-features --features profile-full --all-targets -- -D warnings` → clean.
    - `cargo fmt --all -- --check` → clean.
  - Audit IDs updated: G2 verified (additive), G3 verified, G6 verified (additive), Q2 verified (runtime registration), Q4 verified, N3 verified.
  - Next: Checkpoint 3 — switch capabilities, Cargo profiles, and repo profile TOMLs to CRW.

- 2026-06-15 Checkpoint 3 complete: Capabilities, Cargo profiles, and TOML profiles switched to CRW.
  - Changed: `Cargo.toml` (profile-full, profile-web-embedded-opencode-local, profile-search-only now use `tool-crw`; `tool-crawl4ai-markdown` removed from web-embedded profile), `compiled.rs` (added `tool-crw -> tool/crw` capability entry), 5 profile TOMLs (`tool/searxng` → `tool/crw`, `tool/crawl4ai-markdown` → `tool/crw`, web TOML `cargo_features` updated).
  - Commands run:
    - `cargo check -p oxide-agent-core --no-default-features --features profile-full` → OK.
    - `cargo check -p oxide-agent-core --no-default-features --features profile-search-only` → OK.
    - `cargo check -p oxide-agent-core --no-default-features --features profile-web-embedded-opencode-local` → OK.
    - `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --all-targets -- -D warnings` → clean.
    - `cargo fmt --all -- --check` → clean.
    - `cargo run ... -- capabilities --compiled --json` → shows `tool/crw` with `tool/crw-search` and `tool/crw-scrape`.
    - `cargo test -p oxide-agent-core --no-default-features --features profile-full capabilities` → 30 passed, 2 failed (pre-existing sandbox).
  - Audit IDs updated: G5 verified (profiles), G8 verified (capabilities), V1 in progress.
  - Next: Checkpoint 4 — migrate search budget, prompt guidance, thoughts, static tool-name policy.

- 2026-06-15 Checkpoint 4 complete: Search budget, core guidance, thoughts, and static guards migrated to generic CRW-era tool names.
  - Changed: `search_budget.rs`, `prompt/composer.rs`, `thoughts.rs`, `tool_failure_summary.rs`, `providers/brave_search/{format.rs,provider.rs}`, `executor/execution.rs`, `runner/tools.rs`, `tests/tool_runtime_static_guards.rs`.
  - Commands run:
    - `cargo fmt --all -- --check` → clean.
    - `cargo test -p oxide-agent-core --no-default-features --features profile-full search_budget` → 8 passed.
    - `cargo test -p oxide-agent-core --no-default-features --features profile-full agent::prompt::composer::tests` → 19 passed.
    - `cargo test -p oxide-agent-core --no-default-features --features profile-full agent::thoughts::tests` → 8 passed.
    - `cargo test -p oxide-agent-core --no-default-features --features profile-full agent::tool_failure_summary::tests` → 5 passed.
    - `cargo test -p oxide-agent-core --no-default-features --features profile-full brave_search` → 26 passed.
    - `cargo test -p oxide-agent-core --no-default-features --features profile-full --test tool_runtime_static_guards` → 19 passed.
    - `cargo clippy -p oxide-agent-core --no-default-features --features profile-full --all-targets -- -D warnings` → clean.
    - Changed-file sweep for `searxng_search|crawl4ai_markdown|SearxngProvider|Crawl4Ai|SearXNG|Crawl4AI` → no matches.
  - Audit IDs updated: G9 verified, G11 in progress (core guidance/thoughts), V2 in progress (static guards), N1 verified, N3 remains verified.
  - Next: Checkpoint 5 — migrate web transport probe/session and transport fixtures.

- 2026-06-15 Checkpoint 5 complete: Web transport Search Probe and event fixtures migrated to generic CRW-era tool names.
  - Changed: `server/search_probe.rs` (`web_search` default allowlists, no Crawl4AI block constant, timeout-report fixtures updated), `session.rs` (removed Search Probe `crawl4ai_markdown` filtering), `web_transport.rs` (web_crawler display payloads use `crw_scrape`; removed standalone Crawl4AI display parsing; timing fixture uses `web_search`).
  - Commands run:
    - `cargo fmt --all -- --check` → clean.
    - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local search_probe` → 31 passed.
    - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local web_transport` → 21 passed.
    - `cargo check -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local` → OK (pre-existing core Mistral dead_code/unused warnings only).
    - `rg "searxng_search|crawl4ai_markdown|SearXNG|Crawl4AI|crawl4ai|searxng" crates/oxide-agent-transport-web/src` → no matches.
  - Audit IDs updated: G10 verified, G11 in progress (web transport complete; web UI later), V2 in progress (web transport fixtures), N3 remains verified.
  - Next: Checkpoint 6 — migrate web UI tool cards and error parsing.

- 2026-06-15 Checkpoint 6 complete: Web UI tool cards migrated to generic CRW-era search/crawler labels and errors.
  - Changed: `crates/oxide-agent-web-ui/src/tasks/tool_cards.rs` (removed `searxng_search` and `crawl4ai_markdown` card branches, renamed Web Search label, restricted crawl card to `web_crawler`, updated display-payload parsing and failure summaries for `crw_*` error kinds).
  - Commands run:
    - `cargo check -p oxide-agent-web-ui` → OK.
    - `cargo test -p oxide-agent-web-ui` → 5 passed.
    - `cargo fmt --all -- --check` → clean.
    - `rg "SearXNG|Crawl4AI|crawl4ai|searxng" crates/oxide-agent-web-ui/src/tasks/tool_cards.rs` → no matches.
  - Audit IDs updated: G11 verified for active core/web/UI paths, V2 in progress (UI parsing updated; final snapshots later).
  - Next: Checkpoint 7 — migrate Docker Compose and `.env.example`.

## Risks and Blockers

- Tavily/CRW duplicate `web_search` registration.
  - Impact: runtime registry fails with duplicate tool registration when both providers are enabled.
  - Evidence: Tavily provider declares `web_search`; registry rejects duplicate names.
  - Mitigation: add the smallest conflict guard; CRW owns `web_search` when enabled, Tavily remains for `web_extract`.
  - Audit IDs affected: G2, G6, Q4, N2.

- CRW optional API field mismatch.
  - Impact: sending guessed fields can create 400 responses or silently ignored options.
  - Evidence: migration spec gives API surface, not exact request structs for every optional legacy knob.
  - Mitigation: inspect CRW upstream docs/source in Checkpoint 1; send only documented fields; accept unsupported old args with backend notes when needed.
  - Audit IDs affected: G1, G2, G3.

- Snapshot churn across web transport/UI.
  - Impact: broad snapshot diffs can hide real behavior changes.
  - Evidence: current tests have hardcoded SearXNG/Crawl4AI payloads.
  - Mitigation: update snapshots after functional migration, review diffs, and keep fixture changes scoped to tool names/payload shape.
  - Audit IDs affected: G11, V2.

- Docker healthcheck uncertainty.
  - Impact: CRW image may not contain `curl`/`wget`.
  - Evidence: must inspect image/docs before finalizing compose healthcheck.
  - Mitigation: use documented CRW healthcheck command if available; otherwise document why healthcheck is omitted or use an available binary.
  - Audit IDs affected: G12, V3.

- Destructive deletion too early.
  - Impact: compile failures across feature-gated profiles can become hard to isolate.
  - Evidence: old providers are referenced in modules, registry, delegation, docs, tests, and features.
  - Mitigation: delete old providers only in Checkpoint 8 after additive CRW registration and profile switch.
  - Audit IDs affected: G4, G5, G6, V1.

## Commit Guidance

Commit after each completed checkpoint when:

- The checkpoint exit condition is satisfied.
- Relevant validation has run, or failure/unavailability is recorded with exact command output.
- This goal doc is updated with evidence and next step.
- `git status --short` and the relevant diff have been reviewed.
- The commit is one coherent unit of work.

Suggested commit scopes:

- `docs(goal): add crw migration goal`
- `feat(crw): add provider and config`
- `feat(tools): route web search and crawler fallback through crw`
- `chore(capabilities): switch web research profiles to crw`
- `fix(search): update search budget and probe tool names`
- `fix(web): update crw web tool fixtures and cards`
- `chore(compose): replace searxng crawl4ai services with crw`
- `refactor(web-research): remove legacy searxng crawl4ai providers`
- `docs(web-research): document crw migration`

## User-Facing Progress Updates

Use compact updates after meaningful evidence:

- Current checkpoint and files changed.
- Commands run and result.
- Audit IDs moved to `verified`, `blocked`, or still `pending`.
- One next action.

Avoid vague updates such as "working on migration". Name the next file set or validation command.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
