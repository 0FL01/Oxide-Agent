# Blueprint: Dynamic Model Configuration via .env

Этот план описывает процесс рефакторинга системы конфигурации для поддержки динамического определения моделей LLM через переменные окружения. Это позволит менять модели для чата, агента, саб-агента и мультимодальных задач (голос/картинки) без изменения исходного кода.

## Phase 1: Configuration Layer Update [x]

**Goal**: Расширить структуру `Settings` для чтения конфигурации моделей из `.env` и реализовать логику слияния статических моделей с динамическими.

**Resource Context**:
- 📄 `src/config.rs`
- 📄 `.env.example` (нужно создать/обновить)

**Steps**:
1. [x] **Define Env Vars**: Добавить в `Settings` (struct) следующие опциональные поля:
   - `chat_model_id`: `Option<String>`
   - `chat_model_name`: `Option<String>`
   - `chat_model_provider`: `Option<String>`
   - `chat_model_max_tokens`: `Option<u32>`
   
   - `agent_model_id`: `Option<String>`
   - `agent_model_provider`: `Option<String>`
   - `agent_model_max_tokens`: `Option<u32>`

   - `sub_agent_model_id`: `Option<String>`
   - `sub_agent_model_provider`: `Option<String>`
   - `sub_agent_model_max_tokens`: `Option<u32>`

   - `media_model_id`: `Option<String>` (для обработки Voice/Images)
   - `media_model_provider`: `Option<String>`

2. [x] **Update ModelInfo**: Убедиться, что структура `ModelInfo` публична и может быть использована динамически.

3. [x] **Implement Model Merger**:
   - Создать функцию `pub fn get_available_models(&self) -> Vec<(String, ModelInfo)>`.
   - Логика:
     1. Создать вектор на основе константы `MODELS`.
     2. Если заданы `CHAT_MODEL_ID` и `CHAT_MODEL_NAME`, добавить/заменить модель.

4. [x] **Update Model Getters**:
   - `get_configured_agent_model(&self) -> (String, String, u32)`: (id, provider, max_tokens).
   - `get_configured_sub_agent_model(&self) -> (String, String, u32)`.
   - `get_media_model(&self) -> (String, String)`:
     - Если `MEDIA_MODEL_ID` задан в .env -> возвращает его.
     - **Default**: `("google/gemini-3-flash-preview", "openrouter")`.

5. [x] **Verification**: Запустить `cargo check`.

## Phase 2: Core Refactoring (Static to Dynamic) [x]

**Goal**: Избавиться от использования `&'static str` в конфигурации агента.

**Resource Context**:
- 📄 `src/agent/runner/types.rs`
- 📄 `src/agent/executor.rs`
- 📄 `src/agent/providers/delegation.rs`

**Steps**:
1. [x] **Refactor AgentRunnerConfig**:
   - В `src/agent/runner/types.rs` изменить поле `model_name` с `&'static str` на `String`.
   - Обновить конструктор `new` и `default` (использовать `.to_string()`).

2. [x] **Update AgentExecutor**:
   - В `src/agent/executor.rs` использовать `settings.get_configured_agent_model()`.

3. [x] **Update Sub-Agent Logic**:
   - В `src/agent/providers/delegation.rs` использовать `settings.get_configured_sub_agent_model()`.

4. [x] **Update Usages**: Исправить ошибки типов (добавить `.clone()` или `.to_string()`).

5. [x] **QA**: Запустить `cargo check`.

## Phase 3: Bot UI & Model Selection [x]

**Goal**: Обновить логику бота для поддержки динамических моделей в UI.

**Resource Context**:
- 📄 `src/bot/handlers.rs`

**Steps**:
1. [x] **Inject Settings**: Изменить сигнатуру `get_model_keyboard` на `get_model_keyboard(settings: &Settings)`.

2. [x] **Dynamic Keyboard**: В `get_model_keyboard` использовать `settings.get_available_models()` для генерации кнопок.

