# Goal: Life Solo Bridge Chat

Date started: 2026-06-28
Status: active
Codex goal: Implement `docs/goals/2026-06-28-life-solo-bridge.md` until every Completion Audit item is verified by its required evidence, while preserving listed constraints and non-goals from `docs/prd/PRD-perm.md`. Work checkpoint by checkpoint (B1-B8), update the goal doc after each meaningful verification, commit after each completed checkpoint, compress before starting the next checkpoint or when context is high/critical, and stop only on verified completion or an exact blocker with required evidence and the smallest external action needed.
Source spec: `docs/prd/PRD-perm.md` — Permanent Life Mode: Solo Bridge Chat
Goal doc owner: Codex
Last updated: 2026-06-28 B7 delivery outbox

## Objective

Implement the bridge-first Life Mode redesign from `docs/prd/PRD-perm.md`: one ordinary chat agent synchronized across Web `/life` and a dedicated Telegram Life bot, with open transport bindings, durable queue correctness, crash recovery, and delivery outbox.

Done when every required Completion Audit item below is verified by its listed evidence, each completed phase is committed separately, and the bridge remains scoped to solo-owner configuration rather than multi-user linking or memory UX.

## Scope

In scope:
- `docs/prd/PRD-perm.md` as the source contract.
- `migrations/0010_life_mode.sql` and life SQLx storage/repository code.
- `crates/oxide-agent-life/`: domain types, gateway submit contract, runtime wake/claim, worker queue semantics, run leases/reaper, delivery outbox repository.
- `crates/oxide-agent-transport-web/src/server/`: Life routes, runtime bootstrap, env/config binding bootstrap, outbox delivery orchestration.
- `crates/oxide-agent-transport-telegram/`: dedicated Life bot input behavior, owner chat filtering, `/start`/`/help`/`/status`, plain-text delivery constraints if implemented there.
- `crates/oxide-agent-web-contracts/` and `crates/oxide-agent-web-ui/` only where contract/UI labels need open transport ids or bridge status.
- Tests, docs, and validation evidence proving audit items.

Out of scope:
- Engram, memory curator, memory generations, memory inspector/editor, recall UX.
- User-facing token linking flow, multi-user SaaS linking, account/device discovery.
- Telegram group/multi-chat routing.
- Rich Telegram MarkdownV2 rendering in the first bridge milestone.
- Cross-transport reminders.
- Replacing ordinary web sessions or existing non-Life Telegram flows beyond the dedicated Life bot path.

## Missing Inputs

None at goal creation. Secrets are intentionally not documented here. Runtime validation that requires a real Telegram token/chat may be recorded as manual evidence with redacted identifiers.

## Repository Context

- Source PRD highlights:
  - PRD §1: first focus is ordinary chat-agent bridge, not memory UX.
  - PRD §4: solo owner via env/config bindings (`LIFE_OWNER_WEB_LOGIN`, `LIFE_TELEGRAM_BOT_TOKEN`, `LIFE_TELEGRAM_CHAT_ID`).
  - PRD §5: verified mines are closed transport enums/checks, accidental principal allocation, follow-up input consumption, missing run crash recovery, delivery in executor, Telegram MarkdownV2 risk.
  - PRD §6: target architecture uses `life_transport_bindings` and `life_delivery_outbox`.
  - PRD §8: implementation phases B1-B8.
- Existing entry points:
  - `crates/oxide-agent-life/src/domain/` — life ids/enums/status domain.
  - `crates/oxide-agent-life/src/gateway/` — current submit path.
  - `crates/oxide-agent-life/src/storage/` — repository trait and SQLx backend.
  - `crates/oxide-agent-life/src/runtime/` and `worker/` — wake, run lifecycle, input execution.
  - `crates/oxide-agent-transport-web/src/server/life_executor.rs` — ordinary `AgentExecutor` adapter; must not own external delivery.
  - `crates/oxide-agent-transport-web/src/server/life_routes.rs` — Web submit/read/SSE.
  - `crates/oxide-agent-transport-telegram/src/runner.rs` — current Telegram life command path.
