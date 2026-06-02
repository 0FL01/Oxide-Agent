# PRD: удаление Chat Mode и chat-only runtime

## 1. Executive Summary

Цель работ — перевести проект в agent-only режим и удалить Chat Mode как пользовательский режим, runtime path, конфигурационный слой, storage surface, provider surface и документацию. Удаление должно быть жёстким: без миграций, без deprecated aliases, без скрытого fallback в plain chat completion и без поддержки старых `CHAT_MODEL_*` настроек.

Остаётся Agent Mode / agent runtime / tool-capable providers. ChatGPT provider не считается Chat Mode: его нужно сохранить, если он остаётся agent-compatible provider с `chat_with_tools` и корректной capability policy.

Desired end-state:

- пользовательский текст, команды и мультимодальные входы из Telegram больше не могут попасть в обычный `chat_completion` path;
- голосовые сообщения обрабатываются только через явный STT/media route (`MEDIA_MODEL_*` с поддержкой audio transcription; например OpenRouter Gemini-family route или Mistral/Voxtral route). Если media route не настроен или не поддерживает audio, Telegram возвращает явный отказ без agent/chat fallback;
- фото, видео, аудио-файлы и документы скачиваются только в agent sandbox / per-run upload area при включённой media/file capability, передаются агенту как attachment descriptors, а анализ выполняется через explicit agent tools (`describe_image_file`, `describe_video_file`, `transcribe_audio_file`) или agent preprocessor. При отсутствии media capability возвращается явный unsupported ответ;
- состояние `ChatMode` отсутствует в state machine и persisted state не восстанавливается как рабочий режим;
- `CHAT_MODEL_*`, per-user chat model selection, chat history и chat flow UUID удалены из runtime surface;
- provider route selection допускает только agent-compatible routes;
- Groq удалён, потому что в текущем коде он зарегистрирован как `llm-provider/groq` с `supports_tool_calling=false` и реализует только plain chat completion;
- OpenRouter и NVIDIA NIM допускаются только через явные model-level / route-level gates;
- внутренние plain-text completion задачи, если они нужны агенту, переименованы и изолированы как internal-only API, недоступный transport/user layer.

Точно не делаем:

- не сохраняем обратную совместимость с `CHAT_MODEL_*`;
- не мигрируем старые chat histories;
- не оставляем hidden fallback в Chat Mode;
- не оставляем legacy wrappers/aliases ради старых chat-настроек;
- не переписываем весь проект вне области удаления Chat Mode.

## 2. Problem Statement

В текущем репозитории Chat Mode существует не как один UI label, а как сквозной второй runtime path рядом с Agent Mode. Это создаёт архитектурную неоднозначность: пользовательское сообщение может быть обработано либо агентным loop с tool calling, либо обычным `chat_completion` без tools, памяти агента и route-policy гарантий.

Проблемы, обнаруженные в recon:

- Telegram transport содержит отдельное состояние `State::ChatMode`, отдельные menus/callbacks, model switcher, prompt editing и chat flow controls.
- `/start` для части сценариев выставляет persisted state `chat_mode`, хотя продукт должен стать agent-only.
- `process_llm_request` в Telegram transport напрямую грузит chat history, выбирает per-user chat model и вызывает `LlmClient::chat_completion`.
- `AgentSettings` жёстко требует `CHAT_MODEL_ID` и `CHAT_MODEL_PROVIDER` в `AgentSettings::new()`, а Agent route fallback может падать обратно на chat model.
- Storage trait и реализации содержат chat history, scoped chat history, per-user prompt/model и `current_chat_uuid`.
- Provider registry содержит Groq как compiled provider feature `llm-groq`, но capabilities показывают `supports_tool_calling=false`.
- Capability fallback сейчас default-allow: неизвестный provider получает `supports_tool_calling=true` через `default_provider_capabilities()`. Для agent-only это небезопасно.
- Docs, env examples, workflows, profiles, scripts и snapshots всё ещё рекламируют Chat Mode, `CHAT_MODEL_*` и Groq.

Agent-only архитектура должна быть однозначной: пользовательский ввод либо идёт в Agent Mode, либо получает понятный отказ/инструкцию, но никогда не уходит в plain chat completion.

## 3. Goals

- Полностью удалить Chat Mode из state machine, Telegram UX, callbacks, storage, config, docs, tests и snapshots.
- Удалить chat-only providers и все связанные feature flags, env vars, capability registry entries, profiles и tests.
- Удалить `CHAT_MODEL_*` конфигурацию, validation и fallback-и на chat model.
- Удалить per-user chat model selection и model keyboard.
- Удалить chat history storage APIs, chat UUID и scoped chat histories.
- Перевести `/start`, текст, media и документы в agent-only flow.
- Зафиксировать explicit modality contract: voice → STT/media route → normal Agent Mode text input; missing media/STT route → clear reject.
- Зафиксировать media/file contract: photo/video/audio/document → sandbox attachment + agent tool/preprocessor; missing media capability/feature → clear reject.
- Усилить provider compatibility gates: provider/model нельзя считать agent-compatible только потому, что он умеет обычный chat completion.
- Сделать route selection безопасным: incompatible routes должны исключаться до execution attempt, включая failover.
- OpenRouter сделать default-deny без explicit model/route compatibility.
- NVIDIA NIM проверять по конкретной модели до запуска agent loop.
- Сохранить ChatGPT provider, если он продолжает работать как agent-compatible provider.
- Обновить tests/docs/profiles/scripts/snapshots так, чтобы CI проверял отсутствие Chat Mode и chat-only runtime.

## 4. Non-Goals

- Не делать миграции старых persisted chat histories и `current_chat_uuid`.
- Не сохранять обратную совместимость с `CHAT_MODEL_ID`, `CHAT_MODEL_PROVIDER`, `CHAT_MODEL_NAME`, `CHAT_MODEL_MAX_OUTPUT_TOKENS`, `CHAT_MODEL_CONTEXT_WINDOW_TOKENS`.
- Не оставлять hidden chat fallback для пользователей без agent access.
- Не оставлять deprecated wrappers вокруг chat storage/config/model selection.
- Не поддерживать старые chat histories как first-class runtime data.
- Не считать provider совместимым “на веру” без фактической capability evidence.
- Не менять Agent Mode semantics сверх необходимого для удаления Chat Mode и unsafe route fallback.
- Не переписывать весь проект ради эстетической чистоты, если участок не связан с удалением Chat Mode, chat-only provider или unsafe capability gate.
- Не добавлять новые provider integrations, если они не нужны для замены удалённого chat-only route.
- Не вводить dual-mode runtime.

- **Fresh DB:** деплой выполняется на пустом storage. Legacy `chat_mode` state-записи, старые chat histories, `current_chat_uuid`, per-user chat model и prompt editing state физически отсутствуют и не поддерживаются как runtime input.
- Не добавлять read-path compatibility, parser branch, migration job или тесты совместимости для legacy persisted-state literals вроде `chat_mode` / `EditingPrompt`.
- Unknown/invalid persisted state values обрабатываются generic-путём как `None` и проходят через agent-only access/configuration flow.
- Не переносить Chat Mode prompt editor в Agent Mode.
- Не добавлять новый Telegram UX для редактирования agent/system prompt.
- Не мигрировать старые per-user prompts и не сохранять `user_prompt` как hidden compatibility layer.
- Не оставлять `State::EditingPrompt`, `MenuCallbackData::EditPrompt` или любую другую форму prompt editing как agent feature в рамках этой задачи.

## 5. Current Architecture Recon

Recon выполнялся по загруженному репозиторию. Ниже перечислены фактические места, которые нужно затронуть при реализации.

### 5.1 Recon search inventory

Ключевые `rg`-результаты на текущем дереве:

- `chat_mode`: 112 hits / 12 files.
- `ChatMode`: 10 hits / 3 files.
- `Chat Mode`: 8 hits / 3 files.
- `CHAT_MODEL`: 53 hits / 4 files.
- `chat_model`: 96 hits / 9 files.
- `chat_completion`: 80 hits / 29 files.
- `process_llm_request`: 3 hits / 1 file.
- `Groq`: 26 hits / 12 files.
- `GROQ`: 14 hits / 8 files.
- `llm-groq`: 15 hits / 8 files.
- `llm-provider/groq`: 29 hits / 10 files.
- `get_chat_history`: 27 hits / 11 files.
- `save_message_for_chat`: 19 hits / 11 files.
- `clear_chat_history`: 35 hits / 12 files.
- `current_chat_uuid`: 35 hits / 7 files.
- `user_prompt`: 37 hits / 12 files.
- `user_model`: 34 hits / 11 files.

### 5.2 Telegram/menu/state chat layer

Key files:

- `crates/oxide-agent-transport-telegram/src/bot/state.rs`
- `crates/oxide-agent-transport-telegram/src/runner.rs`
- `crates/oxide-agent-transport-telegram/src/bot/handlers.rs`
- `crates/oxide-agent-transport-telegram/src/bot/context.rs`
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/controls.rs`

Findings:

- `bot/state.rs:16-27` defines `State::{Start, EditingPrompt, AgentMode, ChatMode, AgentConfirmation}`. `ChatMode` is explicitly documented as “Normal chat mode with management buttons”.
- `runner.rs:144-196` has separate `State::Start` and `State::ChatMode` branches. Both route text/voice/photo/video/document to the same start handlers, which then branch into chat runtime.
- `handlers.rs:32-41` defines chat/menu callback constants: `CHAT_ATTACH_PREFIX`, `CHAT_DETACH_CALLBACK`, `MENU_CALLBACK_CHAT_MODE`, `MENU_CALLBACK_CHANGE_MODEL`, `MENU_CALLBACK_EXTRA_FUNCTIONS`, `MENU_CALLBACK_EDIT_PROMPT`, `MENU_CALLBACK_MODEL_PREFIX`.
- `handlers.rs:58-65` defines `resolve_chat_model()`, which prefers `storage.get_user_model()` and falls back to `settings.agent.get_default_chat_model_name()`.
- `handlers.rs:153-195` restores persisted `agent_mode` or `chat_mode`. When persisted state is `chat_mode`, it updates dialogue to `State::ChatMode` and allows chat handling to continue.
- `handlers.rs:197-206` describes `/clear` as “Clear chat history”.
- `handlers.rs:225-239` builds main keyboard / inline keyboard with both “Agent Mode” and “Chat Mode”.
- `handlers.rs:241-330` builds chat menu, extra functions menu and model selection keyboard from `settings.agent.get_chat_models()`.
- `handlers.rs:332-342` defines `MenuCallbackData::ChatMode`, `ChangeModel`, `ExtraFunctions`, `EditPrompt`, `Model(usize)`.
- `handlers.rs:385-479` implements `/start`. For supergroups with agent access it defaults to Agent Mode. Otherwise it resets to `State::Start`, persists `Some("chat_mode")`, loads `storage.get_user_model()`, resolves a chat model and sends welcome text mentioning Chat Mode.
- `handlers.rs:486-506` implements `clear()` by resetting scoped chat UUID and returning chat menu.
- `handlers.rs:509-535` builds chat flow Attach/Detach controls.
- `handlers.rs:611-684` handles chat flow callbacks only when persisted state is `chat_mode`, mutating `current_chat_uuid`.
- `handlers.rs:689-792` handles Chat Mode activation, Change Model, Extra Functions, Edit Prompt and per-user model update.
- `handlers.rs:857-943` routes user text to `process_llm_request()` only when persisted state is `chat_mode`; otherwise it shows “Please select a mode”.
- `handlers.rs:945-1030` handles text menu commands including “💬 Chat Mode”, “Change Model”, “Extra Functions”, “Edit Prompt”, “Back”.
- `handlers.rs:1032-1070` defines `activate_chat_mode()`: creates chat UUID, sets context state `chat_mode`, updates dialogue to `State::ChatMode` and shows “Chat mode activated.”
- `handlers.rs:1195-1233` stores user-edited prompt with `storage.update_user_prompt()` and returns to `State::ChatMode`.
- `handlers.rs:1235-1326` defines `process_llm_request()`: loads scoped chat history, gets per-user chat model, saves user message, calls `llm.chat_completion(...)`, saves assistant response and sends chat flow controls.
- `handlers.rs:1336-1446` handles voice only in `chat_mode`, transcribes audio through media route and then calls `process_llm_request()`.
- `handlers.rs:1453-1574` handles photo only in `chat_mode`, calls `llm.analyze_image(...)`, saves `[Image]` and assistant response to chat history and sends chat flow controls.
- `handlers.rs:1581-1696` handles video with the same chat-mode pattern.
- `handlers.rs:1704-1750` routes documents to Agent Mode only if persisted state is `agent_mode`; otherwise it returns “File upload is available only in Agent Mode”.
- `handlers.rs:1753-1777` selects system prompt from topic override, user prompt, or `SYSTEM_MESSAGE`; user prompt is tied to chat prompt editing and must be reviewed before removal.
- `bot/context.rs:33-36` defines `scoped_chat_storage_id(context_key, chat_uuid)`.
- `bot/context.rs:103-159` manages `ensure_current_chat_uuid()` / `reset_current_chat_uuid()`, persisted globally for DM and context-scoped for topics.
- `bot/context.rs:161-214` manages agent flow IDs but reuses `generate_chat_uuid()`. If chat UUID is removed, this generator should be renamed to a generic flow/run ID generator.
- `agent_handlers/controls.rs:637-675` exits Agent Mode by setting persisted state to `Some("chat_mode")`, updates dialogue to `State::Start` and asks the user to select a working mode. In agent-only target this must not fall back to chat.

### 5.3 Config/env chat layer

Key files:

- `crates/oxide-agent-core/src/config.rs`
- `.env.example`
- `README.md`
- `.github/workflows/ci-cd.yml`
- `scripts/check-runtime-env-surface.sh`

Findings:

- `config.rs:16-40` defines chat temperature constants, including `GROQ_CHAT_TEMPERATURE`, `OPENROUTER_CHAT_TEMPERATURE`, `NVIDIA_CHAT_TEMPERATURE`, `MINIMAX_CHAT_TEMPERATURE`, `OPENCODE_GO_CHAT_TEMPERATURE`.
- `config.rs:88-98` defines `AgentSettings` fields for `chat_model_id`, `chat_model_name`, `chat_model_provider`, `chat_model_max_output_tokens`, `chat_model_context_window_tokens`.
- `config.rs:353-386` in `AgentSettings::new()` hard-requires `CHAT_MODEL_ID` and `CHAT_MODEL_PROVIDER` before route validation.
- `config.rs:400-424` validates `CHAT_MODEL_PROVIDER` as a configured route provider.
- `config.rs:489-493` canonicalizes `CHAT_MODEL_PROVIDER`.
- `config.rs:579-600` includes `self.chat_model_provider` in configured route provider values.
- `config.rs:767-782` builds `chat_model_spec()` from chat model settings.
- `config.rs:830-844` builds `media_model_spec()` using `chat_model_max_output_tokens` and `chat_model_context_window_tokens`; this couples media defaults to removed chat config and must be fixed.
- `config.rs:862-871` exposes `get_chat_models()`.
- `config.rs:873-903` includes `chat_model_spec()` in `get_available_models()`.
- `config.rs:905-911` exposes `get_default_chat_model_name()`.
- `config.rs:913-926` falls back from sub-agent to agent to chat model and then default model in `resolve_execution_model()`.
- `config.rs:930-992` returns configured agent/sub-agent routes; empty agent routes currently fall back through `resolve_execution_model(false)`, which can pick chat model.
- `config.rs:1083-1088` implements `get_model_info_by_name()` by searching only `get_chat_models()`; used by Telegram chat model selection.
- `config.rs:1883-1890` `get_agent_model()` falls back to `CHAT_MODEL_ID`.
- `config.rs:1904-1907` defines `DEFAULT_CHAT_MODEL_MAX_OUTPUT_TOKENS` and `DEFAULT_CHAT_MODEL_CONTEXT_WINDOW_TOKENS`.
- `.env.example` contains `GROQ_API_KEY`, Chat Mode wording and `CHAT_MODEL_*` examples.
- `README.md` says Groq is supported in Chat Mode only, describes Chat/Agent providers, recommends `CHAT_MODEL_*` for multimodal routes and says chat mode keyboard lists configured names.
- `.github/workflows/ci-cd.yml` exports and writes `GROQ_API_KEY` and `CHAT_MODEL_*` into CI/deployment `.env`.
- `scripts/check-runtime-env-surface.sh` currently forbids only legacy `CHAT_MODEL_MAX_TOKENS`, not the entire removed `CHAT_MODEL_*` family.

### 5.4 Storage chat layer

Key files:

- `crates/oxide-agent-core/src/storage/provider.rs`
- `crates/oxide-agent-core/src/storage/user.rs`
- `crates/oxide-agent-core/src/storage/keys.rs`
- `crates/oxide-agent-core/src/storage/r2_user.rs`
- `crates/oxide-agent-core/src/storage/r2_provider.rs`
- `crates/oxide-agent-core/src/storage/telemetry.rs`
- `crates/oxide-agent-transport-web/src/in_memory_storage.rs`
- Telegram test mocks in `crates/oxide-agent-transport-telegram/src/bot/topic_route.rs`, `bot/agent_handlers/tests.rs`, `tests/topic_routing_thread_integration.rs`

Findings:

- `storage/provider.rs:23-35` defines `update_user_prompt`, `get_user_prompt`, `update_user_model`, `get_user_model`.
- `storage/provider.rs:40-90` defines chat history APIs: `save_message`, `get_chat_history`, `clear_chat_history`, `save_message_for_chat`, `get_chat_history_for_chat`, `clear_chat_history_for_chat`, `clear_chat_history_for_context`.
- `storage/user.rs:6-18` contains `UserConfig.system_prompt`, `model_name`, `state`, `current_chat_uuid`, `contexts`.
- `storage/user.rs:22-29` contains `UserContextConfig.state`, `current_chat_uuid`, `current_agent_flow_id`.
- `storage/user.rs:50-57` defines generic `Message { role, content }` used by chat history.
- `storage/keys.rs:22-38` defines `user_history_key`, `user_chat_history_key`, `user_context_chat_history_prefix`.
- `storage/keys.rs` exports `generate_chat_uuid()`, which is also reused for agent flow IDs via `bot/context.rs:178` and `bot/context.rs:212`.
- `r2_user.rs` implements prompt/model and chat history inner methods.
- `r2_provider.rs` exposes these methods on `StorageProvider`; `r2_provider.rs:321` clears chat history inside broader context clearing.
- `storage/telemetry.rs` classifies `history.json` under `/chats/` as chat history / user chat history.
- `transport-web/src/in_memory_storage.rs` keeps `user_prompts`, `user_models`, `chat_histories`, `chat_histories_by_chat` and implements chat storage methods for tests.

### 5.5 LLM trait / chat completion layer

Key files:

- `crates/oxide-agent-core/src/llm/provider.rs`
- `crates/oxide-agent-core/src/llm/client.rs`
- `crates/oxide-agent-core/src/llm/support/openai_compat.rs`
- provider implementations under `crates/oxide-agent-core/src/llm/providers/*`
- internal agent modules using plain completion:
  - `crates/oxide-agent-core/src/agent/compaction/local_llm_summary.rs`
  - `crates/oxide-agent-core/src/agent/loop_detection/llm_detector.rs`
  - `crates/oxide-agent-core/src/agent/loop_detection/service.rs`
  - `crates/oxide-agent-core/src/agent/executor/execution.rs`
  - `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/input_intent.rs`

Findings:

- `llm/provider.rs:6-15` requires every `LlmProvider` to implement `chat_completion`.
- `llm/provider.rs:65-76` has optional `chat_with_tools()` defaulting to “Tool calling not supported by this provider”.
- `llm/client.rs:12-25` stores `models`, `chat_model_name`, `media_model_name`, `media_model_id`, `media_model_provider`.
- `llm/client.rs:32-69` resolves media routes by preferring `MEDIA_MODEL_*`, then falling back to `chat_model_name` if it supports the modality.
- `llm/client.rs:71-108` documentation explicitly says media routes fall back to chat model.
- `llm/client.rs:158-178` initializes `chat_model_name` from `settings.get_default_chat_model_name()` and `models` from `settings.get_available_models()`.
- `llm/client.rs:219-236` defines public `chat_completion()` by model name.
- `llm/client.rs:238-296` defines public `chat_completion_for_model_info()` by explicit route.
- `llm/client.rs:324-368` and `396-507` execute `chat_with_tools` with capability checks and tool history validation.
- `agent/compaction/local_llm_summary.rs:59-64` uses `chat_completion_for_model_info()` for local compaction summary.
- `agent/loop_detection/llm_detector.rs:161-165` uses `chat_completion()` for loop detection scout model.
- `agent/executor/execution.rs:613-616` uses `chat_completion_for_model_info()` for background Wiki Memory writer.
- `bot/agent_handlers/input_intent.rs:204-207` uses `chat_completion_for_model_info()` for Agent Mode input classifier.

These internal uses are not Chat Mode UX, but their API names and visibility create ambiguity. They require internal-only rename/isolation rather than blind deletion.

### 5.6 Provider modules and capabilities

Key files:

- `crates/oxide-agent-core/src/llm/capabilities.rs`
- `crates/oxide-agent-core/src/llm/providers/mod.rs`
- `crates/oxide-agent-core/src/llm/providers/modules.rs`
- `crates/oxide-agent-core/src/llm/providers/chatgpt/module.rs`
- `crates/oxide-agent-core/src/llm/providers/groq.rs`
- `crates/oxide-agent-core/src/llm/providers/groq/module.rs`
- `crates/oxide-agent-core/src/llm/providers/mistral/module.rs`
- `crates/oxide-agent-core/src/llm/providers/minimax/module.rs`
- `crates/oxide-agent-core/src/llm/providers/nvidia.rs`
- `crates/oxide-agent-core/src/llm/providers/nvidia/module.rs`
- `crates/oxide-agent-core/src/llm/providers/opencode_go/module.rs`
- `crates/oxide-agent-core/src/llm/providers/openrouter/module.rs`
- `crates/oxide-agent-core/src/llm/providers/zai/module.rs`
- `crates/oxide-agent-core/src/capabilities/compiled.rs`
- `crates/oxide-agent-core/Cargo.toml`
- `profiles/full.toml`
- `scripts/check-compiled-capabilities.sh`
- `crates/oxide-agent-core/tests/snapshots/*`

Findings:

- `llm/capabilities.rs:52-70` defines `ProviderCapabilities { tool_history_mode, supports_tool_calling, supports_structured_output }`.
- `llm/capabilities.rs:102-106` defines agent tool compatibility as `supports_tool_calling`.
- `llm/capabilities.rs:108-119` allows `chat_with_tools` style requests without tools when either tool calling is supported or JSON mode is supported.
- `llm/capabilities.rs:128-155` falls back to `default_provider_capabilities()`, currently `BestEffort, supports_tool_calling=true, supports_structured_output=true`. This is unsafe default-allow.
- `providers/mod.rs:1-6` includes `pub mod groq` behind `llm-groq`.
- `providers/mod.rs:82-85` exports `GroqProvider` behind `llm-groq`.
- `providers/modules.rs:182-203` pushes `GroqProviderModule` into `compiled_provider_modules()` when `llm-groq` is enabled.
- `providers/modules.rs:405-426` has tests proving Groq registers aliases and base capabilities with `supports_tool_calling=false`, `supports_structured_output=true`.
- `chatgpt/module.rs:15-49` maps provider id `llm-provider/openai-chatgpt`, aliases `chatgpt`, `openai-chatgpt`, env `CHATGPT_AUTH_PATH`, capabilities `BestEffort, supports_tool_calling=true, supports_structured_output=false`.
- `groq/module.rs:14-35` maps provider id `llm-provider/groq`, alias `groq`, env `GROQ_API_KEY`, capabilities `BestEffort, supports_tool_calling=false, supports_structured_output=true`.
- `groq.rs:28-68` implements `chat_completion`, `transcribe_audio` as not implemented and `analyze_image` as not implemented. It does not override `chat_with_tools()`.
- `mistral/module.rs:14-45` maps `llm-provider/mistral`, alias `mistral`, env `MISTRAL_API_KEY`, capabilities `Strict, supports_tool_calling=true, supports_structured_output=true`, audio transcription media support.
- `minimax/module.rs:14-35` maps `llm-provider/minimax`, alias `minimax`, env `MINIMAX_API_KEY`, capabilities `Strict, supports_tool_calling=true, supports_structured_output=false`.
- `nvidia/module.rs:17-58` maps `llm-provider/nvidia`, alias `nvidia`, envs `NVIDIA_API_KEY`/`NVIDIA_API_BASE`, base capabilities true but `capabilities_for_model()` delegates to `nvidia::model_capabilities()`.
- `nvidia.rs:53-121` contains explicit model allowlist/wildcard logic for tool calling and structured output.
- `nvidia.rs:440-459` rejects `chat_with_tools` at provider call time when model capabilities do not support tool calling.
- `opencode_go/module.rs:17-66` maps `llm-provider/opencode-go`, aliases `opencode-go`, `opencode_go`, capabilities `Strict, supports_tool_calling=true`, model-specific structured output only for `deepseek-v4-flash` / `deepseek-v4-pro`.
- `openrouter/module.rs:14-44` maps `llm-provider/openrouter`, alias `openrouter`, env `OPENROUTER_API_KEY`, capabilities currently `BestEffort, supports_tool_calling=true, supports_structured_output=false`, media all true. For agent-only this is too broad without model-level gating.
- `zai/module.rs:17-63` maps `llm-provider/zai`, alias `zai`, envs `ZAI_API_KEY`/`ZAI_API_BASE`, capabilities `BestEffort, supports_tool_calling=true`, structured output by model id allowlist.
- `capabilities/compiled.rs:123-126` defines Groq config property with `GROQ_API_KEY`.
- `capabilities/compiled.rs:277-281` includes `llm-groq` / `llm-provider/groq` in compiled capability manifest.
- `Cargo.toml:79-90` includes `llm-groq` in `profile-full`.
- `Cargo.toml:212-220` defines atomic feature `llm-groq = ["dep:async-openai"]`.
- `profiles/full.toml:8` enables `llm-provider/groq`.
- `scripts/check-compiled-capabilities.sh:363-375` requires `llm-provider/groq` in full profile.
- Snapshot files list Groq in `modular_registry_snapshot@profile-full.snap` and `@all-features.snap`.

### 5.7 Agent route selection and failover

Key files:

- `crates/oxide-agent-core/src/agent/runner/execution.rs`
- `crates/oxide-agent-core/src/agent/runner/types.rs`
- `crates/oxide-agent-core/src/config.rs`
- `crates/oxide-agent-core/src/llm/client.rs`

Findings:

- `agent/runner/types.rs:37-38` stores optional weighted model routes in `AgentRunnerConfig`.
- `execution.rs:213-220` uses single-route path when `ctx.config.model_routes` is empty, otherwise failover path.
- `execution.rs:241-273` single-route path checks structured-output and tool-capability before calling LLM.
- `execution.rs:329-390` failover path selects a route, logs it, checks `can_run_chat_with_tools_request()`, then skips unsupported routes before calling provider.
- `execution.rs:654-741` `select_model_route_index()` checks quarantine, provider availability, JSON mode and v1 tool runtime route constraints, but does not check `supports_tool_calling` / model-level compatibility in `route_is_available()`.
- `execution.rs:743-745` blocks JSON mode for provider string equal to `chatgpt`; alias/canonicalization must be checked because canonical ChatGPT provider id is `llm-provider/openai-chatgpt`.
- `execution.rs:2983-3065` has regression test proving unsupported NVIDIA route is skipped and backup route used.

The current failover path skips unsupported NVIDIA before provider call, but selection can still choose unsupported candidates. Target policy should make route availability itself capability-aware and default-deny unknown/unsupported routes.

### 5.8 Docs/examples/profiles/tests/snapshots

Key files:

- `.env.example`
- `README.md`
- `AGENTS.md`
- `.github/workflows/ci-cd.yml`
- `.github/workflows/modular-architecture.yml`
- `profiles/full.toml`
- `scripts/check-runtime-env-surface.sh`
- `scripts/check-compiled-capabilities.sh`
- `scripts/check-registry-snapshots.sh`
- `crates/oxide-agent-core/tests/modular_registry_snapshots.rs`
- `crates/oxide-agent-core/tests/snapshots/modular_registry_snapshots__*.snap`
- `crates/oxide-agent-telegram-bot/tests/integration_validation.rs`
- `crates/oxide-agent-core/tests/*` mocks implementing `chat_completion`
- `crates/oxide-agent-transport-telegram/src/bot/handlers.rs` tests
- `crates/oxide-agent-transport-telegram/src/bot/context.rs` tests
- `crates/oxide-agent-transport-telegram/src/bot/topic_route.rs` mocks
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/tests.rs` mocks
- `crates/oxide-agent-transport-web/src/in_memory_storage.rs`
- `crates/oxide-agent-transport-web/src/scripted_llm.rs`

Findings:

- README advertises Chat/Agent provider mix and states Groq is Chat Mode only.
- `.env.example` exposes `GROQ_API_KEY` and `CHAT_MODEL_*`.
- CI deploy env writes `CHAT_MODEL_*` and `GROQ_API_KEY` into `.env`.
- Snapshot tests encode Groq and full profile expectations.
- Many test mocks implement `StorageProvider` chat APIs because trait currently requires them.
- Several provider/test mocks implement `chat_completion` because `LlmProvider` currently requires it.

### 5.9 Decisions and implementation notes

Эти пункты больше не являются открытыми product decisions. Они оставлены здесь как implementation notes, чтобы исполнитель не переоткрыл уже принятые решения.

- **Internal plain completion API:** resolved in DR-002. Keep auxiliary text completion only as crate-private `complete_internal_text` with mandatory `InternalTextPurpose`, purpose-based routing, no chat context and no transport visibility.
- **Per-user prompt:** resolved in DR-001. Delete `UserConfig.system_prompt`, `update_user_prompt`, `get_user_prompt`, `State::EditingPrompt`, `EditPrompt` callbacks, `pick_system_prompt()`, `resolve_system_prompt()` and `SYSTEM_MESSAGE` fallback if they are only Chat Mode surfaces.
- **Existing agent media surface:** repo already has agent-side media primitives: `crates/oxide-agent-core/src/agent/preprocessor.rs`, `crates/oxide-agent-transport-telegram/src/bot/agent/media.rs`, `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/input.rs`, `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/task_runner.rs` and sandbox media tools in `crates/oxide-agent-core/src/agent/providers/media_file.rs` (`transcribe_audio_file`, `describe_image_file`, `describe_video_file`). Reuse these instead of preserving Chat Mode media handlers.
- **Media route semantics:** resolved. Voice is transcribed through explicit `MEDIA_MODEL_*` STT route and then injected into Agent Mode as text. Photo/video/audio/document are preserved in sandbox as agent attachments when media/file capability is enabled, and the agent may call media tools with a prompt. If required route/capability is absent, reject clearly. No chat fallback is allowed.
- **Direct Gemini provider:** resolved. Do not add direct `llm-provider/gemini`. Gemini-family media/STT means OpenRouter model IDs such as `google/gemini-*` routed through `llm-provider/openrouter`, unless a separate provider-integration PR changes repo policy.
- **Media/internal defaults:** resolved in DR-009. Delete chat-named defaults and introduce media/internal constants with non-chat ownership.
- **OpenRouter compatibility source:** resolved in DR-003. Use a small static in-code allowlist with separate observed/provider-declared capabilities and product-approved usage flags. Unknown OpenRouter model ids are default-denied.
- **NVIDIA allowlist ownership:** resolved in DR-004. Use a code-owned exact-match allowlist for the initial approved NVIDIA Agent Mode text/tool models. No config overrides, wildcards or runtime metadata discovery in this refactor.
- **ChatGPT alias safety:** resolved. Do not do a broad identity refactor; add a small local helper such as `is_chatgpt_provider_id(&str)` covering `chatgpt`, `openai-chatgpt`, and `llm-provider/openai-chatgpt` wherever route policy enforces ChatGPT structured-output / JSON restrictions.
- **Old persisted state:** resolved in DR-005. Fresh DB only. Do not implement legacy `chat_mode` compatibility. Unknown/invalid state values use generic `None` fallback through agent-only access/configuration flow.

### 5.10 Resolved Decisions

#### DR-001: Per-user prompt (`UserConfig.system_prompt`) — delete completely

Status: resolved.

Decision: delete the per-user prompt editing surface completely. This includes `UserConfig.system_prompt`, `update_user_prompt`, `get_user_prompt`, `State::EditingPrompt`, `MenuCallbackData::EditPrompt`, `MENU_CALLBACK_EDIT_PROMPT`, `handle_editing_prompt()`, `begin_prompt_editing()`, `pick_system_prompt()`, `resolve_system_prompt()` and any Telegram UX that treats the next user message as a new prompt value.

Keep only existing Agent Mode prompt surfaces that are already agent-owned and independent from Chat Mode: topic-level prompt, profile prompt instructions, topic context tools and Topic `AGENTS.md`.

Rationale: per-user prompt editing is Chat Mode legacy. Agent prompt editing would be a new product feature and is out of scope for this deletion PRD.

#### DR-002: Internal text completion — keep as crate-private `complete_internal_text`

Status: resolved.

Decision: auxiliary LLM tasks remain, but not as public/user-facing chat completion. Rename project-level plain completion to `complete_internal_text`, make it crate-private inside `oxide-agent-core`, require an `InternalTextPurpose` enum, and restrict request shape so it cannot carry `chat_id`, Telegram user id, chat history, stored user prompt, reply markup or chat flow metadata.

Allowed purposes:

- `CompactionSummary`
- `LoopDetection`
- `WikiMemoryWriter`
- `InputIntentClassification`

Allowed callers:

- `agent/compaction/*`
- `agent/loop_detection/*`
- `agent/executor/*` for wiki writer
- `agent/input_intent/*` after moving classifier code from Telegram transport into core

Forbidden callers:

- `crates/oxide-agent-transport-telegram/*`
- `crates/oxide-agent-transport-web/*`
- bot handlers, callbacks, menus, storage, commands and model picker code

Do not force these auxiliary tasks through `chat_with_tools`; they are not agent loops and should not depend on tool-call history semantics. Do not keep chat-only providers such as Groq for these tasks.

Minimum acceptable implementation: transport crates cannot call any plain text completion API at compile time. If a full provider-trait split is too large, provider-internal plumbing may remain inside `oxide-agent-core`, but no public `LlmClient` method or public re-export may expose plain text completion outside core.

#### DR-003: OpenRouter capability source — static in-code allowlist

Status: resolved.

Decision: OpenRouter provider-level capability is not enough. Use a compact in-code allowlist keyed by OpenRouter model id. Each entry must separate two planes:

- observed/provider-declared capabilities: what OpenRouter metadata or a checked fixture says the model/provider route supports;
- Oxide-approved usage: what this product intentionally allows for main Agent Mode and each media modality.

Required approval flags:

- `approved_for_main_agent`
- `approved_for_media_audio`
- `approved_for_media_image`
- `approved_for_media_video`
- `approved_for_media_document`

Required observed capability fields:

- `supports_tools_parameter`
- `supports_structured_outputs`
- `input_modalities`
- equivalent document/PDF support signal when document understanding is enabled

Initial allowlist scope:

- `google/gemini-3-flash-preview`
- `google/gemini-3.1-flash-lite-preview`
- `google/gemini-2.5-flash-lite`
- `deepseek/deepseek-v4-flash`
- `deepseek/deepseek-v4-pro`

Policy notes:

- Unknown OpenRouter model ids are rejected for Agent Mode and media routes by default.
- A model with `supports_tools_parameter=true` is still not automatically approved for Agent Mode if `approved_for_main_agent=false`.
- Do not implement dynamic OpenRouter discovery, background sync, endpoint-level cache or user-editable capability metadata in this refactor.
- For `google/gemini-3-flash-preview`, current OpenRouter metadata indicates tool use, structured output and multimodal inputs, so do not mark structured output as unsupported without a newer checked fixture.
- For other OpenRouter entries, do not invent capability facts. Implementation must add a checked capability fixture or smoke test for every observed field used by validation.
- OpenRouter agent/tool requests must use provider routing safeguards such as `require_parameters=true` where supported, so routes do not silently ignore required parameters.

Validation rules:

- `AGENT_MODEL_PROVIDER=llm-provider/openrouter` requires allowlist entry with `supports_tools_parameter=true` and `approved_for_main_agent=true`.
- `MEDIA_MODEL_PROVIDER=llm-provider/openrouter` requires allowlist entry plus the approval flag and observed input modality needed for the requested media operation.
- Text-only DeepSeek entries must never be selected for media handling.
- Media-only Gemini entries must not be selected for main Agent Mode unless explicitly promoted in a future PRD/change.

Acceptance criteria:

- OpenRouter provider-level `supports_tool_calling=true` is no longer sufficient to pass Agent Mode validation.
- Unknown OpenRouter model id fails validation before runtime execution.
- Every allowlisted OpenRouter main-agent model has a minimal tool-call fixture or smoke test.
- Every allowlisted OpenRouter media model has a modality-specific routing test.
- Tests cover all sanctioned model ids plus at least one unknown OpenRouter model id.

#### DR-004: NVIDIA NIM compatibility — code-owned exact-match allowlist

Status: resolved.

Decision: keep a small compile-time exact-match allowlist in the NVIDIA provider module. Initial allowed NVIDIA Agent Mode text/tool models:

- `deepseek-ai/deepseek-v4-pro`
- `deepseek-ai/deepseek-v4-flash`

Rules:

- No config-level capability overrides.
- No runtime model metadata discovery.
- No wildcard matching.
- No experimental allow flags.
- Exact string match only.
- NVIDIA routes are text/tool routes only and must not be reused for media/STT/vision/video.

#### DR-005: Fresh DB only; no legacy `chat_mode` compatibility

Status: resolved.

Decision: deployment uses fresh DB. Legacy `chat_mode` state rows are not supported and are not normalized through a special branch. Do not add `LegacyChatMode`, `UnknownChatMode`, `normalize_chat_mode_state`, parser cases, fixtures or migrations for the legacy literal.

If storage returns an unknown/invalid persisted state value, treat it generically as `None` and route through normal agent-only access/configuration flow. Tests for invalid persisted state must use generic invalid values, not legacy literals such as `chat_mode` or `EditingPrompt`, so final hard-zero grep remains meaningful.

#### DR-006: Rejected alternative — remove all plain/internal text completion

Status: rejected.

Reason: compaction, loop detection, wiki writer and input classifier are auxiliary LLM tasks, not agent loops. The accepted decision is DR-002.

#### DR-007: `SYSTEM_MESSAGE` env var — delete completely

Status: resolved.

Decision: remove `SYSTEM_MESSAGE` and `AgentSettings.system_message` if they are only Chat Mode fallback surfaces. Do not add `AGENT_SYSTEM_PROMPT` in this PRD. Agent Mode keeps its existing prompt composition from profiles, topic settings, topic context and Topic `AGENTS.md`.

Acceptance criteria:

- `SYSTEM_MESSAGE` is absent from crates, `.env.example` and CI/deployment env writing.
- Agent Mode prompt composition does not start reading `SYSTEM_MESSAGE`.

#### DR-008: `/clear` becomes reset agent session

Status: resolved.

Decision: `/clear` resets only current agent session/flow/transient context and keeps `State::AgentMode`. It must not clear long-term memory, topic profile, topic `AGENTS.md`, audit records, topic bindings or persistent agent profile data. If there is no active session, `/clear` is a no-op with readiness guidance.

#### DR-009: Media/internal auxiliary defaults after `CHAT_MODEL_*` removal

Status: resolved.

Decision: delete `DEFAULT_CHAT_MODEL_MAX_OUTPUT_TOKENS` and `DEFAULT_CHAT_MODEL_CONTEXT_WINDOW_TOKENS`. Introduce non-chat constants for media and internal auxiliary routes:

- `DEFAULT_MEDIA_MODEL_MAX_OUTPUT_TOKENS`
- `DEFAULT_MEDIA_MODEL_CONTEXT_WINDOW_TOKENS`
- `DEFAULT_INTERNAL_TEXT_MAX_OUTPUT_TOKENS`
- `DEFAULT_INTERNAL_TEXT_CONTEXT_WINDOW_TOKENS`

`media_model_spec()`, wiki memory writer and internal completion routes must not depend on chat defaults or `chat_model_*` fields.

## 6. Target Architecture

### 6.1 User flow

- `/start` is agent-only.
- Private Telegram chat:
  - if user is allowed and has agent access, activate `State::AgentMode` or show Agent Mode controls;
  - if user is allowed but not agent-enabled, return a clear access/configuration message;
  - do not set `chat_mode` and do not show mode selection.
- Supergroup/topic:
  - topic routing and mention policy may still control whether bot processes the message;
  - when processing is allowed, route user input to Agent Mode / agent handler only;
  - no Chat Mode menu or “Please select a mode” fallback.
- Agent cancellation/exit must never leave Agent Mode. `/start` activates `State::AgentMode` immediately for authorized users. There is no mode picker and no “exit to chat”. Cancellation only clears the current agent run, pending confirmation, or transient flow state, then keeps/persists `State::AgentMode` and replies with short guidance such as “Agent task cancelled. Send a new task.”

### 6.1.1 Agent activation, cancellation and reset policy

Product decision: after Chat Mode removal, Agent Mode is the only supported user runtime.

`/start` behavior:

- In private chat, authorized users are immediately placed into `State::AgentMode`.
- `/start` is idempotent: repeated calls keep `State::AgentMode` and do not create a new mode-selection flow.
- `/start` must not read chat model settings, write `chat_mode`, show Chat Mode copy, or show mode picker buttons.
- If the user is allowed by Telegram policy but lacks agent access/configuration, `/start` must not fall back to Chat Mode. It returns explicit access/configuration guidance.
- In supergroups/topics, `/start` and message handling must respect existing topic routing and mention policy, but any accepted user input routes only to Agent Mode.

Cancellation/reset behavior:

- Remove user-facing “Exit Agent Mode” semantics. In an agent-only product there is no alternate mode to exit into.
- Existing exit callbacks, if kept temporarily during refactor, must be remapped to agent cancellation/reset semantics and must never set `chat_mode`.
- Preferred UX labels are `Cancel current task` and `Reset agent session`, not `Exit Agent Mode`.
- `Cancel current task` cancels the active run, pending confirmation, or in-flight tool execution where possible, then keeps `State::AgentMode`.
- `Reset agent session` clears only current agent flow/session/transient context. It must not clear long-term agent memory, topic profile, topic `AGENTS.md`, audit records, topic bindings or persistent agent profile data.
- If no active run exists, cancel/reset is a no-op that keeps `State::AgentMode` and replies with “Agent Mode is ready. Send a task.”
- Stale inline callbacks from old messages or old confirmations must be rejected with a short expired/unsupported message. They must not restore any removed runtime or execute stale actions.
- Unsupported/unknown persisted state values are treated as no valid runtime state (`None`) and routed through agent-only access/configuration flow. Do not add special branches, parser cases, test fixtures or compatibility paths for legacy literals. See DR-005.

### 6.2 State and persisted config

- No `State::ChatMode` variant.
- No `chat_mode` persisted state accepted as active runtime.
- Unsupported/unknown persisted state values are treated as no valid runtime state (`None`) and routed through agent-only access/configuration flow. Do not add a special branch, parser case, test fixture, or compatibility path for legacy `chat_mode`. See DR-005.
- `State::EditingPrompt` is removed completely. This PRD does not introduce, rename, or preserve any Telegram user-facing prompt editor.
- `MENU_CALLBACK_EDIT_PROMPT`, `MenuCallbackData::EditPrompt`, text-menu “Edit Prompt” branches, and the handler that stores edited prompt text are deleted.
- Per-user prompt editing via `storage.update_user_prompt()` / `storage.get_user_prompt()` is treated as Chat Mode surface and removed from Telegram runtime.
- Existing agent-owned prompt mechanisms are preserved only if they are already used by Agent Mode and do not depend on `SYSTEM_MESSAGE` or per-user prompt editing.
- Adding Agent prompt editing is explicitly out of scope and requires a separate PRD.
- `current_chat_uuid` is removed from `UserConfig` and `UserContextConfig`.
- Agent flow ID remains, but any helper named `generate_chat_uuid()` is renamed to a generic flow/run ID generator.

### 6.3 Config

- `CHAT_MODEL_ID`, `CHAT_MODEL_PROVIDER`, `CHAT_MODEL_NAME`, `CHAT_MODEL_MAX_OUTPUT_TOKENS`, `CHAT_MODEL_CONTEXT_WINDOW_TOKENS` are removed from `AgentSettings`, `.env.example`, README, workflows and scripts.
- Agent route config is source of truth:
  - `AGENT_MODEL_*` or `AGENT_MODEL_ROUTES__*` for main agent;
  - `SUB_AGENT_MODEL_*` or `SUB_AGENT_MODEL_ROUTES__*` for sub-agent;
  - explicit internal routes for loop detection/wiki writer/classifier if needed;
  - explicit `MEDIA_MODEL_*` only for media auxiliary routes, not chat fallback.
- Missing agent route is fail-fast config error. No fallback to chat model or default `ModelInfo::default()`.

### 6.4 Storage

- Remove chat history storage APIs and keys.
- Remove per-user chat model storage.
- Remove user prompt storage if it is only used by chat prompt editing.
- Keep agent memory, agent flow records, topic contexts, topic bindings, agent profiles and audit storage.
- `clear_all_context()` must stop referencing chat histories. It must not be used for `/clear` unless audited to affect only transient agent session/flow data. `/clear` must not delete long-term memory, topic profile, topic `AGENTS.md`, audit records, topic bindings or persistent agent profile data.

### 6.5 LLM API

- No user-facing `LlmClient::chat_completion()` path.
- Telegram transport must not call `chat_completion` or any renamed internal text completion.
- Provider trait should make agent capability explicit. Preferred target:
  - `chat_with_tools` / agent request interface is the primary provider contract for agent routes;
  - plain text completion is removed from public `LlmProvider` contract or split into an internal trait not visible to transport.
- Internal completion is kept only as crate-private API:
  - renamed to `complete_internal_text`;
  - requires `InternalTextPurpose` enum (`CompactionSummary`, `LoopDetection`, `WikiMemoryWriter`, `InputIntentClassification`);
  - request is restricted: no `chat_id`, `telegram_user_id`, `chat_history`, `stored_user_prompt`, `per_user_prompt`, `reply_markup` or chat flow metadata;
  - route resolution uses purpose-based route or main agent route; never falls back to `CHAT_MODEL_*`;
  - callable only from core agent internals;
  - prohibited from transport crates at compile time (`pub(crate)` in core crate, no re-export).
- Do not force compaction, loop detection, wiki writer or input classifier through `chat_with_tools`. These are auxiliary LLM tasks, not agent loops, and should not depend on tool-call history semantics.
- Tests must assert no transport module references internal completion.

### 6.6 Provider route policy

- Provider selection only through agent-compatible routes.
- Unknown provider capabilities are default-deny.
- `supports_tool_calling=false` excludes a route from agent loop even if structured output is supported.
- OpenRouter default-deny for agent and media routes unless the selected model id exists in the static in-code allowlist from DR-003 with the required observed capability and product approval flag.
- NVIDIA requires the exact-match code-owned allowlist from DR-004 before route selection/execution.
- ChatGPT provider remains only as agent provider with tool calling support; JSON/structured-output limitations must be handled alias-safely.
- Groq is removed.


### 6.7 Modality and media architecture

All user media is agent input. It must never be answered through Chat Mode, chat history, `process_llm_request()` or transport-level `chat_completion`.

Voice messages:

- Telegram voice payload is downloaded only after agent access/topic routing allows processing.
- STT requires an explicit media route: `MEDIA_MODEL_ID` + `MEDIA_MODEL_PROVIDER` with audio transcription support.
- Acceptable current providers are the existing media-capable routes, especially OpenRouter Gemini-family models through `llm-provider/openrouter` and Mistral/Voxtral-style STT through `llm-provider/mistral` when configured.
- Do not add a direct Google Gemini provider as part of this refactor. Direct Gemini provider IDs are currently forbidden by repo checks; Gemini-family means OpenRouter route unless a separate product decision changes that.
- On successful STT, the transcript is dispatched as normal Agent Mode text input with metadata such as source=`telegram_voice`, mime type and optional caption/context.
- If no STT route exists, reply with a clear unsupported message: voice messages require a configured media/STT provider. Do not activate chat, do not show mode menu, do not call plain completion.

Photo, video, audio files and documents:

- Telegram handler must gate by agent access, topic route, file size and media/file feature availability before download.
- Files are stored only in the agent sandbox / per-run upload area using sanitized filenames and existing sandbox scope logic.
- The agent receives an attachment descriptor/path plus user caption/task text; the raw file is not written to chat history or R2 chat history.
- Preferred behavior is tool-first: expose `describe_image_file`, `describe_video_file` and `transcribe_audio_file` from `MediaFileProvider` so the agent can decide when and how to inspect the file.
- Eager preprocessing is allowed only as an agent input preprocessor that converts media into agent context text. Its output must be fed into Agent Mode, not sent directly to the user as a chat response.
- Missing media model, missing media feature/profile, unsupported MIME type, oversize files or sandbox write failure must produce explicit unsupported/error messages.

Media tool safety:

- Tool arguments must resolve through sandbox path validation; no arbitrary host paths.
- Agent-provided prompts to media tools are task prompts, not replacement system prompts. Keep a fixed tool/system instruction that asks for faithful description/transcription and preserves user intent separation.
- Media route selection must check modality capability: audio transcription, image understanding and video understanding are separate capabilities.
- Media capability does not imply agent tool-calling capability. A route may be valid for media tools while still invalid as the main Agent Mode LLM route.

## 7. Provider Compatibility Policy

Provider compatibility must be based on concrete agent requirements, not on existence of a chat completion endpoint.

Rules:

- A provider is not agent-compatible just because it can answer a normal chat request.
- Agent-compatible route must support `chat_with_tools` or equivalent agent loop mechanics required by current runtime.
- Tool history behavior must be explicit: `Strict` or `BestEffort`, not implicit.
- Model-level capabilities override provider-level assumptions.
- Unknown provider/model capabilities are treated as incompatible.
- Failover must not choose incompatible routes.
- Config validation should fail fast when the only configured agent routes are incompatible.
- Runtime should skip or reject unsupported routes before execution attempt; no provider should be called just to discover missing tool support when capability data says unsupported.
- Structured output support does not imply agent/tool support.
- Media support does not imply agent/tool support.


### Media Capability Policy

Media capability is separate from agent compatibility:

- `MEDIA_MODEL_*` routes are auxiliary media routes, not replacements for `AGENT_MODEL_*` / `AGENT_MODEL_ROUTES__*`.
- A media route may support audio/image/video/document understanding while not supporting tool calling; that route can be used only by media tools/preprocessor, not as the main agent route.
- Audio STT, image understanding, video understanding and document understanding must be checked independently via `MediaModality` / `MediaCapabilities` or equivalent route metadata.
- OpenRouter media routes are model-dependent. Gemini-family model IDs can be valid through OpenRouter, but OpenRouter should still be default-deny for main agent tool routes unless explicitly marked agent-compatible.
- Mistral/Voxtral-style audio STT can be valid for voice transcription even if the selected main agent model is another provider.
- Missing `MEDIA_MODEL_*` must disable media understanding gracefully; it must not fallback to `CHAT_MODEL_*`, `chat_model_name` or plain chat completion.
- Direct provider IDs such as `gemini`, `google-gemini` or `llm-provider/gemini` remain forbidden unless a separate provider integration PR intentionally changes repo policy.


### Required Provider Categories

#### Keep

- `llm-provider/opencode-go`
  - Evidence: `opencode_go/module.rs` declares `ToolHistoryMode::Strict`, `supports_tool_calling=true`; model-specific structured output is handled in `capabilities_for_model()`.
  - Action: keep as agent-compatible. Ensure aliases `opencode-go` and `opencode_go` canonicalize to `llm-provider/opencode-go`.

- `llm-provider/openai-chatgpt`
  - Evidence: `chatgpt/module.rs` declares `supports_tool_calling=true`, `supports_structured_output=false`, aliases `chatgpt`, `openai-chatgpt`.
  - Action: keep as agent provider when auth path is valid. Do not remove due to name similarity with Chat Mode. Ensure JSON-mode restrictions are canonical/alias-safe.

- `llm-provider/zai`
  - Evidence: `zai/module.rs` declares `supports_tool_calling=true`; structured output is model allowlist-based.
  - Action: keep as agent-compatible with existing model-level structured-output gating.

- `llm-provider/mistral`
  - Evidence: `mistral/module.rs` declares `Strict`, `supports_tool_calling=true`, `supports_structured_output=true`; audio media support exists.
  - Action: keep. Media/audio use must remain auxiliary to Agent Mode, not Chat Mode.

- `llm-provider/minimax`
  - Evidence: `minimax/module.rs` declares `Strict`, `supports_tool_calling=true`, `supports_structured_output=false`.
  - Action: keep. Structured-output limitations must be respected by route selection.

#### Keep with model-level gating

- `llm-provider/nvidia`
  - Evidence: `nvidia/module.rs` delegates capabilities to `nvidia::model_capabilities()`; `nvidia.rs` currently has allowlist/wildcard logic and rejects unsupported model ids in `chat_with_tools()`.
  - Policy: NVIDIA NIM is supported only through the code-owned exact-match allowlist from DR-004.
  - Initial allowed Agent Mode text/tool models:
    - `deepseek-ai/deepseek-v4-pro`
    - `deepseek-ai/deepseek-v4-flash`
  - Required change: replace wildcard/broad compatibility logic with exact-match allowlist; route selection and config validation must reject unsupported NVIDIA models before execution attempt. NVIDIA must not be selected for media/STT/vision/video.

- `llm-provider/openrouter`
  - Evidence: current `openrouter/module.rs` marks provider-level `supports_tool_calling=true`, but OpenRouter compatibility is route/model-dependent.
  - Required change: default-deny OpenRouter for agent and media routes unless the selected model id exists in the static in-code allowlist from DR-003 with the required observed capability and approval flag. Do not add user-editable capability config, runtime discovery, or dynamic capability cache in this refactor.

#### Remove

- `llm-provider/groq`
  - Evidence: `groq/module.rs` declares `supports_tool_calling=false`; `groq.rs` implements only `chat_completion`, with STT/image not implemented and no `chat_with_tools()` override.
  - Action: remove provider module, feature `llm-groq`, env `GROQ_API_KEY`, compiled capability entry, profile entry, docs, workflows, snapshots and tests.

- Any route/provider that only supports plain chat completion and cannot participate in agent loop.
  - Action: remove if it exists solely for Chat Mode; otherwise exclude from agent route selection and do not expose to user transport.

#### Requires verification

- None for provider category policy.

Implementation still must verify exact code locations and update tests/snapshots, but product decisions for internal completion, OpenRouter, NVIDIA, Groq removal, media routes and ChatGPT alias handling are resolved by DR-002, DR-003, DR-004 and the policy sections above.

## 8. Functional Requirements

### FR-001: Remove Chat Mode state

ID: `FR-001`

Название: удалить `ChatMode` / `chat_mode` из state machine и persisted runtime.

Описание:

- Remove `State::ChatMode` from `crates/oxide-agent-transport-telegram/src/bot/state.rs`.
- Remove `State::ChatMode` branch from `crates/oxide-agent-transport-telegram/src/runner.rs`.
- Remove code paths that set, restore, compare or persist `"chat_mode"`.
- Unsupported/unknown persisted state values must not activate any removed runtime. Treat them as absent state (`None`) and apply agent-only access/configuration flow. Do not add a special parser branch, compatibility path, or test fixture for legacy `chat_mode`; see DR-005.

Rationale:

- Chat Mode is the root runtime switch allowing user messages to avoid Agent Mode.

Acceptance Criteria:

- `rg -n "ChatMode|chat_mode" crates/oxide-agent-transport-telegram/src` returns no live runtime references.
- Dialogue can compile without `State::ChatMode` branch.
- Unsupported/unknown persisted state does not activate any plain chat path.
- Agent confirmation and Agent Mode states still work.

Affected Areas:

- `bot/state.rs`
- `runner.rs`
- `bot/handlers.rs`
- `bot/context.rs`
- `bot/agent_handlers/controls.rs`
- Telegram tests/mocks referencing removed state; invalid-state tests must use generic invalid values, not legacy literals

Edge Cases:

- User sends message with unsupported/unknown persisted state.
- User has no persisted state.
- Confirmation flow is active while unknown/invalid state exists elsewhere in storage.
- Group topic has unsupported/unknown context state while DM global state differs.

### FR-002: Remove Chat Mode menu/callbacks

ID: `FR-002`

Название: удалить Chat Mode из Telegram UX, inline keyboards and callbacks.

Описание:

- Remove “💬 Chat Mode” button and `MENU_CALLBACK_CHAT_MODE`.
- Remove Chat Mode menu entries: `Clear Flow`, `Change Model`, `Extra Functions`, `Edit Prompt`, `Back`. `/clear` is redefined separately as agent session reset in DR-008, not preserved as chat flow clear.
- Remove model keyboard driven by `settings.agent.get_chat_models()`.
- Remove chat attach/detach controls and callbacks: `CHAT_ATTACH_PREFIX`, `CHAT_DETACH_CALLBACK`, `handle_chat_flow_callback()`.
- Remove `MenuCallbackData::ChatMode`, `ChangeModel`, `ExtraFunctions`, `EditPrompt`, `Model(usize)` unconditionally in this PRD; no agent-equivalent model/prompt UX is introduced here.

Rationale:

- UI must not expose a removed runtime or controls that mutate chat-specific storage.

Acceptance Criteria:

- No Telegram button/callback activates Chat Mode.
- No callback mutates `current_chat_uuid`.
- No menu path updates per-user chat model.
- `/start` and agent controls show only agent-related UX.

Affected Areas:

- `bot/handlers.rs:32-41`, `225-330`, `332-342`, `509-792`, `945-1154`
- `runner.rs` callback chain if chat callbacks are registered there
- `bot/agent_handlers/controls.rs` exit flow
- tests in `bot/handlers.rs`

Edge Cases:

- User presses stale inline callback `menu:chat` or `chat_attach:*` from old messages.
- User types old keyboard label “💬 Chat Mode”.
- User types a model display name that previously selected chat model.
- Forum topics still using inline controls.

### FR-003: Make `/start` agent-only

ID: `FR-003`

Название: `/start` не должен fallback-иться в Chat Mode.

Описание:

- Refactor `handlers.rs:start()` so it never sets `Some("chat_mode")`.
- For agent-authorized user, activate or present Agent Mode only.
- For allowed Telegram user without agent access, return access/configuration guidance.
- Remove welcome copy mentioning Chat Mode.
- Remove `storage.get_user_model()` and `resolve_chat_model()` from `/start`.

Rationale:

- Current `/start` makes Chat Mode the default fallback for private chats and non-supergroup scenarios.

Acceptance Criteria:

- `/start` never writes `chat_mode` to storage.
- `/start` never reads per-user chat model.
- `/start` response contains no “Chat Mode” wording.
- Private chats and groups have deterministic agent-only behavior.

Affected Areas:

- `bot/handlers.rs:385-479`
- `bot/context.rs:set_current_context_state()` call sites
- Telegram integration/unit tests around default mode

Edge Cases:

- Private chat user is not listed in `TELEGRAM_ALLOWED_USERS`.
- Supergroup user has agent access.
- Supergroup user lacks agent access.
- User calls `/start` during active Agent Mode or confirmation flow.

### FR-004: Route user text to Agent Mode only

ID: `FR-004`

Название: весь пользовательский текст должен идти только в Agent Mode.

Описание:

- Refactor `handle_text()` so non-command text goes to `agent_handlers::handle_agent_message()` when access and topic route allow processing.
- Remove `process_llm_request()` call from Telegram transport.
- Remove “Please select a mode” fallback as a mode-selection UX.
- Preserve topic routing/mention logic, but output target is agent runtime only.
- If access is missing, return access denied/config guidance, not chat response.

Rationale:

- User text is the highest-risk path for hidden chat fallback.

Acceptance Criteria:

- `rg -n "process_llm_request" crates/oxide-agent-transport-telegram/src` returns no live code.
- `handle_text()` does not call `llm.chat_completion` or internal text completion.
- Plain text from authorized agent user starts/continues agent flow.
- Plain text from unauthorized/non-agent user gets a clear refusal/guidance.

Affected Areas:

- `bot/handlers.rs:857-943`
- `bot/agent_handlers/*`
- `bot/topic_route.rs`
- Telegram handler tests

Edge Cases:

- User sends message before any state exists.
- User sends message in topic that requires mention but mention is absent.
- User sends old menu labels.
- Agent flow is completed/timed out and user sends follow-up text.

### FR-004A: Replace Agent exit with Agent cancellation/reset

ID: `FR-004A`

Название: заменить выход из Agent Mode на cancel/reset semantics.

Описание:

- Remove or rename any user-facing “Exit Agent Mode” action.
- Existing exit callbacks must not set `chat_mode`, clear to Chat Mode, or show mode picker.
- If a run is active, cancel it and keep `State::AgentMode`.
- If a confirmation is pending, expire it and keep `State::AgentMode`.
- If no run is active, keep `State::AgentMode` and show readiness guidance.
- Reset controls may clear current agent session/flow but must not clear unrelated persistent agent memory/profile.

Rationale:

- Agent-only UX has no second runtime to exit into. “Exit” was previously a Chat Mode fallback and must be removed with Chat Mode.

Acceptance Criteria:

- No callback or command can transition from Agent Mode to Chat Mode.
- No callback or command shows a mode picker.
- Cancellation leaves the user able to send the next task immediately.
- Stale exit/cancel callbacks do not resurrect removed state.
- Agent confirmation flow remains safe: expired confirmations cannot execute tools.

Affected Areas:

- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/controls.rs`
- `crates/oxide-agent-transport-telegram/src/bot/handlers.rs`
- `crates/oxide-agent-transport-telegram/src/runner.rs`
- `crates/oxide-agent-transport-telegram/src/bot/context.rs`
- Telegram tests around callbacks, cancellation, confirmation and `/start`

Edge Cases:

- User presses old “Exit Agent Mode” inline button.
- User presses old removed-mode callback.
- User cancels while a tool is running.
- User cancels while confirmation is pending.
- User sends `/start` while a run is active.
- User has unknown/invalid persisted state from damaged storage; it must fall back to agent-only access/configuration flow.
- User has no agent access.
- Group topic has unsupported/unknown context state.

### FR-005: Remove `CHAT_MODEL_*` config

ID: `FR-005`

Название: удалить chat model config and validation.

Описание:

- Remove `chat_model_id`, `chat_model_name`, `chat_model_provider`, `chat_model_max_output_tokens`, `chat_model_context_window_tokens` from `AgentSettings`.
- Remove `CHAT_MODEL_ID` / `CHAT_MODEL_PROVIDER` hard requirements from `AgentSettings::new()`.
- Remove `CHAT_MODEL_PROVIDER` validation/canonicalization and configured route provider enumeration.
- Remove `chat_model_spec()`, `get_chat_models()`, `get_default_chat_model_name()`, `get_model_info_by_name()` if only used for chat model selection.
- Remove `CHAT_MODEL_ID` fallback from `get_agent_model()`.
- Replace media use of chat token/context defaults with explicit media defaults or agent defaults.

Rationale:

- Agent-only config must not require or silently use chat model.

Acceptance Criteria:

- `rg -n "CHAT_MODEL|chat_model" crates/oxide-agent-core/src/config.rs` returns no live config surface except deleted-history notes if any.
- `AgentSettings::new()` fails when agent route is missing, not when chat model is missing.
- `MEDIA_MODEL_*` does not inherit from removed chat defaults.
- Old env files containing `CHAT_MODEL_*` do not change runtime behavior.

Affected Areas:

- `crates/oxide-agent-core/src/config.rs`
- `.env.example`
- `README.md`
- `.github/workflows/ci-cd.yml`
- config tests
- media route tests in `llm/client.rs`

Edge Cases:

- No `CHAT_MODEL_*` env vars present.
- Old `.env` still has `CHAT_MODEL_*` vars.
- Agent model routes absent.
- Sub-agent route absent but main agent route present.
- Media model absent.

### FR-006: Remove chat model selection

ID: `FR-006`

Название: удалить per-user chat model selection.

Описание:

- Remove `resolve_chat_model()`.
- Remove “Change Model” menu and model keyboard.
- Remove `storage.update_user_model()` / `storage.get_user_model()` usage from Telegram transport.
- Remove `UserConfig.model_name` if it is only used for chat model selection.
- Remove tests that select model by display name.

Rationale:

- Agent model selection must be route/config-driven, not per-user chat keyboard-driven.

Acceptance Criteria:

- No Telegram path calls `update_user_model` or `get_user_model`.
- `get_model_keyboard()` and `get_model_inline_keyboard()` are deleted or repurposed only for explicit agent route administration if product requires it later.
- Text matching a model display name is treated as normal agent input, not as config mutation.

Affected Areas:

- `bot/handlers.rs:58-65`, `297-330`, `741-787`, `911-923`
- `storage/provider.rs:31-35`
- `storage/user.rs:9-10`
- `r2_user.rs`, `r2_provider.rs`, `in_memory_storage.rs`

Edge Cases:

- Old stored `model_name` exists.
- User sends text equal to an old model display name.
- Agent route has multiple weighted routes.

### FR-007: Remove chat history storage

ID: `FR-007`

Название: удалить chat history APIs and chat UUID storage.

Описание:

- Remove storage trait methods for global/scoped chat history.
- Remove R2 keys for `users/{user}/history.json` and `users/{user}/chats/.../history.json` from runtime code.
- Remove `current_chat_uuid` from user/context config.
- Remove `ensure_current_chat_uuid()`, `reset_current_chat_uuid()`, `scoped_chat_storage_id()`.
- Rename `generate_chat_uuid()` if used by agent flow IDs.
- Update mocks/tests after trait cleanup.

Rationale:

- Chat history is persistence for the removed plain chat runtime. Agent memory/flows are separate and must remain.

Acceptance Criteria:

- `StorageProvider` trait has no chat history methods.
- `UserConfig` and `UserContextConfig` have no `current_chat_uuid`.
- Runtime never reads/writes `users/*/chats/*` chat histories.
- Agent memory and agent flow storage still compile and pass tests.

Affected Areas:

- `storage/provider.rs:40-90`
- `storage/user.rs:13-14`, `25-26`
- `storage/keys.rs:22-38`
- `storage/r2_user.rs`
- `storage/r2_provider.rs`
- `storage/telemetry.rs`
- `transport-web/src/in_memory_storage.rs`
- all StorageProvider test mocks

Edge Cases:

- Old R2 chat history files exist.
- Old `current_chat_uuid` exists in `config.json`.
- `clear_all_context()` currently clears both chat and agent context; it must stop referencing chat histories and must not be wired to `/clear` unless audited to affect only transient agent session data.
- Tests rely on `Message` chat history struct.

### FR-008: Remove user-facing chat completion

ID: `FR-008`

Название: удалить `process_llm_request` and transport-level `chat_completion` calls.

Описание:

- Delete `process_llm_request()` from Telegram handlers.
- Ensure Telegram transport never calls `LlmClient::chat_completion()` or `chat_completion_for_model_info()` directly.
- Remove user-facing history-to-LLM conversion in `handlers.rs:1271-1283`.
- Remove save user/assistant chat messages around `chat_completion`.

Rationale:

- This is the direct plain chat runtime path that bypasses Agent Mode.

Acceptance Criteria:

- `rg -n "chat_completion|process_llm_request" crates/oxide-agent-transport-telegram/src` returns no live user-facing references, except explicitly internal agent classifier if it is moved out of transport or renamed.
- User messages cannot be answered by a plain assistant response outside Agent Mode.
- No Telegram handler builds `Vec<LlmMessage>` from chat history for direct chat completion.

Affected Areas:

- `bot/handlers.rs:1235-1326`
- `bot/handlers.rs:857-943`
- `bot/agent_handlers/input_intent.rs` if internal completion remains in transport; preferred move into core/internal service

Edge Cases:

- Voice transcription currently calls `process_llm_request()` after STT.
- Old chat histories are unavailable.
- Error handling must return agent error/guidance, not chat error.

### FR-009: Internal-only text completion with purpose-based routing

ID: `FR-009`

Название: isolate internal-only text completion with purpose-based routing and compile-time caller restrictions.

Описание:

- Remove public `LlmClient::chat_completion()` and `LlmClient::chat_completion_for_model_info()`.
- Add crate-private `complete_internal_text` that requires `InternalTextPurpose` and a restricted `InternalTextCompletionRequest` without chat context.
- Do not force compaction, loop detection, wiki writer or input classifier through `chat_with_tools`.
- Restrict visibility so transport/user layers cannot call it at compile time (`pub(crate)`, no re-export).
- Keep internal uses only where required: local compaction summary, loop detection, wiki memory writer, input intent classification.
- Move `input_intent.rs` from Telegram transport to core so transport calls a high-level classifier service rather than the LLM client directly.
- Provider boundary must be closed enough that transport crates cannot call any plain text completion API. A full trait split is preferred, but the minimum acceptable implementation is no public `LlmClient` method and no public re-export outside `oxide-agent-core`.

Rationale:

- Some agent internals use plain text completion for auxiliary tasks. Deleting them blindly can regress Agent Mode. Keeping them under `chat_completion` preserves removed Chat Mode vocabulary and risks future misuse. Forcing them through `chat_with_tools` adds coupling to tool-call semantics that these tasks do not need.

Acceptance Criteria:

- No public `LlmClient::chat_completion()` user-facing method remains.
- Internal completion API is named `complete_internal_text`, is crate-private, and is not re-exported from public API.
- Internal completion API requires `InternalTextPurpose` and uses a request without `chat_id`, `telegram_user_id`, `chat_history`, `stored_user_prompt`, `per_user_prompt` or `reply_markup`.
- Route resolution for internal completion never falls back to `CHAT_MODEL_*`.
- Chat-only providers are not kept or used for internal completion.
- Allowed callers: `agent/compaction/*`, `agent/loop_detection/*`, `agent/executor/*` for wiki writer, `agent/input_intent/*`.
- Forbidden callers: Telegram transport, web transport, bot handlers/callbacks/menu, storage, commands, model picker.
- Each internal task has a working fallback when internal route is unavailable:
  - Compaction: deterministic/extractive fallback or conservative truncation.
  - Loop detection: heuristic-only mode; agent loop is not disabled.
  - Wiki writer: skip write and log; user run does not fail.
  - Input classifier: deterministic classification; safe default treats input as normal agent task.
- `input_intent.rs` lives in core crate, not in Telegram transport.
- Tests assert transport crates do not reference `chat_completion`, `chat_completion_for_model_info` or `complete_internal_text`.

Affected Areas:

- `llm/provider.rs`
- `llm/client.rs:219-296`
- `llm/support/openai_compat.rs`
- provider implementations
- `agent/compaction/local_llm_summary.rs`
- `agent/loop_detection/llm_detector.rs`
- `agent/executor/execution.rs`
- `bot/agent_handlers/input_intent.rs` move to core
- mocks/tests implementing `chat_completion`

Edge Cases:

- Local compaction currently depends on text completion retry behavior.
- Loop detector disables itself on LLM errors.
- Wiki writer runs in background with timeout.
- Input classifier has deterministic fallback.
- Internal completion must not accidentally re-introduce chat history dependence.

### FR-010: Remove chat-only providers

ID: `FR-010`

Название: удалить or exclude providers incompatible with Agent Mode.

Описание:

- Classify provider modules by actual capabilities.
- Remove providers that do not implement/advertise agent tool calling.
- Remove their cargo features, capability manifest entries, env vars, docs, tests, snapshots and profiles.
- If a provider is media-only or internal-only, make that capability explicit and prevent agent route selection unless it supports agent requirements.

Rationale:

- Agent-only runtime cannot contain providers that only satisfy old Chat Mode.

Acceptance Criteria:

- No provider with `supports_tool_calling=false` is selectable as an agent route.
- Unknown provider capabilities are default-deny.
- Full profile and all-features snapshots no longer include removed providers.
- CI profile scripts do not require removed modules.

Affected Areas:

- `llm/providers/*`
- `llm/providers/modules.rs`
- `llm/capabilities.rs`
- `capabilities/compiled.rs`
- `Cargo.toml`
- `profiles/*.toml`
- `scripts/check-compiled-capabilities.sh`
- registry snapshots

Edge Cases:

- Provider supports structured output but no tool calling.
- Provider supports media but no agent tools.
- Provider alias remains after module deletion.
- Cargo feature still compiles deleted module.

### FR-011: Remove Groq if chat-only

ID: `FR-011`

Название: полностью удалить Groq provider.

Описание:

- Delete `crates/oxide-agent-core/src/llm/providers/groq.rs` and `providers/groq/module.rs` or remove them from compilation.
- Remove `llm-groq` from `Cargo.toml` and from `profile-full` feature composition.
- Remove `GROQ_CHAT_TEMPERATURE`.
- Remove `GROQ_API_KEY` from config schema/capabilities, env examples, workflows and docs.
- Remove `llm-provider/groq` from `profiles/full.toml` and `scripts/check-compiled-capabilities.sh`.
- Update snapshots and tests that expect Groq registration/capabilities.

Rationale:

- Current code proves Groq is chat-only: `groq/module.rs` sets `supports_tool_calling=false`, and `groq.rs` implements plain `chat_completion` only.

Acceptance Criteria:

- `rg -n "Groq|GROQ|llm-groq|llm-provider/groq" .` has no live runtime/docs/profile references after cleanup.
- `cargo check --workspace --all-features` succeeds without `llm-groq`.
- No capability manifest or profile contains Groq.
- README no longer advertises Groq.

Affected Areas:

- `llm/providers/groq.rs`
- `llm/providers/groq/module.rs`
- `llm/providers/mod.rs`
- `llm/providers/modules.rs`
- `config.rs`
- `capabilities/compiled.rs`
- `Cargo.toml`
- `profiles/full.toml`
- `.env.example`, README, AGENTS.md, workflows, integration tests, snapshots

Edge Cases:

- `async-openai` dependency might still be needed by Mistral; remove only `llm-groq` feature, not shared dependencies used by other providers.
- Snapshot tests with profile-full/all-features.

### FR-012: Harden OpenRouter compatibility

ID: `FR-012`

Название: OpenRouter default-deny for agent and media routes unless model/route is verified.

Описание:

- Replace provider-level blanket `supports_tool_calling=true` for OpenRouter with model-level decision based on the static in-code allowlist from DR-003.
- Implement the allowlist as a compact in-code data structure covering the sanctioned model ids.
- Store observed/provider-declared capabilities separately from Oxide approval flags.
- Route selection must reject OpenRouter routes without allowlist confirmation before LLM attempt.
- OpenRouter requests with tools must use provider routing safeguards such as `require_parameters=true` where supported.
- Tests must include all sanctioned model ids from DR-003 plus one unknown OpenRouter model id.

Rationale:

- OpenRouter aggregates many upstream models; provider-level capability is not enough. DR-003 provides the exact decision model.

Acceptance Criteria:

- OpenRouter route without allowlist entry is not selected for Agent Mode or media routes.
- Compatible OpenRouter route can be selected when allowlisted and credentials exist.
- Failover does not pick unverified OpenRouter route.
- Every OpenRouter model approved for main Agent Mode has a minimal multi-turn or tool-call fixture/smoke test.
- Every OpenRouter model approved for media has a modality-specific media routing test.
- For `google/gemini-3-flash-preview`, do not mark structured output unsupported unless a newer checked fixture proves it; current OpenRouter metadata advertises structured output and tool use.
- Unknown OpenRouter model id fails configuration validation before runtime execution.
- Error message explains missing model-level tool capability or missing allowlist entry.

Affected Areas:

- `openrouter/module.rs`
- `llm/capabilities.rs`
- `llm/providers/modules.rs`
- `agent/runner/execution.rs`
- OpenRouter static allowlist module/tests
- docs/env examples
- route selection tests

Edge Cases:

- OpenRouter model claims tools but fails tool calls at runtime.
- OpenRouter route supports media but not tools.
- Alias/canonical provider id mismatch.
- Weighted failover prefers OpenRouter route with high weight.

### FR-013: Harden NVIDIA NIM compatibility with a minimal allowlist

ID: `FR-013`

Название: restrict NVIDIA NIM to explicitly approved Agent Mode text/tool models.

Описание:

NVIDIA NIM must not be treated as provider-wide agent-compatible. For this refactor, NVIDIA compatibility must be implemented as a small exact-match code-owned allowlist, as specified in DR-004.

Initial allowed models:

- `deepseek-ai/deepseek-v4-pro`
- `deepseek-ai/deepseek-v4-flash`

These models are allowed only for Agent Mode text/tool execution.

Do not add config-level capability overrides, runtime model metadata discovery, wildcard matching, or experimental allow flags as part of this PRD. Replace existing wildcard/broad NVIDIA compatibility logic with exact-match allowlist.

Rationale:

- The goal is to remove Chat Mode and unsafe provider tails with minimal architectural churn. NVIDIA model support varies by model, but current product decision only needs two known Agent Mode text/tool models.

Acceptance Criteria:

- `deepseek-ai/deepseek-v4-pro` is accepted by NVIDIA Agent Mode route selection.
- `deepseek-ai/deepseek-v4-flash` is accepted by NVIDIA Agent Mode route selection.
- Any other NVIDIA model ID is rejected before execution unless added to the allowlist in code.
- Rejection message clearly says the selected NVIDIA model is not approved for Agent Mode tool execution.
- NVIDIA route does not use `CHAT_MODEL_*` fallback.
- NVIDIA route is not used for media/STT/vision/video.
- Existing test `run_skips_unsupported_nvidia_route_and_uses_backup` is strengthened to assert unsupported route is skipped before provider execution attempt.
- Single-route unsupported NVIDIA fails fast with a clear config/runtime error.
- Unit tests cover both allowed model IDs and at least one rejected model ID.
- Route-selection tests prove unsupported NVIDIA models cannot be selected through failover.

Affected Areas:

- NVIDIA provider capability checks.
- `nvidia.rs:model_capabilities()` — replace wildcards with exact-match allowlist.
- `nvidia/module.rs:capabilities_for_model()`.
- Route selection / provider registry.
- Agent model validation.
- Config validation.
- Provider capability tests.
- Documentation/examples mentioning NVIDIA models.

Edge Cases:

- Model ID casing or alias mismatch.
- Existing config references an older NVIDIA model.
- Failover tries NVIDIA after primary provider fails.
- NVIDIA endpoint accepts a model but tool calling fails at runtime.
- Media route tries to reuse the NVIDIA agent model.

### FR-014: Preserve ChatGPT as agent provider

ID: `FR-014`

Название: сохранить ChatGPT provider как agent provider.

Описание:

- Do not remove `llm-provider/openai-chatgpt` because of “Chat” naming.
- Preserve aliases `chatgpt`, `openai-chatgpt` if they are used by config and docs.
- Ensure route canonicalization maps aliases to `llm-provider/openai-chatgpt`.
- Fix structured-output restrictions to be canonical/alias-aware.
- Document that ChatGPT provider is supported only as Agent Mode provider.

Rationale:

- ChatGPT provider and Chat Mode are different concepts. Removing ChatGPT would delete an agent-compatible provider.

Acceptance Criteria:

- ChatGPT provider still builds under `llm-chatgpt`.
- Agent Mode can select ChatGPT route when tools are required and JSON/structured-output constraints permit it.
- Search for “Chat Mode” does not match ChatGPT provider documentation except explicit “not Chat Mode” notes if retained.

Affected Areas:

- `chatgpt/module.rs`
- `providers/modules.rs`
- `config.rs` provider canonicalization
- `agent/runner/execution.rs:743-745`
- docs/env examples
- tests around ChatGPT route selection

Edge Cases:

- Route provider is `chatgpt` alias.
- Route provider is canonical `llm-provider/openai-chatgpt`.
- JSON mode requires structured output and ChatGPT is primary.
- ChatGPT auth path missing.

### FR-015: Update media handling

ID: `FR-015`

Название: voice/photo/video/audio/document flows must become agent-only modality inputs.

Описание:

- Delete or bypass Chat Mode-only media handlers in `bot/handlers.rs`; Telegram media must enter Agent Mode through `bot/agent/media.rs`, `agent_handlers/input.rs`, `task_runner.rs` and `agent/preprocessor.rs`.
- Voice messages:
  - download voice payload only after agent access/topic route checks pass;
  - require explicit STT-capable `MEDIA_MODEL_ID` + `MEDIA_MODEL_PROVIDER`;
  - use `LlmClient::resolve_media_model_name_for_audio_stt()` / `transcribe_audio*` through the media route;
  - dispatch the transcript as normal Agent Mode text input;
  - if STT route is absent/unsupported, reject with “voice messages require a configured media/STT provider” style message.
- Photo/video/audio-file/document inputs:
  - download into the agent sandbox/per-run upload area when media/file capability is enabled;
  - sanitize filenames and enforce Telegram/agent upload size limits;
  - pass an attachment descriptor/path and caption/task text to the agent;
  - expose/use sandbox media tools (`describe_image_file`, `describe_video_file`, `transcribe_audio_file`) so the agent can request analysis with a prompt;
  - allow eager preprocessor output only as agent context text, never as direct chat response.
- Remove media writes to chat history: no `save_message_for_chat`, no scoped chat UUID, no chat flow controls.
- Remove media route fallback to `chat_model_name` / `CHAT_MODEL_*`; media either uses explicit `MEDIA_MODEL_*` or returns unsupported.
- Do not add direct Google Gemini provider. Gemini-family media/STT is allowed only through existing OpenRouter routes unless a separate product decision introduces a direct provider.

Rationale:

- Current voice/photo/video handlers are hidden Chat Mode paths: voice transcribes then calls `process_llm_request()`, image/video analyze directly and write assistant replies into chat history.
- The repo already contains agent-side media primitives. Reusing them preserves Agent Mode semantics, lets the agent decide when to inspect media, and avoids lossy “describe immediately then answer as chat” behavior.

Acceptance Criteria:

- `handle_voice`, `handle_photo`, `handle_video`, `handle_document` never require `chat_mode` and never show mode-selection fallback.
- Voice with configured STT-capable media route becomes Agent Mode text input; voice without media route returns explicit unsupported message.
- Photo/video/audio/document files are either preserved in sandbox and made available to the agent/tool runtime, or explicitly rejected when media/file capability is disabled.
- Media tools can analyze sandbox files by prompt while enforcing sandbox path validation.
- Media paths never call `save_message_for_chat`, `ensure_scoped_chat_uuid`, `send_chat_flow_controls*`, `process_llm_request` or transport-level `chat_completion`.
- `LlmClient` media resolution does not fallback to chat model after `CHAT_MODEL_*` removal.
- Direct Gemini provider IDs remain absent; OpenRouter Gemini-family model IDs remain possible as `MEDIA_MODEL_ID` values through `llm-provider/openrouter`.

Affected Areas:

- `bot/handlers.rs:1336-1750`
- `crates/oxide-agent-transport-telegram/src/bot/agent/media.rs`
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/input.rs`
- `crates/oxide-agent-transport-telegram/src/bot/agent_handlers/task_runner.rs`
- `crates/oxide-agent-core/src/agent/preprocessor.rs`
- `crates/oxide-agent-core/src/agent/providers/media_file.rs`
- `crates/oxide-agent-core/src/llm/client.rs`
- `crates/oxide-agent-core/src/llm/capabilities.rs`
- media docs/tests/snapshots/profile checks

Edge Cases:

- Voice previously worked only in Chat Mode.
- Voice has no STT media route configured.
- Media route supports image/video but not audio STT.
- Photo/video have captions that are tasks or tool prompts.
- Video exceeds inline size limit.
- File is valid but media/file feature/profile is disabled.
- Agent has pending input that specifically requires preserving the binary file.
- Sandbox path sanitization rejects or rewrites unsafe filenames.
- OpenRouter Gemini-family model is configured for media but not for main agent tools.
- User asks for “Gemini” directly; config must use OpenRouter route, not direct `llm-provider/gemini`.
- Topic route requires mention.
- Agent access missing.

### FR-016: Update docs/env/profiles

ID: `FR-016`

Название: сделать README, env, workflows, profiles and scripts agent-only.

Описание:

- Remove Chat Mode sections, wording and examples from README/.env.
- Remove `CHAT_MODEL_*` from `.env.example`, workflows and deployment env generation.
- Remove `GROQ_API_KEY` and Groq docs.
- Update provider documentation to list only agent-compatible providers and gating caveats.
- Update `profiles/full.toml`, capability scripts and registry snapshots.
- Update runtime env surface guard to forbid removed `CHAT_MODEL_*` family.

Rationale:

- Docs/env/profile drift will reintroduce removed runtime expectations.

Acceptance Criteria:

- `rg -n "Chat Mode|chat mode|CHAT_MODEL|GROQ|Groq|llm-groq|llm-provider/groq" README.md .env.example .github profiles scripts crates/oxide-agent-core/tests/snapshots` has no obsolete references.
- Full profile no longer includes Groq.
- CI workflows do not require or write `CHAT_MODEL_*`.
- Docs explain Agent Mode-only operation.

Affected Areas:

- `.env.example`
- `README.md`
- `AGENTS.md`
- `.github/workflows/ci-cd.yml`
- `.github/workflows/modular-architecture.yml` if any checks assume removed provider
- `profiles/full.toml`
- `scripts/check-runtime-env-surface.sh`
- `scripts/check-compiled-capabilities.sh`
- snapshots

Edge Cases:

- Hidden docs in docker/config examples.
- Workflow deploy step still writes deleted env vars.
- Runtime env surface script only catches old `CHAT_MODEL_MAX_TOKENS`, not all `CHAT_MODEL_*`.

### FR-017: Update tests and snapshots

ID: `FR-017`

Название: удалить Chat Mode tests and add agent-only regressions.

Описание:

- Delete or refactor tests for Chat Mode menus, callbacks, chat UUID, chat model selection, chat prompt editing and chat history.
- Update mocks after `StorageProvider` and `LlmProvider` trait cleanup.
- Update modular registry snapshots after Groq removal.
- Add regression tests that prove text/media cannot hit chat completion path.
- Add provider gating tests for Groq removal, OpenRouter default-deny and NVIDIA model checks.

Rationale:

- Tests currently encode legacy Chat Mode behavior; without regression tests, hidden fallback can return.

Acceptance Criteria:

- No test requires `State::ChatMode` or `CHAT_MODEL_*`.
- No snapshot lists `llm-provider/groq`.
- Tests fail if Telegram transport calls `chat_completion` for user input.
- Tests fail if unknown provider gets default tool support.

Affected Areas:

- `bot/handlers.rs` tests
- `bot/context.rs` tests
- `bot/topic_route.rs` test storage mocks
- `bot/agent_handlers/tests.rs`
- `crates/oxide-agent-core/tests/*`
- `transport-web` test providers/mocks
- `modular_registry_snapshots.rs` and `.snap` files

Edge Cases:

- Mock traits become simpler after removing chat methods.
- Snapshot suffixes for profile-full/all-features.
- Tests named with “chat” but actually testing provider protocol; rename if internal-only.

### FR-018: Add final grep invariants

ID: `FR-018`

Название: добавить финальные grep-инварианты удаления.

Описание:

- Add a verification section in PRD and implementation checklist with `rg` commands proving removal.
- Add CI guard script or extend existing `scripts/check-runtime-env-surface.sh` to reject deleted chat runtime tokens.
- Decide allowed exceptions explicitly. Recommended: no exceptions for `ChatMode`, `chat_mode`, `CHAT_MODEL`, `llm-groq`, `llm-provider/groq`, `process_llm_request`.

Rationale:

- This refactor is easy to regress through docs/tests/profiles. Grep invariants are cheap, reliable and CI-friendly.

Acceptance Criteria:

- Final PR includes command outputs showing invariants pass.
- CI or local verification fails on reintroduced Chat Mode tokens.
- Any remaining `chat_completion` references are internal-only and renamed, or have documented exceptions in code review.

Affected Areas:

- PR verification checklist
- `scripts/check-runtime-env-surface.sh` or new guard script
- CI workflow if guard is added

Edge Cases:

- Provider APIs may still use upstream endpoint name `/chat/completions`; if retained, isolate in provider internals and avoid user-facing `chat_completion` symbol.
- ChatGPT provider name contains “chat”; do not ban `chatgpt`.
- Generic Telegram `chat_id` is not Chat Mode and should not be banned.

### FR-019: Remove user-facing prompt editing

ID: `FR-019`

Название: удалить Telegram prompt editing как Chat Mode surface.

Описание:

- Delete `State::EditingPrompt` from `bot/state.rs`.
- Delete `MENU_CALLBACK_EDIT_PROMPT` and `MenuCallbackData::EditPrompt` from Telegram handlers.
- Delete “Edit Prompt” menu entries and text command branches.
- Delete the prompt-editing handler that accepts the next user message and calls `storage.update_user_prompt()`.
- Remove `storage.get_user_prompt()` from user-facing prompt construction unless the call is proven to be required by Agent Mode independent of Chat Mode.
- Preserve topic/profile/global agent prompt configuration only if it is not controlled through the deleted Telegram prompt editing flow.

Rationale:

- Prompt editing is part of the old Chat Mode UX and writes per-user chat prompt state. Reusing it for Agent Mode would create a new product feature and unclear prompt precedence.

Acceptance Criteria:

- `State::EditingPrompt` no longer exists.
- Telegram UI contains no “Edit Prompt” action.
- Telegram callbacks contain no `EditPrompt` variant.
- No user text can be interpreted as “new prompt value”.
- `storage.update_user_prompt()` is not called from Telegram transport.
- Agent Mode still uses existing topic/profile/global prompt sources if they already existed independently.
- No fallback from deleted per-user prompt to Chat Mode exists.

Affected Areas:

- `crates/oxide-agent-transport-telegram/src/bot/state.rs`
- `crates/oxide-agent-transport-telegram/src/bot/handlers.rs`
- `crates/oxide-agent-core/src/storage/provider.rs`
- `crates/oxide-agent-core/src/storage/user.rs`
- `crates/oxide-agent-core/src/storage/r2_user.rs`
- `crates/oxide-agent-core/src/storage/r2_provider.rs`
- Telegram tests/mocks that implement user prompt storage

Edge Cases:

- User has an unsupported/unknown prompt-editing persisted state: treat as no valid state and route through agent-only access evaluation.
- User presses old inline button from a stale Telegram message: ignore callback or show “This action is no longer supported.”
- Existing `UserConfig.system_prompt` remains in storage data: do not migrate; stop reading it for runtime unless separately required by Agent Mode.
- Topic/profile prompt still exists: preserve it because it is not user Chat Mode prompt editing.

Grep verification:

```bash
rg -n "EditingPrompt|EditPrompt|MENU_CALLBACK_EDIT_PROMPT|Edit Prompt" crates/oxide-agent-transport-telegram
rg -n "update_user_prompt|get_user_prompt" crates/oxide-agent-transport-telegram
```

Both commands must return empty.

## 9. Non-Functional Requirements

- **Maintainability:** after removal, there should be one user runtime path: Agent Mode. Names, states and storage fields must not imply dual runtime.
- **Explicit capabilities over assumptions:** provider/model compatibility must be declared by module/model metadata or config, not inferred from provider presence.
- **Fail-fast config validation:** missing or incompatible agent route should fail at startup/config validation when possible.
- **Zero hidden fallbacks:** no fallback from Agent Mode or `/start` to Chat Mode, chat model or plain chat completion.
- **Clear error messages:** unsupported provider/model/access should explain which route or capability is missing.
- **Explicit media degradation:** missing media/STT route should produce a direct unsupported message, not a mode menu, silent ignore or fallback to text chat.
- **Sandbox media isolation:** downloaded Telegram files must stay inside agent sandbox scope with sanitized names and validated tool paths.
- **Minimal runtime ambiguity:** internal completion, if kept, must be named internal and not reachable from transport/user handlers.
- **CI-verifiable removal:** `rg` invariants and profile/snapshot checks must prove removal.
- **Agent semantics preservation:** agent memory, tool execution, confirmation, topic routing and provider failover should continue working unless explicitly changed by chat removal.
- **Alias safety:** route compatibility checks must use canonical provider IDs and aliases consistently.
- **No silent provider downgrade:** failover must never choose a route that lacks tool calling just because it can answer text.

## 10. Edge Cases

- Private Telegram chat without agent access: return access/config guidance; do not activate Chat Mode.
- Group chat/thread context: topic routing and mention policy remain, but allowed input goes to Agent Mode only.
- Existing persisted state has an unsupported/unknown value: treat it as no valid runtime state (`None`) and route through agent-only access/configuration flow. Tests should use generic invalid values, not legacy literals such as `chat_mode`.
- Old persisted chat histories in R2: leave orphaned, do not load, do not migrate, do not clear unless broader cleanup tool is separately requested.
- Old `current_chat_uuid`: ignore after schema cleanup; serde defaults should not require it.
- Missing `CHAT_MODEL_*`: should be normal; no startup error.
- Old env files containing `CHAT_MODEL_*`: variables are ignored or flagged by CI/docs; runtime does not read them.
- Missing agent model route: fail fast with clear error.
- Provider exists but model does not support tools: exclude route before execution attempt.
- Provider supports tools partially: require exact capability metadata; default deny uncertain routes.
- OpenRouter model claims compatibility but fails tool calls: quarantine/failover as runtime provider failure, and consider removing from allowlist.
- NVIDIA model support differs by model: use `model_capabilities()` before selection and keep provider-level guard.
- ChatGPT JSON/structured output restrictions: ensure canonical/alias-safe checks for `chatgpt`, `openai-chatgpt`, `llm-provider/openai-chatgpt`.
- Internal summarization currently using `chat_completion`: rename/isolate rather than delete blindly.
- Media input previously worked only in Chat Mode: voice must become STT → Agent Mode text; photo/video/audio/document must become sandbox attachment/tool input or explicit unsupported response.
- Voice without configured STT media route: reject clearly and explain that `MEDIA_MODEL_*` with audio transcription support is required.
- Photo/video/document with configured media/file capability disabled: reject clearly; do not show Chat Mode menu.
- OpenRouter Gemini-family media model configured: allowed as `MEDIA_MODEL_ID` through `llm-provider/openrouter`; direct `llm-provider/gemini` remains invalid.
- Media route supports modality but main agent route does not support tools: media route may be used only by media tools/preprocessor, not selected as agent route.
- Agent media tool prompt tries to access a path outside sandbox: reject via path resolver.
- Docs still showing deleted env vars: `rg` invariant must fail review.
- Cargo feature still compiling deleted provider: `cargo check --workspace --all-features` and `rg llm-groq` must catch it.
- Capability snapshots still listing deleted provider: update insta snapshots and script checks.
- Tests/mocks broken after storage trait cleanup: update all StorageProvider mocks in Telegram/web/core tests.
- Binary/profile scripts still expecting deleted feature: update `profiles/full.toml`, `check-compiled-capabilities.sh`, Docker/workflow feature bundles.
- User sends message before any state exists: agent-only access policy decides; no mode menu fallback.
- Confirmation flow active while removed/unknown state exists elsewhere in storage: confirmation should remain isolated to `AgentConfirmation`.
- Agent flow cancellation vs chat flow clearing: `/clear` performs only agent session reset (current flow/session/transient context) or no-op readiness guidance. It must not clear long-term memory/profile data and must not reset chat UUID.
- Provider failover selecting incompatible route: route availability must include capability check.
- Route registry containing stale provider IDs: config validation must reject `llm-provider/groq`, `groq`, or any disabled provider.
- Aliases/canonical IDs mismatch, especially ChatGPT/OpenRouter/NVIDIA: canonicalize before capability checks and docs.

## 11. Implementation Plan

### Phase 1: Recon and inventory

- Re-run targeted grep commands for Chat Mode, `CHAT_MODEL_*`, chat storage, `chat_completion`, Groq and provider route logic.
- Classify each hit as delete, keep, refactor, internal-only rename, or requires verification.
- Produce a short implementation inventory before touching code:
  - Telegram state/menu/handler paths;
  - config/env paths;
  - storage trait/impl paths;
  - provider/capability/profile paths;
  - tests/docs/snapshots;
  - internal completion users.

### Phase 2: Remove provider surface

- Remove Groq module from `providers/mod.rs`, `providers/modules.rs` and provider tests.
- Remove `llm-groq` from `Cargo.toml` feature graph and profile-full.
- Remove Groq from `capabilities/compiled.rs`, `profiles/full.toml`, `scripts/check-compiled-capabilities.sh`, registry snapshots, README, `.env.example`, workflows and integration validation.
- Verify `rg -n "Groq|GROQ|llm-groq|llm-provider/groq" .` returns no live references.

### Phase 3: Remove chat config

- Remove chat fields from `AgentSettings` and all builders/tests.
- Remove `CHAT_MODEL_*` validation, canonicalization, configured provider enumeration and fallback.
- Remove chat model spec/list/name lookup methods.
- Make missing/incompatible agent route fail fast.
- Replace media model token/window defaults with explicit media or agent defaults.
- Update env examples, workflows and config tests.

### Phase 4: Remove Telegram Chat Mode

- Remove `State::ChatMode` and runner branch.
- Remove Chat Mode button, menu callbacks, model switcher, prompt editing, extra functions and chat flow attach/detach controls.
- Refactor `/start` to agent-only.
- Refactor `handle_text()` to route user text to agent handler only.
- Refactor agent exit/cancel flows so they do not write `chat_mode`.
- Handle stale callback data with a harmless “This control is no longer supported” response or no-op, not chat activation.

### Phase 5: Remove chat storage

- Remove chat history and per-user chat model methods from `StorageProvider`.
- Remove R2 key helpers for chat history.
- Remove `current_chat_uuid` from user/context config.
- Remove `ensure_current_chat_uuid`, `reset_current_chat_uuid`, `scoped_chat_storage_id`.
- Rename `generate_chat_uuid` to generic agent flow/run ID helper if still needed.
- Update R2 storage, in-memory storage, telemetry and all mocks/tests.

### Phase 6: Refactor LLM trait

- Remove user-facing `LlmClient::chat_completion()`.
- Rename `chat_completion_for_model_info()` to crate-private `complete_internal_text` if internal tasks still need it.
- Move or restrict internal completion so Telegram and web transports cannot call it at compile time.
- Decide whether `LlmProvider` remains dual-method or splits into agent-capable provider + internal text provider.
- Update provider implementations and mocks accordingly.

### Phase 7: Provider compatibility gates

- Change unknown provider capability fallback to default-deny.
- Add route/model capability check to route availability before selection.
- Harden OpenRouter with explicit allowlist/route flag/metadata.
- Keep NVIDIA model-level check and move it before route selection.
- Preserve ChatGPT as agent provider and fix alias/canonical JSON-mode checks.
- Add regression tests for incompatible provider rejection and failover.

### Phase 8: Media handling

- Route Telegram voice/photo/video/document handlers through agent access/topic policy and agent input extraction; remove Chat Mode checks from those handlers.
- Voice: download payload, require explicit STT-capable `MEDIA_MODEL_*`, transcribe through media route, dispatch transcript as Agent Mode text. If route is absent, return explicit unsupported response.
- Photo/video/audio/document: download into agent sandbox/per-run upload area when media/file feature is enabled; pass attachment descriptor and caption/task text to agent.
- Prefer tool-first media analysis using `MediaFileProvider` tools (`describe_image_file`, `describe_video_file`, `transcribe_audio_file`); keep eager preprocessor only as agent-context generation, not direct reply.
- Remove media chat history writes, scoped chat UUID use and chat flow controls from media handlers.
- Verify media route resolution has no chat model fallback and no direct Gemini provider is introduced.

### Phase 9: Docs/tests/snapshots

- Update README, `.env.example`, workflows, profiles, scripts and examples.
- Update `scripts/check-runtime-env-surface.sh` or add new guard for removed Chat Mode env/runtime tokens.
- Update modular registry snapshots with `INSTA_UPDATE=always` only after code changes are correct.
- Add regression tests:
  - `/start` never writes `chat_mode`;
  - text input calls agent handler, not chat completion;
  - stale chat callbacks cannot activate Chat Mode;
  - missing `CHAT_MODEL_*` is accepted;
  - Groq provider is absent;
  - OpenRouter unverified model route is rejected;
  - NVIDIA unsupported model is skipped before provider call.

### Phase 10: Final verification

- Run formatting, check and tests.
- Run profile/capability/snapshot scripts.
- Run final grep invariants from section 13.
- Attach command outputs to implementation PR.

## 12. Acceptance Criteria

- Repository builds without Chat Mode.
- No `State::ChatMode` variant exists.
- No `chat_mode` persisted state activates runtime.
- `/start` is agent-only and never sets chat state.
- No `CHAT_MODEL_*` config fields, env requirements, docs examples or workflows remain.
- No per-user chat model selection exists.
- No chat history storage APIs exist in `StorageProvider`.
- No `current_chat_uuid` exists in user/context config.
- No user-facing `chat_completion` path exists.
- `process_llm_request` is deleted.
- Internal plain text completion, if still needed, is renamed/internal-only and inaccessible from transport.
- Groq provider is removed if classified incompatible; current recon classifies it as incompatible.
- `llm-groq`, `llm-provider/groq`, `GROQ_API_KEY` and Groq snapshots/profiles/docs are removed.
- OpenRouter requires the static in-code model allowlist from DR-003 and matching approval/capability flags.
- NVIDIA NIM routes are checked by exact-match code-owned allowlist before selection/execution.
- Unsupported providers/routes are skipped or rejected before execution attempt.
- ChatGPT provider remains available as agent-compatible provider when configured.
- Voice/photo/video/document flows do not fallback to Chat Mode.
- Voice requires explicit STT-capable media route and becomes Agent Mode text input; missing route returns explicit unsupported message.
- Photo/video/audio/document inputs are sandbox attachments/tool inputs or explicit unsupported responses; they are never direct chat replies.
- Media route resolution uses `MEDIA_MODEL_*` and modality capabilities only; it does not fallback to removed chat model config.
- Direct Gemini provider IDs remain absent; Gemini-family media use goes through OpenRouter model IDs if configured.
- README, `.env.example`, workflows, scripts and profiles describe agent-only operation.
- Tests cover agent-only routing, removed chat path and provider gating.
- Final grep commands pass with zero live legacy hits, except explicitly documented internal endpoint naming if unavoidable.

## 13. Verification Commands

Run from repository root.

```bash
# Common rg excludes for repo-wide invariants.
COMMON_GLOBS=(
  --glob '!target'
  --glob '!Cargo.lock'
  --glob '!PRD*.md'
  --glob '!*.patch'
  --glob '!docs/decisions/*chat*mode*'
)

# Hard-zero invariants: no live runtime/docs/profile references.
rg -n "Chat Mode|chat mode|chat_mode|ChatMode|State::ChatMode|persisted chat_mode|CHAT_MODEL_|llm-groq|llm-provider/groq" . "${COMMON_GLOBS[@]}"

# Transport/user layers must not call plain/internal completion APIs.
rg -n "chat_completion|chat_completion_for_model_info|complete_internal_text|process_llm_request" \
  crates/oxide-agent-transport-telegram/src \
  crates/oxide-agent-transport-web/src

# Provider-internal endpoint terminology requires manual review only.
# Allowed examples may include upstream `/chat/completions` endpoint names or SDK helper names inside provider internals.
rg -n "chat_completion|/chat/completions" . "${COMMON_GLOBS[@]}"

# Chat storage invariant: should be empty in live code.
rg -n "get_chat_history|save_message_for_chat|clear_chat_history|current_chat_uuid|user_prompt|user_model" . "${COMMON_GLOBS[@]}"

# Prompt editing invariant: should be empty in Telegram transport.
rg -n "EditingPrompt|EditPrompt|MENU_CALLBACK_EDIT_PROMPT|Edit Prompt" crates/oxide-agent-transport-telegram
rg -n "update_user_prompt|get_user_prompt|pick_system_prompt|resolve_system_prompt|SYSTEM_MESSAGE" crates/oxide-agent-transport-telegram

# Provider features/profiles must not contain Groq.
rg -n "llm-groq|llm-provider/groq|GROQ_API_KEY" Cargo.toml crates profiles scripts .github .env.example README.md AGENTS.md

# Media/modality invariant: Telegram media handlers must not use Chat Mode storage/controls.
rg -n "chat_mode|save_message_for_chat|ensure_scoped_chat_uuid|send_chat_flow_controls|process_llm_request" \
  crates/oxide-agent-transport-telegram/src/bot/handlers.rs \
  crates/oxide-agent-transport-telegram/src/bot/agent \
  crates/oxide-agent-transport-telegram/src/bot/agent_handlers

# Media route/tool surface should remain agent-owned.
rg -n "resolve_media_model|MEDIA_MODEL|transcribe_audio_file|describe_image_file|describe_video_file|MediaFileProvider|Preprocessor" \
  crates/oxide-agent-core/src \
  crates/oxide-agent-transport-telegram/src

# Direct Gemini provider remains absent; Gemini-family model IDs are allowed only as OpenRouter model IDs.
rg -n "llm-provider/gemini|llm-provider/google-gemini|GOOGLE_GEMINI_API_KEY" \
  crates Cargo.toml profiles scripts .github .env.example README.md AGENTS.md

# NVIDIA allowlist verification: should show only exact allowed model ids and no wildcard/config override path.
rg -n "deepseek-ai/deepseek-v4-pro|deepseek-ai/deepseek-v4-flash" crates/oxide-agent-core/src
rg -n "nvidia.*wildcard|wildcard.*nvidia|supports_tools.*nvidia|CHAT_MODEL.*nvidia|nvidia.*CHAT_MODEL" crates/oxide-agent-core/src

# Unknown/invalid persisted-state fallback must not introduce legacy-state compatibility symbols.
rg -n "legacy read-path compatibility|normalize_chat_mode|LegacyChatMode|UnknownChatMode" crates/oxide-agent-core/src crates/oxide-agent-transport-telegram/src

# Targeted provider/media tests after implementation.
cargo test --workspace --all-features nvidia
cargo test --workspace --all-features provider
cargo test --workspace --all-features route
cargo test -p oxide-agent-core preprocessor --all-features
cargo test -p oxide-agent-core media_file --all-features

# Build and tests.
cargo fmt --check
cargo check --workspace --all-features
cargo test --workspace --all-features

# Profile and capability checks present in this repo.
scripts/check-runtime-env-surface.sh
scripts/check-compiled-capabilities.sh embedded-opencode-local
scripts/check-compiled-capabilities.sh lite
scripts/check-compiled-capabilities.sh search-only
scripts/check-compiled-capabilities.sh no-sandbox
scripts/check-compiled-capabilities.sh media-enabled
scripts/check-compiled-capabilities.sh full
scripts/check-registry-snapshots.sh embedded-opencode-local
scripts/check-registry-snapshots.sh lite
scripts/check-registry-snapshots.sh search-only
scripts/check-registry-snapshots.sh no-sandbox
scripts/check-registry-snapshots.sh media-enabled
scripts/check-registry-snapshots.sh full

# Optional profile compile checks referenced by AGENTS.md.
cargo check --workspace --no-default-features --features profile-embedded-opencode-local
cargo check --workspace --no-default-features --features profile-lite
cargo check --workspace --no-default-features --features profile-search-only
cargo check --workspace --no-default-features --features profile-no-sandbox
cargo check --workspace --no-default-features --features profile-media-enabled
cargo check --workspace --no-default-features --features profile-host-bwrap
cargo check --workspace --no-default-features --features profile-full
```

If internal provider implementations still need to call upstream `/chat/completions`, do not whitelist the old public project-level symbol casually. Rename project-level APIs first; then document any remaining provider-internal endpoint helper as an allowed exception in code review.

Tests for invalid persisted state must use generic invalid values, not legacy literals such as `chat_mode` or `EditingPrompt`, so hard-zero grep remains meaningful.

## 14. Risks

### Risk: accidental deletion of internal summarization path

Mitigation:

- Audit all `chat_completion_for_model_info()` uses before deletion.
- Rename internal text completion API and keep compaction/loop detection/wiki writer/input classifier tests.
- Add visibility boundaries preventing transport calls.

### Risk: media UX regression

Mitigation:

- Use the explicit contract: voice → STT media route → Agent Mode text; photo/video/audio/document → sandbox attachment + media tool/preprocessor.
- Add tests for voice with/without STT route, photo/video/document with/without media feature, and captions as agent tasks.
- Preserve existing agent-side media primitives instead of direct Chat Mode handlers.
- Do not preserve UX by falling back to Chat Mode or direct chat completion.

### Risk: provider false positives

Mitigation:

- Default-deny unknown providers/models.
- Require `supports_tool_calling=true` at route/model level.
- Add tests for structured-output-only provider not being selected for agent tools.

### Risk: OpenRouter ambiguity

Mitigation:

- Add explicit OpenRouter model/route allowlist or capability flag.
- Treat all unlisted OpenRouter models as incompatible.
- Add runtime quarantine/failover for models that pass metadata but fail tool calls.

### Risk: NVIDIA model variance

Mitigation:

- Keep `model_capabilities()` and use it before route selection.
- Add tests for known good and known bad NVIDIA model IDs.
- Keep provider-level guard as defense-in-depth.

### Risk: stale docs/env/profile scripts

Mitigation:

- Run final `rg` invariants over docs, workflows, profiles and scripts.
- Extend `check-runtime-env-surface.sh` to reject removed `CHAT_MODEL_*` and Groq.
- Update registry snapshots only after scripts pass.

### Risk: storage trait mock breakage

Mitigation:

- Remove chat methods from trait and update all mocks in one phase.
- Prefer smaller trait surface to keeping no-op chat methods.
- Run workspace tests after storage cleanup.

### Risk: ChatGPT provider accidentally removed because of name confusion

Mitigation:

- Keep separate requirement `FR-014`.
- Add tests that ChatGPT provider aliases resolve and can be selected when compatible.
- Docs should say ChatGPT provider is not Chat Mode.

### Risk: hidden fallback through generic chat completion

Mitigation:

- Delete `process_llm_request()`.
- Remove public `LlmClient::chat_completion()` and replace allowed auxiliary use with crate-private `complete_internal_text`.
- Add transport-level grep/test ensuring Telegram handlers cannot call completion directly.

### Risk: unknown persisted state resurrects legacy behavior

Mitigation:

- Treat unsupported/unknown persisted state as `None` and route it through agent-only access/configuration handling.
- Remove `State::ChatMode` as a runtime variant; no code path should recognize legacy persisted-state literals as active runtime.
- Storage must not have chat-specific execution branches.
- No `LegacyChatMode`, `UnknownChatMode` or `normalize_chat_mode_state` runtime symbols.
- Tests for invalid persisted state must use generic invalid values, not legacy literals such as `chat_mode` or `EditingPrompt`, so hard-zero grep stays meaningful.

### Risk: failover selects incompatible route

Mitigation:

- Put capability check into `route_is_available()` / selection stage.
- Use canonical provider ID for capability lookup.
- Add tests for primary incompatible, backup compatible; primary compatible, backup incompatible; all incompatible.

### Risk: broad refactor breaks Agent Mode semantics

Mitigation:

- Scope changes to chat removal, provider gating and internal completion isolation.
- Preserve Agent memory, tools, topic routing and confirmation flows.
- Use phased tests after each layer.

## 15. Out of Scope

- Data migrations for old chat histories.
- Preserving old chat histories.
- Supporting old `CHAT_MODEL_*` env vars.
- Soft deprecation of Chat Mode.
- Dual-mode runtime.
- New provider integrations unless required to replace a removed provider.
- Broad architecture rewrite unrelated to Chat Mode removal.
- User-facing model switching UX for agent routes.
- Cleanup of orphaned R2 chat objects.
- Changing topic routing semantics except to remove chat fallback.

## 16. Required Output

Implementation PR must produce an agent-only codebase where an engineer can verify:

- what was removed: Chat Mode state/menu/callbacks/runtime, `CHAT_MODEL_*`, chat storage, Groq, chat-only provider routes;
- why it was removed: to eliminate second runtime path and unsafe non-tool providers from user flow;
- where it was removed: Telegram state/handlers/context, core config/storage/LLM/provider registry, docs/env/workflows/profiles/scripts/tests/snapshots;
- how Agent Mode remains safe: only agent-compatible routes, explicit provider/model capabilities, no hidden plain completion fallback;
- which edge cases were tested: stale persisted state, stale callbacks, missing agent access, media input, provider gating, OpenRouter/NVIDIA model checks;
- how completion is proven: build/test/profile checks and final grep invariants.

Target artifact filename for this planning work: `PRD_удаление_chat_mode.md`.