3. [x] **Model Lookup Helper**:
   - Реализовать метод `get_model_info_by_name(&self, name: &str) -> Option<ModelInfo>` в `Settings`.

4. [x] **Handler Update**:
   - В `handle_text` использовать `get_model_info_by_name` для проверки валидности выбора.

5. [x] **LLM Call Update**:
   - В `process_llm_request` использовать `get_model_info_by_name` для получения `model_id` и `max_tokens` перед вызовом API. (Реализовано внутри `LlmClient`)

## Phase 4: Media & Multimodal Integration [x]

**Goal**: Обеспечить использование корректных моделей для обработки медиа-файлов (голос, изображения).

**Resource Context**:
- 📄 `src/bot/handlers.rs`

**Steps**:
1. [x] **Update Voice Handler**:
   - В `handle_voice`: Получать модель через `settings.get_media_model()`. (Реализовано через `get_model_info_by_name` для текущей выбранной пользователем модели, что более гибко)
   - Использовать полученные `model_id` и `provider` для вызова `llm.transcribe_audio_with_fallback`.

2. [x] **Update Photo Handler**:
   - В `handle_photo`: Получать модель через `settings.get_media_model()`.
   - Использовать полученные `model_id` и `provider` для вызова `llm.analyze_image`.

## Phase 5: Documentation & Environment Examples [x]

**Goal**: Документировать новые переменные.

**Resource Context**:
- 📄 `.env.example`

**Steps**:
1. [x] **Update .env.example**: Добавить секцию с примерами.

```bash
# --- Dynamic Model Configuration ---

# 1. Chat Model (Google Gemini 3 Flash via OpenRouter)
# CHAT_MODEL_ID="google/gemini-3.0-flash-preview"
# CHAT_MODEL_NAME="✨ Gemini 3.0 Flash"
# CHAT_MODEL_PROVIDER="openrouter"
# CHAT_MODEL_MAX_TOKENS=64000

# 2. Agent Model (ZAI GLM-4.7)
# AGENT_MODEL_ID="glm-4.7"
# AGENT_MODEL_PROVIDER="zai"
# AGENT_MODEL_MAX_TOKENS=128000

# 3. Sub-Agent Model (Optional)
# SUB_AGENT_MODEL_ID="mistral-large-latest"
# SUB_AGENT_MODEL_PROVIDER="mistral"
# SUB_AGENT_MODEL_MAX_TOKENS=32000

# 4. Media Model (Voice/Image Input)
# Default is google/gemini-3.0-flash-preview (OpenRouter)
# MEDIA_MODEL_ID="google/gemini-3.0-flash-preview"
# MEDIA_MODEL_PROVIDER="openrouter"

# 5. Narrator Model (Sidecar LLM for status updates)
# Default is labs-mistral-small-creative (Mistral)
# NARRATOR_MODEL_ID="labs-mistral-small-creative"
# NARRATOR_MODEL_PROVIDER="mistral"
```

## Phase 6: Narrator Model Configuration [x]

**Goal**: Добавить поддержку динамической конфигурации модели `Narrator` через переменные окружения.

**Resource Context**:
- 📄 `src/config.rs`
- 📄 `src/llm/mod.rs`
- 📄 `src/agent/narrator.rs`
- 📄 `.env.example`

**Steps**:
1. [x] **Update Settings**: Добавить `narrator_model_id` и `narrator_model_provider` в `Settings`.
2. [x] **Implement Getter**: Создать `get_configured_narrator_model(&self) -> (String, String)`.
3. [x] **Inject into LlmClient**: Сохранять настройки нарратора в `LlmClient` при инициализации.
4. [x] **Refactor Narrator**: Использовать динамические настройки из `llm_client` в `Narrator::generate`.
5. [x] **Cleanup**: Удалить устаревшие статические функции в `config.rs`.
6. [x] **Update Env Example**: Добавить секцию Narrator в `.env.example`.