- Existing validation commands from repo guidance:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`
  - `cargo test --workspace --no-default-features --features profile-embedded-opencode-local --no-run`
  - `cargo test -p oxide-agent-life` for life storage/runtime tests.
  - `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local` for web routes where profile-specific.
  - If touching web UI: `cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown` and `trunk build --release` from `crates/oxide-agent-web-ui`.
  - If touching module/profile wiring: `cargo run -p xtask -- module-registry check`.

## Completion Audit

### Functional requirements (G*)

- G1: Bridge terminology/config is implemented and memory-first scope is not reintroduced.
  - Source: PRD §1, §8 B1, §10 out of scope.
  - Acceptance: config/docs/code use bridge/chat ownership wording; `LIFE_OWNER_WEB_LOGIN`, `LIFE_TELEGRAM_BOT_TOKEN`, and `LIFE_TELEGRAM_CHAT_ID` are the configured solo bridge inputs; no new Engram/curator/memory-generation code is introduced.
  - Evidence required: diff/code review; grep for removed memory-first bridge paths; relevant config docs/tests.
  - Status: verified
  - Evidence collected: B1 documented the solo bridge env contract in `.env.example`, `README.md`, and `docs/deploy.md`: `LIFE_OWNER_WEB_LOGIN`, `LIFE_TELEGRAM_BOT_TOKEN`, and `LIFE_TELEGRAM_CHAT_ID`. `README.md` explicitly distinguishes the dedicated Life Telegram bot from ordinary Agent Mode `TELEGRAM_TOKEN` and states the milestone is chat synchronization, not memory/curator UX. `git diff --check`, targeted env grep, and `cargo fmt --all -- --check` passed. `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed with pre-existing warnings only; B1 changed no Rust code. Grep for stale `LIFE_WEB_USER_LOGIN` returned no matches.

- G2: Life core uses an open transport id contract.
  - Source: PRD §5.1, §8 B2.
  - Acceptance: life input/source/binding code supports arbitrary transport ids like `linux`/`android` without Rust enum edits; `internal` remains reserved for assistant/system source where needed; SQL no longer enumerates concrete transports in CHECK constraints.
  - Evidence required: migration/schema diff; grep proving closed `LifeIdentityProvider::{Web, Telegram}` / transport CHECKs are gone or no longer authoritative; tests with a non-web/non-telegram transport id.
  - Status: verified
  - Evidence collected: B2 replaced closed `LifeIdentityProvider` and `LifeSourceTransport` enums with open validated `LifeTransportId`; `grep` found no remaining `LifeIdentityProvider|LifeSourceTransport|source_transport_as_str|source_transport_from_str` Rust references. `migrations/0010_life_mode.sql` now stores `life_identity_links.transport_id TEXT NOT NULL CHECK (btrim(transport_id) <> '')` and `life_turns.source_transport TEXT NOT NULL CHECK (btrim(source_transport) <> '')`, so SQL no longer enumerates `web`/`telegram`/`internal`. `gateway::tests::submit_accepts_future_transport_without_enum_or_schema_change` verifies a `linux` transport id resolves and persists as source transport without enum/schema edits. `tests::transport_ids_are_open_but_non_empty` verifies `web`, `telegram`, and `linux` are accepted and blank ids are rejected. `cargo test -p oxide-agent-life --lib` passed (19 tests). `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` and `cargo test --workspace --no-default-features --features profile-embedded-opencode-local --no-run` passed with pre-existing core/web warnings only.

- G3: Solo transport bindings are durable and env/bootstrap driven.
  - Source: PRD §4.4, §8 B3.
  - Acceptance: `life_transport_bindings` exists with principal, transport id, inbound address, delivery address, enabled, timestamps; web startup can bind owner web login and Telegram chat id to the same principal; unknown Telegram chat id cannot create a hidden principal.
  - Evidence required: SQLx storage tests for binding insert/update/resolve; startup/bootstrap test or code review; negative test for unknown inbound address.
  - Status: verified
  - Evidence collected: B3 added `life_transport_bindings` to `migrations/0010_life_mode.sql` with `binding_id`, `principal_user_id`, open `transport_id`, JSONB `inbound_address`, JSONB `delivery_address`, `enabled`, and timestamps. `LifeTransportBinding` plus `upsert_transport_binding` / `resolve_transport_binding` are implemented in `oxide-agent-life` SQLx storage. `sqlx_transport_bindings_resolve_open_enabled_addresses` verifies enabled Telegram binding resolution, disabled binding denial, and a future `linux` binding without enum/schema edits. Web SQLx startup now runs `bootstrap_life_solo_bridge_from_env`: it resolves `LIFE_OWNER_WEB_LOGIN` through the existing Web login index, upserts the Life principal, links Web `user_id` and Telegram `chat_id` identities to the same principal, and stores Web/Telegram bindings. Bootstrap config tests verify unconfigured no-op, Telegram env requiring an owner login, and no bot token value exposure. B4 removed gateway principal auto-allocation from submit and added `gateway::tests::submit_rejects_unknown_binding_before_persistence`, proving an unknown Telegram-like inbound address returns `UnboundTransport` before any turn/input is persisted.

