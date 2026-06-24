# Goal: Permanent Life Mode memory implementation

Date started: 2026-06-24
Status: active
Codex goal: Implement `docs/goals/2026-06-24-permanent-life-memory.md` until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals. Work checkpoint by checkpoint, update the doc after each meaningful verification, commit after each completed phase, compress before starting the next phase, and stop only on verified completion or an exact blocker with required external action.
Source spec: `docs/prd/PRD-perm.md`
Goal doc owner: Codex
Last updated: 2026-06-24 04:00

## Objective

Implement the Permanent Life Mode memory foundation from `docs/prd/PRD-perm.md`: an AuDHD-first, Postgres-authoritative, rebuildable/wipe-capable memory system in a new `oxide-agent-life` bounded context, with generic prompt context seams in core and transport-neutral integration points for future Web/Telegram `/life` UX.

Done when every required Completion Audit item is verified by its listed evidence, ordinary chat/topic/session mode remains behavior-compatible, and the implementation can be resumed from this document without relying on hidden conversation history.

## Scope

In scope:
- `docs/prd/PRD-perm.md` as the source contract.
- New `crates/oxide-agent-life/` bounded context with PRD-approved modules: `domain/`, `storage/`, `gateway/`, `worker/`, `context/`, `curator/`, `engram/`, `api/`.
- Workspace/Cargo wiring for the new crate.
- New `migrations/0010_life_mode.sql` for `life_*` tables.
- Core prompt-context seam needed to remove hardcoded wiki-only context assembly for future life provider use while preserving chat behavior.
- Life identity/principal model, active memory generations, canonical memory ledger, task state, friction/support state, event/run/input queue, and Engram outbox storage contracts.
- Storage/repository services for Postgres source-of-truth state.
- Transport-neutral `LifeGateway` submit contract.
- Life context assembler and provider contract for AuDHD operating profile, task resume, protocols, hot handoff, canonical memory, and Engram-derived evidence.
- Post-run curator and sensitivity-gate contracts, initially with deterministic/test doubles where live LLM/Engram is not yet verified.
- Engram adapter trait and outbox projection contracts; live HTTP adapter only after PRD §17.1 live contract verification.
- Tests and docs needed to prove the audit items.

Out of scope:
- Replacing ordinary web sessions or Telegram topic/session mode.
- Enabling Engram in ordinary chat mode.
- Replacing current wiki memory for ordinary chat mode.
- Using Engram `/v1/chat/completions` as agent runtime.
- Making Engram source of truth.
- Storing raw secrets in memory or Engram.
- Ambient group/forum listening as personal life memory.
- Cross-transport reminders unless a separate transport-agnostic reminder PRD/checkpoint is approved.
- Splitting life mode into multiple `oxide-agent-life-*` crates before proven need.
- Adding new LLM/provider infrastructure for the curator; use existing `llm/client.rs` routes.

## Missing Inputs

- Engram live contract and legal path are not yet verified for implementation of the concrete HTTP adapter.
  - Impact: `engram/` can define traits, outbox semantics, and test doubles, but production HTTP calls must stay behind a blocked/disabled adapter until raw responses and licensing path are documented.
  - Low-risk assumption or fallback: implement a `Noop`/test backend plus trait and outbox projection; keep PG source-of-truth fully functional without Engram.
  - User/external action needed: approve AGPL/commercial/rewrite path before shipping a modified networked Engram service.

## Repository Context

