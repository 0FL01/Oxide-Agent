# Lazy Tool Migration Plan

> Status: APPROVED — implementation pending.
> Owner: agent. No fallback, no backward-compat, no stubs, no deferred-loading-as-primary.

## 0. Objective

Migrate agent tool exposure from **eager full-list** to a **Lazy Tool protocol**:

- Runtime owns the full **executable tool catalog**.
- Model sees only a small **bootstrap surface** + a typed `retrieve_tools` control tool.
- Exact tool schemas reach the provider `tools[]` array **only after** the model activates a capability group via `retrieve_tools`.
- Per-iteration dynamic resolution: visible tools can grow within a run as the task evolves.

Goals: save input tokens (tool schemas), reduce tool-selection noise, keep prompt-cache stability for the stable prefix.

Non-goals: external MCP proxy, `defer_loading` as primary mechanism, stub descriptions, direct/hybrid modes, backward-compatible eager mode.

---

## Glossary

- **Tool Catalog** — the full set of allowed, executable tools for the session/run. Internal. Never fully serialized to the model.
- **Tool Catalog Entry** — `{ executor, spec (ToolDefinition), module_id, capability_group, visibility }`.
- **Tool Surface** — the live, ordered set of tool names currently visible to the model. Mutated only by `retrieve_tools` activation and history-presence rules.
- **Bootstrap Surface** — the initial visible surface at run start.
- **Capability Group** — a coarse, typed class (`files`, `shell`, `web`, `browser`, `memory`, `media`, `delegation`, `agents_md`, `manager`, `ssh`, `stack_logs`, `tts`, `ytdlp`, `reminders`, `compression`). Tools map 1→N groups.

---

## Mines (risks that must be closed by design, not patched)

| # | Mine | Why it happens | Closure |
|----|------|----------------|--------|
| M1 | Provider rejects a request because a tool referenced in **history** is no longer in `tools[]` | Tool was activated, called, then dropped from surface on a later turn | Surface is **monotonic within a run**: once activated, stays visible for the rest of the run (or until compaction rewrites history). Surface grows, never shrinks mid-run. |
| M2 | Tool-execution runtime cannot find a tool the model called | Visible surface and executable catalog drift | Catalog is the single source of truth for execution. Surface is a projection of catalog + activation. `ToolRegistry::execute_or_normalize` already handles unknown tools — keep catalog complete. |
| M3 | Prompt cache invalidated every turn because active-tools block changes | Active tools are placed in the cacheable prefix | Active-tools block lives in the **volatile suffix** (after date context), never in the stable prefix. Stable prefix contains only the lazy catalog **category list** (static for a session). |
| M4 | Hooks read `ctx.tools` (full) and misbehave when surface is smaller | `HookContext.available_tools` is `&[ToolDefinition]` borrowed for the whole run | Hooks receive **visible surface** (what the model can call this turn), not the catalog. `has_tool` semantics: "can the model call this now". |
| M5 | Compaction/token budget computed on full catalog → wrong headroom | `estimate_tool_schema_tokens` reads `ctx.tools` | Budget uses **visible surface** tokens. Compaction decision reflects what will actually be sent. |
| M6 | `current_tool_definitions()` is the public "specs right now" API and callers assume full set | Used by web UI, search probe, snapshots | Split into `current_tool_catalog()` (full, for admin/UI) and `current_visible_tool_surface()` (model-facing). Update every call site explicitly. |
| M7 | Structured-output validator rejects unknown tool names that were valid but not yet activated | `parse_structured_output` checks `tools.iter().any(name)` | Validator checks **visible surface**. If model calls a not-yet-activated tool name, that's a model error → structured-output failure path handles it. Do NOT auto-activate on parse. |
| M8 | Model calls `retrieve_tools` in a loop, re-activating the same groups | No idempotency / no terminal signal | `retrieve_tools` returns already-active groups as `already_active`; activation is idempotent. Loop detection covers repeated identical tool calls. |
| M9 | Sub-agent allowlist (`allowed_tools` whitelist) bypassed by lazy activation | Sub-agent constructs its own catalog from `allowed_tools` | Sub-agent catalog is **pre-filtered** to `allowed_tools` before surface is computed. `retrieve_tools` can only activate from the pre-filtered catalog. |
| M10 | Search Probe has its own executor with an explicit tool allowlist | `create_search_probe_executor(tool_allowlist)` | Search probe is a specialized agent with a tiny tool set. Probe catalog = allowlist. Bootstrap surface = allowlist (no lazy retrieval needed — probe already has ≤5 tools). Document as "lazy not applicable". |
| M11 | Manager control-plane dynamically enables/disables topic tools mid-session | `topic_agent_tools_enable/disable` mutates profile | Profile mutation rebuilds the catalog. Active surface is recomputed: previously-activated tools that are now blocked are **dropped from surface** only if not present in live history (M1). If in history, keep visible (monotonic-within-run unless profile explicitly forbids — then block at execution via hook, not at visibility). |
| M12 | Provider payload encoders assume `tools` is non-empty when tool calls exist | `build_tool_body` etc. | Encoders already handle empty tools. Verify: first-turn payload has bootstrap surface (non-empty). Never send a request with zero visible tools when the model might call tools. |
| M13 | Insta snapshots / static-guard tests assert exact full tool list | `modular_registry_snapshots`, `tool_runtime_static_guards` | Snapshots split: catalog snapshot (full, stable) + initial-surface snapshot (small, stable). Static guards rewritten to assert the catalog↔surface separation, not a single static list. |
| M14 | `retrieve_tools` leaks blocked/disabled tool names | Model could discover forbidden capabilities | `retrieve_tools` returns only **activatable** groups (catalog ∩ policy). Blocked tools' groups are omitted entirely, not "permission denied". |
| M15 | Tool name collisions across capability groups | Two groups contain the same tool name | Catalog is keyed by canonical tool name (existing `ToolRegistry` invariant). Group→tools is a projection; activation is set-union on names. No duplicates. |
| M16 | `retrieve_tools` itself must always be visible | If deferred, model can't bootstrap | `retrieve_tools` is in bootstrap surface, `visibility: AlwaysVisible`. Never deferred, never activatable by `retrieve_tools` (no self-reference). |