- G4: Submit path is narrowed to known binding resolution.
  - Source: PRD §5.2, §6.2, §8 B4.
  - Acceptance: transport adapters submit `transport_id + inbound_address + source_ref + content + attachments + metadata`; core resolves an existing binding/principal; no random transport input auto-allocates a new principal. Authenticated Web `/life` remains mapped to the owner principal/binding model.
  - Evidence required: gateway API diff; tests for accepted configured binding and denied unknown binding; grep/code review proving accidental principal allocation is not used for bridge submit.
  - Status: verified
  - Evidence collected: B4 changed `LifeInputSubmission` to the binding contract: `transport_id`, `inbound_address`, `source_ref`, `content`, `attachments`, `metadata`, and sensitivity. `LifeGateway` now depends only on `resolve_transport_binding` plus append/enqueue, and no longer owns `LifePrincipalAllocator`, principal creation, or `link_identity` in the submit path. Web `/life` submits inbound `{ "user_id": web_user_id }`; Telegram submits inbound `{ "chat_id": msg.chat.id.0 }` and stores `message_id` as `source_ref`. `gateway::tests::submit_uses_configured_binding_for_turn_and_input` verifies configured binding submit; `submit_rejects_unknown_binding_before_persistence` verifies no hidden principal/turn/input for unknown inbound; `submit_accepts_future_transport_without_enum_or_schema_change` keeps the future `linux` binding case. Grep after B4 found no `LifePrincipalAllocator`, `TelegramLifePrincipalAllocator`, `FixedPrincipalAllocator`, or gateway submit `provider_subject` call-sites outside the remaining identity-link bootstrap/storage API. `cargo test -p oxide-agent-life --lib`, `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --lib --no-run`, and `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` passed with pre-existing core/web warnings only.

- G5: Queue semantics are one-input-one-run or otherwise execute every consumed input exactly once in order.
  - Source: PRD §5.3, §8 B5, §9 validation.
  - Acceptance: later inputs are not marked consumed merely because a run is active; two fast Telegram messages are both executed exactly once and in order, or explicitly dead-lettered with evidence.
  - Evidence required: unit/SQLx worker tests for two queued inputs; code review of worker drain/claim semantics.
  - Status: verified
  - Evidence collected: B5 removed the old worker/storage drain contract that marked queued follow-up inputs consumed without executing their content. `LifeWorker::execute_claimed_run` now executes exactly one claimed input, marks only that claimed input consumed after executor success, completes that run, then claims the oldest remaining queued input as a separate run. `worker::tests::execute_claimed_run_executes_follow_up_inputs_as_separate_runs` verifies two queued inputs are passed to the executor in order and both input ids are consumed exactly once. SQLx storage now exposes `claim_next_queued_input_and_start_run` instead of `drain_queued_inputs_for_run`; `sqlx_life_worker_claim_start_complete_and_claim_next_are_db_backed` verifies a follow-up remains queued while a run is active, cannot be consumed by `mark_input_consumed` while queued, and is claimed with its own run/content after the first run completes. Full-repo grep found no `drain_queued_inputs_for_run` or queued-drain wording left in Rust.

- G6: Running runs have leases and expired runs are reaped.
  - Source: PRD §5.4, §8 B6, §9 validation.
  - Acceptance: running `life_runs` have lease owner/expiry/heartbeat; expired running runs are marked interrupted/failed and do not block later queued inputs for the solo principal.
  - Evidence required: migration/storage/runtime tests for lease acquisition, heartbeat, expiry/reap, and subsequent input claim.
  - Status: verified
  - Evidence collected: B6 added `lease_owner`, `lease_expires_at`, and `last_heartbeat_at` to `life_runs`, plus a running-row CHECK requiring lease fields and `life_runs_running_lease_idx`. SQLx claim paths now reap expired or legacy-null-lease running rows under the principal advisory transaction lock before checking for an active run or inserting a new run. New claims set lease owner/expiry/heartbeat, and `heartbeat_run_lease` extends the lease only for the owning worker while it is still unexpired. `LifeWorker` refreshes the lease during executor execution via a heartbeat select loop backed by an explicit `tokio` runtime dependency and returns `LostLease` if ownership is gone. `sqlx_life_run_lease_heartbeat_and_expiry_unblock_claims` verifies lease fields on claim, successful owner heartbeat, failed wrong-owner heartbeat, active non-expired run blocking follow-up claim, expired run reaped to `failed` with `run lease expired`, and subsequent queued input claimed as a new run. `cargo test -p oxide-agent-life --lib` passed (21 tests); workspace `cargo check` and `cargo test --workspace ... --no-run` passed with pre-existing core/web warnings only.

