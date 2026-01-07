# Улучшение детекции петель агента

## Цель

Устранить ложные срабатывания `ToolCallLoop` при последовательном выполнении разных shell-команд через `execute_command`, сохранив способность детектировать реальные петли.

## User Review Required

> [!IMPORTANT]
> Этот план предполагает изменения в критической подсистеме агента. Рекомендуется поэтапное внедрение с тестированием после каждой фазы.

---

## Фаза 1: Расширенное логирование (Диагностика) [READY]

**Цель:** Определить, какой именно детектор вызывает ложные срабатывания.

### [MODIFY] [tool_detector.rs](file:///home/stfu/ai/Another-Chat-with-LLM/src/agent/loop_detection/tool_detector.rs)

- Добавить `tracing::debug!` в метод `check()` с выводом:
  - `tool_name`
  - Первые 100 символов `args` (preview)
  - Текущий и предыдущий хеш
  - Счётчик повторений `repetition_count`
- Логировать момент срабатывания threshold

### [MODIFY] [service.rs](file:///home/stfu/ai/Another-Chat-with-LLM/src/agent/loop_detection/service.rs)

- В `check_tool_call()` добавить лог какой тип петли сработал
- Логировать состояние всех детекторов перед возвратом `true`
- Добавить трейс в `check_content()` и `check_llm_periodic()`

### [MODIFY] [content_detector.rs](file:///home/stfu/ai/Another-Chat-with-LLM/src/agent/loop_detection/content_detector.rs)

- Добавить аналогичное debug-логирование
- Логировать размер chunk'ов и совпадения

---

## Фаза 2: Sliding Window Pattern Detection

**Цель:** Заменить простой последовательный счётчик на window-based анализ паттернов.

### [MODIFY] [tool_detector.rs](file:///home/stfu/ai/Another-Chat-with-LLM/src/agent/loop_detection/tool_detector.rs)

Полная переработка структуры `ToolCallDetector`:

| Было | Станет |
|------|--------|
| `last_key: Option<String>` | `history: VecDeque<String>` |
| `repetition_count: usize` | `window_size: usize` |
| Простой counter | Pattern matching алгоритм |

**Новая логика:**
1. Хранить последние N хешей (configurable, default = 10)
2. Детектировать паттерны:
   - **Прямое повторение:** `A → A → A → A` (текущее поведение)
   - **Чередование:** `A → B → A → B → A → B`
   - **Циклы:** `A → B → C → A → B → C`
3. Не считать петлёй: `A → B → C → D → E` (разные вызовы)

### [MODIFY] [config.rs](file:///home/stfu/ai/Another-Chat-with-LLM/src/agent/loop_detection/config.rs)

Добавить новые параметры:
- `tool_window_size` (default: 10)
- `tool_pattern_min_repeats` (default: 3)
- `tool_max_pattern_length` (default: 4)

### [NEW] [pattern_matcher.rs](file:///home/stfu/ai/Another-Chat-with-LLM/src/agent/loop_detection/pattern_matcher.rs)

Новый модуль для алгоритмов поиска паттернов:
- Функция поиска повторяющихся подпоследовательностей
- Эффективный алгоритм (rolling hash или suffix matching)

### [MODIFY] [mod.rs](file:///home/stfu/ai/Another-Chat-with-LLM/src/agent/loop_detection/mod.rs)

- Добавить экспорт `pattern_matcher`

---

## Фаза 3: Улучшение LLM Scout

**Цель:** Сделать LLM-детектор основным арбитром с лучшим контекстом.

### [MODIFY] [llm_detector.rs](file:///home/stfu/ai/Another-Chat-with-LLM/src/agent/loop_detection/llm_detector.rs)

**Изменения в промпте:**
- Переписать `SYSTEM_PROMPT` с явным указанием:
  - `execute_command` с разными программами — это НЕ петля
  - Признаки прогресса: разные файлы, последовательные этапы pipeline
  - Признаки петли: одинаковые команды, отсутствие изменений в output

**Изменения в подготовке контекста:**
- В `prepare_history()` добавить summary последних tool calls
- Группировать `execute_command` вызовы по программам (ffmpeg: 3, mv: 2, etc.)
- Передавать информацию о прогрессе (если доступна)

**Изменения в интервалах:**
- Вместо фиксированного интервала — адаптивная логика

### [MODIFY] [config.rs](file:///home/stfu/ai/Another-Chat-with-LLM/src/agent/loop_detection/config.rs)

Обновить defaults для LLM детектора:
- `llm_check_after_turns`: 15 (было 30)
- `llm_check_interval`: 5 (было 3)
- `llm_confidence_threshold`: 0.85 (было 0.9)
- `llm_history_count`: 30 (было 20)

### [MODIFY] [types.rs](file:///home/stfu/ai/Another-Chat-with-LLM/src/agent/loop_detection/types.rs)

- Добавить новый `LoopType::PatternLoop` для sliding window детекции
- Расширить `LoopDetectedEvent` полем `pattern_description: Option<String>`

---

## Verification Plan

### Automated Tests

1. Добавить unit-тесты в `tool_detector.rs`:
   - Тест на `ffmpeg → mv → cp` (не должен детектироваться как петля)
   - Тест на `A → B → A → B → A → B` (должен детектироваться)
   - Тест на `A → A → A → A → A` (должен детектироваться)

2. Интеграционные тесты:
   - `cargo test --package another_chat_rs --lib loop_detection`

### Manual Verification

1. Воспроизвести сценарий из bug report:
   - Задача на конвертацию видео (ytdl → ffmpeg → upload)
   - Проверить отсутствие ложных срабатываний на 10-15 итерациях

2. Проверить реальные петли:
   - Агент пытается одну и ту же команду 5+ раз
   - Должен корректно детектироваться

---

## Порядок внедрения

```mermaid
graph LR
    A["Фаза 1<br/>Логирование"] --> B["Анализ логов<br/>+ выводы"]
    B --> C["Фаза 2<br/>Sliding Window"]
    C --> D["Тестирование"]
    D --> E["Фаза 3<br/>LLM Scout"]
    E --> F["Финальное<br/>тестирование"]
```