---

## Phases & Checkpoints

### Phase A — Domain split: Catalog vs Surface

**CP-A1: Introduce catalog/surface types**
- New module `crates/oxide-agent-core/src/agent/tool_runtime/surface.rs`.
- `ToolCatalogEntry { executor: Arc<dyn ToolExecutor>, spec: ToolDefinition, module_id: ModuleId, capability_group: CapabilityGroup, visibility: ToolVisibility }`.
- `ToolVisibility { AlwaysVisible, Deferred }`.
- `CapabilityGroup` enum (typed, exhaustive).
- `ToolCatalog { entries: BTreeMap<ToolName, ToolCatalogEntry> }` with `specs_for(names)`, `executor(name)`, `entries()`.
- `ToolSurface { active: BTreeSet<ToolName> }` with `visible_specs(&catalog)`, `activate(group, &catalog)`, `contains(name)`.
- **No behavior change yet** — types only, no wiring.
- Gate: `cargo check --workspace --no-default-features --features profile-embedded-opencode-local`.
- Acceptance: types compile, unit tests for `activate` idempotency, `visible_specs` ordering (deterministic by name), `specs_for` returns only active + always-visible.

**CP-A2: Map existing tools to capability groups**
- For each `ToolModule` impl, assign `capability_group` and `visibility` (bootstrap vs deferred).
- Bootstrap (AlwaysVisible): `retrieve_tools` (new), `write_todos`, `compress`.
- Deferred: everything else.
- Add a `capability_group()` + `visibility()` method to `ToolModule` trait (or a companion registry mapping module_id → group/visibility).
- Gate: `cargo check` + `module-registry check`.
- Acceptance: every compiled tool module declares a group; a unit test asserts no tool is unmapped.