- G7: Assistant delivery uses durable outbox, not executor-owned external API calls.
  - Source: PRD §5.5, §6.3, §8 B7.
  - Acceptance: assistant turn persistence enqueues delivery rows for enabled bindings; workers claim/send/retry/dead-letter rows; `LifeAgentExecutor` only persists canonical assistant output and does not call Telegram/Linux/Android APIs.
  - Evidence required: schema/storage tests for outbox enqueue/claim/status transitions; grep proving executor has no Telegram/API delivery calls; delivery worker tests with mocked sender.
  - Status: verified
  - Evidence collected: B7 added `life_delivery_outbox` with `delivery_id`, `turn_id`, `binding_id`, `principal_user_id`, open `transport_id`, non-secret `delivery_address`, status `queued|claimed|delivered|failed|dead`, attempt count, claim owner/timestamps/expiry, retry time, last error, and timestamps. `LifeAgentExecutor::persist_assistant_turn` now calls storage method `append_assistant_turn_and_enqueue_deliveries`, which atomically inserts the assistant `life_turns` row and delivery rows for enabled bindings; it does not call Telegram/Linux/Android APIs. SQLx test `sqlx_delivery_outbox_enqueue_claim_retry_and_deliver_are_db_backed` verifies assistant-only enqueue, rows for Telegram plus future `linux`, claim content loading, no double-claim before claim expiry, expired claim reclaim, retry scheduling by `next_attempt_at`, delivered status, and dead-letter status. `delivery::tests::delivery_worker_marks_success_delivered` and `delivery_worker_retries_then_dead_letters` verify the transport-neutral worker boundary with mocked senders. Grep for `sendMessage|LIFE_TELEGRAM|TelegramLife|delivery` shows no executor-owned external Telegram/API delivery calls; delivery code in `oxide-agent-life` is transport-neutral.

- G8: Dedicated Telegram adapter does not require `/life`.
  - Source: PRD §3.2, §7.1, §8 B8.
  - Acceptance: every non-command private DM from configured `LIFE_TELEGRAM_CHAT_ID` is submitted as Life input; other chats are ignored/denied; `/start`, `/help`, `/status` exist; ack is `💭 Обрабатываю...`.
  - Evidence required: Telegram handler tests or focused route/handler tests; code review of command matching; manual/runtime verification if practical.
  - Status: pending
  - Evidence collected:

- G9: Telegram delivery is plain text chunked to Bot API limits for the first milestone.
  - Source: PRD §5.6, §8 B8.
  - Acceptance: delivery does not use raw MarkdownV2; outbound text is split into valid chunks no larger than Telegram `sendMessage` text limit with deterministic ordering.
  - Evidence required: formatter/chunker tests around 4096-char boundary and markdown-looking text; verification skeleton/raw Telegram API facts recorded before implementation if live API behavior is touched.
  - Status: pending
  - Evidence collected:

- G10: Cross-interface synchronization works through Postgres source of truth.
  - Source: PRD §7, §9 validation.
  - Acceptance: Telegram write appears in Web transcript; Web write enqueues delivery to Telegram; future transport can be represented by binding + adapter without executor semantic changes.
  - Evidence required: integration/unit tests covering Telegram-like binding to Web transcript and Web assistant turn to outbox; code review for future `linux`/`android` transport id test case.
  - Status: in_progress
  - Evidence collected: B7 proves the Web/agent side of outbound sync: assistant turn persistence writes canonical `life_turns` for Web SSE/Postgres transcript and atomically creates durable `life_delivery_outbox` rows for enabled bindings including `telegram` and future `linux`. Full Telegram no-`/life` inbound adapter and real Telegram sender/chunking remain for B8.

### Quality/compatibility constraints (Q*)

- Q1: Core/runtime architecture boundaries are preserved.
  - Source: `AGENTS.md` architectural invariants; PRD §6.
  - Acceptance: `oxide-agent-core` and `oxide-agent-runtime` do not depend on transport crates; `oxide-agent-life` remains transport-agnostic; Telegram SDK stays out of web/core/life.
  - Evidence required: `cargo tree`/grep or code review plus workspace check.
  - Status: in_progress
  - Evidence collected: B2 kept the open transport id in `oxide-agent-life` domain/storage/gateway and updated web/telegram call-sites without adding transport crate dependencies to core/runtime/life. Telegram SDK remains confined to `oxide-agent-transport-telegram`; web executor stores `source_transport="internal"` via `LifeTransportId`. B7 added transport-neutral `oxide-agent-life::delivery` traits/worker and SQLx outbox methods; no Telegram SDK or external API client was added to life/core/runtime/web executor. Full Q1 remains open until B8 wires the concrete Telegram adapter and final boundary audit passes.

