# Agent Mode Compaction

Текущая реализация staged compaction pipeline для Agent Mode. Документ описывает именно код в репозитории, а не ранние design notes из `docs/compaction/`.

## Scope

- Работает только для Agent Mode.
- Chat mode этой логикой не пользуется.
- Старый локальный auto-compact из `memory.rs` удален.

## Где находится код

- `crates/oxide-agent-core/src/agent/compaction/mod.rs`
- `crates/oxide-agent-core/src/agent/compaction/types.rs`
- `crates/oxide-agent-core/src/agent/compaction/budget.rs`
- `crates/oxide-agent-core/src/agent/compaction/classifier.rs`
- `crates/oxide-agent-core/src/agent/compaction/externalize.rs`
- `crates/oxide-agent-core/src/agent/compaction/prune.rs`
- `crates/oxide-agent-core/src/agent/compaction/prompt.rs`
- `crates/oxide-agent-core/src/agent/compaction/summarizer.rs`
- `crates/oxide-agent-core/src/agent/compaction/rebuild.rs`
- `crates/oxide-agent-core/src/agent/compaction/archive.rs`
- `crates/oxide-agent-core/src/agent/compaction/service.rs`
- `crates/oxide-agent-core/src/agent/runner/execution.rs`
- `crates/oxide-agent-core/src/agent/executor.rs`

## Архитектура

Pipeline запускается orchestration layer через `CompactionService`, а не памятью напрямую.

Порядок стадий:

1. `budget.rs` — оценивает полный budget запроса
2. `classifier.rs` — делит hot memory на pinned / protected / prunable / compactable
3. `externalize.rs` — выносит крупные tool outputs в artifact-style placeholder
4. `prune.rs` — схлопывает старые тяжелые артефакты вне recent raw window
5. `summarizer.rs` — вызывает отдельную compaction model для старой истории
6. `archive.rs` — создает archive refs для вытесненных history chunks
7. `rebuild.rs` — пересобирает hot context в безопасном порядке

## Что считается при budget check

`estimate_request_budget()` считает не только память, а весь projected request:

- system prompt
- tool schemas/descriptions
- hot memory
- loaded skills
- reserve под ответ модели
- hard safety reserve

Состояния budget:

- `Healthy`
- `Warning`
- `ShouldPrune`
- `ShouldCompact`
- `OverLimit`

При `AGENT_MAX_TOKENS = 200000` дефолтные пороги сейчас такие:

- `Warning` = `65%` = `130000`
- `ShouldPrune` = `75%` = `150000`
- `ShouldCompact` = `85%` = `170000`
- `OverLimit` = `95%` = `190000`

Важно: решение принимается по `projected_total_tokens`, а не только по сырому размеру памяти.

## Что защищено от compaction

Классификатор никогда не должен терять:

- base system context
- topic `AGENTS.md`
- current task
- active todos
- runtime injections
- approval replay messages
- recent raw user/assistant turns
- недавний tool working set

## Что облегчается первым

Сначала pipeline работает с тяжелыми артефактами, а не со смыслом задачи:

- длинные `stdout` / `stderr`
- большие file contents
- search/web extracts
- bulky JSON
- старые tool results

После externalize/prune в hot context остается короткий след:

- preview
- размер
- имя tool
- artifact ref / storage key

## Что делает summary stage

Если budget дошел до `ShouldCompact` или `OverLimit`, старая compactable history передается sidecar-модели.

Модель возвращает structured summary со схемой:

- `goal`
- `constraints`
- `decisions`
- `discoveries`
- `relevant_files_entities`
- `remaining_work`
- `risks`

Если модель недоступна, timeout'ится или вернула плохой JSON, используется deterministic fallback summary.

## Что остается после rebuild

Итоговый hot context собирается так:

1. pinned context
2. protected live context
3. structured summary старой истории
4. optional archive reference
5. recent raw turns
6. recent raw tool context

Это позволяет агенту продолжить задачу без потери цели и текущего working set.

## Где pipeline запускается

- `PreRun` — перед первым вызовом модели
- `PreIteration` — перед следующими итерациями runner loop
- manual compact action — отдельная команда в Agent Mode UI
- overflow retry path — если модель вернула ошибку переполнения контекста

## Transport / UX

В Agent Mode transport показывает lifecycle через progress events:

- `CompactionStarted`
- `PruningApplied`
- `CompactionCompleted`
- `CompactionFailed`
- `RepeatedCompactionWarning`

В Telegram Agent Mode есть ручное действие `Compact Context`.

## Конфигурация

Отдельная compaction model настраивается через `.env`:

```env
COMPACTION_MODEL_ID="glm-4.7"
COMPACTION_MODEL_PROVIDER="zai"
COMPACTION_MODEL_MAX_TOKENS=1024
COMPACTION_MODEL_TIMEOUT_SECS=300
```

Что означает `COMPACTION_MODEL_MAX_TOKENS`:

- это максимум токенов ответа sidecar-модели
- это не размер всего контекста агента
- слишком большое значение делает summary тяжелым и частично съедает выигрыш от compaction

Практически разумный диапазон:

- `512` для короткого summary
- `1024-2048` если нужна более детальная сводка

## Пример 1: длинные CLI outputs

До compaction:

```text
user: проведи аудит linux sandbox
assistant: план аудита
tool: execute_command ls -R /usr/bin
tool result: 3000+ lines
tool: execute_command dpkg -l
tool result: 2000+ lines
assistant: промежуточные выводы
user: отдельно проверь python/node/cargo
tool: execute_command python --version && node --version && cargo --version
tool result: versions
```

После externalize/prune:

```text
tool result: [externalized tool result]
tool=execute_command
size_chars=48000
preview="/usr/bin/apt\n/usr/bin/bash\n/usr/bin/cargo\n..."
artifact_ref=...
```

После summary + rebuild:

```text
system prompt
topic AGENTS.md
current task
todos
structured summary of early audit
recent raw user turn
recent raw tool result with versions
```

## Пример 2: что агент сохраняет по смыслу

Если старая история была большой, summary обычно хранит такие вещи:

```text
Goal:
Audit Linux sandbox and report tooling inconsistencies.

Decisions:
- Inspect /usr/bin inventory
- Read Debian package metadata
- Compare python/node/cargo separately

Discoveries:
- Environment is Debian-based
- Python, Node and Cargo are present
- Large package inventory was already collected

Remaining Work:
- Verify suspicious version mismatches
- Produce final concise report
```

То есть raw dumps исчезают, но смысл и план работы остаются.

## Ограничения текущей реализации

- Retrieval из архива не реализован
- Архивный semantic search не реализован
- Автоматическая подгрузка archive context обратно в hot context не реализована
- Chat mode в scope не входит
