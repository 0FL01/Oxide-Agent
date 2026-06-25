# Audit Evidence: oxide-agent-core П0-fit

Date: 2026-06-21
Status: locked baseline
Source: 5-axis audit via 5 parallel `general` subagents, 285 `.rs` files inspected
Reference: `docs/goals/2026-06-21-core-fit-remediation.md` (goal doc)

This document is the evidence-of-record. The goal doc references it for full reasoning behind each audit finding. Condensed tables live in the goal doc; full verdicts, traces, and design assessments live here.

---

## A1 — Architectural invariants

### A1.1 No transport dependency leak — PASS

- `Cargo.toml:19-61` — no `teloxide`, no `oxide-agent-transport-telegram`, no `oxide-agent-transport-web`, no `leptos`. Only path dependency: `oxide-browser-contracts` (`:29`), explicitly allowed (shared REST contract, independent of transport internals).
- Grep `teloxide|oxide_agent_transport_telegram|oxide_agent_transport_web|leptos` across all `src/**/*.rs`: 0 hits.
- `transport-telegram` and `transport-web` in `Cargo.toml:172-173` are empty feature flags — composition markers consumed by transport crates, not leakage points.

### A1.2 Explicit mod.rs convention — PARTIAL (low)

3 directories use modern Rust 2018+ `foo.rs + foo/` style instead of `mod.rs`:

| Directory | Module file | Declaration |
|---|---|---|
| `src/agent/executor/` (6 non-mod .rs files) | `src/agent/executor.rs` | `pub mod executor;` at `src/agent/mod.rs:15` |
| `src/llm/providers/opencode_go/` (2 files) | `src/llm/providers/opencode_go.rs` | `pub mod opencode_go;` at `src/llm/providers/mod.rs:38` |
| `src/llm/providers/openrouter/` (2 files) | `src/llm/providers/openrouter.rs` | `pub mod openrouter;` at `src/llm/providers/mod.rs:41` |

Valid Rust, still explicit module roots. Stylistic deviation, not structural.

### A1.3 cfg-gating on oxide_module_<id> — VIOLATION (high)

- `build.rs:29-41,84-87` correctly emits `oxide_module_<id>` cfg aliases from `module_registry.toml`. Infrastructure works.
- Counts (attribute form, `src/**/*.rs`):
  - `#[cfg(oxide_module_*)]`: 107 (+1 compound)
  - `#[cfg(feature = "<module-feature>")]`: 490 simple + 13 compound = ~503
  - `#[cfg(feature = "profile-*")]` (allowed raw): 0 attribute gates (profile gating uses `cfg!()` macro: 11 instances in `capabilities/compiled.rs:272-285`)
- All ~503 raw feature gates are for module features, not profile features.
- Top violators by feature: `sandbox-backend-docker-direct` (94), `tool-webfetch-md` (44), `llm-opencode-go` (33), `sandbox-backend-sandboxd-client` (29), `integration-ssh-mcp` (22), `tool-crw` (19), `tool-stack-logs` (15), `llm-openrouter` (14).
- Representative: `src/agent/providers/delegation.rs:44-71` (10 raw gates in 30 lines), `src/agent/providers/manager_control_plane/mod.rs:203-256` (9 raw gates).
- Compliant contrast: `src/agent/executor/tests/registry.rs` uses 42 `oxide_module_*` gates correctly; `src/llm/providers/modules.rs:564-1001` uses ~17.
- Impact: `unexpected_cfgs` drift detection cannot fire when modules are renamed; domain-level intent lost.

### A1.4 thiserror for library errors — VIOLATION (medium)

- `anyhow` is a regular (non-dev) dependency: `Cargo.toml:33`.
- Counts: `thiserror::Error` / `#[derive(Error)]` in 19-20 files; `anyhow::Error` 39, `anyhow::Result` 34, `anyhow!` 272, `anyhow::anyhow` 44.
- `StorageError` (`src/storage/error.rs:5`) and `LlmError` (`src/llm/error.rs:5`) are correctly thiserror-based. Public trait APIs return typed errors.
- Non-test anyhow usage by directory: `src/agent` 56, `src/sandbox` 7, `src/utils.rs` 1, `src/llm` 1.
- Top files: `src/sandbox/broker.rs` (38 `anyhow!`), `src/sandbox/manager.rs` (22), `src/agent/providers/media_file.rs` (14), `src/agent/providers/ssh_mcp.rs` (13), `src/agent/providers/delegation.rs` (12).
- `src/utils.rs:13`: `use anyhow::Result;` — public utility functions return untyped errors.
- `src/llm/providers/chatgpt/auth.rs:3`: `use anyhow::{Context, Result, anyhow, bail};` — OAuth auth logic in lib code.
- `SandboxError` enum does not exist anywhere (grep confirmed). `SandboxBackend` trait (`src/sandbox/traits.rs:50`) returns `anyhow::Result`.
- Some public trait surfaces leak `anyhow::Error`: `ManagerTopicSandboxCleanup`/`ManagerTopicSandboxControl` traits (`manager_control_plane/mod.rs:378,385,395-408`) return `anyhow::Result`.

### A1.5 Context-scoped storage — PASS

`src/storage/provider.rs` — `StorageProvider` trait exposes three tiers:
- Legacy (single-context, required): `save_agent_memory` (`:29`), `load_agent_memory` (`:45`), `clear_agent_memory` (`:56`) — no `context_key`, required methods.
- Context-scoped: `save_agent_memory_for_context` (`:35`), `load_agent_memory_for_context` (`:47`), `clear_agent_memory_for_context` (`:58`) — default impls discard `context_key` and delegate to legacy (`:40-43,51-54,62-65`).
- Flow-scoped: `save_agent_memory_for_flow` (`:67`), `load_agent_memory_for_flow` (`:79`), `clear_agent_memory_for_flow` (`:90`) — default impls discard `flow_id` and delegate to context-scoped (`:74-77,85-88,96-99`).
- Flow record APIs: `get_agent_flow_record` (`:101`), `upsert_agent_flow_record` (`:108`) — both take `context_key` + `flow_id`.
- Legacy fallback explicitly marked: `let _ = context_key;` / `let _ = flow_id;`.

### A1.6 Provider trait boundaries — PARTIAL (low-medium)

- `LlmProvider` (`src/llm/provider.rs:6-95`): fully typed. `ChatWithToolsRequest<'a>` → `ChatResponse` (`:87-94`), `LlmError` typed error. No `Box<dyn Any>`. No stringly-typed payloads.
- `StorageProvider` (`src/storage/provider.rs:15-568`): typed records, typed options structs, `StorageError` typed error. No `Box<dyn Any>` (grep 0 hits).
- Smell: `check_connection(&self) -> Result<(), String>` (`provider.rs:214`) — only method returning `String` instead of `StorageError`. Implemented identically in `sqlx/mod.rs:877`.
- Smell: `ManagerTopicSandboxCleanup` (`mod.rs:372-379`) and `ManagerTopicSandboxControl` (`mod.rs:383-409`) return `anyhow::Result`.

### A1.7 No premature abstractions — PARTIAL (low-medium)

| Trait | Location | Prod impls in core | Verdict |
|---|---|---|---|
| `ManagerTopicLifecycle` | `manager_control_plane/mod.rs:333` | 0 (transport-injected DI seam) | Legitimate |
| `ManagerTopicSandboxCleanup` | `mod.rs:372` | 1: `SandboxAdminTopicSandboxCleanup` (private, `:411`) | Single-implementor |
| `ManagerTopicSandboxControl` | `mod.rs:383` | 1: `SandboxAdminTopicSandboxControl` (private, `:415`) | Single-implementor |
| `BrowserSidecar` | `browser_live/client.rs:115` | 1 prod + 1 test | Legitimate (test double) |
| `ReminderScheduleNotifier` | `reminder.rs:81` | 0 (transport-injected DI seam) | Legitimate |