### Phase B — `retrieve_tools` tool

**CP-B1: Implement `retrieve_tools` executor**
- New tool `retrieve_tools` with typed schema:
  ```json
  {
    "type": "object",
    "properties": {
      "capabilities": { "type": "array", "items": { "type": "string", "enum": [...] } },
      "reason": { "type": "string" }
    },
    "required": ["capabilities"]
  }
  ```
- Execution:
  - Resolve requested groups against catalog ∩ policy.
  - Activate matching tools in the shared `ToolSurface`.
  - Return compact JSON: `{ activated: [{name, group}], already_active: [{name, group}], unknown_groups: [...] }`.
  - Never execute downstream tools. Never reveal blocked tools.
- The surface is shared (Arc/RwLock) between the executor and the runner so activation persists across the run.
- Gate: unit test — activate `files` → surface contains `read_file`/`write_file`/`apply_file_edit`/`list_files`; activate twice → second returns `already_active`.
- Acceptance: idempotent, policy-filtered, no blocked-tool leakage.

### Phase C — Runner refactor: dynamic visible tools

**CP-C1: Replace `ctx.tools` with surface-backed snapshot**
- `AgentRunnerContext.tools` becomes a **per-iteration resolved** `Vec<ToolDefinition>` (owned, recomputed before each LLM call from `ctx.tool_surface.visible_specs(&ctx.tool_catalog)`).
- `AgentRunnerContext` gains `tool_catalog: Arc<ToolCatalog>` and `tool_surface: Arc<RwLock<ToolSurface>>`.
- `tool_runtime_registry` stays as the execution handle (backed by catalog).
- All `ctx.tools` readers now read the resolved visible snapshot.
- Gate: `cargo check` full workspace; `cargo test -p oxide-agent-core`.
- Acceptance: runner compiles; existing tests pass with a surface initialized to full catalog (temporary, until CP-D1).

**CP-C2: Recompute visible snapshot each iteration**
- Before `call_llm_with_tools`, resolve `visible = surface.visible_specs(&catalog)` and pass to LLM call, token snapshot, hooks, compaction budget.
- Gate: `cargo test` runner tools tests.
- Acceptance: visible tools list is consistent across snapshot, LLM payload, hooks, and structured-output validator within one iteration.

### Phase D — Prompt refactor

**CP-D1: Lazy prompt composition**
- `create_agent_system_prompt` / `create_sub_agent_system_prompt` accept a **prompt surface snapshot** (visible specs + catalog category list), not the full tool list.
- Stable prefix: fallback + instructions + workflow hints (driven by **catalog groups**, not individual deferred tools) + dynamic context.
- Volatile suffix (after date): **Active Tools** block = visible tool names only.
- Category list block: "You can retrieve tools for: files, shell, web, ... Call `retrieve_tools` to load them."
- Gate: prompt snapshot tests updated; cache-stability test asserts active-tools block is in volatile suffix.
- Acceptance: stable prefix unchanged when surface grows; only volatile suffix changes.

### Phase E — Provider path

**CP-E1: Pass visible snapshot to provider encoders**
- `ChatWithToolsRequest.tools` already `&[ToolDefinition]` — feed it the visible snapshot. No encoder changes needed.
- Verify all encoders handle the small bootstrap payload on turn 1.
- Gate: `cargo test` provider unit tests + `RUN_LLM_E2E_CHECKS=1` live probe (see Phase 0).
- Acceptance: turn-1 payload = bootstrap only; turn-2 payload = bootstrap + activated; provider accepts both.

### Phase F — Hooks, compaction, tokens

**CP-F1: Hooks see visible surface**
- `HookContext.available_tools` = visible snapshot.
- `has_tool` = "model can call this now".
- Gate: hook unit tests.
- Acceptance: `tool_access_policy` hook still blocks at execution (catalog-level); hooks that branch on tool presence use visible set.

**CP-F2: Compaction budget uses visible surface**
- `estimate_tool_schema_tokens`, `CompactionRequest.tools`, `AdmissionBudget.tool_schema_tokens` → visible snapshot.
- Gate: compaction budget tests updated.
- Acceptance: token snapshot `tool_schema_tokens` reflects visible, not catalog.