- Q2: No hidden secret leakage.
  - Source: `AGENTS.md` secret refs/instructions.
  - Acceptance: bot tokens/env values are read from config/env, never written to prompts, memory, logs, docs, tests, or goal evidence.
  - Evidence required: code review/grep for token logging or persistence; tests use fake/redacted values.
  - Status: in_progress
  - Evidence collected: B1 documents only placeholder/redacted values (`YOUR_DEDICATED_LIFE_BOT_TOKEN`, numeric example chat ids) and does not add token logging or persistence. B3 reads only whether `LIFE_TELEGRAM_BOT_TOKEN` is configured; it never stores the token in `life_transport_bindings`, identity links, logs, or goal evidence. `bootstrap_config_never_exposes_bot_token_value` verifies startup error text does not include a fake token secret. B7 outbox stores only non-secret `delivery_address` snapshots and assistant turn references/content loaded from Postgres; it does not read, persist, or log bot tokens. Full Q2 remains open for B8 concrete Telegram token usage.

- Q3: Existing Web `/life` behavior remains functional.
  - Source: PRD §2, §3.1.
  - Acceptance: transcript, composer, attachments, activity, paging, SSE continue to work after bridge changes.
  - Evidence required: relevant web/life tests and checks; UI wasm/trunk checks if UI touched.
  - Status: in_progress
  - Evidence collected: B2 preserved the Web `/life` REST response contract because `ApiLifeTurnResponse.source_transport` was already a `String`; `life_routes.rs` now forwards `turn.source_transport.as_str()` directly. B4 preserves the existing authenticated Web route shape and runtime wake path while changing submit authorization to the B3 bootstrap binding `{ "user_id": web_user_id }`; unconfigured users now receive a clear 403 instead of hidden state allocation. B5 preserves the Web route wake contract: fast follow-up submits that receive `WakeOutcome::AttachedToActive` remain queued, and the active worker claims them as separate runs after completion. B6 preserves the same wake/worker route boundary while adding storage-owned lease reaping to claim paths, so an expired crashed run unblocks later Web/Telegram queued input instead of changing the HTTP/SSE contract. B7 preserves Web transcript/SSE by keeping `life_turns` as canonical source and adding outbox rows atomically after assistant turn persistence; no Web UI/contracts were changed. `cargo test -p oxide-agent-life --lib` passed (24 tests), and workspace `cargo check` plus `cargo test --workspace ... --no-run` passed with pre-existing core/web warnings only. Full Q3 remains open for B8 behavior changes and final route/runtime/UI checks.

- Q4: Validation breadth is monorepo-wide before completion.
  - Source: repo instructions P0.6; PRD §9 code gates.
  - Acceptance: final pass runs broad checks or classifies failures with evidence.
  - Evidence required: command outputs/summaries for fmt, clippy, cargo check, tests; failure classification by revert/import evidence if any fail.
  - Status: pending
  - Evidence collected:

### Non-goals (N*)

- N1: Do not reintroduce Engram/curator/memory-generation bridge work.
  - Source: PRD §1, §10.
  - Must preserve: bridge milestone remains ordinary chat synchronization.
  - Evidence required: diff/grep review.
  - Status: verified
  - Evidence collected: B1 changed only `.env.example`, `README.md`, `docs/deploy.md`, and this goal doc. Targeted code grep found only the existing Wiki memory curator prompt in `oxide-agent-core`, outside Life bridge and untouched; no Life Engram/curator/memory-generation code was added.

- N2: Do not implement multi-user linking/token exchange.
  - Source: PRD §4.1, §10.
  - Must preserve: solo owner env/config binding model only.
  - Evidence required: diff/code review.
  - Status: verified
  - Evidence collected: B1 documents explicit solo-owner env/config bindings only. No token-linking flow, account discovery, multi-user link tables, or runtime code paths were added.

- N3: Do not require `/life` in the dedicated Telegram bot.
  - Source: PRD §3.2, user correction.
  - Must preserve: ordinary private DM text is Life input.
  - Evidence required: handler tests/code review.
  - Status: pending
  - Evidence collected:

- N4: Do not add rich Telegram MarkdownV2 rendering in this milestone.
  - Source: PRD §5.6, §10.
  - Must preserve: plain text delivery with safe chunking only.
  - Evidence required: delivery tests/code review.
  - Status: pending
  - Evidence collected:

## Implementation Plan

1. B1 — Bridge terminology/config contract
   - Audit IDs: G1, Q2, N1, N2.
   - Expected changes: config/docs/constants for `LIFE_OWNER_WEB_LOGIN`, `LIFE_TELEGRAM_BOT_TOKEN`, `LIFE_TELEGRAM_CHAT_ID`; remove command-prefixed dedicated-bot assumptions where isolated.
   - Validation: targeted grep/tests; `cargo fmt --all -- --check`; relevant `cargo check`.
   - Exit condition: bridge config contract is present and documented without memory-first scope.

