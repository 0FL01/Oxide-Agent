# Goal: Remove Chat Mode

Date started: 2026-05-26
Status: active
Codex goal: Implement docs/prd/PRD_remove_chat_mode.md iteratively: remove Telegram Chat Mode/chat-only runtime, chat storage/config/provider surfaces, unsafe route fallbacks, and update validation/docs until agent-only behavior is verified.

## Objective

Implement the agent-only architecture described in `docs/prd/PRD_remove_chat_mode.md`. The stopping condition is a compiling, tested repository where Telegram user input cannot enter a plain Chat Mode/chat history/chat completion path, `CHAT_MODEL_*` and chat-only providers are removed from runtime surface, provider route policy is default-deny for incompatible routes, and docs/scripts/snapshots no longer advertise Chat Mode.

## Scope

In scope:
- Remove Telegram `ChatMode`, prompt editing, mode picker, chat model selection, chat flow attach/detach, and chat-only handlers.
- Route authorized text and supported media only into Agent Mode or explicit media/STT/attachment paths.
- Remove chat history, per-user chat model, per-user prompt, current chat UUID, and scoped chat history storage APIs.
- Remove `CHAT_MODEL_*`, `SYSTEM_MESSAGE`, Groq provider wiring, Groq feature/profile/docs/snapshots, and chat route fallbacks.
- Rename/isolate internal plain text completion as core-only `complete_internal_text` with explicit purpose.
- Harden provider compatibility gates for unknown providers, OpenRouter, NVIDIA, and ChatGPT structured-output restrictions.
- Update tests, scripts, README/env examples, CI/deployment env generation, and registry snapshots.

Out of scope:
- Migrating legacy chat histories, `chat_mode` persisted states, chat UUIDs, or per-user prompts.
- Adding a new Agent Mode prompt editor or preserving prompt editing as an agent feature.
- Adding direct Google Gemini provider support.
- Introducing dynamic provider capability discovery or user-editable compatibility metadata.
- Rewriting unrelated agent runtime semantics beyond what Chat Mode removal requires.

## Repository Context

- PRD: `docs/prd/PRD_remove_chat_mode.md`.
- Telegram state/UX: `crates/oxide-agent-transport-telegram/src/bot/state.rs`, `runner.rs`, `bot/handlers.rs`, `bot/context.rs`, `bot/agent_handlers/controls.rs`.
- Agent media path: `crates/oxide-agent-transport-telegram/src/bot/agent/media.rs`, `bot/agent_handlers/input.rs`, `bot/agent_handlers/task_runner.rs`, `crates/oxide-agent-core/src/agent/providers/media_file.rs`.
- Config/env: `crates/oxide-agent-core/src/config.rs`, `.env.example`, `.github/workflows/ci-cd.yml`, README files, `scripts/check-runtime-env-surface.sh`.
- Storage: `crates/oxide-agent-core/src/storage/`, `crates/oxide-agent-transport-web/src/in_memory_storage.rs`, Telegram storage mocks.
- LLM/internal completion: `crates/oxide-agent-core/src/llm/`, agent compaction, loop detection, wiki writer, and input intent classification.
- Provider policy: `crates/oxide-agent-core/src/llm/capabilities.rs`, `llm/providers/`, `crates/oxide-agent-core/src/capabilities/`, `profiles/*.toml`, registry snapshots.

## Implementation Plan

1. Goal and inventory checkpoint: create this document, confirm active goal, and capture the first implementation slice.
2. Telegram state/UX checkpoint: remove `State::ChatMode` and `State::EditingPrompt`, mode picker/menu/model/prompt callbacks, chat flow callbacks, and make `/start` agent-only.
3. Telegram input checkpoint: route text/voice/media/documents to Agent Mode or explicit unsupported responses; delete `process_llm_request()` and chat media responses.
4. Storage checkpoint: remove chat history, per-user prompt/model, current chat UUID, scoped chat history keys, and update in-memory/mocks/tests.
5. Config checkpoint: remove `CHAT_MODEL_*`, `SYSTEM_MESSAGE`, chat defaults, chat model helpers, and require explicit agent/media/internal routes.
6. LLM/internal completion checkpoint: expose no transport-callable plain completion; add core-only `complete_internal_text` with `InternalTextPurpose`.
7. Provider policy checkpoint: remove Groq, default-deny unknown provider capabilities, add OpenRouter/NVIDIA allowlists and alias-safe ChatGPT JSON restrictions.
8. Docs/scripts/snapshots checkpoint: update env examples, workflows, README/docs, capability/profile checks, and registry snapshots.
9. Validation checkpoint: run formatting, focused tests, cargo checks/clippy, grep guards, and document any remaining blocked checks.

