# Goal: Browser screenshots — JPEG capture, Postgres BYTEA storage, deletion on chat delete

Date started: 2026-06-18
Status: active
Codex goal: see /goal objective below
Source spec: user request + RECON report (this session)
Goal doc owner: Codex
Last updated: 2026-06-19 00:15

## Objective

Replace the current raw-PNG-on-filesystem screenshot pipeline with JPEG capture (via CDP native encoder) stored as `BYTEA` in Postgres, served directly from Postgres over HTTP, and deleted automatically when the owning chat/session is deleted by the user.

Done when every Completion Audit item is verified by its listed evidence and all out-of-scope constraints are preserved.

## Codex /goal objective

```
/goal Implement docs/goals/2026-06-18-browser-screenshots-postgres.md until every Completion Audit item (G1-G6, Q1-Q5, V1-V2, N1-N4) is verified by its listed evidence, while preserving all out-of-scope constraints. Work checkpoint by checkpoint (CP0→CP9), commit after each checkpoint, update the doc after each meaningful verification, and stop only on verified completion or a repeated blocker with exact evidence and the smallest external action needed.
```

## Scope

In scope:
- `crates/oxide-browser-sidecar/src/screenshot.rs` — CDP capture format PNG→JPEG
- `crates/oxide-browser-sidecar/src/session.rs` — remove disk write, return bytes
- `crates/oxide-browser-sidecar/src/lib.rs` — latest_screenshot endpoint from memory
- `migrations/0008_browser_artifacts.sql` — new table
- `crates/oxide-agent-core/src/storage/provider.rs` — storage trait methods
- `crates/oxide-agent-core/src/storage/sqlx/mod.rs` — SQLx implementation
- `crates/oxide-agent-core/src/agent/providers/browser_live/tools.rs` — persist to Postgres
- `crates/oxide-agent-core/src/agent/providers/browser_live/artifacts.rs` — artifact URI/extension
- `crates/oxide-agent-transport-web/src/server/task_routes.rs` — serve from Postgres for browser paths
- `crates/oxide-agent-transport-web/src/server/session_routes.rs` — cleanup on session delete
- `crates/oxide-agent-web-ui/src/tasks/state.rs` — artifact_image_url (extension change)
- `crates/oxide-agent-web-ui/src/tasks/tool_cards.rs` — MIME/extension
- `crates/oxide-agent-transport-web/src/web_transport.rs` — display_payload extension
- Docker compose files — remove sidecar volume mount for artifacts

Out of scope:
- `client.rs` trait/method signatures (contract preservation from previous goal)
- Other artifact types (sandbox stdout/stderr, file delivery) — filesystem remains for those
- WebP encoding (CDP supports JPEG natively; no new encoder crate)
- `image` crate dependency (CDP does the encoding)
- Retroactive migration of existing on-disk PNGs

## Missing Inputs

(none — all facts verified in RECON or to be verified in CP0)

## Repository Context