Additional single-implementor traits outside `agent/providers/`: `ToolCallEncoder` (`tool_call_encoder.rs:82`, 1 impl), `ToolResultEncoder` (`tool_result_encoder.rs:81`, 1 impl), `StorageBackendModule` (`storage/modules.rs:23`, 1 impl).

`ManagerTopicSandboxCleanup`/`Control` could be replaced by direct method calls on `SandboxAdmin` (which already has multiple implementors and is the real polymorphism point).

---

## A2 — П0 crutch signals

### Category 1 — String-matching / regex over LLM output

1. `src/agent/recovery.rs:469` — `if trimmed_name.starts_with('{') && trimmed_name.contains("\"todos\"")` — **CRUTCH**. Detects "LLM put JSON args in the tool *name* field" by substring-matching LLM-generated name. Root cause = model violates tool-call schema; fix = regex-clean and reinject. Special-cased to `write_todos` only.
2. `src/agent/recovery.rs:496-497` — `if (trimmed_name.contains("todos") || trimmed_name.contains("write_todos")) && trimmed_name.contains('[')` — **CRUTCH**. Same class: substring-match over LLM tool-name.
3. `src/agent/recovery.rs:691` — `if !content.contains(tool_name) { continue; }` (loop over 12 hardcoded tool names) — **CRUTCH** (dead code). Scans free-form LLM content for tool-name substrings.
4. `src/agent/recovery.rs:824-826` — `trimmed.starts_with("recreate_sandbox") || trimmed.contains("[Call tools: recreate_sandbox]") || trimmed.contains("[Tool calls: recreate_sandbox]")` — **CRUTCH** (dead code).
5. `src/agent/recovery.rs:871-876` — `text.contains("[Tool call") || text.contains("Tool calls:") || text.contains("Call tools")` — **CRUTCH** (dead code). English/Russian marker substring-match.
6. `src/agent/recovery.rs:899` — `if text.contains(tool_name) { return true; }` — **CRUTCH** (dead code). Bare `contains` for 14 tool names.
7. `src/agent/recovery.rs:455-458` — `regex!(r"</?(?:tool_call|tool_name|filepath|arg_key|arg_value|command|query|url|content|directory|path|arg_key_[0-9]+|arg_value_[0-9]+|arg[0-9]+)>")` via `sanitize_xml_tags` — **CRUTCH** (live: called from `runner/tools.rs:294-295,359` and `response_dispatch.rs:129`). Strips leaked control XML from LLM output and feeds cleaned text back.
8. `src/agent/thoughts.rs:146-151` — `regex_replace_all!(r"^(I need to|Let me|I will|I should|First,?|Now,?|Next,?)\s*", cleaned, "")` — **CRUTCH**. Regex-strips English filler prefixes from LLM reasoning text.
9. `src/agent/loop_detection/llm_detector.rs:212-218` — `reasoning_lower.contains("times") || ...contains("repeated") || ...contains("same file") || ...contains("identical") || ...contains("loop")` — **CRUTCH**. Overrides scout LLM's `is_stuck=true`+high-confidence verdict unless reasoning contains one of 5 English keywords.
10. `src/agent/loop_detection/llm_detector.rs:333` — `LlmError::Unknown(msg) => msg.contains("Model") && msg.contains("not found")` — **BORDERLINE**. Substring-match over provider error string.
11. `src/agent/structured_output.rs:95,138,227` — `trimmed.starts_with("```")` / `!sanitized.contains('{')` / `trimmed.starts_with("[SYSTEM:")` — **BORDERLINE-LEGIT**. Triage on JSON-mode envelope.
12. `src/agent/tool_failure_summary.rs:185` — `error_message.to_ascii_lowercase().contains("do not retry")` — **BORDERLINE-LEGIT**. Match on tool error-message field, gated by `provider_unavailable && retryable==Some(false)`.
13. `src/agent/tool_failure_summary.rs:199,206` — `RE_ANTI_BOT_HOST.captures(content)` / `RE_HTTP_STATUS.captures(content)` — **BORDERLINE**. Text-fallback path over tool-error text; structured path at `:131` is primary.
14. `src/agent/thoughts.rs:127-133` — `cmd_lower.starts_with(pattern) || cmd_lower.contains(&format!(" {pattern}"))` — **LEGIT**. Display-only heuristic over deterministic shell command string.
15. `src/agent/memory_behavior.rs:32` — `scope.context_key.starts_with("session:")` — **LEGIT**. Deterministic internal scope-key prefix.
16. `src/agent/input_intent.rs:153-154` — `let start = raw.find('{')?; let end = raw.rfind('}')?;` — **BORDERLINE**. Naive JSON extraction from LLM classifier output.
17. `src/agent/loop_detection/content_detector.rs:44,116-117` — `content.contains("```")` / `content.contains('|')` / `content.contains('#')` — **LEGIT**. Markdown-structure detection, deterministic surface syntax.

### Category 2 — temporary/fallback/пока/потом patterns

- 0 `TODO`/`FIXME`/`HACK`/`XXX`/`unimplemented!`/`todo!()` markers. Crate is clean of deferral markers.
- `src/agent/recovery.rs:672,682` — `/// BUGFIX AGENT-2026-001: Extended to support ytdlp tools` — patch-history breadcrumb on malformed-tool-call recovery (added 5 ytdlp tool names to hardcoded list). Symptom of crutch subsystem being incrementally patched rather than redesigned.
- `src/agent/recovery.rs:1240` — `// Tests for is_valid_argument function (BUG-2026-0108-001 fix)` — targeted patch for garbage the LLM-in-content recovery was producing.

### Category 3 — Symptom treatment vs root cause (recovery design)

1. `src/agent/recovery.rs:32-200` (`repair_agent_message_history*`, `extract_valid_tool_calls`, `process_tool_results`, `filter_tool_calls_by_results`) — **LEGIT / class-closing**. Canonicalizes tool-call↔tool-result pairing by `invocation_id`, drops orphans/duplicates, repairs empty wire IDs, trims incomplete parallel batches. Treats protocol invariant directly.
2. `src/agent/recovery.rs:443-547` (`sanitize_xml_tags`, `sanitize_tool_call` PATTERN 1/2) — **CRUTCH / symptom-patching**. When LLM emits JSON args in tool-name field or leaks XML tags, code regex-cleans bad output and feeds it back as valid `ToolCall`. JSON-in-name branch special-cased to `write_todos` only (`:469-491`). Live via `response_dispatch.rs:129`.
3. `src/agent/recovery.rs:673-905` (`try_parse_malformed_tool_call`, `extract_*_arguments`, `looks_like_tool_call_text`) — **CRUTCH + dead code**. Hand-maintained per-tool argument extractor with 12 hardcoded tool names (`:674-688`) and bespoke parsers. No callers outside `recovery.rs` tests.
4. `src/agent/structured_output.rs:83-170` (`parse_structured_output` 4-pass recovery) — **MIXED**. Control-char stripping (`:106-133`) is legit deterministic-lexer fix. Prose-wrap branch (`:138-152`, via `looks_like_prose`) is symptom patch: "model ignored JSON mode → accept its prose as `final_answer`".
5. `src/agent/runner/responses.rs:31-41` (`should_salvage_structured_output_failure` → accept raw prose) — **CRUTCH**. Symptom: model returned prose instead of JSON; patch: accept prose and skip retry.
6. `src/agent/runner/responses.rs:44-69` (`structured_output_failures >= 3` → accept raw response) — **CRUTCH (fail-fast cap)**. After 3 consecutive JSON failures, raw text accepted as final answer. Caps damage but normalizes the failure mode.

### Category 4 — Default-shift / silent fallback

1. `src/llm/providers/chat_completions/response.rs:92-96` — `finish_reason: choice.get("finish_reason").and_then(Value::as_str).unwrap_or("unknown").to_string()` — **CRUTCH (mild)**. Silently invents `"unknown"` when provider omits `finish_reason`. Downstream cannot distinguish "provider sent null" from "we failed to parse".
2. `src/llm/providers/chatgpt/mod.rs:648-654` — `state.finish_reason = reason.unwrap_or(if state.tool_calls.is_empty() {"stop"} else {"tool_calls"}).to_string()` — **BORDERLINE**. Synthesizes finish_reason from tool-call presence. Masks missing field on `response.incomplete`.
3. `src/llm/providers/chat_completions/response.rs:128-131` — `arguments: function.get("arguments").map(normalize_tool_arguments).unwrap_or_else(|| "{}".to_string())` — **LEGIT**. Tools with no args legitimately default to `{}`.
4. `src/agent/input_intent.rs:122-135` — classifier error/timeout → `return None` (deterministic fallback, logged) — **LEGIT**.
5. `src/agent/loop_detection/llm_detector.rs:177-184` — scout model unavailable → `self.enabled = false; return Ok(false)` with `warn!` — **LEGIT**.
6. `src/llm/providers/chatgpt/auth.rs:190,241,427` — `response.text().await.unwrap_or_default()` — **BORDERLINE**. Empty body on HTTP error; HTTP status surfaced separately.

### Category 5 — Boolean-flag sprawl / config-as-control-flow

1. `src/agent/loop_detection/config.rs:12` (`enabled`) + `service.rs:54` (`disable_for_session`) + `llm_detector.rs:90` (`enabled`, separate field) — **BORDERLINE**. Three overlapping "enabled" flags with distinct scopes. Justified but borders on sprawl.
2. `src/agent/recovery.rs:35,43,52` — `allow_terminal_incomplete_batch` / `strict_tool_history` threaded through wrappers — **LEGIT**. Provider capability genuinely differs (`capabilities.strict_tool_history()` at `runner/llm_calls.rs:675`).
3. `src/llm/providers/chat_completions/profile.rs:165,187` — `json_mode: JsonModePolicy` / `structured_output: StructuredOutputPolicy` — **LEGIT**. Typed policy enums.
4. `src/agent/runner/responses.rs:47` — `if state.structured_output_failures >= 3` (magic threshold) — **BORDERLINE**. Hardcoded counter drives control flow.

### Category 6 — Duplicate / parallel logic

1. **JSON-extraction — 3+ variants:**
   - `src/agent/recovery.rs:574` `extract_first_json` — brace-depth scan with string/escape tracking + `serde_json` validation. (live)
   - `src/agent/loop_detection/llm_detector.rs:338` `extract_first_json_object` — near-identical brace-depth scan, *no* final `serde_json` validation. **DUPLICATE** with behavior gap.
   - `src/agent/executor/execution.rs:1084` `extract_json_object` — naive `find('{')`..`rfind('}')` slice. **DUPLICATE** (weaker; breaks on nested objects with trailing text).
   - `src/agent/input_intent.rs:153-154` — inline `find('{')`/`rfind('}')` slice. **DUPLICATE** of naive variant.
2. **"Looks like prose" — 2 divergent copies:**
   - `src/agent/structured_output.rs:223-240` `looks_like_prose` — rejects ```` ``` ````, `<`, `[SYSTEM:`; requires ≥24 non-ws + alphabetic; `unfinished_tail = ['{','[',':',',','-']`.
   - `src/agent/runner/responses.rs:288-313` `should_salvage_structured_output_failure` — same core logic but *also* rejects `{`/`[`-prefixed and adds `'"'` to `unfinished_tail`. **DUPLICATE with divergent edge cases** — latent bug source.

### Category 7 — Class-open vs class-closing

1. **Loop detection** — **MOSTLY class-closing, one open class.**
   - `tool_detector.rs` (SHA-256 over canonicalized args, `:78-90`) and `content_detector.rs` (chunk-hash sliding window with bounded history + reset hooks) CLOSE the class "deterministic repetition".
   - **OPEN CLASS:** `llm_detector.rs:202-230` `validate_detection` overrides scout LLM's `is_stuck=true` + `confidence ≥ threshold` unless `reasoning` contains one of 5 English keywords (`:212-218`). A model that correctly detects a loop with non-English or differently-phrased reasoning is silently rejected.
2. **Structured output** — **class-closing core, open recovery layer.**
   - **CLOSES:** `executor/execution.rs:452` selects structured vs unstructured mode from provider capability; `structured_output.rs:242-339` `validate_structured_output` enforces schema (exactly-one-of, known tool name, object args, non-empty fields). Protocol-forcing design.
   - **LEAVES OPEN:** `parse_structured_output` (`structured_output.rs:83-170`) runs 4 fallback passes; `responses.rs:31` accepts raw prose; `responses.rs:47` accepts raw after `>=3` failures. Model that persistently ignores JSON mode is *rewarded* with prose accepted.
3. **Compaction** — **class-closing.**
   - Typed message classes (`AgentMessageKind`), budget estimator (`budget.rs`), hot-memory classifier, externalized large tool payloads (`PrunedArtifact` in `memory.rs:176-183`), LLM summarization sidecar (`local_llm_summary.rs`), atomic history replacement with typed outcomes (`CompactionControllerError`). Failures are typed errors, not silent truncation. Legacy staged pipeline deliberately removed.

---

## A3 — Contracts and error handling

### A3.1 thiserror vs anyhow discipline — VIOLATION

- `thiserror::Error` in 19 files vs `anyhow` in 69 non-test lib files.
- `StorageError` (`src/storage/error.rs:4`) — 9 variants, two with structured fields: `DuplicateTopicPromptContent{topic_id, existing_kind, attempted_kind}` (`:31`), `ConcurrencyConflict{key, attempts}` (`:41`). Rest are `String` blobs.
- `LlmError` (`src/llm/error.rs:4`) — 10 variants. `ApiError{status, message}` (`:8`), `RateLimit{wait_secs, message}` (`:34`). **No variant carries provider name, model id, or operation.**
- `SandboxError` does not exist. `SandboxBackend` trait (`sandbox/traits.rs:50`) returns `anyhow::Result`. ~70 anyhow uses across `sandbox/manager.rs`, `sandbox/broker.rs`, `sandbox/traits.rs`.
- `src/agent/providers/manager_control_plane/{bindings,contexts,profiles,infra,agents_md}.rs` — ~8 `.map_err(|err| anyhow!("failed to upsert ...: {err}"))` each (e.g. `bindings.rs:160,322,328`, `contexts.rs:136,296,302`). Flattening typed `StorageError` into anyhow.
- `src/llm/providers/chatgpt/auth.rs:191,242,260,328,330,354,369` — `bail!`/`with_context` in lib code; converted to `LlmError` via `.to_string()` (`:292,302,305`), losing structure.

### A3.2 Provider contract: what the sender must know — SOUND

- `LlmProvider` (`src/llm/provider.rs:6-95`): every method's parameters are caller-knowable. `model_id` resolved by `LlmClient` from its own `model_info.id` (`client.rs:642`). No downstream-state probe.
- `StorageProvider` (`src/storage/provider.rs:15-568`): `claim_reminder_job(user_id, reminder_id, lease_until, now)` (`:447`) — caller supplies `reminder_id` (from `list_due_reminder_jobs`), `lease_until`/`now` (intent + observable clock). Receiver atomically verifies precondition: `WHERE status='scheduled' AND next_run_at<=$5 AND (lease_until IS NULL OR lease_until <= $5)` (`sqlx/mod.rs:1721-1729`). Caller not required to know whether job is still claimable — checked inside receiver. Textbook "move unreliable requirement inside receiver."
- `load_browser_artifact(user_id, artifact_uri)` (`:139`) — `artifact_uri` from prior `save_browser_artifact` response; `user_id` enforces ownership at storage layer.
- All id parameters (`reminder_id`, `topic_id`, `agent_id`, `flow_id`, `context_key`) are either caller-generated or returned from prior call. No id-of-unowned-object smell.
- Smell: `check_connection(&self) -> Result<(), String>` (`provider.rs:214`) — only method returning `String` instead of `StorageError`.

### A3.3 Tool runtime contract — SMELL

- `ToolRegistry` (`registry.rs:26`) — `BTreeMap<ToolName, Arc<dyn ToolExecutor>>`. Exact canonical-name dispatch (`registry.rs:80-93`); duplicate registration fails fast (`RegistryError::DuplicateTool`, `:44-48`).
- `ToolCallRuntime::execute_batch_with_blocked_calls` (`runtime.rs:120-174`) — records assistant calls, spawns one `tokio::task` per call (`:146`), awaits all (`:155-164`), sorts by `batch_index` (`:166`), verifies pairing.
- `ToolCallCorrelation` (`llm/types.rs:491-561`) — typed struct: `invocation_id: InvocationId` (newtype, `:304`), `provider_tool_call_id: Option<ProviderToolCallId>` (newtype, `:361`), `provider_item_id`, `protocol: ToolProtocol`, `transport: ToolTransport`. Builders enforce composition.
- `InvocationId`, `ProviderToolCallId`, `ProviderItemId`, `ToolCallId`, `ToolName`, `ToolBatchId`, `TurnId` — all `#[serde(transparent)]` newtypes. Prevent mixing id spaces at type level.
- **Call↔output pairing is runtime-enforced, not type-system-enforced:** `verify_outputs_match_calls` (`runtime.rs:267-302`) iterates `batch.calls.iter().zip(outputs)` and string-compares `call.tool_call_id != output.tool_call_id` (`:287`), `call.batch_index != output.batch_index` (`:281`), detects duplicates via `BTreeSet` (`:293`). Mismatch → `ToolRuntimeFatal::Invariant(...)` (`:282,288,294`). A buggy executor could return an output with wrong `tool_call_id` and type system won't catch it — only post-hoc verify will.

### A3.4 Schema drift surface — SOUND

- Versioning constants (`src/storage/schema.rs:1-10`): `AGENT_PROFILE_SCHEMA_VERSION=1`, `TOPIC_CONTEXT_SCHEMA_VERSION=1`, `TOPIC_AGENTS_MD_SCHEMA_VERSION=1`, `TOPIC_INFRA_CONFIG_SCHEMA_VERSION=1`, `AGENT_FLOW_SCHEMA_VERSION=1`, `TOPIC_BINDING_SCHEMA_VERSION=2` (bumped 1→2), `AUDIT_EVENT_SCHEMA_VERSION=1`, `REMINDER_JOB_SCHEMA_VERSION=2` (bumped 1→2).
- Every required record carries `schema_version`: `AgentProfileRecord` (`control_plane.rs:12`), `TopicBindingRecord` (`:150`), `ReminderJobRecord` (`reminder.rs:132`), `AuditEventRecord` (`control_plane.rs:183`), `AgentFlowRecord` (`flows.rs:7`), `TopicContextRecord` (`:31`), `TopicAgentsMdRecord` (`:50`), `TopicInfraConfigRecord` (`:100`).
- Written on INSERT/UPSERT: `sqlx/mod.rs:616,958,1055,1146` bind `record.schema_version`; INSERTs at `:180,302,346,397,604,943,1040,1140` include column.
- Read back: `sqlx/rows.rs:34,48,63,78,104,133,153,183` deserialize via `i32_to_u32`.
- `version` field (logical revision) bumped per mutation: `reminder_tx.rs:52,120`; `sqlx/mod.rs:1723` `version = version + 1`.
- Migration story: `SqlxStorage::run_configured_migrations` (`sqlx/mod.rs:96`) → `run_migrations_from_path` (`:107`) uses `sqlx::migrate::Migrator::new(path)` (`:111`) + `migrator.run(&pool)` (`:115`). Migrations at workspace root `/home/stfu/ai/Oxide-Agent/migrations/` (0001–0009+). Errors map to `StorageError::DatabaseMigration`.
- Smell (minor): migrations loaded from runtime path (`config.migrations_dir`, `sqlx_config.rs:30,92`), not embedded via `sqlx::migrate!`. Deploy-time risk if path missing in container.

### A3.5 Race / concurrency contract — SOUND

- `claim_reminder_job` (`sqlx/mod.rs:1711-1746`): atomic `UPDATE ... SET lease_until=$3, version=version+1, updated_at=$4 WHERE ... AND status='scheduled' AND next_run_at<=$5 AND (lease_until IS NULL OR lease_until <= $5) RETURNING ...`. No SELECT-then-UPDATE race. Two concurrent workers cannot both claim.
- `list_due_reminder_jobs` (`sqlx/mod.rs:1693-1699`): filters `(lease_until IS NULL OR lease_until <= $2)` — leased jobs not listed.
- `ReminderJobRecord::is_due`/`is_leased` (`reminder.rs:180-191`) mirror SQL guard in-memory.
- Topic records use `FOR UPDATE` inside transactions: `topic_tx.rs:142` (binding), `:184` (agents_md), `:146` (context).
- No `std::sync::Mutex`/`RwLock` guard held across `.await` found. `executor/policy_hooks.rs:5,10` — `std::sync::RwLock<HookAccessPolicy>`, guard scope `:33-37` inside sync `handle()`, dropped before any await. `executor/config.rs:35,38` — same pattern.
- Serialization smells (tokio::Mutex across await, clippy-legal but serializes):
  - `AgentRunner.loop_detector: Arc<tokio::sync::Mutex<LoopDetectionService>>` (`runner/mod.rs:37,55`). Held across `.await` at `loop_detection.rs:64-67`.
  - `ManagerControlPlaneToolExecutor.execution_lock: Arc<tokio::sync::Mutex<()>>` (`manager_control_plane/mod.rs:608,722`), acquired at `:739` then `execute_tool(...).await` — all manager control-plane tools serialize on one global lock. Acceptable at ≤5 RPS personal scale.
- Bounded concurrency: `executor/execution.rs:38` `WIKI_MEMORY_BACKGROUND_WRITER_SEMAPHORE: Semaphore = const_new(1)` acquired at `:673`. Correct.

### A3.6 Error propagation — SMELL

- **Trace 1 — LLM provider API error (e.g. OpenRouter 500):** Provider returns `Err(LlmError::ApiError { status: Some(500), message: "..." })`. `LlmClient::chat_with_tools` (`client.rs:599-702`): on `Err(e)` at `:671`, logs `model`, `attempt`, `error` (`:672-679`) — observability good. After retries exhausted, returns `Err(e)` **verbatim** at `:696` — no wrapping with provider name or model id. Final fallback `Err(LlmError::api_error("All retry attempts exhausted"))` at `:701` — bare string, no provider/model context. `LlmError::ApiError` (`error.rs:8-14`) has only `{status, message}` — no `provider`, `model`, or `operation` field. Caller cannot programmatically determine which provider failed without string-parsing `message`.
- **Trace 2 — ChatGPT OAuth/session error:** `chatgpt/auth.rs:285` `get_valid_session` returns `Result<ChatGptSession, LlmError>`. Internal anyhow errors (`:191,242,328,330`). Conversion at `:292` `.map_err(|error| LlmError::MissingConfig(error.to_string()))` and `:302,305` `.map_err(|error| LlmError::api_error(error.to_string()))` — flattens anyhow context chain into single string. Word "ChatGPT" appears only because bail messages hardcode it; typed `LlmError` does not record provider.
- **Trace 3 — Sandbox exec error:** `SandboxManager::exec_command` (`manager.rs:721`) returns `anyhow::Result<ExecResult>`. Backend errors propagate as `anyhow::Error` with `.context()` / `anyhow!()` strings (`manager.rs:336`, broker `:273,290,612,624`). No `SandboxError` enum. Caller gets anyhow blob, must `to_string()` into `ToolRuntimeError::Failure(...)` — agent sees raw string, no way to distinguish "container not found" from "exec timeout" from "image pull failed" without substring matching.
- **Trace 4 — Storage conflict (counterexample, GOOD):** `StorageError::ConcurrencyConflict { key, attempts }` (`error.rs:41`) — structured, names key and retry count. Caller can match on variant. Actionable.

### A3.7 Secret handling contract — SOUND

- `ssh_mcp.rs:1748-1764` `resolve_secret_ref(storage, user_id, secret_ref)`: `env:` prefix → `std::env::var(env_name)` (`:1753-1755`); `storage:` prefix → `storage.get_secret_value(user_id, storage_key)` (`:1758-1762`); bare ref → treated as storage key.
- Second resolution: `ssh_mcp.rs:1933-1940` `resolve_secret_material` — same prefix logic.
- `StorageProvider::get_secret_value` (`provider.rs:330`) / `put_secret_value` (`:340`) / `delete_secret_value` (`:354`) — storage namespace boundary.
- `SecretProbeReport` (`ssh_mcp.rs:78-100`) — structurally carries no secret value: fields are `secret_ref, source, kind, present, usable, status, fingerprint, key_type, comment, error`. `error` field doc (`:98-99`): "Safe error summary without secret material."
- `probe_secret_ref` (`:1322-1349`) resolves value but only passes to validators; returned report never includes value.
- `SecretProbeReport::summary()` (`:153-184`) emits only metadata strings.
- LLM-facing tool `execute_private_secret_probe` (`manager_control_plane/mod.rs:623-631`) serializes `json!({ "ok": true, "secret_probe": report })` — report has no value field. **SAFE.**
- `ResolvedBackendAuth` (`ssh_mcp.rs:1799-1804`) holds `password/key_file/sudo_password` — used only to write tempfile (`:1789`) and feed SSH process args; never serialized into `ToolOutput` or prompt.
- `TopicInfraConfigRecord.secret_ref`/`sudo_secret_ref` (`control_plane.rs:118,120`) store reference, not material. Manager tool outputs echo ref string (`infra.rs:172,191,339`), not resolved value.
- **Caveat (SMELL, not VIOLATION):** No defense-in-depth secret-redaction pass in output normalizer (`tool_runtime/normalizer.rs`, `output.rs` — grep for secret/redact/password returned empty). Safety relies on each tool not emitting secret-bearing fields. Per-tool discipline, not enforced property.

---

## A4 — Testing discipline

### A4.1 cfg-gating hygiene — PARTIAL

- `build.rs:29-41` emits `oxide_module_<id>` aliases + `rustc-check-cfg` declarations for `unexpected_cfgs`.
- Top-level `tests/` directory clean: 6 of 11 files gate at crate root with `#![cfg(oxide_module_...)]` (`tests/anthropic_e2e.rs:1`, `tests/mistral_e2e.rs:1`, `tests/hermetic_agent.rs:3`, `tests/rate_limit.rs:11-14`, `tests/json_decode_error.rs:10-13`, `tests/sub_agent_delegation.rs:3,:213,:271`). One profile-level raw gate — `tests/modular_registry_snapshots.rs:2-7` — explicitly allowed.
- 26 raw module-level `#[cfg(feature = "...")]` gates in test contexts:
  - `src/agent/providers/delegation.rs:2071,2117` `tool-todos`; `:2236,2238` `tool-sandbox-exec`/`tool-sandbox-fileops`
  - `src/agent/providers/media_file.rs:1043` `llm-mistral`
  - `src/agent/runner/llm_calls.rs:1115,1165` `llm-openai-base`; `:1429,1510,1611` `llm-opencode-go`
  - `src/agent/runner/model_routes.rs:179,265` `llm-chatgpt`; `:311,381` `llm-opencode-go`
  - `src/agent/runner/response_dispatch.rs:350` `llm-opencode-go`
  - `src/agent/runner/runtime_compaction.rs:532,625,935` `llm-opencode-go`
  - `src/agent/providers/manager_control_plane/tests/sandboxes.rs:16`, `agent_controls.rs:338`, `forum_topics.rs:279`, `infra.rs:216`, `support.rs:146,184,216,251` — `integration-ssh-mcp`

### A4.2 Test category coverage — SOUND

- Hermetic: dominant. ~95+ src files carry inline `#[cfg(test)] mod tests`. Dedicated: `tests/hermetic_agent.rs`, `tests/cancellation_respected.rs`, `tests/json_decode_error.rs`, `tests/rate_limit.rs`, `tests/sub_agent_delegation.rs`, `tests/tool_runtime_static_guards.rs`.
- Integration (real Postgres): `src/storage/sqlx/tests.rs` — 1 file, properly skip-gated.
- Integration (live LLM): `tests/anthropic_e2e.rs`, `tests/mistral_e2e.rs`.
- Snapshot (insta): 7 tracked `.snap` files under `tests/snapshots/`, 4 `assert_snapshot!` call sites. Per-profile snapshots: `@profile-full`, `@profile-embedded-opencode-local`, `@profile-web-embedded-opencode-local`, `@profile-search-only`, `@all-features`.
- Property (proptest): 1 file, 3 properties (`tests/proptest_recovery.rs`).
- Static-guards: `tests/tool_runtime_static_guards.rs` greps source for forbidden legacy labels.
- Counts: 141 inline test modules in src, 11 dedicated `tests/*.rs`, 1394 total `#[test]`/`#[tokio::test]` fns.

### A4.3 Hermetic vs integration — SOUND

- Real Postgres: `src/storage/sqlx/tests.rs:871-894` `sqlx_test_storage_with_connections()` returns `Option<SqlxStorage>`; `:876 eprintln!("OXIDE_DATABASE_TEST_URL not set; skipping")` then `return None`. Every consumer uses `let Some(storage) = ... else { return }`.
- Real LLM: double-gated — `RUN_LLM_E2E_CHECKS=1` env (`tests/anthropic_e2e.rs:24-26`, `tests/mistral_e2e.rs:23-25`) AND valid non-`dummy` API key (`tests/anthropic_e2e.rs:61-67`). Soft-skip on expected errors via `is_expected_error` (`:105-108`).
- Local hermetic HTTP: `src/agent/providers/webfetch_md/tests.rs:57,93,125,162` uses `TcpListener::bind("127.0.0.1:0")` (ephemeral) + `tokio::spawn` local server. `:273 rejects_localhost_and_private_ips` asserts SSRF guard.
- Local hermetic concurrency: `tests/hermetic_agent.rs:224`, `tests/json_decode_error.rs:120,174` `tokio::spawn` for mock providers.
- No `dotenvy::dotenv` outside two e2e files. No raw Docker integration tests in this crate.

### A4.4 Mock quality — PARTIAL

- `mock_storage_noop` (`src/testing.rs:100-108`): blanket-returns contract-masking values — `get_user_state -> Ok(None)` (`:116`), `load_agent_memory[_for_context] -> Ok(None)` (`:125-127`), `load_agent_memory_for_flow -> Ok(None)` (`:133-134`), `get_agent_flow_record -> Ok(None)` (`:137-138`), `get_agent_profile -> Ok(None)` (`:153`), `get_topic_binding -> Ok(None)` (`:169`), all reminder get/claim/reschedule/complete/fail/cancel/pause/resume/retry -> `Ok(None)` (`:233-253`), `list_* -> Ok(Vec::new())` (`:154-155,202-205,234-237`), mutations all `Ok(())`. A test using `mock_storage_noop` that expects storage to return a record would silently pass against `None`.
- `mock_llm_simple` (`src/testing.rs:58-71`): asymmetric and safer — only sets `expect_complete_internal_text`, *errors* on `transcribe_audio`/`analyze_image` (`:65,68`); other mockall methods unset → panic with "No matching expectation set". Contract-faithful.
- Mitigating: `mock_storage_noop` used at only 2 sites (`src/agent/providers/reminder.rs:1063,1084`, `src/agent/providers/browser_live/tools.rs:2741`). `mock_llm_simple` at 1 site (`src/agent/runner/hooks.rs:266`). 99 other sites build `MockStorageProvider::new()` + explicit per-method `.expect_*().with(eq(...)).returning(...)` — contract-faithful.

### A4.5 П0.5 live-contract coverage — PARTIAL

- Live-shape-asserting tests: `tests/anthropic_e2e.rs` hits `https://api.anthropic.com` (`:73,137`), asserts `!response.content.is_empty()` (`:94-101`), `!first_response.tool_calls.is_empty()` (`:271`), second-turn tool-result round-trip (`:488-491`). `tests/mistral_e2e.rs` — same pattern (`:71-74`, `first_response.tool_calls.clone()`).
- **Absent:** no live e2e for OpenRouter, ChatGPT/Codex OAuth, OpenCode Go, ZAI/Zhipu, MiniMax. Provider code paths exercised by mocked-shape unit tests only (`src/llm/providers/modules.rs:819,847,978,1001`).
- E2e tests have `is_expected_error` soft-skip (`tests/anthropic_e2e.rs:105-108`) — assert shape only on success, not strictly.

### A4.6 Test helpers centralization — PARTIAL

- `src/testing.rs` is documented single source. `test_set_env`/`test_remove_env` (`:16,26`) correctly used for Rust-2024 `unsafe` env ops across crate.
- Mock helpers barely used: `mock_storage_noop` at 2 sites, `mock_llm_simple` at 1 site. 99 raw `MockStorageProvider::new()` + ~27 raw `MockLlmProvider::new()` outside `src/testing.rs`.
- Partial consolidation: `src/agent/runner/test_support.rs:50,59,77,106` wraps `MockLlmProvider::new()` into `single_final_response_provider` / `stub_non_chat_methods` / `build_llm_client_for_provider`. Storage has no equivalent builder.

### A4.7 Snapshot discipline — SOUND

- 7 `.snap` files tracked in git. Locked/reviewed via standard `cargo insta review` workflow. No `insta.toml` / `.insta` config — defaults fine for current scale.
- Per-profile isolation: `tests/modular_registry_snapshots.rs:120-123` `insta::with_settings!({ snapshot_suffix => profile }, { insta::assert_snapshot!(...) })` — one snapshot per compiled profile.
- Snapshots carry insta metadata headers (`source:` / `expression:`).
- Content meaningful: `modular_registry_snapshots` pins full compiled capability manifest, registered tool names, provider IDs/aliases, storage/sandbox backend module IDs, external-service requirements per profile. `snapshot_prompts` pins fallback prompt and structured-output instructions.
- Minor gap: no snapshots for LLM request/response shaping or compaction output.

### A4.8 Property/fuzz coverage — WEAK

- Sole site: `tests/proptest_recovery.rs`. Three properties:
  - `:7-11 does_not_crash(s in "\\PC*")` — fuzz-style: any valid UTF-8 string does not panic in `sanitize_xml_tags`.
  - `:14-37 removes_forbidden_tags` — parameterized over entire enumerated tag class, asserts tag absent and prefix/suffix preserved.
  - `:40-56 removes_multiple_tags` — nested/interleaved tags all removed, content preserved.
- Good properties. Weakness is breadth: covers exactly one function. No proptest for: `canonicalize_tool_call_args` (loop detection JSON canonicalization), `parse_structured_output` (malformed-JSON handling), storage key encoding, wiki slug derivation, compaction budget arithmetic.

### A4.9 Loop detection / structured output test strength — PARTIAL

- **Structured output** (`src/agent/structured_output.rs:342` mod tests, 19 tests): strong enumerated coverage — three terminal fields exercised (`parses_valid_final_answer:351`, `parses_valid_tool_call:367`, `parses_valid_awaiting_user_input:386`); rejection classes (`rejects_missing_both:405`, `rejects_both_set:413`, `rejects_unknown_tool:427`, `rejects_non_object_arguments:434`, `rejects_empty_thought:441`); real-world malformed inputs (`parses_json_inside_code_fence:458`, `parses_json_with_leading_text:468`, `parses_json_with_control_chars:478`); prose-heuristic boundary (`wraps_prose_as_final_answer:491`, `does_not_wrap_short_text:510`, `does_not_wrap_code_fence:519`). Not formally class-closing: no proptest "any non-JSON garbage → Err".
- **Loop detection** (4 files, 11 tests): `content_detector.rs:203` (5 tests: `detects_repetition:207`, `skips_code_blocks:217`, `truncates_history:226`, `ignores_tables_and_lists:236`, `reset_tracking_clears_history:245`). Missing: boundary at N-1 vs N, partial-overlap, unicode normalization. `tool_detector.rs:115` (4 tests: `detects_at_threshold:119`, `resets_on_tool_change:128`, `resets_on_args_change:137`, `canonicalize_tool_call_args_sorts_object_keys_recursively:146`). **Gap:** no test proves detector uses canonicalization to catch reordered-arg loops. `service.rs:187` (2 tests: `disables_for_session:204`, `tool_call_detection_triggers:213`). `MockScout` hardcodes `is_stuck:false` — LLM-scout escalation path never asserted to fire. `llm_detector.rs:372` (2 tests: `detects_loop_when_confident:397`, `skips_before_threshold:417`). Missing: confidence-boundary, malformed scout JSON, scout error propagation, `reasoning` field surfaced.

---

## A5 — LLM integration correctness

### A5.1 tool_call_id integrity — SOUND (minor dual-field smell)

- `ToolCallCorrelation` (`src/llm/types.rs:491-561`): typed struct with `invocation_id: InvocationId` (newtype, `:304`), `provider_tool_call_id: Option<ProviderToolCallId>` (newtype, `:361`), `provider_item_id`, `protocol: ToolProtocol`, `transport: ToolTransport`. Runtime invocation IDs UUID-generated: `InvocationId::new(format!("call_{}", Uuid::new_v4()))` (`src/agent/runner/tools.rs:161`).
- Tool results carry correlation forward: `ToolCallCorrelation::new(output.invocation_id.clone()).with_provider_tool_call_id(output.tool_call_id.as_str())` (`tools.rs:376-379`), stored via `Message::tool_with_correlation` (`tools.rs:385-390`).
- `validate_tool_history` (`src/llm/support/history.rs:202-243`) runs after system-message folding but before provider call (`src/llm/client.rs:538`, `:623`). Rejects: empty tool-call batches (`:214-218`), empty `invocation_id` (`:93-97`), duplicate `invocation_id` (`:98-103`), empty explicit `provider_tool_call_id` (`:104-109`, `:245-250`), tool results missing invocation_id (`:126-128`), results referencing unknown ids (`:146-150`), duplicate results (`:152-156`), incomplete batches under strict/non-terminal policy (`:164-184`), orphaned tool results (`:235-237`). Every failure → `LlmError::RepairableHistory`.
- `LlmError::RepairableHistory` triggers `repair_history_before_retry` (`src/agent/runner/llm_calls.rs:556-572`, `:664-720`), calls `repair_agent_message_history_for_provider` (`src/agent/recovery.rs:48-53`). Repair (`recovery.rs:164-200`, `:210-251`, `:254-346`): drops orphaned tool results, trims orphaned tool calls, converts empty-batch messages back to plain assistant text, canonicalizes result correlations to match assistant batch (`:298-310`), repairs empty wire IDs (`:231-240`).
- `ToolCorrelationNormalizer` (`src/llm/providers/tool_correlation.rs:22-33`) backfills `provider_tool_call_id` from `invocation_id` if absent.
- Smell: `Message` carries *both* `tool_call_id: Option<String>` (`types.rs:47`) and `tool_call_correlation: Option<ToolCallCorrelation>` (`:49`), likewise `tool_calls` + `tool_call_correlations`. Dual representation can drift, but `resolved_tool_call_correlation()` (`:173-177`) and `resolved_tool_call_correlations()` (`:181-190`) canonicalize with fallback; repair pass sets both consistently (`recovery.rs:298-310`). Provider never sees inconsistent history because validation + repair run before every send. **RECEIVER guarantees integrity; SENDER not trusted.**

### A5.2 Structured output parsing — SMELL

- Happy path typed: `try_parse_structured_output` uses `serde_json::from_str::<StructuredOutput>` (`src/agent/structured_output.rs:176`), then `validate_structured_output` enforces exactly-one-of (`:264-273`), non-empty fields (`:246-258`), known tool name (`:284-299`), object arguments (`:301-305`).
- **Design does NOT force provider-side JSON when tools present:** `should_use_native_json_mode` (`src/llm/providers/chat_completions/request.rs:356-361`) is `matches!(profile.json_mode, JsonModePolicy::Standard) && json_mode && !has_tools`. Agent loop almost always has tools, so `response_format: {type: "json_object"}` is **not** set (`request.rs:180-181`). Structured output enforced only via prompt instructions — `build_structured_output_instructions` (`src/agent/prompt/composer.rs:480-537`) injects "You MUST respond ONLY with a valid JSON object..." as system-prompt text. Model is asked, not forced.
- Post-hoc recovery uses string heuristics over LLM text: `parse_structured_output` (`structured_output.rs:84-170`) cascades through code-fence stripping (`:95-99`), `strip_control_chars` (`:106`), `strip_all_control_chars` (`:122`), prose wrapper synthesizing `{"final_answer": <escaped prose>}` when `!sanitized.contains('{') && looks_like_prose(...)` (`:138-152`), `recovery_candidates` calling `extract_first_json` (`:154-165`). `looks_like_prose` (`:223-240`) uses `starts_with`, `ends_with`, character-count heuristics.

### A5.3 Recovery from malformed responses — SPLIT

- **History repair — SOUND, class-closing:** `repair_agent_message_history*` (`:32-53`, `:164-200`) and `prune_tool_history_by_availability` (`:60-162`) operate on typed `AgentMessage`/`ToolCallCorrelation`. Drop orphaned tool results (`:188-193`, `:269-273`), trim orphaned tool calls (`:223-229`), convert empty-batch messages (`:358-373`), canonicalize correlations (`:291-312`), repair empty wire IDs (`:231-240`). Triggered by `LlmError::RepairableHistory` from pre-request validation (`llm_calls.rs:556-572`). Inconsistent history **repaired before provider ever sees it**. RECEIVER handles whatever arrives.
- **Content/tool-call sanitization — symptom-patching:** `sanitize_xml_tags` (`:443-448`) uses `regex!` (`:455-458`) to strip leaked control XML tags and feeds cleaned text back. `sanitize_tool_call` (`:463-547`) uses `starts_with('{')`, `contains("\"todos\"")`, `find('[')` (`:469`, `:496-500`) to reconstruct malformed tool calls. `try_parse_malformed_tool_call` (`:673-703`) + `extract_*_arguments` (`:705-860`) parse XML-like syntax. `looks_like_tool_call_text` (`:869-905`) uses `contains` over hardcoded tool-name list. Regex-clean bad output and feed back.
- **Structured-output error recovery** (`src/agent/runner/responses.rs:19-97`): (1) `should_salvage_structured_output_failure` (`:288-313`) accepts raw prose as `final_answer` without retry (`:31-42`); (2) after 3 consecutive failures, gives up and accepts raw (`:47-69`); (3) otherwise re-prompts with `[SYSTEM: ...Return ONLY valid JSON...]` (`:82-96`) — adds system note but does **not** change `json_mode`, switch provider mode, or escalate to structured-output-capable route.

### A5.4 Loop detection class-closing — SMELL (partial; halt-only remediation)

- **Tool-sequence layer** (`src/agent/loop_detection/tool_detector.rs`): signal = SHA256 of `tool_name + ":" + canonicalized_args` (`:78-84`), args canonicalized via `serde_json` parse + recursive key sort (`:92-112`). Detection = consecutive identical hash, `repetition_count >= threshold` (`:35-54`). **Only catches consecutive identical calls**; A-B-A-B alternating cycles reset counter (`:38-40`) and evade. Remediation = **STOP** (`src/agent/runner/loop_detection.rs:24-56` cancels token, returns error). Recovered tool calls (`is_recovered=true`) **bypass** tool loop detector (`runner/loop_detection.rs:93-99`).
- **Content layer** (`src/agent/loop_detection/content_detector.rs`): signal = SHA256 of fixed-size char chunks within sliding window (`:43-86`, `:151-155`), occurrence count + average-distance constraint (`:157-199`). Skips code blocks, tables, lists, headers via `contains('|')`/`contains('#')` + `lazy_regex!` (`:44-60`, `:115-121`). Remediation = **STOP**.
- **LLM layer** (`src/agent/loop_detection/llm_detector.rs`): signal = scout model judges `is_stuck` + `confidence` as typed JSON (`LlmLoopResponse`, `:43-51`), parsed via `serde_json::from_str` then brace-counting fallback (`:312-328`, `:338-368`). `validate_detection` (`:202-230`) requires `is_stuck && confidence >= threshold` **and** reasoning containing evidence keywords via `reasoning_lower.contains("times"/"repeated"/"same file"/"identical"/"loop")` (`:213-218`). On scout-model unavailability, disables LLM checks for session (`:177-184`). Remediation = **STOP**. All three layers remediate by **halt** — no re-prompt with "you are looping, change approach", no strategy escalation, no context injection.

### A5.5 Route failover & 429 quarantine — SOUND

- Weighted selection: Route 0 preferred if available (`src/agent/runner/model_routes.rs:34-43`). Fallback candidates (index ≥ 1) filtered by `route_is_available` (`:84-106`) and selected by weighted round-robin (`:68-79`). `route_is_available` checks v1-tool-route compatibility, `json_mode_forbids_route` (ChatGPT routes forbidden for json_mode, `:108-114`), capabilities, not-exhausted, provider-available, not quarantined (`:100-105`).
- Quarantine is typed, time-based state: `route_failover_state.route_quarantine: HashMap<String, Instant>` keyed by `provider:id` (`:126-128`). `quarantine_model_route` (`:116-124`) inserts `Instant::now() + duration`. Expired entries evicted on each selection via `retain(|_, until| *until > now)` (`:26-28`). Duration = `rate_limit_quarantine_duration` = `LlmClient::get_retry_delay(error, attempt).unwrap_or(60s)` (`llm_calls.rs:722-724`).
- Triggered on persistent 429: rate-limit failover at `llm_calls.rs:654-656` returns `FailoverToNextRoute` only when retry budget on same route exhausted (`:584-651` retries with backoff first; `:654` reached only when `retry_budget_remaining` is false), then `:520-527` quarantines + fails over.
- Backoff exponential-capped: rate-limit cap 120s, transient cap 30s, respects server `wait_secs` (`src/llm/support/backoff.rs:24-61`, `:88-119`). `MAX_RETRIES = 15` (`backoff.rs:6`).

### A5.6 Prompt cache hit architecture — SOUND (minor wiki-in-base smell)

- `ComposedPrompt` splits into `base` (cacheable) + `date_suffix` (volatile) (`src/agent/prompt/composer.rs:19-24`). `base` = fallback prompt + role instructions + workflow guidance + wiki context + structured-output instructions (`:564-598`). **No timestamp or per-request user data in `base`.** `date_suffix` = `build_date_context` using `chrono::Local::now()` (`:50-60`, `:602`) — timestamp isolated in suffix.
- Fold pipeline assembly order (`src/llm/support/history.rs:29-83`): `system_prompt(base) + stable_system_messages + date_suffix + volatile_system_messages` (`:56-80`). Stable system messages (starting with `[TOPIC_AGENTS_MD]` or `[OXIDE_COMPACTED_SUMMARY_V1]`, `:10`) placed **before** date suffix to extend cacheable prefix (`:66-69`). Volatile system messages (retry notes, `[SYSTEM: ...Return ONLY valid JSON...]` recovery note) go **after** date suffix (`:77-80`). Sub-agent task excluded from system prompt, delivered via user message (`composer.rs:632-634`).
- Smells (minor): (1) `wiki_context` appended to `base` (`composer.rs:587`) despite comment calling it "dynamic (varies per task keywords)" (`:586`) — if wiki memory updates mid-task, base changes and busts cache. (2) `date_suffix` includes tool-derived search hints (`composer.rs:55-56`) alongside timestamp — mixing stable with volatile in suffix; acceptable since suffix is after cache boundary.

### A5.7 Compaction design — SOUND

- Typed message classes: `AgentMessageKind` enum (`src/agent/compaction/types.rs:22-47`) with 12 variants. `CompactionRetention` enum (Pinned/ProtectedLive/PrunableArtifact/CompactableHistory, `:51-60`). `retention()` maps kind→retention deterministically (`:65-78`); `resolve_retention` adds tool-aware override (`:83-90`). `ToolResult → PrunableArtifact`; `TopicAgentsMd/Summary → Pinned`; `UserTask/RuntimeContext → ProtectedLive`.
- Budget estimator deterministic: cached `cl100k_base` tokenizer via `OnceLock` (`src/agent/compaction/budget.rs:15-20`). `estimate_request_budget` (`:30-77`) counts system-prompt tokens, tool-schema tokens, hot-memory tokens grouped by retention class (`:79-117`), computes `projected_total_tokens` and derives typed `BudgetState` (Healthy/Warning/ShouldCompact/OverLimit) from percentage thresholds (`:52-60`, `types.rs:272-290`).
- Large tool payloads externalized: `bounded_summary_source_messages` (`src/agent/compaction/controller.rs:178-234`) selects messages within `source_budget` derived from route window (`:188-197`), always including Pinned+ProtectedLive, then collecting recent messages until budget exhausted (`:202-228`). `build_local_compaction_user_message` (`src/agent/compaction/prompt.rs:38-83`) **truncates each message content to 1,600 chars** (`:66`, `LOCAL_COMPACT_MESSAGE_PREVIEW_CHARS`) and reasoning to 500 chars (`:70`). Summarizer receives bounded, truncated previews via `complete_internal_text` — **never raw huge payloads.**
- Deterministic replacement: `build_compacted_history` (`history.rs:83-200`) keeps pinned + one authoritative `[OXIDE_COMPACTED_SUMMARY_V1]` summary + recent tail collected in two phases (budget-constrained then minimum-floor), with **tool-call pairs collected atomically** so tail never has orphaned results (`:117-125`). Tool-pair validation → `CompactedHistoryBuildError::InvalidToolHistory` (`:42-44`). Replacement atomic via `memory.replace_compacted_history` (`controller.rs:168`). Generation monotonic (`controller.rs:273-289`).

### A5.8 Provider capability negotiation — SOUND

- `ProviderCapabilities` (`src/llm/capabilities.rs:62-70`): `tool_history_mode: ToolHistoryMode` (BestEffort/Strict, `:53-59`), `supports_tool_calling`, `supports_structured_output`. Methods: `can_run_agent_tools` (`:104-106`), `can_run_chat_with_tools_request` (`:113-119` — `has_tools` requires `supports_tool_calling`; no-tools allows `supports_tool_calling || (json_mode && supports_structured_output)`).
- Static table per compiled module, default-deny: `provider_capabilities` delegates to `module.capabilities()` (`src/llm/providers/modules.rs:432-434`); `provider_capabilities_for_model` delegates to `module.capabilities_for_model(model_info)` (`:493-498`). Module list compiled-in (`compiled_provider_modules`, `:530-550`). Unknown provider/model → `None` → `default_provider_capabilities()` = `(BestEffort, false, false)` (`capabilities.rs:155-157`) — **default-deny**, not assumed.
- Model-level explicit allowlist (verified, not assumed): OpenRouter has `openrouter_model_policy` (`src/llm/providers/openrouter/module.rs:84-114`) — explicit match on normalized model IDs with per-model `approved_for_main_agent`, `supports_tools_parameter`, `supports_structured_outputs`, media flags. Gemini routes: structured-output yes, `approved_for_main_agent=false` (`:86-98`). DeepSeek: tools + structured + main-agent yes (`:100-111`). Unknown model → `None` → provider default-deny.

### A5.9 Hot context health hook — SOUND

- Always active: registered unconditionally in `src/agent/executor/config.rs:43` (`runner.register_hook(Box::new(HotContextHealthHook::new()))`).
- Typed signal, deterministic thresholds: `HotContextHealthHook` (`src/agent/hooks/hot_context.rs:10-110`) handles only `BeforeIteration` (`:85-87`). Soft limit = `percent_of(max_tokens, warning_threshold_percent)` (`:23-28`); below → `HookResult::Continue` (`:91-93`). At soft limit → `HookResult::InjectTransientContext(notice)` (`:108`). Hard limit = `percent_of(max_tokens, compact_threshold_percent).max(soft+1)` (`:30-37`); at/above → `HookResult::RequestCompaction { reason, context }` (`:99-105`). `HookResult` is typed enum (`InjectTransientContext`/`RequestCompaction`/`Block`/`ForceIteration`/`InjectContext`/`Continue`, `registry.rs:45-90`). `percent_of` is `const fn` (`hot_context.rs:69-71`). `CompactionPolicy` defaults: 65%/85%/95% (`types.rs:199-208`). Notice mentions `compress` only when tool available (`hot_context.rs:51-55`).

---

## Provider Profile Matrix (П0.5 baseline for Phase 1)

Current provider configuration for structured output and json_mode, read from source:

| Provider | Profile constructor | `JsonModePolicy` | `StructuredOutputPolicy` | `supports_structured_output` (default) | `response_format` set when | Source |
|---|---|---|---|---|---|---|
| Mistral | `mistral()` | `Standard` | `BaseCapability` | `true` | `json_mode && !has_tools` | `profile.rs:251-300` |
| ZAI/Zhipu | `zai()` | `Standard` | `ZaiGlmToolModelsOnly` | `false` (per-model override via `zai_supports_structured_output`) | `json_mode && !has_tools` | `profile.rs:303-343` |
| OpenRouter | `openrouter()` | **`None`** | `BaseCapability` | `false` (per-model override in `openrouter/module.rs:84-114`) | **never** (json_mode disabled) | `profile.rs:346-386` |
| OpenCode Go | `opencode_go()` | `Standard` | `BaseCapability` | **`false`** | `json_mode && !has_tools` | `profile.rs:389-431` |
| OpenCode Zen | `opencode_zen()` | `Standard` | `BaseCapability` | `false` (inferred) | `json_mode && !has_tools` | `profile.rs:434-449` |
| Generic | `generic()` | `Standard` | `BaseCapability` | `true` | `json_mode && !has_tools` | `profile.rs:208-248` |
| ChatGPT/Codex | separate path (Responses API) | n/a | n/a | n/a | `if json_mode && tools.is_empty()` | `chatgpt/mod.rs:295` |
| Anthropic | `anthropic/client.rs` | **ignored** (`json_mode: _`) | n/a | n/a | **never** (json_mode discarded) | `anthropic/client.rs:95` |

**Key insight:** NO provider currently gets `response_format` when tools are present. The `!has_tools` gate is universal across ChatCompletions profiles. ChatGPT has the same pattern (`tools.is_empty()`). Anthropic ignores `json_mode` entirely. OpenRouter has `JsonModePolicy::None`.

**`supports_structured_output` defaults:** `true` for Mistral and Generic; `false` for ZAI (overridden per-model), OpenRouter (overridden per-model), OpenCode Go, OpenCode Zen.

**Phase 1 П0.5 verification required:** for each provider, live-probe whether `response_format: {type: "json_object"}` (or `response_format: {type: "json_schema", json_schema: {...}}`) is accepted when `tools` is also present in the request. If accepted → force structured output mode when tools present. If rejected → hard-error + re-request is the class-closing fallback (task fails loudly instead of silently accepting prose).