2. B2 — Open transport id
   - Audit IDs: G2, Q1, Q3.
   - Expected changes: open `TransportId`/source model; migration removes concrete transport CHECKs.
   - Validation: life domain/storage tests including `linux` or `android` id; cargo fmt/check.
   - Exit condition: adding a new transport id requires binding/adapter, not enum/schema edits.

3. B3 — Solo transport bindings
   - Audit IDs: G3, Q2, N2.
   - Expected changes: `life_transport_bindings` schema/repository/bootstrap from owner web login and Telegram chat id.
   - Validation: SQLx tests for upsert/resolve/deny unknown; bootstrap tests where practical.
   - Exit condition: configured Web and Telegram identities resolve to same principal.

4. B4 — Narrow submit path
   - Audit IDs: G4, G10, Q3.
   - Expected changes: submit API moves from provider_subject principal allocation to binding resolution; Web submit remains authenticated and owner-scoped.
   - Validation: gateway tests for known/unknown binding; route tests for Web submit.
   - Exit condition: unknown transport input cannot create a hidden transcript.

5. B5 — Queue correctness
   - Audit IDs: G5, Q3.
   - Expected changes: worker/runtime no longer consume follow-up inputs unless executed; one-input-one-run preferred.
   - Validation: tests for two fast inputs executed exactly once and in order.
   - Exit condition: no queued input is silently drained without execution/dead-letter.

6. B6 — Run lease/reaper
   - Audit IDs: G6, Q3.
   - Expected changes: lease fields, heartbeat, reaper on startup/poller path.
   - Validation: storage/runtime tests for expired run unblocking later input.
   - Exit condition: crashed active run cannot block the solo principal forever.

7. B7 — Delivery outbox
   - Audit IDs: G7, G10, Q1, Q2.
   - Expected changes: outbox schema/repository/enqueue on assistant turn; worker claim/retry/dead-letter with transport sender boundary.
   - Validation: outbox storage tests; mocked sender tests; grep proving executor does not call external APIs.
   - Exit condition: assistant output is durable delivery work for enabled bindings.

8. B8 — Dedicated Telegram adapter
   - Audit IDs: G8, G9, G10, N3, N4.
   - Expected changes: dedicated bot non-command DM submit, owner chat filter, `/start`/`/help`/`/status`, plain chunked delivery.
   - Validation: Telegram handler/chunker tests; runtime/manual check if token/chat available.
   - Exit condition: Telegram and Web are synchronized without `/life` and without MarkdownV2 rendering.

9. Final audit
   - Audit IDs: all.
   - Expected changes: goal doc evidence complete; no code changes unless audit finds gaps.
   - Validation: broad repo gates from Validation Contract.
   - Exit condition: every audit item verified or exact blocker recorded.

## Validation Contract