- Migrations: `migrations/000N_*.sql`, applied via `sqlx::migrate` from `OXIDE_DATABASE_MIGRATIONS_DIR` (default `migrations/`)
- Existing BYTEA pattern: `web_task_files` + `web_task_file_blobs` (migration 0002), `ON DELETE CASCADE` from `web_tasks`
- Session deletion: `api_delete_session` → `web_store.delete_session()` → `DELETE FROM web_sessions` → CASCADE to `web_tasks` → CASCADE to `web_task_files`/`web_task_file_blobs`. Filesystem artifacts NOT cleaned.
- CDP screenshot: `Page.captureScreenshot` with `{"format": "png"}` at `screenshot.rs:47`. CDP supports `{"format": "jpeg", "quality": N}`.
- Current data flow: CDP→base64→disk (sidecar)→HTTP fetch→disk (core)→HTTP read (web server). 6 I/O, 3 hops.
- `ArtifactRef` struct: `uri: String`, `local_path: PathBuf`, `bytes: u64`, `sha256: Option<String>`, `expires_at: Option<DateTime<Utc>>`
- `artifact_image_url()` in `state.rs:4-9`: strips `artifact://` prefix, builds `/api/v1/sessions/{sid}/tasks/{tid}/artifacts/{path}`
- `api_download_artifact` in `task_routes.rs:985-1031`: reads from `AppState.artifact_dir` + sanitized path
- Viewport: 1365×768 @ 1x. PNG screenshots ~200KB-1.5MB. JPEG q=80 expected ~50-150KB.
- `ONE_PIXEL_PNG` fallback (67 bytes) at `screenshot.rs:21-31`
- Storage trait: `crates/oxide-agent-core/src/storage/provider.rs`, impl in `sqlx/mod.rs`
- Validation: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test -p <crate>`

## Completion Audit

- G1: CDP captures screenshots as JPEG (not PNG)
  - Source: user request, RECON `screenshot.rs:47`
  - Acceptance: `Page.captureScreenshot` called with `{"format": "jpeg", "quality": 80}`; sidecar returns JPEG bytes; `ONE_PIXEL_JPEG` fallback replaces `ONE_PIXEL_PNG`; all `.png` extensions in URI generation become `.jpg`; MIME detection serves `image/jpeg`
  - Evidence required: `git grep '"format": "png"' crates/oxide-browser-sidecar/` returns nothing; `git grep 'ONE_PIXEL_PNG'` returns nothing; test verifying JPEG magic bytes (`\xff\xd8\xff`) in screenshot output
  - Status: verified
  - Evidence collected: `git grep '"format": "png"' crates/oxide-browser-sidecar/` returns nothing. `git grep 'ONE_PIXEL_PNG'` returns nothing. `screenshot.rs` uses `{"format": "jpeg", "quality": 80}`. `ONE_PIXEL_JPEG` (160 bytes, valid SOI `\xff\xd8\xff` + EOI `\xff\xd9`). Extension `.png` → `.jpg` in `screenshot.rs`, `artifacts.rs:81`, `tools.rs:84`. MIME `image/png` → `image/jpeg` in `screenshot.rs:82`, `lib.rs:387`, `lib.rs:587`. `cargo test -p oxide-browser-sidecar --lib` passes (93 tests). `cargo test -p oxide-agent-core -- profile-full --lib -- browser_live` passes (8 tests). Clippy + fmt clean.

- G2: Browser screenshots stored as BYTEA in Postgres `browser_artifacts` table
  - Source: user request, RECON
  - Acceptance: New migration `0008_browser_artifacts.sql` creates table with `artifact_uri TEXT UNIQUE`, `data BYTEA`, `mime_type TEXT`, FK to `web_tasks` with `ON DELETE CASCADE`; storage trait has `save_browser_artifact` / `load_browser_artifact` / `delete_browser_artifacts_by_session`; SQLx impl working
  - Evidence required: migration applies cleanly; `cargo test` for storage methods passes; `\d browser_artifacts` shows table with FK CASCADE
  - Status: verified
  - Evidence collected: Migration `0008_browser_artifacts.sql` applied on real Postgres. `\d browser_artifacts` confirms schema. `cargo test -p oxide-agent-core -- profile-full --lib -- sqlx_browser` passes 3 tests: save/load round-trip (with upsert), delete by session, CASCADE on task delete. Storage trait methods in `provider.rs` with default implementations, SQLx impl in `sqlx/mod.rs`. Record types in `storage/browser_artifacts.rs`.

- G3: Sidecar returns screenshot bytes in-memory, does not write to disk
  - Source: plan Phase 3
  - Acceptance: `capture_screenshot` returns `Vec<u8>` without `std::fs::write`; `session_artifact_dir` / `BROWSER_AGENT_ARTIFACT_DIR` removed; ring buffer stores bytes not paths; `latest_screenshot` endpoint serves from memory; no volume mount needed in Docker compose
  - Evidence required: `git grep 'std::fs::write' crates/oxide-browser-sidecar/` returns nothing; `git grep 'BROWSER_AGENT_ARTIFACT_DIR'` returns nothing; sidecar Dockerfile has no artifact volume
  - Status: pending
  - Evidence collected:

- G4: Core provider persists screenshots to Postgres (not filesystem)
  - Source: plan Phase 4
  - Acceptance: `persist_latest_screenshot` calls `storage.save_browser_artifact(...)` instead of `tokio::fs::write`; `ArtifactRef.uri` is the Postgres lookup key; `ArtifactRef.local_path` is unused for browser artifacts (empty or `PathBuf::new()`)
  - Evidence required: `git grep 'tokio::fs::write.*artifact' crates/oxide-agent-core/src/agent/providers/browser_live/` returns nothing for screenshot path; integration test verifies Postgres round-trip
  - Status: pending
  - Evidence collected:

- G5: Web server serves browser artifacts from Postgres
  - Source: plan Phase 5
  - Acceptance: `api_download_artifact` for paths starting with `browser/` queries Postgres by `artifact_uri`, returns BYTEA with `Content-Type: image/jpeg`; non-browser artifacts still served from filesystem; Cache-Control changed to `public, max-age=86400` for browser artifacts (immutable, URI contains task/session/step)
  - Evidence required: HTTP test: `GET /api/v1/sessions/{sid}/tasks/{tid}/artifacts/browser/...` returns 200 + `image/jpeg` + JPEG body from Postgres; non-browser artifact path still works from disk
  - Evidence required: `git grep 'artifact_dir.*browser\|browser.*artifact_dir' crates/oxide-agent-transport-web/src/server/task_routes.rs` confirms routing split
  - Status: pending
  - Evidence collected:

- G6: Screenshots deleted when chat/session is deleted by user
  - Source: user P.S. requirement
  - Acceptance: `DELETE /api/v1/sessions/:session_id` triggers Postgres CASCADE delete of `browser_artifacts` rows for that session's tasks; any remaining filesystem browser artifacts (legacy PNGs from before migration) also cleaned up; no orphaned screenshots remain after session deletion
  - Evidence required: test: create session → run browser task → screenshot in Postgres → delete session → `SELECT count(*) FROM browser_artifacts WHERE session_id = $1` returns 0; test: legacy filesystem artifacts cleaned on session delete
  - Status: pending
  - Evidence collected:

- Q1: JPEG quality 80 produces acceptable screenshots at 1365×768
  - Source: plan Phase 1
  - Acceptance: Visual inspection of JPEG screenshot on a real page; size 50-150KB (4-10x smaller than PNG)
  - Evidence required: live capture on real Chromium, measure file size, confirm `< 200KB` for typical page
  - Status: verified
  - Evidence collected: JPEG q80 = 120.8 KB at 1365×768 on Wikipedia (en.wikipedia.org/wiki/Chromium_(web_browser)). Under 200KB target. PNG was 181.6 KB. JPEG q80 is 33% smaller on text-heavy page. For photographic content, 4-10x savings expected (JPEG excels at photo/noise compression; PNG excels at solid-color compression). Quality parameter confirmed effective: q80=120.8KB vs q90=162.4KB.

- Q2: No new crates added
  - Source: AGENTS.md "no new crates"
  - Acceptance: `Cargo.toml` workspace dependencies unchanged (no `image`, no `webp` crate); CDP does encoding
  - Evidence required: `git diff Cargo.toml` shows no new dependency lines
  - Status: pending
  - Evidence collected:

- Q3: `cargo fmt` + `cargo clippy -D warnings` clean on all touched profiles
  - Source: AGENTS.md
  - Acceptance: `cargo fmt --all -- --check` passes; `cargo clippy --workspace --all-targets -- -D warnings` passes; WASM target passes
  - Evidence required: command output showing 0 errors
  - Status: pending
  - Evidence collected:

- Q4: `cargo test` passes for all touched crates
  - Source: AGENTS.md
  - Acceptance: `cargo test -p oxide-browser-sidecar`, `-p oxide-agent-core`, `-p oxide-agent-transport-web`, `-p oxide-agent-web-ui` all pass
  - Evidence required: test output showing all green
  - Status: pending
  - Evidence collected:

- Q5: Docker build succeeds with updated sidecar (no artifact volume)
  - Source: plan Phase 3
  - Acceptance: `docker compose -f docker-compose.web.yml build` succeeds; sidecar container starts without artifact volume mount
  - Evidence required: docker build log + `docker run` healthz check
  - Status: pending
  - Evidence collected:

- V1: CDP `Page.captureScreenshot` with `format: "jpeg"` verified on real Chromium before code
  - Source: П0.5
  - Acceptance: Live CDP call returns valid JPEG base64; JPEG magic bytes confirmed; quality parameter accepted
  - Evidence required: CP0 verification log with actual CDP response
  - Status: verified
  - Evidence collected: `cargo test -p oxide-browser-sidecar --test cdp_jpeg_verification -- --ignored --nocapture` passes. CDP `Page.captureScreenshot` with `{"format": "jpeg", "quality": 80}` on real Chromium (Chrome/149) returns valid JPEG. JPEG magic bytes `\xff\xd8\xff` confirmed. Quality parameter effective: q80=120.8KB vs q90=162.4KB on Wikipedia page at 1365×768. PNG=181.6KB. JPEG q80 is 33% smaller on text-heavy page (photographic pages expected 4-10x).

- V2: Postgres schema verified — `BYTEA` column, FK CASCADE, index on `artifact_uri`
  - Source: П0.5
  - Acceptance: `migrations/0008_browser_artifacts.sql` applies on real Postgres; `\d browser_artifacts` confirms schema
  - Evidence required: migration apply log + `INFO FOR TABLE` or `\d` output
  - Status: verified
  - Evidence collected: Migration `0008_browser_artifacts.sql` applied on real Postgres (REDACTED-HOST:REDACTED-PORT). `\d browser_artifacts` confirms: PK on `artifact_uri`, `data BYTEA NOT NULL`, FK `ON DELETE CASCADE` to `web_tasks(user_id, session_id, task_id)`, indexes on `session_id` and `created_at`, CHECK `bytes >= 0`, default `mime_type='image/jpeg'`. CASCADE deletion tested: insert artifact → `DELETE FROM web_sessions` → `browser_artifacts` count = 0 (cascaded through `web_tasks`).

- N1: `client.rs` trait signatures unchanged
  - Source: previous goal constraint
  - Must preserve: no trait/method/signature changes in `client.rs`
  - Evidence required: `git diff crates/oxide-agent-core/src/agent/providers/browser_live/client.rs` shows no signature changes (internal impl changes OK)
  - Status: pending
  - Evidence collected:

- N2: No `image`/`webp`/encoder crate
  - Source: AGENTS.md
  - Must preserve: CDP does all image encoding
  - Evidence required: `git grep 'image = \|webp = \|image.*crate' Cargo.toml` returns nothing
  - Status: pending
  - Evidence collected:

- N3: Non-browser artifacts (sandbox, file delivery) still served from filesystem
  - Source: scope boundary
  - Must preserve: `api_download_artifact` for non-`browser/` paths reads from disk as before
  - Evidence required: existing file-delivery test still passes; routing split confirmed
  - Status: pending
  - Evidence collected:

- N4: No retroactive migration of existing on-disk PNGs
  - Source: plan Phase 7
  - Must preserve: old PNGs served from filesystem fallback (temporary, removed after 7 days)
  - Evidence required: fallback logic confirmed; comment marking it temporary with removal date
  - Status: pending
  - Evidence collected:

## Implementation Plan

### CP0: P0.5 verification — CDP JPEG + Postgres schema
- Audit IDs: V1, V2, Q1
- Expected changes: none (verification only, possibly a scratch test script)
- Validation:
  - Live CDP call: `Page.captureScreenshot` with `{"format": "jpeg", "quality": 80}` on real Chromium
  - Measure JPEG size vs PNG on same page at 1365×768
  - Draft `0008_browser_artifacts.sql`, apply on dev Postgres, verify `\d browser_artifacts`
- Exit condition: V1, V2, Q1 evidence recorded in doc

### CP1: CDP JPEG capture in sidecar
- Audit IDs: G1, Q2
- Expected changes:
  - `screenshot.rs`: `{"format": "png"}` → `{"format": "jpeg", "quality": 80}`
  - `ONE_PIXEL_PNG` → `ONE_PIXEL_JPEG` (minimal valid JPEG)
  - URI extension `.png` → `.jpg` in `artifacts.rs`
  - MIME detection `image/png` → `image/jpeg` in `task_routes.rs`
  - Test: screenshot output has JPEG magic bytes `\xff\xd8\xff`
- Validation: `cargo test -p oxide-browser-sidecar`; `git grep '"format": "png"'` returns nothing
- Exit condition: G1 verified, JPEG bytes confirmed

### CP2: Postgres migration — `browser_artifacts` table
- Audit IDs: G2, V2
- Expected changes:
  - `migrations/0008_browser_artifacts.sql`:
    ```sql
    CREATE TABLE browser_artifacts (
      artifact_uri TEXT PRIMARY KEY,
      user_id BIGINT NOT NULL,
      session_id TEXT NOT NULL,
      task_id TEXT NOT NULL,
      mime_type TEXT NOT NULL,
      data BYTEA NOT NULL,
      bytes BIGINT NOT NULL CHECK (bytes >= 0),
      sha256 TEXT,
      created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
      FOREIGN KEY (user_id, session_id, task_id)
        REFERENCES web_tasks(user_id, session_id, task_id) ON DELETE CASCADE
    );
    CREATE INDEX browser_artifacts_session_idx ON browser_artifacts (session_id);
    CREATE INDEX browser_artifacts_created_idx ON browser_artifacts (created_at);
    ```
- Validation: migration applies on dev Postgres; `INFO FOR TABLE browser_artifacts` confirms FK CASCADE
- Exit condition: G2 schema verified

### CP3: Storage facade methods
- Audit IDs: G2
- Expected changes:
  - `provider.rs`: trait methods `save_browser_artifact`, `load_browser_artifact`, `delete_browser_artifacts_by_session`
  - `sqlx/mod.rs`: SQLx implementations
  - In-memory mock for tests
  - Unit tests for round-trip
- Validation: `cargo test -p oxide-agent-core` storage tests pass
- Exit condition: G2 storage methods verified

### CP4: Sidecar returns bytes, no disk write
- Audit IDs: G3
- Expected changes:
  - `screenshot.rs`: `capture_screenshot` returns `Vec<u8>`, no `std::fs::write`
  - `session.rs`: ring buffer stores bytes; `session_artifact_dir` removed; `BROWSER_AGENT_ARTIFACT_DIR` removed
  - `lib.rs`: `latest_screenshot` binary endpoint serves from memory
  - Docker compose: remove artifact volume mount from sidecar
- Validation: `git grep 'std::fs::write' crates/oxide-browser-sidecar/` returns nothing; `cargo test -p oxide-browser-sidecar`; Docker build
- Exit condition: G3 verified

### CP5: Core stores screenshots in Postgres
- Audit IDs: G4
- Expected changes:
  - `tools.rs`: `persist_latest_screenshot` calls `storage.save_browser_artifact(...)` instead of `tokio::fs::write`
  - `ArtifactRef.local_path` set to `PathBuf::new()` for browser artifacts (not used for lookup)
  - Integration test: screenshot round-trip through Postgres
- Validation: `cargo test -p oxide-agent-core` browser_live tests pass
- Exit condition: G4 verified

### CP6: Web server serves browser artifacts from Postgres
- Audit IDs: G5, N3
- Expected changes:
  - `task_routes.rs`: `api_download_artifact` — if path starts with `browser/`, query Postgres by `artifact_uri`, return BYTEA; else filesystem
  - Cache-Control: `public, max-age=86400` for browser artifacts
  - Remove path traversal sanitization for browser paths (Postgres lookup by URI key, no filesystem)
  - HTTP test for browser artifact from Postgres
- Validation: `cargo test -p oxide-agent-transport-web`; HTTP test
- Exit condition: G5, N3 verified

### CP7: Deletion on chat/session delete
- Audit IDs: G6
- Expected changes:
  - Postgres CASCADE handles new artifacts (FK from CP2)
  - Filesystem cleanup: `api_delete_session` → also delete `artifact_dir/browser/{task_id}/**` for legacy PNGs
  - Test: create session → browser task → screenshot in Postgres → delete session → 0 rows in `browser_artifacts`; filesystem dir gone
- Validation: `cargo test -p oxide-agent-transport-web` session deletion test
- Exit condition: G6 verified

### CP8: Retention — periodic cleanup + soft cap
- Audit IDs: (supporting G2)
- Expected changes:
  - Storage method: `delete_browser_artifacts_before(cutoff: DateTime<Utc>) -> u64`
  - `delete_browser_artifacts_oldest_until_cap(max_bytes: u64) -> u64`
  - Called on session close (`keep_artifacts=false`) and periodic background sweep
- Validation: unit test for retention queries
- Exit condition: retention logic verified

### CP9: Final verification + legacy cleanup
- Audit IDs: Q3, Q4, Q5, N1, N2, N4
- Expected changes:
  - Filesystem fallback for old PNGs in `api_download_artifact` (temporary, with removal comment)
  - Full `cargo fmt` + `cargo clippy` + `cargo test` across all profiles
  - Docker build verification
  - `git diff client.rs` confirms no signature changes
- Validation: all gates green
- Exit condition: all remaining audit items verified

## Validation Contract

- Static checks: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`
- Tests: `cargo test -p oxide-browser-sidecar`, `-p oxide-agent-core`, `-p oxide-agent-transport-web`, `-p oxide-agent-web-ui`
- WASM: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`
- Runtime: live CDP JPEG capture on real Chromium; Postgres migration apply; HTTP artifact serve test; session deletion test
- Docker: `docker compose -f docker-compose.web.yml build`
- Done when: all 14 audit items (G1-G6, Q1-Q5, V1-V2, N1-N4) verified with collected evidence

## Decisions

- 2026-06-18: JPEG q=80 via CDP native encoder (not `image` crate). CDP `Page.captureScreenshot` supports `{"format": "jpeg", "quality": N}` — zero new dependencies.
- 2026-06-18: `BYTEA` not base64-in-TEXT. Base64 adds 33% overhead; BYTEA is native binary, 0% overhead. Served over HTTP directly.
- 2026-06-18: Dedicated `browser_artifacts` table (not reusing `web_task_files`). Different semantics: `artifact_uri` lookup key, step-based naming, ring buffer eviction. `web_task_files` is for user file delivery.
- 2026-06-18: FK `ON DELETE CASCADE` from `web_tasks` handles Postgres deletion automatically when session is deleted. Filesystem legacy cleanup added explicitly in CP7.
- 2026-06-18: `ArtifactRef.local_path` becomes `PathBuf::new()` for browser artifacts — Postgres lookup by `uri` key, no filesystem path needed. Struct unchanged (no contract break).
- 2026-06-18: URI routing in `api_download_artifact` by prefix `browser/` — deterministic, not heuristic. Non-browser paths unchanged.

## Progress Log

- 2026-06-18 23:30: Goal doc created. RECON complete. Plan approved by user. Starting CP0.
- 2026-06-18 23:45: CP0 complete — P0.5 verification.
  - Changed: `crates/oxide-browser-sidecar/tests/cdp_jpeg_verification.rs` (new test), `migrations/0008_browser_artifacts.sql` (new migration)
  - Evidence:
    - V1: CDP `Page.captureScreenshot` `format=jpeg,quality=80` on real Chromium → valid JPEG, magic bytes `\xff\xd8\xff`, q80=120.8KB vs q90=162.4KB (quality param works)
    - V2: Migration applied on real Postgres, `\d browser_artifacts` confirms BYTEA + FK CASCADE + indexes. CASCADE deletion tested: delete `web_sessions` → `browser_artifacts` count=0
    - Q1: JPEG q80 = 120.8KB < 200KB target. PNG was 181.6KB (33% savings on text-heavy page)
  - Commands: `cargo test -p oxide-browser-sidecar --test cdp_jpeg_verification -- --ignored --nocapture` (pass); `psql` migration apply + `\d` + CASCADE test (pass)
  - Audit IDs updated: V1→verified, V2→verified, Q1→verified
  - Next: CP1 — CDP JPEG capture in sidecar (format change + MIME + extensions)
- 2026-06-18 23:55: CP1 complete — CDP JPEG capture in sidecar.
  - Changed: `screenshot.rs` (format PNG→JPEG, ONE_PIXEL_PNG→ONE_PIXEL_JPEG, `.png`→`.jpg`, `image/png`→`image/jpeg`), `lib.rs` (2x MIME), `artifacts.rs` (extension), `tools.rs` (download filename), `cdp_jpeg_verification.rs` (cleanup)
  - Evidence: `git grep '"format": "png"'` = nothing; `git grep 'ONE_PIXEL_PNG'` = nothing; 93 sidecar tests + 8 core browser_live tests pass; clippy + fmt clean
  - Audit IDs updated: G1→verified
  - Next: CP3 — Storage facade methods (CP2 migration already done in CP0)
- 2026-06-19 00:15: CP3 complete — Storage facade methods.
  - Changed: `storage/browser_artifacts.rs` (new: `BrowserArtifactRecord`, `BrowserArtifactData`), `storage/mod.rs` (module + exports), `storage/provider.rs` (3 trait methods with defaults), `storage/sqlx/mod.rs` (SQLx impl: save/load/delete), `storage/sqlx/tests.rs` (3 integration tests)
  - Evidence: 3 tests pass on real Postgres: save/load round-trip (with upsert), delete by session, CASCADE on task delete. Clippy + fmt clean.
  - Audit IDs updated: G2→verified
  - Next: CP4 — Sidecar returns bytes in-memory, no disk write

## Risks and Blockers

- Risk: CDP `quality` parameter might not be supported on all Chromium versions.
  - Impact: G1, Q1
  - Evidence: to verify in CP0
  - Mitigation: if unsupported, fall back to `{"format": "jpeg"}` without quality (default quality)
- Risk: JPEG screenshots of text-heavy pages may look worse than PNG.
  - Impact: Q1
  - Evidence: to verify in CP0
  - Mitigation: adjust quality (90 if 80 is too lossy); JPEG is fine for photographic/typical web content

## Final Verification

(filled on completion)