## Validation Contract

- Formatting: `cargo fmt`.
- Lint: `cargo clippy --workspace --no-default-features --features profile-lite` and broader profile clippy when the tree is compiling.
- Focused checks:
  - `cargo check --workspace --no-default-features --features profile-lite`
  - `cargo test -p oxide-agent-transport-telegram --no-default-features --features storage-s3-r2`
  - `cargo test -p oxide-agent-core --no-default-features --features profile-lite`
  - `scripts/check-runtime-env-surface.sh`
  - `scripts/check-compiled-capabilities.sh full`
  - `scripts/check-registry-snapshots.sh full`
- Grep guards:
  - `rg -n "ChatMode|chat_mode" crates/oxide-agent-transport-telegram/src` must find no live runtime references.
  - `rg -n "process_llm_request|CHAT_MODEL|chat_model|GROQ|llm-groq|llm-provider/groq"` must find no live runtime/config/docs references except intentional PRD/goal history while the corresponding checkpoint is complete.
  - `rg -n "chat_completion" crates/oxide-agent-transport-telegram crates/oxide-agent-transport-web` must find no transport-callable user path.
- Done when: all PRD functional requirements are implemented or remaining gaps are explicitly documented with evidence and user acceptance.

## Decisions

- 2026-05-26: Follow PRD fresh-DB decision. Do not add compatibility branches, migrations, aliases, or tests for legacy `chat_mode` and `EditingPrompt` persisted values.
- 2026-05-26: Start with Telegram state/UX before storage/config/provider deletion because it closes the highest-risk user-facing plain chat entry points first.
- 2026-05-26: Keep ChatGPT provider in scope as an agent-compatible provider; do not remove it due to Chat Mode naming overlap.

## Progress Log

- 2026-05-26 21:05 +03: Read `docs/prd/PRD_remove_chat_mode.md`, existing goal-doc convention, active goal state, Telegram state/router entry points, and created this goal contract. Next checkpoint: remove Chat Mode and prompt editing state/router branches, then simplify `/start` and menu handling toward agent-only.
- 2026-05-26 21:17 +03: Completed first Telegram state/UX checkpoint. Removed `State::ChatMode`, `State::EditingPrompt`, runner branches, chat flow callbacks, prompt editing handlers, chat model menu handlers, `process_llm_request()`, and transport-level direct media chat responses. `/start`, text, voice, photo, video, and document handlers now force authorized input into `agent_mode`/Agent Mode paths or return access guidance; agent exit now cancels/resets back to Agent Mode instead of writing `chat_mode`. Verified `cargo check -p oxide-agent-transport-telegram --no-default-features --features storage-s3-r2`, `cargo test -p oxide-agent-transport-telegram --no-default-features --features storage-s3-r2 --lib`, and `rg -n "ChatMode|EditingPrompt|chat_mode|process_llm_request|handle_editing_prompt|activate_chat_mode|pick_system_prompt|resolve_system_prompt|chat_flow|CHAT_ATTACH|MENU_CALLBACK_CHAT|MENU_CALLBACK_EDIT|MENU_CALLBACK_MODEL|scoped_chat_storage_id|ensure_current_chat_uuid|reset_current_chat_uuid|Chat Mode" crates/oxide-agent-transport-telegram/src` with no matches. Next checkpoint: remove storage trait methods and web/Telegram mocks for per-user prompt/model and chat history, then delete `current_chat_uuid` from `UserConfig`/`UserContextConfig`.

## Risks and Blockers

- This is a broad cross-crate removal and will likely pass through temporarily uncompiling intermediate states. Mitigation: keep checkpoints narrow and update this log with exact compile errors and next fixes.
- Storage trait removal will fan out into many mocks and tests. Mitigation: do it after Telegram runtime no longer calls chat APIs, then remove trait methods and fix compile-driven callers.
- Provider capability changes can invalidate snapshots and profiles. Mitigation: update registry checks only after code policy is complete, not before.

## Final Verification

- Pending.