### Phase G — API split & call sites

**CP-G1: Split `current_tool_definitions`**
- `current_tool_catalog() -> Vec<ToolDefinition>` (full, for admin/UI/snapshots).
- `current_visible_tool_surface() -> Vec<ToolDefinition>` (model-facing, for probe/UI display of what the model sees).
- Update all call sites: web UI, search probe, snapshots.
- Gate: `cargo check` workspace + transport-web + web-ui wasm check.
- Acceptance: no caller of the old single API remains.

### Phase H — Sub-agents, search probe, manager

**CP-H1: Sub-agent catalog pre-filter**
- Sub-agent catalog = catalog ∩ `allowed_tools` whitelist.
- Surface initialized to bootstrap ∩ pre-filtered catalog.
- Gate: delegation + sub-agent tests.
- Acceptance: sub-agent cannot activate tools outside its allowlist.

**CP-H2: Search probe — document exception**
- Probe has ≤5 tools; catalog = allowlist; surface = full catalog (no lazy retrieval).
- Add a comment + test asserting probe surface == probe catalog.
- Gate: transport-web session tests.

**CP-H3: Manager dynamic tool enable/disable**
- Profile mutation → catalog rebuild → surface recompute (M11 rules).
- Gate: manager control-plane agent_controls tests.

### Phase I — Tests & snapshots

**CP-I1: New lazy-protocol tests**
- Hidden tools absent from turn-1 LLM payload (capture server).
- Hidden tool names/descriptions/schemas absent from prompt before `retrieve_tools`.
- `retrieve_tools` activates only allowed groups.
- Blocked tools not discoverable.
- Post-activation turn-2 payload contains activated schemas.
- Execution resolves via catalog, not surface.
- Token snapshot counts visible only.
- Sub-agent allowlist applied pre-lazy.

**CP-I2: Update snapshots**
- Split `modular_registry_snapshots` into catalog snapshot + initial-surface snapshot.
- Rewrite `tool_runtime_static_guards` to assert catalog↔surface separation.
- Update prompt snapshots (`snapshot_prompts.rs`, composer tests).
- Regenerate insta snapshots.

### Phase J — Verification gates

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --no-default-features --features profile-embedded-opencode-local
cargo check --workspace --no-default-features --features profile-web-embedded-opencode-local
cargo check -p oxide-agent-web-ui --target wasm32-unknown-unknown
cargo test --workspace --no-default-features --features profile-full
cargo test -p oxide-agent-transport-web --no-default-features --features profile-web-embedded-opencode-local
cargo run -p xtask -- module-registry check
```

### Phase 0 — Live-contract probes (MUST run before Phase C/E)

Before touching runner/provider paths, verify providers accept dynamic tool-list growth across turns:

1. Request 1: `tools = [retrieve_tools_only]` → model calls `retrieve_tools`.
2. Runtime activates a real tool.
3. Request 2: `tools = [retrieve_tools, activated_tool]` + history with tool_call/tool_result for `retrieve_tools`.
4. Confirm: provider accepts the grown `tools[]`; tool history validates; no 400.

Run via `RUN_LLM_E2E_CHECKS=1` for `opencode-go` and `openai-base:*` routes. If a provider rejects → stop, redesign surface/history contract (do not fall back to eager).

---

## Open questions (resolve during Phase A)

1. Exact `CapabilityGroup` enum values — derive from existing tool module IDs / `provides` capabilities in `module_registry.toml`.
2. Whether `compress` should be AlwaysVisible or in a `context-management` group (lean: AlwaysVisible, it's a control tool).
3. Whether `write_todos` stays AlwaysVisible or becomes `task-tracking` group (lean: AlwaysVisible — needed before any work).
4. Whether `retrieve_tools` should also accept a free-text `query` for fuzzy matching (lean: **no** — typed groups only, avoids re-introducing exact-name discovery).
