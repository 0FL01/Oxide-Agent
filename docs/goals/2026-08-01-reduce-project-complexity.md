# Goal: Reduce duplicate owners and supported variants

Status: complete
Source: User-approved simplification plan and audit, 2026-08-01
Last updated: 2026-08-01

## Objective

Remove the approved dead paths, duplicate runtime owners, aggregate context writer, Telegram ingress duplication, and obsolete deployment profiles; finish with one supported root-Compose `profile-full` deployment running on the current host.

## Execution Directive

Complete the frozen Required Outcomes using the listed Change Envelope and Primary Evidence. Work on the smallest unresolved outcome. Do not add requirements from reviews, tests, tools, speculative risks, or optional source text. Finish when every required outcome is resolved and affected constraints remain satisfied.

## Frozen Contract

### Required Outcomes

- R1: Life has one typed memory-checkpoint path and only bounded transcript/event read APIs.
  - Source: Approved M1.1 plan.
  - Acceptance: The raw Life checkpoint contract, unused outcome memory payload, `StableLifeMemoryScope`, and unpaged `list_turns`/`list_events` are absent; typed flow checkpointing, checkpoint timestamps, and privacy wipe remain.
  - Primary evidence: Focused `oxide-agent-life` and Web Life tests.
  - Status: verified
  - Evidence: Raw checkpoint contract/outcome payload and unpaged reads removed; 30 Life tests and 7 focused Web Life executor tests pass; source search finds none of the removed symbols.
- R2: Web task delivery has one active event representation.
  - Source: Approved M1.2 plan.
  - Acceptance: The lightweight `TaskEventEntry` buffer and unused `WebAgentTransport` are absent; persisted replay, broadcast, historical event deserialization, status/progress caches, and task timeline remain.
  - Primary evidence: Web transport unit and SSE replay/live-delivery tests.
  - Status: verified
  - Evidence: Lightweight buffer, collector writes/argument, and `WebAgentTransport` removed (251 net LOC); 22 focused Web event tests and 5 SSE tests pass; removed-symbol search is empty.
- R3: Compaction has no dead scope/budget protocol or duplicate selection owner.
  - Source: Approved M1.3 plan.
  - Acceptance: `CompactionScope` and unread request/trigger state are absent; selection/block resolution is owned by `CompactionEngine`; candidate apply, final apply, graph validation, tool-batch atomicity, and no-op comparison remain.
  - Primary evidence: Focused core compaction tests.
  - Status: verified
  - Evidence: Duplicate controller helpers, unread request/trigger protocol, and `CompactionScope` forwarding removed; focused engine/controller/budget/runner/session tests pass and removed-symbol searches are empty.
- R4: Internal provider/capability compatibility shells are removed without changing live protocols.
  - Source: Approved M1.4 plan.
  - Acceptance: The unused Anthropic module re-export, OpenAI Base profile alias, unused `ModuleRegistry`, and proven duplicate OpenRouter helpers are absent; unique assertions live with canonical implementations and Anthropic Messages protocol identifiers remain.
  - Primary evidence: Focused provider tests and module-registry check.
  - Status: verified
  - Evidence: Anthropic/module registry shells, OpenAI Base alias, and OpenRouter helper removed; unique assertions now live in canonical profile/request tests; registry and focused provider tests pass; live Anthropic protocol identifiers remain.
- R5: Telegram mechanical paths have one session ID, cancellation result, view owner, and progress transport.
  - Source: Approved M1.5 plan.
  - Acceptance: `AgentModeSessionKeys`, impossible todo-clear outcome, sole-implementation `AgentView` trait, and duplicate progress transport implementation are absent; session derivation, silent file delivery, loop notifications, thread targeting, and keyboard cleanup remain.
  - Primary evidence: Telegram library and topic/thread tests.
  - Status: verified
  - Evidence: Session wrapper, impossible cancellation state, sole view trait/dead entries, and duplicate silent transport removed; one transport keeps file/loop behavior and conditionally edits progress; 144 Telegram tests pass.
- R6: Application code no longer maintains the unread `agent_flows` registry.
  - Source: Approved M1.6 plan.
  - Acceptance: Flow record APIs, reads, writes, mocks, and duplicate state are absent; active flow pointers and memory snapshots remain; the existing table and cleanup-only deletes remain for data retention and rollback.
  - Primary evidence: Core storage and Telegram flow lifecycle tests plus source search.
  - Status: verified
  - Evidence: `AgentFlowRecord`, trait APIs, SQL read/upsert, builders/mappers, Telegram writes, in-memory state, mocks, and tests removed; only cleanup-only `DELETE FROM agent_flows` remains; core storage, Web, and full Telegram package tests pass.