- Relevant PRD source: `docs/prd/PRD-perm.md`.
- Existing durable storage: SQLx/Postgres in `crates/oxide-agent-core/src/storage/`, migrations under `migrations/`.
- Existing stable hot-memory scope: `AgentMemoryScope(user_id, context_key, flow_id)` and `agent_memory_snapshots` can support `(principal_user_id, "life", "main")`.
- Existing process-local runtime primitives: `RuntimeContextInbox` and `SessionRegistry`; life mode must persist queue/state in Postgres instead of using process memory as source of truth.
- Existing prompt seam problem: wiki context is hardwired in `AgentExecutor` / prompt composer; life mode needs a generic dynamic context provider while chat mode keeps wiki behavior.
- Existing validation infrastructure:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - `cargo test --workspace --no-default-features --features profile-embedded-opencode-local`
  - `cargo run -p xtask -- module-registry check` if module registry/profile wiring changes.
  - If `oxide-agent-web-ui` changes: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown`.
- Known current anomaly: prior full workspace tests can fail in `crates/oxide-agent-core/tests/proptest_remediation.rs` on whitespace-only `thought`; classify with evidence if it recurs.

## Contract Boundary Analysis

### Transport -> LifeGateway

- Sender: Web `/life` route or Telegram private-DM life router.
- Receiver: `oxide-agent-life::gateway`.
- Reliable sender knowledge: `provider`, `provider_subject`, content, attachments/refs, correlation id, transport metadata.
- Receiver-owned responsibilities: resolve/link principal, create first generation if needed, write `life_turns`, queue `life_inputs`, attach/start run.
- Forbidden sender requirements: transport must not choose `principal_user_id`, `context_key`, `flow_id`, run state, memory ids, or Engram ids.

### LifeGateway -> LifeWorker

- Sender: gateway/storage queue.
- Receiver: DB-backed worker/orchestrator.
- Reliable contract: `process_principal_input(principal_user_id, input_id)`.
- Receiver-owned responsibilities: advisory lock, active generation, prompt context assembly, executor hydration, queue draining, final checkpoint, events, curator/outbox.

### LifeRuntime -> Memory Storage

- Sender: worker/context/curator/storage services.
- Receiver: Postgres source-of-truth tables.
- Required contract: every rebuildable materialized memory row is scoped by active `memory_generation_id`; `life_turns` remains generation-independent source evidence.
- Forbidden contract: reading memory-owned rows by `principal_user_id` alone.

### LifeRuntime -> Engram

- Sender: outbox worker/recall adapter.
- Receiver: derived long-term memory engine.
- Reliable sender knowledge: tenant/principal, active `memory_generation_id`, source turn/memory ids, roles, trusted tool outputs, sensitivity outcome.
- Receiver-owned responsibilities: index/retrieve derived candidates only; no source-of-truth writes.
- Forbidden contract: LLM or runtime depending on raw Engram ids as authoritative memory ids.

### LLM -> Life Memory Tools

- Sender: model-visible assistant.
- Receiver: life tool/runtime layer.
- Reliable sender knowledge: intent, descriptors, content, category/permanence hints.
- Receiver-owned responsibilities: resolve descriptors to scoped local handles or canonical PG ids, enforce confirmation/sensitivity/generation rules.
- Forbidden contract: LLM supplies raw Engram `fact_id`/`episode_id` or downstream state guesses.

## Completion Audit

### Functional requirements (G*)

- G1: Core has a generic dynamic prompt context seam.
  - Source: PRD §2.7, §5 Mine 7, §11.1.
  - Acceptance: prompt assembly can accept typed dynamic context blocks with semantics; ordinary chat mode continues to use wiki context with behavior change `0`; life mode can provide its own blocks without hardcoded composer special-cases.
  - Evidence required: code review/grep proving wiki-only hardcoding is behind a provider adapter; tests for chat-mode prompt output compatibility and life block ordering/semantics.
  - Status: verified
  - Evidence collected: Phase 1 introduced `agent::prompt::context::{DynamicPromptContextProvider, PromptContextBlock, PromptContextSemantics, PromptContextRequest}`; `AgentExecutor` now builds dynamic prompt context blocks through a provider; `WikiPromptContextProvider` adapts existing wiki assembly; composer accepts typed blocks and no longer accepts a wiki-specific string. Tests: `test_create_agent_system_prompt_appends_wiki_context`, `test_create_agent_system_prompt_preserves_dynamic_block_order`, `test_wiki_context_after_workflow_guidance`, `wiki_prompt_context_provider_returns_evidence_block`, `fresh_web_session_wiki_context_skip_is_limited_to_first_message`. Grep: no stale `render_wiki_context_for_task`, `normalize_wiki_context`, or `wiki_context_available` symbols under `crates/oxide-agent-core/src`.

- G2: `oxide-agent-life` bounded context exists with approved module ownership.
  - Source: PRD §21.1.
  - Acceptance: workspace includes `crates/oxide-agent-life` with `domain/`, `storage/`, `gateway/`, `worker/`, `context/`, `curator/`, `engram/`, `api/`; core does not contain a product `src/life` subsystem; transports do not own memory semantics.
  - Evidence required: `Cargo.toml` workspace diff, crate tree inspection, `cargo check` for the new crate.
  - Status: verified
  - Evidence collected: Phase 2 added workspace member `crates/oxide-agent-life` in root `Cargo.toml` plus `Cargo.lock` package entry. Crate tree contains PRD-approved modules: `api/`, `context/`, `curator/`, `domain/`, `engram/`, `gateway/`, `storage/`, `worker/`. `domain/` contains typed contracts for principal/identity, generations, scoped memory, memory items, task state, friction patterns, support protocols, turns, inputs, runs, events, context overrides, ids, and timestamps. `glob crates/oxide-agent-core/src/life/**` found no core product subsystem; `git diff --name-only -- crates/oxide-agent-core crates/oxide-agent-transport-web crates/oxide-agent-transport-telegram` returned no touched core/transport files. `cargo check -p oxide-agent-life` and `cargo test -p oxide-agent-life` passed.

- G3: Life storage migration implements the Postgres source-of-truth schema.
  - Source: PRD §8.3, §9, §10.7, §21.
  - Acceptance: `migrations/0010_life_mode.sql` creates `life_principals`, `life_identity_links`, `life_link_tokens`, `life_turns`, `life_memory_generations`, `life_active_memory_generations`, `life_memory_items`, `life_task_states`, `life_friction_patterns`, `life_support_protocols`, `life_inputs`, `life_runs`, `life_events`, `life_context_overrides`, `life_engram_outbox` with generation FKs/order and stable constraints.
  - Evidence required: migration review; SQLx/storage integration test or migration-run test; `cargo test` evidence for storage round trips where practical.
  - Status: verified
  - Evidence collected: Phase 3 added `migrations/0010_life_mode.sql` creating `life_principals`, `life_identity_links`, `life_link_tokens`, `life_memory_generations`, `life_active_memory_generations`, `life_turns`, `life_runs`, `life_inputs`, `life_events`, `life_context_overrides`, `life_memory_items`, `life_task_states`, `life_friction_patterns`, `life_support_protocols`, and `life_engram_outbox`. Migration order creates generations before any `memory_generation_id` FK. Generation-owned tables use composite `(principal_user_id, memory_generation_id)` FKs to prevent cross-principal generation drift. Focused clean-Postgres validation passed: `docker run --rm -d --name oxide-agent-life-test-pg ... postgres:18-alpine`; `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@127.0.0.1:55433/oxide_agent_test cargo test -p oxide-agent-life sqlx_life_storage_migrates_and_scopes_memory_by_active_generation -- --nocapture` (1 passed); container stopped after validation.

- G4: Principal identity and linking model is transport-neutral.
  - Source: PRD §2.4, §5 Mine 1, §6.1, §8.
  - Acceptance: `(provider, provider_subject)` resolves to a canonical `principal_user_id`; web and Telegram subjects can link to one principal; transports do not pass life identity internals.
  - Evidence required: unit/integration tests for principal creation, link lookup, duplicate link rejection, web-first token linking, Telegram-first allocator path or documented blocked subset.
  - Status: pending
  - Evidence collected: Phase 3 SQLx repository adds `upsert_principal`, `link_identity`, and `resolve_identity(provider, provider_subject)` over `life_principals` and `life_identity_links`; focused storage test validates web provider subject resolves to the canonical `principal_user_id`. Phase 4 adds transport-neutral `LifeGateway` with receiver-owned `LifePrincipalAllocator`, existing identity reuse, new principal onboarding, and first-generation bootstrap. `link_identity` now rejects mismatched duplicate provider subjects instead of relinking them; SQLx storage test asserts `IdentityLinkConflict` for duplicate `(provider, provider_subject)` with another principal. Gateway tests prove transports submit only provider/subject/content/attachments/metadata and never principal/generation ids. Full G4 remains pending for explicit link-token flow coverage.

- G5: Memory generations are first-class and every memory-owned read path is generation-scoped.
  - Source: PRD §1, §9.6-9.12, §10.2 Layer 0, §10.7, §20 criteria 19-21.
  - Acceptance: active pointer is loaded before memory reads; `life_memory_items`, `life_task_states`, `life_friction_patterns`, `life_support_protocols`, `life_engram_outbox`, and `life_runs` use `memory_generation_id`; old/archived/deleted generations cannot enter prompt or Engram recall.
  - Evidence required: repository grep/call-site audit; unit tests for build N+1 without mutating active N, activation/rollback pointer switch, derived wipe/generation wipe/soft reset semantics.
  - Status: pending
  - Evidence collected: Phase 2 added domain-level generation contracts: `MemoryGenerationId`, `MemoryScope`, `ActiveMemoryGeneration`, `GenerationScoped`, `LifeMemoryGeneration`, generation-scoped `LifeRun`, `LifeMemoryItem`, `LifeTaskState`, `LifeFrictionPattern`, `LifeSupportProtocol`, `CuratorCandidate`, `EngramNamespace`, and `SubmitLifeInputResult`. Unit test `generation_scoped_records_require_active_scope` proves stale generation rows are rejected by the domain guard. Phase 3 adds SQL schema composite generation FKs and repository methods that require explicit `MemoryScope` for active memory/task/friction/protocol reads. Focused clean-Postgres test proves generation N+1 rows can be inserted without mutating active generation N, activation switches the active pointer, and rollback returns reads to generation N. Full G5 remains pending for Engram recall namespace enforcement and wipe/reset services.

- G6: LifeGateway submit path writes canonical turns and queued inputs.
  - Source: PRD §6.1, §12.1, §16 examples.
  - Acceptance: `submit_life_input(provider, provider_subject, content, attachments, metadata)` resolves principal, creates first generation if needed, writes user `life_turns`, writes queued `life_inputs`, and returns queued/started run info without caller-selected context ids.
  - Evidence required: gateway tests for first-run generation creation, existing principal path, unknown/unlinked provider behavior, attachment metadata preservation.
  - Status: verified
  - Evidence collected: Phase 4 implements `LifeGateway::submit_life_input` over the narrow `LifeInputSubmission { provider, provider_subject, content, attachments, metadata }` contract. Gateway resolves existing identity or allocates/creates a new `life_principals` row through `LifePrincipalAllocator`, creates and activates first `life_memory_generations` row when no active generation exists, appends a user `LifeTurn`, enqueues a queued `LifeInput`, and returns `SubmitLifeInputResult` with resolved principal, active `MemoryScope`, turn id, input id, and `run_id: None` until worker ownership in Phase 6. `life_turns`/`LifeTurn` gained `transport_metadata` so gateway preserves submitted metadata durably instead of dropping/smuggling it. Tests: `submit_creates_principal_generation_turn_and_input`, `submit_reuses_existing_identity_and_active_generation`, `submit_rejects_empty_content_before_allocating_principal`, and real-Postgres `sqlx_life_gateway_submit_persists_turn_metadata_and_input`.

- G7: LifeWorker runtime is DB-backed, serialized, and restart-safe.
  - Source: PRD §2.5-2.6, §5 Mine 2, §12.
  - Acceptance: worker uses per-principal advisory lock, active generation, DB queue, `AgentMemoryScope(principal, "life", "main")`, final synchronous checkpoint, `life_events`, and safe draining of `life_inputs` into runtime injections.
  - Evidence required: unit/integration tests for lock key, queue claim/drain, continuation boundary, final checkpoint transaction, crash-before-Engram-flush persistence.
  - Status: pending
  - Evidence collected:

- G8: AuDHD context provider assembles deterministic support context before evidence recall.
  - Source: PRD §4.2, §5 Mine 9, §6.5, §10, §11, §16.6-16.7.
  - Acceptance: prompt blocks include life defaults, operating profile, active task resume/open loops, overrides, support protocols/friction patterns, hot handoff, and long-term evidence in PRD precedence order; long-term memory is framed as evidence, not instruction.
  - Evidence required: context assembler tests for block order, semantics, active-generation filtering, context restore scenario, overload/option narrowing scenario.
  - Status: pending
  - Evidence collected:

- G9: Canonical memory ledger and support tables are Postgres-authoritative.
  - Source: PRD §9.8-9.11, §10.4, §13.5-13.6.
  - Acceptance: durable memory writes go to `life_memory_items` and support/task tables first; Engram receives only outbox projections; explicit permanent memory is recallable via PG dereference; assistant free-form output is not authoritative by default.
  - Evidence required: repository tests for explicit remember, profile/operating candidate rules, task-state updates, friction/protocol candidates, PG dereference after recall candidate.
  - Status: pending
  - Evidence collected: Phase 3 added `life_memory_items`, `life_task_states`, `life_friction_patterns`, and `life_support_protocols` tables plus SQLx repository upsert/read methods for canonical memory, task state, friction patterns, and support protocols. Clean-Postgres test inserts old/new generation rows for all four categories and verifies active reads return only the active generation. Full G9 remains pending for curator/candidate writer, Engram outbox projection, and PG dereference after recall.

- G10: Secret/sensitivity gate prevents raw secrets from entering durable memory/Engram.
  - Source: PRD §2.8, §5 Mine 6, §10.5, §16.5, §17.2 check 8.
  - Acceptance: secret-like inputs are redacted/denied or routed to `private_secrets`; no raw secret appears in `life_memory_items` or `life_engram_outbox`; transcript redaction state is respected.
  - Evidence required: tests for API key/token input, redacted transcript, denied outbox, private secret routing or explicit refusal path.
  - Status: pending
  - Evidence collected:

- G11: Curator is a single structured-output post-run step using existing LLM client and cannot mutate source-of-truth directly.
  - Source: PRD §10.6, §12.3, §20 criteria 9,16,17.
  - Acceptance: curator produces typed candidates (`durable_fact`, `project_decision`, `operating_profile_update`, `task_state_update`, `friction_pattern`, `support_protocol`, `ephemeral`, `secret_candidate`, `skip`); writes pass through confirmation/sensitivity/candidate writer; no new LLM provider/client is introduced.
  - Evidence required: structured schema tests, deterministic/test LLM tests, grep proving no direct curator writes to `profile_state`/`operating_profile`/Engram.
  - Status: pending
  - Evidence collected:

- G12: Engram integration is derived, generation-scoped, and replaceable.
  - Source: PRD §3, §6.3, §13, §20 criteria 1,7,8,15,18.
  - Acceptance: `LifeLongTermMemoryBackend` trait exists; outbox worker projects canonical memory with idempotency/generation namespace; recall returns candidates that are dereferenced in PG; ordinary chat mode makes no Engram calls; HTTP adapter is gated behind verified contract/legal path.
  - Evidence required: trait/API tests, outbox idempotency tests, recall dereference tests, grep audit for ordinary chat mode no Engram path, PRD §17.1 raw-response evidence before enabling live HTTP adapter.
  - Status: pending
  - Evidence collected:

- G13: Web/Telegram integration points preserve transport boundaries.
  - Source: PRD §14-15, §19.
  - Acceptance: Web `/life` and Telegram private-DM `/life` can submit to `LifeGateway`; ordinary web session and Telegram topic paths remain unchanged; linking flow is explicit; group/forum ambient life memory is not enabled.
  - Evidence required: route/handler tests or compile checks, grep/call-site audit of ordinary paths, manual/API tests where implemented.
  - Status: pending
  - Evidence collected:

- G14: User-visible inspector/API can view and manage memory state.
  - Source: PRD §14.2-14.3, §20 criteria 14.
  - Acceptance: API contracts exist for state, turns, events, profile, operating profile, task states, friction patterns, support protocols, memory search/conflicts/forget, generations rebuild/activate/wipe; UI implementation may be phased but contracts and backend paths must preserve source-of-truth semantics.
  - Evidence required: API contract tests, route tests, and wasm check if UI touched.
  - Status: pending
  - Evidence collected:

### Quality/security/non-functional requirements (Q*)

- Q1: Ordinary chat/topic/session mode is behavior-compatible.
  - Source: PRD §1, §4.3, §11.2, §19, §20 criterion 1.
  - Acceptance: no Engram calls or life memory injection in ordinary modes; wiki memory chat behavior remains intact.
  - Evidence required: tests/grep for chat-mode prompt assembly and Engram call absence; existing web/telegram checks pass.
  - Status: verified
  - Evidence collected: Phase 1 kept ordinary chat wiki memory behind `with_wiki_memory_store` / `set_wiki_memory_store`, which install `WikiPromptContextProvider` while preserving the wiki store for background updates. Prompt ordering tests verify dynamic/wiki blocks remain after workflow guidance and before structured output. Repo grep found no Engram/life integration in ordinary chat paths in Phase 1; no transport files changed.

- Q2: Postgres remains source of truth; process memory is never canonical life state.
  - Source: PRD §5 Mines 2-3, §7.1, §13.5.
  - Acceptance: life inputs/runs/events/checkpoints/memory are persisted; `SessionRegistry` can only be optimization, not source.
  - Evidence required: storage tests, restart/crash scenario tests, code audit.
  - Status: pending
  - Evidence collected: Phase 3 added source-of-truth life tables and `SqlxLifeStorage` repository over Postgres; no process-local registry/session state was introduced. Clean-Postgres test exercises persisted principals, identity links, generations, memory items, task states, friction patterns, and support protocols. Phase 4 gateway writes canonical turns and queued inputs through storage, and real-Postgres focused test verifies submitted content, attachments, transport metadata, active generation, and queued input are persisted. Full Q2 remains pending for worker/restart/crash scenarios.

- Q3: No raw secrets reach prompts, durable memory, or Engram.
  - Source: PRD §2.8, §5 Mine 6, §10.5.
  - Acceptance: sensitivity gate runs before canonical/outbox writes; prompt blocks and recalled evidence exclude secret-blocked content.
  - Evidence required: secret tests and prompt-context tests.
  - Status: pending
  - Evidence collected:

- Q4: AuDHD support is user-confirmed operating contract, not silent diagnosis.
  - Source: PRD §4.3, §10.2, §10.4, §16.7.
  - Acceptance: system never infers neurotype without confirmation; overload/context triggers use confirmed protocols; `operating_profile` updates require explicit confirmation/UI action.
  - Evidence required: tests for candidate-not-applied behavior and prompt assembly from confirmed state only.
  - Status: pending
  - Evidence collected:

- Q5: Memory rebuild/wipe is plastic and auditable.
  - Source: PRD §10.7, §20 criteria 19-21.
  - Acceptance: rebuild never mutates active generation; comparison report/policy/source scope are stored; wipe modes are distinct and deletion order is explicit.
  - Evidence required: generation lifecycle tests; API/service tests for rebuild/activate/rollback/wipe; code review of hard-wipe path.
  - Status: pending
  - Evidence collected: Phase 3 stores generation `build_policy`, `source_scope`, `comparison_report`, active pointer, activation timestamps, and active/archived status. Focused test verifies build-N+1/activate/rollback without overwriting active generation rows. Full Q5 remains pending for explicit derived wipe, generation wipe, soft reset, and privacy hard wipe services/API.

- Q6: No new unnecessary provider/client/storage abstractions.
  - Source: `AGENTS.md` implementation bias; PRD §10.6.
  - Acceptance: curator uses existing `llm/client.rs`; storage remains SQLx/Postgres; new crate is justified by PRD bounded context; no extra queues/services beyond Engram derived engine already in PRD.
  - Evidence required: dependency/Cargo diff review, grep for new clients/providers, Cargo metadata/check.
  - Status: pending
  - Evidence collected: Phase 2 introduced no provider/client implementations and no new external service. `oxide-agent-life` initially depended only on already-used workspace ecosystem crates (`serde`, `serde_json`, `thiserror`, `uuid`). Phase 3 added only already-used direct Postgres crates (`async-trait`, `sqlx-core`, `sqlx-postgres`, `tokio` dev-dependency) to implement the approved SQLx/Postgres source-of-truth storage; no new storage backend or provider/client was added. Phase 4 reused those dependencies and added no new crates/services/providers; gateway allocator/store/clock are narrow in-process traits needed to keep transport identity and time allocation out of callers. `cargo tree -p oxide-agent-life -i sqlx-sqlite` reported no matching package. `cargo run -p xtask -- module-registry check` passed in Phase 2; no `module_registry.toml` diff in Phases 3-4. Full Q6 remains pending because later curator/Engram phases must continue preserving the constraint.

### Validation requirements (V*)

- V1: Formatting and lint gates pass or failures are classified with evidence.
  - Source: `AGENTS.md` format/lint.
  - Evidence required: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings` or documented scoped alternative with exact blocker.
  - Status: verified
  - Evidence collected: Phase 1 ran `cargo fmt --all -- --check` (passed) and `cargo clippy --workspace --all-targets -- -D warnings` (passed). Phase 2 ran `cargo fmt --all -- --check` (passed) and `cargo clippy --workspace --all-targets -- -D warnings` (passed). Phase 3 ran `cargo fmt --all -- --check` (passed), `cargo clippy --workspace --all-targets -- -D warnings` (passed), and `git diff --check` (passed). Phase 4 ran `git diff --check` (passed), `cargo fmt --all -- --check` (passed), and `cargo clippy --workspace --all-targets -- -D warnings` (passed).

- V2: Workspace compile/test gates pass or failures are classified with evidence.
  - Source: `AGENTS.md` build/testing.
  - Evidence required: `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; `cargo test --workspace --no-default-features --features profile-embedded-opencode-local`; additional scoped tests for touched crates.
  - Status: verified
  - Evidence collected: Phase 1 ran `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (passed with existing rustc unused/dead-code warnings). Focused tests passed for dynamic block order and wiki provider/skip behavior. `cargo test --workspace --no-default-features --features profile-embedded-opencode-local` ran broadly and failed only on the known pre-existing `crates/oxide-agent-core/tests/proptest_remediation.rs` whitespace-only `thought` property; failing file had no diff and generated `.proptest-regressions` artifact was removed. Phase 2 ran `cargo check -p oxide-agent-life` (passed), `cargo test -p oxide-agent-life` (3 passed), `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (passed with the same existing core warnings), and workspace `cargo test --workspace --no-default-features --features profile-embedded-opencode-local` again failed only on the same known pre-existing core proptest with untouched failing file; generated `.proptest-regressions` artifact was removed. Phase 3 ran `cargo check -p oxide-agent-life` (passed), `cargo test -p oxide-agent-life` (4 passed, SQLx test skips without `OXIDE_DATABASE_TEST_URL`), clean-Postgres focused SQLx test with explicit `OXIDE_DATABASE_TEST_URL` (1 passed), `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (passed with existing core warnings), and `cargo test --workspace --no-default-features --features profile-embedded-opencode-local` (passed completely). Phase 4 focused checks: `cargo check -p oxide-agent-life` (passed), `cargo test -p oxide-agent-life` (8 passed; SQLx tests skip without `OXIDE_DATABASE_TEST_URL`), two real-Postgres focused SQLx tests passed against temporary `postgres:18-alpine` on port `55434`, `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed with existing core warnings, and `cargo test --workspace --no-default-features --features profile-embedded-opencode-local` passed.

- V3: Module registry/profile checks pass if profile/module wiring changes.
  - Source: `AGENTS.md` module registry.
  - Evidence required: `cargo run -p xtask -- module-registry check` after any module registry/profile change, or note that no module registry change occurred.
  - Status: verified
  - Evidence collected: Phase 2 added a workspace crate but did not change capability module/profile wiring; `git diff --name-only -- crates/oxide-agent-core/module_registry.toml` returned no changes. Additional guard `cargo run -p xtask -- module-registry check` passed: 39 modules, 44 Cargo features, 39 compiled declarations.

- V4: Web UI wasm check passes if `oxide-agent-web-ui` is touched.
  - Source: `AGENTS.md` format/lint.
  - Evidence required: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown` when UI changes; otherwise not applicable evidence.
  - Status: pending
  - Evidence collected:

- V5: Engram live contract verification is recorded before live HTTP adapter is enabled.
  - Source: PRD §17.1 and П0.5.
  - Evidence required: raw `/health`, `/v1/remember`, `/v1/recall`, facts/memories/conflicts/export responses against the chosen Engram deployment; legal path decision recorded.
  - Status: pending
  - Evidence collected:

### Non-goals and exclusions (N*)

- N1: Ordinary web sessions and Telegram topics are not replaced by life mode.
  - Source: PRD §4.3, §19.
  - Must preserve: existing `web-session-*` and Telegram context-key behavior remains available and semantically unchanged.
  - Evidence required: grep/call-site audit and existing transport tests/checks.
  - Status: pending
  - Evidence collected: Phase 4 changed only `oxide-agent-life`, `migrations/0010_life_mode.sql`, and this goal doc; `git diff --name-only -- crates/oxide-agent-core crates/oxide-agent-transport-web crates/oxide-agent-transport-telegram crates/oxide-agent-core/module_registry.toml` returned no changes. Gateway tests use fake submissions and do not alter Web/Telegram session/topic paths. Full N1 remains pending until later transport integration phases also preserve ordinary modes.

- N2: Engram is not source of truth and does not write back into Postgres.
  - Source: PRD §13.5-13.6.
  - Must preserve: Engram-derived recall only yields candidate/evidence; PG updates require user turns/UI actions/confirmed candidates through Oxide services.
  - Evidence required: code audit, adapter tests, grep for reverse-write paths.
  - Status: pending
  - Evidence collected:

- N3: Cross-transport reminders are not part of v1 life memory implementation.
  - Source: PRD §2.9, §5 Mine 8.
  - Must preserve: no reminder destination-contract rewrite unless separately approved.
  - Evidence required: diff review showing reminder schema/contract untouched or separately approved.
  - Status: pending
  - Evidence collected:

## Implementation Plan

Each phase ends with: update this goal doc, run relevant validation, inspect `git status`/diff, commit one coherent checkpoint, then call `compress` before starting the next phase.

### Phase 0. Goal contract and implementation RECON

- Audit IDs: all pending IDs setup.
- Expected changes: this goal doc only.
- Validation: `git diff --check`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` if practical; no code behavior changes.
- Exit condition: goal doc committed and session `/goal` points at this file.

### Phase 1. Core dynamic prompt context seam

- Audit IDs: G1, Q1, V1, V2.
- Expected changes: core prompt/executor types to support `DynamicPromptContextProvider`, `PromptContextBlock`, `PromptContextSemantics`; wiki context becomes behavior-preserving provider adapter.
- Validation: focused core tests for prompt ordering and chat-mode wiki compatibility; workspace check/test.
- Exit condition: ordinary chat prompt behavior is preserved and life provider can be added without composer special-case.

### Phase 2. Life crate skeleton and domain contracts

- Audit IDs: G2, G5, Q6, V1, V2, V3 if registry touched.
- Expected changes: `crates/oxide-agent-life/` crate, workspace membership, domain types for principal, generations, memory items, task states, protocols, inputs/runs/events, errors/config.
- Validation: `cargo check -p oxide-agent-life`; workspace check; module registry check only if capabilities/profiles change.
- Exit condition: life crate compiles with domain types and no transport ownership leaks.

### Phase 3. Postgres migration and storage repositories

- Audit IDs: G3, G4, G5, G9, Q2, Q5, V1, V2.
- Expected changes: `migrations/0010_life_mode.sql`; storage repository traits/SQLx implementation/test support; active generation lifecycle operations.
- Validation: SQLx/storage tests for migrations and CRUD/generation lifecycle; workspace check/test.
- Exit condition: all source-of-truth tables can be created and exercised in tests; every memory-owned query is generation-scoped.

### Phase 4. LifeGateway submit path

- Audit IDs: G4, G6, Q2, N1, V1, V2.
- Expected changes: transport-neutral gateway service resolving principal/linking, creating first generation, appending turns, queueing inputs.
- Validation: gateway tests with web/telegram provider subjects; no transport path rewrites.
- Exit condition: `submit_life_input` contract is implemented and verified without Web/Telegram UI changes.

### Phase 5. Context assembler and AuDHD memory provider

- Audit IDs: G1, G5, G8, G9, Q1, Q4, V1, V2.
- Expected changes: `context/` provider that loads active generation, profile/operating state, task resume, overrides, protocols, hot handoff, canonical evidence; tests for precedence and examples.
- Validation: context block tests for PRD §11/§16.6/§16.7; chat-mode compatibility tests.
- Exit condition: life prompt context pack can be assembled deterministically from PG/test repositories.

### Phase 6. Worker runtime and DB-backed continuation

- Audit IDs: G7, G8, Q2, V1, V2.
- Expected changes: worker/orchestrator skeleton with advisory lock, queue claim/drain, stable `AgentMemoryScope`, event writing, final checkpoint contract; use test executor/mocks where needed.
- Validation: concurrency/lock tests, continuation safety tests, final checkpoint tests.
- Exit condition: worker can process queued input in tests and persist restart-safe state without Engram.

### Phase 7. Canonical memory writer, sensitivity gate, and curator contract

- Audit IDs: G9, G10, G11, Q3, Q4, Q6, V1, V2.
- Expected changes: typed curator schema, deterministic/test curator adapter, sensitivity gate, candidate writer, confirmation rules for profile/operating updates.
- Validation: explicit remember tests, borderline friction/protocol tests, secret-deny tests, no-direct-write grep audit.
- Exit condition: post-run memory candidates become canonical PG writes/outbox projections only through gate/confirmation policy.

### Phase 8. Engram trait, outbox projector, and derived recall

- Audit IDs: G9, G12, N2, V1, V2, V5.
- Expected changes: `LifeLongTermMemoryBackend` trait, no-op/test backend, outbox worker semantics, generation-scoped idempotency/namespace, recall candidate dereference.
- Validation: trait/outbox/dereference tests; ordinary chat no-Engram grep; live HTTP adapter remains disabled/blocked until V5 raw evidence exists.
- Exit condition: derived-memory contract works with test backend and cannot become source of truth.

### Phase 9. Backend Web/Telegram integration points

- Audit IDs: G13, G14, N1, N3, V1, V2, V4 if UI touched.
- Expected changes: minimal backend routes/handlers to submit life messages and inspect state; Telegram private-DM command/linking hooks only if safe; no group/forum ambient memory.
- Validation: route/handler tests; existing transport checks; wasm check if UI changes.
- Exit condition: transports call `LifeGateway` through the narrow contract and ordinary modes remain unchanged.

### Phase 10. Rebuild/wipe tooling and final hardening

- Audit IDs: G5, G10, G12, G14, Q3, Q5, V1-V5.
- Expected changes: generation rebuild/compare/activate/rollback, derived wipe, generation wipe, soft reset, privacy hard wipe services/API; final docs and audit.
- Validation: generation lifecycle/wipe tests; full validation contract; final Completion Audit.
- Exit condition: every Completion Audit item is verified or explicitly blocked with exact evidence and user-needed action.

## Validation Contract

- Static checks:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `git diff --check`
- Compile checks:
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - scoped `cargo check -p <crate>` for touched crates where faster feedback is needed
  - `cargo run -p xtask -- module-registry check` if module registry/profile wiring changes
- Tests:
  - `cargo test --workspace --no-default-features --features profile-embedded-opencode-local`
  - focused `cargo test -p oxide-agent-life` once the crate exists
  - focused core/transport tests for changed areas
- Web UI:
  - `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown` if `oxide-agent-web-ui` changes
- External/runtime verification:
  - PRD §17.1 Engram raw-response verification before enabling live Engram HTTP adapter
  - explicit legal path decision before shipping modified Engram as network service
- Done when: every Completion Audit item is `verified`, all required gates either pass or have classified pre-existing/environment blockers, and final audit is filled.

## Decisions

- 2026-06-24: Source spec is `docs/prd/PRD-perm.md`, not a separate `PRD.md`, because it is the opened/user-referenced Permanent Life Mode PRD and contains the full approved architecture.
- 2026-06-24: Use `docs/goals/2026-06-24-permanent-life-memory.md` following existing repo `docs/goals/<date>-<slug>.md` convention.
- 2026-06-24: Treat live Engram HTTP adapter as blocked behind PRD §17.1 verification; implement traits/test backend first so PG source-of-truth work can proceed.
- 2026-06-24: Commits are per phase/checkpoint; after each phase commit, compress conversation context before continuing.
- 2026-06-24: `oxide-agent-life` uses direct `sqlx-core` + `sqlx-postgres` dependencies, matching existing repo SQLx/Postgres practice and avoiding the top-level `sqlx` crate/SQLite feature surface.
- 2026-06-24: Phase 4 adds `life_turns.transport_metadata` because the approved Transport -> LifeGateway contract includes metadata; dropping it or hiding it in attachments/source_ref would violate the boundary.
- 2026-06-24: `LifeGateway` creates DB-queued input records but leaves `run_id` as `None` until Phase 6 worker/runtime owns run claiming and lifecycle; process memory does not become canonical run state.

## Progress Log

- 2026-06-24 00:00: Phase 0 started — goal contract creation.
  - Changed: created this goal document from `docs/prd/PRD-perm.md`.
  - Evidence: preflight read `AGENTS.md`, `README.md`, root `Cargo.toml`, existing `docs/goals/2026-06-22-compaction-dcp-redesign.md`, and PRD sections covering solution, contracts, storage, memory model, runtime, Engram, UX, verification, implementation, acceptance, and code boundary.
  - Commands: `git diff --check` (passed), `cargo fmt --all -- --check` (passed), `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (passed with existing warnings), `cargo test --workspace --no-default-features --features profile-embedded-opencode-local` (passed).
  - Audit IDs updated: all initialized as pending.
  - Next: commit Phase 0 goal contract, then compress and start Phase 1.

- 2026-06-24 00:00: Session `/goal` set.
  - Changed: updated `Codex goal` metadata with the active session objective.
  - Evidence: `set_goal` returned active goal for this document.
  - Commands: `set_goal`.
  - Audit IDs updated: Phase 0 metadata only.
  - Next: commit Phase 0 goal contract.

- 2026-06-24 01:00: Phase 1 completed — core dynamic prompt context seam.
  - Changed: added typed dynamic prompt context module; changed composer to accept ordered `PromptContextBlock`s; added `DynamicPromptContextProvider` field/setters to `AgentExecutor`; moved ordinary wiki prompt context behind `WikiPromptContextProvider`; preserved wiki background writer storage path.
  - Evidence: grep found no stale `render_wiki_context_for_task`, `normalize_wiki_context`, or `wiki_context_available`; `git diff --check` passed; `cargo fmt --all -- --check` passed; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed; `cargo clippy --workspace --all-targets -- -D warnings` passed; focused prompt/wiki tests passed; full workspace test failure classified as known pre-existing proptest whitespace-only `thought` issue with untouched failing file.
  - Commands: `cargo check -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local`; focused `cargo test -p oxide-agent-core --no-default-features --features profile-embedded-opencode-local <test-filter>` for dynamic block order/wiki provider/skip tests; `cargo fmt --all -- --check`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-default-features --features profile-embedded-opencode-local`.
  - Audit IDs updated: G1, Q1, V1, V2 verified for current repo state.
  - Next: commit Phase 1, compress, then start Phase 2 life crate skeleton/domain contracts.

- 2026-06-24 02:00: Phase 2 completed — life crate skeleton and domain contracts.
  - Changed: added `crates/oxide-agent-life/` workspace member with PRD-approved module tree; added pure domain contracts for principals, identity links, generations, memory scope guards, canonical memory items, task states, friction patterns, support protocols, turns, inputs, runs, events, context overrides, gateway submit DTOs, worker/context/curator/Engram/API skeleton contracts; updated `Cargo.lock`; corrected stale audit statuses that had incorrectly marked G2/G3/G4 verified with Phase 1 evidence.
  - Evidence: crate tree inspection found `api/`, `context/`, `curator/`, `domain/`, `engram/`, `gateway/`, `storage/`, `worker/`; no `crates/oxide-agent-core/src/life/**`; no core/web/telegram transport diffs; no module registry diff; `cargo run -p xtask -- module-registry check` passed.
  - Commands: `cargo check -p oxide-agent-life` (passed), `cargo test -p oxide-agent-life` (3 passed), `cargo fmt --all -- --check` (passed), `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (passed with existing core warnings), `cargo clippy --workspace --all-targets -- -D warnings` (passed), `cargo test --workspace --no-default-features --features profile-embedded-opencode-local` (failed only on known pre-existing core proptest whitespace-only `thought`; artifact removed), `cargo run -p xtask -- module-registry check` (passed).
  - Audit IDs updated: G2 verified; G5 partial evidence added but remains pending for storage/read-path implementation; Q6 partial evidence added but remains pending for future provider/client phases; V1/V2 evidence extended; V3 verified.
  - Next: commit Phase 2, compress, then start Phase 3 Postgres migration and storage repositories.

- 2026-06-24 03:00: Phase 3 completed — Postgres migration and storage repositories.
  - Changed: added `migrations/0010_life_mode.sql`; added SQLx/Postgres storage dependencies to `oxide-agent-life`; implemented `SqlxLifeStorage` and `LifeStorageRepository` for principal upsert, identity link/resolve, memory generation insert/activate/active pointer, generation-scoped canonical memory/task/friction/protocol upsert and active reads; exported storage SQLx module.
  - Evidence: migration creates all PRD Phase 3 source-of-truth tables with composite `(principal_user_id, memory_generation_id)` FKs for generation-owned rows; focused clean-Postgres test passed against temporary `postgres:18-alpine` on port `55433`; repository grep shows active memory/task/friction/protocol reads filter by both `principal_user_id = $1` and `memory_generation_id = $2`; no core/web/telegram/module-registry diffs.
  - Commands: `cargo fmt --all -- --check` (passed), `cargo check -p oxide-agent-life` (passed), `cargo test -p oxide-agent-life` (4 passed), `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@127.0.0.1:55433/oxide_agent_test cargo test -p oxide-agent-life sqlx_life_storage_migrates_and_scopes_memory_by_active_generation -- --nocapture` (1 passed against clean temporary Postgres), `cargo clippy --workspace --all-targets -- -D warnings` (passed), `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (passed with existing core warnings), `cargo test --workspace --no-default-features --features profile-embedded-opencode-local` (passed), `git diff --check` (passed), `cargo tree -p oxide-agent-life -i sqlx-sqlite` (no matching package).
  - Audit IDs updated: G3 verified; G4/G5/G9/Q2/Q5/Q6 partial evidence added; V1/V2 evidence extended.
  - Next: commit Phase 3, compress, then start Phase 4 LifeGateway submit path.

- 2026-06-24 04:00: Phase 4 completed — LifeGateway submit path.
  - Changed: implemented transport-neutral `LifeGateway` with narrow store/allocator/clock boundaries; added first-generation bootstrap, existing identity reuse, new principal onboarding, canonical `LifeTurn` append, queued `LifeInput` creation, empty-content rejection, metadata preservation, and duplicate identity-link conflict semantics; added `transport_metadata` to `life_turns`/`LifeTurn`.
  - Evidence: gateway unit tests cover new principal+generation+turn+input path, existing linked identity with active generation, and empty-content rejection. SQLx storage now supports `next_memory_generation_number`, `append_turn`, and `enqueue_input`; real-Postgres `sqlx_life_gateway_submit_persists_turn_metadata_and_input` verifies submitted Telegram content, attachments, metadata, active generation, and queued input persisted in DB. Existing SQLx generation test now also checks duplicate provider-subject relinking is rejected.
  - Commands: `git diff --check` (passed), `cargo fmt --all -- --check` (passed), `cargo check -p oxide-agent-life` (passed), `cargo test -p oxide-agent-life` (8 passed), `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@127.0.0.1:55434/oxide_agent_test cargo test -p oxide-agent-life sqlx_life_gateway_submit_persists_turn_metadata_and_input -- --nocapture` (1 passed against temporary Postgres), `OXIDE_DATABASE_TEST_URL=postgres://oxide_agent:oxide_agent@127.0.0.1:55434/oxide_agent_test cargo test -p oxide-agent-life sqlx_life_storage_migrates_and_scopes_memory_by_active_generation -- --nocapture` (1 passed), temp container stopped, `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (passed with existing core warnings), `cargo clippy --workspace --all-targets -- -D warnings` (passed), `cargo test --workspace --no-default-features --features profile-embedded-opencode-local` (passed).
  - Audit IDs updated: G6 verified; G4/Q2/Q6/N1 partial evidence added; V1/V2 evidence extended.
  - Next: run final Phase 4 validation, commit, compress, then start Phase 5 context assembler and AuDHD memory provider.

## Risks and Blockers

- Engram live/legal path not verified.
  - Impact: live HTTP adapter cannot be safely enabled.
  - Evidence: PRD status says implementation after live contract verification; PRD §3.4 license requires AGPL/commercial/rewrite choice.
  - Mitigation or requested decision: keep adapter trait/test backend until raw API responses and legal path are recorded.
  - Audit IDs affected: G12, V5.

- Workspace full tests may expose unrelated proptest failure.
  - Impact: full test gate may need classification rather than green result until pre-existing issue is fixed.
  - Evidence: prior runs failed in `crates/oxide-agent-core/tests/proptest_remediation.rs` with whitespace-only `thought` while only PRD docs changed.
  - Mitigation or requested decision: rerun, classify by diff/import evidence, remove generated `.proptest-regressions` artifact if produced.
  - Audit IDs affected: V2.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