- Per phase: run the smallest meaningful targeted tests/checks before committing the phase.
- Before final completion: run broad available gates: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`, and a workspace/profile test gate or no-run compilation gate appropriate to feature constraints.
- If touching Web UI: also run wasm check and `trunk build --release`.
- If touching external Telegram API behavior: first record a verification skeleton and raw factual evidence for request/response/limits, then design/code against those facts.
- Done when: all G/Q/N items are verified by current repo evidence, all phase commits exist, and no unclassified validation failure remains.

## Decisions

- 2026-06-28: Goal source is `docs/prd/PRD-perm.md` after commit `ce2d4492`; older memory-first PRD content is not authoritative for this bridge milestone.
- 2026-06-28: Use one durable goal with atomic checkpoints B1-B8 because the PRD changes a cross-cutting bridge contract; each checkpoint must be separately validated and committed.
- 2026-06-28: Delivery belongs behind a durable outbox boundary, not in `LifeAgentExecutor`, so adding Linux/Android later does not require executor changes.
- 2026-06-28: `LifeTransportId` is an open non-empty string newtype, not an enum. Concrete ids such as `web`, `telegram`, future `linux`/`android`, and internal source id `internal` are values at the transport/binding layer, so adding a transport does not require a Rust enum or SQL CHECK edit.
- 2026-06-28: `life_transport_bindings` stores only observable routing addresses (`inbound_address`, `delivery_address`) and never transport credentials. `LIFE_TELEGRAM_BOT_TOKEN` remains an adapter/runtime secret for B7/B8 delivery, not durable Life state.
- 2026-06-28: Life submit is receiver-resolved by enabled `life_transport_bindings`; transport adapters submit only observed inbound address plus source reference. Principal allocation is outside the submit contract, so an unknown Telegram chat/Linux instance/Android device cannot create hidden Life state.
- 2026-06-28: Life queue progression is one input per run. A worker may claim the next queued input only after the current run is completed, and `mark_input_consumed` is narrowed to claimed rows only, so queued follow-ups cannot be silently consumed without their content crossing the executor boundary.
- 2026-06-28: Life run liveness is a storage-owned lease invariant, not a trust assumption about prior workers. Claim transactions reap expired running rows under the principal advisory lock before active-run checks; workers heartbeat their own running lease during executor execution.
- 2026-06-28: Assistant outbound sync is a storage-owned outbox invariant. `LifeAgentExecutor` atomically persists the assistant turn plus outbox rows but never owns transport API calls; concrete adapters implement the `LifeDeliverySender` boundary and outbox claims have expiry so delivery-worker crashes are recoverable.

## Progress Log

- 2026-06-28 initial contract
  - Changed: created this goal document from `docs/prd/PRD-perm.md`.
  - Evidence: PRD read and mapped to G1-G10, Q1-Q4, N1-N4, checkpoints B1-B8.
  - Commands: `git status --short` before writing was clean; `git diff --check` passed.
  - Commit: `docs(life): add solo bridge goal contract`.
  - Audit IDs updated: none verified yet.
  - Next: create in-session goal pointing to this document; start B1 verification/design.

- 2026-06-28 B1 bridge config contract
  - Changed: added Permanent Life Mode solo bridge config contract to `.env.example`, `README.md`, and `docs/deploy.md`; kept ordinary `TELEGRAM_TOKEN` distinct from dedicated `LIFE_TELEGRAM_BOT_TOKEN`.
  - Evidence: targeted grep found canonical env names in config/docs and no stale `LIFE_WEB_USER_LOGIN`; targeted code grep found no new Life memory/Engram/curator code; diff review shows no Rust runtime changes.
  - Commands: `git diff --check`; `cargo fmt --all -- --check`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (passed with pre-existing core/web warnings only).
  - Audit IDs updated: G1 verified; N1 verified; N2 verified; Q2 in progress with B1 no-secret-doc evidence.
  - Next: commit B1; compress; start B2 open transport id.

- 2026-06-28 B2 open transport id
  - Changed: replaced closed life identity/source enums with `LifeTransportId`; changed `life_identity_links.provider` to open `transport_id`; removed concrete transport SQL CHECK values from `life_identity_links` and `life_turns`; updated Web and Telegram submit call-sites plus assistant turn source to use explicit transport id values.
  - Evidence: full-repo grep found no `LifeIdentityProvider`/`LifeSourceTransport` Rust references; migration grep shows only non-empty transport checks; Web API still exposes `source_transport` as string.
  - Commands: `git diff --check`; `cargo fmt --all -- --check`; `cargo test -p oxide-agent-life --lib` (19 passed); `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; `cargo test --workspace --no-default-features --features profile-embedded-opencode-local --no-run` (both passed with pre-existing core/web warnings only).
  - Audit IDs updated: G2 verified; Q1 in progress; Q3 in progress.
  - Next: commit B2; compress; start B3 solo transport bindings.

- 2026-06-28 B3 solo transport bindings
  - Changed: added durable `life_transport_bindings` schema; added `BindingId`/`LifeTransportBinding`; added SQLx upsert/resolve binding repository methods; added Web SQLx startup bootstrap from `LIFE_OWNER_WEB_LOGIN` and optional `LIFE_TELEGRAM_CHAT_ID`; kept `LIFE_TELEGRAM_BOT_TOKEN` out of durable state.
  - Evidence: `sqlx_transport_bindings_resolve_open_enabled_addresses` verifies Telegram binding resolution, disabled binding denial, and future `linux` transport binding. `life_bootstrap` tests verify no-op when unconfigured, owner-login requirement for Telegram binding env, and no bot token value leakage. Blast-radius grep found new binding/token symbols confined to life storage/domain, web bootstrap, docs, and tests.
  - Commands: `cargo fmt --all -- --check`; `cargo test -p oxide-agent-life --lib` (20 passed); `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local life_bootstrap --lib` (3 passed); `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --lib --no-run`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; `cargo test --workspace --no-default-features --features profile-embedded-opencode-local --no-run` (workspace gates passed with pre-existing core/web warnings only).
  - Audit IDs updated: G3 in progress; Q2 in progress; N2 remains verified.
  - Next: commit B3; compress; start B4 narrow submit path so unknown transport/chat cannot auto-create hidden principals.