- R7: Web, session runtime, and tool execution each have one internal owner for duplicated state.
  - Source: Approved M2 plan.
  - Acceptance: Web session DTO/hash/checkpoint/upload mechanics have one owner each; `SessionRegistry` uses one `SessionEntry` map; `ToolCatalog` is the sole executor/spec registry and preserves lazy/sub-agent behavior.
  - Primary evidence: Focused Web, runtime, tool runtime, delegation, and history-correlation tests.
  - Status: verified
  - Evidence: Web DTO/hash/checkpoint/upload duplicates removed; `SessionRegistry` uses one entry map; `ToolRegistry` and duplicate executor registration are absent, with `ToolCatalog` executing full filtered catalogs. Focused Web, runtime, tool-runtime, runner, delegation, and static-guard tests pass.
- R8: Context persistence uses row- and field-scoped operations instead of whole-user replacement.
  - Source: Approved M3 plan.
  - Acceptance: Telegram and manager production paths no longer call aggregate `update_user_config`; atomic flow ensure, DM mirror, context lifecycle, and model selection preserve existing rows without a schema change.
  - Primary evidence: Real-Postgres concurrent context, flow, DM mirror, manager lifecycle, and model-selection tests.
  - Status: verified
  - Evidence: Aggregate `UserConfig` and its get/update API are absent; Telegram and manager use row/field operations. Six real PostgreSQL context tests cover concurrent rows, atomic flow ensure, transactional DM mirror, model/field independence, and exact-row deletion; storage, manager, Telegram, and Web tests pass.
- R9: Telegram has one non-command ingress owner.
  - Source: Approved M4 plan.
  - Acceptance: Topic route resolves once; first accepted text/media input is processed once; commands, controls, confirmations, DM fallback, mention gating, thread isolation, preprocessing, and one activity touch remain; the handlers-to-agent-handlers cycle is absent.
  - Primary evidence: Telegram ingress, confirmation, and topic/thread integration tests.
  - Status: verified
  - Evidence: `State::AgentMode` is the default and sole non-command dispatcher path; duplicated first-message modality handlers, handler cycle, and repeated routing/state/activity paths are absent. Non-DM destructive confirmations use thread-local inline callbacks. Full Telegram package tests (143 library plus integrations/docs) and the full-profile bot check pass.
- R10: The repository supports one deployment profile and production Compose entrypoint.
  - Source: User confirmation that deployment will use one profile.
  - Acceptance: Root `docker-compose.yml` and `profile-full` are the supported deployment path; obsolete production split Compose files, slim Cargo profiles, reference profile TOMLs, and their CI/xtask/docs branches are absent; module registry and atomic module features remain.
  - Primary evidence: Module-registry check, full-profile workspace gates, root Compose validation, and release workflow inspection.
  - Status: verified
  - Evidence: Only `profile-full` and root `docker-compose.yml` remain; slim profile declarations/artifacts/snapshots, four split/overlay Compose files, orphaned SearXNG config, and current CI/workflow/docs branches are absent. Module-registry check, full workspace check, registry snapshot, Web tests, and root Compose validation pass.
- R11: The completed simplification is committed, pushed, built, and redeployed on the current host.
  - Source: Explicit user instruction.
  - Acceptance: Atomic implementation commits are on `origin/main`; release images build; the unified stack runs with migrations current and required services healthy.
  - Primary evidence: Clean synchronized Git state, successful Docker build/Compose rollout, migration query, Web health, container state, and startup logs.
  - Status: verified
  - Evidence: Implementation commits are synchronized with `origin/main`; full workspace tests, Clippy, fmt, module-registry, wasm, and Compose checks pass. Images `oxide-agent@fe0c48f83eca` and `oxide-agent-web@79d2d301eacc` built and run in root Compose; Web health is OK, sandboxd/browser-sidecar are healthy, migration `14|true` is current, Telegram/Web workers started, and recent service logs contain no errors.

### Constraints

- C1: Do not edit `.env`, `.env.example`, environment storage, or environment generation behavior.
- C2: Existing PostgreSQL data must be preserved. Do not edit applied migrations, drop `agent_flows`, or add a destructive migration.
- C3: Keep the implementation solo-project simple: no versioned release framework, immutable-tag framework, compatibility layer, new dependency, service, queue, cache, or generic repository abstraction.
- C4: Preserve current external HTTP/SSE/Telegram behavior and persisted historical deserialization unless an R-item explicitly changes an internal source surface.
- C5: Use one coherent commit per independently verifiable batch; run proportional checks before each commit and the full required gates once before final deployment.

### Non-goals

- Persistent SSH MCP session redesign or cancellation framework changes.
- Crawler tool-name contraction.
- Scalar/vector model configuration contraction or MiniMax identity migration.
- Reminder scheduler, Browser Live retention, Docker daemon relocation, or unrelated cleanup.
- Removing local-service support unless it is a direct orphan of the approved single deployment path.
- Supporting unknown external Rust consumers of internal `0.1` workspace crates through compatibility wrappers.

## Change Envelope

- Target: Approved M1-M5 symbols and their direct consumers/tests/docs; deployment composition/profile declarations and generated artifacts owned by the module registry.
- Expected paths, symbols, and direct consumers: `oxide-agent-life`; core compaction/providers/storage/tool runtime; Web event/session/Life paths and contracts/UI direct consumers; Telegram session/view/progress/ingress paths; runtime session registry; Cargo/profile/Compose/CI/xtask/current docs.
- Allowed artifacts: Rust source/tests, existing manifests, Compose/workflow/docs, checked-in generated profile artifacts, and this goal document.
- Forbidden artifacts: `.env*` changes; new dependencies/services/background mechanisms; destructive or compatibility migrations; old/new permanent runtime paths; speculative hardening.
- User or harness budget: optimize for deletion of owners, branches, state, and supported variants rather than line-golf; stop each batch once its stated owner is singular and evidence passes.

## Current Checkpoint

- Closes: None; the goal is complete.
- Smallest next action: Stop.
- Expected evidence: All R-items are verified below.
- Stop or replan if: Not applicable.

## Current State

- Resolved: R1-R11.
- Last relevant evidence: Full validation and current-host root-Compose rollout passed; Git is synchronized, migration 14 is current, Web health is OK, and required containers are running.
- Blocker: None.
- Next: None.

## Material Decisions

- 2026-08-01: Use root `docker-compose.yml` and `profile-full` as the sole deployment path.
- 2026-08-01: Do not touch environment files or their storage/generation behavior.
- 2026-08-01: Keep the program deletion-dominant; defer SSH, crawler, model-config, reminder, browser, and Docker-daemon redesigns.
- 2026-08-01: Preserve existing database state; leave obsolete tables inert or cleanup-only rather than dropping them.

## Checkpoint History

- 2026-08-01: Frozen from the user-approved audited plan; implementation not yet started.
- 2026-08-01: R1 checkpoint 1 removed the raw Life checkpoint contract and unused outcome memory payload (192 net LOC); focused Life and Web Life tests passed. Next: remove unpaged reads.
- 2026-08-01: R1 completed by removing uncalled unpaged Life turn/event queries (48 LOC); all 30 Life tests passed and removed-symbol search was empty. Next: R2.
- 2026-08-01: R2 removed the lightweight Web event plane and unused transport (251 net LOC) while retaining persisted replay/broadcast/caches/timeline; event and SSE tests passed. Next: R3.
- 2026-08-01: R3 checkpoint 1 made `CompactionEngine` the selection/block-resolution owner and deleted 52 duplicate controller LOC; engine/controller tests passed. Next: budget protocol deletion.
- 2026-08-01: R3 checkpoint 2 removed `CompactionRequest`, `CompactionTrigger`, and unread task/model/sub-agent plumbing (95 net LOC); focused budget and runner tests passed. Next: `CompactionScope` deletion.
- 2026-08-01: R3 completed by deleting `CompactionScope` and all trait/session projections (55 net LOC); session tests passed and references are absent. Next: R4.
- 2026-08-01: R4 checkpoint 1 removed the unused Anthropic compatibility module and `ModuleRegistry` (53 LOC); registry snapshot passed. Next: OpenAI Base alias.
- 2026-08-01: R4 checkpoint 2 replaced the OpenAI Base profile alias with direct canonical imports and moved unique assertions; profile/OpenAI Base tests passed. Next: OpenRouter helper.
- 2026-08-01: R4 completed by moving unique OpenRouter request assertions to the canonical owner and deleting the test helper (104 net LOC); request/response/OpenRouter tests passed. Next: R5.
- 2026-08-01: R5 checkpoint 1 removed singleton `AgentModeSessionKeys` and no-choice lookup helpers; 143 Telegram tests passed. Next: cancellation/view shells.
- 2026-08-01: R5 checkpoint 2 removed the impossible todo-clear result, sole view trait, and dead view entries; 143 Telegram tests passed. Next: progress transport.
- 2026-08-01: R5 completed by merging visible/silent progress adapters behind an optional progress target; 144 Telegram tests passed. Next: R6.
- 2026-08-01: R6 removed the unread application `agent_flows` registry while retaining table cleanup for rollback; core/Web/Telegram tests passed and only cleanup SQL remains. Next: R7.
- 2026-08-01: R7 checkpoint 1 made `SessionSummary` the sole session response DTO and deleted duplicate backend/UI conversions; contracts, Web transport, and wasm checks passed. Next: session-ID derivation.
- 2026-08-01: R7 checkpoint 2 made `WebSessionManager::resolve_session_id` the task executor's session-ID owner and removed duplicate hashing; all 168 Web tests passed. Next: checkpoint adapter.
- 2026-08-01: R7 checkpoint 3 reused `StorageFlowCheckpoint` for Life execution and removed its duplicate adapter; all 168 Web tests passed. Next: attachment staging.
- 2026-08-01: R7 checkpoint 4 shared multipart attachment staging between Web sessions and Life while preserving distinct sandbox scopes; all 168 Web tests passed. Next: `SessionRegistry`.
- 2026-08-01: R7 checkpoint 5 replaced three session maps with one `SessionEntry` map, deleted unused `get_or_create`, and paired task execution handles; runtime, Telegram, and Web tests passed. Next: `ToolCatalog`.
- 2026-08-01: R7 completed by deleting `ToolRegistry`, duplicate executor registration/conversion/state, and sub-agent executor vectors; `ToolCatalog` now owns execution and specs, with focused runtime/runner/delegation/static tests passing. Next: R8.
- 2026-08-01: R8 checkpoint 1 moved Telegram context state and flow persistence to field-scoped operations with atomic flow initialization and transactional DM mirroring; real PostgreSQL plus full Telegram/Web library tests pass. Next: manager context catalog operations.
- 2026-08-01: R8 checkpoint 2 moved manager forum catalog writes/deletes and sandbox/catalog reads to row-scoped operations; 54 manager tests, Telegram cleanup integration, and five real PostgreSQL context tests pass. Next: remove aggregate config API.
- 2026-08-01: R8 completed by deleting `UserConfig`, aggregate get/update storage APIs, and the unused standalone global-state writer (430 net LOC); concurrent PostgreSQL and full affected transport tests pass. Next: R9.
- 2026-08-01: R9 replaced the `Start` modality fan-out and handlers cycle with one default Agent Mode ingress, one topic-route resolution, and private-chat-only dialogue confirmations (749 net LOC); full Telegram package tests and bot composition check pass. Next: R10.
- 2026-08-01: R10 checkpoint 1 removed both slim Cargo profiles, per-module profile membership, generated profile TOMLs, obsolete snapshots, and profile-specific CI/test branches; registry check, full workspace check, registry snapshot, and 168 Web tests pass. Next: root Compose contraction.
- 2026-08-01: R10 completed by making root Compose the sole current deployment entrypoint and deleting four split/overlay files plus orphaned SearXNG config (616 net LOC); root Compose validation and current-reference inspection pass. Next: R11.
- 2026-08-01: R11 completed after two stale test-only assertions were corrected: full workspace tests, Clippy, fmt, module-registry, wasm, and Compose checks passed; commits were pushed; release images built and root Compose redeployed with migration 14 current and healthy services.

## Completion

- Resolved outcomes: R1-R11 verified.
- Commands and artifacts: Full-profile workspace tests; all-target/all-feature Clippy; fmt; module-registry check; Web UI wasm check; root Compose validation/build/rollout; migration, health, container, image, and log checks.
- Constraint and diff-scope check: No `.env*`, schema, applied migration, new dependency/service, queue, cache, or compatibility path was added; existing database data and historical contracts remain.
- Final status: Complete on the current host.