- 2026-06-28 B4 narrowed submit path
  - Changed: replaced gateway submit `provider_subject`/auto-allocation with `transport_id + inbound_address + source_ref`; removed `LifePrincipalAllocator`; Web submits inbound `{user_id}` and Telegram submits inbound `{chat_id}`; unknown bindings return `UnboundTransport` before transcript/input persistence.
  - Evidence: gateway tests cover configured Web binding, denied unknown Telegram-like binding, future `linux` binding, empty/private-secret prechecks, and redacted transcript preservation. Grep confirms no gateway submit allocator/provider-subject call-sites remain; identity-link `ProviderSubject` remains only for B3 bootstrap/storage compatibility.
  - Commands: `cargo fmt --all`; `cargo test -p oxide-agent-life --lib` (20 passed); `cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local --lib --no-run`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local` (passed with pre-existing core/web warnings only).
  - Audit IDs updated: G3 verified; G4 verified; Q3 in progress.
  - Next: commit B4; compress; start B5 queue correctness.

- 2026-06-28 B5 queue correctness
  - Changed: replaced follow-up drain/consume semantics with `claim_next_queued_input_and_start_run`; `LifeWorker` now executes one claimed input per run, consumes only that claimed input after executor success, then claims queued follow-ups as separate runs. `mark_input_consumed` now only updates `claimed` rows, not `queued` rows.
  - Evidence: worker unit test verifies two fast inputs are executed in order as two runs and both consumed exactly once; SQLx storage test verifies follow-up cannot be claimed while a run is active, cannot be consumed while queued, and is claimed with its own content/run after completion. Blast-radius grep found no old drain API or queued-drain wording left in Rust.
  - Commands: `cargo fmt --all`; `cargo fmt --all -- --check`; `cargo test -p oxide-agent-life --lib` (20 passed); `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; `cargo test --workspace --no-default-features --features profile-embedded-opencode-local --no-run` (workspace gates passed with pre-existing core/web warnings only).
  - Audit IDs updated: G5 verified; Q3 in progress.
  - Next: commit B5; compress; start B6 run lease/reaper.

- 2026-06-28 B6 run lease/reaper
  - Changed: added lease owner/expiry/heartbeat fields and index to `life_runs`; claim paths now reap expired running rows before active-run checks; running claims set leases; `LifeWorker` heartbeats active runs during executor execution; storage exposes `heartbeat_run_lease`.
  - Evidence: SQLx test `sqlx_life_run_lease_heartbeat_and_expiry_unblock_claims` verifies lease acquisition, owner heartbeat, wrong-owner denial, non-expired active-run blocking, expired-run reaping to failed, and subsequent queued input claim. Blast-radius grep shows lease symbols confined to `oxide-agent-life` and migration.
  - Commands: `cargo fmt --all`; `cargo fmt --all -- --check`; `cargo test -p oxide-agent-life --lib` (21 passed); `git diff --check`; `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; `cargo test --workspace --no-default-features --features profile-embedded-opencode-local --no-run` (workspace gates passed with pre-existing core/web warnings only).
  - Audit IDs updated: G6 verified; Q3 in progress.
  - Next: run broader B6 validation, commit B6; compress; start B7 delivery outbox.

- 2026-06-28 B7 delivery outbox
  - Changed: added `DeliveryId`, `LifeDeliveryOutbox`, `ClaimedLifeDelivery`, and transport-neutral `LifeDeliveryWorker`/`LifeDeliverySender`; added durable `life_delivery_outbox` schema with claim expiry; added SQLx atomic assistant-turn+outbox enqueue and claim/deliver/fail/dead transitions; changed `LifeAgentExecutor` to persist assistant turns through the outbox-enqueue storage method.
  - Evidence: SQLx outbox test covers assistant-only enqueue, Telegram and future `linux` delivery rows, claim/reclaim/retry/deliver/dead-letter transitions, and assistant content loading from Postgres. Mocked delivery worker tests cover successful send, retry scheduling, and dead-letter on retry exhaustion. Grep shows no executor-owned `sendMessage`, Telegram notifier, or `LIFE_TELEGRAM` API path.
  - Commands: `cargo fmt --all -- --check`; `git diff --check`; `cargo test -p oxide-agent-life --lib` (24 passed); `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`; `cargo test --workspace --no-default-features --features profile-embedded-opencode-local --no-run` (workspace gates passed with pre-existing core/web warnings only).
  - Audit IDs updated: G7 verified; G10 in progress; Q1/Q2/Q3 in progress.
  - Next: commit B7; compress; start B8 dedicated Telegram adapter and plain chunked delivery.

## Risks and Blockers

- Telegram runtime verification may require a real bot token/chat id.
  - Impact: live end-to-end delivery evidence may be unavailable in CI/local checks.
  - Evidence: secrets are not stored in repo and must not be logged.
  - Mitigation: use deterministic handler/chunker/outbox tests; record manual evidence only with redacted ids if the user provides a live environment.
  - Audit IDs affected: G8, G9, G10.

## Final Verification

Filled only when complete.

- Completion Audit result:
- Commands run:
- Artifacts inspected:
- Remaining gaps:
- User-accepted exceptions:
- Final status:
